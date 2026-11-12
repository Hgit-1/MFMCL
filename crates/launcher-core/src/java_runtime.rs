use crate::{InstanceRecord, JavaRuntimeInfo, LaunchError};
use download_engine::{DownloadControl, DownloadError, verify_file};
use futures_util::{StreamExt, TryStreamExt};
use mc_metadata::GameVersionMetadata;
use regex::Regex;
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    time::Duration,
};
use thiserror::Error;
use tokio::{fs, io::AsyncWriteExt};
use uuid::Uuid;

pub const MOJANG_RUNTIME_INDEX: &str = "https://launchermeta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";
const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const MAX_RUNTIME_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum JavaRuntimeError {
    #[error("{0}")]
    Selection(String),
    #[error("Java runtime metadata is invalid: {0}")]
    Manifest(String),
    #[error("no official Java runtime is available for {0}")]
    Unavailable(String),
    #[error("unsafe Java runtime path or URL: {0}")]
    Unsafe(String),
    #[error("Java runtime request failed: {0}")]
    Network(#[from] reqwest::Error),
    #[error("Java runtime file operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Download(#[from] DownloadError),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JavaSelectionMode {
    #[default]
    Auto,
    Range,
    Folder,
    Forced,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct JavaSelectionRequest {
    pub mode: JavaSelectionMode,
    pub range: String,
    pub folder: String,
    pub path: String,
    pub ordered_paths: Option<Vec<String>>,
    /// None inherits the global preference; Some("") explicitly clears it for this request.
    pub global_path: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct JavaRule {
    pub mode: JavaSelectionMode,
    pub range: String,
    pub folder: String,
    pub path: String,
}

impl JavaRule {
    pub fn validate(&self) -> Result<(), String> {
        if self.range.len() > 256 || self.range.chars().any(char::is_control) {
            return Err("invalid Java version range".into());
        }
        for path in [&self.folder, &self.path] {
            if !path.is_empty() {
                validate_java_preference_path(path)?;
            }
        }
        match self.mode {
            JavaSelectionMode::Range => {
                parse_range(&self.range).map_err(|e| e.to_string())?;
            }
            JavaSelectionMode::Folder => validate_java_preference_path(&self.folder)?,
            JavaSelectionMode::Forced => validate_java_preference_path(&self.path)?,
            JavaSelectionMode::Auto => {}
        }
        Ok(())
    }
}

pub fn validate_java_preference_path(path: &str) -> Result<(), String> {
    if path.trim().is_empty()
        || path != path.trim()
        || path.len() > 32768
        || path.chars().any(char::is_control)
        || !Path::new(path).is_absolute()
    {
        return Err(
            "Java paths must be absolute, non-empty paths without control characters".into(),
        );
    }
    Ok(())
}

pub fn validate_java_priority(paths: &[String]) -> Result<(), String> {
    if paths.len() > 256 {
        return Err("too many Java candidates".into());
    }
    let mut seen = HashSet::new();
    for path in paths {
        validate_java_preference_path(path)?;
        let identity = if cfg!(windows) {
            path.replace('/', "\\").to_lowercase()
        } else {
            path.clone()
        };
        if !seen.insert(identity) {
            return Err("duplicate Java candidate path".into());
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct JavaPreferences {
    pub ordered_paths: Vec<String>,
    pub instance_rules: BTreeMap<String, JavaRule>,
    pub legacy_migrated: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct JavaPreferencesUpdate {
    pub ordered_paths: Option<Vec<String>>,
    pub instance_id: Option<String>,
    pub rule: Option<JavaRule>,
}

/// An explicit request replaces the rule, not just its non-empty fields.
/// An omitted request inherits the stored rule; an explicit Auto never does.
pub fn resolve_java_selection(
    preferences: &JavaPreferences,
    instance_id: &str,
    selection: Option<JavaSelectionRequest>,
    legacy_path: Option<String>,
    global_path: Option<&Path>,
) -> JavaSelectionRequest {
    let mut request = if let Some(selection) = selection {
        selection
    } else if let Some(path) = legacy_path {
        JavaSelectionRequest {
            mode: JavaSelectionMode::Forced,
            path,
            ..Default::default()
        }
    } else {
        let rule = preferences
            .instance_rules
            .get(instance_id)
            .cloned()
            .unwrap_or_default();
        JavaSelectionRequest {
            mode: rule.mode,
            range: rule.range,
            folder: rule.folder,
            path: rule.path,
            ..Default::default()
        }
    };
    if request.ordered_paths.is_none() {
        request.ordered_paths = Some(preferences.ordered_paths.clone());
    }
    if request.global_path.is_none() {
        request.global_path = global_path.map(|p| p.to_string_lossy().into_owned());
    }
    request
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaRequirement {
    pub min_major: u32,
    /// Exclusive loader/legacy upper bound, not a guess that newer Java always works.
    pub max_major: Option<u32>,
    pub component: String,
}
impl JavaRequirement {
    pub fn accepts(&self, major: u32) -> bool {
        major >= self.min_major && self.max_major.is_none_or(|max| major < max)
    }
    pub fn validate(&self, runtime: &JavaRuntimeInfo) -> Result<(), JavaRuntimeError> {
        if self.accepts(runtime.major_version) {
            return Ok(());
        }
        Err(JavaRuntimeError::Selection(format!(
            "Java {} is incompatible; required Java {}{}",
            runtime.major_version,
            self.min_major,
            self.max_major
                .map_or_else(|| "+".into(), |max| format!("..{}", max - 1))
        )))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaSelectionResult {
    pub requirement: JavaRequirement,
    pub runtime: Option<JavaRuntimeInfo>,
    pub can_auto_install: bool,
}

fn numeric_version(text: &str) -> Vec<u32> {
    text.split(['.', '-', '+', '_'])
        .filter_map(|p| p.parse().ok())
        .collect()
}
fn between(version: &str, lower: &str, upper: &str) -> bool {
    let value = numeric_version(version);
    value >= numeric_version(lower) && value <= numeric_version(upper)
}

pub fn java_requirement(
    instance: &InstanceRecord,
    metadata: &GameVersionMetadata,
) -> JavaRequirement {
    let release = Regex::new(r"^1\.(\d+)(?:\.(\d+))?(?:$|[- ])").unwrap();
    let parsed = release.captures(&instance.game_version).map(|v| {
        (
            v[1].parse::<u32>().unwrap_or(0),
            v.get(2)
                .and_then(|p| p.as_str().parse::<u32>().ok())
                .unwrap_or(0),
        )
    });
    let fallback = match parsed {
        Some((minor, patch)) if minor > 20 || (minor == 20 && patch >= 5) => 21,
        Some((minor, _)) if minor >= 18 => 17,
        Some((17, _)) => 16,
        Some(_) => 8,
        None => 17,
    };
    let mut min = metadata
        .java_version
        .as_ref()
        .map_or(fallback, |v| v.major_version);
    let mut max = parsed.filter(|(minor, _)| *minor <= 5).map(|_| 9);
    let (loader, version) = instance
        .loader
        .split_once('@')
        .unwrap_or((&instance.loader, ""));
    let mut cap = |bound: u32| max = Some(max.map_or(bound, |old| old.min(bound)));
    match loader {
        "fabric" | "quilt" => {
            if parsed.is_some_and(|(minor, _)| minor >= 18) {
                min = min.max(17);
            }
            if !version.is_empty() && numeric_version(version) < numeric_version("0.17.0") {
                cap(25);
            }
        }
        "cleanroom" => {
            min = min.max(if numeric_version(version) >= numeric_version("0.7.0") {
                25
            } else {
                21
            });
        }
        "liteloader" => cap(9),
        "legacyfabric" => {
            if !version.is_empty() && numeric_version(version) < numeric_version("0.17.0") {
                cap(25);
            }
        }
        "neoforge" => {
            if parsed == Some((20, 1))
                || (!version.is_empty()
                    && numeric_version(version) < numeric_version("20.2.62-beta"))
            {
                cap(22);
            }
        }
        "forge" => {
            if let Some((minor, _)) = parsed {
                match minor {
                    6..=7 => {
                        if metadata.java_version.is_none() {
                            min = 7;
                        }
                        cap(8);
                    }
                    0..=12 => cap(9),
                    13..=14 => cap(11),
                    15 => cap(16),
                    _ if !version.is_empty() => {
                        if between(version, "34.0.0", "36.2.25") {
                            cap(9);
                        } else if numeric_version(version) >= numeric_version("36.2.26")
                            && numeric_version(version) < numeric_version("37.0.0")
                        {
                            cap(24);
                        } else if between(version, "37.0.0", "37.0.79") {
                            cap(17);
                        } else if between(version, "45.0.21", "45.0.65") {
                            cap(20);
                        } else if between(version, "45.0.66", "47.4.8") {
                            cap(22);
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    let component = metadata
        .java_version
        .as_ref()
        .filter(|v| v.major_version >= min && max.is_none_or(|limit| v.major_version < limit))
        .map(|v| v.component.clone())
        .unwrap_or_else(|| {
            match min {
                0..=8 => "jre-legacy",
                9..=16 => "java-runtime-alpha",
                17..=20 => "java-runtime-gamma",
                21..=24 => "java-runtime-delta",
                _ => "java-runtime-epsilon",
            }
            .into()
        });
    JavaRequirement {
        min_major: min,
        max_major: max,
        component,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct JavaVersion(u32, u32, u32);
fn parse_version(text: &str) -> Option<JavaVersion> {
    let parts: Vec<_> = text
        .trim()
        .split('.')
        .map(str::parse::<u32>)
        .collect::<Result<_, _>>()
        .ok()?;
    if parts.is_empty() || parts.len() > 3 || parts[0] == 0 {
        return None;
    }
    Some(JavaVersion(
        parts[0],
        *parts.get(1).unwrap_or(&0),
        *parts.get(2).unwrap_or(&0),
    ))
}
fn runtime_version(runtime: &JavaRuntimeInfo) -> JavaVersion {
    let legacy = Regex::new(r"1\.(\d+)\.\d+_(\d+)").unwrap();
    if let Some(c) = legacy.captures(&runtime.version_text) {
        return JavaVersion(
            c[1].parse().unwrap_or(runtime.major_version),
            0,
            c[2].parse().unwrap_or(0),
        );
    }
    let modern = Regex::new(r#"(?:version\s+"?|openjdk\s+)(\d+(?:\.\d+){0,2})"#).unwrap();
    modern
        .captures(&runtime.version_text)
        .and_then(|c| parse_version(&c[1]))
        .unwrap_or(JavaVersion(runtime.major_version, 0, 0))
}
struct JavaRange {
    min: Option<JavaVersion>,
    max: Option<JavaVersion>,
    min_inclusive: bool,
    max_inclusive: bool,
}
impl JavaRange {
    fn contains(&self, value: JavaVersion) -> bool {
        self.min
            .is_none_or(|min| value > min || (self.min_inclusive && value == min))
            && self
                .max
                .is_none_or(|max| value < max || (self.max_inclusive && value == max))
    }
}
fn parse_range(text: &str) -> Result<JavaRange, JavaRuntimeError> {
    let invalid = || JavaRuntimeError::Selection("invalid Java version range".into());
    let text = text
        .trim()
        .replace('≥', ">=")
        .replace('≤', "<=")
        .replace(['～', '〜'], "~")
        .replace(['—', '–'], "-");
    let version = |s: &str| parse_version(s).ok_or_else(invalid);
    let upper = |s: &str| -> Result<(JavaVersion, bool), JavaRuntimeError> {
        let value = version(s)?;
        if s.contains('.') {
            Ok((value, true))
        } else {
            Ok((
                JavaVersion(value.0.checked_add(1).ok_or_else(invalid)?, 0, 0),
                false,
            ))
        }
    };
    let mut range = JavaRange {
        min: None,
        max: None,
        min_inclusive: false,
        max_inclusive: false,
    };
    if text.starts_with(['[', '(']) && text.ends_with([']', ')']) {
        let (left, right) = text[1..text.len() - 1]
            .split_once(',')
            .ok_or_else(invalid)?;
        range.min = if left.trim().is_empty() {
            None
        } else {
            Some(version(left)?)
        };
        range.max = if right.trim().is_empty() {
            None
        } else {
            Some(version(right)?)
        };
        range.min_inclusive = text.starts_with('[');
        range.max_inclusive = text.ends_with(']');
    } else if let Some(c) = Regex::new(r"^(>=|<=|>|<)\s*(\d+(?:\.\d+){0,2})$")
        .unwrap()
        .captures(&text)
    {
        let bound = version(&c[2])?;
        match &c[1] {
            ">=" | ">" => {
                range.min = Some(bound);
                range.min_inclusive = &c[1] == ">=";
            }
            "<=" => {
                let (value, inclusive) = upper(&c[2])?;
                range.max = Some(value);
                range.max_inclusive = inclusive;
            }
            _ => {
                range.max = Some(bound);
                range.max_inclusive = false;
            }
        }
    } else if let Some(c) = Regex::new(r"^(\d+(?:\.\d+){0,2})\s*([+-])$")
        .unwrap()
        .captures(&text)
    {
        if &c[2] == "+" {
            range.min = Some(version(&c[1])?);
            range.min_inclusive = true;
        } else {
            let (value, inclusive) = upper(&c[1])?;
            range.max = Some(value);
            range.max_inclusive = inclusive;
        }
    } else if let Some(c) =
        Regex::new(r"^(\d+(?:\.\d+){0,2})\s*(?:-|~|到|至|to)\s*(\d+(?:\.\d+){0,2})$")
            .unwrap()
            .captures(&text)
    {
        let lower = version(&c[1])?;
        if lower > version(&c[2])? {
            return Err(invalid());
        }
        let (value, inclusive) = upper(&c[2])?;
        range.min = Some(lower);
        range.max = Some(value);
        range.min_inclusive = true;
        range.max_inclusive = inclusive;
    } else {
        let value = version(&text)?;
        range.min = Some(JavaVersion(value.0, 0, 0));
        range.min_inclusive = true;
        range.max = Some(JavaVersion(
            value.0.checked_add(1).ok_or_else(invalid)?,
            0,
            0,
        ));
    }
    if let (Some(min), Some(max)) = (range.min, range.max) {
        if min > max || (min == max && !(range.min_inclusive && range.max_inclusive)) {
            return Err(invalid());
        }
    }
    Ok(range)
}

pub fn java_executable(path: &Path) -> PathBuf {
    if !path.is_dir() {
        return path.to_owned();
    }
    let executable = if cfg!(windows) { "java.exe" } else { "java" };
    [
        path.join("bin").join(executable),
        path.join(executable),
        path.join("Contents/Home/bin").join(executable),
    ]
    .into_iter()
    .find(|p| p.is_file())
    .unwrap_or_else(|| path.join("bin").join(executable))
}

pub(crate) fn folder_java_candidates(folder: &Path) -> Result<Vec<PathBuf>, JavaRuntimeError> {
    let root = folder.canonicalize()?;
    let mut pending = vec![(root.clone(), 0usize)];
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    let executable = if cfg!(windows) { "java.exe" } else { "java" };
    while let Some((directory, depth)) = pending.pop() {
        let identity = directory.canonicalize()?;
        if !identity.starts_with(&root) || !seen.insert(identity.clone()) {
            continue;
        }
        if seen.len() > 4096 {
            return Err(JavaRuntimeError::Selection(
                "Java folder search is too large".into(),
            ));
        }
        let candidate = identity.join(executable);
        if candidate.is_file()
            && candidate
                .canonicalize()
                .is_ok_and(|path| path.starts_with(&root))
        {
            candidates.push(candidate);
        }
        if depth == 5 {
            continue;
        }
        let mut children: Vec<_> = std::fs::read_dir(identity)?
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .map(|entry| entry.path())
            .collect();
        children.sort();
        pending.extend(children.into_iter().rev().map(|path| (path, depth + 1)));
    }
    Ok(candidates)
}

pub fn managed_java_runtimes(data_root: &Path) -> Vec<JavaRuntimeInfo> {
    let Ok(entries) = std::fs::read_dir(data_root.join("java-runtimes")) else {
        return vec![];
    };
    let mut paths: Vec<_> = entries
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("mojang-"))
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect();
    paths.sort();
    paths
        .into_iter()
        .filter_map(|path| {
            let relative = std::fs::read_to_string(path.join(".mfmcl-complete")).ok()?;
            let relative = safe_relative(&relative).ok()?;
            crate::inspect_java_path(path.join(relative)).ok()
        })
        .collect()
}

fn preferred_java_paths(request: &JavaSelectionRequest) -> Vec<&str> {
    request
        .global_path
        .iter()
        .map(String::as_str)
        .chain(request.ordered_paths.iter().flatten().map(String::as_str))
        .filter(|path| !path.trim().is_empty())
        .collect()
}

pub fn select_java(
    data_root: &Path,
    instance: &InstanceRecord,
    metadata: &GameVersionMetadata,
    request: &JavaSelectionRequest,
) -> Result<JavaSelectionResult, JavaRuntimeError> {
    let requirement = java_requirement(instance, metadata);
    if request.mode == JavaSelectionMode::Forced {
        if request.path.trim().is_empty() {
            return Err(JavaRuntimeError::Selection("no Java path selected".into()));
        }
        let runtime = crate::inspect_java_path(PathBuf::from(request.path.trim()))
            .map_err(selection_error)?;
        requirement.validate(&runtime)?;
        return Ok(JavaSelectionResult {
            requirement,
            runtime: Some(runtime),
            can_auto_install: false,
        });
    }
    let range = if request.mode == JavaSelectionMode::Range {
        Some(parse_range(&request.range)?)
    } else {
        None
    };
    let mut runtimes = Vec::new();
    if request.mode == JavaSelectionMode::Folder {
        let folder = PathBuf::from(request.folder.trim());
        if request.folder.trim().is_empty() || !folder.is_dir() {
            return Err(JavaRuntimeError::Selection(
                "Java folder does not exist".into(),
            ));
        }
        for candidate in folder_java_candidates(&folder)? {
            if let Ok(runtime) = crate::inspect_java_path(candidate) {
                runtimes.push(runtime);
            }
        }
    } else {
        for path in preferred_java_paths(request) {
            if let Ok(runtime) = crate::inspect_java_path(path.into()) {
                runtimes.push(runtime);
            }
        }
        runtimes.extend(managed_java_runtimes(data_root));
        runtimes.extend(crate::discover_java_runtimes());
    }
    let runtime = choose_candidate(runtimes, &requirement, range.as_ref());
    if runtime.is_none() && request.mode != JavaSelectionMode::Auto {
        return Err(JavaRuntimeError::Selection(
            "no compatible Java runtime matches the explicit selection".into(),
        ));
    }
    Ok(JavaSelectionResult {
        requirement,
        runtime,
        can_auto_install: request.mode == JavaSelectionMode::Auto,
    })
}
fn selection_error(error: LaunchError) -> JavaRuntimeError {
    JavaRuntimeError::Selection(error.to_string())
}
fn choose_candidate(
    runtimes: Vec<JavaRuntimeInfo>,
    requirement: &JavaRequirement,
    range: Option<&JavaRange>,
) -> Option<JavaRuntimeInfo> {
    runtimes.into_iter().find(|runtime| {
        requirement.accepts(runtime.major_version)
            && range.is_none_or(|r| r.contains(runtime_version(runtime)))
    })
}

pub fn mojang_platform(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("windows", "x86_64") => Some("windows-x64"),
        ("windows", "x86") => Some("windows-x86"),
        ("windows", "aarch64") => Some("windows-arm64"),
        ("linux", "x86_64") => Some("linux"),
        ("linux", "x86") => Some("linux-i386"),
        ("linux", "aarch64") => Some("linux-arm64"),
        ("macos", "x86_64") => Some("mac-os"),
        ("macos", "aarch64") => Some("mac-os-arm64"),
        _ => None,
    }
}
fn safe_url(url: &Url) -> bool {
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return false;
    }
    if cfg!(test)
        && url.scheme() == "http"
        && url
            .host_str()
            .is_some_and(|h| h == "127.0.0.1" || h == "localhost" || h == "[::1]")
    {
        return true;
    }
    url.scheme() == "https"
        && url.port().is_none()
        && matches!(
            url.host_str(),
            Some(
                "launchermeta.mojang.com"
                    | "piston-meta.mojang.com"
                    | "piston-data.mojang.com"
                    | "resources.download.minecraft.net"
            )
        )
}
#[cfg(test)]
fn runtime_client() -> Result<Client, JavaRuntimeError> {
    runtime_client_with_settings(&crate::DownloadSettings::default())
}
fn runtime_client_with_settings(
    settings: &crate::DownloadSettings,
) -> Result<Client, JavaRuntimeError> {
    Ok(settings
        .to_engine_settings()
        .configure_client(
            Client::builder().user_agent(concat!("MFMCL/", env!("CARGO_PKG_VERSION"))),
        )?
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if !safe_url(attempt.url()) || attempt.previous().len() >= 5 {
                attempt.error("unsafe Java runtime redirect")
            } else {
                attempt.follow()
            }
        }))
        .build()?)
}
fn safe_relative(name: &str) -> Result<PathBuf, JavaRuntimeError> {
    if name.is_empty()
        || name.contains(['\\', ':', '\0'])
        || name.starts_with('/')
        || name.ends_with('/')
        || name.split('/').any(|part| {
            part.is_empty()
                || part == "."
                || part == ".."
                || part.ends_with([' ', '.'])
                || part
                    .chars()
                    .any(|c| c.is_control() || "<>\"|?*".contains(c))
                || matches!(
                    part.split('.')
                        .next()
                        .unwrap_or("")
                        .to_ascii_uppercase()
                        .as_str(),
                    "CON"
                        | "PRN"
                        | "AUX"
                        | "NUL"
                        | "COM1"
                        | "COM2"
                        | "COM3"
                        | "COM4"
                        | "COM5"
                        | "COM6"
                        | "COM7"
                        | "COM8"
                        | "COM9"
                        | "LPT1"
                        | "LPT2"
                        | "LPT3"
                        | "LPT4"
                        | "LPT5"
                        | "LPT6"
                        | "LPT7"
                        | "LPT8"
                        | "LPT9"
                )
        })
    {
        return Err(JavaRuntimeError::Unsafe(name.into()));
    }
    Ok(PathBuf::from(name))
}

#[derive(Debug, Clone, Deserialize)]
struct RuntimeDownload {
    url: String,
    sha1: String,
    size: u64,
}
impl RuntimeDownload {
    fn validate(&self) -> Result<(), JavaRuntimeError> {
        let url = Url::parse(&self.url).map_err(|_| JavaRuntimeError::Unsafe(self.url.clone()))?;
        if !safe_url(&url) {
            return Err(JavaRuntimeError::Unsafe(self.url.clone()));
        }
        if self.sha1.len() != 40
            || !self.sha1.bytes().all(|c| c.is_ascii_hexdigit())
            || self.size > MAX_RUNTIME_BYTES
        {
            return Err(JavaRuntimeError::Manifest(
                "invalid SHA-1 or byte count".into(),
            ));
        }
        Ok(())
    }
}
#[derive(Deserialize)]
struct RuntimeEntry {
    manifest: RuntimeDownload,
    version: RuntimeEntryVersion,
}
#[derive(Deserialize)]
struct RuntimeEntryVersion {
    name: String,
}
type RuntimeIndex = BTreeMap<String, BTreeMap<String, Vec<RuntimeEntry>>>;
#[derive(Debug, Clone, Deserialize)]
struct RuntimeManifest {
    files: BTreeMap<String, RuntimeFile>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum RuntimeFile {
    File {
        downloads: RuntimeFileDownloads,
        #[serde(default)]
        executable: bool,
    },
    Directory,
    Link {
        target: String,
    },
}
#[derive(Debug, Clone, Deserialize)]
struct RuntimeFileDownloads {
    raw: RuntimeDownload,
}

fn link_destination(name: &str, target: &str) -> Result<String, JavaRuntimeError> {
    if target.is_empty() || target.starts_with('/') || target.contains(['\\', ':', '\0']) {
        return Err(JavaRuntimeError::Unsafe(target.into()));
    }
    let mut parts: Vec<&str> = name.split('/').collect();
    parts.pop();
    for part in target.split('/') {
        match part {
            ".." => {
                if parts.pop().is_none() {
                    return Err(JavaRuntimeError::Unsafe(target.into()));
                }
            }
            "." => {}
            "" => return Err(JavaRuntimeError::Unsafe(target.into())),
            value => parts.push(value),
        }
    }
    let result = parts.join("/");
    safe_relative(&result)?;
    Ok(result)
}
fn validate_manifest(manifest: &RuntimeManifest) -> Result<String, JavaRuntimeError> {
    if manifest.files.is_empty() || manifest.files.len() > 20000 {
        return Err(JavaRuntimeError::Manifest("invalid file count".into()));
    }
    let mut total = 0u64;
    let mut identities = HashSet::new();
    let by_identity: BTreeMap<_, _> = manifest
        .files
        .iter()
        .map(|(name, file)| (name.to_lowercase(), file))
        .collect();
    for (name, file) in &manifest.files {
        safe_relative(name)?;
        if name.starts_with(".mfmcl-") || !identities.insert(name.to_lowercase()) {
            return Err(JavaRuntimeError::Unsafe(name.clone()));
        }
        let mut parent = name.rsplit_once('/').map(|(parent, _)| parent);
        while let Some(path) = parent {
            if let Some(value) = by_identity.get(&path.to_lowercase()) {
                if !matches!(value, RuntimeFile::Directory) {
                    return Err(JavaRuntimeError::Unsafe(name.clone()));
                }
            }
            parent = path.rsplit_once('/').map(|(parent, _)| parent);
        }
        match file {
            RuntimeFile::File { downloads, .. } => {
                downloads.raw.validate()?;
                total = total
                    .checked_add(downloads.raw.size)
                    .ok_or_else(|| JavaRuntimeError::Manifest("runtime too large".into()))?;
            }
            RuntimeFile::Link { target } => {
                let destination = link_destination(name, target)?;
                // Links never become write parents and may only target materialized files/directories.
                if !matches!(
                    manifest.files.get(&destination),
                    Some(RuntimeFile::File { .. } | RuntimeFile::Directory)
                ) {
                    return Err(JavaRuntimeError::Unsafe(target.clone()));
                }
                #[cfg(not(unix))]
                return Err(JavaRuntimeError::Unavailable(
                    "runtime requires symbolic links on this platform".into(),
                ));
            }
            RuntimeFile::Directory => {}
        }
    }
    if total > MAX_RUNTIME_BYTES {
        return Err(JavaRuntimeError::Manifest("runtime too large".into()));
    }
    let executable = if cfg!(windows) {
        "bin/java.exe"
    } else {
        "bin/java"
    };
    [
        executable,
        "jre.bundle/Contents/Home/bin/java",
        "Contents/Home/bin/java",
    ]
    .into_iter()
    .find(|path| matches!(manifest.files.get(*path), Some(RuntimeFile::File { .. })))
    .map(str::to_owned)
    .ok_or_else(|| JavaRuntimeError::Manifest("Java executable missing".into()))
}

async fn cancellable<F: std::future::Future>(
    control: &DownloadControl,
    future: F,
) -> Result<F::Output, JavaRuntimeError> {
    tokio::pin!(future);
    loop {
        control.checkpoint().await?;
        tokio::select! {
            result = &mut future => return Ok(result),
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }
}
async fn fetch_bytes(
    client: &Client,
    url: &str,
    limit: u64,
    control: &DownloadControl,
) -> Result<Vec<u8>, JavaRuntimeError> {
    let parsed = Url::parse(url).map_err(|_| JavaRuntimeError::Unsafe(url.into()))?;
    if !safe_url(&parsed) {
        return Err(JavaRuntimeError::Unsafe(url.into()));
    }
    let response = cancellable(control, client.get(parsed).send())
        .await??
        .error_for_status()?;
    if response.content_length().is_some_and(|n| n > limit) {
        return Err(JavaRuntimeError::Manifest("response too large".into()));
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = cancellable(control, stream.next()).await? {
        let chunk = chunk?;
        if bytes.len() as u64 + chunk.len() as u64 > limit {
            return Err(JavaRuntimeError::Manifest("response too large".into()));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
async fn fetch_pinned(
    client: &Client,
    download: &RuntimeDownload,
    control: &DownloadControl,
) -> Result<Vec<u8>, JavaRuntimeError> {
    download.validate()?;
    if download.size > MAX_MANIFEST_BYTES {
        return Err(JavaRuntimeError::Manifest("manifest too large".into()));
    }
    let bytes = fetch_bytes(client, &download.url, download.size, control).await?;
    if bytes.len() as u64 != download.size
        || !hex::encode(Sha1::digest(&bytes)).eq_ignore_ascii_case(&download.sha1)
    {
        return Err(JavaRuntimeError::Manifest(
            "manifest SHA-1 or byte count mismatch".into(),
        ));
    }
    Ok(bytes)
}
async fn download_raw(
    client: &Client,
    download: &RuntimeDownload,
    destination: &Path,
    control: &DownloadControl,
) -> Result<(), JavaRuntimeError> {
    download.validate()?;
    let response = cancellable(control, client.get(&download.url).send())
        .await??
        .error_for_status()?;
    if response
        .content_length()
        .is_some_and(|n| n != download.size)
    {
        return Err(JavaRuntimeError::Manifest(
            "raw file byte count mismatch".into(),
        ));
    }
    let mut output = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .await?;
    let mut stream = response.bytes_stream();
    let mut received = 0;
    while let Some(chunk) = cancellable(control, stream.next()).await? {
        let chunk = chunk?;
        received += chunk.len() as u64;
        if received > download.size {
            return Err(JavaRuntimeError::Manifest(
                "raw file exceeds expected length".into(),
            ));
        }
        output.write_all(&chunk).await?;
    }
    output.sync_all().await?;
    drop(output);
    verify_file(destination, Some(download.size), Some(&download.sha1), None).await?;
    Ok(())
}

struct StagingDirectory(PathBuf);
impl Drop for StagingDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
async fn install_manifest(
    client: &Client,
    root: &Path,
    manifest: RuntimeManifest,
    label: &str,
    requirement: Option<&JavaRequirement>,
    control: &DownloadControl,
    retries: u8,
) -> Result<PathBuf, JavaRuntimeError> {
    let executable = validate_manifest(&manifest)?;
    control.checkpoint().await?;
    fs::create_dir_all(root).await?;
    if fs::symlink_metadata(root).await?.file_type().is_symlink() {
        return Err(JavaRuntimeError::Unsafe(root.display().to_string()));
    }
    let root = root.canonicalize()?;
    let staging = StagingDirectory(root.join(format!(".staging-{}", Uuid::new_v4())));
    fs::create_dir(&staging.0).await?;
    for (name, file) in &manifest.files {
        let path = staging.0.join(name);
        if matches!(file, RuntimeFile::Directory) {
            fs::create_dir_all(&path).await?;
        } else if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
    }
    let files: Vec<_> = manifest
        .files
        .iter()
        .filter_map(|(name, file)| match file {
            RuntimeFile::File {
                downloads,
                executable,
            } => Some((staging.0.join(name), downloads.raw.clone(), *executable)),
            _ => None,
        })
        .collect();
    futures_util::stream::iter(files)
        .map(|(destination, download, executable)| {
            let client = client.clone();
            let control = control.clone();
            async move {
                for attempt in 0..=retries {
                    control.checkpoint().await?;
                    match download_raw(&client, &download, &destination, &control).await {
                        Ok(()) => break,
                        Err(error) => {
                            let _ = fs::remove_file(&destination).await;
                            if control.is_cancelled() || attempt == retries {
                                return Err(error);
                            }
                        }
                    }
                }
                #[cfg(unix)]
                if executable {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o755))
                        .await?;
                }
                #[cfg(not(unix))]
                let _ = executable;
                Ok::<_, JavaRuntimeError>(())
            }
        })
        .buffer_unordered(8)
        .try_collect::<Vec<_>>()
        .await?;
    for (name, file) in &manifest.files {
        if let RuntimeFile::Link { target } = file {
            control.checkpoint().await?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, staging.0.join(name))?;
            #[cfg(not(unix))]
            let _ = (name, target);
        }
    }
    control.checkpoint().await?;
    if let Some(requirement) = requirement {
        let runtime =
            crate::inspect_java_path(staging.0.join(&executable)).map_err(selection_error)?;
        requirement.validate(&runtime)?;
    }
    control.checkpoint().await?;
    fs::write(staging.0.join(".mfmcl-complete"), &executable).await?;
    control.checkpoint().await?;
    let destination = root.join(format!("mojang-{label}-{}", Uuid::new_v4()));
    fs::rename(&staging.0, &destination).await?;
    Ok(destination.join(executable))
}

fn index_runtime_major(name: &str) -> Option<u32> {
    Regex::new(r"^(?:1\.)?(\d+)")
        .unwrap()
        .captures(name)
        .and_then(|value| value[1].parse().ok())
}

pub async fn install_official_java(
    data_root: &Path,
    requirement: &JavaRequirement,
    control: &DownloadControl,
) -> Result<JavaRuntimeInfo, JavaRuntimeError> {
    install_official_java_with_settings(
        data_root,
        requirement,
        control,
        &crate::DownloadSettings::default(),
    )
    .await
}

pub async fn install_official_java_with_settings(
    data_root: &Path,
    requirement: &JavaRequirement,
    control: &DownloadControl,
    settings: &crate::DownloadSettings,
) -> Result<JavaRuntimeInfo, JavaRuntimeError> {
    let platform =
        mojang_platform(std::env::consts::OS, std::env::consts::ARCH).ok_or_else(|| {
            JavaRuntimeError::Unavailable(format!(
                "{}/{}",
                std::env::consts::OS,
                std::env::consts::ARCH
            ))
        })?;
    let client = runtime_client_with_settings(settings)?;
    let bytes = fetch_bytes(&client, MOJANG_RUNTIME_INDEX, MAX_MANIFEST_BYTES, control).await?;
    let index: RuntimeIndex =
        serde_json::from_slice(&bytes).map_err(|e| JavaRuntimeError::Manifest(e.to_string()))?;
    let components = index
        .get(platform)
        .ok_or_else(|| JavaRuntimeError::Unavailable(platform.into()))?;
    let entries = components.get(&requirement.component).ok_or_else(|| {
        JavaRuntimeError::Unavailable(format!("{platform}/{}", requirement.component))
    })?;
    let entry = entries
        .iter()
        .find(|entry| {
            index_runtime_major(&entry.version.name).is_some_and(|major| requirement.accepts(major))
        })
        .ok_or_else(|| {
            JavaRuntimeError::Unavailable(format!("{platform}/{}", requirement.component))
        })?;
    let bytes = fetch_pinned(&client, &entry.manifest, control).await?;
    let manifest: RuntimeManifest =
        serde_json::from_slice(&bytes).map_err(|e| JavaRuntimeError::Manifest(e.to_string()))?;
    let path = install_manifest(
        &client,
        &data_root.join("java-runtimes"),
        manifest,
        platform,
        Some(requirement),
        control,
        settings.retries,
    )
    .await?;
    let runtime = crate::inspect_java_path(path).map_err(selection_error)?;
    requirement.validate(&runtime)?;
    Ok(runtime)
}

/// Call before either authenticated or offline preparation; explicit selections never download or fall back.
pub async fn ensure_java(
    data_root: &Path,
    instance: &InstanceRecord,
    metadata: &GameVersionMetadata,
    request: &JavaSelectionRequest,
    control: &DownloadControl,
) -> Result<JavaRuntimeInfo, JavaRuntimeError> {
    let selected = select_java(data_root, instance, metadata, request)?;
    if let Some(runtime) = selected.runtime {
        return Ok(runtime);
    }
    if !selected.can_auto_install {
        return Err(JavaRuntimeError::Selection(
            "Java selection has no runtime".into(),
        ));
    }
    install_official_java(data_root, &selected.requirement, control).await
}

#[cfg(test)]
#[path = "java_runtime_tests.rs"]
mod tests;
