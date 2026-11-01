//! Release-pinned sidecar resolution. A missing or unpinned binary never gets executed.
use serde::{Deserialize, Serialize};
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

/// aria2 的可用状态，供界面显示。
///
/// 为什么需要它：aria2 是否真的被启用，之前只在启动下载时决定，失败时仅写一行
/// stderr。用户打开「使用 aria2 加速」开关后，无法知道它到底生效了没有——这类
/// 「开关看起来可用但实际没用」的状态正是要避免的。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Aria2Availability {
    /// 本构建是否附带了这个平台的 aria2（清单里有 pin 且文件校验通过）。
    pub bundled: bool,
    /// 可执行文件的绝对路径；未附带时为 None。
    pub path: Option<String>,
    /// 二进制报告的版本；未运行时为 None。
    pub version: Option<String>,
    /// 不可用的原因（未附带、校验失败、版本不符等）。
    pub problem: Option<String>,
}

/// 查询 aria2 可用性，不启动下载。
///
/// 与 `verified_aria2` 共用同一套校验，但返回结构化结果而不是 `Err`：界面需要
/// 区分「这个平台没有附带」和「附带了但校验失败」，两者对用户的含义完全不同。
pub async fn aria2_availability(resource_dir: &Path) -> Aria2Availability {
    match verified_aria2(resource_dir).await {
        Ok(path) => Aria2Availability {
            bundled: true,
            path: Some(path.to_string_lossy().into_owned()),
            version: Some("1.37.0".to_owned()),
            problem: None,
        },
        Err(problem) => Aria2Availability {
            bundled: false,
            path: None,
            version: None,
            problem: Some(problem),
        },
    }
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
