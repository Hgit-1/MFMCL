//! The non-orchestration halves of installation: loader markers and repair.
//!
//! These were `pub(crate)` helpers inside `src-tauri/src/management.rs`. They
//! touch no UI and no AppHandle, so they belong with the rest of the runtime.

use crate::{Result, RuntimeContext, RuntimeError};
use download_engine::DownloadEngine;
use launcher_core::{InstanceRecord, InstanceState, LauncherStore, loaders};

/// Ask the next install to also install a loader.
///
/// The request is a marker file (`<kind>@<version>`), which
/// [`install_pending_pack_loader`] consumes. Reusing the modpack mechanism means
/// "loader chosen while creating an instance" and "loader required by a pack"
/// follow one code path — including rollback and progress.
pub fn request_pack_loader(instance: &InstanceRecord, request: &str) -> Result<()> {
    let (kind, version) = request
        .split_once('@')
        .ok_or_else(|| RuntimeError::message("加载器标识格式无效。"))?;
    let kind = loaders::LoaderKind::from_name(kind)
        .ok_or_else(|| RuntimeError::message("未识别的加载器。"))?;
    loaders::safe_version(version).map_err(RuntimeError::message)?;
    write_pending_loader(instance, &format!("{}@{version}", kind.name()))
}

fn write_pending_loader(instance: &InstanceRecord, loader: &str) -> Result<()> {
    let directory = &instance.instance_dir;
    /*
     * The record is written before any directory exists: `create_instance` only
     * picks a free path and inserts a row. Create it here so a loader requested at
     * creation time has somewhere to land — without this, "create an instance with
     * NeoForge" failed before downloading anything, because writing the marker hit
     * a missing directory.
     */
    std::fs::create_dir_all(directory).map_err(|error| {
        RuntimeError::Io(format!("无法创建实例目录 {}：{error}", directory.display()))
    })?;
    let metadata = std::fs::symlink_metadata(directory)
        .map_err(|error| RuntimeError::Io(error.to_string()))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(RuntimeError::message("实例目录不能是符号链接或普通文件。"));
    }
    let marker = directory.join("pack-loader.txt");
    let temporary = directory.join(format!(".pack-loader-{}.pending", uuid::Uuid::new_v4()));

    /*
     * An existing marker is not an error: it means the loader was already requested
     * — once when the instance was created, again when the install was started —
     * and the later request supersedes the earlier one. Before this, creating an
     * instance with `--mod-loader` failed outright with "当文件已存在时，无法创建该
     * 文件" (os error 183), because the second write tried to hard-link onto a
     * marker that the first write had already put there.
     *
     * Only a *fresh* request is an error worth reporting, and that case is still
     * caught by the hard_link below when no removal happened.
     */
    match std::fs::remove_file(&marker) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(RuntimeError::message(format!(
                "无法替换已有的加载器请求标记：{error}"
            )));
        }
    }

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| RuntimeError::Io(error.to_string()))?;
    /*
     * Write-then-link, not write-in-place. `hard_link` fails if the destination
     * already exists, which is what makes "two installs raced to write the
     * marker" impossible to resolve into a half-written file.
     */
    let result = (|| {
        use std::io::Write;
        file.write_all(loader.as_bytes())
            .map_err(|error| RuntimeError::Io(error.to_string()))?;
        file.sync_all()
            .map_err(|error| RuntimeError::Io(error.to_string()))?;
        std::fs::hard_link(&temporary, &marker)
            .map_err(|error| RuntimeError::message(format!("无法创建加载器请求标记：{error}")))?;
        Ok::<(), RuntimeError>(())
    })();
    drop(file);
    if let Err(mut error) = result {
        if let Err(cleanup) = std::fs::remove_file(&temporary) {
            error = RuntimeError::message(format!("{error}；清理临时标记失败：{cleanup}"));
        }
        return Err(error);
    }
    std::fs::remove_file(temporary).map_err(|error| {
        RuntimeError::message(format!("加载器请求已写入，但临时文件清理失败：{error}"))
    })
}

/// Install the loader a marker file asks for, if there is one.
pub async fn install_pending_pack_loader(
    context: &RuntimeContext,
    instance: &InstanceRecord,
    store: &LauncherStore,
    engine: &DownloadEngine,
) -> Result<()> {
    let path = instance.instance_dir.join("pack-loader.txt");
    let marker = match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() => metadata,
        Ok(_) => {
            return Err(RuntimeError::message(
                "加载器请求标记必须是普通文件，不能是符号链接。",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(RuntimeError::Io(error.to_string())),
    };
    if marker.len() > 256 {
        return Err(RuntimeError::message("整合包加载器请求无效。"));
    }
    let request = tokio::fs::read_to_string(&path)
        .await
        .map_err(|error| RuntimeError::Io(error.to_string()))?;
    if request == "vanilla" {
        if instance.loader != "vanilla" {
            return Err(RuntimeError::message("整合包加载器请求与实例记录不一致。"));
        }
    } else {
        let (kind, version) = request
            .split_once('@')
            .ok_or_else(|| RuntimeError::message("整合包加载器请求无效。"))?;
        let kind = loaders::LoaderKind::from_name(kind)
            .ok_or_else(|| RuntimeError::message("整合包加载器暂不支持。"))?;
        loaders::safe_version(version).map_err(RuntimeError::message)?;
        /*
         * Re-read the record: the caller's copy may predate this install, and
         * `install_vanilla` just changed the instance state.
         */
        let latest = store
            .list_instances()?
            .into_iter()
            .find(|record| record.id == instance.id)
            .ok_or_else(|| RuntimeError::message("找不到实例。"))?;
        if latest.loader == format!("{}@{version}", kind.name()) {
            if !loaders::loader_overlay_is_installed(&latest, kind, version)
                .map_err(RuntimeError::message)?
            {
                return Err(RuntimeError::message(
                    "实例已记录加载器，但对应配置不存在或无效。",
                ));
            }
        } else if latest.loader == "vanilla" {
            let mut target = latest;
            target.state = InstanceState::Ready;
            loaders::install_loader(
                context.data_root(),
                &target,
                store,
                engine,
                kind,
                version,
                None,
            )
            .await
            .map_err(RuntimeError::message)?;
        } else {
            return Err(RuntimeError::message(
                "整合包加载器请求与已安装加载器不一致。",
            ));
        }
    }
    tokio::fs::remove_file(path).await.map_err(|error| {
        RuntimeError::message(format!("加载器已处理，但请求标记无法移除：{error}"))
    })
}

/// Reinstall the loader the instance already records.
///
/// Only called when [`loaders::diagnose_loader`] reports `Broken`. The instance
/// is temporarily treated as ready because `install_loader` refuses a
/// non-installed instance, and a broken-but-present overlay is exactly the case
/// this repairs.
pub async fn repair_loader(
    context: &RuntimeContext,
    instance: &InstanceRecord,
    store: &LauncherStore,
    engine: &DownloadEngine,
) -> Result<()> {
    let Some((name, version)) = instance.loader.split_once('@') else {
        return Err(RuntimeError::message(format!(
            "加载器记录格式无效：{}",
            instance.loader
        )));
    };
    let Some(kind) = loaders::LoaderKind::from_name(name) else {
        return Err(RuntimeError::message(format!("未识别的加载器：{name}")));
    };
    let latest = store
        .list_instances()?
        .into_iter()
        .find(|record| record.id == instance.id)
        .ok_or_else(|| RuntimeError::message("找不到实例。"))?;
    let mut target = latest;
    target.state = InstanceState::Ready;
    loaders::reinstall_loader(
        context.data_root(),
        &target,
        store,
        engine,
        kind,
        version,
        None,
    )
    .await
    .map_err(RuntimeError::message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use launcher_core::{CreateInstanceInput, LauncherSettings};

    fn fixture() -> (std::path::PathBuf, LauncherStore, InstanceRecord) {
        let root = std::env::temp_dir().join(format!("mfmcl-marker-{}", uuid::Uuid::new_v4()));
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

    #[test]
    fn a_second_request_replaces_the_first_instead_of_failing() {
        /*
         * The real sequence for `download --mod-loader fabric` writes the marker
         * twice: once when the record is created, once before the install. The
         * second write used to fail with os error 183 ("cannot create a file when
         * that file already exists"), because the hard_link target was already
         * there — so creating an instance with a loader never worked from the CLI.
         */
        let (root, _store, instance) = fixture();
        request_pack_loader(&instance, "fabric@0.16.10").unwrap();
        request_pack_loader(&instance, "fabric@0.20.1").unwrap();
        let marker = instance.instance_dir.join("pack-loader.txt");
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap(),
            "fabric@0.20.1",
            "the later request must win"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_request_creates_the_instance_directory() {
        // `create_instance` only picks a path and inserts a row; the directory itself
        // does not exist yet. Writing the marker must not assume otherwise.
        let (root, _store, instance) = fixture();
        let _ = std::fs::remove_dir_all(&instance.instance_dir);
        request_pack_loader(&instance, "fabric@0.20.1").unwrap();
        assert!(instance.instance_dir.join("pack-loader.txt").is_file());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_valid_request_writes_a_normalised_marker() {
        let (root, _store, instance) = fixture();
        // "NeoForge" casing and a bare version must normalise to `neoforge@…`.
        request_pack_loader(&instance, "neoforge@21.1.80").unwrap();
        let marker = instance.instance_dir.join("pack-loader.txt");
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap(),
            "neoforge@21.1.80"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_request_without_a_version_is_rejected_before_writing() {
        let (root, _store, instance) = fixture();
        let error = request_pack_loader(&instance, "neoforge").unwrap_err();
        assert!(error.to_string().contains("格式无效"), "got {error}");
        assert!(
            !instance.instance_dir.join("pack-loader.txt").exists(),
            "a rejected request must leave no marker"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_unknown_loader_name_is_rejected() {
        let (root, _store, instance) = fixture();
        let error = request_pack_loader(&instance, "risugami@1.0").unwrap_err();
        assert!(error.to_string().contains("未识别"), "got {error}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_path_traversal_version_is_rejected() {
        let (root, _store, instance) = fixture();
        // The version becomes part of a directory name downstream; `..` there
        // would escape the instance root.
        let error = request_pack_loader(&instance, "fabric@../../etc").unwrap_err();
        assert!(!error.to_string().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }
}
