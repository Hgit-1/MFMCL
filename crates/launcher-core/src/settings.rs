use crate::{GarbageCollector, LauncherSettings, MemoryMode, PreparedLaunch, ProcessPriority};
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
        let mut settings = LauncherSettings::default();
        settings.memory_mode = MemoryMode::Auto;
        settings.garbage_collector = GarbageCollector::G1;
        settings.game_args = "--demo --width=1".into();
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
        let mut settings = LauncherSettings::default();
        settings.verify_ssl = false;
        assert!(validate_settings(&settings, Some(8192)).is_err());
        settings.verify_ssl = true;
        settings.default_isolated = false;
        assert!(validate_settings(&settings, Some(8192)).is_err());
        settings.default_isolated = true;
        settings.process_priority = ProcessPriority::High;
        assert!(validate_settings(&settings, Some(8192)).is_err());
    }
}
