use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::collections::{BTreeMap, HashMap};
use thiserror::Error;
use url::Url;

pub const OFFICIAL_MANIFEST_URL: &str =
    "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
pub const BMCLAPI_MANIFEST_URL: &str =
    "https://bmclapi2.bangbang93.com/mc/game/version_manifest.json";

#[derive(Debug, Error)]
pub enum MetadataError {
    #[error("network request failed: {0}")]
    Network(#[from] reqwest::Error),
    #[error("metadata could not be parsed: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("no version manifest source responded successfully")]
    NoSource,
    #[error("official metadata hash check failed for {0}")]
    Integrity(String),
    #[error("Minecraft version {0} was not found in the version manifest")]
    VersionNotFound(String),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SourcePreference {
    #[default]
    Official,
    Bmclapi,
    Mixed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionManifest {
    pub latest: LatestVersions,
    pub versions: Vec<VersionSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatestVersions {
    pub release: String,
    pub snapshot: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionSummary {
    pub id: String,
    #[serde(rename = "type")]
    pub version_type: String,
    pub url: String,
    pub time: String,
    pub release_time: String,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub compliance_level: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameVersionMetadata {
    pub id: String,
    #[serde(default)]
    pub inherits_from: Option<String>,
    #[serde(default)]
    pub main_class: Option<String>,
    #[serde(default)]
    pub assets: Option<String>,
    #[serde(default)]
    pub asset_index: Option<AssetIndexRef>,
    #[serde(default)]
    pub java_version: Option<JavaVersionInfo>,
    #[serde(default)]
    pub downloads: HashMap<String, Artifact>,
    #[serde(default)]
    pub libraries: Vec<Library>,
    #[serde(default)]
    pub arguments: Option<LaunchArguments>,
    #[serde(default)]
    pub minecraft_arguments: Option<String>,
    #[serde(default)]
    pub logging: Option<serde_json::Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetIndexRef {
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub total_size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaVersionInfo {
    pub component: String,
    pub major_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    #[serde(default)]
    pub path: Option<String>,
    pub url: String,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Library {
    pub name: String,
    #[serde(default)]
    pub downloads: LibraryDownloads,
    #[serde(default)]
    pub rules: Vec<LibraryRule>,
    #[serde(default)]
    pub natives: HashMap<String, String>,
    #[serde(default)]
    pub extract: Option<NativeExtractRules>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LibraryDownloads {
    #[serde(default)]
    pub artifact: Option<Artifact>,
    #[serde(default)]
    pub classifiers: HashMap<String, Artifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryRule {
    pub action: String,
    #[serde(default)]
    pub os: Option<OsRule>,
    #[serde(default)]
    pub features: Option<HashMap<String, bool>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OsRule {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arch: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct NativeExtractRules {
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LaunchArguments {
    #[serde(default)]
    pub game: Vec<ArgumentEntry>,
    #[serde(default)]
    pub jvm: Vec<ArgumentEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ArgumentEntry {
    Literal(String),
    Conditional(ConditionalArgument),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConditionalArgument {
    #[serde(default)]
    pub rules: Vec<LibraryRule>,
    pub value: ArgumentValue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ArgumentValue {
    Single(String),
    Multiple(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetIndex {
    #[serde(default)]
    pub objects: HashMap<String, AssetObject>,
    #[serde(default)]
    pub virtual_assets: bool,
    #[serde(default)]
    pub map_to_resources: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetObject {
    pub hash: String,
    pub size: u64,
}

pub fn manifest_sources(_preference: SourcePreference) -> Vec<Url> {
    // The version index is the trust root for artifact hashes. Always fetch it
    // from the official endpoint; the selected mirror applies to hash-pinned files.
    vec![Url::parse(OFFICIAL_MANIFEST_URL).expect("valid official manifest URL")]
}

pub async fn fetch_manifest(
    client: &reqwest::Client,
    preference: SourcePreference,
) -> Result<VersionManifest, MetadataError> {
    let mut last_error = None;
    for url in manifest_sources(preference) {
        match client
            .get(url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
        {
            Ok(response) => match response.json::<VersionManifest>().await {
                Ok(manifest) => return Ok(manifest),
                Err(error) => last_error = Some(error),
            },
            Err(error) => last_error = Some(error),
        }
    }
    if let Some(error) = last_error {
        return Err(MetadataError::Network(error));
    }
    Err(MetadataError::NoSource)
}

pub fn version_metadata_sources(
    version: &VersionSummary,
    preference: SourcePreference,
) -> Vec<Url> {
    let official = Url::parse(&version.url).ok();
    let mirror = Url::parse(&format!(
        "https://bmclapi2.bangbang93.com/version/{}.json",
        version.id
    ))
    .ok();
    match preference {
        SourcePreference::Official => official.into_iter().collect(),
        SourcePreference::Bmclapi => mirror.into_iter().chain(official).collect(),
        SourcePreference::Mixed => mirror.into_iter().chain(official).collect(),
    }
}

pub async fn fetch_version_metadata(
    client: &reqwest::Client,
    manifest: &VersionManifest,
    version_id: &str,
    preference: SourcePreference,
) -> Result<GameVersionMetadata, MetadataError> {
    let summary = manifest
        .versions
        .iter()
        .find(|version| version.id == version_id)
        .ok_or_else(|| MetadataError::VersionNotFound(version_id.to_owned()))?;
    let mut last_error = None;
    for url in version_metadata_sources(summary, preference) {
        let response = match client
            .get(url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
        {
            Ok(response) => response,
            Err(error) => {
                last_error = Some(MetadataError::Network(error));
                continue;
            }
        };
        let bytes = match response.bytes().await {
            Ok(bytes) => bytes,
            Err(error) => {
                last_error = Some(MetadataError::Network(error));
                continue;
            }
        };
        if let Some(expected_hash) = &summary.sha1 {
            let actual_hash = hex::encode(Sha1::digest(&bytes));
            if !actual_hash.eq_ignore_ascii_case(expected_hash) {
                last_error = Some(MetadataError::Integrity(summary.id.clone()));
                continue;
            }
        }
        match serde_json::from_slice::<GameVersionMetadata>(&bytes) {
            Ok(metadata) => return Ok(metadata),
            Err(error) => last_error = Some(MetadataError::Parse(error)),
        }
    }
    Err(last_error.unwrap_or(MetadataError::NoSource))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preference_uses_the_official_manifest_as_trust_root() {
        for preference in [
            SourcePreference::Official,
            SourcePreference::Bmclapi,
            SourcePreference::Mixed,
        ] {
            let urls = manifest_sources(preference);
            assert_eq!(urls.len(), 1);
            assert!(urls[0].host_str().unwrap().contains("mojang.com"));
        }
    }
}
