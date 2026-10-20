use crate::{
    AccountKind, AccountProfile, InstanceRecord, InstanceState, installer::library_allowed,
    installer::maven_path, installer::rule_matches,
};
use mc_metadata::{ArgumentEntry, ArgumentValue, GameVersionMetadata};
use md5::{Digest, Md5};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum LaunchError {
    #[error("only offline accounts are supported by the current launch backend")]
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaRuntimeInfo {
    pub path: String,
    pub major_version: u32,
    pub version_text: String,
}

#[derive(Debug, Clone)]
pub struct PreparedLaunch {
    pub java_path: PathBuf,
    pub args: Vec<String>,
    pub working_directory: PathBuf,
    pub log_path: PathBuf,
}

pub fn prepare_offline_launch(
    data_root: &Path,
    instance: &InstanceRecord,
    account: &AccountProfile,
    metadata: &GameVersionMetadata,
    configured_java: Option<PathBuf>,
) -> Result<PreparedLaunch, LaunchError> {
    if instance.state != InstanceState::Ready {
        return Err(LaunchError::NotInstalled);
    }
    if account.kind != AccountKind::Offline {
        return Err(LaunchError::UnsupportedAccount);
    }
    let java_path = configured_java.unwrap_or_else(|| PathBuf::from("java"));
    let detected_java = detect_java_major(&java_path)?;
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
        let path = libraries_dir.join(relative_path);
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

    let player_name = account.display_name.clone();
    let player_uuid = offline_uuid(&player_name).to_string().replace('-', "");
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
    variables.insert("${assets_index_name}".to_owned(), asset_index);
    variables.insert("${auth_player_name}".to_owned(), player_name);
    variables.insert("${auth_uuid}".to_owned(), player_uuid);
    variables.insert("${auth_access_token}".to_owned(), "0".to_owned());
    variables.insert("${auth_session}".to_owned(), "0".to_owned());
    variables.insert("${clientid}".to_owned(), String::new());
    variables.insert("${auth_xuid}".to_owned(), String::new());
    variables.insert("${user_type}".to_owned(), "legacy".to_owned());
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
        log_path: data_root
            .join("logs")
            .join(format!("{}-latest.log", instance.id)),
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
    let mut result = value.to_owned();
    for (placeholder, replacement) in variables {
        result = result.replace(placeholder, replacement);
    }
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

pub fn detect_java_runtime(
    configured_java: Option<PathBuf>,
) -> Result<JavaRuntimeInfo, LaunchError> {
    let candidates = configured_java.into_iter().chain(java_candidates());
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
    let mut candidates = Vec::new();
    if let Some(java_home) = std::env::var_os("JAVA_HOME") {
        let executable = if cfg!(windows) { "java.exe" } else { "java" };
        candidates.push(PathBuf::from(java_home).join("bin").join(executable));
    }
    candidates.push(PathBuf::from(if cfg!(windows) {
        "java.exe"
    } else {
        "java"
    }));
    candidates
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

fn detect_java_major(java_path: &Path) -> Result<u32, LaunchError> {
    inspect_java(java_path).map(|runtime| runtime.major_version)
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
}
