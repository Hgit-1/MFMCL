use crate::{
    AccountKind, AccountProfile, InstanceRecord, InstanceState, installer::library_allowed,
    installer::maven_path, installer::rule_matches,
};
use mc_metadata::{ArgumentEntry, ArgumentValue, GameVersionMetadata};
use md5::{Digest, Md5};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop};

#[derive(Debug, Error)]
pub enum LaunchError {
    #[error("offline launch preparation requires an offline profile")]
    UnsupportedAccount,
    #[error("the selected instance is not installed")]
    NotInstalled,
    #[error("Minecraft metadata is incomplete: {0}")]
    InvalidMetadata(String),
    #[error("required game file is missing: {0}")]
    MissingFile(PathBuf),
    #[error("Java was not found. Install a compatible Java runtime or configure its path.")]
    JavaNotFound,
    #[error("Minecraft {version} requires Java {required}, but detected Java {detected}.")]
    JavaVersion {
        version: String,
        required: u32,
        detected: u32,
    },
    #[error("failed to inspect Java: {0}")]
    JavaInspection(String),
    #[error("invalid legacy game arguments")]
    InvalidLegacyArguments,
    #[error("invalid Minecraft account identity")]
    InvalidIdentity,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaRuntimeInfo {
    pub path: String,
    pub major_version: u32,
    pub version_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemMemoryInfo {
    pub total_mb: u32,
    /// 当前已被系统与其它进程占用的内存（MB）。
    ///
    /// `None` 表示这台机器或这个平台读不到——界面据此只在能拿到时才画
    /// 「已使用」那一段，而不是画一条假的 0。
    #[serde(default)]
    pub used_mb: Option<u32>,
    /// 当前可用内存（MB）。Linux/macOS 是真实的 available，Windows 退化为 free。
    #[serde(default)]
    pub available_mb: Option<u32>,
    pub recommended_mb: u32,
    pub maximum_game_mb: u32,
}

fn to_mb(bytes: u64) -> u32 {
    (bytes / (1024 * 1024)).min(u32::MAX as u64) as u32
}

pub fn system_memory_info() -> SystemMemoryInfo {
    let mut system = sysinfo::System::new();
    system.refresh_memory();

    let physical_bytes = system.total_memory();
    // sysinfo reports host RAM on some Linux container setups; respect the container's limit too.
    let limit_bytes =
        cgroup_memory_limit_bytes().filter(|limit| *limit > 0 && *limit < u64::MAX / 2);
    let total_bytes = limit_bytes
        .map_or(physical_bytes, |limit| physical_bytes.min(limit))
        .max(1);
    let raw_total_mb = to_mb(total_bytes);

    // sysinfo 的 used/available 读的是宿主机 /proc/meminfo。容器里总量已被 cgroup
    // 压小，但 used 仍是整机数字，直接画会得到「已用 > 总量」。所以：
    //   有 cgroup 限制（容器）→ 用 cgroup 的 memory.current，它才是容器真实用量；
    //   否则 → 用 sysinfo 的 used，语义与界面其它地方一致（不含页缓存）。
    // 注意不能无条件读 memory.current：普通 systemd 机器的根 cgroup 也有这个文件，
    // 它把页缓存算进用量，会显示成「59/61 GB 已用」这种与 free 矛盾的数字。
    let host_used = system.used_memory();
    let host_available = system.available_memory();
    let ratio = match limit_bytes {
        Some(limit) if physical_bytes > 0 => {
            limit.min(physical_bytes) as f64 / physical_bytes as f64
        }
        _ => 1.0,
    };

    let used_bytes = match limit_bytes {
        Some(_) => cgroup_memory_current_bytes().map(|bytes| bytes.min(total_bytes)),
        None => (host_used > 0).then(|| host_used.min(total_bytes)),
    };
    let available_bytes =
        (host_available > 0).then(|| ((host_available as f64 * ratio) as u64).min(total_bytes));

    let (total_mb, maximum_game_mb, recommended_mb) = recommended_memory_for_total(raw_total_mb);
    SystemMemoryInfo {
        total_mb,
        used_mb: used_bytes.map(to_mb),
        available_mb: available_bytes.map(to_mb),
        recommended_mb,
        maximum_game_mb,
    }
}

fn cgroup_memory_limit_bytes() -> Option<u64> {
    for path in [
        "/sys/fs/cgroup/memory.max",
        "/sys/fs/cgroup/memory/memory.limit_in_bytes",
    ] {
        if let Ok(value) = std::fs::read_to_string(path) {
            if let Ok(limit) = value.trim().parse::<u64>() {
                return Some(limit);
            }
        }
    }
    None
}

/// 容器当前已用内存。只在 cgroup v2 / v1 的用量文件可读时返回。
fn cgroup_memory_current_bytes() -> Option<u64> {
    for path in [
        "/sys/fs/cgroup/memory.current",
        "/sys/fs/cgroup/memory/memory.usage_in_bytes",
    ] {
        if let Ok(value) = std::fs::read_to_string(path) {
            if let Ok(used) = value.trim().parse::<u64>() {
                return Some(used);
            }
        }
    }
    None
}

fn recommended_memory_for_total(total_mb: u32) -> (u32, u32, u32) {
    // Keep at least 1.5 GiB for the OS, desktop and launcher; reserve 20% on larger machines.
    let reserve_mb = 1536_u32.max(total_mb / 5);
    let maximum_game_mb = total_mb.saturating_sub(reserve_mb).max(512);
    let recommended_mb = (maximum_game_mb / 2).clamp(512, 4096);
    (total_mb, maximum_game_mb, recommended_mb)
}

#[cfg(test)]
mod memory_tests {
    use super::*;

    #[test]
    fn memory_recommendation_leaves_system_headroom() {
        assert_eq!(recommended_memory_for_total(8192), (8192, 6554, 3277));
        assert_eq!(recommended_memory_for_total(1024), (1024, 512, 512));
    }

    #[test]
    fn reported_usage_never_exceeds_total() {
        // used / available 都是 Option：平台读不到时不panic、不返回 0，
        // 让界面自己决定不画那一段。
        let info = system_memory_info();
        assert!(info.total_mb > 0);
        if let Some(used) = info.used_mb {
            assert!(
                used <= info.total_mb,
                "used {used} > total {}",
                info.total_mb
            );
        }
        if let Some(available) = info.available_mb {
            assert!(
                available <= info.total_mb,
                "available {available} > total {}",
                info.total_mb
            );
        }
    }
}

// Launch commands include access tokens for online sessions; never derive Debug for them.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct PreparedLaunch {
    #[zeroize(skip)]
    pub java_path: PathBuf,
    pub args: Vec<String>,
    #[zeroize(skip)]
    pub working_directory: PathBuf,
    #[zeroize(skip)]
    pub log_path: PathBuf,
}

/// Backend-only identity, supplied only after provider authentication and ownership checks.
/// Never serialize or log this value.
pub struct LaunchIdentity<'a> {
    pub kind: AccountKind,
    pub profile_id: &'a str,
    pub player_name: &'a str,
    pub access_token: &'a str,
    pub client_id: &'a str,
    pub xuid: &'a str,
}

pub fn prepare_offline_launch(
    data_root: &Path,
    instance: &InstanceRecord,
    account: &AccountProfile,
    metadata: &GameVersionMetadata,
    configured_java: Option<PathBuf>,
) -> Result<PreparedLaunch, LaunchError> {
    if account.kind != AccountKind::Offline {
        return Err(LaunchError::UnsupportedAccount);
    }
    let id = offline_uuid(&account.display_name).simple().to_string();
    let identity = LaunchIdentity {
        kind: AccountKind::Offline,
        profile_id: &id,
        player_name: &account.display_name,
        access_token: "0",
        client_id: "",
        xuid: "",
    };
    prepare_launch(data_root, instance, &identity, metadata, configured_java)
}

pub fn prepare_authenticated_launch(
    data_root: &Path,
    instance: &InstanceRecord,
    identity: &LaunchIdentity<'_>,
    metadata: &GameVersionMetadata,
    configured_java: Option<PathBuf>,
) -> Result<PreparedLaunch, LaunchError> {
    if identity.kind == AccountKind::Offline {
        return Err(LaunchError::UnsupportedAccount);
    }
    prepare_launch(data_root, instance, identity, metadata, configured_java)
}

fn prepare_launch(
    data_root: &Path,
    instance: &InstanceRecord,
    identity: &LaunchIdentity<'_>,
    metadata: &GameVersionMetadata,
    configured_java: Option<PathBuf>,
) -> Result<PreparedLaunch, LaunchError> {
    if instance.state != InstanceState::Ready {
        return Err(LaunchError::NotInstalled);
    }
    validate_identity(identity)?;
    let runtime = detect_java_runtime(configured_java)?;
    prepare_with_runtime(data_root, instance, identity, metadata, runtime)
}

fn validate_identity(identity: &LaunchIdentity<'_>) -> Result<(), LaunchError> {
    if identity.profile_id.len() != 32
        || !identity.profile_id.bytes().all(|b| b.is_ascii_hexdigit())
        || Uuid::parse_str(identity.profile_id).map_or(true, |id| id.is_nil())
        || identity.player_name.is_empty()
        || identity.player_name.len() > 16
        || !identity
            .player_name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        || identity.access_token.is_empty()
        || identity.access_token.len() > 65536
        || identity.access_token.chars().any(char::is_control)
    {
        return Err(LaunchError::InvalidIdentity);
    }
    Ok(())
}

fn prepare_with_runtime(
    data_root: &Path,
    instance: &InstanceRecord,
    identity: &LaunchIdentity<'_>,
    metadata: &GameVersionMetadata,
    runtime: JavaRuntimeInfo,
) -> Result<PreparedLaunch, LaunchError> {
    let java_path = PathBuf::from(runtime.path);
    let detected_java = runtime.major_version;
    if let Some(required) = metadata
        .java_version
        .as_ref()
        .map(|value| value.major_version)
    {
        if detected_java < required {
            return Err(LaunchError::JavaVersion {
                version: metadata.id.clone(),
                required,
                detected: detected_java,
            });
        }
    }

    crate::loaders::safe_version(&metadata.id).map_err(LaunchError::InvalidMetadata)?;
    if metadata.id != instance.game_version {
        return Err(LaunchError::InvalidMetadata(
            "profile version does not match instance".into(),
        ));
    }
    let version_dir = instance.instance_dir.join("versions").join(&metadata.id);
    let client_jar = version_dir.join(format!("{}.jar", metadata.id));
    require_file(&client_jar)?;
    let libraries_dir = data_root.join("libraries");
    let natives_dir = version_dir.join("natives");
    let assets_dir = data_root.join("assets");
    let mut classpath = Vec::new();
    for library in &metadata.libraries {
        if !library_allowed(library) {
            continue;
        }
        let Some(artifact) = &library.downloads.artifact else {
            continue;
        };
        let relative_path = artifact
            .path
            .as_deref()
            .map(PathBuf::from)
            .or_else(|| maven_path(&library.name))
            .ok_or_else(|| {
                LaunchError::InvalidMetadata(format!("invalid library path: {}", library.name))
            })?;
        let path = libraries_dir.join(
            crate::installer::safe_library_path(relative_path)
                .map_err(|_| LaunchError::InvalidMetadata("unsafe library path".into()))?,
        );
        require_file(&path)?;
        classpath.push(path);
    }
    classpath.push(client_jar);
    let classpath_separator = if cfg!(windows) { ";" } else { ":" };
    let classpath = classpath
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(classpath_separator);

    let player_name = identity.player_name.to_owned();
    let player_uuid = identity.profile_id.to_ascii_lowercase();
    let asset_index = metadata
        .asset_index
        .as_ref()
        .map(|index| index.id.clone())
        .unwrap_or_else(|| {
            metadata
                .assets
                .clone()
                .unwrap_or_else(|| "legacy".to_owned())
        });
    let mut variables = HashMap::new();
    variables.insert(
        "${natives_directory}".to_owned(),
        natives_dir.to_string_lossy().into_owned(),
    );
    variables.insert("${launcher_name}".to_owned(), "MFMCL".to_owned());
    variables.insert(
        "${launcher_version}".to_owned(),
        env!("CARGO_PKG_VERSION").to_owned(),
    );
    variables.insert("${classpath}".to_owned(), classpath.clone());
    variables.insert(
        "${classpath_separator}".to_owned(),
        classpath_separator.to_owned(),
    );
    variables.insert(
        "${library_directory}".to_owned(),
        libraries_dir.to_string_lossy().into_owned(),
    );
    variables.insert("${version_name}".to_owned(), metadata.id.clone());
    variables.insert(
        "${version_type}".to_owned(),
        metadata
            .extra
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("release")
            .to_owned(),
    );
    variables.insert(
        "${game_directory}".to_owned(),
        instance.game_dir.to_string_lossy().into_owned(),
    );
    variables.insert(
        "${assets_root}".to_owned(),
        assets_dir.to_string_lossy().into_owned(),
    );
    if let Some(path) = metadata
        .logging
        .as_ref()
        .and_then(|logging| logging.get("client"))
        .and_then(|client| client.get("file"))
        .and_then(|file| file.get("id"))
        .and_then(serde_json::Value::as_str)
        .map(|id| assets_dir.join("log_configs").join(id))
    {
        variables.insert("${path}".to_owned(), path.to_string_lossy().into_owned());
    }
    variables.insert("${assets_index_name}".to_owned(), asset_index.clone());
    variables.insert(
        "${game_assets}".to_owned(),
        assets_dir
            .join("virtual")
            .join(asset_index)
            .to_string_lossy()
            .into_owned(),
    );
    variables.insert("${auth_player_name}".to_owned(), player_name);
    variables.insert("${auth_uuid}".to_owned(), player_uuid);
    variables.insert(
        "${auth_access_token}".to_owned(),
        identity.access_token.to_owned(),
    );
    variables.insert(
        "${auth_session}".to_owned(),
        identity.access_token.to_owned(),
    );
    variables.insert("${clientid}".to_owned(), identity.client_id.to_owned());
    variables.insert("${auth_xuid}".to_owned(), identity.xuid.to_owned());
    let user_type = match &identity.kind {
        AccountKind::Microsoft => "msa",
        AccountKind::SkinServer => "mojang",
        AccountKind::Offline => "legacy",
    };
    variables.insert("${user_type}".to_owned(), user_type.to_owned());
    variables.insert("${user_properties}".to_owned(), "{}".to_owned());
    variables.insert("${resolution_width}".to_owned(), "1280".to_owned());
    variables.insert("${resolution_height}".to_owned(), "720".to_owned());
    variables.insert("${quickPlayPath}".to_owned(), String::new());
    variables.insert("${quickPlaySingleplayer}".to_owned(), String::new());
    variables.insert("${quickPlayMultiplayer}".to_owned(), String::new());
    variables.insert("${quickPlayRealms}".to_owned(), String::new());

    let mut args = Vec::new();
    let mut classpath_supplied = false;
    if let Some(launch_arguments) = &metadata.arguments {
        for entry in &launch_arguments.jvm {
            for argument in expand_argument(entry, &variables) {
                if argument == classpath || argument == format!("{}", variables["${classpath}"]) {
                    classpath_supplied = true;
                }
                args.push(argument);
            }
        }
    } else {
        args.push(format!("-Djava.library.path={}", natives_dir.display()));
        args.push("-cp".to_owned());
        args.push(classpath.clone());
        classpath_supplied = true;
    }
    if !classpath_supplied {
        args.push("-cp".to_owned());
        args.push(classpath);
    }
    if let Some(argument) = metadata
        .logging
        .as_ref()
        .and_then(|v| v.get("client"))
        .and_then(|v| v.get("argument"))
        .and_then(serde_json::Value::as_str)
    {
        if let Some(path) = variables.get("${path}") {
            require_file(Path::new(path))?;
        }
        args.push(replace_variables(argument, &variables));
    }
    let main_class = metadata
        .main_class
        .as_deref()
        .ok_or_else(|| LaunchError::InvalidMetadata("mainClass is missing".to_owned()))?;
    args.push(main_class.to_owned());

    if let Some(launch_arguments) = &metadata.arguments {
        for entry in &launch_arguments.game {
            args.extend(expand_argument(entry, &variables));
        }
    } else if let Some(legacy) = &metadata.minecraft_arguments {
        let parsed = shlex::split(legacy).ok_or(LaunchError::InvalidLegacyArguments)?;
        args.extend(
            parsed
                .into_iter()
                .map(|argument| replace_variables(&argument, &variables)),
        );
    }

    Ok(PreparedLaunch {
        java_path,
        args,
        working_directory: instance.game_dir.clone(),
        log_path: instance
            .instance_dir
            .join("logs")
            .join("launcher-latest.log"),
    })
}

fn expand_argument(entry: &ArgumentEntry, variables: &HashMap<String, String>) -> Vec<String> {
    match entry {
        ArgumentEntry::Literal(value) => vec![replace_variables(value, variables)],
        ArgumentEntry::Conditional(conditional) => {
            if !conditional.rules.is_empty() && !rules_allow(&conditional.rules) {
                return Vec::new();
            }
            match &conditional.value {
                ArgumentValue::Single(value) => vec![replace_variables(value, variables)],
                ArgumentValue::Multiple(values) => values
                    .iter()
                    .map(|value| replace_variables(value, variables))
                    .collect(),
            }
        }
    }
}

fn replace_variables(value: &str, variables: &HashMap<String, String>) -> String {
    // Expand only placeholders in the metadata, never text inside an access token or path.
    let mut result = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        result.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('}') else {
            result.push_str(&rest[start..]);
            return result;
        };
        let end = start + end + 1;
        let placeholder = &rest[start..end];
        result.push_str(
            variables
                .get(placeholder)
                .map(String::as_str)
                .unwrap_or(placeholder),
        );
        rest = &rest[end..];
    }
    result.push_str(rest);
    result
}

fn rules_allow(rules: &[mc_metadata::LibraryRule]) -> bool {
    let features = [
        ("is_demo_user", false),
        ("has_custom_resolution", false),
        ("has_quick_plays_support", false),
        ("is_quick_play_singleplayer", false),
        ("is_quick_play_multiplayer", false),
        ("is_quick_play_realms", false),
    ];
    let mut allowed = false;
    for rule in rules {
        if rule_matches(rule, &features) {
            allowed = rule.action == "allow";
        }
    }
    allowed
}

pub fn inspect_java_path(path: PathBuf) -> Result<JavaRuntimeInfo, LaunchError> {
    inspect_java(&path)
}

pub fn discover_java_runtimes() -> Vec<JavaRuntimeInfo> {
    let mut seen = HashSet::new();
    java_candidates()
        .into_iter()
        .take(128)
        .filter_map(|candidate| {
            let identity = candidate
                .canonicalize()
                .unwrap_or_else(|_| candidate.clone());
            let key = if cfg!(windows) {
                identity.to_string_lossy().to_lowercase()
            } else {
                identity.to_string_lossy().into_owned()
            };
            if !seen.insert(key) {
                return None;
            }
            inspect_java(&candidate).ok()
        })
        .collect()
}

pub fn detect_java_runtime(
    configured_java: Option<PathBuf>,
) -> Result<JavaRuntimeInfo, LaunchError> {
    if let Some(path) = configured_java {
        return inspect_java(&path);
    }
    let candidates = java_candidates();
    let mut last_error = None;
    for candidate in candidates {
        match inspect_java(&candidate) {
            Ok(info) => return Ok(info),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or(LaunchError::JavaNotFound))
}

fn java_candidates() -> Vec<PathBuf> {
    let executable = if cfg!(windows) { "java.exe" } else { "java" };
    let mut candidates = Vec::new();
    if let Some(java_home) = std::env::var_os("JAVA_HOME") {
        candidates.push(PathBuf::from(java_home).join("bin").join(executable));
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|directory| directory.join(executable)));
    }

    let mut roots = Vec::new();
    if cfg!(target_os = "linux") {
        roots.extend([
            PathBuf::from("/usr/lib/jvm"),
            PathBuf::from("/usr/java"),
            PathBuf::from("/opt/java"),
        ]);
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            roots.extend([
                home.join(".jdks"),
                home.join(".sdkman/candidates/java"),
                home.join(".local/share/mise/installs/java"),
            ]);
        }
    } else if cfg!(target_os = "macos") {
        roots.extend([
            PathBuf::from("/Library/Java/JavaVirtualMachines"),
            PathBuf::from("/opt/homebrew/opt"),
            PathBuf::from("/usr/local/opt"),
        ]);
        if let Some(home) = std::env::var_os("HOME") {
            roots.push(PathBuf::from(home).join("Library/Java/JavaVirtualMachines"));
        }
    } else if cfg!(windows) {
        for variable in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            if let Some(root) = std::env::var_os(variable) {
                let root = PathBuf::from(root);
                roots.extend([
                    root.join("Java"),
                    root.join("Eclipse Adoptium"),
                    root.join("Microsoft"),
                    root.join("Programs/Eclipse Adoptium"),
                ]);
            }
        }
    }
    for root in roots {
        add_java_root_candidates(&mut candidates, &root, executable);
    }

    let mut seen = HashSet::new();
    candidates.retain(|candidate| {
        let identity = candidate
            .canonicalize()
            .unwrap_or_else(|_| candidate.clone());
        let key = if cfg!(windows) {
            identity.to_string_lossy().to_lowercase()
        } else {
            identity.to_string_lossy().into_owned()
        };
        seen.insert(key)
    });
    candidates
}

fn add_java_root_candidates(candidates: &mut Vec<PathBuf>, root: &Path, executable: &str) {
    candidates.push(root.join("bin").join(executable));
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten().take(64) {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let directory = entry.path();
        candidates.push(directory.join("bin").join(executable));
        if cfg!(target_os = "macos") {
            candidates.push(directory.join("Contents/Home/bin").join(executable));
        }
    }
}

fn inspect_java(java_path: &Path) -> Result<JavaRuntimeInfo, LaunchError> {
    let output = Command::new(java_path)
        .arg("-version")
        .output()
        .map_err(|_| LaunchError::JavaNotFound)?;
    let text = String::from_utf8_lossy(&output.stderr).to_string()
        + &String::from_utf8_lossy(&output.stdout);
    let major_version = parse_java_major(&text)?;
    if !output.status.success() {
        return Err(LaunchError::JavaInspection(text.trim().to_owned()));
    }
    Ok(JavaRuntimeInfo {
        path: java_path.to_string_lossy().into_owned(),
        major_version,
        version_text: text.lines().next().unwrap_or_default().trim().to_owned(),
    })
}

fn parse_java_major(text: &str) -> Result<u32, LaunchError> {
    let regex = Regex::new(r#"(?:version\s+"?|openjdk\s+)(\d+)"#)
        .map_err(|error| LaunchError::JavaInspection(error.to_string()))?;
    let mut major = regex
        .captures(text)
        .and_then(|capture| capture.get(1))
        .and_then(|capture| capture.as_str().parse::<u32>().ok())
        .ok_or_else(|| {
            LaunchError::JavaInspection("could not parse java -version output".to_owned())
        })?;
    if major == 1 {
        let legacy = Regex::new(r#"version\s+\"1\.(\d+)"#)
            .map_err(|error| LaunchError::JavaInspection(error.to_string()))?;
        major = legacy
            .captures(text)
            .and_then(|capture| capture.get(1))
            .and_then(|capture| capture.as_str().parse::<u32>().ok())
            .unwrap_or(major);
    }
    Ok(major)
}

fn offline_uuid(player_name: &str) -> Uuid {
    let mut digest = Md5::digest(format!("OfflinePlayer:{player_name}").as_bytes()).to_vec();
    digest[6] = (digest[6] & 0x0f) | 0x30;
    digest[8] = (digest[8] & 0x3f) | 0x80;
    Uuid::from_bytes(digest.try_into().expect("MD5 produces 16 bytes"))
}

fn require_file(path: &Path) -> Result<(), LaunchError> {
    if path.is_file() {
        Ok(())
    } else {
        Err(LaunchError::MissingFile(path.to_path_buf()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_uuid_is_stable_and_uses_minecraft_name_namespace() {
        assert_eq!(
            offline_uuid("Notch").to_string(),
            "b50ad385-829d-3141-a216-7e7d7539ba7f"
        );
        assert_eq!(offline_uuid("Notch"), offline_uuid("Notch"));
        assert_ne!(offline_uuid("Notch"), offline_uuid("Alex"));
    }

    #[test]
    fn java_version_parser_handles_modern_and_legacy_formats() {
        assert_eq!(
            parse_java_major("openjdk version \"21.0.2\" 2024-01-16").unwrap(),
            21
        );
        assert_eq!(parse_java_major("java version \"1.8.0_402\"").unwrap(), 8);
        assert_eq!(parse_java_major("openjdk 17.0.10 2024-01-16").unwrap(), 17);
    }

    #[test]
    fn variables_are_replaced_inside_arguments() {
        let vars = HashMap::from([("${auth_player_name}".to_owned(), "Alex".to_owned())]);
        assert_eq!(
            replace_variables("--username=${auth_player_name}", &vars),
            "--username=Alex"
        );
    }

    #[test]
    fn authenticated_arguments_preserve_literal_zero_and_expand_identity_once() {
        let root = std::env::temp_dir().join(format!("mfmcl-launch-{}", Uuid::new_v4()));
        let instance = InstanceRecord {
            id: "test".into(),
            name: "test".into(),
            game_version: "1.21.4".into(),
            loader: "vanilla".into(),
            instance_dir: root.join("instance"),
            game_dir: root.join("instance/game"),
            isolated: true,
            state: InstanceState::Ready,
            created_at: String::new(),
        };
        let dir = instance.instance_dir.join("versions/1.21.4");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("1.21.4.jar"), b"test").unwrap();
        let runtime = JavaRuntimeInfo {
            path: "java".into(),
            major_version: 21,
            version_text: String::new(),
        };
        for kind in [AccountKind::Microsoft, AccountKind::SkinServer] {
            let identity = LaunchIdentity {
                kind: kind.clone(),
                profile_id: "123456781234423481231234567890ab",
                player_name: "Online_Name",
                access_token: "test-secret-${user_type}",
                client_id: "public-id",
                xuid: "12345",
            };
            let metadata: GameVersionMetadata = serde_json::from_value(serde_json::json!({
                "id":"1.21.4", "mainClass":"net.minecraft.Main", "libraries":[],
                "arguments":{"jvm":["-Dliteral=0"],"game":["--accessToken","${auth_access_token}","--username=${auth_player_name}","--uuid","${auth_uuid}","--type","${user_type}","--clientId","${clientid}","--xuid","${auth_xuid}","0"]}
            })).unwrap();
            let prepared =
                prepare_with_runtime(&root, &instance, &identity, &metadata, runtime.clone())
                    .unwrap();
            assert!(prepared.args.contains(&"-Dliteral=0".to_owned()));
            assert!(prepared.args.contains(&identity.access_token.to_owned()));
            assert!(prepared.args.contains(&"--username=Online_Name".to_owned()));
            assert!(prepared.args.contains(&identity.profile_id.to_owned()));
            assert!(
                prepared.args.contains(
                    &if kind == AccountKind::Microsoft {
                        "msa"
                    } else {
                        "mojang"
                    }
                    .to_owned()
                )
            );
            assert_eq!(prepared.args.last().unwrap(), "0");
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
