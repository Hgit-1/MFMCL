//! Launcher self-update manifest parsing and version comparison.
//!
//! The manifest format is original to MFMCL. Files are downloaded and verified
//! by the desktop host with the shared download engine; this module only parses
//! and compares so it stays deterministic and unit-testable.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

const MAX_MANIFEST_BYTES: usize = 256 * 1024;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum UpdateError {
    #[error("更新清单不是有效的 UTF-8 文本")]
    InvalidText,
    #[error("更新清单超过大小限制")]
    TooLarge,
    #[error("更新清单格式无效：{0}")]
    Invalid(&'static str),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlatformRelease {
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateManifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    pub platforms: BTreeMap<String, PlatformRelease>,
}

/// Semantic-ish version: `major.minor.patch` with an optional `-pre.N` suffix.
/// Pre-release builds sort before the same release version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Option<u64>,
}

impl Version {
    pub fn parse(value: &str) -> Result<Self, UpdateError> {
        let (core, pre) = match value.split_once('-') {
            Some((core, pre)) => {
                let digits = pre
                    .strip_prefix("pre.")
                    .or_else(|| pre.strip_prefix("beta."));
                let number = digits
                    .or_else(|| pre.strip_prefix('b'))
                    .and_then(|text| text.parse::<u64>().ok())
                    .ok_or(UpdateError::Invalid("unsupported pre-release tag"))?;
                (core, Some(number))
            }
            None => (value, None),
        };
        let parts: Vec<&str> = core.split('.').collect();
        if parts.len() != 3 {
            return Err(UpdateError::Invalid("version must be major.minor.patch"));
        }
        let mut numbers = [0_u64; 3];
        for (slot, part) in numbers.iter_mut().zip(parts) {
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return Err(UpdateError::Invalid("version segments must be numeric"));
            }
            *slot = part
                .parse::<u64>()
                .map_err(|_| UpdateError::Invalid("version segment out of range"))?;
        }
        Ok(Self {
            major: numbers[0],
            minor: numbers[1],
            patch: numbers[2],
            pre,
        })
    }

    pub fn as_str(&self) -> String {
        match self.pre {
            Some(pre) => format!("{}.{}.{}-pre.{}", self.major, self.minor, self.patch, pre),
            None => format!("{}.{}.{}", self.major, self.minor, self.patch),
        }
    }

    pub fn is_newer_than(&self, current: &Version) -> bool {
        let left = (self.major, self.minor, self.patch);
        let right = (current.major, current.minor, current.patch);
        if left != right {
            return left > right;
        }
        // Equal core: only a release build is newer than a pre-release.
        current.pre.is_some() && self.pre.is_none()
    }
}

pub fn platform_key(os: &str, arch: &str) -> String {
    // Keys use the raw OS/architecture names from the manifest; no aliasing is applied.
    format!("{os}-{arch}")
}

impl UpdateManifest {
    pub fn parse(bytes: &[u8]) -> Result<Self, UpdateError> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(UpdateError::TooLarge);
        }
        let text = std::str::from_utf8(bytes).map_err(|_| UpdateError::InvalidText)?;
        let manifest: UpdateManifest = serde_json::from_str(text)
            .map_err(|_| UpdateError::Invalid("json structure mismatch"))?;
        Version::parse(&manifest.version)?;
        if manifest.notes.len() > 16 * 1024 {
            return Err(UpdateError::Invalid("notes too long"));
        }
        if manifest.platforms.is_empty() || manifest.platforms.len() > 32 {
            return Err(UpdateError::Invalid("platform count out of range"));
        }
        for (key, release) in &manifest.platforms {
            if !key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                || key.is_empty()
                || key.len() > 64
            {
                return Err(UpdateError::Invalid("invalid platform key"));
            }
            let url = reqwest::Url::parse(&release.url)
                .map_err(|_| UpdateError::Invalid("invalid release URL"))?;
            if url.scheme() != "https"
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return Err(UpdateError::Invalid("release URL must be HTTPS"));
            }
            if release.sha256.len() != 64 || !release.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(UpdateError::Invalid("invalid release SHA-256"));
            }
        }
        Ok(manifest)
    }

    pub fn release_for(&self, os: &str, arch: &str) -> Option<&PlatformRelease> {
        self.platforms.get(&platform_key(os, arch))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_json(version: &str) -> String {
        format!(
            r#"{{"version":"{version}","notes":"修复","platforms":{{"linux-x86_64":{{"url":"https://example.com/mfmcl.msi","sha256":"{}"}}}}}}"#,
            "a".repeat(64)
        )
    }

    #[test]
    fn parses_bounded_manifest_and_resolves_platform() {
        let manifest = UpdateManifest::parse(manifest_json("0.2.0").as_bytes()).unwrap();
        assert_eq!(manifest.version, "0.2.0");
        assert_eq!(manifest.notes, "修复");
        let release = manifest.release_for("linux", "x86_64").unwrap();
        assert_eq!(release.url, "https://example.com/mfmcl.msi");
        assert!(manifest.release_for("windows", "aarch64").is_none());
    }

    #[test]
    fn rejects_unsafe_manifests() {
        let oversized = vec![b'x'; MAX_MANIFEST_BYTES + 1];
        assert_eq!(
            UpdateManifest::parse(&oversized),
            Err(UpdateError::TooLarge)
        );
        let bad_json = UpdateManifest::parse(b"{");
        assert!(bad_json.is_err());
        let bad_version = UpdateManifest::parse(manifest_json("1.2").as_bytes());
        assert!(bad_version.is_err());
        let plain_http = r#"{"version":"0.2.0","platforms":{"linux-x86_64":{"url":"http://example.com/a.exe","sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}}"#;
        assert!(UpdateManifest::parse(plain_http.as_bytes()).is_err());
        let bad_hash = r#"{"version":"0.2.0","platforms":{"linux-x86_64":{"url":"https://example.com/a.exe","sha256":"short"}}}"#;
        assert!(UpdateManifest::parse(bad_hash.as_bytes()).is_err());
        assert!(UpdateManifest::parse(manifest_json("0.2.0").as_bytes()).is_ok());
    }

    #[test]
    fn version_comparison_orders_releases_and_preleases() {
        let older = Version::parse("0.1.9").unwrap();
        let newer = Version::parse("0.2.0").unwrap();
        let newer_still = Version::parse("0.10.0").unwrap();
        assert!(newer.is_newer_than(&older));
        assert!(!older.is_newer_than(&newer));
        assert!(newer_still.is_newer_than(&newer));
        let same = Version::parse("0.2.0").unwrap();
        assert!(!same.is_newer_than(&newer));

        let pre = Version::parse("0.2.0-pre.3").unwrap();
        assert!(!pre.is_newer_than(&newer));
        assert!(newer.is_newer_than(&pre));
        // Same-core pre-releases never update one another: mixed tag spellings
        // make strict ordering ambiguous, and a manifest carries one canonical version.
        let other_pre = Version::parse("0.2.0-beta.2").unwrap();
        assert!(!pre.is_newer_than(&other_pre));
        assert!(!other_pre.is_newer_than(&pre));
        assert_eq!(
            Version::parse("1.2.3-pre.4").unwrap().as_str(),
            "1.2.3-pre.4"
        );
        assert!(Version::parse("1.2").is_err());
        assert!(Version::parse("1.2.x").is_err());
    }

    #[test]
    fn platform_keys_use_stable_names() {
        assert_eq!(platform_key("linux", "x86_64"), "linux-x86_64");
        assert_eq!(platform_key("macos", "aarch64"), "macos-aarch64");
        assert_eq!(platform_key("windows", "x86_64"), "windows-x86_64");
    }
}
