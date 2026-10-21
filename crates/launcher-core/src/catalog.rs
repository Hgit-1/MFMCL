//! Modrinth online mod catalogue: search, per-project versions, verified install.
//!
//! Same trust surface as mod updates: `api.modrinth.com` for metadata,
//! `cdn.modrinth.com` for files, SHA-1 pinned before publication. Searches
//! send the query and the instance's game version/loader facets to Modrinth;
//! the UI discloses this. Installing never overwrites an existing mod file.
use crate::InstanceRecord;
use download_engine::{ArtifactSpec, DownloadEngine};
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};

const API: &str = "https://api.modrinth.com/v2";
const MAX_QUERY: usize = 128;
const MAX_RESULTS: usize = 20;
const MAX_RESPONSE: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CatalogMod {
    pub project_id: String,
    pub title: String,
    pub description: String,
    pub author: String,
    pub downloads: u64,
    pub follows: u64,
    pub icon_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CatalogVersion {
    pub version_id: String,
    pub version_number: String,
    pub date_created: String,
    pub file_name: String,
    pub url: String,
    pub sha1: String,
    pub size: u64,
}

pub fn catalog_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("MFMCL/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| "无法初始化目录客户端。".to_owned())
}

fn loader_of(instance: &InstanceRecord) -> Result<&'static str, String> {
    crate::mod_updates::instance_loader_name(instance)
        .ok_or_else(|| "在线模组目录需要 Fabric 或 Quilt 实例。".to_owned())
}

async fn bounded(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length as usize > limit)
    {
        return Err("Modrinth 响应过大。".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "Modrinth 读取失败。")? {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err("Modrinth 响应过大。".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Search mods for the instance's game version and loader.
pub async fn search_mods(
    client: &reqwest::Client,
    instance: &InstanceRecord,
    query: &str,
) -> Result<Vec<CatalogMod>, String> {
    let loader = loader_of(instance)?;
    let query = query.trim();
    if query.is_empty() || query.chars().count() > MAX_QUERY {
        return Err("搜索词需为 1–128 个字符。".into());
    }
    // Facets restrict hits to mods with a release for this game version + loader.
    let facets = format!(
        "[[\"project_type:mod\"],[\"categories:{loader}\"],[\"versions:{}\"]]",
        instance.game_version
    );
    let url = reqwest::Url::parse_with_params(
        &format!("{API}/search"),
        &[
            ("query", query),
            ("facets", facets.as_str()),
            ("limit", MAX_RESULTS.to_string().as_str()),
        ],
    )
    .map_err(|_| "搜索地址无效。")?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| "无法连接 Modrinth。")?
        .error_for_status()
        .map_err(|_| "Modrinth 拒绝了搜索请求。")?;
    let bytes = bounded(response, MAX_RESPONSE).await?;
    parse_search(&bytes)
}

fn parse_search(bytes: &[u8]) -> Result<Vec<CatalogMod>, String> {
    #[derive(Deserialize)]
    struct Hit {
        project_id: String,
        title: String,
        description: String,
        author: String,
        downloads: u64,
        follows: u64,
        #[serde(default)]
        icon_url: Option<String>,
    }
    #[derive(Deserialize)]
    struct SearchResponse {
        #[serde(default)]
        hits: Vec<Hit>,
    }
    let parsed: SearchResponse =
        serde_json::from_slice(bytes).map_err(|_| "Modrinth 搜索结果无效。")?;
    if parsed.hits.len() > MAX_RESULTS {
        return Err("Modrinth 搜索结果数量异常。".into());
    }
    // A single malformed hit is skipped, not fatal: browsing stays usable.
    let valid = parsed
        .hits
        .into_iter()
        .filter_map(|hit| {
            if hit.project_id.is_empty() || hit.title.chars().count() > 256 {
                return None;
            }
            Some(CatalogMod {
                project_id: hit.project_id,
                title: hit.title,
                description: truncate(&hit.description, 512),
                author: truncate(&hit.author, 128),
                downloads: hit.downloads,
                follows: hit.follows,
                icon_url: hit.icon_url.filter(|url| {
                    reqwest::Url::parse(url).is_ok_and(|parsed| parsed.scheme() == "https")
                }),
            })
        })
        .collect::<Vec<_>>();
    Ok(valid)
}

fn truncate(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

/// List installable versions of a project for this instance.
pub async fn mod_versions(
    client: &reqwest::Client,
    instance: &InstanceRecord,
    project_id: &str,
) -> Result<Vec<CatalogVersion>, String> {
    let loader = loader_of(instance)?;
    if project_id.is_empty()
        || project_id.len() > 128
        || !project_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("项目标识无效。".into());
    }
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
    parse_versions(&bytes)
}

fn parse_versions(bytes: &[u8]) -> Result<Vec<CatalogVersion>, String> {
    #[derive(Deserialize)]
    struct File {
        url: String,
        filename: String,
        #[serde(default)]
        primary: bool,
        size: u64,
        #[serde(default)]
        hashes: std::collections::HashMap<String, String>,
    }
    #[derive(Deserialize)]
    struct Version {
        id: String,
        version_number: String,
        date_created: String,
        #[serde(default)]
        files: Vec<File>,
    }
    let parsed: Vec<Version> =
        serde_json::from_slice(bytes).map_err(|_| "Modrinth 版本列表无效。")?;
    if parsed.len() > 100 {
        return Err("Modrinth 版本数量异常。".into());
    }
    parsed
        .into_iter()
        .map(|version| {
            let file = version
                .files
                .iter()
                .find(|file| file.primary)
                .or_else(|| version.files.first())
                .ok_or_else(|| "此版本没有可下载文件。".to_owned())?;
            let url = reqwest::Url::parse(&file.url).map_err(|_| "下载地址无效。")?;
            if url.scheme() != "https" || url.host_str() != Some("cdn.modrinth.com") {
                return Err("文件不来自 Modrinth 官方分发域名。".into());
            }
            let sha1 = file
                .hashes
                .get("sha1")
                .filter(|hash| hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
                .ok_or_else(|| "此版本缺少有效 SHA-1 校验值。".to_owned())?;
            if !crate::mod_updates::is_safe_mod_name(&file.filename) {
                return Err("此版本的文件名不适用于安全安装。".into());
            }
            Ok(CatalogVersion {
                version_id: version.id,
                version_number: truncate(&version.version_number, 128),
                date_created: version.date_created,
                file_name: file.filename.clone(),
                url: file.url.clone(),
                sha1: sha1.clone(),
                size: file.size,
            })
        })
        .collect()
}

/// Verified install into the instance's mods directory. An existing file with
/// the same name is never overwritten; the update flow handles replacements.
pub async fn install_mod_version(
    update_target: &CatalogVersion,
    mods_dir: &Path,
    engine: &DownloadEngine,
) -> Result<String, String> {
    if !crate::mod_updates::is_safe_mod_name(&update_target.file_name) {
        return Err("模组文件名无效。".into());
    }
    let destination = mods_dir.join(&update_target.file_name);
    if std::fs::symlink_metadata(&destination).is_ok() {
        return Err(format!(
            "mods 目录中已存在同名文件 {}；如需替换请使用更新功能。",
            update_target.file_name
        ));
    }
    let temporary = mods_dir.join(format!(".install-{}.jar", uuid::Uuid::new_v4().simple()));
    engine
        .download(
            &ArtifactSpec {
                id: format!("mod-install:{}", update_target.version_id),
                label: format!("安装模组 {}", update_target.file_name),
                url: update_target.url.clone(),
                mirror_url: None,
                destination: temporary.clone(),
                size: Some(update_target.size),
                sha1: Some(update_target.sha1.clone()),
                sha256: None,
            },
            None,
        )
        .await
        .map_err(|_| "模组下载或校验失败；没有改动任何文件。")?;
    // hard_link atomically refuses an existing destination (also dangling symlinks).
    if let Err(error) = std::fs::hard_link(&temporary, &destination) {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("无法发布模组文件（不会覆盖已有文件）：{error}"));
    }
    std::fs::remove_file(&temporary).map_err(|e| format!("模组已安装，但清理临时文件失败：{e}"))?;
    Ok(update_target.file_name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn search_json() -> Vec<u8> {
        r#"{"hits":[{"project_id":"p1","title":"Sodium","description":"渲染优化","author":"作者","downloads":1000,"follows":50,"icon_url":"https://cdn.modrinth.com/icon.png"},{"project_id":"","title":"bad","description":"","author":"","downloads":0,"follows":0}]}"#.as_bytes().to_vec()
    }

    #[test]
    fn search_results_are_bounded_and_validated() {
        let parsed = parse_search(&search_json()).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].project_id, "p1");
        assert_eq!(parsed[0].title, "Sodium");
        assert!(parsed[0].icon_url.is_some());
        assert!(parse_search(b"{").is_err());
        let oversized = format!(
            "{{\"hits\":[{}]}}",
            (0..21)
                .map(|i| format!("{{\"project_id\":\"p{i}\",\"title\":\"t\",\"description\":\"\",\"author\":\"\",\"downloads\":0,\"follows\":0}}"))
                .collect::<Vec<_>>()
                .join(",")
        );
        assert!(parse_search(oversized.as_bytes()).is_err());
    }

    #[test]
    fn versions_require_cdn_https_and_strong_hash() {
        let base = r#"[{"id":"v1","version_number":"1.0","date_created":"2026-01-01T00:00:00Z","files":[{"url":"https://cdn.modrinth.com/m.jar","filename":"m.jar","primary":true,"size":10,"hashes":{"sha1":"%HASH%"}}]}]"#;
        let good = base.replace("%HASH%", &"a".repeat(40));
        let parsed = parse_versions(good.as_bytes()).unwrap();
        assert_eq!(parsed[0].version_number, "1.0");
        assert_eq!(parsed[0].file_name, "m.jar");
        for bad in [
            base.replace("cdn.modrinth.com", "evil.example.com"),
            base.replace("%HASH%", "short"),
            base.replace("%HASH%", &"z".repeat(40)),
            base.replace("m.jar", "../m.jar"),
            base.replace("https://", "http://"),
        ] {
            assert!(parse_versions(bad.as_bytes()).is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn install_refuses_existing_name_and_never_overwrites() {
        let root = std::env::temp_dir().join(format!("mfmcl-catalog-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("mods")).unwrap();
        std::fs::write(root.join("mods/taken.jar"), b"existing").unwrap();
        let version = CatalogVersion {
            version_id: "v1".into(),
            version_number: "1.0".into(),
            date_created: String::new(),
            file_name: "taken.jar".into(),
            url: "https://cdn.modrinth.com/taken.jar".into(),
            sha1: "0".repeat(40),
            size: 1,
        };
        let engine = download_engine::DownloadEngine::new(Default::default()).unwrap();
        let result = install_mod_version(&version, &root.join("mods"), &engine).await;
        assert!(result.is_err());
        assert_eq!(
            std::fs::read(root.join("mods/taken.jar")).unwrap(),
            b"existing"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
