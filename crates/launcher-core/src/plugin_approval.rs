//! Authoritative third-party plugin definitions shared by desktop and CLI.
use crate::LauncherStore;
use crate::plugins::{Contribution, PluginDefinition, PluginState};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub const PACKAGE_ID_FILE: &str = ".mfmcl-package-id";
pub const MAX_MANIFEST_BYTES: u64 = 512 * 1024;

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub fn bounded_file(path: &Path, limit: u64, too_large: &'static str) -> io::Result<Vec<u8>> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(invalid("请选择普通文件，而非目录或符号链接。"));
    }
    if metadata.len() > limit {
        return Err(invalid(too_large));
    }
    let file = std::fs::File::open(path)?;
    let opened = file.metadata()?;
    if !opened.is_file() || opened.len() > limit {
        return Err(invalid(too_large));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(invalid(too_large));
    }
    Ok(bytes)
}

pub fn external_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
        && !id.contains("..")
        && !["launch-logger", "download-mirror"]
            .iter()
            .any(|builtin| builtin.eq_ignore_ascii_case(id))
}

pub fn read_installed(root: &Path, id: &str) -> io::Result<(Value, PathBuf)> {
    if !external_id(id) {
        return Err(invalid("插件 ID 不合法或已被内置插件占用。"));
    }
    let directory = root.join(id);
    if !std::fs::symlink_metadata(&directory)?.file_type().is_dir() {
        return Err(invalid("插件目录必须为普通目录。"));
    }
    let bytes = bounded_file(
        &directory.join("manifest.json"),
        MAX_MANIFEST_BYTES,
        "插件清单过大。",
    )?;
    let manifest: Value = serde_json::from_slice(&bytes)?;
    if manifest.get("id").and_then(Value::as_str).map(str::trim) != Some(id) {
        return Err(invalid("插件清单 ID 与目录不匹配。"));
    }
    Ok((manifest, directory))
}

pub fn review_digest(directory: &Path, manifest: &Value) -> io::Result<String> {
    let identity_path = directory.join(PACKAGE_ID_FILE);
    let identity = match std::fs::symlink_metadata(&identity_path) {
        Ok(_) => bounded_file(&identity_path, 128, "插件包标识过大。")?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error),
    };
    let mut digest = Sha256::new();
    digest.update((identity.len() as u64).to_le_bytes());
    digest.update(identity);
    digest.update(manifest.to_string());
    Ok(hex::encode(digest.finalize()))
}

pub fn package_definition(id: &str, manifest: &Value) -> Option<PluginDefinition> {
    let version = manifest.get("version")?.as_str()?.trim();
    if version.is_empty() || version.len() > 24 {
        return None;
    }
    let strings = |key| {
        manifest
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    };
    let contributions = manifest
        .get("contributions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| serde_json::from_value::<Contribution>(item.clone()).ok())
        .filter(|item| !item.value.trim().is_empty())
        .collect();
    Some(PluginDefinition {
        id: id.to_owned(),
        version: version.to_owned(),
        hooks: strings("hooks"),
        permissions: strings("permissions"),
        contributions,
        incompatible: false,
    })
}

pub fn explicit_cli_key(id: &str) -> String {
    format!("cli:{id}")
}

pub fn definition_digest(definition: &PluginDefinition) -> String {
    hex::encode(Sha256::digest(
        serde_json::to_vec(definition).expect("serializable definition"),
    ))
}

/// Legacy mirrored frontend rows are untrusted and never execute. Definitions
/// explicitly configured by the CLI have a separate approval namespace.
pub fn approved_state(store: &LauncherStore, data_root: &Path) -> PluginState {
    let mut state = store.load_plugin_state().unwrap_or_default();
    state.custom.retain(|definition| {
        external_id(&definition.id)
            && store
                .plugin_approval_matches(
                    &explicit_cli_key(&definition.id),
                    &definition_digest(definition),
                )
                .unwrap_or(false)
    });
    let root = data_root.join("plugins");
    let Ok(entries) = std::fs::read_dir(&root) else {
        return state;
    };
    for entry in entries.flatten() {
        let Some(id) = entry.file_name().into_string().ok() else {
            continue;
        };
        let Ok((manifest, directory)) = read_installed(&root, &id) else {
            continue;
        };
        // An installed package always takes precedence over an identically named CLI entry.
        state.custom.retain(|plugin| plugin.id != id);
        let Ok(digest) = review_digest(&directory, &manifest) else {
            continue;
        };
        if !store.plugin_approval_matches(&id, &digest).unwrap_or(false) {
            continue;
        }
        if let Some(definition) = package_definition(&id, &manifest) {
            state.enabled.entry(id).or_insert(
                manifest
                    .get("enabledByDefault")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
            );
            state.custom.push(definition);
        }
    }
    state.custom.sort_by(|a, b| a.id.cmp(&b.id));
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::ContributionKind;

    #[test]
    fn installed_packages_and_legacy_frontend_rows_stay_inactive_until_exact_approval() {
        let root =
            std::env::temp_dir().join(format!("mfmcl-plugin-approval-{}", uuid::Uuid::new_v4()));
        let directory = root.join("plugins/review-test");
        std::fs::create_dir_all(&directory).unwrap();
        let db = root.join("mfmcl.sqlite");
        let store = LauncherStore::open(&db).unwrap();
        let id = "review-test";
        let mut manifest = serde_json::json!({
            "id": id, "version": "1.0", "hooks": ["launch"], "permissions": ["launchHook"],
            "contributions": [{"kind":"env", "value":"MFMCL_REVIEW_TEST=1"}]
        });
        let manifest_path = directory.join("manifest.json");
        std::fs::write(&manifest_path, manifest.to_string()).unwrap();
        std::fs::write(directory.join(PACKAGE_ID_FILE), "package-one").unwrap();
        let mut old_state = PluginState::default();
        old_state.enabled.insert(id.into(), true);
        old_state
            .custom
            .push(package_definition(id, &manifest).unwrap());
        store.save_plugin_state(&old_state).unwrap();
        drop(store);
        let store = LauncherStore::open(&db).unwrap();
        assert!(approved_state(&store, &root).custom.is_empty());
        let digest = review_digest(&directory, &manifest).unwrap();
        store.approve_plugin_package(id, &digest).unwrap();
        let state = approved_state(&store, &root);
        let approved = &state.custom[0];
        assert!(matches!(
            approved.contributions[0].kind,
            ContributionKind::Env
        ));
        assert!(
            crate::plugins::contribution_verdict(approved, true, &approved.contributions[0])
                .is_ok()
        );
        manifest["contributions"][0]["value"] = "MFMCL_REVIEW_TEST=2".into();
        std::fs::write(&manifest_path, manifest.to_string()).unwrap();
        assert!(approved_state(&store, &root).custom.is_empty());
        let digest = review_digest(&directory, &manifest).unwrap();
        store.approve_plugin_package(id, &digest).unwrap();
        std::fs::write(directory.join(PACKAGE_ID_FILE), "package-two").unwrap();
        assert!(approved_state(&store, &root).custom.is_empty());
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
