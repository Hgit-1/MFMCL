//! `mfmcl help` — per-function documentation.
//!
//! ## Why this exists next to clap's own `--help`
//!
//! Clap answers "which flags does this take". It cannot answer "why would I use
//! this, what will it touch, and what happens when it fails" — which is what a
//! user asking for help actually needs. The long text lives here, in data, so it
//! stays in one place and can be printed without touching the network, the data
//! directory, or Java.

use crate::output::{Palette, Table, display_width, truncate};

/// One command's documentation.
struct Doc {
    /// Invocation form, e.g. `download`.
    name: &'static str,
    /// One-line summary.
    summary: &'static str,
    /// What it does, in prose.
    description: &'static str,
    /// Concrete invocations.
    examples: &'static [&'static str],
    /// Things that surprise people.
    notes: &'static [&'static str],
}

const DOCS: &[Doc] = &[
    Doc {
        name: "download",
        summary: "创建实例并下载游戏文件与加载器",
        description: "把 Minecraft 客户端、全部库、资源索引与可选加载器下载到实例目录。\n\
                      文件按官方清单里的 SHA-1 校验，校验失败会尝试下一个源，不会把不匹配的文件落盘。\n\
                      对已存在的实例，这个命令是「补全」：只下载缺失或校验失败的文件，不会重装整个游戏。",
        examples: &[
            "# 原版 1.21.1",
            "mfmcl download -v 1.21.1",
            "# Fabric，自动选最新稳定加载器",
            "mfmcl download -v 1.21.1 --mod-loader fabric latest --name \"1.21.1_fabric\"",
            "# NeoForge，指定具体加载器版本",
            "mfmcl download -v 1.21.1 --mod-loader neoforge@21.1.80 --name \"1.21.1_neoforge\"",
            "# 装完同时装一个模组",
            "mfmcl download -v 1.21.1 --mod-loader fabric latest --mod jei --mod sodium",
            "# 只创建记录，稍后再下载",
            "mfmcl download -v 1.21.1 --name test --no-install",
            "# 装完直接启动",
            "mfmcl download -v 1.21.1 --mod-loader quilt latest --launch",
        ],
        notes: &[
            "加载器必须装在已下载的原版之上；命令会自动按这个顺序执行。",
            "1.20.1 及更早没有 NeoForge，请改用 forge。",
            "NeoForge 的版本号形如 21.1.80，对应 MC 1.21.1，不是 1.21.1-…。",
            "--instance 用于补全已有实例，--name 用于创建新实例，两者不能同时出现。",
            "--mod 只能装进带加载器的实例；原版实例里的模组不会被游戏读取。",
            "--mod 不自动安装前置模组，但会告诉你该版本声明了几个前置。",
        ],
    },
    Doc {
        name: "launch",
        summary: "启动一个已安装的实例",
        description: "读取实例配置、合并实例级设置覆盖、准备 Java 参数并启动 JVM，同时把游戏日志转发到终端。\n\
                      默认等待游戏退出并返回它的退出码，所以可以直接用在脚本的 && 链里。",
        examples: &[
            "mfmcl launch --instance 1.21.1_neoforge",
            "mfmcl launch --last",
            "# 指定账户与 Java",
            "mfmcl launch --instance test --account Steve --java \"C:\\Java\\21\\bin\\java.exe\"",
            "# 启动后立即返回，不转发日志",
            "mfmcl launch --instance test --detach --no-log",
        ],
        notes: &[
            "离线档案不验证正版所有权，每次启动都需要确认；脚本里用 --offline-ok。",
            "--detach 退出后不再转发日志，但游戏进程继续运行。",
            "游戏日志写到实例目录下的 logs/launcher-latest.log，并做令牌脱敏。",
        ],
    },
    Doc {
        name: "list",
        summary: "列出实例、版本、账户、任务或模组",
        description: "所有「看看现在有什么」的查询都在这里。加 --json 输出机器可读结果，\n\
                      格式与图形界面使用的是同一套结构。",
        examples: &[
            "mfmcl list instances",
            "mfmcl list versions --limit 10",
            "mfmcl list loaders neoforge -v 1.21.1",
            "mfmcl list mods --instance 1.21.1_neoforge",
            "mfmcl list search jei --instance 1.21.1_fabric",
            "mfmcl list mod-versions jei --instance 1.21.1_fabric",
            "mfmcl list tasks --json",
        ],
        notes: &[
            "list versions、list loaders、list search 与 list mod-versions 需要联网。",
            "list search 不带 --instance 时搜索全部版本与加载器，容易选错；加上它只会显示兼容当前实例的结果。",
            "list mod-versions 给出可安装的具体版本，配合 download --mod 项目@版本 固定版本。",
            "--json 只影响 stdout，进度与提示仍在 stderr。",
        ],
    },
    Doc {
        name: "backup",
        summary: "备份与恢复实例的游戏数据",
        description: "备份包含存档、模组、配置等游戏数据，写入一个带索引与逐文件 SHA-256 的归档。\n\
                      恢复always 创建新实例，绝不覆盖已有实例。",
        examples: &[
            "mfmcl backup create --instance 1.21.1_fabric --to \"D:\\Backups\"",
            "mfmcl backup preview \"D:\\Backups\\MFMCL-backup-20261107-050311-ab12.mfmclbackup\"",
            "mfmcl backup restore \"D:\\Backups\\...mfmclbackup\" --name restored",
        ],
        notes: &[
            "备份目录必须是已存在的绝对路径，且不能位于游戏目录内部。",
            "restore 会用备份里的版本创建新实例，不能恢复成另一个 MC 版本。",
            "preview 显示的 SHA-256 会在 restore 时被复核，文件被换过会拒绝恢复。",
        ],
    },
    Doc {
        name: "plugin",
        summary: "管理内置与自定义插件",
        description: "列出插件、启用或停用、解释某个插件在启动时实际会做什么，\n\
                      以及添加只做声明式贡献的自定义插件。",
        examples: &[
            "mfmcl plugin list",
            "mfmcl plugin disable launch-logger",
            "mfmcl plugin inspect launch-logger",
            "mfmcl plugin add gc-tune --hook launch --permission launchHook --jvm-arg \"-XX:+UseG1GC\"",
        ],
        notes: &[
            "声明 JVM 参数、环境变量或启动前命令需要 launchHook 权限，否则贡献会在启动时被跳过。",
            "inspect 会列出被跳过的贡献及原因，这是「启用了却没效果」的答案。",
            "内置插件不能删除，只能停用；自定义插件可以 remove。",
        ],
    },
    Doc {
        name: "config",
        summary: "查看与修改启动器设置",
        description: "用一个固定的键集合读写设置，每个键都有类型与范围校验。\n\
                      写入前会跑一遍与图形界面相同的设置校验，所以不会存进一个启动时才炸的值。",
        examples: &[
            "mfmcl config list",
            "mfmcl config get memory",
            "mfmcl config set memory 4096",
            "mfmcl config set source bmclapi",
            "mfmcl config set java \"C:\\Java\\21\\bin\\java.exe\"",
        ],
        notes: &[
            "内存上限由本机可用内存决定，超过会被拒绝。",
            "concurrency 与 connections 的范围都是 1–16。",
            "data-dir 是只读项，只能通过全局参数 --data-dir 改变。",
        ],
    },
    Doc {
        name: "account",
        summary: "账户管理：离线档案、切换与删除",
        description: "添加离线档案、切换当前账户、删除账户并清除系统凭据库中的令牌。\n\
                      交互式微软登录尚未在命令行提供，目前请在图形界面完成。",
        examples: &[
            "mfmcl account add-offline Steve",
            "mfmcl account list",
            "mfmcl account select Steve",
            "mfmcl account remove Steve",
        ],
        notes: &[
            "离线角色名需为 3–16 位字母、数字或下划线。",
            "删除账户会同时删除它在系统凭据库里的条目；清不掉时不会删除记录。",
        ],
    },
    Doc {
        name: "doctor",
        summary: "诊断实例：版本识别、加载器健康、崩溃分析",
        description: "依次检查记录与磁盘是否一致、加载器文件是否完整、Java 版本是否满足要求，\n\
                      并对已有日志做崩溃原因分析。不做任何下载或启动，因此可以在坏掉的实例上安全运行。",
        examples: &[
            "mfmcl doctor",
            "mfmcl doctor --instance 1.21.1_neoforge",
            "mfmcl doctor --json",
        ],
        notes: &[
            "不带 --instance 时检查全部实例。",
            "退出码仍为 0：doctor 报告问题本身是成功的诊断，不是命令失败。",
        ],
    },
    Doc {
        name: "remove",
        summary: "删除实例（需要确认）",
        description: "删除实例记录；加 --files 同时删除磁盘文件。\n\
                      文件删除被限制在启动器的实例目录内，记录里的路径被改过也不会误删别处。",
        examples: &[
            "mfmcl remove --instance old_test",
            "mfmcl remove --instance old_test --files",
        ],
        notes: &[
            "不加 --files 时磁盘文件保留，可以用 --instance 重新接管。",
            "删除文件包含存档，无法撤销。",
            "非交互环境下需要 --yes，否则命令会拒绝执行。",
        ],
    },
];

/// Print the overview or one command's documentation.
pub fn run(function: Option<&str>, palette: &Palette) {
    match function {
        None => overview(palette),
        Some(name) => {
            let normalised = name.trim().to_ascii_lowercase();
            match DOCS.iter().find(|doc| doc.name == normalised) {
                Some(doc) => detail(doc, palette),
                None => {
                    eprintln!("没有名为「{name}」的命令。");
                    eprintln!();
                    overview(palette);
                    eprintln!();
                    eprintln!("{}", palette.dim("用 `mfmcl --help` 查看全部参数。"));
                }
            }
        }
    }
}

fn overview(palette: &Palette) {
    println!("{}", palette.bold("MFMCL 命令行启动器"));
    println!();
    println!("下载、启动、备份与配置 Minecraft Java 版实例，不需要图形界面。");
    println!("与图形界面共用同一份数据目录，因此两边的实例与账户互为可见。");
    println!();
    let mut table = Table::new(&["命令", "用途"]);
    for doc in DOCS {
        table.row(vec![doc.name.to_owned(), doc.summary.to_owned()]);
    }
    table.row(vec![
        "help".into(),
        "显示这份说明，或某个命令的详细说明".into(),
    ]);
    print!("{}", table.render(palette, terminal_width()));
    println!();
    println!("{}", palette.bold("全局参数"));
    let mut flags = Table::new(&["参数", "说明"]);
    flags.row(vec!["--json".into(), "机器可读输出（仅 stdout）".into()]);
    flags.row(vec!["-q, --quiet".into(), "只输出错误与结果".into()]);
    flags.row(vec!["--colour WHEN".into(), "auto、always、never".into()]);
    flags.row(vec!["--data-dir DIR".into(), "覆盖数据目录".into()]);
    flags.row(vec!["-y, --yes".into(), "跳过交互确认".into()]);
    print!("{}", flags.render(palette, terminal_width()));
    println!();
    println!("{}", palette.bold("详细说明"));
    println!("  mfmcl help --function <命令>      例如 mfmcl help --function download");
    println!("  mfmcl <命令> --help               查看该命令的全部参数");
    println!();
    println!("{}", palette.bold("退出码"));
    println!("  0  成功      1  失败      2  参数错误      3  用户取消");
}

fn detail(doc: &Doc, palette: &Palette) {
    println!("{}", palette.bold(&format!("mfmcl {}", doc.name)));
    println!("{}", doc.summary);
    println!();
    for line in doc.description.lines() {
        println!("{line}");
    }
    if !doc.examples.is_empty() {
        println!();
        println!("{}", palette.bold("示例"));
        for example in doc.examples {
            if example.starts_with('#') {
                println!("  {}", palette.dim(example));
            } else {
                println!("  {example}");
            }
        }
    }
    if !doc.notes.is_empty() {
        println!();
        println!("{}", palette.bold("注意"));
        for note in doc.notes {
            println!("  · {note}");
        }
    }
    println!();
    println!(
        "{}",
        palette.dim(&format!("完整参数：mfmcl {} --help", doc.name))
    );
}

/// Columns to draw into, mirroring the download module's rule.
fn terminal_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|width| *width >= 20)
        .unwrap_or(100)
}

/// Names that `help --function` accepts, for tab completion and tests.
#[allow(dead_code)]
pub fn function_names() -> impl Iterator<Item = &'static str> {
    DOCS.iter().map(|doc| doc.name)
}

/// Keep the helpers referenced; they exist for tests that check the docs fit.
#[allow(dead_code)]
fn widest_example() -> usize {
    DOCS.iter()
        .flat_map(|doc| doc.examples.iter())
        .map(|example| display_width(&truncate(example, 200)))
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_cli_command_has_a_doc_entry() {
        // `help --function X` must work for every command the parser accepts,
        // otherwise the most discoverable place to look answers "no such command".
        for name in [
            "download", "launch", "list", "backup", "plugin", "config", "account", "doctor",
            "remove",
        ] {
            assert!(
                DOCS.iter().any(|doc| doc.name == name),
                "{name} has no documentation entry"
            );
        }
    }

    #[test]
    fn docs_are_not_empty() {
        for doc in DOCS {
            assert!(
                !doc.summary.trim().is_empty(),
                "{} has no summary",
                doc.name
            );
            assert!(
                !doc.description.trim().is_empty(),
                "{} has no description",
                doc.name
            );
            assert!(
                !doc.examples.is_empty(),
                "{} has no examples; documentation without an example is hard to use",
                doc.name
            );
        }
    }

    #[test]
    fn examples_look_like_invocations_of_their_command() {
        for doc in DOCS {
            for example in doc.examples {
                if example.starts_with('#') {
                    continue;
                }
                assert!(
                    example.starts_with(&format!("mfmcl {}", doc.name)),
                    "{} has an example for another command: {example}",
                    doc.name
                );
            }
        }
    }
}
