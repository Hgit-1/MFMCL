pub mod aria2;

use futures_util::StreamExt;
use mc_metadata::SourcePreference;
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use sha2::Sha256;
use std::path::{Path, PathBuf};
use thiserror::Error;
use tokio::{fs, io::AsyncWriteExt, sync::mpsc};
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("download request failed: {0}")]
    Network(#[from] reqwest::Error),
    #[error("file operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid URL: {0}")]
    Url(#[from] url::ParseError),
    #[error("aria2 download failed: {0}")]
    Aria2(String),
    #[error("no download source is available for {0}")]
    NoSource(String),
    #[error("downloaded file failed integrity validation: {0}")]
    Integrity(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadSettings {
    pub source: SourcePreference,
    pub max_concurrent_downloads: u8,
    pub connections_per_file: u8,
    pub aria2_enabled: bool,
}

impl Default for DownloadSettings {
    fn default() -> Self {
        Self {
            source: SourcePreference::Official,
            max_concurrent_downloads: 4,
            connections_per_file: 4,
            aria2_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactSpec {
    pub id: String,
    pub label: String,
    pub url: String,
    #[serde(default)]
    pub mirror_url: Option<String>,
    pub destination: PathBuf,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub artifact_id: String,
    pub label: String,
    pub received: u64,
    pub total: Option<u64>,
    pub source: String,
    pub stage: DownloadStage,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DownloadStage {
    Connecting,
    Downloading,
    Verifying,
    Completed,
    Failed,
}

#[derive(Clone)]
pub struct DownloadEngine {
    client: Client,
    settings: DownloadSettings,
    aria2: Option<aria2::Aria2Session>,
}

impl DownloadEngine {
    pub fn new(settings: DownloadSettings) -> Result<Self, reqwest::Error> {
        let client = Client::builder()
            .user_agent(concat!("MFMCL/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(300))
            .build()?;
        Ok(Self {
            client,
            settings,
            aria2: None,
        })
    }

    pub fn with_aria2(mut self, session: aria2::Aria2Session) -> Self {
        self.aria2 = Some(session);
        self
    }

    pub fn settings(&self) -> &DownloadSettings {
        &self.settings
    }

    pub async fn download(
        &self,
        artifact: &ArtifactSpec,
        progress: Option<mpsc::UnboundedSender<DownloadProgress>>,
    ) -> Result<PathBuf, DownloadError> {
        let original = Url::parse(&artifact.url)?;
        let expected_hash_exists = artifact.sha1.is_some() || artifact.sha256.is_some();
        let candidates = source_candidates_with_mirror(
            original,
            artifact.mirror_url.as_deref(),
            self.settings.source,
            expected_hash_exists,
        );
        if candidates.is_empty() {
            return Err(DownloadError::NoSource(artifact.label.clone()));
        }

        if artifact.destination.exists()
            && verify_file(
                &artifact.destination,
                artifact.size,
                artifact.sha1.as_deref(),
                artifact.sha256.as_deref(),
            )
            .await
            .is_ok()
        {
            return Ok(artifact.destination.clone());
        }

        if let Some(parent) = artifact.destination.parent() {
            fs::create_dir_all(parent).await?;
        }

        let mut last_error = None;
        for (source_index, source) in candidates.iter().enumerate() {
            let temporary = artifact
                .destination
                .with_extension(format!("part-{}", Uuid::new_v4().simple()));
            if let Some(tx) = &progress {
                let _ = tx.send(DownloadProgress {
                    artifact_id: artifact.id.clone(),
                    label: artifact.label.clone(),
                    received: 0,
                    total: artifact.size,
                    source: source.to_string(),
                    stage: DownloadStage::Connecting,
                });
            }

            let result = if let Some(session) = &self.aria2 {
                let use_multi_source = source_index == 0
                    && self.settings.source == SourcePreference::Mixed
                    && candidates.len() > 1
                    && expected_hash_exists
                    && all_sources_support_range(&self.client, &candidates, artifact.size).await;
                let selected_sources = if use_multi_source {
                    candidates.as_slice()
                } else {
                    std::slice::from_ref(source)
                };
                self.download_with_aria2(
                    artifact,
                    selected_sources,
                    session,
                    &temporary,
                    progress.clone(),
                )
                .await
            } else {
                self.download_one(artifact, source, &temporary, progress.clone())
                    .await
            };
            match result {
                Ok(()) => {
                    if let Err(error) = fs::rename(&temporary, &artifact.destination).await {
                        // Windows does not replace an existing destination with rename.
                        if artifact.destination.exists() {
                            fs::remove_file(&artifact.destination).await?;
                            if let Err(retry_error) =
                                fs::rename(&temporary, &artifact.destination).await
                            {
                                let _ = fs::remove_file(&temporary).await;
                                return Err(DownloadError::Io(retry_error));
                            }
                        } else {
                            let _ = fs::remove_file(&temporary).await;
                            return Err(DownloadError::Io(error));
                        }
                    }
                    if let Some(tx) = &progress {
                        let _ = tx.send(DownloadProgress {
                            artifact_id: artifact.id.clone(),
                            label: artifact.label.clone(),
                            received: artifact.size.unwrap_or_default(),
                            total: artifact.size,
                            source: source.to_string(),
                            stage: DownloadStage::Completed,
                        });
                    }
                    return Ok(artifact.destination.clone());
                }
                Err(error) => {
                    let _ = fs::remove_file(&temporary).await;
                    last_error = Some(error);
                }
            }
        }

        if let Some(error) = last_error {
            if let Some(tx) = progress {
                let _ = tx.send(DownloadProgress {
                    artifact_id: artifact.id.clone(),
                    label: artifact.label.clone(),
                    received: 0,
                    total: artifact.size,
                    source: String::new(),
                    stage: DownloadStage::Failed,
                });
            }
            return Err(error);
        }
        Err(DownloadError::NoSource(artifact.label.clone()))
    }

    async fn download_with_aria2(
        &self,
        artifact: &ArtifactSpec,
        sources: &[Url],
        session: &aria2::Aria2Session,
        temporary: &Path,
        progress: Option<mpsc::UnboundedSender<DownloadProgress>>,
    ) -> Result<(), DownloadError> {
        let parent = temporary.parent().unwrap_or_else(|| Path::new("."));
        let file_name = temporary
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| DownloadError::NoSource(artifact.label.clone()))?;
        let urls: Vec<String> = sources.iter().map(ToString::to_string).collect();
        let checksum = artifact
            .sha256
            .as_ref()
            .map(|hash| format!("sha-256={hash}"))
            .or_else(|| artifact.sha1.as_ref().map(|hash| format!("sha-1={hash}")));
        let gid = session
            .add_uri(&urls, parent, file_name, checksum.as_deref())
            .await
            .map_err(|error| DownloadError::Aria2(error.to_string()))?;
        let started = std::time::Instant::now();
        loop {
            if started.elapsed() > std::time::Duration::from_secs(30 * 60) {
                let _ = session.remove(&gid).await;
                return Err(DownloadError::Aria2(
                    "download timed out after 30 minutes".to_owned(),
                ));
            }
            let status = match session.tell_status(&gid).await {
                Ok(status) => status,
                Err(error) => {
                    let _ = session.remove(&gid).await;
                    return Err(DownloadError::Aria2(error.to_string()));
                }
            };
            let completed = status.completed_length.parse::<u64>().unwrap_or_default();
            let total = status
                .total_length
                .parse::<u64>()
                .ok()
                .filter(|value| *value > 0)
                .or(artifact.size);
            if let Some(tx) = &progress {
                let _ = tx.send(DownloadProgress {
                    artifact_id: artifact.id.clone(),
                    label: artifact.label.clone(),
                    received: completed,
                    total,
                    source: urls.join(" + "),
                    stage: if status.status == "complete" {
                        DownloadStage::Verifying
                    } else {
                        DownloadStage::Downloading
                    },
                });
            }
            match status.status.as_str() {
                "complete" => break,
                "error" => {
                    return Err(DownloadError::Aria2(
                        status
                            .error_message
                            .unwrap_or_else(|| "unknown aria2 error".to_owned()),
                    ));
                }
                "removed" => {
                    return Err(DownloadError::Aria2("download task was removed".to_owned()));
                }
                _ => tokio::time::sleep(std::time::Duration::from_millis(250)).await,
            }
        }
        verify_file(
            temporary,
            artifact.size,
            artifact.sha1.as_deref(),
            artifact.sha256.as_deref(),
        )
        .await
    }

    async fn download_one(
        &self,
        artifact: &ArtifactSpec,
        source: &Url,
        temporary: &Path,
        progress: Option<mpsc::UnboundedSender<DownloadProgress>>,
    ) -> Result<(), DownloadError> {
        let response = self
            .client
            .get(source.clone())
            .send()
            .await?
            .error_for_status()?;
        let total = response.content_length().or(artifact.size);
        let mut file = fs::File::create(temporary).await?;
        let mut stream = response.bytes_stream();
        let mut received = 0_u64;
        let mut sha1 = Sha1::new();
        let mut sha256 = Sha256::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            received += chunk.len() as u64;
            sha1.update(&chunk);
            sha256.update(&chunk);
            if let Some(tx) = &progress {
                let _ = tx.send(DownloadProgress {
                    artifact_id: artifact.id.clone(),
                    label: artifact.label.clone(),
                    received,
                    total,
                    source: source.to_string(),
                    stage: DownloadStage::Downloading,
                });
            }
        }
        file.flush().await?;
        drop(file);

        if let Some(expected) = artifact.size {
            if received != expected {
                return Err(DownloadError::Integrity(format!(
                    "{} byte count differs from metadata (expected {expected}, got {received})",
                    artifact.label
                )));
            }
        }
        if let Some(expected) = &artifact.sha1 {
            let actual = hex::encode(sha1.finalize());
            if !actual.eq_ignore_ascii_case(expected) {
                return Err(DownloadError::Integrity(format!(
                    "{} SHA-1 mismatch",
                    artifact.label
                )));
            }
        }
        if let Some(expected) = &artifact.sha256 {
            let actual = hex::encode(sha256.finalize());
            if !actual.eq_ignore_ascii_case(expected) {
                return Err(DownloadError::Integrity(format!(
                    "{} SHA-256 mismatch",
                    artifact.label
                )));
            }
        }
        if let Some(tx) = &progress {
            let _ = tx.send(DownloadProgress {
                artifact_id: artifact.id.clone(),
                label: artifact.label.clone(),
                received,
                total,
                source: source.to_string(),
                stage: DownloadStage::Verifying,
            });
        }
        Ok(())
    }
}

async fn all_sources_support_range(
    client: &Client,
    sources: &[Url],
    expected_size: Option<u64>,
) -> bool {
    for source in sources {
        let response = match tokio::time::timeout(
            std::time::Duration::from_secs(6),
            client
                .get(source.clone())
                .header(reqwest::header::RANGE, "bytes=0-0")
                .send(),
        )
        .await
        {
            Ok(Ok(response)) if response.status() == reqwest::StatusCode::PARTIAL_CONTENT => {
                response
            }
            _ => return false,
        };
        let Some(content_range) = response.headers().get(reqwest::header::CONTENT_RANGE) else {
            return false;
        };
        let Ok(content_range) = content_range.to_str() else {
            return false;
        };
        let Some(total) = content_range.strip_prefix("bytes 0-0/") else {
            return false;
        };
        let Ok(total) = total.parse::<u64>() else {
            return false;
        };
        if expected_size.is_some_and(|expected| expected != total) {
            return false;
        }
        match response.bytes().await {
            Ok(bytes) if bytes.len() == 1 => {}
            _ => return false,
        }
    }
    true
}

pub fn source_candidates(
    original: Url,
    preference: SourcePreference,
    checksum_available: bool,
) -> Vec<Url> {
    source_candidates_with_mirror(original, None, preference, checksum_available)
}

pub fn source_candidates_with_mirror(
    original: Url,
    explicit_mirror: Option<&str>,
    preference: SourcePreference,
    checksum_available: bool,
) -> Vec<Url> {
    let mut candidates = Vec::new();
    if preference != SourcePreference::Official && checksum_available {
        if let Some(mirror) = explicit_mirror
            .and_then(|value| Url::parse(value).ok())
            .or_else(|| bmclapi_url(&original))
        {
            candidates.push(mirror);
        }
    }
    candidates.push(original);
    candidates.dedup();
    candidates
}

/// Maps only known Mojang artifact hosts to documented BMCLAPI path families.
/// Unknown hosts remain official-only rather than being rewritten speculatively.
pub fn bmclapi_url(original: &Url) -> Option<Url> {
    let host = original.host_str()?;
    let mut mirror = Url::parse("https://bmclapi2.bangbang93.com/").ok()?;
    let path = original.path();
    let mapped_path = match host {
        "resources.download.minecraft.net" => format!("/assets{}", path),
        "libraries.minecraft.net" => format!("/libraries{}", path),
        "piston-meta.mojang.com" if path == "/mc/game/version_manifest_v2.json" => {
            "/mc/game/version_manifest.json".to_owned()
        }
        "piston-meta.mojang.com" => {
            let version_file = path.rsplit('/').next()?;
            if !version_file.ends_with(".json") || !path.contains("/v1/packages/") {
                return None;
            }
            format!("/version/{version_file}")
        }
        _ => return None,
    };
    mirror.set_path(&mapped_path);
    mirror.set_query(original.query());
    Some(mirror)
}

pub async fn verify_file(
    path: &Path,
    expected_size: Option<u64>,
    expected_sha1: Option<&str>,
    expected_sha256: Option<&str>,
) -> Result<(), DownloadError> {
    let metadata = fs::metadata(path).await?;
    if let Some(expected) = expected_size {
        if metadata.len() != expected {
            return Err(DownloadError::Integrity(format!(
                "{} byte count mismatch",
                path.display()
            )));
        }
    }
    if expected_sha1.is_none() && expected_sha256.is_none() {
        return Ok(());
    }

    let mut file = fs::File::open(path).await?;
    let mut sha1 = Sha1::new();
    let mut sha256 = Sha256::new();
    let mut buffer = vec![0_u8; 128 * 1024];
    loop {
        let read = tokio::io::AsyncReadExt::read(&mut file, &mut buffer).await?;
        if read == 0 {
            break;
        }
        sha1.update(&buffer[..read]);
        sha256.update(&buffer[..read]);
    }
    if let Some(expected) = expected_sha1 {
        if !hex::encode(sha1.finalize()).eq_ignore_ascii_case(expected) {
            return Err(DownloadError::Integrity(format!(
                "{} SHA-1 mismatch",
                path.display()
            )));
        }
    }
    if let Some(expected) = expected_sha256 {
        if !hex::encode(sha256.finalize()).eq_ignore_ascii_case(expected) {
            return Err(DownloadError::Integrity(format!(
                "{} SHA-256 mismatch",
                path.display()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_known_mojang_hosts_only() {
        let original = Url::parse("https://resources.download.minecraft.net/ab/abcdef").unwrap();
        let mirror = bmclapi_url(&original).unwrap();
        assert_eq!(
            mirror.as_str(),
            "https://bmclapi2.bangbang93.com/assets/ab/abcdef"
        );
        let unknown = Url::parse("https://example.com/file.jar").unwrap();
        assert!(bmclapi_url(&unknown).is_none());
    }

    #[test]
    fn mirror_requires_a_hash_before_it_is_a_candidate() {
        let original = Url::parse("https://libraries.minecraft.net/org/example/file.jar").unwrap();
        let no_hash = source_candidates(original.clone(), SourcePreference::Mixed, false);
        assert_eq!(no_hash, vec![original.clone()]);
        let with_hash = source_candidates(original, SourcePreference::Mixed, true);
        assert_eq!(with_hash.len(), 2);
        assert!(with_hash[0].host_str().unwrap().contains("bangbang93.com"));
    }
}
