//! `mfmcl config` — read and change launcher settings.
//!
//! ## Why keys are a fixed list rather than raw JSON
//!
//! Writing the whole settings object from the CLI would let a typo wipe unrelated
//! fields, and it would expose fields a terminal cannot sensibly edit (window
//! opacity, theme accent). A fixed key list means every key has a documented type,
//! a validation rule, and the same meaning in the GUI.
//!
//! ## Validation is not optional
//!
//! `config set memory 100` must fail here, not at launch time three days later.
//! Every write goes through the same `validate_settings` the GUI uses.

use crate::cli::{ConfigAction, ConfigArgs};
use crate::commands::download::terminal_width;
use crate::commands::{Failure, Outcome, Session};
use crate::output::{Palette, Table};
use launcher_core::{
    GarbageCollector, LauncherSettings, MemoryMode, ProcessPriority, system_memory_info,
    validate_settings,
};
use mc_metadata::SourcePreference;

pub fn run(session: &Session<'_>, args: &ConfigArgs) -> Outcome {
    let palette = Palette::new(session.sink.palette().on);
    match &args.action {
        None => list(session, &palette),
        Some(ConfigAction::List) => list(session, &palette),
        Some(ConfigAction::Get { key }) => get(session, &palette, key),
        Some(ConfigAction::Set { key, value }) => set(session, &palette, key, value),
    }
}

/// Every key the CLI will read or write, with a one-line description.
///
/// Ordering is the order a person would configure things: how much memory, which
/// Java, then download behaviour, then extras.
const KEYS: &[(&str, &str)] = &[
    ("memory", "游戏最大内存（MB，最小 512）"),
    ("memory-mode", "内存模式：auto 或 custom"),
    ("java", "Java 可执行文件路径；留空表示自动探测"),
    ("jvm-args", "追加的 JVM 参数"),
    ("game-args", "追加的游戏参数"),
    ("pre-launch", "启动前执行的命令"),
    ("gc", "垃圾回收器：auto、g1、serial、parallel"),
    ("fullscreen", "是否全屏启动（true/false）"),
    ("source", "下载源：official、bmclapi、mixed"),
    ("concurrency", "同时下载的文件数（1–16）"),
    ("connections", "每个文件的连接数（1–16）"),
    ("speed-limit", "下载限速（KiB/s，0 表示不限）"),
    ("game-root", "游戏文件根目录；留空表示 ~/.minecraft"),
    ("data-dir", "启动器数据目录（只读，由 --data-dir 决定）"),
];

fn list(session: &Session<'_>, palette: &Palette) -> Outcome {
    let settings = session.store.load_settings()?;
    if session.cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&settings).unwrap_or_default()
        );
        return Ok(());
    }
    let mut table = Table::new(&["键", "当前值", "说明"]);
    for (key, description) in KEYS {
        let value = read_key(&settings, key, session);
        table.row(vec![
            key.to_string(),
            value.unwrap_or_else(|| palette.dim("—")),
            palette.dim(description),
        ]);
    }
    print!("{}", table.render(palette, terminal_width()));
    println!();
    println!("{}", palette.dim("修改：mfmcl config set memory 4096"));
    Ok(())
}

fn get(session: &Session<'_>, palette: &Palette, key: &str) -> Outcome {
    let settings = session.store.load_settings()?;
    let value = read_key(&settings, key, session).ok_or_else(|| {
        Failure::message(format!(
            "未知设置项「{key}」。可用：{}",
            KEYS.iter()
                .map(|(key, _)| *key)
                .collect::<Vec<_>>()
                .join("、")
        ))
    })?;
    if session.cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "key": key, "value": value }))
                .unwrap_or_default()
        );
        return Ok(());
    }
    println!("{}", palette.bold(&value));
    Ok(())
}

fn set(session: &Session<'_>, palette: &Palette, key: &str, value: &str) -> Outcome {
    let mut settings = session.store.load_settings()?;
    let value = value.trim();

    match key {
        "memory" => {
            let mb: u32 = value.parse().map_err(|_| {
                Failure::message(format!("内存必须是一个整数（MB），收到「{value}」。"))
            })?;
            settings.memory_mb = mb;
            settings.memory_mode = MemoryMode::Custom;
        }
        "memory-mode" => {
            settings.memory_mode = match value.to_ascii_lowercase().as_str() {
                "auto" => MemoryMode::Auto,
                "custom" => MemoryMode::Custom,
                other => {
                    return Err(Failure::message(format!(
                        "内存模式只能是 auto 或 custom，收到「{other}」。"
                    )));
                }
            };
        }
        "java" => {
            settings.java_path = if value.is_empty() {
                None
            } else {
                // Fail now if the path is obviously wrong. Checking that the file
                // runs is the launcher's job; checking that it exists is cheap and
                // catches the common typo.
                let path = std::path::PathBuf::from(value);
                if !path.is_file() {
                    return Err(Failure::message(format!(
                        "找不到 Java 可执行文件：{}",
                        path.display()
                    )));
                }
                Some(path)
            };
        }
        "jvm-args" => settings.extra_jvm_args = value.to_owned(),
        "game-args" => settings.game_args = value.to_owned(),
        "pre-launch" => settings.pre_launch_command = value.to_owned(),
        "gc" => {
            settings.garbage_collector = match value.to_ascii_lowercase().as_str() {
                "auto" => GarbageCollector::Auto,
                "g1" => GarbageCollector::G1,
                "serial" => GarbageCollector::Serial,
                "parallel" => GarbageCollector::Parallel,
                other => {
                    return Err(Failure::message(format!(
                        "垃圾回收器只能是 auto、g1、serial 或 parallel，收到「{other}」。"
                    )));
                }
            };
        }
        "fullscreen" => {
            settings.fullscreen = parse_bool(value)?;
        }
        "source" => {
            settings.community_source = value.to_ascii_lowercase();
            settings.download.source = match value.to_ascii_lowercase().as_str() {
                "official" => SourcePreference::Official,
                "bmclapi" => SourcePreference::Bmclapi,
                "mixed" => SourcePreference::Mixed,
                other => {
                    return Err(Failure::message(format!(
                        "下载源只能是 official、bmclapi 或 mixed，收到「{other}」。"
                    )));
                }
            };
        }
        "concurrency" => {
            settings.download.max_concurrent_downloads =
                parse_range(value, 1, 16, "并发数")? as u16;
        }
        "connections" => {
            settings.download.connections_per_file = parse_range(value, 1, 16, "连接数")? as u16;
        }
        "speed-limit" => {
            settings.speed_limit_kbps = value.parse().map_err(|_| {
                Failure::message(format!("限速必须是整数（KiB/s），收到「{value}」。"))
            })?;
        }
        "game-root" => {
            settings.game_root = if value.is_empty() {
                None
            } else {
                let path = std::path::PathBuf::from(value);
                if !path.is_absolute() {
                    return Err(Failure::message("游戏根目录必须是绝对路径。".to_owned()));
                }
                Some(path)
            };
        }
        "data-dir" => {
            return Err(Failure::message(
                "数据目录不能通过 config 修改；请用全局参数 --data-dir。".to_owned(),
            ));
        }
        other => {
            return Err(Failure::message(format!(
                "未知设置项「{other}」。可用：{}",
                KEYS.iter()
                    .map(|(key, _)| *key)
                    .collect::<Vec<_>>()
                    .join("、")
            )));
        }
    }

    /*
     * One validation call for every write. It rejects a memory value above the
     * machine's limit, a malformed cache path, an out-of-range opacity, and so on —
     * the same rules the GUI enforces, so a value set here cannot break a launch.
     */
    validate_settings(&settings, Some(system_memory_info().maximum_game_mb))
        .map_err(Failure::message)?;
    session.store.save_settings(&settings)?;

    if session.cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({ "key": key, "value": read_key(&settings, key, session) })
            )
            .unwrap_or_default()
        );
        return Ok(());
    }
    let shown = read_key(&settings, key, session).unwrap_or_else(|| value.to_owned());
    println!("{} {key} = {}", palette.green("已设置"), shown);
    Ok(())
}

/// Render one key's current value.
///
/// `session` is needed for `data-dir`, which is not part of `LauncherSettings` at all.
fn read_key(settings: &LauncherSettings, key: &str, session: &Session<'_>) -> Option<String> {
    Some(match key {
        "memory" => format!("{} MB", settings.memory_mb),
        "memory-mode" => match settings.memory_mode {
            MemoryMode::Auto => "auto".to_owned(),
            MemoryMode::Custom => "custom".to_owned(),
        },
        "java" => settings
            .java_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "（自动探测）".to_owned()),
        "jvm-args" => settings.extra_jvm_args.clone(),
        "game-args" => settings.game_args.clone(),
        "pre-launch" => settings.pre_launch_command.clone(),
        "gc" => match settings.garbage_collector {
            GarbageCollector::Auto => "auto",
            GarbageCollector::G1 => "g1",
            GarbageCollector::Serial => "serial",
            GarbageCollector::Parallel => "parallel",
        }
        .to_owned(),
        "fullscreen" => settings.fullscreen.to_string(),
        "source" => match settings.download.source {
            SourcePreference::Official => "official",
            SourcePreference::Bmclapi => "bmclapi",
            SourcePreference::Mixed => "mixed",
        }
        .to_owned(),
        "concurrency" => settings.download.max_concurrent_downloads.to_string(),
        "connections" => settings.download.connections_per_file.to_string(),
        "speed-limit" => {
            if settings.speed_limit_kbps == 0 {
                "不限速".to_owned()
            } else {
                format!("{} KiB/s", settings.speed_limit_kbps)
            }
        }
        "game-root" => settings
            .game_root
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "（默认 ~/.minecraft）".to_owned()),
        "data-dir" => session.context.data_root().display().to_string(),
        _ => return None,
    })
}

fn parse_bool(value: &str) -> Outcome<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        other => Err(Failure::message(format!(
            "需要 true 或 false，收到「{other}」。"
        ))),
    }
}

fn parse_range(value: &str, min: u32, max: u32, label: &str) -> Outcome<u32> {
    let parsed: u32 = value
        .parse()
        .map_err(|_| Failure::message(format!("{label}必须是整数，收到「{value}」。")))?;
    if !(min..=max).contains(&parsed) {
        return Err(Failure::message(format!(
            "{label}必须在 {min} 到 {max} 之间，收到 {parsed}。"
        )));
    }
    Ok(parsed)
}

/// Kept for symmetry with the other modules' imports; `ProcessPriority` is a
/// settings field the CLI deliberately does not expose.
#[allow(dead_code)]
fn unused(_: ProcessPriority) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_key_is_readable() {
        // A key in the table that `read_key` does not handle would print "—"
        // forever, which looks like "not configured" rather than a bug.
        let settings = LauncherSettings::default();
        let root = std::env::temp_dir().join("mfmcl-config-test");
        let context = launcher_runtime::RuntimeContext::new(&root);
        let store = launcher_core::LauncherStore::open(root.join("mfmcl.sqlite")).unwrap();
        let cli = crate::cli::Cli {
            json: false,
            quiet: false,
            colour: "never".into(),
            data_dir: None,
            yes: false,
            command: crate::cli::Command::Help { function: None },
        };
        let sink = crate::sink::TerminalSink::new(crate::sink::SinkOptions::new(
            crate::sink::Verbosity::Quiet,
            Palette::new(false),
            true,
        ));
        let session = super::super::Session {
            cli: &cli,
            sink: &sink,
            context,
            store,
        };
        for (key, _) in KEYS {
            assert!(
                read_key(&settings, key, &session).is_some(),
                "key {key} is listed but not readable"
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn booleans_accept_the_obvious_spellings() {
        for value in ["true", "YES", "on", "1"] {
            assert!(parse_bool(value).unwrap(), "{value} should be true");
        }
        for value in ["false", "no", "Off", "0"] {
            assert!(!parse_bool(value).unwrap(), "{value} should be false");
        }
        assert!(parse_bool("maybe").is_err());
    }

    #[test]
    fn ranges_reject_out_of_bounds_values() {
        assert_eq!(parse_range("4", 1, 16, "并发数").unwrap(), 4);
        assert!(parse_range("0", 1, 16, "并发数").is_err());
        assert!(parse_range("17", 1, 16, "并发数").is_err());
        assert!(parse_range("x", 1, 16, "并发数").is_err());
    }
}
