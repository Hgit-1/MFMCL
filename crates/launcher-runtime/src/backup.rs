//! Backup creation and restore, with the privacy confirmation moved to the caller.
//!
//! The desktop handlers asked for `privacy_confirmed: bool`. That flag is user
//! consent, not runtime logic, so it stays in the frontend: the CLI asks with a
//! prompt or a `--yes`, the GUI uses a dialog. Doing it here would force both to
//! share one interaction style.

use crate::{Result, RuntimeContext, RuntimeError};
use launcher_core::{
    CreateInstanceInput, InstanceRecord, LauncherStore,
    backups::{self, BackupPreview},
};
use std::path::PathBuf;

/// Seven days of "is this a backup directory" sanity is out of scope; what matters
/// is that the caller passes an absolute path and we do not invent one.
fn absolute(path: &str) -> Result<PathBuf> {
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err(RuntimeError::message(format!(
            "备份目录必须是绝对路径：{}",
            path.display()
        )));
    }
    Ok(path)
}

/// Write a backup of one instance into `directory`.
pub async fn create(store: &LauncherStore, instance_id: &str, directory: &str) -> Result<PathBuf> {
    let directory = absolute(directory)?;
    let target = crate::install::find_instance(store, instance_id)?;
    tokio::task::spawn_blocking(move || backups::create_backup(&target, &directory))
        .await
        .map_err(|_| RuntimeError::message("备份任务未能完成。"))?
        .map_err(RuntimeError::message)
}

/// Inspect a backup without extracting it.
pub async fn preview(path: &str) -> Result<BackupPreview> {
    let path = absolute(path)?;
    tokio::task::spawn_blocking(move || backups::preview_backup(&path))
        .await
        .map_err(|_| RuntimeError::message("备份预览任务未能完成。"))?
        .map_err(RuntimeError::message)
}

/// Restore a backup as a **new** instance.
///
/// Restoring in place would overwrite saves the user may still want. A new
/// instance also means a failed restore cannot damage a working one.
///
/// `archive_sha256` must come from [`preview`]; it is what binds the confirmation
/// the user saw to the bytes actually extracted.
pub async fn restore(
    context: &RuntimeContext,
    store: &LauncherStore,
    path: &str,
    archive_sha256: &str,
    name: &str,
) -> Result<InstanceRecord> {
    let path = absolute(path)?;
    if archive_sha256.len() != 64 || !archive_sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(RuntimeError::message("备份预览校验值无效，请重新选择。"));
    }
    let store = store.clone();
    let root = context.data_root().to_path_buf();
    let archive_sha256 = archive_sha256.to_owned();
    let name = name.to_owned();
    tokio::task::spawn_blocking(move || {
        let prepared =
            backups::prepare_backup(&path, Some(&archive_sha256)).map_err(RuntimeError::message)?;
        /*
         * `create_instance_in` with no game root, not `create_instance`:
         * `restore_backup` extracts into `<instance_dir>/game` and refuses any
         * other layout. `create_instance` would resolve the shared `~/.minecraft`
         * root from settings and produce `<root>/shared`, so every restore failed
         * with "恢复目标必须使用隔离的默认游戏目录".
         */
        let target = store
            .create_instance_in(
                &CreateInstanceInput {
                    name,
                    game_version: prepared.preview().minecraft.clone(),
                },
                &root,
                None,
            )
            .map_err(RuntimeError::from)?;
        if let Err(mut error) = backups::restore_backup(prepared, &target) {
            /*
             * Roll the record back if the extraction failed: an instance that
             * claims to be restored but has half its files is worse than none,
             * because the user has no way to tell.
             */
            if let Err(cleanup) = store.discard_uninstalled_instance(&target.id) {
                error = format!("{error}；实例记录清理失败：{cleanup}");
            }
            return Err(RuntimeError::message(error));
        }
        Ok(target)
    })
    .await
    .map_err(|_| RuntimeError::message("备份恢复任务未能完成。"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use launcher_core::{CreateInstanceInput, LauncherSettings};

    fn fixture() -> (PathBuf, LauncherStore, InstanceRecord) {
        let root = std::env::temp_dir().join(format!("mfmcl-backup-{}", uuid::Uuid::new_v4()));
        let store = LauncherStore::open(root.join("mfmcl.sqlite")).unwrap();
        store.save_settings(&LauncherSettings::default()).unwrap();
        let instance = store
            .create_instance_in(
                &CreateInstanceInput {
                    name: "probe".into(),
                    game_version: "1.21.4".into(),
                },
                &root,
                None,
            )
            .unwrap();
        (root, store, instance)
    }

    #[tokio::test]
    async fn a_relative_backup_directory_is_refused() {
        let (root, store, instance) = fixture();
        let error = create(&store, &instance.id, "relative/backups")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("绝对路径"), "got {error}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn restoring_without_a_valid_hash_is_refused() {
        let (root, store, _instance) = fixture();
        let context = RuntimeContext::new(&root);
        let archive = root.join("backup.zip");
        let error = restore(
            &context,
            &store,
            &archive.to_string_lossy(),
            "not-a-hash",
            "restored",
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("校验值"), "got {error}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_round_trip_creates_a_second_independent_instance() {
        let (root, store, instance) = fixture();
        let context = RuntimeContext::new(&root);
        // Write one file that the backup format permits.
        std::fs::create_dir_all(instance.game_dir.join("saves")).unwrap();
        std::fs::write(instance.game_dir.join("saves/level.dat"), b"world").unwrap();
        std::fs::write(instance.game_dir.join("options.txt"), b"lang:zh_cn").unwrap();

        let directory = root.join("backups");
        std::fs::create_dir_all(&directory).unwrap();
        let archive = create(&store, &instance.id, &directory.to_string_lossy())
            .await
            .unwrap();
        let preview = preview(&archive.to_string_lossy()).await.unwrap();
        assert!(preview.file_count >= 2, "got {}", preview.file_count);

        let restored = restore(
            &context,
            &store,
            &archive.to_string_lossy(),
            &preview.archive_sha256,
            "restored",
        )
        .await
        .unwrap();
        assert_ne!(restored.id, instance.id, "restore must not reuse the id");
        assert_eq!(
            std::fs::read(restored.game_dir.join("saves/level.dat")).unwrap(),
            b"world"
        );
        assert_eq!(store.list_instances().unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(root);
    }
}
