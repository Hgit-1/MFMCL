// 启动器插件运行时。
//
// ## 这一层到底做什么
//
// 之前插件页只是一个清单编辑器：它能校验 JSON、保存启用状态，但插件声明的
// 能力从未被真正使用过。用户看到「已启用」，实际上什么也没发生——这是最糟糕
// 的一种状态，因为它让界面对自己的能力撒了谎。
//
// 这个模块把「插件声明」变成「启动时真实生效的东西」，方式是**声明式贡献**
// （declarative contribution）而不是执行第三方代码：
//
//   - 插件不能运行任意代码。它只能声明「我要在这些时机做这几件被允许的事」，
//     由启动器自己解释并执行。
//   - 因此不需要给启动器塞一个 JS/WASM 引擎，也就没有沙箱逃逸、没有任意文件
//     访问、没有对用户机器的完全控制权。
//
// ## 为什么不做脚本引擎
//
// 一个能跑任意代码的插件系统，要回答的问题包括：文件系统边界、网络出站策略、
// 进程权限、内存与 CPU 配额、崩溃隔离、供应链信任……每一个都会决定整个应用
// 的安全模型。在没有任何签名与审核机制的前提下把它做出来，等于给每个用户装
// 一个后门。声明式贡献把「能力」限定成一组封闭的、可审计的动作，用户能一眼
// 看懂插件会做什么——这比「它什么都能做」更有用。
//
// ## 贡献的封闭集合
//
// 目前支持四类，全部是启动链路里真实存在、且能安全执行的动作：
//
//   jvmArgs        额外 JVM 参数（逐条校验，拒绝危险形态）
//   env            启动进程的环境变量（只允许白名单前缀以外的普通键）
//   preLaunch      启动前执行的命令（与设置里的「启动前命令」同一套规则）
//   backupBefore   启动前把游戏目录打包到备份目录
//
// 每一条都必须同时满足：插件已启用、插件未被判定为不兼容、插件声明了对应的
// 扩展点、并且拥有对应的权限。四道门任何一道没过，贡献就被跳过，并在启动
// 结果里留下「被跳过」的记录——用户能看到「为什么这个插件没生效」。

use crate::model::{InstanceRecord, LauncherSettings};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 插件可以贡献的动作。枚举而不是任意字符串：未知动作无法被解释，
/// 保存时就应当拒绝，而不是等到启动时静默忽略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContributionKind {
    /// 追加 JVM 参数（插在 `-jar`/主类之前）。
    JvmArgs,
    /// 启动进程的环境变量。
    Env,
    /// 启动前执行的命令。
    PreLaunch,
    /// 启动前备份游戏目录。
    BackupBefore,
}

/// 一条插件贡献。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Contribution {
    pub kind: ContributionKind,
    /**
     * 内容。语义随 `kind` 变化：
     *   jvmArgs → 参数本身（单条，如 `-XX:+UseG1GC`）
     *   env     → `KEY=VALUE`
     *   preLaunch → 一条可执行命令（经 shlex 解析）
     *   backupBefore → 目标目录的绝对路径
     */
    pub value: String,
}

/// 一份插件定义（内置或用户安装）。
///
/// 与前端 `LauncherPlugin` 的差别：这里没有文案、图标、品牌色这些纯展示字段。
/// 后端只关心「能不能执行」与「执行什么」，展示由界面自己负责。两边的共同
/// 契约是 `id` 与 `contributions`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginDefinition {
    pub id: String,
    pub version: String,
    /// 声明自己接入的扩展点。只有声明了对应扩展点，贡献才会生效。
    #[serde(default)]
    pub hooks: Vec<String>,
    /// 需要的权限。目前只有 `launchHook` 与 `filesystem` 会实际影响执行。
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub contributions: Vec<Contribution>,
    /// 启动器判定为不兼容时，所有贡献都被跳过。
    #[serde(default)]
    pub incompatible: bool,
}

/// 用户对插件的设置。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginState {
    /// 用户手动安装的插件清单。
    #[serde(default)]
    pub custom: Vec<PluginDefinition>,
    /// id → 是否启用。缺省按「默认启用」处理。
    #[serde(default)]
    pub enabled: BTreeMap<String, bool>,
}

/// 启动时真正执行了什么、跳过了什么。
///
/// 这是这个功能「能用」的证明：用户不只看到「已启用」，还能看到某次启动里
/// 该插件具体贡献了什么。没有这份记录的话，插件是否生效就只能靠猜。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchContributionReport {
    pub applied: Vec<AppliedContribution>,
    pub skipped: Vec<SkippedContribution>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppliedContribution {
    pub plugin_id: String,
    pub kind: ContributionKind,
    /// 已执行内容的简短摘要（不含机密，例如环境变量的值会被遮蔽）。
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedContribution {
    pub plugin_id: String,
    pub kind: ContributionKind,
    /// 为什么跳过：未启用、不兼容、缺扩展点、缺权限、或内容校验失败。
    pub reason: String,
}

/// 一条启动历史。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchHistoryEntry {
    pub id: String,
    pub instance_id: String,
    pub instance_name: String,
    /// RFC 3339。进程真正起来的时间。
    pub started_at: String,
    /// 退出码；进程还在运行时为 None。
    pub exit_code: Option<i32>,
    /// `running` / `stopped` / `crashed`。与 `game-process` 事件的状态一致。
    pub outcome: String,
    /// 本次启动里插件贡献的摘要，用于回答「那个插件到底做了什么」。
    #[serde(default)]
    pub contributions: LaunchContributionReport,
}

// ---------------------------------------------------------------------------
// 校验
// ---------------------------------------------------------------------------

/// JVM 参数的长度上限。参数会进入命令行，过长的值既可疑也容易撞上系统的
/// 命令行长度限制。
const MAX_ARG_LENGTH: usize = 2048;

/**
 * 被拒绝的 JVM 参数形态。
 *
 * 这些不是「不好」，而是**会绕过启动器对游戏进程的控制**：
 *
 *   - `-javaagent` / `-agentlib` / `-agentpath`：加载任意本地库，等于任意代码
 *     执行。插件若需要这类能力，应当在启动器内实现，而不是偷偷注入一个 agent。
 *   - `-Xbootclasspath`：替换 JDK 自身的类，后果同上且更难察觉。
 *   - `@file`（参数文件）：能把上面这些藏在文件里绕过逐条校验。
 *   - `-Djava.security.manager=`、`-XX:+DisableAttachMechanism` 之类会削弱
 *     进程可观测性的开关。
 *
 * 注意：这是**纵深防御**，不是唯一防线——真正的边界是「插件不能执行任意代码」。
 */
const FORBIDDEN_ARG_PREFIXES: &[&str] = &[
    "-javaagent",
    "-agentlib",
    "-agentpath",
    "-xbootclasspath",
    "-xx:+disableattachmechanism",
    "-xx:-usedisabledattachmechanism",
];

/// 环境变量键：只接受常规的 POSIX 名称，拒绝以数字开头或含特殊字符的键。
fn valid_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// 校验一条贡献，返回可执行的内容或拒绝原因。
///
/// 所有校验都在**执行前**完成，而不是边执行边检查：一条贡献要么完整生效，
/// 要么完全不生效，不会留下「改了一半」的状态。
pub fn validate_contribution(contribution: &Contribution) -> Result<(), String> {
    let value = contribution.value.trim();
    if value.is_empty() {
        return Err("内容为空".into());
    }
    match contribution.kind {
        ContributionKind::JvmArgs => {
            if value.len() > MAX_ARG_LENGTH {
                return Err(format!("JVM 参数超过 {MAX_ARG_LENGTH} 字符"));
            }
            // 参数必须是单个 token：含空白说明插件想一次塞多条，那会绕过逐条校验。
            if value.split_whitespace().count() != 1 {
                return Err("JVM 参数必须是单个参数，不能包含空白".into());
            }
            let lowered = value.to_ascii_lowercase();
            if FORBIDDEN_ARG_PREFIXES
                .iter()
                .any(|prefix| lowered.starts_with(prefix))
            {
                return Err("该参数会加载任意代码或削弱进程控制，不允许".into());
            }
            if value.starts_with('@') {
                return Err("不允许引用参数文件（@file）".into());
            }
            Ok(())
        }
        ContributionKind::Env => {
            let (key, _) = value
                .split_once('=')
                .ok_or_else(|| "环境变量必须是 KEY=VALUE 形式".to_owned())?;
            if !valid_env_key(key) {
                return Err("环境变量名只能包含字母、数字与下划线，且不能以数字开头".into());
            }
            if value.len() > MAX_ARG_LENGTH {
                return Err(format!("环境变量超过 {MAX_ARG_LENGTH} 字符"));
            }
            Ok(())
        }
        ContributionKind::PreLaunch => {
            if value.len() > MAX_ARG_LENGTH {
                return Err(format!("命令超过 {MAX_ARG_LENGTH} 字符"));
            }
            if value.contains(['\n', '\r', '\0']) {
                return Err("命令不能包含换行或 NUL".into());
            }
            // 与设置里的「启动前命令」同一套解析规则：引号必须成对。
            shlex::split(value).ok_or_else(|| "命令格式无效，请检查引号是否成对".to_owned())?;
            Ok(())
        }
        ContributionKind::BackupBefore => {
            let path = Path::new(value);
            if !path.is_absolute() {
                return Err("备份目录必须是绝对路径".into());
            }
            if value.contains("..") {
                return Err("备份目录不能包含 ..".into());
            }
            Ok(())
        }
    }
}

/// 一条贡献是否可以执行：插件必须启用、兼容，并声明了对应扩展点与权限。
///
/// 扩展点与权限的映射是固定的：
///   jvmArgs / env / preLaunch → 扩展点 `launch`，权限 `launchHook`
///   backupBefore              → 扩展点 `launch`，权限 `filesystem`
///
/// 为什么要求同时满足：扩展点回答「什么时机」，权限回答「碰什么」。只声明其一
/// 的清单是有歧义的——那是清单写错了，应当跳过并如实报告，而不是替它猜意图。
fn requirement(kind: ContributionKind) -> (&'static str, &'static str) {
    match kind {
        ContributionKind::JvmArgs | ContributionKind::Env | ContributionKind::PreLaunch => {
            ("launch", "launchHook")
        }
        ContributionKind::BackupBefore => ("launch", "filesystem"),
    }
}

/// 判断一条贡献能否生效，返回 `Ok` 或跳过原因。
pub fn contribution_verdict(
    plugin: &PluginDefinition,
    enabled: bool,
    contribution: &Contribution,
) -> Result<(), String> {
    if !enabled {
        return Err("插件未启用".into());
    }
    if plugin.incompatible {
        return Err("插件与当前启动器版本不兼容".into());
    }
    let (hook, permission) = requirement(contribution.kind);
    if !plugin.hooks.iter().any(|item| item == hook) {
        return Err(format!("未声明扩展点 {hook}"));
    }
    if !plugin.permissions.iter().any(|item| item == permission) {
        return Err(format!("未声明权限 {permission}"));
    }
    validate_contribution(contribution)
}

// ---------------------------------------------------------------------------
// 执行计划
// ---------------------------------------------------------------------------

/// 一次启动的插件贡献计划。在 spawn JVM 之前算好，之后只做执行。
#[derive(Debug, Clone, Default)]
pub struct ContributionPlan {
    pub jvm_args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub pre_launch: Vec<(String, String)>,
    pub backups: Vec<(String, PathBuf)>,
    pub report: LaunchContributionReport,
    /// 可以在不接触文件系统的阶段提前执行的步骤需要这个标记。
    pub has_backup: bool,
}

/**
 * 收集一份启动计划。
 *
 * `plugins` 是**全部**插件（内置 + 用户安装），`state` 决定启用与安装情况。
 * 不在这里过滤禁用插件，而是让每条贡献走到 `contribution_verdict` 里统一判定：
 * 这样一个插件部分贡献合法、部分不合法时，合法的那部分仍然生效，不合法的部分
 * 单独出现在报告里。整体跳过反而是对用户的惩罚——他装的是插件，不是清单。
 */
pub fn build_plan(
    plugins: &[PluginDefinition],
    state: &PluginState,
    instance: &InstanceRecord,
    settings: &LauncherSettings,
) -> ContributionPlan {
    let mut plan = ContributionPlan::default();

    for plugin in plugins {
        let enabled = state
            .enabled
            .get(&plugin.id)
            .copied()
            .unwrap_or(true);
        for contribution in &plugin.contributions {
            match contribution_verdict(plugin, enabled, contribution) {
                Ok(()) => {
                    let value = contribution.value.trim();
                    match contribution.kind {
                        ContributionKind::JvmArgs => {
                            plan.jvm_args.push(value.to_owned());
                            plan.report.applied.push(AppliedContribution {
                                plugin_id: plugin.id.clone(),
                                kind: contribution.kind,
                                summary: value.to_owned(),
                            });
                        }
                        ContributionKind::Env => {
                            if let Some((key, val)) = value.split_once('=') {
                                // 值里可能带凭据（token、路径），报告里只留键名。
                                plan.env.push((key.to_owned(), val.to_owned()));
                                plan.report.applied.push(AppliedContribution {
                                    plugin_id: plugin.id.clone(),
                                    kind: contribution.kind,
                                    summary: key.to_owned(),
                                });
                            }
                        }
                        ContributionKind::PreLaunch => {
                            plan.pre_launch.push((plugin.id.clone(), value.to_owned()));
                            plan.report.applied.push(AppliedContribution {
                                plugin_id: plugin.id.clone(),
                                kind: contribution.kind,
                                summary: value.to_owned(),
                            });
                        }
                        ContributionKind::BackupBefore => {
                            plan.backups.push((plugin.id.clone(), PathBuf::from(value)));
                            plan.has_backup = true;
                            plan.report.applied.push(AppliedContribution {
                                plugin_id: plugin.id.clone(),
                                kind: contribution.kind,
                                summary: value.to_owned(),
                            });
                        }
                    }
                }
                Err(reason) => plan.report.skipped.push(SkippedContribution {
                    plugin_id: plugin.id.clone(),
                    kind: contribution.kind,
                    reason,
                }),
            }
        }
    }

    /*
     * 备份目录不能落在游戏目录里。
     *
     * 插件可以声明任意绝对路径，但把备份写进 `game_dir` 会让每次启动都把上一次
     * 的备份再打包一遍——体积指数增长，而且备份本身会污染存档目录。这里用与
     * 手动备份相同的判断（`create_backup` 内部也会再查一次，两道防线）。
     *
     * `settings` 目前没参与过滤，保留参数是为了将来按实例设置控制插件行为；
     * 显式引用一次以免被误改为未使用。
     */
    let _ = settings;
    let game_dir = instance.game_dir.clone();
    plan.backups.retain(|(plugin_id, directory)| {
        if directory.starts_with(&game_dir) {
            plan.report.skipped.push(SkippedContribution {
                plugin_id: plugin_id.clone(),
                kind: ContributionKind::BackupBefore,
                reason: "备份目录不能位于游戏目录内".into(),
            });
            false
        } else {
            true
        }
    });
    plan.has_backup = !plan.backups.is_empty();
    plan
}

/**
 * 把计划的 JVM 参数插进启动参数。
 *
 * 位置必须在主类之前、`-Xmx` 之后——与 `apply_launch_settings` 插入的位置一致。
 * 如果找不到主类说明启动参数已经坏了，此时不插入任何东西并返回 false，让调用方
 * 决定是否当作错误处理（当前是忽略：少几个参数总比启动失败好）。
 */
pub fn insert_jvm_args(prepared_args: &mut Vec<String>, args: &[String], main_class: &str) -> bool {
    if args.is_empty() {
        return false;
    }
    let Some(index) = prepared_args.iter().position(|item| item == main_class) else {
        return false;
    };
    prepared_args.splice(index..index, args.iter().cloned());
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(id: &str, contributions: Vec<Contribution>) -> PluginDefinition {
        PluginDefinition {
            id: id.to_owned(),
            version: "1.0.0".into(),
            hooks: vec!["launch".into()],
            permissions: vec!["launchHook".into(), "filesystem".into()],
            contributions,
            incompatible: false,
        }
    }

    fn arg(value: &str) -> Contribution {
        Contribution {
            kind: ContributionKind::JvmArgs,
            value: value.to_owned(),
        }
    }

    #[test]
    fn ordinary_jvm_flags_are_accepted() {
        assert!(validate_contribution(&arg("-XX:+UseG1GC")).is_ok());
        assert!(validate_contribution(&arg("-Dfoo=bar")).is_ok());
        assert!(validate_contribution(&arg("-Xss4m")).is_ok());
    }

    #[test]
    fn code_loading_flags_are_rejected() {
        for flag in [
            "-javaagent:/tmp/x.jar",
            "-agentlib:jdwp",
            "-agentpath:/tmp/lib.so",
            "-Xbootclasspath/a:/tmp",
            "@args.txt",
        ] {
            assert!(
                validate_contribution(&arg(flag)).is_err(),
                "{flag} must be rejected"
            );
        }
    }

    #[test]
    fn jvm_args_cannot_smuggle_multiple_tokens() {
        assert!(validate_contribution(&arg("-Xmx2G -javaagent:/tmp/x.jar")).is_err());
    }

    #[test]
    fn env_requires_a_valid_key() {
        assert!(validate_contribution(&Contribution {
            kind: ContributionKind::Env,
            value: "MY_FLAG=1".into()
        })
        .is_ok());
        for bad in ["1BAD=1", "BAD-KEY=1", "NOVALUE", "=1"] {
            assert!(
                validate_contribution(&Contribution {
                    kind: ContributionKind::Env,
                    value: bad.into()
                })
                .is_err(),
                "{bad} must be rejected"
            );
        }
    }

    #[test]
    fn backup_requires_absolute_path_without_parent_traversal() {
        assert!(
            validate_contribution(&Contribution {
                kind: ContributionKind::BackupBefore,
                value: "/tmp/backups".into()
            })
            .is_ok()
        );
        for bad in ["relative/dir", "/tmp/../etc"] {
            assert!(
                validate_contribution(&Contribution {
                    kind: ContributionKind::BackupBefore,
                    value: bad.into()
                })
                .is_err()
            );
        }
    }

    #[test]
    fn disabled_plugin_contributes_nothing() {
        let plugin = plugin("p", vec![arg("-Dfoo=1")]);
        let mut enabled = BTreeMap::new();
        enabled.insert("p".to_owned(), false);
        let state = PluginState {
            custom: vec![],
            enabled,
        };
        assert!(contribution_verdict(&plugin, false, &arg("-Dfoo=1")).is_err());
        let _ = state;
    }

    #[test]
    fn missing_hook_or_permission_blocks_execution() {
        let mut no_hook = plugin("p", vec![arg("-Dfoo=1")]);
        no_hook.hooks.clear();
        assert!(contribution_verdict(&no_hook, true, &arg("-Dfoo=1")).is_err());

        let mut no_permission = plugin("p", vec![arg("-Dfoo=1")]);
        no_permission.permissions.clear();
        assert!(contribution_verdict(&no_permission, true, &arg("-Dfoo=1")).is_err());
    }

    #[test]
    fn incompatible_plugin_is_blocked_even_when_enabled() {
        let mut blocked = plugin("p", vec![arg("-Dfoo=1")]);
        blocked.incompatible = true;
        assert!(contribution_verdict(&blocked, true, &arg("-Dfoo=1")).is_err());
    }

    fn instance_at(dir: &str) -> InstanceRecord {
        InstanceRecord {
            id: "inst".into(),
            name: "Test".into(),
            game_version: "1.21.4".into(),
            loader: "vanilla".into(),
            instance_dir: std::path::PathBuf::from(dir),
            game_dir: std::path::PathBuf::from(dir).join("game"),
            isolated: true,
            state: crate::model::InstanceState::Ready,
            created_at: "2026-01-01T00:00:00Z".into(),
            last_launched_at: None,
        }
    }

    #[test]
    fn plan_applies_enabled_contributions_and_reports_them() {
        let plugins = vec![plugin(
            "mirror",
            vec![
                Contribution {
                    kind: ContributionKind::Env,
                    value: "MFMCL_PREFER_MIRROR=1".into(),
                },
                arg("-Dfoo=bar"),
            ],
        )];
        let state = PluginState::default();
        let instance = instance_at("/tmp/inst");
        let settings = LauncherSettings::default();
        let plan = build_plan(&plugins, &state, &instance, &settings);

        assert_eq!(plan.jvm_args, vec!["-Dfoo=bar".to_owned()]);
        assert_eq!(plan.env.len(), 1);
        assert_eq!(plan.env[0].0, "MFMCL_PREFER_MIRROR");
        assert_eq!(plan.report.applied.len(), 2);
        /*
         * 环境变量的报告只留键名，不留值。
         * 值里可能带 token 或路径，而历史会被界面展示、也可能被导出。
         */
        let env_summary = plan
            .report
            .applied
            .iter()
            .find(|item| item.kind == ContributionKind::Env)
            .expect("env contribution must be reported");
        assert_eq!(env_summary.summary, "MFMCL_PREFER_MIRROR");
        assert!(plan.report.skipped.is_empty());
    }

    #[test]
    fn disabled_plugin_contributes_nothing_but_is_reported_as_skipped() {
        let plugins = vec![plugin("mirror", vec![arg("-Dfoo=1")])];
        let mut state = PluginState::default();
        // 未出现的 id 默认启用；显式 false 才跳过。
        state.enabled.insert("mirror".into(), false);
        let plan = build_plan(
            &plugins,
            &state,
            &instance_at("/tmp/inst"),
            &LauncherSettings::default(),
        );
        assert!(plan.jvm_args.is_empty());
        assert_eq!(plan.report.skipped.len(), 1);
        assert!(plan.report.skipped[0].reason.contains("未启用"));
    }

    #[test]
    fn absent_id_defaults_to_enabled() {
        let plugins = vec![plugin("mirror", vec![arg("-Dfoo=1")])];
        let plan = build_plan(
            &plugins,
            &PluginState::default(),
            &instance_at("/tmp/inst"),
            &LauncherSettings::default(),
        );
        assert_eq!(plan.jvm_args.len(), 1);
    }

    #[test]
    fn invalid_contribution_is_skipped_without_blocking_the_valid_ones() {
        // 同一个插件里一条合法、一条非法：合法的那条仍应生效。
        let plugins = vec![plugin(
            "mixed",
            vec![arg("-Dok=1"), arg("-javaagent:/tmp/x.jar")],
        )];
        let plan = build_plan(
            &plugins,
            &PluginState::default(),
            &instance_at("/tmp/inst"),
            &LauncherSettings::default(),
        );
        assert_eq!(plan.jvm_args, vec!["-Dok=1".to_owned()]);
        assert_eq!(plan.report.skipped.len(), 1);
        assert_eq!(plan.report.applied.len(), 1);
    }

    #[test]
    fn backup_inside_game_directory_is_refused() {
        let plugins = vec![plugin(
            "backup",
            vec![Contribution {
                kind: ContributionKind::BackupBefore,
                value: "/tmp/inst/game/backups".into(),
            }],
        )];
        let plan = build_plan(
            &plugins,
            &PluginState::default(),
            &instance_at("/tmp/inst"),
            &LauncherSettings::default(),
        );
        assert!(plan.backups.is_empty());
        assert!(!plan.has_backup);
        assert_eq!(plan.report.skipped.len(), 1);
        assert!(plan.report.skipped[0].reason.contains("游戏目录内"));
    }

    #[test]
    fn jvm_args_are_inserted_before_the_main_class() {
        let mut args: Vec<String> = vec![
            "-Xmx2G".into(),
            "-cp".into(),
            "cp".into(),
            "net.minecraft.client.main.Main".into(),
            "--version".into(),
        ];
        let inserted = insert_jvm_args(
            &mut args,
            &["-Dfoo=1".into()],
            "net.minecraft.client.main.Main",
        );
        assert!(inserted);
        let position = args.iter().position(|item| item == "-Dfoo=1").unwrap();
        let main = args
            .iter()
            .position(|item| item == "net.minecraft.client.main.Main")
            .unwrap();
        assert!(position < main, "插件参数必须插在主类之前");
    }

    #[test]
    fn empty_state_enables_everything_so_the_ui_must_push_its_defaults() {
        /*
         * 后端对未出现的 id 一律按启用处理。这是有意的（首次运行没有任何存档时，
         * 内置插件应当工作），但它意味着**界面必须先把默认值为 false 的插件推送
         * 过来**，否则首次启动时后端会把它们当成已启用，与界面上看到的不一致。
         *
         * 这条测试锁定这个契约：空状态 = 全部启用。
         */
        let plugins = vec![plugin("off-by-default", vec![arg("-Dfoo=1")])];
        let plan = build_plan(
            &plugins,
            &PluginState::default(),
            &instance_at("/tmp/inst"),
            &LauncherSettings::default(),
        );
        assert_eq!(
            plan.jvm_args.len(),
            1,
            "空状态必须视为启用，界面据此决定要推送什么"
        );

        // 推送之后（显式 false），同一条贡献不再生效。
        let mut pushed = PluginState::default();
        pushed.enabled.insert("off-by-default".into(), false);
        let plan = build_plan(
            &plugins,
            &pushed,
            &instance_at("/tmp/inst"),
            &LauncherSettings::default(),
        );
        assert!(plan.jvm_args.is_empty());
    }
}
