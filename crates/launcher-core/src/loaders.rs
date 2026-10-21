//! Original Fabric/Quilt adapters. Forge/NeoForge processors are deliberately not run here.
use crate::{InstanceRecord, InstanceState, LauncherStore, installer::maven_path};
use download_engine::{ArtifactSpec, DownloadEngine, DownloadProgress};
use mc_metadata::{Artifact, GameVersionMetadata, Library, LibraryDownloads};
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LoaderKind {
    Fabric,
    Quilt,
}

impl LoaderKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Fabric => "fabric",
            Self::Quilt => "quilt",
        }
    }
    fn meta(self) -> &'static str {
        match self {
            Self::Fabric => "https://meta.fabricmc.net/v2/",
            Self::Quilt => "https://meta.quiltmc.org/v3/",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoaderVersion {
    pub version: String,
    pub stable: bool,
}

pub fn safe_version(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-+_".contains(c))
        || value.contains("..")
    {
        return Err("版本标识包含不允许的字符。".into());
    }
    crate::modpacks::safe_pack_path(value)?;
    Ok(())
}

pub async fn list_loader_versions(
    kind: LoaderKind,
    game: &str,
) -> Result<Vec<LoaderVersion>, String> {
    safe_version(game)?;
    let client = metadata_client()?;
    let response = client
        .get(format!("{}versions/loader/{game}", kind.meta()))
        .send()
        .await
        .map_err(|_| "无法连接加载器版本服务。")?
        .error_for_status()
        .map_err(|_| "该游戏版本没有可用加载器。")?;
    let bytes = bounded_response(response, 4 * 1024 * 1024).await?;
    let values: Vec<serde_json::Value> =
        serde_json::from_slice(&bytes).map_err(|_| "加载器版本响应无效。")?;
    Ok(values
        .into_iter()
        .filter_map(|value| {
            let loader = value.get("loader")?;
            let version = loader.get("version")?.as_str()?.to_owned();
            if safe_version(&version).is_err() {
                return None;
            }
            Some(LoaderVersion {
                version,
                stable: loader
                    .get("stable")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            })
        })
        .collect())
}

fn metadata_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("MFMCL/0.1.0")
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "无法初始化 HTTPS 客户端。".into())
}

pub async fn install_loader(
    root: &Path,
    instance: &InstanceRecord,
    store: &LauncherStore,
    engine: &DownloadEngine,
    kind: LoaderKind,
    version: &str,
    progress: Option<mpsc::UnboundedSender<DownloadProgress>>,
) -> Result<(), String> {
    safe_version(version)?;
    safe_version(&instance.game_version)?;
    if instance.state != InstanceState::Ready {
        return Err("请先完成原版安装。".into());
    }
    if instance.loader != "vanilla" {
        return Err("实例已安装加载器；不会覆盖已有配置。".into());
    }
    let client = metadata_client()?;
    let response = client
        .get(format!(
            "{}versions/loader/{}/{version}/profile/json",
            kind.meta(),
            instance.game_version
        ))
        .send()
        .await
        .map_err(|_| "无法获取加载器配置。")?
        .error_for_status()
        .map_err(|_| "此加载器与游戏版本组合不可用。")?;
    let bytes = bounded_response(response, 4 * 1024 * 1024).await?;
    let patch: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| "加载器配置格式无效。")?;
    if patch.get("processors").is_some() {
        return Err("加载器配置包含不支持的处理器；不会执行任意程序。".into());
    }
    let version_dir = instance
        .instance_dir
        .join("versions")
        .join(&instance.game_version);
    let version_dir_meta = tokio::fs::symlink_metadata(&version_dir)
        .await
        .map_err(|e| e.to_string())?;
    if version_dir_meta.file_type().is_symlink() || !version_dir_meta.file_type().is_dir() {
        return Err("游戏版本目录不能是符号链接或普通文件。".into());
    }
    let base_path = version_dir.join(format!("{}.json", instance.game_version));
    let base_meta = tokio::fs::symlink_metadata(&base_path)
        .await
        .map_err(|e| e.to_string())?;
    if base_meta.file_type().is_symlink()
        || !base_meta.file_type().is_file()
        || base_meta.len() > 16 * 1024 * 1024
    {
        return Err("原版游戏配置不是有效的普通文件。".into());
    }
    let base: GameVersionMetadata = serde_json::from_slice(
        &tokio::fs::read(base_path)
            .await
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let library_list = patch
        .get("libraries")
        .and_then(|v| v.as_array())
        .ok_or("加载器缺少依赖清单。")?;
    if library_list.len() > 10_000 {
        return Err("加载器依赖数量超过限制。".into());
    }
    let mut libraries = Vec::new();
    let mut coordinates = std::collections::HashSet::new();
    for library in library_list {
        let name = library
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or("加载器依赖坐标无效。")?;
        let relative = safe_maven_path(name)?;
        if !coordinates.insert(name.to_owned()) {
            return Err("加载器依赖清单包含重复 Maven 坐标。".into());
        }
        let repository = library
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("https://libraries.minecraft.net/");
        let mut repository = reqwest::Url::parse(repository).map_err(|_| "依赖仓库 URL 无效。")?;
        if repository.scheme() != "https"
            || repository.query().is_some()
            || repository.fragment().is_some()
            || !matches!(
                repository.host_str(),
                Some(
                    "maven.fabricmc.net"
                        | "maven.quiltmc.org"
                        | "libraries.minecraft.net"
                        | "repo1.maven.org"
                        | "repo.maven.apache.org"
                )
            )
            || !repository.username().is_empty()
            || repository.password().is_some()
        {
            return Err("加载器配置引用了未获准的依赖仓库。".into());
        }
        if !repository.path().ends_with('/') {
            repository.set_path(&format!("{}/", repository.path()));
        }
        let url = repository
            .join(&relative)
            .map_err(|e| e.to_string())?
            .to_string();
        let sha1 = if let Some(value) = library.get("sha1") {
            let hash = value
                .as_str()
                .ok_or("加载器 SHA-1 校验值无效。")?
                .to_owned();
            hash
        } else {
            let response = client
                .get(format!("{url}.sha1"))
                .send()
                .await
                .map_err(|_| "无法取得加载器依赖校验值。")?
                .error_for_status()
                .map_err(|_| "加载器依赖没有可用 SHA-1 校验值。")?;
            let bytes = bounded_response(response, 4096).await?;
            let text = std::str::from_utf8(&bytes).map_err(|_| "校验值不是有效文本。")?;
            text.split_whitespace().next().unwrap_or("").to_owned()
        };
        if sha1.len() != 40 || !sha1.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err("加载器 SHA-1 校验值无效。".into());
        }
        let sha256 = match library.get("sha256") {
            Some(value) => {
                let hash = value.as_str().ok_or("加载器 SHA-256 校验值无效。")?;
                if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err("加载器 SHA-256 校验值无效。".into());
                }
                Some(hash.to_owned())
            }
            None => None,
        };
        let size = match library.get("size") {
            Some(value) => Some(
                value
                    .as_u64()
                    .filter(|size| *size <= 512 * 1024 * 1024)
                    .ok_or("加载器依赖大小无效。")?,
            ),
            None => None,
        };
        let artifact = Artifact {
            path: Some(relative.clone()),
            url: url.clone(),
            sha1: Some(sha1.clone()),
            size,
        };
        engine
            .download(
                &ArtifactSpec {
                    id: format!("loader:{name}"),
                    label: format!("加载器依赖 {name}"),
                    url,
                    mirror_url: None,
                    destination: root.join("libraries").join(&relative),
                    size,
                    sha1: Some(sha1),
                    sha256,
                },
                progress.clone(),
            )
            .await
            .map_err(|e| e.to_string())?;
        libraries.push(Library {
            name: name.to_owned(),
            downloads: LibraryDownloads {
                artifact: Some(artifact),
                ..Default::default()
            },
            ..Default::default()
        });
    }
    let merged = merge_loader_profile(base, &patch, libraries)?;
    // Publish through a unique temporary file. hard_link atomically refuses replacement.
    publish_profile(
        &version_dir,
        &serde_json::to_vec_pretty(&merged).map_err(|e| e.to_string())?,
        || {
            store
                .set_instance_loader(&instance.id, &format!("{}@{version}", kind.name()))
                .map_err(|e| e.to_string())
        },
    )
}

fn publish_profile(
    directory: &Path,
    bytes: &[u8],
    commit: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    use std::io::Write;
    let temp = directory.join(format!(".launch-{}.pending", uuid::Uuid::new_v4()));
    let target = directory.join("launch.json");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| e.to_string())?;
    let mut published = false;
    let result = (|| {
        file.write_all(bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        std::fs::hard_link(&temp, &target)
            .map_err(|e| format!("无法发布加载器配置（不会覆盖已有文件）：{e}"))?;
        published = true;
        commit()
    })();
    drop(file);
    if let Err(error) = result {
        let mut error = error;
        if published {
            if let Err(rollback) = std::fs::remove_file(&target) {
                error = format!("{error}；加载器配置回滚失败：{rollback}");
            }
        }
        if let Err(cleanup) = std::fs::remove_file(&temp) {
            error = format!("{error}；清理临时配置失败：{cleanup}");
        }
        return Err(error);
    }
    // Once committed, cleanup can be retried without reinstalling the loader.
    std::fs::remove_file(&temp).map_err(|e| format!("加载器已安装，但清理临时配置失败：{e}"))
}

async fn bounded_response(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err("加载器响应过大。".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "加载器响应读取失败。")? {
        if bytes
            .len()
            .checked_add(chunk.len())
            .is_none_or(|size| size > limit)
        {
            return Err("加载器响应过大。".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn safe_maven_path(name: &str) -> Result<String, String> {
    if name.contains(['/', '\\'])
        || name.contains("..")
        || name.matches('@').count() > 1
        || name
            .chars()
            .any(|c| !c.is_ascii_alphanumeric() && !".:-_+@".contains(c))
    {
        return Err("不安全的 Maven 坐标。".into());
    }
    let (coordinate, extension) = name.split_once('@').unwrap_or((name, "jar"));
    let parts: Vec<_> = coordinate.split(':').collect();
    if !(3..=4).contains(&parts.len())
        || parts.iter().any(|part| part.is_empty())
        || extension.is_empty()
        || !extension.chars().all(|c| c.is_ascii_alphanumeric())
    {
        return Err("Maven 坐标无效。".into());
    }
    let path = maven_path(name)
        .and_then(|p| p.to_str().map(str::to_owned))
        .ok_or("Maven 坐标无效。")?;
    #[cfg(windows)]
    let path = path.replace('\\', "/");
    crate::modpacks::safe_pack_path(&path)?;
    Ok(path)
}

/// A pending marker can survive a successful install if its removal failed.
pub fn loader_overlay_is_installed(
    instance: &InstanceRecord,
    kind: LoaderKind,
    version: &str,
) -> Result<bool, String> {
    safe_version(version)?;
    safe_version(&instance.game_version)?;
    if instance.loader != format!("{}@{version}", kind.name()) {
        return Ok(false);
    }
    let path = instance
        .instance_dir
        .join("versions")
        .join(&instance.game_version)
        .join("launch.json");
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    if !metadata.file_type().is_file() || metadata.len() > 16 * 1024 * 1024 {
        return Err("已记录的加载器配置不是有效的普通文件。".into());
    }
    let profile: GameVersionMetadata =
        serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|_| "已记录的加载器配置格式无效。")?;
    Ok(profile.id == instance.game_version
        && profile
            .main_class
            .as_deref()
            .is_some_and(|value| !value.is_empty()))
}

fn merge_loader_profile(
    mut base: GameVersionMetadata,
    patch: &serde_json::Value,
    libraries: Vec<Library>,
) -> Result<GameVersionMetadata, String> {
    if patch.get("inheritsFrom").and_then(|v| v.as_str()) != Some(base.id.as_str()) {
        return Err("加载器继承版本与实例不一致。".into());
    }
    base.main_class = Some(
        patch
            .get("mainClass")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or("加载器缺少入口类。")?
            .to_owned(),
    );
    // Replace conflicting Maven module versions instead of adding duplicate classpath entries.
    for library in libraries {
        let key = library
            .name
            .split(':')
            .take(2)
            .collect::<Vec<_>>()
            .join(":");
        base.libraries
            .retain(|old| old.name.split(':').take(2).collect::<Vec<_>>().join(":") != key);
        base.libraries.push(library);
    }
    if let Some(args) = patch.get("arguments") {
        if base.arguments.is_none() {
            return Err("当前加载器适配器需要现代版本启动参数；此旧版本暂不支持。".into());
        }
        let extra: mc_metadata::LaunchArguments =
            serde_json::from_value(args.clone()).map_err(|_| "加载器启动参数无效。")?;
        let base_args = base.arguments.get_or_insert_with(Default::default);
        base_args.jvm.extend(extra.jvm);
        base_args.game.extend(extra.game);
    }
    Ok(base)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn publishing_is_atomic_and_rolls_back_store_failure() {
        let directory = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir(&directory).unwrap();
        assert!(publish_profile(&directory, b"new", || Err("store failure".into())).is_err());
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 0);
        std::fs::write(directory.join("launch.json"), b"old").unwrap();
        assert!(publish_profile(&directory, b"new", || Ok(())).is_err());
        assert_eq!(
            std::fs::read(directory.join("launch.json")).unwrap(),
            b"old"
        );
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        std::fs::remove_file(directory.join("launch.json")).unwrap();
        publish_profile(&directory, b"committed", || Ok(())).unwrap();
        assert_eq!(
            std::fs::read(directory.join("launch.json")).unwrap(),
            b"committed"
        );
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn pending_retry_recognizes_the_committed_loader() {
        let directory = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(directory.join("versions/1.21.4")).unwrap();
        let instance = InstanceRecord {
            id: "test".into(),
            name: "test".into(),
            game_version: "1.21.4".into(),
            loader: "fabric@0.16.10".into(),
            instance_dir: directory.clone(),
            game_dir: directory.join("game"),
            isolated: true,
            state: InstanceState::Ready,
            created_at: String::new(),
        };
        assert!(!loader_overlay_is_installed(&instance, LoaderKind::Fabric, "0.16.10").unwrap());
        std::fs::write(
            directory.join("versions/1.21.4/launch.json"),
            br#"{"id":"1.21.4","mainClass":"loader.Main","libraries":[]}"#,
        )
        .unwrap();
        assert!(loader_overlay_is_installed(&instance, LoaderKind::Fabric, "0.16.10").unwrap());
        assert!(!loader_overlay_is_installed(&instance, LoaderKind::Quilt, "0.16.10").unwrap());
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn coordinates_cannot_escape_library_directory() {
        assert_eq!(
            safe_maven_path("net.fabricmc:fabric-loader:0.16.10").unwrap(),
            "net/fabricmc/fabric-loader/0.16.10/fabric-loader-0.16.10.jar"
        );
        for value in [
            "a:../b:c",
            "a:/b:c",
            "a:b:c/evil",
            ".:b:c",
            "a::c",
            "a:b:c@",
            "a:b:c@jar@txt",
            "a:CON:c",
        ] {
            assert!(safe_maven_path(value).is_err(), "{value}");
        }
    }
    #[test]
    fn overlay_keeps_official_id_and_rejects_wrong_base() {
        let base: GameVersionMetadata = serde_json::from_value(serde_json::json!({"id":"1.21.4","mainClass":"original","libraries":[],"arguments":{"game":[],"jvm":[]}})).unwrap();
        let patch = serde_json::json!({"inheritsFrom":"1.21.4","mainClass":"loader.Main","arguments":{"game":[],"jvm":["-Dloader=true"]}});
        let merged = merge_loader_profile(base.clone(), &patch, vec![]).unwrap();
        assert_eq!(merged.id, "1.21.4");
        assert_eq!(merged.main_class.as_deref(), Some("loader.Main"));
        assert!(
            merge_loader_profile(base, &serde_json::json!({"inheritsFrom":"bad"}), vec![]).is_err()
        );
    }
}
