//! The command line surface.
//!
//! ## Conventions worth knowing
//!
//!   - `--instance` takes a **name or id**. Names are what users remember; ids are
//!     what survive a rename. Both resolve, and an ambiguous name lists the
//!     candidates instead of picking one.
//!   - Version churn is expressed the same way everywhere: `latest` resolves to the
//!     newest stable release. A bare value is taken literally, so `--mod-loader
//!     fabric latest` and `--mod-loader fabric 0.16.10` both work.
//!   - Destructive commands require `--yes` when stdin is not a terminal. A script
//!     that would delete an instance must say so explicitly.

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(
    name = "mfmcl",
    // The binary is `mfmcl.exe` on Windows; clap would otherwise print
    // "Usage: mfmcl.exe …" in every help screen, which is not the command people
    // type or the one the docs show.
    bin_name = "mfmcl",
    version,
    // Our own `help` subcommand takes `--function`, which would collide with
    // clap's built-in one. `--help` still works on every command.
    disable_help_subcommand = true,
    about = "MFMCL 命令行启动器：下载、启动、备份与配置 Minecraft Java 版",
    long_about = "MFMCL 命令行启动器。

与图形界面共用同一份数据目录、同一套实例与账户，因此两边的操作互为可见。
所有命令都在终端中完成，不需要图形界面。",
    after_help = "常用例子：\n  \
        mfmcl download --game 1.21.1 --mod-loader neoforge latest --name \"1.21.1_neoforge\"\n  \
        mfmcl launch --instance 1.21.1_neoforge\n  \
        mfmcl backup create --instance 1.21.1_neoforge --to \"D:\\Backups\"\n  \
        mfmcl help --function download\n\n\
        退出码：0 成功；1 失败；2 参数错误；3 用户取消。"
)]
pub struct Cli {
    /// 输出机器可读的 JSON（仅影响 stdout）。
    #[arg(long, global = true)]
    pub json: bool,

    /// 只输出错误与最终结果，不显示进度。
    #[arg(long, short = 'q', global = true, conflicts_with = "json")]
    pub quiet: bool,

    /// 颜色输出：auto、always、never。
    #[arg(long, global = true, default_value = "auto", value_name = "WHEN")]
    pub colour: String,

    /// 数据目录；默认与图形界面相同。
    #[arg(long, global = true, value_name = "DIR")]
    pub data_dir: Option<String>,

    /// 跳过交互确认（等价于对每个提示回答「是」）。
    #[arg(long, short = 'y', global = true)]
    pub yes: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// 创建实例并下载游戏文件与加载器。
    Download(DownloadArgs),
    /// 启动一个已安装的实例。
    Launch(LaunchArgs),
    /// 列出实例、版本、账户、任务或模组。
    List(ListArgs),
    /// 备份与恢复实例的游戏数据。
    Backup(BackupArgs),
    /// 管理内置与自定义插件。
    Plugin(PluginArgs),
    /// 查看与修改启动器设置。
    Config(ConfigArgs),
    /// 账户管理（离线档案、皮肤站登录）。
    Account(AccountArgs),
    /// 诊断实例：版本识别、加载器健康、崩溃分析。
    Doctor(DoctorArgs),
    /// 删除实例（需要确认）。
    Remove(RemoveArgs),
    /// 查看某个命令的详细说明（用途、副作用、失败时的行为）。
    ///
    /// 与 `mfmcl <命令> --help` 互补：那个列出参数，这个解释「为什么用它」。
    Help {
        /// 命令名，例如 `download`。省略时显示总览。
        #[arg(long, value_name = "COMMAND")]
        function: Option<String>,
    },
}

#[derive(Args, Debug)]
pub struct DownloadArgs {
    /// 目标实例名；不存在时自动创建，存在时补全缺失文件。
    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,

    /// 只补全/安装这个已存在的实例（按名称或 id）。
    #[arg(long, value_name = "INSTANCE", conflicts_with = "name")]
    pub instance: Option<String>,

    /// Minecraft 版本，例如 1.21.1。
    #[arg(long, short = 'v', value_name = "VERSION")]
    pub game: String,

    /// 模组加载器：vanilla、fabric、quilt、forge、neoforge。
    ///
    /// 也接受 `loader@version` 的合写形式，例如 `neoforge@21.1.80`。
    #[arg(
        long = "mod-loader",
        visible_alias = "mod-launcher",
        value_name = "LOADER"
    )]
    pub mod_loader: Option<String>,

    /// 加载器版本；`latest` 表示该游戏版本下最新的稳定版。
    #[arg(
        long = "mod-loader-version",
        visible_alias = "mod-launcher-version",
        value_name = "VERSION"
    )]
    pub mod_loader_version: Option<String>,

    /// 加载器版本（可省略 `--mod-loader-version` 直接跟在命令末尾）。
    ///
    /// 存在的理由只有一个：`mfmcl download -v 1.21.1 --mod-loader neoforge latest`
    /// 是最自然的写法，强迫用户改成 `--mod-loader-version latest` 只会制造失败。
    #[arg(value_name = "LOADER_VERSION")]
    pub loader_version_hint: Option<String>,

    /// 安装完成后直接启动。
    #[arg(long)]
    pub launch: bool,

    /// 用于启动的账户名；省略时使用当前选中的账户。
    #[arg(long, value_name = "ACCOUNT")]
    pub account: Option<String>,

    /// 只创建记录，不下载任何文件。
    #[arg(long)]
    pub no_install: bool,

    /// 安装完成后从 Modrinth 安装一个模组，可重复。
    ///
    /// 取值是 Modrinth 的项目 slug 或项目 id（例如 `jei`、`fabric-api`、
    /// `AANobbMI`）。版本按实例的游戏版本与加载器自动选择最新兼容版。
    /// 也可写成 `项目@版本号` 指定版本，例如 `jei@19.21.0.247`。
    #[arg(long = "mod", value_name = "SLUG[@VERSION]", visible_alias = "mods")]
    pub mods: Vec<String>,
}

impl DownloadArgs {
    /// The loader as `(kind, version)`, normalised from every accepted spelling.
    ///
    /// Empty means vanilla. Both `--mod-loader neoforge@21.1` and
    /// `--mod-loader neoforge 21.1` produce the same pair, and `latest` stays
    /// unresolved here because resolving it needs a network call.
    pub fn loader_request(&self) -> Result<Option<(String, String)>, String> {
        let Some(raw) = self
            .mod_loader
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            if self.loader_version_hint.is_some() || self.mod_loader_version.is_some() {
                return Err(
                    "指定了加载器版本但没有指定加载器；请加上 --mod-loader 或去掉版本。".to_owned(),
                );
            }
            return Ok(None);
        };

        // `neoforge@21.1.80` carries its own version, which wins over the flag.
        let (kind, inline_version) = match raw.split_once('@') {
            Some((kind, version)) => (kind.trim(), Some(version.trim().to_owned())),
            None => (raw, None),
        };
        let kind = kind.to_ascii_lowercase();
        if !matches!(
            kind.as_str(),
            "vanilla" | "fabric" | "quilt" | "forge" | "neoforge"
        ) {
            return Err(format!(
                "未识别的加载器 {kind}；可用：vanilla、fabric、quilt、forge、neoforge。"
            ));
        }
        if kind == "vanilla" {
            return Ok(None);
        }

        /*
         * Version precedence: inline `@` first, then the positional, then the flag.
         * Two different explicit versions is a mistake worth reporting rather than
         * silently resolving.
         */
        let positional = self.loader_version_hint.clone();
        let flag = self.mod_loader_version.clone();
        if let (Some(a), Some(b)) = (&positional, &flag)
            && a != b
        {
            return Err(format!(
                "加载器版本出现两次且不一致（{a} 与 {b}）；请只保留一处。"
            ));
        }
        let version = inline_version
            .or(positional)
            .or(flag)
            .unwrap_or_else(|| "latest".to_owned());
        Ok(Some((kind, version)))
    }
}

#[derive(Args, Debug)]
pub struct LaunchArgs {
    /// 实例名或 id。
    #[arg(long, value_name = "INSTANCE", required_unless_present = "last")]
    pub instance: Option<String>,

    /// 启动上一次使用的实例。
    #[arg(long, conflicts_with = "instance")]
    pub last: bool,

    /// 账户名或 id；省略时使用当前选中的账户。
    #[arg(long, value_name = "ACCOUNT")]
    pub account: Option<String>,

    /// Java 可执行文件路径；省略时用设置里的路径或自动探测。
    #[arg(long, value_name = "PATH")]
    pub java: Option<String>,

    /// 立即返回，不等待游戏退出。
    #[arg(long)]
    pub detach: bool,

    /// 不转发游戏日志到终端。
    #[arg(long)]
    pub no_log: bool,

    /// 离线账户启动确认（离线档案每次启动都需要显式确认）。
    #[arg(long)]
    pub offline_ok: bool,
}

#[derive(Args, Debug)]
pub struct ListArgs {
    #[command(subcommand)]
    pub what: ListWhat,
}

#[derive(Subcommand, Debug)]
pub enum ListWhat {
    /// 已创建的实例。
    Instances,
    /// 官方版本清单（需要联网）。
    Versions {
        /// 只显示这么多条。
        #[arg(long, default_value_t = 25)]
        limit: usize,
        /// 包含快照版本。
        #[arg(long)]
        snapshots: bool,
    },
    /// 某个加载器的可用版本（需要联网）。
    Loaders {
        /// fabric、quilt、forge 或 neoforge。
        loader: String,
        /// Minecraft 版本。
        #[arg(long, short = 'v')]
        game: String,
        #[arg(long, default_value_t = 25)]
        limit: usize,
    },
    /// 已保存的账户。
    Accounts,
    /// 下载任务历史。
    Tasks,
    /// 某个实例的模组。
    Mods {
        #[arg(long, value_name = "INSTANCE")]
        instance: String,
    },
    /// 某个 Modrinth 项目可用于该实例的版本（需要联网）。
    ModVersions {
        /// Modrinth 项目 slug 或 id，例如 `jei`。
        project: String,
        /// 实例名或 id；用它决定游戏版本与加载器。
        #[arg(long, value_name = "INSTANCE")]
        instance: String,
        #[arg(long, default_value_t = 25)]
        limit: usize,
    },
    /// 搜索 Modrinth 上的模组（需要联网）。
    Search {
        /// 关键词；省略时按下载量浏览。
        query: Option<String>,
        /// 限定为某个实例的游戏版本与加载器。
        #[arg(long, value_name = "INSTANCE")]
        instance: Option<String>,
        /// 内容类型：mod、resourcepack、shader、datapack、modpack。
        #[arg(long, default_value = "mod", value_name = "TYPE")]
        project_type: String,
        #[arg(long, default_value_t = 15)]
        limit: usize,
    },
}

#[derive(Args, Debug)]
pub struct BackupArgs {
    #[command(subcommand)]
    pub action: BackupAction,
}

#[derive(Subcommand, Debug)]
pub enum BackupAction {
    /// 备份一个实例的游戏数据。
    Create {
        #[arg(long, value_name = "INSTANCE")]
        instance: String,
        /// 备份输出目录（绝对路径）。
        #[arg(long, value_name = "DIR")]
        to: String,
    },
    /// 查看备份内容，不释放任何文件。
    Preview {
        /// 备份文件路径。
        path: String,
    },
    /// 把备份恢复成一个**新**实例（不会覆盖已有实例）。
    Restore {
        /// 备份文件路径。
        path: String,
        /// 新实例名；省略时用备份里的名字加时间后缀。
        #[arg(long, value_name = "NAME")]
        name: Option<String>,
    },
}

#[derive(Args, Debug)]
pub struct PluginArgs {
    #[command(subcommand)]
    pub action: PluginAction,
}

#[derive(Subcommand, Debug)]
pub enum PluginAction {
    /// 列出插件及其启用状态。
    List,
    /// 启用一个插件。
    Enable { id: String },
    /// 停用一个插件。
    Disable { id: String },
    /// 解释某个插件在启动时究竟会做什么。
    Inspect { id: String },
    /// 添加一个自定义插件。
    Add {
        id: String,
        /// 扩展点，可重复：launch。
        #[arg(long = "hook")]
        hooks: Vec<String>,
        /// 权限，可重复：launchHook、network、storage、filesystem。
        #[arg(long = "permission")]
        permissions: Vec<String>,
        /// JVM 参数贡献，可重复。
        ///
        /// 值可以以 `-` 开头（JVM 参数全都这样），所以 `allow_hyphen_values` 是必须
        /// 的：否则 `--jvm-arg "-XX:+UseG1GC"` 会被 clap 当成一个未知的 `-X` 标志，
        /// 而这是添加 JVM 参数唯一自然的写法。
        #[arg(long = "jvm-arg", allow_hyphen_values = true)]
        jvm_args: Vec<String>,
        /// 环境变量贡献，形如 KEY=VALUE，可重复。
        #[arg(long = "env")]
        env: Vec<String>,
        /// 启动前命令贡献，可重复。
        #[arg(long = "pre-launch")]
        pre_launch: Vec<String>,
    },
    /// 删除一个自定义插件（内置插件不能删除）。
    Remove { id: String },
}

#[derive(Args, Debug)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub action: Option<ConfigAction>,
}

#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// 读取一项设置。
    Get { key: String },
    /// 修改一项设置。
    Set { key: String, value: String },
    /// 列出全部可修改的设置及其当前值。
    List,
}

#[derive(Args, Debug)]
pub struct AccountArgs {
    #[command(subcommand)]
    pub action: AccountAction,
}

#[derive(Subcommand, Debug)]
pub enum AccountAction {
    /// 列出账户。
    List,
    /// 添加一个离线档案（不验证正版所有权）。
    AddOffline {
        /// 角色名：3–16 位字母、数字或下划线。
        name: String,
    },
    /// 切换当前选中的账户。
    Select { account: String },
    /// 删除一个账户，并清除系统凭据库中的令牌。
    Remove { account: String },
}

#[derive(Args, Debug)]
pub struct DoctorArgs {
    #[arg(long, value_name = "INSTANCE")]
    pub instance: Option<String>,
}

#[derive(Args, Debug)]
pub struct RemoveArgs {
    /// 实例名或 id。
    #[arg(long, value_name = "INSTANCE")]
    pub instance: String,
    /// 同时删除磁盘上的文件（默认只删除记录）。
    #[arg(long)]
    pub files: bool,
}

/// Source preference for downloads, surfaced on `config set`.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum SourceArg {
    Official,
    Bmclapi,
    Mixed,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_tree_is_internally_consistent() {
        // Catches duplicate flags, conflicting names, and missing derives at test
        // time rather than when a user types `--help`.
        Cli::command().debug_assert();
    }

    #[test]
    fn the_reported_example_in_the_request_parses() {
        /*
         * This is the exact invocation the user asked for, including the
         * `--mod-launcher` spelling and the bare `latest`. It must work without
         * being rewritten, otherwise the documented example is a lie.
         */
        let cli = Cli::try_parse_from([
            "mfmcl",
            "download",
            "-v",
            "1.21.1",
            "--mod-launcher",
            "neoforge",
            "latest",
            "--name",
            "1.21.1_neoforge",
        ])
        .expect("the documented command line must parse");
        let Command::Download(args) = cli.command else {
            panic!("expected download");
        };
        assert_eq!(args.game, "1.21.1");
        assert_eq!(
            args.loader_request().unwrap(),
            Some(("neoforge".to_owned(), "latest".to_owned()))
        );
    }

    #[test]
    fn download_parses_the_canonical_spelling() {
        let cli = Cli::try_parse_from([
            "mfmcl",
            "download",
            "--game",
            "1.21.1",
            "--mod-loader",
            "neoforge",
            "--mod-loader-version",
            "latest",
            "--name",
            "1.21.1_neoforge",
        ])
        .expect("valid command line");
        let Command::Download(args) = cli.command else {
            panic!("expected download");
        };
        assert_eq!(
            args.loader_request().unwrap(),
            Some(("neoforge".to_owned(), "latest".to_owned()))
        );
    }

    #[test]
    fn an_inline_loader_version_is_accepted() {
        let cli = Cli::try_parse_from([
            "mfmcl",
            "download",
            "-v",
            "1.21.1",
            "--mod-loader",
            "neoforge@21.1.80",
        ])
        .unwrap();
        let Command::Download(args) = cli.command else {
            panic!("expected download");
        };
        assert_eq!(
            args.loader_request().unwrap(),
            Some(("neoforge".to_owned(), "21.1.80".to_owned()))
        );
    }

    #[test]
    fn vanilla_needs_no_loader_version() {
        let cli = Cli::try_parse_from([
            "mfmcl",
            "download",
            "-v",
            "1.21.1",
            "--mod-loader",
            "vanilla",
        ])
        .unwrap();
        let Command::Download(args) = cli.command else {
            panic!("expected download");
        };
        assert_eq!(args.loader_request().unwrap(), None);
    }

    #[test]
    fn an_unknown_loader_is_reported_with_the_valid_names() {
        let cli = Cli::try_parse_from([
            "mfmcl",
            "download",
            "-v",
            "1.21.1",
            "--mod-loader",
            "risugami",
        ])
        .unwrap();
        let Command::Download(args) = cli.command else {
            panic!("expected download");
        };
        let error = args.loader_request().unwrap_err();
        assert!(error.contains("neoforge"), "got {error}");
    }

    #[test]
    fn a_loader_version_without_a_loader_is_an_error() {
        // Otherwise the version is silently ignored and the user gets vanilla,
        // which is the opposite of what they asked for.
        let cli = Cli::try_parse_from(["mfmcl", "download", "-v", "1.21.1", "latest"]).unwrap();
        let Command::Download(args) = cli.command else {
            panic!("expected download");
        };
        assert!(args.loader_request().is_err());
    }

    #[test]
    fn two_conflicting_explicit_loader_versions_are_refused() {
        let cli = Cli::try_parse_from([
            "mfmcl",
            "download",
            "-v",
            "1.21.1",
            "--mod-loader",
            "fabric",
            "0.16.10",
            "--mod-loader-version",
            "0.16.9",
        ])
        .unwrap();
        let Command::Download(args) = cli.command else {
            panic!("expected download");
        };
        assert!(args.loader_request().is_err());
    }

    #[test]
    fn launch_requires_a_target() {
        assert!(Cli::try_parse_from(["mfmcl", "launch"]).is_err());
        assert!(Cli::try_parse_from(["mfmcl", "launch", "--last"]).is_ok());
        assert!(
            Cli::try_parse_from(["mfmcl", "launch", "--instance", "x", "--last"]).is_err(),
            "--last and --instance are mutually exclusive"
        );
    }

    #[test]
    fn quiet_and_json_cannot_be_combined() {
        // They express opposite intents; allowing both would force the code to
        // pick a winner silently.
        assert!(Cli::try_parse_from(["mfmcl", "--quiet", "--json", "list", "instances"]).is_err());
    }

    #[test]
    fn download_name_and_instance_are_exclusive() {
        assert!(
            Cli::try_parse_from([
                "mfmcl",
                "download",
                "--game",
                "1.21.1",
                "--name",
                "a",
                "--instance",
                "b",
            ])
            .is_err()
        );
    }

    #[test]
    fn mods_can_be_repeated_and_accept_a_pinned_version() {
        let cli = Cli::try_parse_from([
            "mfmcl",
            "download",
            "-v",
            "1.21.1",
            "--mod-loader",
            "fabric",
            "--mod",
            "jei",
            "--mod",
            "sodium@0.6.5",
        ])
        .unwrap();
        let Command::Download(args) = cli.command else {
            panic!("expected download");
        };
        assert_eq!(args.mods, vec!["jei".to_owned(), "sodium@0.6.5".to_owned()]);
    }

    #[test]
    fn jvm_arguments_may_start_with_a_hyphen() {
        // `--jvm-arg "-XX:+UseG1GC"` is the only natural way to write this; without
        // allow_hyphen_values clap rejects it as an unknown `-X` flag.
        let cli = Cli::try_parse_from([
            "mfmcl",
            "plugin",
            "add",
            "gc-tune",
            "--jvm-arg",
            "-XX:+UseG1GC",
            "--jvm-arg",
            "-Xss4M",
        ])
        .unwrap();
        let Command::Plugin(args) = cli.command else {
            panic!("expected plugin");
        };
        let PluginAction::Add { jvm_args, .. } = args.action else {
            panic!("expected add");
        };
        assert_eq!(
            jvm_args,
            vec!["-XX:+UseG1GC".to_owned(), "-Xss4M".to_owned()]
        );
    }

    #[test]
    fn the_help_subcommand_takes_a_function_name() {
        // Requested spelling: `mfmcl help --function download`.
        let cli = Cli::try_parse_from(["mfmcl", "help", "--function", "download"]).unwrap();
        let Command::Help { function } = cli.command else {
            panic!("expected help");
        };
        assert_eq!(function.as_deref(), Some("download"));
    }
}
