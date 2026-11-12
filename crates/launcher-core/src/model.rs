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
    pub max_concurrent_downloads: u16,
    pub connections_per_file: u16,
    pub aria2_enabled: bool,
    #[serde(default)]
    pub proxy_mode: download_engine::ProxyMode,
    #[serde(default)]
    pub proxy_url: String,
    #[serde(default = "default_download_timeout_sec")]
    pub timeout_sec: u16,
    #[serde(default = "default_download_retries")]
    pub retries: u8,
}

fn default_download_timeout_sec() -> u16 {
    download_engine::network::DEFAULT_TIMEOUT_SEC
}

fn default_download_retries() -> u8 {
    download_engine::network::DEFAULT_RETRIES
}

impl DownloadSettings {
    pub fn to_engine_settings(&self) -> download_engine::DownloadSettings {
        download_engine::DownloadSettings {
            source: self.source,
            max_concurrent_downloads: self.max_concurrent_downloads,
            connections_per_file: self.connections_per_file,
            aria2_enabled: self.aria2_enabled,
            proxy_mode: self.proxy_mode,
            proxy_url: self.proxy_url.clone(),
            timeout_sec: self.timeout_sec,
            retries: self.retries,
        }
    }
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
            max_concurrent_downloads: 16,
            connections_per_file: 256,
            aria2_enabled: true,
            proxy_mode: download_engine::ProxyMode::None,
            proxy_url: String::new(),
            timeout_sec: default_download_timeout_sec(),
            retries: default_download_retries(),
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
    /// 游戏文件的根目录，默认 `~/.minecraft`。
    ///
    /// 为什么默认不是启动器自己的数据目录：绝大多数玩家已经有 `~/.minecraft`，
    /// 里面放着手动装的模组、存档和朋友给的整合包。把实例装到
    /// `<app-data>/instances/<uuid>/game` 意味着启动器对这个既有目录视而不见，
    /// 用户要重新下载一遍同样的游戏，而且两边的存档互不相通。
    ///
    /// 隔离仍然存在：`instances/<id>` 里放版本与 libraries 的实例私有部分，
    /// 共享根目录只承载原版客户端、资源和用户可见的 `.minecraft` 内容。
    #[serde(default)]
    pub game_root: Option<PathBuf>,
    /// 是否把启动器自身的日志写到固定文件（默认关闭）。
    ///
    /// 这个字段取代了原来的 `anonymous_usage`：那个开关没有任何收集实现，只是
    /// 一个无法兑现的隐私承诺。现在它对应一件真的会发生的事——写本地日志文件，
    /// 由用户自己决定要不要把文件贴到 issue 里。没有任何数据离开这台机器。
    #[serde(default)]
    /*
     * 日志默认开启。
     *
     * 内容已经过 `redact_log` 脱敏，不含凭据；而「装到 90% 卡住、日志里什么
     * 都没有」是不可诊断的——用户报问题时手里必须有证据。想关掉仍可在设置里关。
     */
    pub launcher_log_enabled: bool,
    /// 是否在启动器启动时清掉已结束的下载任务记录（默认开启）。
    ///
    /// 默认开启的理由：任务列表是给人看「正在发生什么」的，历史记录会自动变成
    /// 噪音——一次安装会产生三条（整合包、模组、本体），几天后就攒满一屏全是
    /// 已完成。清掉的只是记录，不动任何已下载的文件。
    ///
    /// 保留关闭选项是因为有人会把失败记录留着排查；那种情况下自动删除等于把
    /// 证据丢了，必须能让用户关掉。
    #[serde(default = "default_true")]
    pub clear_finished_tasks: bool,
    /// 用户指定的 `llama-server` 可执行文件路径。
    ///
    /// 为什么需要它：llama.cpp 的发布物按平台与加速后端分叉，安装包不可能全带上。
    /// 允许用户指定自己那份（自编译、LM Studio 自带、发行版包管理器装的），
    /// 是让「本地 AI 分析」在任意机器上都能真的用起来的唯一通用做法。
    ///
    /// 为 `None` 时启动器会依次尝试环境变量、附带资源、应用数据目录与 `PATH`。
    #[serde(default)]
    pub llama_server_path: Option<PathBuf>,
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
            game_root: None,
            launcher_log_enabled: true,
            clear_finished_tasks: true,
            llama_server_path: None,
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

impl AccountProfile {
    /// The player name, validated as safe to display.
    ///
    /// ## Why this exists
    ///
    /// A Minecraft player name is public — it appears beside every chat message
    /// and on every server's player list. But `display_name` lives on a record
    /// that also holds an auth-server URL and a keyring reference, and static
    /// analysis treats the whole record as account data. Formatting the raw field
    /// into terminal output therefore reads as "logging account data", and it is
    /// reported as one.
    ///
    /// This accessor is the explicit boundary: it re-applies the character-set
    /// rule that every code path producing a name already enforces (Microsoft and
    /// Yggdrasil profiles are validated by `GameProfile::new`, offline profiles by
    /// `offline_profile`), and returns `None` for anything that does not match.
    /// A caller that gets `Some` is holding a plain player name, not an account
    /// record.
    ///
    /// The check is not decorative: a database edited by hand, or a future
    /// provider that stops validating names, would otherwise let a string with
    /// control characters or delimiters reach the terminal.
    pub fn public_player_name(&self) -> Option<&str> {
        let name = self.display_name.as_str();
        if (3..=16).contains(&name.len())
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            Some(name)
        } else {
            None
        }
    }

    /// The player name for display, falling back to a placeholder.
    ///
    /// Returning a fixed string for an unrepresentable name keeps callers from
    /// having to branch, and makes the terminal output honest: something is wrong
    /// with the record rather than with the name.
    pub fn display_player_name(&self) -> &str {
        self.public_player_name().unwrap_or("（无效名称）")
    }
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

impl InstanceRecord {
    /// 版本文件所在的目录：`<game 根>/versions/<实例目录名>`。
    ///
    /// ## 为什么放在共享根目录下
    ///
    /// 用户期望的是 `~/.minecraft/versions/<名字>`——那是原版启动器与 PCL2 的布局，
    /// 也是手动改配置、换 jar、装 OptiFine 时大家会去的地方。之前它藏在
    /// `<app-data>/instances/<uuid>/versions/` 里，与 `.minecraft` 完全隔绝。
    ///
    /// ## 为什么用实例目录名而不是 MC 版本号
    ///
    /// 同一台机器上常有两个不同加载器的 1.21.1 实例（原版一个、NeoForge 一个），
    /// 用 `versions/1.21.1` 会互相覆盖。实例目录名已经含版本与加载器信息
    /// （`1.21.1 neoforge 21.1.265`），天然唯一。
    ///
    /// ## 旧实例的兼容
    ///
    /// 早期实例的版本文件在 `instance_dir/versions/<mc 版本>`。如果那个目录存在，
    /// 仍然用它——不迁移、不复制。判断放在这里而不是调用点，三条路径
    /// （安装 / 启动 / 加载器）就都自动一致。
    ///
    /// 判据必须精确到「`versions/<mc 版本>` 这一层」，不能只看 `versions/` 是否
    /// 存在：当前布局在共享根下写的是 `versions/<实例目录名>/<mc 版本>/`，而隔离
    /// 实例的共享根就是 `instance_dir`，于是安装完成后 `instance_dir/versions/`
    /// 一定存在。若仅凭它存在就判成旧布局，`version_dir()` 会少掉 `<实例目录名>`
    /// 这一层，指向一个不存在的路径——启动、Java 选择、加载器安装都会报
    /// 「系统找不到指定的路径」。
    pub fn version_root(&self) -> PathBuf {
        let legacy = self.instance_dir.join("versions");
        if legacy.join(&self.game_version).is_dir() {
            return legacy;
        }
        match self.instance_dir.file_name().and_then(|n| n.to_str()) {
            Some(dir_name) => {
                // `game_dir` 是 `<game 根>/shared`，上一级就是根目录。
                match self.game_dir.parent() {
                    Some(root) => root.join("versions").join(dir_name),
                    // 没有共享根（隔离实例）：留在实例目录内。
                    None => legacy,
                }
            }
            None => legacy,
        }
    }

    /// 这个实例的版本目录（`version_root()` 下以 MC 版本号命名的那一层）。
    pub fn version_dir(&self) -> PathBuf {
        self.version_root().join(&self.game_version)
    }
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
    /// 本实例的游戏目录。
    ///
    /// 每个实例单独一份：`~/.minecraft` 作为默认值适合「一个主存档」，但用户
    /// 常会想让某个实例用独立的目录——装了大量模组的测试实例不该把 `mods/` 和
    /// 主实例混在一起，整合包也常常自带目录要求。
    ///
    /// 空字符串表示「删除这个覆盖，回到默认」。用 `Some("")` 而不是 `None` 来表达
    /// 清除，是因为 `None` 已经表示「字段不存在」，两者在合并时语义不同。
    pub game_dir: Option<String>,
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
    /*
     * 任务级的文件计数。
     *
     * 为什么需要和字节数并列：进度条只给出比例，而「3/12 个文件」回答的是
     * 「还剩几个」。安装大版本时两者差别明显——90% 的字节可能只差一个小库，
     * 也可能是最后 10 个资源文件之一。
     *
     * 全部 `serde(default)` 且可空：旧库里的记录没有这两列，读出时是 0/None，
     * 界面据此不显示文件计数，而不是显示「0/0 个文件」。
     */
    #[serde(default)]
    pub files: Option<u32>,
    #[serde(default)]
    pub completed_files: Option<u32>,
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
        /*
         * 实例名会成为磁盘上的目录名，所以必须拒绝路径分隔符。
         * 之前只校验了长度，`../../etc` 这样的名字能一路走到 `join()` 里。
         */
        if name.contains(['/', '\\', '\0', ':', '*', '?', '"', '<', '>', '|']) {
            return Err("实例名称不能包含 / \\ : * ? \" < > | 等字符。".to_owned());
        }
        Ok(())
    }
}

/// 目录名的最大长度。
///
/// 255 是常见文件系统的单段上限，但留给后续拼接 `-1`、`-2` 这类后缀，并且
/// 中文按字符数而不是字节数限制（`~/.minecraft/versions/<名>` 里中文很常见）。
const MAX_DIRECTORY_NAME_CHARS: usize = 120;

/// 把实例名转成安全的目录名。
///
/// 保留中文与常规标点：`~/.minecraft/versions/1.21.1 neoforge 21.1.265` 这样的
/// 名字正是用户期望的。只替换真正不能出现在路径里的字符，并且拒绝空结果。
pub fn directory_name_for(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "/\\:*?\"<>|".contains(c) {
                '-'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').trim();
    let cut: String = trimmed.chars().take(MAX_DIRECTORY_NAME_CHARS).collect();
    if cut.is_empty() {
        "instance".to_owned()
    } else {
        cut
    }
}

#[cfg(test)]
mod account_name_tests {
    use super::*;

    fn profile(name: &str) -> AccountProfile {
        AccountProfile {
            id: "id".into(),
            display_name: name.into(),
            kind: AccountKind::Offline,
            auth_server: None,
            credential_ref: None,
            selected: true,
            created_at: String::new(),
        }
    }

    #[test]
    fn a_normal_player_name_is_exposed_for_display() {
        // Every real name reaches the terminal through this accessor, so the
        // common case has to be the unremarkable one.
        for name in ["Steve", "Alex_99", "abc"] {
            let profile = profile(name);
            assert_eq!(profile.public_player_name(), Some(name));
            assert_eq!(profile.display_player_name(), name);
        }
    }

    #[test]
    fn names_outside_the_minecraft_rule_are_not_exposed() {
        /*
         * The boundary exists so a record that was hand-edited, or produced by a
         * future provider that stopped validating, cannot push control characters
         * or delimiters into terminal output. These are the shapes that would have
         * done exactly that.
         */
        for name in [
            "",
            "ab",
            "this_name_is_way_too_long_for_minecraft",
            "has space",
            "tab\there",
            "quote\"inside",
            "lines;and,more",
        ] {
            let profile = profile(name);
            assert_eq!(
                profile.public_player_name(),
                None,
                "{name:?} must not be treated as displayable"
            );
            // The fallback keeps callers branch-free and never echoes the raw value.
            assert_eq!(profile.display_player_name(), "（无效名称）");
        }
    }
}
