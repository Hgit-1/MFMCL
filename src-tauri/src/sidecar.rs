//! Release-pinned sidecar resolution. A missing or unpinned binary never gets executed.
use serde::Deserialize;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Deserialize)]
struct Manifest {
    version: String,
    platforms: HashMap<String, Pin>,
}
#[derive(Deserialize)]
struct Pin {
    sha256: String,
}

pub async fn verified_aria2(resource_dir: &Path) -> Result<PathBuf, String> {
    let manifest_path = resource_dir.join("resources/aria2-manifest.json");
    let bytes = tokio::fs::read(manifest_path)
        .await
        .map_err(|_| "此构建未包含固定版本 aria2。")?;
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|_| "aria2 版本清单无效。")?;
    if manifest.version != "1.37.0" {
        return Err("aria2 版本不在此构建的允许清单中。".into());
    }
    let key = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    let pin = manifest
        .platforms
        .get(&key)
        .ok_or("此平台尚未附带已校验的 aria2，将使用原生下载器。")?;
    if pin.sha256.len() != 64 || !pin.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("aria2 校验值无效。".into());
    }
    let path = download_engine::aria2::find_aria2_binary(resource_dir);
    download_engine::verify_file(&path, None, None, Some(&pin.sha256))
        .await
        .map_err(|_| "aria2 文件与发布清单不一致，拒绝执行。")?;
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new(&path)
            .arg("--version")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| "aria2 版本检测超时。")?
    .map_err(|_| "aria2 无法执行。")?;
    if !output.status.success()
        || !String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .is_some_and(|line| line.trim() == "aria2 version 1.37.0")
    {
        return Err("aria2 可执行文件报告了不匹配的版本。".into());
    }
    Ok(path)
}
