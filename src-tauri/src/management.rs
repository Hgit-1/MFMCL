use download_engine::{DownloadEngine, DownloadProgress};
use launcher_core::check_client_update as client_update_status_for;
use launcher_core::{
    ClientUpdateStatus, CreateInstanceInput, InstanceRecord, InstanceState, LauncherStore,
    loaders::{self, LoaderKind, LoaderVersion},
    mod_updates::{self, ModUpdate, ModUpdateReport},
    modpacks::{self, PackResolution},
    mods::{self, ModFile},
};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tauri::{AppHandle, Emitter, Manager, State};

pub(crate) fn task_progress(
    id: &str,
    app: &AppHandle,
    store: &LauncherStore,
    downloads: &Arc<crate::downloads::Downloads>,
) -> (
    tokio::sync::mpsc::UnboundedSender<DownloadProgress>,
    tauri::async_runtime::JoinHandle<()>,
) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<DownloadProgress>();
    let id = id.to_owned();
    let app = app.clone();
    let store = store.clone();
    let downloads = downloads.clone();
    let task = tauri::async_runtime::spawn(async move {
        while let Some(progress) = rx.recv().await {
            let task_id = downloads.progress(&id, &progress, &store, &app);
            /*
             * 事件带上任务归属。`progress` 在上一行已被借用，这里克隆一份后
             * 连同 `task_id` 一起发出——两者必须成对，否则界面无法把文件行
             * 挂到正确的任务卡片下。
             */
            let event = crate::downloads::ProgressEvent {
                task_id: task_id.unwrap_or_default(),
                instance_id: id.clone(),
                progress,
            };
            let _ = app.emit("download-progress", event);
        }
        let _ = downloads.flush(&id, None, "文件进度通道关闭", &store, &app);
    });
    (tx, task)
}

#[derive(Default)]
pub struct Operations(pub Arc<Mutex<HashSet<String>>>, Mutex<HashSet<PathBuf>>);
pub struct Guard {
    key: String,
    registry: Arc<Mutex<HashSet<String>>>,
}
impl Operations {
    pub fn acquire(&self, key: &str) -> Result<Guard, String> {
        let mut registry = self.0.lock().map_err(|_| "操作状态不可用。")?;
        if !registry.insert(key.to_owned()) {
            return Err("此实例有安装、游戏或管理任务正在运行。".into());
        }
        Ok(Guard {
            key: key.to_owned(),
            registry: self.0.clone(),
        })
    }
}
impl Drop for Operations {
    fn drop(&mut self) {
        if let Ok(cached) = self.1.get_mut() {
            for path in cached.iter() {
                if let Some(directory) = path.parent() {
                    let _ = std::fs::remove_dir_all(directory);
                }
            }
        }
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        if let Ok(mut registry) = self.registry.lock() {
            registry.remove(&self.key);
        }
    }
}

pub(crate) fn instance(store: &LauncherStore, id: &str) -> Result<InstanceRecord, String> {
    store
        .list_instances()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|i| i.id == id)
        .ok_or("找不到实例。".into())
}
pub(crate) fn engine(store: &LauncherStore) -> Result<DownloadEngine, String> {
    let settings = store.load_settings().map_err(|e| e.to_string())?;
    let mut download_settings = settings.download.to_engine_settings();
    download_settings.aria2_enabled = false;
    DownloadEngine::new(download_settings).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn loader_versions(
    kind: LoaderKind,
    game_version: String,
    app: AppHandle,
    store: State<'_, LauncherStore>,
) -> Result<Vec<LoaderVersion>, String> {
    let settings = store.load_settings().map_err(|e| e.to_string())?;
    /*
     * 版本清单带磁盘缓存：命中时立刻返回，过期时先返回旧列表再后台刷新。
     *
     * 之前每次打开对话框都要重新拉 213 KB 的 maven 元数据，官方源在中文区常见
     * 网络下还会先超时 20 秒——用户看到的就是「正在加载…」长时间转圈。加载器
     * 版本一天最多变几次，缓存十分钟的收益远大于它的陈旧风险。
     */
    let cache_root = app.path().app_data_dir().ok();
    loaders::list_loader_versions_cached(
        kind,
        &game_version,
        &settings.download,
        cache_root.as_deref(),
    )
    .await
}
#[tauri::command]
pub async fn install_loader(
    instance_id: String,
    kind: LoaderKind,
    version: String,
    app: AppHandle,
    store: State<'_, LauncherStore>,
    operations: State<'_, Operations>,
    downloads: State<'_, Arc<crate::downloads::Downloads>>,
) -> Result<(), String> {
    let _guard = operations.acquire(&instance_id)?;
    let _shared_files = operations.acquire("shared-game-files")?;
    let target = instance(&store, &instance_id)?;
    if target.loader != "vanilla" {
        return Err("每个实例仅允许安装一个加载器；请新建实例切换。".into());
    }
    let root = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let task = downloads.start(
        &instance_id,
        &format!("安装 {} 加载器", kind.name()),
        &store,
        &app,
    )?;
    let engine = engine(&store)?
        .with_control(task.control)
        .with_task_progress(task.progress);
    let (tx, progress_task) = task_progress(&instance_id, &app, store.inner(), downloads.inner());
    let result =
        loaders::install_loader(&root, &target, &store, &engine, kind, &version, Some(tx)).await;
    let _ = progress_task.await;
    downloads.finish(&instance_id, result.is_ok(), &store, &app)?;
    result
}
#[tauri::command]
pub fn list_mods(
    instance_id: String,
    store: State<'_, LauncherStore>,
) -> Result<Vec<ModFile>, String> {
    mods::list_mods(&instance(&store, &instance_id)?)
}

/// 从磁盘识别实例的真实版本（与记录可能不一致）。
///
/// 只读不写：是否把识别结果写回记录由用户决定。静默改掉 `game_version` 会让
/// 「我明明设成了 X」变成一个无法解释的状态。
#[tauri::command]
pub fn detect_instance_version(
    instance_id: String,
    store: State<'_, LauncherStore>,
) -> Result<launcher_core::version_detect::VersionDetection, String> {
    let target = instance(&store, &instance_id)?;
    Ok(launcher_core::version_detect::detect_instance_version(
        &target,
    ))
}

/// 把识别出的版本写回实例记录。
///
/// 只有用户确认后才应调用。写入前重新识别一次，避免界面上看到的旧快照被写入。
#[tauri::command]
pub fn apply_detected_version(
    instance_id: String,
    store: State<'_, LauncherStore>,
) -> Result<launcher_core::version_detect::VersionDetection, String> {
    let target = instance(&store, &instance_id)?;
    let detected = launcher_core::version_detect::detect_instance_version(&target);
    if let Some(version) = &detected.version {
        store
            .set_instance_game_version(&instance_id, version)
            .map_err(|error| error.to_string())?;
    }
    Ok(detected)
}

/// 查询实例的 Fabric API 状态（是否适用、是否已装、可装哪个版本）。
///
/// 拆成「查」与「装」两条命令而不是一条：界面需要在用户点击之前就告诉他是
/// 「已安装」还是「可以安装」，装完之后还要重新查一次来刷新显示。
#[tauri::command]
pub async fn fabric_api_status(
    instance_id: String,
    store: State<'_, LauncherStore>,
) -> Result<launcher_core::fabric_api::FabricApiStatus, String> {
    let target = instance(&store, &instance_id)?;
    launcher_core::fabric_api::fabric_api_status(&target).await
}

/// 安装 Fabric API 到实例的 mods 目录。
#[tauri::command]
pub async fn install_fabric_api(
    instance_id: String,
    app: AppHandle,
    store: State<'_, LauncherStore>,
    operations: State<'_, Operations>,
    downloads: State<'_, Arc<crate::downloads::Downloads>>,
) -> Result<String, String> {
    let _guard = operations.acquire(&instance_id)?;
    let target = instance(&store, &instance_id)?;
    if !launcher_core::fabric_api::instance_uses_fabric(&target) {
        return Err("只有 Fabric 或 Quilt 实例需要 Fabric API。".into());
    }
    let task = downloads.start(&instance_id, "安装 Fabric API", &store, &app)?;
    let engine = engine(&store)?
        .with_control(task.control)
        .with_task_progress(task.progress);
    let (tx, progress_task) = task_progress(&instance_id, &app, store.inner(), downloads.inner());
    let result = launcher_core::fabric_api::install_fabric_api(&target, &engine, Some(tx)).await;
    let _ = progress_task.await;
    downloads.finish(&instance_id, result.is_ok(), &store, &app)?;
    result
}

/// Static, best-effort mod compatibility findings (deps/conflicts). Reads JAR
/// metadata only; never loads classes or executes mod code.
#[tauri::command]
pub fn inspect_mods(
    instance_id: String,
    store: State<'_, LauncherStore>,
) -> Result<Vec<mods::ModIssue>, String> {
    Ok(mods::inspect_mods(&instance(&store, &instance_id)?))
}
#[tauri::command]
pub fn set_mod_enabled(
    instance_id: String,
    file_name: String,
    enabled: bool,
    store: State<'_, LauncherStore>,
    operations: State<'_, Operations>,
) -> Result<(), String> {
    let _guard = operations.acquire(&instance_id)?;
    mods::set_mod_enabled(&instance(&store, &instance_id)?, &file_name, enabled)
}

/// Query Modrinth for newer mod versions. The frontend has already disclosed
/// that installed file hashes are sent to Modrinth for this lookup.
#[tauri::command]
pub async fn check_mod_updates(
    instance_id: String,
    store: State<'_, LauncherStore>,
    operations: State<'_, Operations>,
) -> Result<Vec<ModUpdateReport>, String> {
    let _guard = operations.acquire(&instance_id)?;
    let target = instance(&store, &instance_id)?;
    let mods_dir = target.game_dir.join("mods");
    let files = mods::list_mods(&target)?;
    let names: Vec<String> = files.iter().map(|file| file.file_name.clone()).collect();
    let client = mod_updates::modrinth_client()?;
    mod_updates::check_mod_updates(&client, &target, &mods_dir, &names).await
}

#[tauri::command]
pub async fn update_mod(
    instance_id: String,
    update: ModUpdate,
    app: AppHandle,
    store: State<'_, LauncherStore>,
    operations: State<'_, Operations>,
    downloads: State<'_, Arc<crate::downloads::Downloads>>,
) -> Result<String, String> {
    let _guard = operations.acquire(&instance_id)?;
    let target = instance(&store, &instance_id)?;
    let task = downloads.start(
        &instance_id,
        &format!("更新模组 {}", update.new_file_name),
        &store,
        &app,
    )?;
    let engine = engine(&store)?
        .with_control(task.control)
        .with_task_progress(task.progress);
    let (tx, progress_task) = task_progress(&instance_id, &app, store.inner(), downloads.inner());
    let result =
        mod_updates::apply_mod_update(&target, &update, &target.game_dir.join("mods"), &engine)
            .await;
    // Emit final progress for the single artifact so task history closes cleanly.
    drop(tx);
    let _ = progress_task.await;
    downloads.finish(&instance_id, result.is_ok(), &store, &app)?;
    result
}

/// Compare the installed official profile against the current Mojang manifest.
#[tauri::command]
pub async fn check_client_update(
    instance_id: String,
    store: State<'_, LauncherStore>,
) -> Result<ClientUpdateStatus, String> {
    let target = instance(&store, &instance_id)?;
    let client = reqwest::Client::builder()
        .user_agent(concat!("MFMCL/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    client_update_status_for(&client, &target)
        .await
        .map_err(|e| e.to_string())
}

/// Delete an instance. File removal happens only for a directory inside the
/// launcher's own instances root, and only after the row is gone.
#[tauri::command]
pub async fn delete_instance(
    instance_id: String,
    delete_files: bool,
    app: AppHandle,
    store: State<'_, LauncherStore>,
    operations: State<'_, Operations>,
) -> Result<(), String> {
    let _guard = operations.acquire(&instance_id)?;
    let removed = store
        .delete_instance(&instance_id)
        .map_err(|e| e.to_string())?;
    if !delete_files {
        return Ok(());
    }
    let target_real = match tokio::fs::canonicalize(&removed.instance_dir).await {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("实例目录不可用：{error}")),
    };
    let mut roots = vec![
        app.path()
            .app_data_dir()
            .map_err(|e| e.to_string())?
            .join("instances"),
    ];
    if let Some(root) = store.load_settings().map_err(|e| e.to_string())?.game_root {
        roots.push(root.join("instances"));
    }
    let mut instances_real = None;
    for root in roots {
        if let Ok(root) = tokio::fs::canonicalize(root).await {
            if target_real.parent() == Some(root.as_path()) {
                instances_real = Some(root);
                break;
            }
        }
    }
    let Some(instances_real) = instances_real else {
        return Err("已删除实例记录；目录不在启动器管理范围内，未删除任何文件。".into());
    };
    if let Some(game_real) = tokio::fs::canonicalize(&removed.game_dir)
        .await
        .ok()
        .filter(|game| game.starts_with(&instances_real))
        && !game_real.starts_with(&target_real)
    {
        // Isolated instances keep their game dir inside the instance dir; if
        // anything points outside, leave the parent removal to the user.
        return Err("已删除实例记录；游戏目录在实例目录之外，请手动确认后删除。".into());
    }
    tokio::fs::remove_dir_all(&removed.instance_dir)
        .await
        .map_err(|e| format!("已删除实例记录，但移除目录失败：{e}"))
}

#[tauri::command]
pub async fn search_mods(
    instance_id: String,
    request: launcher_core::catalog::ModQuery,
    store: State<'_, LauncherStore>,
    operations: State<'_, Operations>,
) -> Result<Vec<launcher_core::CatalogMod>, String> {
    let _guard = operations.acquire(&instance_id)?;
    let target = instance(&store, &instance_id)?;
    let client = launcher_core::catalog::catalog_client_with_settings(
        &store.load_settings().map_err(|e| e.to_string())?.download,
    )?;
    launcher_core::catalog::search_mods(&client, &target, &request).await
}

/// 没有目标实例时的搜索（商店的「直连下载」模式）。
///
/// 与 `search_mods` 的区别只有两点：不要求实例、不做实例级 operation 锁
/// （没有实例可锁）。返回同样的 `CatalogMod`，前端不必区分两种结果的形状。
#[tauri::command]
pub async fn search_mods_unscoped(
    request: launcher_core::catalog::ModQuery,
    store: State<'_, LauncherStore>,
) -> Result<Vec<launcher_core::CatalogMod>, String> {
    let client = launcher_core::catalog::catalog_client_with_settings(
        &store.load_settings().map_err(|e| e.to_string())?.download,
    )?;
    launcher_core::catalog::search_mods_unscoped(&client, &request).await
}

/// 没有目标实例时列出版本（直连下载模式）。
///
/// 模组/资源包在直连模式下也需要版本列表，而 `list_mod_versions` 依赖实例的
/// 游戏版本与加载器。这里走 `project_versions`：不按实例收窄，由用户在文件列表里
/// 自己挑合适的版本——这正是「不装进任何实例」的含义。
#[tauri::command]
pub async fn list_project_versions(
    project_id: String,
    project_type: Option<String>,
    store: State<'_, LauncherStore>,
) -> Result<Vec<launcher_core::CatalogVersion>, String> {
    let client = launcher_core::catalog::catalog_client_with_settings(
        &store.load_settings().map_err(|e| e.to_string())?.download,
    )?;
    let kind = project_type.unwrap_or_else(|| "mod".to_owned());
    launcher_core::catalog::project_versions(&client, &project_id, &kind).await
}

#[tauri::command]
pub async fn list_mod_versions(
    instance_id: String,
    project_id: String,
    project_type: Option<String>,
    store: State<'_, LauncherStore>,
    operations: State<'_, Operations>,
) -> Result<Vec<launcher_core::CatalogVersion>, String> {
    let _guard = operations.acquire(&instance_id)?;
    let target = instance(&store, &instance_id)?;
    let client = launcher_core::catalog::catalog_client_with_settings(
        &store.load_settings().map_err(|e| e.to_string())?.download,
    )?;
    let kind = project_type.unwrap_or_default();
    launcher_core::catalog::mod_versions_of(&client, &target, &project_id, &kind).await
}

/// 内容类型 → 实例内的安装目录。
///
/// 资源包与光影包**不装进 `mods/`**：游戏只从 `resourcepacks/` 与 `shaderpacks/`
/// 读取它们，放错目录的结果是「装成功了但游戏里看不到」，比报错更难排查。
fn content_dir(project_type: &str) -> Result<&'static str, String> {
    match launcher_core::catalog::normalize_project_type(project_type)? {
        "resourcepack" => Ok("resourcepacks"),
        "shader" => Ok("shaderpacks"),
        "datapack" => Ok("datapacks"),
        // 模组与整合包的文件都落到 mods/（整合包走另一条导入路径，不经过这里）。
        _ => Ok("mods"),
    }
}

#[tauri::command]
pub async fn install_mod_version(
    instance_id: String,
    version: launcher_core::CatalogVersion,
    project_type: Option<String>,
    /*
     * 项目名（例如「JEI物品管理器」）。
     *
     * 任务卡标题要回答「用户在装什么」，而不是「正在传哪个文件」。`version.file_name`
     * 是 `jei-1.20.1-forge.jar` 这种文件名，作为任务标题既认不出是哪个模组，也会让
     * 用户以为自己在手动下载一个 jar。
     *
     * 可选：旧前端、以及没有项目上下文的调用方不传，此时退回文件名。
     */
    project_name: Option<String>,
    app: AppHandle,
    store: State<'_, LauncherStore>,
    operations: State<'_, Operations>,
    downloads: State<'_, Arc<crate::downloads::Downloads>>,
) -> Result<String, String> {
    let _guard = operations.acquire(&instance_id)?;
    let target = instance(&store, &instance_id)?;
    let kind = project_type.unwrap_or_default();
    let dir_name = content_dir(&kind)?;
    /*
     * 模组必须装进带加载器的实例。
     *
     * 版本列表现在对原版实例也开放（看版本是公开信息，不该被实例类型挡住），
     * 但「安装」不行：Forge/Fabric 的模组在纯原版客户端里不会被加载，游戏启动后
     * 它们只是躺在 mods 目录里，用户会以为装好了却没效果。
     *
     * 资源包 / 光影 / 数据包不在此列——它们在原版里同样生效。
     */
    if dir_name == "mods" && target.loader == "vanilla" {
        return Err(
            "这个实例是原版，没有安装模组加载器；请先安装 Fabric、Quilt、Forge 或 NeoForge。"
                .into(),
        );
    }
    if !target.loader.starts_with("vanilla") && dir_name == "mods" {
        let loader = target.loader.split('@').next().unwrap_or("");
        if !version
            .game_versions
            .iter()
            .any(|v| v == &target.game_version)
            || !version.loaders.iter().any(|v| v == loader)
        {
            return Err("此模组版本与实例不兼容".into());
        }
    }
    let label = match dir_name {
        "resourcepacks" => "资源包",
        "shaderpacks" => "光影包",
        "datapacks" => "数据包",
        _ => "模组",
    };
    /*
     * 标题优先用项目名，没有时退回文件名。
     *
     * 两种都带版本号：同一个整合里装两个不同版本的同一个模组并不罕见，只看项目名
     * 会分不清任务卡上是哪一次安装。
     */
    let title = match project_name.as_deref().map(str::trim) {
        Some(name) if !name.is_empty() => {
            let name: String = name.chars().take(64).collect();
            let version = version.version_number.trim();
            if version.is_empty() {
                format!("安装{label} {name}")
            } else {
                format!("安装{label} {name} {version}")
            }
        }
        _ => format!("安装{label} {}", version.file_name),
    };
    let task = downloads.start(&instance_id, &title, &store, &app)?;
    let engine = engine(&store)?
        .with_control(task.control)
        .with_task_progress(task.progress);
    let (tx, progress_task) = task_progress(&instance_id, &app, store.inner(), downloads.inner());
    let result = if dir_name == "mods" {
        launcher_core::catalog::dependencies::install(&target, &version, &engine).await
    } else {
        launcher_core::catalog::install_mod_version(
            &version,
            &target.game_dir.join(dir_name),
            &engine,
        )
        .await
    };
    drop(tx);
    let _ = progress_task.await;
    downloads.finish(&instance_id, result.is_ok(), &store, &app)?;
    result
}

#[tauri::command]
pub async fn download_mod_version(
    instance_id: String,
    version: launcher_core::CatalogVersion,
    directory: String,
    // 项目名，用于任务标题；见 `install_mod_version` 的同名参数。
    project_name: Option<String>,
    app: AppHandle,
    store: State<'_, LauncherStore>,
    operations: State<'_, Operations>,
    downloads: State<'_, Arc<crate::downloads::Downloads>>,
) -> Result<String, String> {
    let _guard = operations.acquire(&instance_id)?;
    let target = std::path::PathBuf::from(directory);
    if target.as_os_str().is_empty() {
        return Err("请选择下载目录。".into());
    }
    let title = match project_name.as_deref().map(str::trim) {
        Some(name) if !name.is_empty() => {
            let name: String = name.chars().take(64).collect();
            format!("下载模组 {name}（{}）", version.file_name)
        }
        _ => format!("下载模组 {}", version.file_name),
    };
    let task = downloads.start(&instance_id, &title, &store, &app)?;
    let engine = engine(&store)?
        .with_control(task.control)
        .with_task_progress(task.progress);
    let (tx, progress_task) = task_progress(&instance_id, &app, store.inner(), downloads.inner());
    let result = launcher_core::catalog::download_mod_version_to(&version, &target, &engine).await;
    drop(tx);
    let _ = progress_task.await;
    downloads.finish(&instance_id, result.is_ok(), &store, &app)?;
    result
}

#[tauri::command]
pub async fn preview_modpack(
    path: String,
    local_paths: Option<Vec<PathBuf>>,
    store: State<'_, LauncherStore>,
) -> Result<PackResolution, String> {
    let key = crate::curseforge::read_key_for_internal().await;
    let client = launcher_core::curseforge::metadata_client_with_settings(
        &store.load_settings().map_err(|e| e.to_string())?.download,
    )?;
    modpacks::prepare_pack_for_import_with_client(
        &client,
        &PathBuf::from(path),
        &local_paths.unwrap_or_default(),
        key.as_deref(),
    )
    .await
}
#[tauri::command]
pub async fn import_modpack(
    path: String,
    local_paths: Option<Vec<PathBuf>>,
    app: AppHandle,
    store: State<'_, LauncherStore>,
    operations: State<'_, Operations>,
    downloads: State<'_, Arc<crate::downloads::Downloads>>,
) -> Result<InstanceRecord, String> {
    let path = PathBuf::from(path);
    let result = import_modpack_inner(
        path.clone(),
        &local_paths.unwrap_or_default(),
        &app,
        store.inner(),
        &operations,
        downloads.inner(),
    )
    .await;
    if result.is_ok() {
        let _ = discard_cached_pack(&path, &operations);
    }
    result
}

/// 导入一个已在本地的整合包文件。
///
/// 拆出内核是为了让「下载并导入」能复用同一套事务，而不必经过 Tauri 的
/// `State` 包装——命令函数之间不能直接互相调用，`State` 只由框架注入。
async fn import_modpack_inner(
    path: PathBuf,
    local_paths: &[PathBuf],
    app: &AppHandle,
    store: &LauncherStore,
    operations: &Operations,
    downloads: &Arc<crate::downloads::Downloads>,
) -> Result<InstanceRecord, String> {
    let _guard = operations.acquire("modpack-import")?;
    let key = crate::curseforge::read_key_for_internal().await;
    let client = launcher_core::curseforge::metadata_client_with_settings(
        &store.load_settings().map_err(|e| e.to_string())?.download,
    )?;
    let mut resolution =
        modpacks::prepare_pack_for_import_with_client(&client, &path, local_paths, key.as_deref())
            .await?;
    if !resolution.unresolved_files.is_empty() {
        let _ = app.emit("modpack-import-warnings", &resolution.unresolved_files);
    }
    modpacks::require_complete_curseforge_files(&resolution.unresolved_files)?;
    let prepared = resolution.prepared.take().ok_or("整合包尚未补全。")?;
    let preview = prepared.preview().clone();
    let root = app.path().app_data_dir().map_err(|e| e.to_string())?;
    // Construct the engine before creating a persistent instance, so initialization failures leave no row.
    let engine = engine(store)?;
    let target = store
        .create_isolated_instance(
            &CreateInstanceInput {
                name: preview.name.chars().take(48).collect(),
                game_version: preview.minecraft.clone(),
            },
            &root,
        )
        .map_err(|e| e.to_string())?;
    let _instance_guard = match operations.acquire(&target.id) {
        Ok(guard) => guard,
        Err(error) => {
            let _ = store.discard_uninstalled_instance(&target.id);
            return Err(error);
        }
    };
    let handle = match downloads.start(&target.id, &format!("导入 {}", preview.name), store, app)
    {
        Ok(handle) => handle,
        Err(error) => {
            let _ = store.discard_uninstalled_instance(&target.id);
            return Err(error);
        }
    };
    let engine = engine
        .with_control(handle.control)
        .with_task_progress(handle.progress);
    let (tx, progress_task) = task_progress(&target.id, app, store, downloads);
    let result = import_pack_contents(prepared, &target, &engine, store, Some(tx)).await;
    let _ = progress_task.await;
    let task_saved = downloads.finish(&target.id, result.is_ok(), store, app);
    if let Err(mut error) = result {
        match std::fs::remove_dir_all(&target.instance_dir) {
            Ok(()) => {}
            Err(cleanup) if cleanup.kind() == std::io::ErrorKind::NotFound => {}
            Err(cleanup) => error = format!("{error}；清理导入目录失败：{cleanup}"),
        }
        if let Err(cleanup) = store.discard_uninstalled_instance(&target.id) {
            error = format!("{error}；清理实例记录失败：{cleanup}");
        }
        if let Err(task_error) = task_saved {
            error = format!("{error}；任务状态保存失败：{task_error}");
        }
        return Err(error);
    }
    // Import has committed; a task-history write failure must not make the UI retry creation.
    let _ = task_saved;
    Ok(target)
}

async fn import_pack_contents(
    mut prepared: modpacks::PreparedPack,
    target: &InstanceRecord,
    engine: &DownloadEngine,
    store: &LauncherStore,
    progress: Option<tokio::sync::mpsc::UnboundedSender<DownloadProgress>>,
) -> Result<(), String> {
    let loader = prepared.preview().loader.clone();
    let overrides = prepared.take_imported_overrides();
    if let Some(overrides) = &overrides {
        launcher_core::validate_instance_overrides(overrides)?;
    }
    modpacks::import_prepared_pack(prepared, target, engine, progress).await?;
    write_pending_loader(target, &loader)?;
    if let Some(overrides) = overrides {
        store
            .save_instance_overrides(&target.id, &overrides)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 下载并预检整合包。受限依赖等待本地补件，包路径保留到确认或取消。
#[tauri::command]
pub async fn download_and_import_modpack(
    instance_hint: Option<String>,
    version: launcher_core::curseforge::CurseForgeVersion,
    app: AppHandle,
    store: State<'_, LauncherStore>,
    operations: State<'_, Operations>,
    downloads: State<'_, Arc<crate::downloads::Downloads>>,
) -> Result<PackResolution, String> {
    /*
     * `instance_hint` 只用于给下载任务挂一个可见的归属。
     *
     * 整合包会创建**新**实例，所以这里没有真正意义上的「当前实例」；但下载任务
     * 需要一个 instance_id 才能出现在下载页。用调用方给的 id 是一个折中：任务
     * 会显示在用户当前正在看的实例下面，导入完成后新实例才出现。
     */
    let _guard = operations.acquire("modpack-download")?;
    let task_owner = instance_hint.unwrap_or_else(|| "modpack-download".to_owned());

    // 文件名与扩展名的校验都在 `download_to` 内部按内容类型完成（整合包允许
    // `.zip`/`.mrpack`），这里不再重复——重复一份规则只会在两处漂移。

    // 下载目录归本次待确认来源所有，只有取消、导入成功或退出时清理。
    let staging = std::env::temp_dir().join(format!("mfmcl-pack-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&staging).map_err(|e| format!("无法创建下载目录：{e}"))?;

    let result = async {
        // engine 必须在 downloads.start 之前构造：先 start 后建 engine 的话，一旦
        // engine 失败（设置读取失败、并发数非法），那条下载任务会永远停在 Running，
        // 之后同一个 instance 的下载都会被拒绝。
        let engine = engine(store.inner())?;
        let handle = downloads.start(
            &task_owner,
            &format!("下载整合包 {}", version.file_name),
            store.inner(),
            &app,
        )?;
        let engine = engine
            .with_control(handle.control)
            .with_task_progress(handle.progress);
        let (tx, progress_task) =
            task_progress(&task_owner, &app, store.inner(), downloads.inner());

        let downloaded = launcher_core::curseforge::download_to(
            &engine,
            &version,
            launcher_core::curseforge::ContentKind::Modpack,
            &staging,
        )
        .await;
        drop(tx);
        let _ = progress_task.await;
        downloads.finish(&task_owner, downloaded.is_ok(), store.inner(), &app)?;
        let path = downloaded?;

        let key = crate::curseforge::read_key_for_internal().await;
        let client = launcher_core::curseforge::metadata_client_with_settings(
            &store.load_settings().map_err(|e| e.to_string())?.download,
        )?;
        let resolution = modpacks::prepare_pack_for_import_with_client(
            &client,
            &PathBuf::from(&path),
            &[],
            key.as_deref(),
        )
        .await?;
        operations
            .1
            .lock()
            .map_err(|_| "整合包缓存状态不可用。")?
            .insert(PathBuf::from(path));
        Ok::<_, String>(resolution)
    }
    .await;

    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

fn discard_cached_pack(path: &std::path::Path, operations: &Operations) -> Result<(), String> {
    let mut cached = operations.1.lock().map_err(|_| "整合包缓存状态不可用。")?;
    if cached.contains(path) {
        let parent = path.parent().ok_or("缓存路径无效。")?;
        match std::fs::remove_dir_all(parent) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        cached.remove(path);
    }
    Ok(())
}

#[tauri::command]
pub fn discard_modpack_source(
    path: String,
    operations: State<'_, Operations>,
) -> Result<(), String> {
    let _guard = operations.acquire("modpack-import")?;
    discard_cached_pack(&PathBuf::from(path), &operations)
}

/// 列出 Modrinth 项目的**全部**版本（不按实例过滤）。
///
/// 整合包自带游戏版本与加载器，导入时会据此创建新实例。按当前实例去筛会让用户
/// 看不到「装一个别的 MC 版本的整合包」，而那正是整合包最常见的用法。
#[tauri::command]
pub async fn project_versions(
    project_id: String,
    project_type: Option<String>,
    store: State<'_, LauncherStore>,
) -> Result<Vec<launcher_core::CatalogVersion>, String> {
    let client = launcher_core::catalog::catalog_client_with_settings(
        &store.load_settings().map_err(|e| e.to_string())?.download,
    )?;
    let kind = project_type.unwrap_or_else(|| "modpack".to_owned());
    launcher_core::catalog::project_versions(&client, &project_id, &kind).await
}

/// 下载并导入一个 Modrinth 的 `.mrpack` 为新实例。
///
/// 与 `download_and_import_modpack`（CurseForge）的关系：两者最终都走
/// `import_modpack_inner` 这同一套事务（校验结构 → 建实例 → 下载文件 → 失败回滚），
/// 区别只在「包从哪里来」。所以这里只负责把文件取到本地，其余完全复用。
#[tauri::command]
pub async fn download_and_import_modrinth_pack(
    version: launcher_core::CatalogVersion,
    instance_hint: Option<String>,
    app: AppHandle,
    store: State<'_, LauncherStore>,
    operations: State<'_, Operations>,
    downloads: State<'_, Arc<crate::downloads::Downloads>>,
) -> Result<InstanceRecord, String> {
    let task_owner = instance_hint.unwrap_or_else(|| "modpack-download".to_owned());
    let staging = std::env::temp_dir().join(format!("mfmcl-mrpack-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&staging).map_err(|e| format!("无法创建下载目录：{e}"))?;

    let result = async {
        let engine = engine(store.inner())?;
        let handle = downloads.start(
            &task_owner,
            &format!("下载整合包 {}", version.file_name),
            store.inner(),
            &app,
        )?;
        let engine = engine
            .with_control(handle.control)
            .with_task_progress(handle.progress);
        let (tx, progress_task) =
            task_progress(&task_owner, &app, store.inner(), downloads.inner());

        let downloaded =
            launcher_core::curseforge::download_modrinth_pack(&engine, &version, &staging).await;
        drop(tx);
        let _ = progress_task.await;
        downloads.finish(&task_owner, downloaded.is_ok(), store.inner(), &app)?;
        let path = downloaded?;

        import_modpack_inner(
            path.into(),
            &[],
            &app,
            store.inner(),
            &operations,
            downloads.inner(),
        )
        .await
    }
    .await;

    let _ = std::fs::remove_dir_all(&staging);
    result
}

#[tauri::command]
pub fn export_modpack(
    instance_id: String,
    directory: String,
    rights_confirmed: bool,
    store: State<'_, LauncherStore>,
    operations: State<'_, Operations>,
) -> Result<String, String> {
    if !rights_confirmed {
        return Err("导出包含本地模组文件，请先确认你有权再分发这些文件。".into());
    }
    let _guard = operations.acquire(&instance_id)?;
    modpacks::export_pack(&instance(&store, &instance_id)?, &PathBuf::from(directory))
        .map(|p| p.to_string_lossy().into_owned())
}

/// 请求在下次安装原版文件时顺带装一个加载器。
///
/// 与整合包导入写的是同一个标记、同一个格式（`<kind>@<version>`），因此
/// `install_instance` 无需区分「用户新建实例时选的加载器」和「整合包自带的
/// 加载器」——两者都由 `install_pending_pack_loader` 消费。
///
/// 这里额外做一层格式校验：标记文件的格式错误会在**之后**的安装阶段才暴露，
/// 那时实例已经建好了，用户看到的是一条莫名其妙的「加载器请求无效」。在写入前
/// 就拒绝，错误能归因到正确的操作上。
pub fn request_pack_loader(instance: &InstanceRecord, request: &str) -> Result<(), String> {
    let (kind, version) = request.split_once('@').ok_or("加载器标识格式无效。")?;
    let kind = launcher_core::loaders::LoaderKind::from_name(kind).ok_or("未识别的加载器。")?;
    launcher_core::loaders::safe_version(version)?;
    write_pending_loader(instance, &format!("{}@{version}", kind.name()))
}

fn write_pending_loader(instance: &InstanceRecord, loader: &str) -> Result<(), String> {
    if loader != "vanilla" {
        let (kind, version) = loader.split_once('@').ok_or("加载器标识格式无效。")?;
        LoaderKind::from_name(kind).ok_or("未识别的加载器。")?;
        loaders::safe_version(version)?;
    }
    let directory = &instance.instance_dir;
    /*
     * `create_instance` 只挑一个空闲路径并插入数据库行，目录本身还不存在。
     * 之前这里直接 `symlink_metadata`，于是「新建带加载器的实例」必然失败在
     * `os error 2`（系统找不到指定的文件）——而且实例记录已经写进去了。
     * 先建目录，与 `launcher-runtime` 里同一段逻辑保持一致。
     */
    std::fs::create_dir_all(directory)
        .map_err(|e| format!("无法创建实例目录 {}：{e}", directory.display()))?;
    let metadata = std::fs::symlink_metadata(directory).map_err(|e| e.to_string())?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err("实例目录不能是符号链接或普通文件。".into());
    }
    let marker = directory.join("pack-loader.txt");
    let temporary = directory.join(format!(".pack-loader-{}.pending", uuid::Uuid::new_v4()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|e| e.to_string())?;
    let result = (|| {
        use std::io::Write;
        file.write_all(loader.as_bytes())
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        std::fs::hard_link(&temporary, &marker)
            .map_err(|e| format!("无法创建加载器请求标记：{e}"))?;
        Ok::<(), String>(())
    })();
    drop(file);
    if let Err(mut error) = result {
        if let Err(cleanup) = std::fs::remove_file(&temporary) {
            error = format!("{error}；清理临时标记失败：{cleanup}");
        }
        return Err(error);
    }
    std::fs::remove_file(temporary)
        .map_err(|e| format!("加载器请求已写入，但临时文件清理失败：{e}"))
}

pub async fn install_pending_pack_loader(
    root: &std::path::Path,
    instance: &InstanceRecord,
    store: &LauncherStore,
    engine: &DownloadEngine,
) -> Result<(), String> {
    let path = instance.instance_dir.join("pack-loader.txt");
    let marker = match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() => metadata,
        Ok(_) => return Err("加载器请求标记必须是普通文件，不能是符号链接。".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if marker.len() > 256 {
        return Err("整合包加载器请求无效。".into());
    }
    let request = tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| e.to_string())?;
    if request == "vanilla" {
        if instance.loader != "vanilla" {
            return Err("整合包加载器请求与实例记录不一致。".into());
        }
    } else {
        let (kind, version) = request.split_once('@').ok_or("整合包加载器请求无效。")?;
        /*
         * 四种加载器都要认。
         *
         * 之前这里只映射 fabric/quilt，Forge 与 NeoForge 的整合包会在这一步被
         * 拒绝（「整合包加载器暂不支持」），尽管 `install_loader` 早就支持它们。
         * CurseForge 上大量整合包是 Forge 的，这个缺口等于把一整类整合包挡在门外。
         */
        let kind = LoaderKind::from_name(kind).ok_or("整合包加载器暂不支持。")?;
        loaders::safe_version(version)?;
        let latest = store
            .list_instances()
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|record| record.id == instance.id)
            .ok_or("找不到实例。")?;
        if latest.loader == format!("{}@{version}", kind.name()) {
            if !loaders::loader_overlay_is_installed(&latest, kind, version)? {
                return Err("实例已记录加载器，但对应配置不存在或无效。".into());
            }
        } else if latest.loader == "vanilla" {
            let mut target = latest;
            target.state = InstanceState::Ready;
            loaders::install_loader(root, &target, store, engine, kind, version, None).await?;
        } else {
            return Err("整合包加载器请求与已安装加载器不一致。".into());
        }
    }
    tokio::fs::remove_file(path)
        .await
        .map_err(|e| format!("加载器已处理，但请求标记无法移除：{e}"))
}

/// 重装实例上记录的那个加载器，用于修复损坏的加载器文件。
///
/// 只在 [`launcher_core::loaders::diagnose_loader`] 报 `Broken` 时调用。
/// 它复用正常的安装流程，只是允许覆盖同一个加载器：
///   - Fabric / Quilt 会重新下载 loader 的库并重写 `launch.json`；
///   - Forge / NeoForge 会重跑整套 processor（这也是修复被打补丁客户端唯一的办法）。
///
/// 需要临时把实例状态当成 `Ready`：`install_loader` 会拒绝非就绪实例，而调用点
/// 可能是 `failed` 的实例（补全正是在这种实例上最有用）。状态不会写回数据库——
/// 外层拿到成功结果后会由 `install_vanilla` 设成 `Ready`。
pub async fn repair_loader(
    root: &std::path::Path,
    instance: &InstanceRecord,
    store: &LauncherStore,
    engine: &DownloadEngine,
) -> Result<(), String> {
    let Some((name, version)) = instance.loader.split_once('@') else {
        return Err(format!("加载器记录格式无效：{}", instance.loader));
    };
    let Some(kind) = launcher_core::loaders::LoaderKind::from_name(name) else {
        return Err(format!("未识别的加载器：{name}"));
    };
    // 从数据库重读一次：调用点手里的记录可能是补全前的快照。
    let latest = store
        .list_instances()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|record| record.id == instance.id)
        .ok_or("找不到实例。")?;
    let mut target = latest;
    target.state = InstanceState::Ready;
    launcher_core::loaders::reinstall_loader(root, &target, store, engine, kind, version, None)
        .await
}

#[cfg(test)]
mod pack_tests {
    use super::*;

    #[tokio::test]
    async fn hmcl_import_preferences_are_persisted_only_for_the_new_instance() {
        use std::io::Write;
        let root =
            std::env::temp_dir().join(format!("mfmcl-hmcl-desktop-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("hmcl.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        for (name, bytes) in [
            ("modpack.json", br#"{"name":"HMCL Native","version":"1","gameVersion":"1.21.4"}"#.as_slice()),
            ("minecraft/pack.json", br#"{"id":"1.21.4","mainClass":"net.minecraft.client.main.Main","libraries":[]}"#.as_slice()),
            ("minecraft/hmclversion.cfg", br#"{"usesGlobal":false,"autoMemory":false,"maxMemory":3072,"width":1280,"height":720}"#.as_slice()),
            ("minecraft/config/test.json", b"{}".as_slice()),
        ] {
            zip.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
        let modpacks::PackPreparation::Ready(prepared) = modpacks::prepare_pack(&path).unwrap()
        else {
            panic!("HMCL must be recognized");
        };
        let store = LauncherStore::open(":memory:").unwrap();
        let global = store.load_settings().unwrap();
        let target = store
            .create_isolated_instance(
                &CreateInstanceInput {
                    name: "Native".into(),
                    game_version: "1.21.4".into(),
                },
                &root,
            )
            .unwrap();
        import_pack_contents(prepared, &target, &engine(&store).unwrap(), &store, None)
            .await
            .unwrap();
        let overrides = store.load_instance_overrides(&target.id).unwrap();
        assert_eq!(overrides.memory_mb, Some(3072));
        assert_eq!(overrides.window_width, Some(1280));
        assert_eq!(overrides.window_height, Some(720));
        assert_eq!(
            serde_json::to_value(store.load_settings().unwrap()).unwrap(),
            serde_json::to_value(global).unwrap()
        );
        assert!(target.game_dir.join("config/test.json").is_file());
        assert!(!target.game_dir.join("hmclversion.cfg").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancelling_cached_pack_never_deletes_user_selected_sources() {
        let root = std::env::temp_dir().join(format!("mfmcl-pack-cancel-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let user_source = root.join("user.zip");
        std::fs::write(&user_source, b"user").unwrap();
        let operations = Operations::default();
        discard_cached_pack(&user_source, &operations).unwrap();
        assert_eq!(std::fs::read(&user_source).unwrap(), b"user");
        let staging = root.join("backend-staging");
        std::fs::create_dir(&staging).unwrap();
        let cached = staging.join("pack.zip");
        std::fs::write(&cached, b"cache").unwrap();
        operations.1.lock().unwrap().insert(cached.clone());
        discard_cached_pack(&cached, &operations).unwrap();
        discard_cached_pack(&cached, &operations).unwrap();
        assert!(!staging.exists());
        assert!(operations.1.lock().unwrap().is_empty());
        assert_eq!(std::fs::read(user_source).unwrap(), b"user");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn desktop_default_creation_imports_forge_packs_and_writes_pending_loader() {
        use std::io::Write;
        for (kind, version) in [("forge", "54.0.0"), ("neoforge", "21.4.1")] {
            let root =
                std::env::temp_dir().join(format!("mfmcl-desktop-pack-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let pack = root.join("test.mrpack");
            let mut writer = zip::ZipWriter::new(std::fs::File::create(&pack).unwrap());
            writer
                .start_file(
                    "modrinth.index.json",
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
            let index = serde_json::json!({"formatVersion":1,"game":"minecraft","name":"Import","versionId":"1","files":[],"dependencies":{"minecraft":"1.21.4",kind:version}});
            writer
                .write_all(&serde_json::to_vec(&index).unwrap())
                .unwrap();
            writer
                .start_file(
                    "overrides/config/mod.json",
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
            writer.write_all(b"{}").unwrap();
            writer.finish().unwrap();
            let modpacks::PackPreparation::Ready(prepared) = modpacks::prepare_pack(&pack).unwrap()
            else {
                panic!("mrpack must be ready")
            };
            let store = LauncherStore::open(":memory:").unwrap();
            let target = store
                .create_instance(
                    &CreateInstanceInput {
                        name: "Import".into(),
                        game_version: "1.21.4".into(),
                    },
                    &root,
                )
                .unwrap();
            import_pack_contents(prepared, &target, &engine(&store).unwrap(), &store, None)
                .await
                .unwrap();
            assert!(target.isolated);
            assert_eq!(
                std::fs::read(target.game_dir.join("config/mod.json")).unwrap(),
                b"{}"
            );
            assert_eq!(
                std::fs::read_to_string(target.instance_dir.join("pack-loader.txt")).unwrap(),
                format!("{kind}@{version}")
            );
            assert_eq!(
                store.list_instances().unwrap()[0].loader,
                "vanilla",
                "loader remains pending until real installation"
            );
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[tokio::test]
    async fn pending_marker_retry_uses_committed_store_loader_without_reinstalling() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let store = LauncherStore::open(":memory:").unwrap();
        let target = store
            .create_instance(
                &CreateInstanceInput {
                    name: "retry".into(),
                    game_version: "1.21.4".into(),
                },
                &root,
            )
            .unwrap();
        let version_dir = target.instance_dir.join("versions/1.21.4");
        std::fs::create_dir_all(&version_dir).unwrap();
        let overlay = br#"{"id":"1.21.4","mainClass":"loader.Main","libraries":[]}"#;
        std::fs::write(version_dir.join("launch.json"), overlay).unwrap();
        store
            .set_instance_loader(&target.id, "fabric@0.16.10")
            .unwrap();
        write_pending_loader(&target, "fabric@0.16.10").unwrap();
        let engine = engine(&store).unwrap();
        // target intentionally still says vanilla: retry must read the committed database record.
        install_pending_pack_loader(&root, &target, &store, &engine)
            .await
            .unwrap();
        assert!(!target.instance_dir.join("pack-loader.txt").exists());
        assert_eq!(
            std::fs::read(version_dir.join("launch.json")).unwrap(),
            overlay
        );
        assert_eq!(store.list_instances().unwrap()[0].loader, "fabric@0.16.10");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pending_marker_publish_never_replaces_an_existing_request() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let store = LauncherStore::open(":memory:").unwrap();
        let target = store
            .create_instance(
                &CreateInstanceInput {
                    name: "marker".into(),
                    game_version: "1.21.4".into(),
                },
                &root,
            )
            .unwrap();
        std::fs::create_dir_all(&target.instance_dir).unwrap();
        write_pending_loader(&target, "vanilla").unwrap();
        assert!(write_pending_loader(&target, "fabric@0.16.10").is_err());
        assert_eq!(
            std::fs::read(target.instance_dir.join("pack-loader.txt")).unwrap(),
            b"vanilla"
        );
        assert_eq!(std::fs::read_dir(&target.instance_dir).unwrap().count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// 新建实例时选的加载器走的是整合包那条标记通路。
    #[test]
    fn create_instance_loader_request_is_written_for_the_installer() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let store = LauncherStore::open(":memory:").unwrap();
        let target = store
            .create_instance(
                &CreateInstanceInput {
                    name: "fabric".into(),
                    game_version: "1.21.4".into(),
                },
                &root,
            )
            .unwrap();
        std::fs::create_dir_all(&target.instance_dir).unwrap();
        request_pack_loader(&target, "fabric@0.16.10").unwrap();
        let written = std::fs::read_to_string(target.instance_dir.join("pack-loader.txt")).unwrap();
        // 归一化后的标识：`from_name` 决定大小写，安装阶段按这个格式解析。
        assert_eq!(written, "fabric@0.16.10");
        std::fs::remove_dir_all(root).unwrap();
    }

    /// 桌面端新建实例时目录还不存在，写标记必须自己建目录。
    ///
    /// `create_instance` 只挑一个空闲路径并插入数据库行。之前这里直接
    /// `symlink_metadata`，于是界面上选任何加载器再点「开始安装」都会立刻失败在
    /// `os error 2`（系统找不到指定的文件），而实例记录已经写进去了。
    #[test]
    fn loader_request_creates_a_missing_instance_directory() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let store = LauncherStore::open(":memory:").unwrap();
        let target = store
            .create_instance(
                &CreateInstanceInput {
                    name: "forge".into(),
                    game_version: "1.21.1".into(),
                },
                &root,
            )
            .unwrap();
        assert!(
            !target.instance_dir.exists(),
            "create_instance must not pre-create the directory; otherwise this test proves nothing"
        );
        request_pack_loader(&target, "forge@1.21.1-52.1.21").unwrap();
        assert_eq!(
            std::fs::read_to_string(target.instance_dir.join("pack-loader.txt")).unwrap(),
            "forge@1.21.1-52.1.21"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    /// 格式错误必须在写入前就拒绝。
    ///
    /// 若放过这里，错误要到用户点「安装」时才出现，那时实例已经建好，报错归因
    /// 不到「你刚才选的加载器写错了」这件事上。
    #[test]
    fn malformed_loader_requests_are_rejected_before_writing() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let store = LauncherStore::open(":memory:").unwrap();
        let target = store
            .create_instance(
                &CreateInstanceInput {
                    name: "bad".into(),
                    game_version: "1.21.4".into(),
                },
                &root,
            )
            .unwrap();
        std::fs::create_dir_all(&target.instance_dir).unwrap();
        for bad in [
            "",               // 空
            "fabric",         // 缺版本
            "unknown@1.0",    // 未识别的加载器
            "fabric@../evil", // 路径穿越
            "fabric@",        // 空版本
        ] {
            assert!(
                request_pack_loader(&target, bad).is_err(),
                "{bad:?} must be rejected"
            );
        }
        // 被拒绝的请求不该留下标记文件，否则安装阶段会去解析一个坏值。
        assert!(!target.instance_dir.join("pack-loader.txt").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
