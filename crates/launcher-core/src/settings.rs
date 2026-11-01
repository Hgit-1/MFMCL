use crate::{
    GarbageCollector, InstanceOverrides, LauncherSettings, MemoryMode, PreparedLaunch,
    ProcessPriority,
};
use thiserror::Error;

const MAX_JVM_ARGS_LENGTH: usize = 8192;
const MAX_GAME_ARGS_LENGTH: usize = 8192;
const MAX_PRE_LAUNCH_COMMAND_LENGTH: usize = 4096;
const MAX_SOURCE_LENGTH: usize = 4096;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LaunchSettingsError {
    #[error("游戏参数格式无效，请检查引号是否成对。")]
    InvalidGameArguments,
    #[error("游戏参数不能包含换行或 NUL 字符。")]
    InvalidGameArgumentCharacters,
    #[error("游戏参数不能使用 @argfile。")]
    ArgumentFileNotAllowed,
    #[error("游戏参数不能覆盖受保护的账户身份参数：{0}。")]
    ProtectedIdentityArgument(String),
    #[error("自定义 JVM 参数不能重复设置最大内存。")]
    DuplicateMaximumMemory,
    #[error("自定义 JVM 参数不能覆盖启动器选择的垃圾回收器。")]
    DuplicateGarbageCollector,
    #[error("当前 Java 版本不支持所选垃圾回收器：{0}。")]
    UnsupportedGarbageCollector(String),
}

/// Validate settings that can affect persistence or game execution.
/// `maximum_game_mb` is supplied by the host when the machine-specific limit is known.
pub fn validate_settings(
    settings: &LauncherSettings,
    maximum_game_mb: Option<u32>,
) -> Result<(), String> {
    if !(1..=16).contains(&settings.download.max_concurrent_downloads)
        || !(1..=16).contains(&settings.download.connections_per_file)
    {
        return Err("下载并发数必须在 1 到 16 之间。".to_owned());
    }
    if !(512..=u32::MAX).contains(&settings.memory_mb) {
        return Err("游戏内存必须至少为 512 MB。".to_owned());
    }
    if settings.memory_mode == MemoryMode::Custom
        && maximum_game_mb.is_some_and(|maximum| settings.memory_mb > maximum)
    {
        return Err(format!(
            "自定义游戏内存不能超过本机可用上限 {} MB。",
            maximum_game_mb.unwrap_or_default()
        ));
    }
    if !(640..=7680).contains(&settings.window_width)
        || !(360..=4320).contains(&settings.window_height)
    {
        return Err("窗口宽度必须为 640–7680、高度必须为 360–4320。".to_owned());
    }
    if settings.extra_jvm_args.len() > MAX_JVM_ARGS_LENGTH {
        return Err("JVM 参数最多 8192 字符。".to_owned());
    }
    if settings.game_args.len() > MAX_GAME_ARGS_LENGTH {
        return Err("游戏参数最多 8192 字符。".to_owned());
    }
    if settings.pre_launch_command.len() > MAX_PRE_LAUNCH_COMMAND_LENGTH {
        return Err("启动前命令最多 4096 字符。".to_owned());
    }
    if settings.pre_launch_command.contains(['\n', '\r', '\0']) {
        return Err("启动前命令不能包含换行或 NUL 字符。".to_owned());
    }
    if settings.optimize_memory {
        return Err("内存优化暂不支持，已拒绝保存此设置。".to_owned());
    }
    if settings.process_priority != ProcessPriority::Normal {
        return Err("进程优先级暂不支持，已拒绝保存此设置。".to_owned());
    }
    if settings.close_after_launch {
        return Err("启动后关闭启动器暂不支持。".to_owned());
    }
    if !settings.default_isolated {
        return Err("共享游戏目录暂不支持；实例必须保持隔离。".to_owned());
    }
    if !settings.verify_ssl {
        return Err("TLS 证书校验必须保持开启。".to_owned());
    }
    if settings.custom_accent.len() != 7
        || !settings.custom_accent.starts_with('#')
        || !settings.custom_accent[1..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err("强调色必须为 #RRGGBB。".to_owned());
    }
    if settings.custom_background.as_ref().is_some_and(|image| {
        image.len() > 3 * 1024 * 1024
            || !(image.starts_with("data:image/png;base64,")
                || image.starts_with("data:image/jpeg;base64,")
                || image.starts_with("data:image/webp;base64,"))
    }) {
        return Err("背景只能是有限大小的本地 PNG、JPEG 或 WebP 图片。".to_owned());
    }
    if settings
        .java_path
        .as_ref()
        .is_some_and(|path| path.as_os_str().len() > 4096)
    {
        return Err("Java 路径不能过长。".to_owned());
    }
    if settings.speed_limit_kbps > 1_048_576
        || !(60..=100).contains(&settings.opacity)
        || settings
            .cache_directory
            .as_ref()
            .is_some_and(|path| path.as_os_str().len() > 4096)
        || settings
            .download_directory
            .as_ref()
            .is_some_and(|path| path.as_os_str().len() > 4096)
    {
        return Err("下载速度、透明度或目录设置超出允许范围。".to_owned());
    }
    for value in [
        ("社区源", settings.community_source.as_str()),
        (
            "模组包文件名格式",
            settings.modpack_filename_format.as_str(),
        ),
        ("主页来源", settings.home_source.as_str()),
    ] {
        if value.1.len() > MAX_SOURCE_LENGTH {
            return Err(format!("{}不能超过 {} 字符。", value.0, MAX_SOURCE_LENGTH));
        }
    }
    validate_game_argument_text(&settings.game_args).map_err(|error| error.to_string())?;
    validate_jvm_argument_text(&settings.extra_jvm_args, settings.garbage_collector)
        .map_err(|error| error.to_string())?;
    Ok(())
}

const MAX_WINDOW_WIDTH: u32 = 7680;
const MAX_WINDOW_HEIGHT: u32 = 4320;
const MIN_WINDOW_WIDTH: u32 = 640;
const MIN_WINDOW_HEIGHT: u32 = 360;
const MIN_INSTANCE_MEMORY_MB: u32 = 512;

/// Validate a set of per-instance overrides in isolation.
///
/// Only the fields that are actually present are checked, and each check mirrors
/// the global rule for the same field. Keeping this separate from
/// `resolve_settings` means the store can reject a bad payload at write time,
/// instead of accepting it and failing much later during launch.
pub fn validate_instance_overrides(overrides: &InstanceOverrides) -> Result<(), String> {
    if let Some(memory_mb) = overrides.memory_mb {
        if memory_mb < MIN_INSTANCE_MEMORY_MB {
            return Err(format!("游戏内存必须至少为 {MIN_INSTANCE_MEMORY_MB} MB。"));
        }
        // A heap size stored without custom mode can never take effect, so it is
        // almost certainly a UI bug or a hand-edited database. Reject it rather
        // than silently ignoring a number the user believes they set.
        if overrides.memory_mode != Some(MemoryMode::Custom) {
            return Err("指定内存数值时必须同时选择自定义内存模式。".to_owned());
        }
    }
    if overrides.memory_mode == Some(MemoryMode::Custom) && overrides.memory_mb.is_none() {
        return Err("自定义内存模式必须指定内存数值。".to_owned());
    }

    // Window size is a single setting stored as two numbers. Half of it would
    // produce a window with one overridden dimension and one inherited, which no
    // UI ever offers and which is hard to explain when something looks wrong.
    match (overrides.window_width, overrides.window_height) {
        (Some(width), Some(height)) => {
            if !(MIN_WINDOW_WIDTH..=MAX_WINDOW_WIDTH).contains(&width)
                || !(MIN_WINDOW_HEIGHT..=MAX_WINDOW_HEIGHT).contains(&height)
            {
                return Err(format!(
                    "窗口宽度必须为 {MIN_WINDOW_WIDTH}–{MAX_WINDOW_WIDTH}、高度必须为 {MIN_WINDOW_HEIGHT}–{MAX_WINDOW_HEIGHT}。"
                ));
            }
        }
        (None, None) => {}
        _ => return Err("覆盖窗口尺寸时必须同时指定宽度和高度。".to_owned()),
    }

    if let Some(text) = &overrides.extra_jvm_args {
        if text.len() > MAX_JVM_ARGS_LENGTH {
            return Err(format!("JVM 参数最多 {MAX_JVM_ARGS_LENGTH} 字符。"));
        }
        validate_jvm_argument_text(text, GarbageCollector::Auto)
            .map_err(|error| error.to_string())?;
    }
    if let Some(text) = &overrides.game_args {
        if text.len() > MAX_GAME_ARGS_LENGTH {
            return Err(format!("游戏参数最多 {MAX_GAME_ARGS_LENGTH} 字符。"));
        }
        validate_game_argument_text(text).map_err(|error| error.to_string())?;
    }
    if let Some(command) = &overrides.pre_launch_command {
        if command.len() > MAX_PRE_LAUNCH_COMMAND_LENGTH {
            return Err(format!(
                "启动前命令最多 {MAX_PRE_LAUNCH_COMMAND_LENGTH} 字符。"
            ));
        }
        if command.contains(['\n', '\r', '\0']) {
            return Err("启动前命令不能包含换行或 NUL 字符。".to_owned());
        }
    }
    Ok(())
}

/// Collapse global settings plus instance overrides into the settings this
/// instance will actually launch with.
///
/// This is the single place where inheritance is resolved. Doing the merge here,
/// in the core crate, keeps the launcher UI, the CLI and the tests all applying
/// the same rules — a frontend-only merge would drift the moment another entry
/// point appears.
///
/// The result is a complete `LauncherSettings`, so `apply_launch_settings` and
/// the rest of the launch path stay unaware that instances exist at all.
pub fn resolve_settings(
    global: &LauncherSettings,
    overrides: &InstanceOverrides,
) -> LauncherSettings {
    let mut resolved = global.clone();
    if let Some(mode) = overrides.memory_mode {
        resolved.memory_mode = mode;
    }
    if let Some(memory_mb) = overrides.memory_mb {
        resolved.memory_mb = memory_mb;
    }
    if let Some(optimize) = overrides.optimize_memory {
        resolved.optimize_memory = optimize;
    }
    if let Some(fullscreen) = overrides.fullscreen {
        resolved.fullscreen = fullscreen;
    }
    if let Some(show_game_log) = overrides.show_game_log {
        resolved.show_game_log = show_game_log;
    }
    if let Some(width) = overrides.window_width {
        resolved.window_width = width;
    }
    if let Some(height) = overrides.window_height {
        resolved.window_height = height;
    }
    if let Some(args) = &overrides.extra_jvm_args {
        resolved.extra_jvm_args = args.clone();
    }
    if let Some(args) = &overrides.game_args {
        resolved.game_args = args.clone();
    }
    if let Some(command) = &overrides.pre_launch_command {
        resolved.pre_launch_command = command.clone();
    }
    resolved
}

/// How many settings this instance overrides.
///
/// Pairs that represent one user-visible setting are counted once, matching what
/// the UI shows: window size is two columns, and a memory heap size is part of
/// the memory mode. Counting columns instead of settings would report "3
/// customised" for two changes and make the summary look wrong.
pub fn count_instance_overrides(overrides: &InstanceOverrides) -> usize {
    let mut count = 0;
    if overrides.memory_mode.is_some() || overrides.memory_mb.is_some() {
        count += 1;
    }
    if overrides.fullscreen.is_some() {
        count += 1;
    }
    if overrides.optimize_memory.is_some() {
        count += 1;
    }
    if overrides.show_game_log.is_some() {
        count += 1;
    }
    if overrides.window_width.is_some() || overrides.window_height.is_some() {
        count += 1;
    }
    // Blank text is treated as "no extra content": leaving the box empty has the
    // same effect as inheriting, so it should not read as a customisation.
    if overrides
        .extra_jvm_args
        .as_deref()
        .is_some_and(|text| !text.trim().is_empty())
    {
        count += 1;
    }
    if overrides
        .game_args
        .as_deref()
        .is_some_and(|text| !text.trim().is_empty())
    {
        count += 1;
    }
    if overrides
        .pre_launch_command
        .as_deref()
        .is_some_and(|text| !text.trim().is_empty())
    {
        count += 1;
    }
    count
}

pub fn apply_launch_settings(
    prepared: &mut PreparedLaunch,
    settings: &LauncherSettings,
    recommended_memory_mb: u32,
    main_class: &str,
) -> Result<(), LaunchSettingsError> {
    prepared
        .args
        .retain(|argument| !argument.starts_with("-Xmx"));
    let main_index = prepared
        .args
        .iter()
        .position(|argument| argument == main_class)
        .ok_or(LaunchSettingsError::InvalidGameArguments)?;
    let memory_mb = match settings.memory_mode {
        MemoryMode::Auto => recommended_memory_mb,
        MemoryMode::Custom => settings.memory_mb,
    };
    let mut jvm_args = vec![format!("-Xmx{memory_mb}M")];
    jvm_args.extend(garbage_collector_args(settings.garbage_collector));
    let custom = parse_jvm_args(&settings.extra_jvm_args)?;
    jvm_args.extend(custom);
    prepared.args.splice(main_index..main_index, jvm_args);

    set_game_option(
        &mut prepared.args,
        "--width",
        &settings.window_width.to_string(),
    );
    set_game_option(
        &mut prepared.args,
        "--height",
        &settings.window_height.to_string(),
    );
    if settings.fullscreen {
        if !prepared
            .args
            .iter()
            .any(|argument| argument == "--fullscreen")
        {
            prepared.args.push("--fullscreen".to_owned());
        }
    } else {
        prepared.args.retain(|argument| argument != "--fullscreen");
    }

    let game_args = parse_game_args(&settings.game_args)?;
    prepared.args.extend(game_args);
    Ok(())
}

fn garbage_collector_args(collector: GarbageCollector) -> Vec<String> {
    match collector {
        GarbageCollector::Auto => Vec::new(),
        GarbageCollector::G1 => vec!["-XX:+UseG1GC".to_owned()],
        GarbageCollector::Serial => vec!["-XX:+UseSerialGC".to_owned()],
        GarbageCollector::Parallel => vec!["-XX:+UseParallelGC".to_owned()],
    }
}

fn parse_jvm_args(value: &str) -> Result<Vec<String>, LaunchSettingsError> {
    let args = shlex::split(value).ok_or(LaunchSettingsError::InvalidGameArguments)?;
    if args
        .iter()
        .any(|argument| argument.contains(['\n', '\r', '\0']))
    {
        return Err(LaunchSettingsError::InvalidGameArgumentCharacters);
    }
    if args.iter().any(|argument| argument.starts_with("-Xmx")) {
        return Err(LaunchSettingsError::DuplicateMaximumMemory);
    }
    if args
        .iter()
        .any(|argument| is_garbage_collector_argument(argument))
    {
        return Err(LaunchSettingsError::DuplicateGarbageCollector);
    }
    Ok(args)
}

fn validate_jvm_argument_text(
    value: &str,
    collector: GarbageCollector,
) -> Result<(), LaunchSettingsError> {
    let args = parse_jvm_args(value)?;
    if collector != GarbageCollector::Auto
        && args
            .iter()
            .any(|argument| is_garbage_collector_argument(argument))
    {
        return Err(LaunchSettingsError::DuplicateGarbageCollector);
    }
    Ok(())
}

fn is_garbage_collector_argument(argument: &str) -> bool {
    matches!(
        argument,
        "-XX:+UseG1GC"
            | "-XX:-UseG1GC"
            | "-XX:+UseSerialGC"
            | "-XX:-UseSerialGC"
            | "-XX:+UseParallelGC"
            | "-XX:-UseParallelGC"
            | "-XX:+UseZGC"
            | "-XX:-UseZGC"
            | "-XX:+UseShenandoahGC"
            | "-XX:-UseShenandoahGC"
    )
}

fn parse_game_args(value: &str) -> Result<Vec<String>, LaunchSettingsError> {
    let args = shlex::split(value).ok_or(LaunchSettingsError::InvalidGameArguments)?;
    if args
        .iter()
        .any(|argument| argument.contains(['\n', '\r', '\0']))
    {
        return Err(LaunchSettingsError::InvalidGameArgumentCharacters);
    }
    for (index, argument) in args.iter().enumerate() {
        if argument.starts_with('@') {
            return Err(LaunchSettingsError::ArgumentFileNotAllowed);
        }
        if let Some(flag) = protected_identity_flag(argument) {
            return Err(LaunchSettingsError::ProtectedIdentityArgument(
                flag.to_owned(),
            ));
        }
        if index > 0 && protected_identity_flag(&args[index - 1]).is_some() {
            return Err(LaunchSettingsError::ProtectedIdentityArgument(
                args[index - 1].clone(),
            ));
        }
    }
    Ok(args)
}

fn validate_game_argument_text(value: &str) -> Result<(), LaunchSettingsError> {
    parse_game_args(value).map(|_| ())
}

fn protected_identity_flag(argument: &str) -> Option<&str> {
    const FLAGS: &[&str] = &[
        "--username",
        "--uuid",
        "--accessToken",
        "--access-token",
        "--authPlayerName",
        "--auth_player_name",
        "--authUuid",
        "--auth_uuid",
        "--authAccessToken",
        "--auth_access_token",
        "--authSession",
        "--auth_session",
        "--profile",
        "--profileId",
        "--profile-id",
        "--clientId",
        "--client-id",
        "--xuid",
        "--userType",
        "--user-type",
        "--userProperties",
        "--user-properties",
    ];
    FLAGS.iter().copied().find(|flag| {
        argument == *flag
            || argument
                .strip_prefix(flag)
                .is_some_and(|suffix| suffix.starts_with('='))
    })
}

fn set_game_option(args: &mut Vec<String>, option: &str, value: &str) {
    if let Some(index) = args.iter().position(|argument| argument == option) {
        if let Some(existing) = args.get_mut(index + 1) {
            *existing = value.to_owned();
        } else {
            args.insert(index + 1, value.to_owned());
        }
    } else {
        args.extend([option.to_owned(), value.to_owned()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn prepared() -> PreparedLaunch {
        PreparedLaunch {
            java_path: PathBuf::from("java"),
            args: vec!["-cp".into(), "game.jar".into(), "net.minecraft.Main".into()],
            working_directory: PathBuf::from("."),
            log_path: PathBuf::from("game.log"),
        }
    }

    #[test]
    fn game_arguments_are_appended_and_controlled_identity_is_protected() {
        let mut launch = prepared();
        let mut settings = LauncherSettings {
            memory_mode: MemoryMode::Auto,
            garbage_collector: GarbageCollector::G1,
            game_args: "--demo --width=1".into(),
            ..LauncherSettings::default()
        };
        apply_launch_settings(&mut launch, &settings, 3072, "net.minecraft.Main").unwrap();
        assert!(launch.args.contains(&"-Xmx3072M".to_owned()));
        assert!(launch.args.contains(&"-XX:+UseG1GC".to_owned()));
        assert_eq!(launch.args.last().unwrap(), "--width=1");

        settings.game_args = "--accessToken replaced".into();
        assert!(matches!(
            apply_launch_settings(&mut prepared(), &settings, 3072, "net.minecraft.Main"),
            Err(LaunchSettingsError::ProtectedIdentityArgument(_))
        ));
        settings.game_args = "@options.txt".into();
        assert_eq!(
            validate_settings(&settings, Some(8192)).unwrap_err(),
            LaunchSettingsError::ArgumentFileNotAllowed.to_string()
        );
    }

    #[test]
    fn settings_validation_rejects_unsafe_or_unimplemented_options() {
        let mut settings = LauncherSettings {
            verify_ssl: false,
            ..LauncherSettings::default()
        };
        assert!(validate_settings(&settings, Some(8192)).is_err());
        settings.verify_ssl = true;
        settings.default_isolated = false;
        assert!(validate_settings(&settings, Some(8192)).is_err());
        settings.default_isolated = true;
        settings.process_priority = ProcessPriority::High;
        assert!(validate_settings(&settings, Some(8192)).is_err());
    }

    #[test]
    fn empty_overrides_resolve_to_the_global_settings() {
        let global = LauncherSettings {
            memory_mode: MemoryMode::Custom,
            memory_mb: 4096,
            fullscreen: true,
            window_width: 1280,
            window_height: 720,
            extra_jvm_args: "-XX:+UseZGC".into(),
            ..LauncherSettings::default()
        };
        let resolved = resolve_settings(&global, &InstanceOverrides::default());
        // Default overrides must be a no-op: an instance the user never touched
        // has to behave exactly like the global configuration.
        assert_eq!(resolved.memory_mode, MemoryMode::Custom);
        assert_eq!(resolved.memory_mb, 4096);
        assert!(resolved.fullscreen);
        assert_eq!(resolved.window_width, 1280);
        assert_eq!(resolved.extra_jvm_args, "-XX:+UseZGC");
    }

    #[test]
    fn overrides_replace_only_the_fields_they_set() {
        let global = LauncherSettings {
            memory_mode: MemoryMode::Custom,
            memory_mb: 4096,
            fullscreen: true,
            show_game_log: true,
            window_width: 1280,
            window_height: 720,
            game_args: "--global".into(),
            ..LauncherSettings::default()
        };
        let overrides = InstanceOverrides {
            fullscreen: Some(false),
            window_width: Some(1920),
            window_height: Some(1080),
            ..InstanceOverrides::default()
        };
        let resolved = resolve_settings(&global, &overrides);
        assert!(
            !resolved.fullscreen,
            "explicit false must survive the merge"
        );
        assert_eq!(resolved.window_width, 1920);
        assert_eq!(resolved.window_height, 1080);
        // Untouched fields keep the global value.
        assert_eq!(resolved.memory_mb, 4096);
        assert!(resolved.show_game_log);
        assert_eq!(resolved.game_args, "--global");
    }

    #[test]
    fn memory_override_needs_a_matching_mode() {
        // A heap size without custom mode could never take effect, so storing it
        // would silently lose a number the user believed they had set.
        let memory_without_mode = InstanceOverrides {
            memory_mb: Some(8192),
            ..InstanceOverrides::default()
        };
        assert!(validate_instance_overrides(&memory_without_mode).is_err());

        let mode_without_memory = InstanceOverrides {
            memory_mode: Some(MemoryMode::Custom),
            ..InstanceOverrides::default()
        };
        assert!(validate_instance_overrides(&mode_without_memory).is_err());

        let complete = InstanceOverrides {
            memory_mode: Some(MemoryMode::Custom),
            memory_mb: Some(8192),
            ..InstanceOverrides::default()
        };
        assert!(validate_instance_overrides(&complete).is_ok());
    }

    #[test]
    fn window_size_override_must_be_complete() {
        let half = InstanceOverrides {
            window_width: Some(1920),
            ..InstanceOverrides::default()
        };
        assert!(validate_instance_overrides(&half).is_err());

        let out_of_range = InstanceOverrides {
            window_width: Some(100),
            window_height: Some(100),
            ..InstanceOverrides::default()
        };
        assert!(validate_instance_overrides(&out_of_range).is_err());

        let valid = InstanceOverrides {
            window_width: Some(1920),
            window_height: Some(1080),
            ..InstanceOverrides::default()
        };
        assert!(validate_instance_overrides(&valid).is_ok());
    }

    #[test]
    fn overrides_reuse_the_global_argument_rules() {
        // The instance page must not become a way around the -Xmx and identity
        // protections that the global settings already enforce.
        let duplicate_memory = InstanceOverrides {
            extra_jvm_args: Some("-Xmx2G".into()),
            ..InstanceOverrides::default()
        };
        assert!(validate_instance_overrides(&duplicate_memory).is_err());

        let identity = InstanceOverrides {
            game_args: Some("--accessToken forged".into()),
            ..InstanceOverrides::default()
        };
        assert!(validate_instance_overrides(&identity).is_err());

        let newline_command = InstanceOverrides {
            pre_launch_command: Some("echo a\necho b".into()),
            ..InstanceOverrides::default()
        };
        assert!(validate_instance_overrides(&newline_command).is_err());
    }

    #[test]
    fn override_count_treats_paired_fields_as_one_setting() {
        // Window size is two columns but one user-visible setting; counting both
        // would make the page report "3 customised" for two changes.
        let window = InstanceOverrides {
            window_width: Some(1920),
            window_height: Some(1080),
            ..InstanceOverrides::default()
        };
        assert_eq!(count_instance_overrides(&window), 1);

        let memory = InstanceOverrides {
            memory_mode: Some(MemoryMode::Custom),
            memory_mb: Some(6144),
            ..InstanceOverrides::default()
        };
        assert_eq!(count_instance_overrides(&memory), 1);

        let everything = InstanceOverrides {
            memory_mode: Some(MemoryMode::Auto),
            fullscreen: Some(true),
            show_game_log: Some(false),
            window_width: Some(1920),
            window_height: Some(1080),
            extra_jvm_args: Some("-XX:+UseZGC".into()),
            game_args: Some("--demo".into()),
            pre_launch_command: Some("echo hi".into()),
            ..InstanceOverrides::default()
        };
        assert_eq!(count_instance_overrides(&everything), 7);
        assert_eq!(count_instance_overrides(&InstanceOverrides::default()), 0);
    }

    #[test]
    fn blank_text_overrides_are_not_counted_as_customisations() {
        // Clearing a text box has the same effect as inheriting, so the page must
        // not claim the instance is customised when it behaves identically.
        let blank = InstanceOverrides {
            extra_jvm_args: Some("   ".into()),
            game_args: Some(String::new()),
            pre_launch_command: Some("\t".into()),
            ..InstanceOverrides::default()
        };
        assert_eq!(count_instance_overrides(&blank), 0);
    }
}
