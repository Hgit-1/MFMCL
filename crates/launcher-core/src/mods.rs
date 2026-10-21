use crate::InstanceRecord;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModFile {
    pub file_name: String,
    pub enabled: bool,
    pub size: u64,
}

fn checked_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 255
        || name.contains(['/', '\\', ':', '\0'])
        || Path::new(name)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        || !(name.ends_with(".jar") || name.ends_with(".jar.disabled"))
    {
        return Err("请选择实例 mods 目录内的 JAR 文件。".into());
    }
    Ok(())
}

fn mods_directory(instance: &InstanceRecord) -> Result<Option<PathBuf>, String> {
    // Check the game directory too: a normal mods directory below a symlink escapes the instance.
    let directory = instance.game_dir.join("mods");
    let directories = [&instance.game_dir, &directory];
    for directory in directories {
        match fs::symlink_metadata(directory) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => return Err("游戏或 mods 目录不能是符号链接或普通文件。".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(Some(instance.game_dir.join("mods")))
}

pub fn list_mods(instance: &InstanceRecord) -> Result<Vec<ModFile>, String> {
    let Some(directory) = mods_directory(instance)? else {
        return Ok(vec![]);
    };
    let mut result = Vec::new();
    let mut names = std::collections::HashSet::new();
    for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let metadata = fs::symlink_metadata(entry.path()).map_err(|e| e.to_string())?;
        if checked_name(&name).is_err() || !metadata.file_type().is_file() {
            continue;
        }
        if !names.insert(name.to_lowercase()) {
            return Err("模组文件名存在大小写冲突。".into());
        }
        result.push(ModFile {
            enabled: name.ends_with(".jar"),
            file_name: name,
            size: metadata.len(),
        });
    }
    result.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    Ok(result)
}

pub fn set_mod_enabled(instance: &InstanceRecord, name: &str, enabled: bool) -> Result<(), String> {
    checked_name(name)?;
    let directory = mods_directory(instance)?.ok_or("mods 目录不存在。")?;
    let source = directory.join(name);
    if !fs::symlink_metadata(&source)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_file()
    {
        return Err("拒绝修改符号链接或非普通文件。".into());
    }
    let target_name = if enabled {
        name.strip_suffix(".disabled").unwrap_or(name).to_owned()
    } else if name.ends_with(".disabled") {
        name.to_owned()
    } else {
        format!("{name}.disabled")
    };
    if target_name == name {
        return Ok(());
    }
    let target = directory.join(&target_name);
    for entry in fs::read_dir(&directory).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let sibling = entry.file_name().to_string_lossy().into_owned();
        if sibling != name && sibling.to_lowercase() == target_name.to_lowercase() {
            return Err("目标文件已经存在或有大小写冲突；没有覆盖任何模组。".into());
        }
    }
    // hard_link atomically refuses an existing target, including dangling symlinks.
    // Unlike rename on Unix this cannot overwrite a file created after the check.
    fs::hard_link(&source, &target).map_err(|e| e.to_string())?;
    if let Err(error) = fs::remove_file(&source) {
        if let Err(rollback) = fs::remove_file(&target) {
            return Err(format!("{error}；回滚失败：{rollback}"));
        }
        return Err(error.to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn instance(root: &Path) -> InstanceRecord {
        InstanceRecord {
            id: "test".into(),
            name: "test".into(),
            game_version: "1.21.4".into(),
            loader: "fabric@0.16.10".into(),
            instance_dir: root.to_owned(),
            game_dir: root.to_owned(),
            isolated: true,
            state: crate::InstanceState::Ready,
            created_at: "".into(),
        }
    }
    #[test]
    fn disallows_path_traversal() {
        for name in ["../a.jar", "x/y.jar", "C:a.jar", "evil.exe", "", "a\0.jar"] {
            assert!(checked_name(name).is_err());
        }
    }
    #[test]
    fn toggle_is_reversible_and_never_overwrites() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("mods")).unwrap();
        fs::write(root.join("mods/a.jar"), b"test").unwrap();
        let instance = instance(&root);
        set_mod_enabled(&instance, "a.jar", false).unwrap();
        assert!(!list_mods(&instance).unwrap()[0].enabled);
        set_mod_enabled(&instance, "a.jar.disabled", true).unwrap();
        assert!(list_mods(&instance).unwrap()[0].enabled);
        fs::write(root.join("mods/a.jar.disabled"), b"other").unwrap();
        assert!(set_mod_enabled(&instance, "a.jar", false).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn symlinks_including_dangling_targets_are_never_followed() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        fs::create_dir(&root).unwrap();
        let instance = instance(&root);
        symlink(root.join("missing"), root.join("mods")).unwrap();
        assert!(list_mods(&instance).is_err());
        assert!(set_mod_enabled(&instance, "a.jar", false).is_err());
        fs::remove_file(root.join("mods")).unwrap();
        fs::create_dir(root.join("mods")).unwrap();
        fs::write(root.join("mods/a.jar"), b"keep").unwrap();
        symlink(root.join("missing"), root.join("mods/a.jar.disabled")).unwrap();
        assert!(set_mod_enabled(&instance, "a.jar", false).is_err());
        assert_eq!(fs::read(root.join("mods/a.jar")).unwrap(), b"keep");
        assert!(set_mod_enabled(&instance, "a.jar.disabled", true).is_err());
        assert_eq!(list_mods(&instance).unwrap().len(), 1);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn case_alias_target_is_not_overwritten() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("mods")).unwrap();
        fs::write(root.join("mods/A.jar"), b"one").unwrap();
        fs::write(root.join("mods/a.jar.disabled"), b"two").unwrap();
        assert!(set_mod_enabled(&instance(&root), "A.jar", false).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
