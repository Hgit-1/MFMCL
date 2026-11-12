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
fn restore_to_new_instance(
    store: &LauncherStore,
    root: &std::path::Path,
    path: &std::path::Path,
    archive_sha256: &str,
    name: String,
) -> Result<InstanceRecord, String> {
    let prepared = backups::prepare_backup(path, Some(archive_sha256))?;
    let target = store
        .create_isolated_instance(
            &CreateInstanceInput {
                name,
                game_version: prepared.preview().minecraft.clone(),
            },
            root,
        )
        .map_err(|e| e.to_string())?;
    if let Err(mut error) = backups::restore_backup(prepared, &target) {
        if let Err(cleanup) = store.discard_uninstalled_instance(&target.id) {
            error = format!("{error}；实例记录清理失败：{cleanup}");
        }
        return Err(error);
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_restore_creates_a_private_instance_and_preserves_source() {
        let root =
            std::env::temp_dir().join(format!("mfmcl-desktop-restore-{}", uuid::Uuid::new_v4()));
        let store = LauncherStore::open(":memory:").unwrap();
        let original = store
            .create_isolated_instance(
                &CreateInstanceInput {
                    name: "Original".into(),
                    game_version: "1.21.4".into(),
                },
                &root,
            )
            .unwrap();
        std::fs::create_dir_all(original.game_dir.join("saves/World")).unwrap();
        std::fs::write(original.game_dir.join("saves/World/level.dat"), b"world").unwrap();
        let path = backups::create_backup(&original, &root).unwrap();
        let preview = backups::preview_backup(&path).unwrap();
        let restored = restore_to_new_instance(
            &store,
            &root,
            &path,
            &preview.archive_sha256,
            "Restored".into(),
        )
        .unwrap();
        assert!(restored.isolated);
        assert_eq!(restored.game_dir, restored.instance_dir.join("game"));
        assert_ne!(restored.game_dir, original.game_dir);
        for game in [&restored.game_dir, &original.game_dir] {
            assert_eq!(
                std::fs::read(game.join("saves/World/level.dat")).unwrap(),
                b"world"
            );
        }
        assert_eq!(
            std::fs::read_to_string(restored.instance_dir.join("pack-loader.txt")).unwrap(),
            "vanilla"
        );
        assert!(
            restore_to_new_instance(&store, &root, &path, &"0".repeat(64), "Bad".into()).is_err()
        );
        assert_eq!(store.list_instances().unwrap().len(), 2);
        std::fs::remove_dir_all(root).unwrap();
    }
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
        restore_to_new_instance(&store, &root, &PathBuf::from(path), &archive_sha256, name)
    })
    .await
    .map_err(|_| "备份恢复任务未能完成。".to_owned())?
}
