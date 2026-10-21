use crate::{InstanceRecord, InstanceState, LauncherStore, StoreError};
use download_engine::{ArtifactSpec, DownloadEngine, DownloadError, DownloadProgress};
use futures_util::{StreamExt, TryStreamExt};
use mc_metadata::{
    Artifact, AssetIndex, AssetIndexRef, GameVersionMetadata, Library, LibraryRule,
    SourcePreference, fetch_manifest, fetch_version_metadata,
};
use regex::Regex;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::File;
use std::io;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;
use tokio::sync::mpsc;
use zip::ZipArchive;

#[derive(Debug, Error)]
pub enum InstallError {
    #[error("Minecraft metadata error: {0}")]
    Metadata(#[from] mc_metadata::MetadataError),
    #[error("download error: {0}")]
    Download(#[from] DownloadError),
    #[error("file operation failed: {0}")]
    Io(#[from] io::Error),
    #[error("database operation failed: {0}")]
    Store(#[from] StoreError),
    #[error("version metadata is incomplete: {0}")]
    InvalidMetadata(String),
    #[error("the selected version requires an inherited modded profile")]
    InheritedVersion,
    #[error("could not extract a native library: {0}")]
    Native(String),
}

#[derive(Debug, Clone)]
struct NativeArchive {
    archive: PathBuf,
    destination: PathBuf,
    excludes: Vec<String>,
}

pub async fn install_vanilla(
    client: &reqwest::Client,
    engine: DownloadEngine,
    store: &LauncherStore,
    data_root: &Path,
    instance: &InstanceRecord,
    progress: Option<mpsc::UnboundedSender<DownloadProgress>>,
) -> Result<InstanceRecord, InstallError> {
    store.set_instance_state(&instance.id, InstanceState::Installing)?;
    let result = install_inner(client, engine, data_root, instance, progress).await;
    match result {
        Ok(()) => {
            store.set_instance_state(&instance.id, InstanceState::Ready)?;
            let mut updated = instance.clone();
            updated.state = InstanceState::Ready;
            Ok(updated)
        }
        Err(error) => {
            let _ = store.set_instance_state(&instance.id, InstanceState::Failed);
            Err(error)
        }
    }
}

/// Compare the locally installed official profile against the current official
/// manifest. Any difference in the artifact-bearing parts (client JAR, asset
/// index, libraries, logging config) means Mojang republished files and the
/// repair install should run. Loader overlays are version-scoped and untouched.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClientUpdateStatus {
    pub up_to_date: bool,
    pub changed_files: usize,
}

pub async fn check_client_update(
    client: &reqwest::Client,
    instance: &InstanceRecord,
) -> Result<ClientUpdateStatus, InstallError> {
    if instance.state != InstanceState::Ready {
        return Err(InstallError::InvalidMetadata(
            "请先完成安装后再检查客户端更新。".to_owned(),
        ));
    }
    let local_path = instance
        .instance_dir
        .join("versions")
        .join(&instance.game_version)
        .join(format!("{}.json", instance.game_version));
    let local_bytes = tokio::fs::read(&local_path).await?;
    let local: GameVersionMetadata = serde_json::from_slice(&local_bytes).map_err(|error| {
        InstallError::InvalidMetadata(format!("local profile parse failed: {error}"))
    })?;
    let manifest = fetch_manifest(client, SourcePreference::Official).await?;
    let remote = fetch_version_metadata(
        client,
        &manifest,
        &instance.game_version,
        SourcePreference::Official,
    )
    .await?;
    let changed = artifact_fingerprint(&remote) != artifact_fingerprint(&local);
    Ok(ClientUpdateStatus {
        up_to_date: !changed,
        changed_files: usize::from(changed),
    })
}

/// Stable digest over the artifact-bearing parts of a profile.
fn artifact_fingerprint(metadata: &GameVersionMetadata) -> String {
    #[derive(serde::Serialize)]
    struct ArtifactView<'a> {
        downloads: &'a std::collections::HashMap<String, Artifact>,
        asset_index: Option<&'a AssetIndexRef>,
        libraries: &'a [Library],
        logging: Option<&'a serde_json::Value>,
    }
    let view = ArtifactView {
        downloads: &metadata.downloads,
        asset_index: metadata.asset_index.as_ref(),
        libraries: &metadata.libraries,
        logging: metadata.logging.as_ref(),
    };
    let bytes = serde_json::to_vec(&view).expect("metadata view serialization");
    hex::encode(Sha256::digest(&bytes))
}

async fn install_inner(
    client: &reqwest::Client,
    engine: DownloadEngine,
    data_root: &Path,
    instance: &InstanceRecord,
    progress: Option<mpsc::UnboundedSender<DownloadProgress>>,
) -> Result<(), InstallError> {
    let control = engine.control();
    control.checkpoint().await?;
    let manifest = fetch_manifest(client, SourcePreference::Official).await?;
    control.checkpoint().await?;
    let metadata = fetch_version_metadata(
        client,
        &manifest,
        &instance.game_version,
        engine.settings().source,
    )
    .await?;
    if metadata.inherits_from.is_some() {
        return Err(InstallError::InheritedVersion);
    }

    tokio::fs::create_dir_all(&instance.instance_dir).await?;
    tokio::fs::create_dir_all(&instance.game_dir).await?;
    let version_dir = instance.instance_dir.join("versions").join(&metadata.id);
    let libraries_dir = data_root.join("libraries");
    let assets_dir = data_root.join("assets");
    let natives_dir = version_dir.join("natives");
    tokio::fs::create_dir_all(&version_dir).await?;
    tokio::fs::create_dir_all(&libraries_dir).await?;
    tokio::fs::create_dir_all(&assets_dir).await?;
    tokio::fs::create_dir_all(&natives_dir).await?;

    let version_json_path = version_dir.join(format!("{}.json", metadata.id));
    tokio::fs::write(
        &version_json_path,
        serde_json::to_vec_pretty(&metadata)
            .map_err(|error| InstallError::InvalidMetadata(error.to_string()))?,
    )
    .await?;

    let mut artifacts = BTreeMap::<PathBuf, ArtifactSpec>::new();
    let mut native_archives = Vec::new();
    let client_download = metadata
        .downloads
        .get("client")
        .ok_or_else(|| InstallError::InvalidMetadata("client JAR entry is absent".to_owned()))?;
    let client_jar_path = version_dir.join(format!("{}.jar", metadata.id));
    let client_mirror = format!(
        "https://bmclapi2.bangbang93.com/version/{}/client",
        metadata.id
    );
    add_artifact(
        &mut artifacts,
        "client-jar",
        &format!("Minecraft {} 客户端", metadata.id),
        client_download,
        client_jar_path.clone(),
        Some(client_mirror),
    );

    for library in &metadata.libraries {
        if !library_allowed(library) {
            continue;
        }
        if let Some(artifact) = &library.downloads.artifact {
            let relative = safe_library_path(
                artifact
                    .path
                    .as_deref()
                    .map(PathBuf::from)
                    .or_else(|| maven_path(&library.name))
                    .ok_or_else(|| {
                        InstallError::InvalidMetadata(format!(
                            "invalid Maven coordinate: {}",
                            library.name
                        ))
                    })?,
            )?;
            add_artifact(
                &mut artifacts,
                &format!("library:{}", library.name),
                &format!("依赖 {}", library.name),
                artifact,
                libraries_dir.join(relative),
                None,
            );
        }
        if let Some((classifier, extract)) = native_classifier(library) {
            if let Some(artifact) = library.downloads.classifiers.get(&classifier) {
                let relative = safe_library_path(
                    artifact
                        .path
                        .as_deref()
                        .map(PathBuf::from)
                        .or_else(|| maven_path(&library.name))
                        .ok_or_else(|| {
                            InstallError::InvalidMetadata(format!(
                                "invalid native library: {}",
                                library.name
                            ))
                        })?,
                )?;
                let archive_path = libraries_dir.join(relative);
                add_artifact(
                    &mut artifacts,
                    &format!("native:{}", library.name),
                    &format!("原生依赖 {}", library.name),
                    artifact,
                    archive_path.clone(),
                    None,
                );
                native_archives.push(NativeArchive {
                    archive: archive_path,
                    destination: natives_dir.clone(),
                    excludes: extract.exclude.clone(),
                });
            }
        }
    }

    if let Some(logging) = &metadata.logging {
        if let Some(client_logging) = logging.get("client") {
            let file = client_logging.get("file").ok_or_else(|| {
                InstallError::InvalidMetadata("client logging file entry is absent".to_owned())
            })?;
            let id = file
                .get("id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    InstallError::InvalidMetadata("client logging id is absent".to_owned())
                })?;
            let url = file
                .get("url")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    InstallError::InvalidMetadata("client logging URL is absent".to_owned())
                })?;
            let target = assets_dir.join("log_configs").join(id);
            artifacts.insert(
                target.clone(),
                ArtifactSpec {
                    id: format!("logging:{id}"),
                    label: format!("日志配置 {id}"),
                    url: url.to_owned(),
                    mirror_url: None,
                    destination: target,
                    size: file.get("size").and_then(serde_json::Value::as_u64),
                    sha1: file
                        .get("sha1")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned),
                    sha256: None,
                },
            );
        }
    }

    let mut asset_index_path = None;
    if let Some(asset_index) = &metadata.asset_index {
        let index_path = assets_dir
            .join("indexes")
            .join(format!("{}.json", asset_index.id));
        let index_artifact = ArtifactSpec {
            id: format!("asset-index:{}", asset_index.id),
            label: format!("资源索引 {}", asset_index.id),
            url: asset_index.url.clone(),
            mirror_url: None,
            destination: index_path.clone(),
            size: asset_index.size,
            sha1: asset_index.sha1.clone(),
            sha256: None,
        };
        artifacts.insert(index_path.clone(), index_artifact);
        asset_index_path = Some((index_path, asset_index.id.clone()));
    }

    download_artifacts(&engine, artifacts.into_values().collect(), progress.clone()).await?;

    for archive in native_archives {
        control.checkpoint().await?;
        let path = archive.archive.clone();
        let destination = archive.destination.clone();
        let excludes = archive.excludes.clone();
        tokio::task::spawn_blocking(move || extract_native_archive(&path, &destination, &excludes))
            .await
            .map_err(|error| InstallError::Native(error.to_string()))??;
    }

    if let Some((index_path, index_id)) = asset_index_path {
        let bytes = tokio::fs::read(&index_path).await?;
        let index: AssetIndex = serde_json::from_slice(&bytes).map_err(|error| {
            InstallError::InvalidMetadata(format!("asset index parse failed: {error}"))
        })?;
        let mut objects = Vec::with_capacity(index.objects.len());
        let mut virtual_assets = Vec::new();
        let mut resource_assets = Vec::new();
        for (name, object) in index.objects {
            let relative_name = safe_relative_asset_path(&name)?;
            if object.hash.len() < 2
                || !object
                    .hash
                    .chars()
                    .all(|character| character.is_ascii_hexdigit())
            {
                return Err(InstallError::InvalidMetadata(format!(
                    "invalid asset hash for {name}"
                )));
            }
            let shard = &object.hash[..2];
            let url = format!(
                "https://resources.download.minecraft.net/{shard}/{}",
                object.hash
            );
            let mirror_url = format!(
                "https://bmclapi2.bangbang93.com/assets/{shard}/{}",
                object.hash
            );
            let destination = assets_dir.join("objects").join(shard).join(&object.hash);
            if index.virtual_assets {
                virtual_assets.push((
                    destination.clone(),
                    assets_dir
                        .join("virtual")
                        .join(&index_id)
                        .join(&relative_name),
                ));
            }
            if index.map_to_resources {
                resource_assets.push((
                    destination.clone(),
                    instance.game_dir.join("resources").join(&relative_name),
                ));
            }
            objects.push(ArtifactSpec {
                id: format!("asset:{}", object.hash),
                label: format!("资源 {}", name),
                url,
                mirror_url: Some(mirror_url),
                destination,
                size: Some(object.size),
                sha1: Some(object.hash),
                sha256: None,
            });
        }
        download_artifacts(&engine, objects, progress).await?;
        let copies = virtual_assets
            .into_iter()
            .chain(resource_assets)
            .collect::<Vec<_>>();
        tokio::task::spawn_blocking(move || materialize_assets(&copies))
            .await
            .map_err(|error| InstallError::Native(error.to_string()))??;
        let _ = index_id;
    }

    control.checkpoint().await?;
    Ok(())
}

#[cfg(test)]
mod asset_tests {
    use super::*;

    #[test]
    fn client_update_fingerprint_tracks_artifact_changes_only() {
        let base: GameVersionMetadata = serde_json::from_value(serde_json::json!({
            "id": "1.21.4",
            "downloads": {"client": {"url": "https://piston-meta.mojang.com/a.jar", "sha1": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "size": 1}},
            "assetIndex": {"id": "5", "url": "https://piston-meta.mojang.com/5.json", "sha1": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "size": 2, "totalSize": 3},
            "libraries": [{"name": "g:a:1", "downloads": {"artifact": {"url": "https://libraries.minecraft.net/g.jar", "sha1": "cccccccccccccccccccccccccccccccccccccccc", "size": 4}}}],
            "arguments": {"game": ["--x"], "jvm": ["-Dy=1"]}
        })).unwrap();
        let mut republished = base.clone();
        republished
            .downloads
            .get_mut("client")
            .unwrap()
            .sha1
            .replace("dddddddddddddddddddddddddddddddddddddddd".to_owned());
        assert_ne!(
            artifact_fingerprint(&republished),
            artifact_fingerprint(&base)
        );
        // Cosmetic-only changes (release time/type/argument text) do not trigger a repair.
        let mut cosmetic = base.clone();
        cosmetic
            .extra
            .insert("time".to_owned(), serde_json::json!("2026-01-01T00:00:00Z"));
        assert_eq!(artifact_fingerprint(&cosmetic), artifact_fingerprint(&base));
    }

    #[cfg(unix)]
    #[test]
    fn virtual_asset_materialization_refuses_symlink_destinations() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!("mfmcl-asset-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("source");
        let victim = root.join("victim");
        let destination = root.join("virtual/item");
        std::fs::write(&source, b"verified asset").unwrap();
        std::fs::write(&victim, b"keep safe").unwrap();
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        symlink(&victim, &destination).unwrap();
        assert!(materialize_assets(&[(source, destination)]).is_err());
        assert_eq!(std::fs::read(victim).unwrap(), b"keep safe");
        std::fs::remove_dir_all(root).unwrap();
    }
}

pub(crate) fn safe_library_path(path: PathBuf) -> Result<PathBuf, InstallError> {
    let text = path
        .to_str()
        .ok_or_else(|| InstallError::InvalidMetadata("non-UTF8 library path".into()))?;
    crate::modpacks::safe_pack_path(text).map_err(InstallError::InvalidMetadata)?;
    Ok(path)
}

fn safe_relative_asset_path(name: &str) -> Result<PathBuf, InstallError> {
    crate::modpacks::safe_pack_path(name).map_err(InstallError::InvalidMetadata)
}

fn materialize_assets(copies: &[(PathBuf, PathBuf)]) -> Result<(), InstallError> {
    for (source, destination) in copies {
        if !source.is_file() {
            return Err(InstallError::InvalidMetadata(format!(
                "asset object is missing: {}",
                source.display()
            )));
        }
        match std::fs::symlink_metadata(destination) {
            Ok(metadata) if metadata.file_type().is_file() => continue,
            Ok(_) => {
                return Err(InstallError::InvalidMetadata(
                    "asset destination is not a regular file".into(),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match std::fs::hard_link(source, destination) {
            Ok(()) => {}
            Err(_) => {
                let parent = destination.parent().ok_or_else(|| {
                    InstallError::InvalidMetadata("asset destination has no parent".into())
                })?;
                let temp = parent.join(format!(".asset-{}.pending", uuid::Uuid::new_v4()));
                let mut output = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&temp)?;
                let result = (|| {
                    let mut input = File::open(source)?;
                    io::copy(&mut input, &mut output)?;
                    output.sync_all()?;
                    std::fs::hard_link(&temp, destination)?;
                    Ok::<(), io::Error>(())
                })();
                drop(output);
                if let Err(error) = result {
                    let _ = std::fs::remove_file(&temp);
                    return Err(error.into());
                }
                std::fs::remove_file(&temp)?;
            }
        }
    }
    Ok(())
}

fn add_artifact(
    artifacts: &mut BTreeMap<PathBuf, ArtifactSpec>,
    id: &str,
    label: &str,
    source: &mc_metadata::Artifact,
    destination: PathBuf,
    mirror_url: Option<String>,
) {
    artifacts
        .entry(destination.clone())
        .or_insert_with(|| ArtifactSpec {
            id: id.to_owned(),
            label: label.to_owned(),
            url: source.url.clone(),
            mirror_url,
            destination,
            size: source.size,
            sha1: source.sha1.clone(),
            sha256: None,
        });
}

async fn download_artifacts(
    engine: &DownloadEngine,
    artifacts: Vec<ArtifactSpec>,
    progress: Option<mpsc::UnboundedSender<DownloadProgress>>,
) -> Result<(), InstallError> {
    let concurrency = engine.settings().max_concurrent_downloads.clamp(1, 16) as usize;
    futures_util::stream::iter(artifacts)
        .map(|artifact| {
            let engine = engine.clone();
            let progress = progress.clone();
            async move { engine.download(&artifact, progress).await }
        })
        .buffer_unordered(concurrency)
        .try_collect::<Vec<_>>()
        .await?;
    Ok(())
}

pub(crate) fn library_allowed(library: &Library) -> bool {
    if library.rules.is_empty() {
        return true;
    }
    let features = [
        ("is_demo_user", false),
        ("has_custom_resolution", false),
        ("has_quick_plays_support", false),
        ("is_quick_play_singleplayer", false),
        ("is_quick_play_multiplayer", false),
        ("is_quick_play_realms", false),
    ];
    let mut allowed = false;
    for rule in &library.rules {
        if rule_matches(rule, &features) {
            allowed = rule.action == "allow";
        }
    }
    allowed
}

pub(crate) fn rule_matches(rule: &LibraryRule, features: &[(&str, bool)]) -> bool {
    if let Some(os) = &rule.os {
        let current_os = match std::env::consts::OS {
            "macos" => "osx",
            other => other,
        };
        if os.name.as_deref().is_some_and(|name| name != current_os) {
            return false;
        }
        if let Some(pattern) = &os.arch {
            let Ok(regex) = Regex::new(pattern) else {
                return false;
            };
            if !regex.is_match(std::env::consts::ARCH) {
                return false;
            }
        }
        if let Some(pattern) = &os.version {
            let Some(version) = current_os_version() else {
                return false;
            };
            let Ok(regex) = Regex::new(pattern) else {
                return false;
            };
            if !regex.is_match(&version) {
                return false;
            }
        }
    }
    if let Some(expected_features) = &rule.features {
        for (name, expected) in expected_features {
            let actual = features
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| *value)
                .unwrap_or(false);
            if actual != *expected {
                return false;
            }
        }
    }
    true
}

fn current_os_version() -> Option<String> {
    #[cfg(target_os = "macos")]
    let output = std::process::Command::new("sw_vers")
        .args(["-productVersion"])
        .output()
        .ok()?;
    #[cfg(target_os = "windows")]
    let output = std::process::Command::new("cmd")
        .args(["/C", "ver"])
        .output()
        .ok()?;
    #[cfg(target_os = "linux")]
    let output = std::process::Command::new("uname")
        .args(["-r"])
        .output()
        .ok()?;
    String::from_utf8(output.stdout)
        .ok()
        .map(|version| version.trim().to_owned())
}

fn native_classifier(library: &Library) -> Option<(String, mc_metadata::NativeExtractRules)> {
    let os = match std::env::consts::OS {
        "macos" => "osx",
        other => other,
    };
    let template = library.natives.get(os)?;
    let arch = if std::env::consts::ARCH == "x86" {
        "32"
    } else {
        "64"
    };
    let classifier = template.replace("${arch}", arch);
    Some((classifier, library.extract.clone().unwrap_or_default()))
}

pub(crate) fn maven_path(coordinate: &str) -> Option<PathBuf> {
    let (coordinate, extension) = coordinate.split_once('@').unwrap_or((coordinate, "jar"));
    let parts: Vec<&str> = coordinate.split(':').collect();
    if parts.len() < 3 || parts.len() > 4 {
        return None;
    }
    let group_path = parts[0].replace('.', "/");
    let classifier = parts
        .get(3)
        .map(|value| format!("-{value}"))
        .unwrap_or_default();
    Some(
        PathBuf::from(group_path)
            .join(parts[1])
            .join(parts[2])
            .join(format!(
                "{}-{}{}.{extension}",
                parts[1], parts[2], classifier
            )),
    )
}

fn extract_native_archive(
    archive_path: &Path,
    destination: &Path,
    excludes: &[String],
) -> Result<(), InstallError> {
    std::fs::create_dir_all(destination)?;
    let file = File::open(archive_path)?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| InstallError::Native(format!("invalid native archive: {error}")))?;
    if archive.len() > 20_000 {
        return Err(InstallError::Native(
            "native archive contains too many entries".to_owned(),
        ));
    }
    let mut total_uncompressed = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| {
            InstallError::Native(format!("could not read native archive: {error}"))
        })?;
        if entry.size() > 32 * 1024 * 1024 {
            return Err(InstallError::Native(
                "native archive entry exceeds size limit".to_owned(),
            ));
        }
        total_uncompressed = total_uncompressed.saturating_add(entry.size());
        if total_uncompressed > 256 * 1024 * 1024 {
            return Err(InstallError::Native(
                "native archive exceeds total size limit".to_owned(),
            ));
        }
        let Some(relative) = entry.enclosed_name() else {
            continue;
        };
        if relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            continue;
        }
        let normalized = relative.to_string_lossy().replace('\\', "/");
        if normalized.starts_with("META-INF/")
            || normalized == "META-INF"
            || excludes
                .iter()
                .any(|exclude| normalized.starts_with(exclude.trim_end_matches("/**")))
        {
            continue;
        }
        let output = destination.join(&relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&output)?;
        } else {
            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut target = File::create(output)?;
            io::copy(&mut entry, &mut target)?;
        }
    }
    Ok(())
}
