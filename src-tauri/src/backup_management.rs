//! User-authorized local backups; restore always creates a new isolated instance.
use crate::management::Operations;
use launcher_core::{
    CreateInstanceInput, InstanceRecord, LauncherStore,
    backups::{self, BackupPreview},
};
use std::path::PathBuf;
use tauri::{AppHandle, Manager, State};

#[tauri::command]
pub async fn create_backup(
    instance_id: String,
    directory: String,
    privacy_confirmed: bool,
    store: State<'_, LauncherStore>,
    operations: State<'_, Operations>,
) -> Result<String, String> {
    if !privacy_confirmed {
        return Err("请先确认本地备份可能包含存档、服务器地址和模组配置中的私人信息。".into());
    }
    let guard = operations.acquire(&instance_id)?;
    let target = store
        .list_instances()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|i| i.id == instance_id)
        .ok_or("找不到实例。")?;
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        backups::create_backup(&target, &PathBuf::from(directory))
            .map(|p| p.to_string_lossy().into_owned())
    })
    .await
    .map_err(|_| "备份任务未能完成。".to_owned())?
}
#[tauri::command]
pub async fn preview_backup(path: String) -> Result<BackupPreview, String> {
    tauri::async_runtime::spawn_blocking(move || backups::preview_backup(&PathBuf::from(path)))
        .await
        .map_err(|_| "备份预览任务未能完成。".to_owned())?
}
#[tauri::command]
pub async fn restore_backup(
    path: String,
    archive_sha256: String,
    name: String,
    confirmed: bool,
    app: AppHandle,
    store: State<'_, LauncherStore>,
    operations: State<'_, Operations>,
) -> Result<InstanceRecord, String> {
    if !confirmed {
        return Err("请先确认将此备份恢复为新的独立实例。".into());
    }
    if archive_sha256.len() != 64 || !archive_sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("备份预览校验值无效，请重新选择。".into());
    }
    let guard = operations.acquire("backup-restore")?;
    let root = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        let prepared = backups::prepare_backup(&PathBuf::from(path), Some(&archive_sha256))?;
        let target = store
            .create_instance(
                &CreateInstanceInput {
                    name,
                    game_version: prepared.preview().minecraft.clone(),
                },
                &root,
            )
            .map_err(|e| e.to_string())?;
        if let Err(mut error) = backups::restore_backup(prepared, &target) {
            if let Err(cleanup) = store.discard_uninstalled_instance(&target.id) {
                error = format!("{error}；实例记录清理失败：{cleanup}");
            }
            return Err(error);
        }
        Ok(target)
    })
    .await
    .map_err(|_| "备份恢复任务未能完成。".to_owned())?
}
