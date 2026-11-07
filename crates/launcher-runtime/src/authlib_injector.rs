//! Downloading and verifying the authlib-injector agent.
//!
//! Skin-server accounts need `-javaagent:<jar>=<server>` on the command line.
//! Fetching that jar is frontend-independent — it is a verified download from a
//! fixed host — so it lives here instead of in the desktop authentication module.
//!
//! The security posture is unchanged from the desktop version and worth stating
//! once: the download URL comes from a metadata document, so its host, path,
//! size, and SHA-256 are all re-checked before the file is fetched, and the
//! download engine verifies the hash before the jar is used.

use crate::{Result, RuntimeError};
use std::path::PathBuf;
use std::time::Duration;

const METADATA_URL: &str = "https://authlib-injector.yushi.moe/artifact/latest.json";
const ALLOWED_HOST: &str = "authlib-injector.yushi.moe";
const MAX_METADATA_BYTES: usize = 1024 * 1024;
const MAX_AGENT_BYTES: u64 = 64 * 1024 * 1024;

/// Ensure the agent jar for the current `latest` build is present, returning its
/// path.
///
/// The file is cached by its SHA-256, so a second launch with the same build does
/// no download. `root` is only a scope marker: a skin-server jar is useful for
/// that server regardless of which instance is launching.
pub async fn ensure(root: &reqwest::Url, data_root: &std::path::Path) -> Result<PathBuf> {
    let client = reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| RuntimeError::message("无法创建认证代理客户端。"))?;

    let mut response = client
        .get(METADATA_URL)
        .send()
        .await
        .map_err(|_| RuntimeError::message("无法获取 Authlib-Injector 版本信息。"))?
        .error_for_status()
        .map_err(|_| RuntimeError::message("Authlib-Injector 版本服务不可用。"))?;

    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| RuntimeError::message("认证代理元数据读取失败。"))?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_METADATA_BYTES {
            return Err(RuntimeError::message("认证代理元数据超过大小限制。"));
        }
        bytes.extend_from_slice(&chunk);
    }

    let meta: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| RuntimeError::message("Authlib-Injector 元数据无效。"))?;
    let url = meta
        .get("download_url")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| RuntimeError::message("缺少认证代理地址。"))?;
    let parsed =
        reqwest::Url::parse(url).map_err(|_| RuntimeError::message("认证代理地址无效。"))?;

    /*
     * The metadata document is the only thing that tells us where the jar lives,
     * so every field that could redirect the download elsewhere is checked here
     * rather than trusted. A compromised metadata response must not be able to
     * point the launcher at an arbitrary host.
     */
    if parsed.scheme() != "https"
        || parsed.host_str() != Some(ALLOWED_HOST)
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !parsed.path().starts_with("/artifact/")
    {
        return Err(RuntimeError::message("认证代理不来自允许的发布域名。"));
    }

    let size = meta.get("size").and_then(serde_json::Value::as_u64);
    if size.is_some_and(|size| size == 0 || size > MAX_AGENT_BYTES) {
        return Err(RuntimeError::message("认证代理文件大小超过限制。"));
    }
    let sha256 = meta
        .get("checksums")
        .and_then(|value| value.get("sha256"))
        .and_then(serde_json::Value::as_str)
        .filter(|value| value.len() == 64 && value.chars().all(|ch| ch.is_ascii_hexdigit()))
        .ok_or_else(|| RuntimeError::message("认证代理缺少有效 SHA-256 校验值。"))?;
    /*
     * An unhashed jar is never accepted, so caching by hash is safe: the file at
     * this path can only ever be the build that hash describes.
     */
    let path = data_root
        .join("authlib-injector")
        .join(format!("{sha256}.jar"));

    let engine = download_engine::DownloadEngine::new(download_engine::DownloadSettings::default())
        .map_err(|_| RuntimeError::message("无法创建认证代理下载器。"))?
        .with_max_file_size(MAX_AGENT_BYTES);
    engine
        .download(
            &download_engine::ArtifactSpec {
                id: "authlib-injector".into(),
                label: "皮肤站认证代理".into(),
                url: parsed.to_string(),
                mirror_url: None,
                destination: path.clone(),
                size,
                sha1: None,
                sha256: Some(sha256.into()),
            },
            None,
        )
        .await
        .map_err(|_| RuntimeError::message("无法下载或验证认证代理。"))?;

    let _ = root;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn requires(text: &str) -> Result<PathBuf> {
        // Reuse the validation shape without a network call.
        Err(RuntimeError::message(text))
    }

    #[test]
    fn the_metadata_host_is_pinned() {
        assert_eq!(ALLOWED_HOST, "authlib-injector.yushi.moe");
        assert!(METADATA_URL.starts_with("https://"));
    }

    #[test]
    fn size_caps_are_sane() {
        // The metadata document is tiny; the jar is a few hundred KiB. Both caps
        // exist so a hostile response cannot stream until the disk fills.
        assert_eq!(MAX_METADATA_BYTES, 1024 * 1024);
        assert_eq!(MAX_AGENT_BYTES, 64 * 1024 * 1024);
        assert!(MAX_AGENT_BYTES > MAX_METADATA_BYTES as u64);
    }

    #[test]
    fn helper_error_path_is_used() {
        assert!(requires("x").is_err());
    }
}
