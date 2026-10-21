use mc_metadata::SourcePreference;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    Light,
    #[default]
    Dark,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThemePreset {
    #[default]
    Moss,
    Ember,
    Lavender,
    Ocean,
    Custom,
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
pub struct LauncherSettings {
    pub schema_version: u32,
    pub theme_mode: ThemeMode,
    pub theme_preset: ThemePreset,
    pub custom_accent: String,
    pub custom_background: Option<String>,
    pub beginner_mode: bool,
    pub show_game_log: bool,
    #[serde(default)]
    pub java_path: Option<PathBuf>,
    #[serde(default = "default_memory")]
    pub memory_mb: u32,
    pub download: DownloadSettings,
}

fn default_memory() -> u32 {
    2048
}

impl Default for LauncherSettings {
    fn default() -> Self {
        Self {
            schema_version: 1,
            theme_mode: ThemeMode::Dark,
            theme_preset: ThemePreset::Moss,
            custom_accent: "#a6d991".to_owned(),
            custom_background: None,
            beginner_mode: true,
            show_game_log: true,
            java_path: None,
            memory_mb: default_memory(),
            download: DownloadSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    Microsoft,
    Offline,
    SkinServer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountProfile {
    pub id: String,
    pub display_name: String,
    pub kind: AccountKind,
    pub auth_server: Option<String>,
    /// Opaque OS credential-store reference, never exposed over frontend IPC.
    #[serde(skip_serializing)]
    pub credential_ref: Option<String>,
    pub selected: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InstanceState {
    NotInstalled,
    Installing,
    Ready,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceRecord {
    pub id: String,
    pub name: String,
    pub game_version: String,
    pub loader: String,
    pub instance_dir: PathBuf,
    pub game_dir: PathBuf,
    pub isolated: bool,
    pub state: InstanceState,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DownloadTaskState {
    Running,
    Paused,
    Completed,
    Cancelled,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadTask {
    pub id: String,
    pub instance_id: String,
    pub label: String,
    pub source: Option<String>,
    pub state: DownloadTaskState,
    pub completed_bytes: u64,
    pub total_bytes: Option<u64>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateInstanceInput {
    pub name: String,
    pub game_version: String,
}

impl CreateInstanceInput {
    pub fn validate(&self) -> Result<(), String> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > 48 {
            return Err("实例名称不能为空，且最多 48 个字符。".to_owned());
        }
        if self.game_version.trim().is_empty() || self.game_version.len() > 64 {
            return Err("Minecraft 版本标识无效。".to_owned());
        }
        if self.game_version.contains(['/', '\\']) || self.game_version.contains("..") {
            return Err("版本标识包含不允许的路径字符。".to_owned());
        }
        Ok(())
    }
}
