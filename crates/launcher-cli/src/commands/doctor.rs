//! `mfmcl doctor` — answer "why is this instance broken?" without a GUI.
//!
//! Three checks, cheapest first:
//!
//!   1. Does the record match what is on disk? (version detection)
//!   2. Is the loader overlay complete? (library presence)
//!   3. Do the game logs contain a recognisable failure? (diagnostic rules)
//!
//! Nothing here launches the game or downloads anything, so it is safe to run on a
//! broken setup — which is exactly when it is needed.

use crate::cli::DoctorArgs;
use crate::commands::download::terminal_width;
use crate::commands::{Failure, Outcome, Session, resolve_instance};
use crate::output::{Palette, Table, bytes};
use launcher_core::InstanceState;

pub fn run(session: &Session<'_>, args: &DoctorArgs) -> Outcome {
    let palette = Palette::new(session.sink.palette().on);
    let json = session.cli.json;

    let targets = match &args.instance {
        Some(target) => vec![resolve_instance(&session.store, target)?],
        None => session.store.list_instances()?,
    };
    if targets.is_empty() {
        return Err(Failure::message(
            "还没有任何实例可以检查。先用 `mfmcl download` 创建一个。",
        ));
    }

    let mut reports = Vec::new();
    for instance in &targets {
        reports.push(report(session, instance));
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&reports).unwrap_or_default()
        );
        return Ok(());
    }

    for report in &reports {
        print_report(report, &palette, targets.len() > 1);
    }
    Ok(())
}

/// Everything one instance's check produced.
///
/// Kept as a plain struct so `--json` can serialize exactly what the human output
/// shows, rather than the CLI having two different notions of "the result".
#[derive(serde::Serialize)]
struct Report {
    id: String,
    name: String,
    game_version: String,
    loader: String,
    state: String,
    instance_dir: String,
    game_dir: String,
    /// Detected version/loader from disk, which may differ from the record.
    detected: Detected,
    /// Problems found, worst first. Empty means nothing obvious is wrong.
    problems: Vec<Problem>,
    /// Crash analysis, when logs exist and a rule matched.
    crash: Option<Crash>,
}

#[derive(serde::Serialize)]
struct Detected {
    version: Option<String>,
    loader: Option<String>,
    loader_version: Option<String>,
    source: String,
    /// Whether the record and the disk agree.
    consistent: bool,
}

#[derive(serde::Serialize)]
struct Problem {
    severity: String,
    title: String,
    detail: String,
}

#[derive(serde::Serialize)]
struct Crash {
    title: String,
    confidence: String,
    explanation: String,
    suggestions: Vec<String>,
}

fn report(session: &Session<'_>, instance: &launcher_core::InstanceRecord) -> Report {
    let mut problems = Vec::new();

    // 1. Record vs disk.
    let detection = launcher_core::version_detect::detect_instance_version(instance);
    let consistent = detection
        .version
        .as_deref()
        .map(|version| version == instance.game_version)
        .unwrap_or(true);
    if !consistent {
        problems.push(Problem {
            severity: "warning".into(),
            title: "记录与磁盘不一致".into(),
            detail: format!(
                "实例记录是 {}，磁盘上识别到 {}（依据：{}）。\n\
                 如果磁盘内容被手动改过，可以用 `mfmcl list instances` 确认后重建实例。",
                instance.game_version,
                detection.version.clone().unwrap_or_else(|| "未知".into()),
                detection.source
            ),
        });
    }

    // 2. Install state.
    match instance.state {
        InstanceState::Ready => {}
        InstanceState::NotInstalled => problems.push(Problem {
            severity: "error".into(),
            title: "实例尚未安装".into(),
            detail: format!(
                "运行 `mfmcl download --instance {}` 下载缺失文件。",
                instance.name
            ),
        }),
        InstanceState::Installing => problems.push(Problem {
            severity: "warning".into(),
            title: "上次安装未完成".into(),
            detail: "可能有另一个进程正在安装；如果没有，重新运行 download 会接着补全。".into(),
        }),
        InstanceState::Failed => problems.push(Problem {
            severity: "error".into(),
            title: "上次安装失败".into(),
            detail: "重新运行 download 会重试失败的部分。".into(),
        }),
    }

    // 3. Loader overlay health, which the installer repairs automatically but which
    //    is worth showing because "it says fabric@0.16 but the libraries are gone"
    //    explains a launch failure nothing else would.
    if instance.loader != "vanilla" {
        let libraries = session.context.data_root().join("libraries");
        match launcher_core::loaders::diagnose_loader(instance, &libraries) {
            launcher_core::loaders::LoaderHealth::Healthy => {}
            launcher_core::loaders::LoaderHealth::NotInstalled => problems.push(Problem {
                severity: "error".into(),
                title: "加载器未安装".into(),
                detail: format!(
                    "记录里有 {}，但配置不存在。重新运行 download 会安装它。",
                    instance.loader
                ),
            }),
            launcher_core::loaders::LoaderHealth::Broken(reason) => problems.push(Problem {
                severity: "error".into(),
                title: "加载器文件不完整".into(),
                detail: format!("{reason}\n重新运行 download 会重装加载器文件。"),
            }),
        }
    }

    // 4. Java, which is the single most common launch blocker.
    let java = launcher_core::detect_java_runtime(
        session
            .store
            .load_settings()
            .ok()
            .and_then(|settings| settings.java_path),
    );
    match java {
        Ok(runtime) => {
            /*
             * Comparing against the manifest's `javaVersion` is what the launch path
             * does; reporting it here turns "it starts and dies" into "it needs Java
             * 21 and you have 17".
             */
            if let Ok(bytes) = std::fs::read(profile_path(instance))
                && let Ok(metadata) =
                    serde_json::from_slice::<mc_metadata::GameVersionMetadata>(&bytes)
                && let Some(required) = metadata.java_version.map(|value| value.major_version)
                && runtime.major_version < required
            {
                problems.push(Problem {
                    severity: "error".into(),
                    title: "Java 版本过低".into(),
                    detail: format!(
                        "该版本需要 Java {required} 或更高，当前是 Java {}（{}）。\n\
                         用 `mfmcl config set java <路径>` 指定一个更新的 Java。",
                        runtime.major_version, runtime.path
                    ),
                });
            }
        }
        Err(error) => problems.push(Problem {
            severity: "error".into(),
            title: "找不到可用的 Java".into(),
            detail: format!(
                "{error}\n用 `mfmcl config set java <路径>` 指定，或安装一个 Java 运行时。"
            ),
        }),
    }

    // 5. Crash analysis over the existing logs.
    let crash = launcher_core::summarize_instance(instance)
        .ok()
        .map(|summary| Crash {
            title: summary.title,
            confidence: summary.confidence,
            explanation: summary.explanation,
            suggestions: summary
                .reasons
                .into_iter()
                .flat_map(|reason| reason.suggestions)
                .collect(),
        });

    Report {
        id: instance.id.clone(),
        name: instance.name.clone(),
        game_version: instance.game_version.clone(),
        loader: instance.loader.clone(),
        state: format!("{:?}", instance.state),
        instance_dir: instance.instance_dir.to_string_lossy().into_owned(),
        game_dir: instance.game_dir.to_string_lossy().into_owned(),
        detected: Detected {
            version: detection.version,
            loader: detection.loader,
            loader_version: detection.loader_version,
            source: detection.source,
            consistent,
        },
        problems,
        crash,
    }
}

fn profile_path(instance: &launcher_core::InstanceRecord) -> std::path::PathBuf {
    let directory = instance.version_dir();
    if instance.loader == "vanilla" {
        directory.join(format!("{}.json", instance.game_version))
    } else {
        directory.join("launch.json")
    }
}

fn print_report(report: &Report, palette: &Palette, show_header: bool) {
    if show_header {
        println!("{}", palette.bold(&format!("实例：{}", report.name)));
    }
    let mut table = Table::new(&["项目", "值"]);
    table.row(vec!["Minecraft".into(), report.game_version.clone()]);
    table.row(vec!["加载器".into(), report.loader.clone()]);
    table.row(vec!["状态".into(), report.state.clone()]);
    table.row(vec![
        "识别结果".into(),
        format!(
            "{}{}",
            report
                .detected
                .version
                .clone()
                .unwrap_or_else(|| "无法识别".into()),
            if report.detected.consistent {
                ""
            } else {
                "（与记录不一致）"
            }
        ),
    ]);
    if let Some(loader) = &report.detected.loader {
        table.row(vec![
            "磁盘加载器".into(),
            format!(
                "{loader}{}",
                report
                    .detected
                    .loader_version
                    .as_ref()
                    .map(|version| format!("@{version}"))
                    .unwrap_or_default()
            ),
        ]);
    }
    table.row(vec!["目录".into(), report.instance_dir.clone()]);
    print!("{}", table.render(palette, terminal_width()));

    if report.problems.is_empty() {
        println!("\n{}", palette.green("没有发现问题。"));
    } else {
        println!();
        for problem in &report.problems {
            let marker = if problem.severity == "error" {
                palette.red("✗")
            } else {
                palette.yellow("!")
            };
            println!("{marker} {}", palette.bold(&problem.title));
            for line in problem.detail.lines() {
                println!("  {line}");
            }
        }
    }

    if let Some(crash) = &report.crash {
        println!("\n{}", palette.bold("最近的崩溃日志："));
        println!("  {}（置信度 {}）", crash.title, crash.confidence);
        if !crash.explanation.is_empty() {
            println!("  {}", palette.dim(&crash.explanation));
        }
        for suggestion in crash.suggestions.iter().take(4) {
            println!("  · {suggestion}");
        }
    }
    println!();
}

/// Bytes formatter re-exported for symmetry with other commands' imports.
#[allow(dead_code)]
fn unused(bytes_value: u64) -> String {
    bytes(bytes_value)
}
