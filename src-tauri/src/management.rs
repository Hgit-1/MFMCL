use download_engine::{DownloadEngine, DownloadProgress, DownloadSettings};
use launcher_core::check_client_update as client_update_status_for;
use launcher_core::{
    ClientUpdateStatus, CreateInstanceInput, InstanceRecord, InstanceState, LauncherStore,
    loaders::{self, LoaderKind, LoaderVersion},
    mod_updates::{self, ModUpdate, ModUpdateReport},
    modpacks::{self, PackPreview},
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
            downloads.progress(&id, &progress, &store, &app);
            let _ = app.emit("download-progress", progress);
        }
    });
    (tx, task)
}

#[derive(Default)]
pub struct Operations(pub Arc<Mutex<HashSet<String>>>);
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
    DownloadEngine::new(DownloadSettings {
        source: settings.download.source,
        max_concurrent_downloads: settings.download.max_concurrent_downloads,
        connections_per_file: settings.download.connections_per_file,
        aria2_enabled: false,
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn loader_versions(
    kind: LoaderKind,
    game_version: String,
) -> Result<Vec<LoaderVersion>, String> {
    loaders::list_loader_versions(kind, &game_version).await
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
    let engine = engine(&store)?.with_control(downloads.start(
        &instance_id,
        &format!("安装 {} 加载器", kind.name()),
        &store,
        &app,
    )?);
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
    let control = downloads.start(
        &instance_id,
        &format!("更新模组 {}", update.new_file_name),
        &store,
        &app,
    )?;
    let engine = engine(&store)?.with_control(control);
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
    let root = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let instances_root = root.join("instances");
    // Canonicalize both paths; refuse anything that escapes the instances root.
    let instances_real = tokio::fs::canonicalize(&instances_root)
        .await
        .map_err(|e| format!("实例目录不可用：{e}"))?;
    let target_real = tokio::fs::canonicalize(&removed.instance_dir)
        .await
        .map_err(|e| format!("实例目录不可用：{e}"))?;
    if !target_real.starts_with(&instances_real) || target_real == instances_real {
        // The database row is already removed; files stay untouched.
        return Err("已删除实例记录；目录不在启动器管理范围内，未删除任何文件。".into());
    }
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
    let client = launcher_core::catalog::catalog_client()?;
    launcher_core::catalog::search_mods(&client, &target, &request).await
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
    let client = launcher_core::catalog::catalog_client()?;
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
    app: AppHandle,
    store: State<'_, LauncherStore>,
    operations: State<'_, Operations>,
    downloads: State<'_, Arc<crate::downloads::Downloads>>,
) -> Result<String, String> {
    let _guard = operations.acquire(&instance_id)?;
    let target = instance(&store, &instance_id)?;
    let kind = project_type.unwrap_or_default();
    let dir_name = content_dir(&kind)?;
    let label = match dir_name {
        "resourcepacks" => "资源包",
        "shaderpacks" => "光影包",
        "datapacks" => "数据包",
        _ => "模组",
    };
    let control = downloads.start(
        &instance_id,
        &format!("安装{label} {}", version.file_name),
        &store,
        &app,
    )?;
    let engine = engine(&store)?.with_control(control);
    let (tx, progress_task) = task_progress(&instance_id, &app, store.inner(), downloads.inner());
    let result = launcher_core::catalog::install_mod_version(
        &version,
        &target.game_dir.join(dir_name),
        &engine,
    )
    .await;
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
    let control = downloads.start(
        &instance_id,
        &format!("下载模组 {}", version.file_name),
        &store,
        &app,
    )?;
    let engine = engine(&store)?.with_control(control);
    let (tx, progress_task) = task_progress(&instance_id, &app, store.inner(), downloads.inner());
    let result = launcher_core::catalog::download_mod_version_to(&version, &target, &engine).await;
    drop(tx);
    let _ = progress_task.await;
    downloads.finish(&instance_id, result.is_ok(), &store, &app)?;
    result
}

#[tauri::command]
pub fn preview_modpack(path: String) -> Result<PackPreview, String> {
    modpacks::preview_pack(&PathBuf::from(path))
}
#[tauri::command]
pub async fn import_modpack(
    path: String,
    app: AppHandle,
    store: State<'_, LauncherStore>,
    operations: State<'_, Operations>,
    downloads: State<'_, Arc<crate::downloads::Downloads>>,
) -> Result<InstanceRecord, String> {
    import_modpack_inner(
        PathBuf::from(path),
        &app,
        store.inner(),
        &operations,
        downloads.inner(),
    )
    .await
}

/// 导入一个已在本地的整合包文件。
///
/// 拆出内核是为了让「下载并导入」能复用同一套事务，而不必经过 Tauri 的
/// `State` 包装——命令函数之间不能直接互相调用，`State` 只由框架注入。
async fn import_modpack_inner(
    path: PathBuf,
    app: &AppHandle,
    store: &LauncherStore,
    operations: &Operations,
    downloads: &Arc<crate::downloads::Downloads>,
) -> Result<InstanceRecord, String> {
    let _guard = operations.acquire("modpack-import")?;
    let prepared = match modpacks::prepare_pack(&path)? {
        modpacks::PackPreparation::Ready(prepared) => prepared,
        modpacks::PackPreparation::NeedsOnlineLookup {
            path: pack_path,
            manifest,
        } => {
            /*
             * CurseForge 格式：清单只给 fileID，下载地址与哈希必须联网查。
             *
             * 这里就地补全，而不是把「待补全」状态抛给前端：补全需要 CurseForge key，
             * 而 key 只存在于后端；让前端拿到 fileID 再去另一次 IPC 换地址，等于把
             * 一个本该原子的步骤拆成两段，中间任何一步失败都会留下半完成状态。
             */
            let key = crate::curseforge::read_key_for_internal()
                .await
                .ok_or_else(|| {
                    "这个整合包是 CurseForge 格式，导入需要查询文件下载地址；\
                     请先在设置里配置 CurseForge API Key。"
                        .to_owned()
                })?;
            let ids = manifest.file_ids();
            let details = launcher_core::curseforge::files_by_ids(&key, &ids).await?;
            let (files, skipped) = modpacks::curseforge_files_to_pack_files(&manifest, &details);
            if files.is_empty() {
                return Err(
                    "这个整合包里的文件都无法自动下载（可能都被作者限制为第三方分发）。\
                     请到 CurseForge 网页手动下载。"
                        .into(),
                );
            }
            if skipped > 0 {
                // 跳过是允许的（受限文件、无法归类的资源包），但必须让用户知道数量。
                eprintln!("整合包导入：跳过 {skipped} 个无法自动下载或无法归类的文件");
            }
            let index = manifest.to_pack_index(files);
            modpacks::prepare_from_completed_index(&pack_path, index)?
        }
    };
    let preview = prepared.preview().clone();
    let root = app.path().app_data_dir().map_err(|e| e.to_string())?;
    // Construct the engine before creating a persistent instance, so initialization failures leave no row.
    let engine = engine(store)?;
    let target = store
        .create_instance(
            &CreateInstanceInput {
                name: preview.name.chars().take(48).collect(),
                game_version: preview.minecraft.clone(),
            },
            &root,
        )
        .map_err(|e| e.to_string())?;
    let _instance_guard = operations.acquire(&target.id)?;
    let control = match downloads.start(&target.id, &format!("导入 {}", preview.name), store, app)
    {
        Ok(control) => control,
        Err(error) => {
            let _ = store.discard_uninstalled_instance(&target.id);
            return Err(error);
        }
    };
    let engine = engine.with_control(control);
    let (tx, progress_task) = task_progress(&target.id, app, store, downloads);
    let result = async {
        modpacks::import_prepared_pack(prepared, &target, &engine, Some(tx)).await?;
        // The pending request participates in the import transaction, including rollback on write failure.
        write_pending_loader(&target, &preview.loader)?;
        Ok::<(), String>(())
    }
    .await;
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
    task_saved?;
    Ok(target)
}

/// 从网络下载一个整合包文件，然后走与本地导入完全相同的流程。
///
/// 为什么不直接让前端「先下载到某个目录、再调用 import_modpack」：
/// 中间文件的位置、校验、失败后的清理都该由后端一次性负责。前端只描述「要哪个
/// 版本」，不接触临时文件路径，也就不会出现「下载到了但导入路径写错」这种跨 IPC
/// 的协调问题。
///
/// 下载下来的文件按其在服务器上的扩展名命名（`.mrpack` / `.zip`），
/// `prepare_pack` 依靠容器内容而不是扩展名判断格式，所以命名只影响可读性。
#[tauri::command]
pub async fn download_and_import_modpack(
    instance_hint: Option<String>,
    version: launcher_core::curseforge::CurseForgeVersion,
    app: AppHandle,
    store: State<'_, LauncherStore>,
    operations: State<'_, Operations>,
    downloads: State<'_, Arc<crate::downloads::Downloads>>,
) -> Result<InstanceRecord, String> {
    /*
     * `instance_hint` 只用于给下载任务挂一个可见的归属。
     *
     * 整合包会创建**新**实例，所以这里没有真正意义上的「当前实例」；但下载任务
     * 需要一个 instance_id 才能出现在下载页。用调用方给的 id 是一个折中：任务
     * 会显示在用户当前正在看的实例下面，导入完成后新实例才出现。
     */
    let task_owner = instance_hint.unwrap_or_else(|| "modpack-download".to_owned());

    // 文件名与扩展名的校验都在 `download_to` 内部按内容类型完成（整合包允许
    // `.zip`/`.mrpack`），这里不再重复——重复一份规则只会在两处漂移。

    // 下载到一个独立的临时目录：导入成功后整个目录删掉，失败也不留垃圾。
    let staging = std::env::temp_dir().join(format!("mfmcl-pack-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&staging).map_err(|e| format!("无法创建下载目录：{e}"))?;

    let result = async {
        // 下载阶段不获取 operations 锁：真正需要串行化的是导入（创建实例、写文件），
        // 那是 import_modpack_inner 自己的责任。外层若也去 acquire，同一个 key 会让
        // 被调用的函数直接报「已有任务在运行」——虽然不会卡死，但功能永远失败。
        //
        // engine 必须在 downloads.start 之前构造：先 start 后建 engine 的话，一旦
        // engine 失败（设置读取失败、并发数非法），那条下载任务会永远停在 Running，
        // 之后同一个 instance 的下载都会被拒绝。
        let engine = engine(store.inner())?;
        let control = downloads.start(
            &task_owner,
            &format!("下载整合包 {}", version.file_name),
            store.inner(),
            &app,
        )?;
        let engine = engine.with_control(control);
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

        // 与本地导入共用同一套事务：校验包结构 → 创建实例 → 下载文件 → 失败回滚。
        import_modpack_inner(
            path.into(),
            &app,
            store.inner(),
            &operations,
            downloads.inner(),
        )
        .await
    }
    .await;

    // 无论成败都清理暂存目录。导入流程已经把需要的文件读进内存/实例目录了。
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

fn write_pending_loader(instance: &InstanceRecord, loader: &str) -> Result<(), String> {
    let directory = &instance.instance_dir;
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
        let kind = match kind {
            "fabric" => LoaderKind::Fabric,
            "quilt" => LoaderKind::Quilt,
            _ => return Err("整合包加载器暂不支持。".into()),
        };
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
}
