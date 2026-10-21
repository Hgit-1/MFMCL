//! Mod update checks against Modrinth, with hash-verified replacement.
//!
//! Interaction principles follow PCL2's public update wording (risk warning,
//! save-backup reminder, recoverable old file); the implementation is original.
//! Checking sends installed mod-file SHA-1 hashes to api.modrinth.com; the UI
//! must disclose that. Downloads come only from cdn.modrinth.com and are
//! verified against the SHA-1 from Modrinth's metadata before publication.
use crate::InstanceRecord;
use download_engine::{ArtifactSpec, DownloadEngine};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::{collections::HashMap, path::Path, time::Duration};

const API: &str = "https://api.modrinth.com/v2";
const MAX_MODS_PER_CHECK: usize = 100;
const MAX_RESPONSE: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModUpdate {
    pub file_name: String,
    pub project_id: String,
    pub installed_version: String,
    pub installed_version_id: String,
    pub new_version: String,
    pub new_version_id: String,
    pub new_file_name: String,
    pub url: String,
    pub sha1: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModUpdateReport {
    pub file_name: String,
    pub status: ModUpdateStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModUpdateStatus {
    UpToDate,
    UpdateAvailable(Box<ModUpdate>),
    UnknownSource,
    Unsupported(String),
}

#[derive(Debug, Clone, Deserialize)]
struct ModrinthVersion {
    id: String,
    project_id: String,
    version_number: String,
    date_created: String,
    #[serde(default)]
    files: Vec<ModrinthFile>,
}

#[derive(Debug, Clone, Deserialize)]
struct ModrinthFile {
    url: String,
    filename: String,
    #[serde(default)]
    primary: bool,
    size: u64,
    #[serde(default)]
    hashes: HashMap<String, String>,
}

pub fn instance_loader_name(instance: &InstanceRecord) -> Option<&'static str> {
    match instance.loader.split_once('@') {
        Some(("fabric", _)) => Some("fabric"),
        Some(("quilt", _)) => Some("quilt"),
        _ => None,
    }
}

/// Convenience wrapper for the desktop host: bounded, HTTPS-only client.
pub fn modrinth_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("MFMCL/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| "无法初始化更新检查客户端。".to_owned())
}

async fn bounded(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length as usize > limit)
    {
        return Err("更新服务响应过大。".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "更新服务读取失败。")? {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err("更新服务响应过大。".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

pub fn file_sha1(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha1::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Check installed mods for newer Modrinth releases matching the instance's
/// game version and loader. Only `.jar` files are considered. The caller has
/// already disclosed that file hashes are sent to Modrinth for this lookup.
pub async fn check_mod_updates(
    client: &reqwest::Client,
    instance: &InstanceRecord,
    mods_dir: &Path,
    mod_files: &[String],
) -> Result<Vec<ModUpdateReport>, String> {
    let Some(loader) = instance_loader_name(instance) else {
        return Err("模组更新检查需要 Fabric 或 Quilt 实例。".into());
    };
    let jars: Vec<&String> = mod_files
        .iter()
        .filter(|name| name.ends_with(".jar"))
        .collect();
    if jars.is_empty() {
        return Ok(vec![]);
    }
    if jars.len() > MAX_MODS_PER_CHECK {
        return Err("模组数量超过单次检查上限（100）。".into());
    }
    let mut hash_to_name = HashMap::new();
    let mut hashes = Vec::with_capacity(jars.len());
    for name in &jars {
        let hash = file_sha1(&mods_dir.join(name))?;
        hashes.push(hash.clone());
        hash_to_name.insert(hash, name.as_str());
    }
    let response = client
        .post(format!("{API}/version_files"))
        .json(&serde_json::json!({ "algorithm": "sha1", "hashes": hashes }))
        .send()
        .await
        .map_err(|_| "无法连接 Modrinth。")?
        .error_for_status()
        .map_err(|_| "Modrinth 拒绝了更新查询。")?;
    let bytes = bounded(response, MAX_RESPONSE).await?;
    let found: HashMap<String, Option<ModrinthVersion>> =
        serde_json::from_slice(&bytes).map_err(|_| "Modrinth 返回的数据无效。")?;

    // Installed version per mod file; one project query per unique project.
    let mut installed_by_name: HashMap<&str, ModrinthVersion> = HashMap::new();
    let mut project_ids: Vec<String> = Vec::new();
    let mut reports = Vec::new();
    for (hash, name) in &hash_to_name {
        if let Some(version) = found.get(hash.as_str()).cloned().flatten() {
            if !project_ids.contains(&version.project_id) {
                project_ids.push(version.project_id.clone());
            }
            installed_by_name.insert(name, version);
        }
    }

    let mut latest_by_project: HashMap<String, ModrinthVersion> = HashMap::new();
    for project_id in &project_ids {
        let query = format!(
            "{API}/project/{project_id}/version?loaders=[\"{loader}\"]&game_versions=[\"{}\"]",
            instance.game_version
        );
        let response = client
            .get(&query)
            .send()
            .await
            .map_err(|_| "无法连接 Modrinth。")?
            .error_for_status()
            .map_err(|_| "Modrinth 项目查询失败。")?;
        let bytes = bounded(response, MAX_RESPONSE).await?;
        let versions: Vec<ModrinthVersion> =
            serde_json::from_slice(&bytes).map_err(|_| "Modrinth 版本列表无效。")?;
        if let Some(newest) = pick_newest(&versions) {
            latest_by_project.insert(project_id.clone(), newest.clone());
        }
    }

    for name in &jars {
        let Some(installed) = installed_by_name.get(name.as_str()) else {
            reports.push(ModUpdateReport {
                file_name: (*name).to_owned(),
                status: ModUpdateStatus::UnknownSource,
            });
            continue;
        };
        let status = match latest_by_project.get(&installed.project_id) {
            Some(newest) if newest.id != installed.id => match release_file(newest) {
                Ok(file) => ModUpdateStatus::UpdateAvailable(Box::new(ModUpdate {
                    file_name: (*name).to_owned(),
                    project_id: installed.project_id.clone(),
                    installed_version: installed.version_number.clone(),
                    installed_version_id: installed.id.clone(),
                    new_version: newest.version_number.clone(),
                    new_version_id: newest.id.clone(),
                    new_file_name: file.filename,
                    url: file.url,
                    sha1: file.hashes["sha1"].clone(),
                    size: file.size,
                })),
                Err(reason) => ModUpdateStatus::Unsupported(reason),
            },
            Some(_) => ModUpdateStatus::UpToDate,
            None => ModUpdateStatus::UpToDate,
        };
        reports.push(ModUpdateReport {
            file_name: (*name).to_owned(),
            status,
        });
    }
    Ok(reports)
}

/// Modrinth returns newest-first, but do not trust ordering: compare dates.
fn pick_newest(versions: &[ModrinthVersion]) -> Option<&ModrinthVersion> {
    versions
        .iter()
        .max_by(|a, b| a.date_created.cmp(&b.date_created))
}

fn release_file(version: &ModrinthVersion) -> Result<ModrinthFile, String> {
    let file = version
        .files
        .iter()
        .find(|file| file.primary)
        .or_else(|| version.files.first())
        .ok_or("新版本没有可下载文件。")?;
    let url = reqwest::Url::parse(&file.url).map_err(|_| "下载地址无效。")?;
    if url.scheme() != "https" || url.host_str() != Some("cdn.modrinth.com") {
        return Err("新版本文件不来自 Modrinth 官方分发域名。".into());
    }
    let sha1 = file
        .hashes
        .get("sha1")
        .filter(|hash| hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or("新版本缺少有效 SHA-1 校验值。")?;
    if !is_safe_mod_name(&file.filename) {
        return Err("新版本文件名不适用于安全安装。".into());
    }
    Ok(ModrinthFile {
        url: file.url.clone(),
        filename: file.filename.clone(),
        primary: true,
        size: file.size,
        hashes: HashMap::from([("sha1".to_owned(), sha1.to_owned())]),
    })
}

pub fn is_safe_mod_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name.ends_with(".jar")
        && !name.contains(['/', '\\', ':', '\0'])
        && !name.starts_with('.')
}

/// Verified download + atomic replacement. The previous file is kept as
/// `<name>.jar.old` so a broken update stays recoverable, mirroring PCL2's
/// "find the old mod in the recycle bin" promise without shell APIs.
pub async fn apply_mod_update(
    instance: &InstanceRecord,
    update: &ModUpdate,
    mods_dir: &Path,
    engine: &DownloadEngine,
) -> Result<String, String> {
    if !is_safe_mod_name(&update.file_name) || !is_safe_mod_name(&update.new_file_name) {
        return Err("模组文件名无效。".into());
    }
    let current = mods_dir.join(&update.file_name);
    let metadata = std::fs::symlink_metadata(&current).map_err(|e| e.to_string())?;
    if !metadata.file_type().is_file() {
        return Err("更新前找不到原始模组文件。".into());
    }
    let destination = mods_dir.join(&update.new_file_name);
    let temporary = mods_dir.join(format!(".update-{}.jar", uuid::Uuid::new_v4().simple()));
    engine
        .download(
            &ArtifactSpec {
                id: format!("mod-update:{}", update.project_id),
                label: format!("模组更新 {}", update.new_file_name),
                url: update.url.clone(),
                mirror_url: None,
                destination: temporary.clone(),
                size: Some(update.size),
                sha1: Some(update.sha1.clone()),
                sha256: None,
            },
            None,
        )
        .await
        .map_err(|_| "模组下载或校验失败；原文件未改动。")?;
    let replace = || -> Result<(), std::io::Error> {
        let backup = mods_dir.join(format!("{}.old", update.file_name));
        if backup.exists() {
            std::fs::remove_file(&backup)?;
        }
        std::fs::rename(&current, &backup)?;
        if let Err(error) = std::fs::rename(&temporary, &destination) {
            // Roll the old file back before surfacing the failure.
            let _ = std::fs::rename(&backup, &current);
            return Err(error);
        }
        Ok(())
    };
    replace().map_err(|e| format!("替换模组文件失败：{e}"))?;
    let _ = instance;
    Ok(update.new_file_name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn version(id: &str, project: &str, number: &str, date: &str) -> ModrinthVersion {
        ModrinthVersion {
            id: id.into(),
            project_id: project.into(),
            version_number: number.into(),
            date_created: date.into(),
            files: vec![],
        }
    }
    fn file(name: &str, primary: bool, sha1: &str) -> ModrinthFile {
        ModrinthFile {
            url: format!("https://cdn.modrinth.com/{name}"),
            filename: name.into(),
            primary,
            size: 1234,
            hashes: HashMap::from([("sha1".to_owned(), sha1.to_owned())]),
        }
    }

    #[test]
    fn newest_version_wins_regardless_of_array_order() {
        let old = version("v1", "p", "1.0", "2026-01-01T00:00:00Z");
        let new = version("v2", "p", "1.1", "2026-06-01T00:00:00Z");
        assert_eq!(pick_newest(&[old.clone(), new.clone()]).unwrap().id, "v2");
        assert_eq!(pick_newest(&[new, old]).unwrap().id, "v2");
    }

    #[test]
    fn release_file_requires_modrinth_cdn_and_strong_hash() {
        let mut v = version("v", "p", "1", "2026-01-01T00:00:00Z");
        v.files = vec![file("a.jar", true, &"a".repeat(40))];
        assert_eq!(release_file(&v).unwrap().filename, "a.jar");
        v.files[0].url = "https://evil.example.com/a.jar".into();
        assert!(release_file(&v).is_err());
        v.files[0].url = "https://cdn.modrinth.com/a.jar".into();
        v.files[0].hashes.insert("sha1".into(), "short".into());
        assert!(release_file(&v).is_err());
        v.files[0].hashes.insert("sha1".into(), "z".repeat(40));
        assert!(release_file(&v).is_err());
        v.files[0].hashes.insert("sha1".into(), "b".repeat(40));
        v.files[0].filename = "../escape.jar".into();
        assert!(release_file(&v).is_err());
    }

    #[test]
    fn safe_names_reject_traversal_and_non_jars() {
        assert!(is_safe_mod_name("mod-1.0.jar"));
        for bad in ["a.jar.disabled", "../a.jar", "a/b.jar", ".hidden.jar", ""] {
            assert!(!is_safe_mod_name(bad), "{bad}");
        }
    }

    #[test]
    fn loader_name_maps_only_supported_kinds() {
        let mut instance = crate::InstanceRecord {
            id: "i".into(),
            name: "n".into(),
            game_version: "1.21.4".into(),
            loader: "vanilla".into(),
            instance_dir: PathBuf::new(),
            game_dir: PathBuf::new(),
            isolated: true,
            state: crate::InstanceState::Ready,
            created_at: String::new(),
        };
        assert_eq!(instance_loader_name(&instance), None);
        instance.loader = "fabric@0.16.10".into();
        assert_eq!(instance_loader_name(&instance), Some("fabric"));
        instance.loader = "quilt@1".into();
        assert_eq!(instance_loader_name(&instance), Some("quilt"));
        instance.loader = "forge@1".into();
        assert_eq!(instance_loader_name(&instance), None);
    }

    #[tokio::test]
    async fn apply_update_keeps_backup_and_rolls_back_on_missing_source() {
        let root = std::env::temp_dir().join(format!("mfmcl-modupd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("mods")).unwrap();
        std::fs::write(root.join("mods/old.jar"), b"old-bytes").unwrap();
        let instance = crate::InstanceRecord {
            id: "i".into(),
            name: "n".into(),
            game_version: "1.21.4".into(),
            loader: "fabric@0.16.10".into(),
            instance_dir: root.clone(),
            game_dir: root.join("game"),
            isolated: true,
            state: crate::InstanceState::Ready,
            created_at: String::new(),
        };
        let update = ModUpdate {
            file_name: "missing.jar".into(),
            project_id: "p".into(),
            installed_version: "1".into(),
            installed_version_id: "v1".into(),
            new_version: "2".into(),
            new_version_id: "v2".into(),
            new_file_name: "new.jar".into(),
            url: "https://cdn.modrinth.com/new.jar".into(),
            sha1: "0".repeat(40),
            size: 1,
        };
        // No HTTP engine interaction happens: the missing source file fails fast.
        let engine = download_engine::DownloadEngine::new(Default::default()).unwrap();
        assert!(
            apply_mod_update(&instance, &update, &root.join("mods"), &engine)
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read(root.join("mods/old.jar")).unwrap(),
            b"old-bytes"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
