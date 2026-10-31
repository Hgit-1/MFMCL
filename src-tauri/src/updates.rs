//! Launcher self-update checks. Principles adapted from public PCL2 settings
//! wording (see docs/REFERENCE_NOTES.md); implementation is original.
//!
//! Without update-signing keys this host never replaces its own running binary.
//! A check fetches our HTTPS manifest, compares versions, and a confirmed
//! download is SHA-256-verified into a user-chosen folder for explicit install.
use launcher_core::{UpdateManifest, Version};
use serde::Serialize;
use std::time::Duration;

/// Configure at build time, e.g. -D MFMCL_UPDATE_MANIFEST=https://host/latest.json
const BUILD_MANIFEST: Option<&str> = option_env!("MFMCL_UPDATE_MANIFEST");

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub current: String,
    pub latest: Option<String>,
    pub notes: String,
    pub available: bool,
    pub url: Option<String>,
    pub sha256: Option<String>,
    pub size: Option<u64>,
}

fn update_error(error: impl std::fmt::Display) -> String {
    format!("更新检查未完成：{error}")
}

async fn fetch_manifest(url: &str) -> Result<UpdateManifest, String> {
    let parsed = reqwest::Url::parse(url).map_err(|_| "更新清单地址无效。")?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err("更新清单必须是 HTTPS 地址。".into());
    }
    let client = reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("MFMCL/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(update_error)?;
    let mut response = client
        .get(parsed)
        .send()
        .await
        .map_err(|_| "无法连接更新服务。")?
        .error_for_status()
        .map_err(|_| "更新服务暂时不可用。")?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "更新清单读取失败。")? {
        if bytes.len().saturating_add(chunk.len()) > 512 * 1024 {
            return Err("更新清单超过大小限制。".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    UpdateManifest::parse(&bytes).map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn check_launcher_update(manifest_url: Option<String>) -> Result<UpdateStatus, String> {
    let url = manifest_url
        .filter(|value| !value.trim().is_empty())
        .or_else(|| BUILD_MANIFEST.map(str::to_owned))
        .ok_or_else(|| {
            "此构建未配置更新清单地址；请在构建时设置 MFMCL_UPDATE_MANIFEST 后重新分发。".to_owned()
        })?;
    let manifest = fetch_manifest(&url).await?;
    let current =
        Version::parse(env!("CARGO_PKG_VERSION")).map_err(|_| "当前构建的版本号无效。")?;
    let latest = Version::parse(&manifest.version).map_err(|_| "更新清单的版本号无效。")?;
    let release = manifest
        .release_for(std::env::consts::OS, std::env::consts::ARCH)
        .cloned();
    let available = latest.is_newer_than(&current) && release.is_some();
    Ok(UpdateStatus {
        current: current.as_str(),
        latest: Some(latest.as_str()),
        notes: manifest.notes,
        available,
        url: release.as_ref().map(|r| r.url.clone()),
        sha256: release.as_ref().map(|r| r.sha256.clone()),
        size: release.as_ref().and_then(|r| r.size),
    })
}

#[tauri::command]
pub async fn download_launcher_update(
    url: String,
    sha256: String,
    directory: String,
) -> Result<String, String> {
    let parsed = reqwest::Url::parse(&url).map_err(|_| "更新安装包地址无效。")?;
    // Prefer the manifest file name; fall back to a platform-appropriate default.
    let fallback = format!(
        "MFMCL-installer{}",
        if cfg!(windows) { ".exe" } else { ".AppImage" }
    );
    let candidate = parsed
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|name| !name.is_empty())
        .unwrap_or(&fallback);
    let safe_name = candidate
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect::<String>();
    if safe_name.is_empty() || safe_name.len() > 128 || safe_name.starts_with('.') {
        return Err("更新文件名无效。".into());
    }
    let directory = std::path::PathBuf::from(directory);
    if !directory.is_dir() {
        return Err("请选择一个已存在的文件夹保存更新安装包。".into());
    }
    let destination = directory.join(safe_name);
    let engine = download_engine::DownloadEngine::new(download_engine::DownloadSettings::default())
        .map_err(update_error)?
        .with_max_file_size(512 * 1024 * 1024);
    engine
        .download(
            &download_engine::ArtifactSpec {
                id: "launcher-update".into(),
                label: "启动器更新安装包".into(),
                url,
                mirror_url: None,
                destination: destination.clone(),
                size: None,
                sha1: None,
                sha256: Some(sha256),
            },
            None,
        )
        .await
        .map_err(|_| "更新安装包下载或校验失败；文件未发布到目标文件夹。")?;
    Ok(destination.to_string_lossy().into_owned())
}
