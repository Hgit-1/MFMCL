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

fn task_progress(
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

fn instance(store: &LauncherStore, id: &str) -> Result<InstanceRecord, String> {
    store
        .list_instances()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|i| i.id == id)
        .ok_or("找不到实例。".into())
}
fn engine(store: &LauncherStore) -> Result<DownloadEngine, String> {
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
    {
        if !game_real.starts_with(&target_real) {
            // Isolated instances keep their game dir inside the instance dir; if
            // anything points outside, leave the parent removal to the user.
            return Err("已删除实例记录；游戏目录在实例目录之外，请手动确认后删除。".into());
        }
    }
    tokio::fs::remove_dir_all(&removed.instance_dir)
        .await
        .map_err(|e| format!("已删除实例记录，但移除目录失败：{e}"))
}

#[tauri::command]
pub async fn search_mods(
    instance_id: String,
    query: String,
    store: State<'_, LauncherStore>,
    operations: State<'_, Operations>,
) -> Result<Vec<launcher_core::CatalogMod>, String> {
    let _guard = operations.acquire(&instance_id)?;
    let target = instance(&store, &instance_id)?;
    let client = launcher_core::catalog::catalog_client()?;
    launcher_core::catalog::search_mods(&client, &target, &query).await
}

#[tauri::command]
pub async fn list_mod_versions(
    instance_id: String,
    project_id: String,
    store: State<'_, LauncherStore>,
    operations: State<'_, Operations>,
) -> Result<Vec<launcher_core::CatalogVersion>, String> {
    let _guard = operations.acquire(&instance_id)?;
    let target = instance(&store, &instance_id)?;
    let client = launcher_core::catalog::catalog_client()?;
    launcher_core::catalog::mod_versions(&client, &target, &project_id).await
}

#[tauri::command]
pub async fn install_mod_version(
    instance_id: String,
    version: launcher_core::CatalogVersion,
    app: AppHandle,
    store: State<'_, LauncherStore>,
    operations: State<'_, Operations>,
    downloads: State<'_, Arc<crate::downloads::Downloads>>,
) -> Result<String, String> {
    let _guard = operations.acquire(&instance_id)?;
    let target = instance(&store, &instance_id)?;
    let control = downloads.start(
        &instance_id,
        &format!("安装模组 {}", version.file_name),
        &store,
        &app,
    )?;
    let engine = engine(&store)?.with_control(control);
    let (tx, progress_task) = task_progress(&instance_id, &app, store.inner(), downloads.inner());
    let result = launcher_core::catalog::install_mod_version(
        &version,
        &target.game_dir.join("mods"),
        &engine,
    )
    .await;
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
    let _guard = operations.acquire("modpack-import")?;
    let path = PathBuf::from(path);
    let prepared = modpacks::prepare_pack(&path)?;
    let preview = prepared.preview().clone();
    let root = app.path().app_data_dir().map_err(|e| e.to_string())?;
    // Construct the engine before creating a persistent instance, so initialization failures leave no row.
    let engine = engine(&store)?;
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
    let control = match downloads.start(&target.id, &format!("导入 {}", preview.name), &store, &app)
    {
        Ok(control) => control,
        Err(error) => {
            let _ = store.discard_uninstalled_instance(&target.id);
            return Err(error);
        }
    };
    let engine = engine.with_control(control);
    let (tx, progress_task) = task_progress(&target.id, &app, &store, &downloads);
    let result = async {
        modpacks::import_prepared_pack(prepared, &target, &engine, Some(tx)).await?;
        // The pending request participates in the import transaction, including rollback on write failure.
        write_pending_loader(&target, &preview.loader)?;
        Ok::<(), String>(())
    }
    .await;
    let _ = progress_task.await;
    let task_saved = downloads.finish(&target.id, result.is_ok(), &store, &app);
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
