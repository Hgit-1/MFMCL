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
            /*
             * 默认镜像优先、官方回退。
             *
             * 为什么不是 Official：官方 CDN 在部分地区很慢，而镜像只在**校验值可用**
             * 时才会被优先使用（`source_candidates` 里以 `checksum_available` 为前提），
             * 所以走镜像不会牺牲完整性——文件下载后仍按官方清单里的 SHA-1 校验。
             * 版本清单本身始终从官方获取（它是哈希的信任根），因此镜像优先只影响
             * 体积最大的那一批文件，正是用户感知最明显的地方。
             */
            source: SourcePreference::Mixed,
            max_concurrent_downloads: 4,
            connections_per_file: 4,
            aria2_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum UpdatePolicy {
    #[default]
    Notify,
    Off,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemoryMode {
    Auto,
    #[default]
    Custom,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum GarbageCollector {
    #[default]
    Auto,
    G1,
    Serial,
    Parallel,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum LauncherVisibility {
    #[default]
    Keep,
    Hide,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProcessPriority {
    #[default]
    Normal,
    High,
    Low,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TitleBarMode {
    #[default]
    Default,
    Text,
    Image,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ModpackMergeMode {
    #[default]
    Ask,
    Replace,
    Merge,
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
    #[serde(default = "default_window_width")]
    pub window_width: u32,
    #[serde(default = "default_window_height")]
    pub window_height: u32,
    #[serde(default)]
    pub extra_jvm_args: String,
    #[serde(default)]
    pub game_args: String,
    #[serde(default)]
    pub pre_launch_command: String,
    #[serde(default)]
    pub memory_mode: MemoryMode,
    #[serde(default)]
    pub garbage_collector: GarbageCollector,
    #[serde(default)]
    pub optimize_memory: bool,
    #[serde(default)]
    pub process_priority: ProcessPriority,
    #[serde(default)]
    pub fullscreen: bool,
    #[serde(default)]
    pub launcher_visibility: LauncherVisibility,
    #[serde(default = "default_true")]
    pub default_isolated: bool,
    #[serde(default)]
    pub close_after_launch: bool,
    #[serde(default)]
    pub speed_limit_kbps: u32,
    #[serde(default)]
    pub download_directory: Option<PathBuf>,
    #[serde(default = "default_true")]
    pub verify_ssl: bool,
    #[serde(default)]
    pub community_source: String,
    #[serde(default)]
    pub modpack_filename_format: String,
    #[serde(default)]
    pub modpack_merge_mode: ModpackMergeMode,
    #[serde(default = "default_true")]
    pub show_game_hints: bool,
    #[serde(default)]
    pub show_debug_hints: bool,
    #[serde(default = "default_true")]
    pub auto_language: bool,
    #[serde(default)]
    pub announcement_policy: UpdatePolicy,
    #[serde(default)]
    pub cache_directory: Option<PathBuf>,
    #[serde(default)]
    pub anonymous_usage: bool,
    #[serde(default = "default_opacity")]
    pub opacity: u8,
    #[serde(default)]
    pub hidden_theme: bool,
    #[serde(default = "default_true")]
    pub show_launcher_logo: bool,
    #[serde(default)]
    pub background_image: Option<String>,
    #[serde(default)]
    pub background_music: bool,
    #[serde(default)]
    pub title_bar: TitleBarMode,
    #[serde(default)]
    pub home_source: String,
    /// Launcher self-update policy; auto-replacement stays disabled without signed installers.
    #[serde(default)]
    pub update_policy: UpdatePolicy,
    pub download: DownloadSettings,
}

fn default_memory() -> u32 {
    2048
}

fn default_window_width() -> u32 {
    854
}

fn default_window_height() -> u32 {
    480
}

fn default_true() -> bool {
    true
}

fn default_opacity() -> u8 {
    100
}

impl Default for LauncherSettings {
    fn default() -> Self {
        Self {
            schema_version: 3,
            theme_mode: ThemeMode::Dark,
            theme_preset: ThemePreset::Moss,
            custom_accent: "#00d285".to_owned(),
            custom_background: None,
            beginner_mode: true,
            show_game_log: true,
            java_path: None,
            memory_mb: default_memory(),
            window_width: default_window_width(),
            window_height: default_window_height(),
            extra_jvm_args: String::new(),
            game_args: String::new(),
            pre_launch_command: String::new(),
            memory_mode: MemoryMode::Custom,
            garbage_collector: GarbageCollector::Auto,
            optimize_memory: false,
            process_priority: ProcessPriority::Normal,
            fullscreen: false,
            launcher_visibility: LauncherVisibility::Keep,
            default_isolated: true,
            close_after_launch: false,
            speed_limit_kbps: 0,
            download_directory: None,
            verify_ssl: true,
            community_source: "official".to_owned(),
            modpack_filename_format: "{name}-{version}".to_owned(),
            modpack_merge_mode: ModpackMergeMode::Ask,
            show_game_hints: true,
            show_debug_hints: false,
            auto_language: true,
            announcement_policy: UpdatePolicy::Notify,
            cache_directory: None,
            anonymous_usage: false,
            opacity: 100,
            hidden_theme: false,
            show_launcher_logo: true,
            background_image: None,
            background_music: false,
            title_bar: TitleBarMode::Default,
            home_source: "official".to_owned(),
            update_policy: UpdatePolicy::Notify,
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
    /// 上次成功启动的时间（RFC 3339）。`None` 表示从未启动过。
    ///
    /// 用于两处：内容商店的「目标实例」默认为最近玩过的实例；实例列表按此排序，
    /// 让最常玩的排在前面。用 `Option` 而不是给个默认时间戳，是因为「从未启动」
    /// 和「很久以前启动」是两件不同的事。
    #[serde(default)]
    pub last_launched_at: Option<String>,
}

/// Per-instance overrides of the global launcher settings.
///
/// Every field is optional and `None` means "inherit the global value". That is
/// deliberate: a new instance must start out behaving exactly like the global
/// configuration, and only diverge where the user explicitly chose to. Modelling
/// inheritance as presence/absence (instead of a competing default value) is what
/// makes "did I change this, or is it just the global value?" answerable later.
///
/// The set of overridable fields is intentionally small: only values that change
/// how this specific game process runs. Theme, language, download concurrency and
/// similar launcher-wide concerns are excluded because a per-instance copy would
/// be meaningless or actively confusing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InstanceOverrides {
    /// Memory allocation mode; `None` inherits the global mode.
    pub memory_mode: Option<MemoryMode>,
    /// Maximum heap in MB. Only meaningful together with `MemoryMode::Custom`.
    pub memory_mb: Option<u32>,
    /// Enable the launcher's memory optimisation for this instance.
    ///
    /// Added so the per-instance page can actually change it: the UI has exposed
    /// this switch since the override model was written, but without a field
    /// here the value was silently dropped on save and the launch used the
    /// global setting.
    pub optimize_memory: Option<bool>,
    /// Launch the game fullscreen.
    pub fullscreen: Option<bool>,
    /// Stream game log lines into the launcher.
    pub show_game_log: Option<bool>,
    /// Initial window size; both dimensions must be overridden together to be valid.
    pub window_width: Option<u32>,
    pub window_height: Option<u32>,
    /// Extra JVM arguments appended after the launcher-managed ones.
    pub extra_jvm_args: Option<String>,
    /// Extra arguments passed to the game itself.
    pub game_args: Option<String>,
    /// Command run before the game process starts.
    pub pre_launch_command: Option<String>,
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
