//! Crash analysis for MFMCL: deterministic rules plus a local LFM3 model.
//!
//! Split out of `launcher-core` so the analysis service can be built and shipped
//! on its own (`cargo run -p crash-ai --bin crash-ai-server`), while the launcher
//! links the same library and therefore behaves identically.

pub mod causes;
pub mod hardware;
pub mod llm;
pub mod rules;

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Ceiling on how much of a log we read. Real logs reach tens of megabytes when
/// a mod spams a line per tick; the crash is at the end.
pub const MAX_LOG_BYTES: usize = 12 * 1024 * 1024;

/// Characters of excerpt handed to the model. About 1500 tokens — enough for the
/// exception plus its stack head, small enough to leave an 8k window room to
/// answer.
pub const MAX_CONTEXT_CHARS: usize = 6000;

/// Confidence in a finding.
///
/// Ordered so the strongest can be picked without string comparison. `Guess` is
/// what model-only speculation is capped at, which keeps it visibly weaker than
/// any rule match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Guess,
    Low,
    Medium,
    High,
}

impl Confidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Guess => "guess",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

/// One cause, either matched by a rule or suggested by the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    /// Rule id, or `model.suggestion` for model-only output. Tests assert on
    /// this rather than on translated prose.
    pub code: String,
    pub title: String,
    pub confidence: Confidence,
    pub explanation: String,
    pub suggestions: Vec<String>,
    /// Log lines that triggered the rule. Empty for model-only output.
    pub evidence: Vec<String>,
    /// Mods named in the evidence, if any were extracted.
    pub mods: Vec<String>,
}

/// Facts the launcher knows and a pasted log does not.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Environment {
    #[serde(default)]
    pub java_version: Option<String>,
    #[serde(default)]
    pub java_bits: Option<u32>,
    #[serde(default)]
    pub memory_mb: Option<u32>,
    #[serde(default)]
    pub game_version: Option<String>,
    #[serde(default)]
    pub loader: Option<String>,
    #[serde(default)]
    pub mod_count: Option<usize>,
    /*
     * The two fields below are filled by the caller when it can — the launcher
     * knows both, a pasted log does not. They are what make the hardware verdict
     * possible instead of guessed.
     */
    #[serde(default)]
    pub hardware: Option<hardware::Hardware>,
    /// Extra files the caller chose to attach (latest.log, a mod list, ...).
    /// Content only: the analyser never opens paths.
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}

/// One extra file supplied by the caller.
///
/// Why content rather than a path: the launcher may read the instance directory,
/// but a page receiving a pasted log must not be able to make the analyser read
/// arbitrary files on the machine. Keeping this as content means "read this
/// folder" is a caller-side decision with its own permission story.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    /// A short label shown in the UI, e.g. `latest.log` or `mods 列表`.
    pub name: String,
    pub content: String,
}

/// What the extractor pulled out of the log, before any model runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogFacts {
    pub total_lines: usize,
    #[serde(default)]
    pub exception: Option<String>,
    #[serde(default)]
    pub involved_mods: Vec<String>,
    /// The excerpt handed to the model.
    pub context: String,
    #[serde(default)]
    pub detected_java: Option<String>,
    #[serde(default)]
    pub detected_game_version: Option<String>,
}

/// Where the recommended steps came from.
///
/// The UI shows this, because "the rules say raise your heap" and "a 1.7B model
/// says lower your heap" must never look the same. The second one was observed
/// in practice and would have made the crash certain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepSource {
    /// Curated suggestions from matched rules.
    Rules,
    /// The model's own advice, used only when no rule matched.
    Model,
}

impl StepSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rules => "rules",
            Self::Model => "model",
        }
    }
}

/// The model's contribution. Never presented as a finding on its own.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Narrative {
    pub explanation: String,
    pub steps: Vec<String>,
    /// Who wrote `steps`. Always `Rules` when any rule matched.
    pub source: StepSource,
    /// The model's confidence in its own explanation, capped at `Medium`.
    pub confidence: Confidence,
    pub model: String,
}

/// 全部分析结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Analysis {
    /// 最强的一条在前。空表示没有命中。
    pub findings: Vec<Finding>,
    /// 堆栈因果分析。这是「是什么问题」的权威结论；`findings` 是它的展开说明。
    ///
    /// 为什么两者并存：`Cause` 是从堆栈本身读出来的（抛出的异常、最近的真实帧），
    /// 而 `Finding` 带着建议与可读解释。界面上先给 `Cause` 的一句话结论，再展开
    /// `Finding`。把两者合成一个结构会让「结论来自堆栈」和「结论来自关键词」
    /// 混在一起，而这两者的可信度完全不同。
    #[serde(default)]
    pub cause: Option<causes::Cause>,
    /// 硬件是否撑得住这个整合包。
    #[serde(default)]
    pub capability: Option<hardware::Capability>,
    #[serde(default)]
    pub narrative: Option<Narrative>,
    pub facts: LogFacts,
    pub model_used: bool,
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// 跑规则层：以堆栈因果分析为准，关键词规则只作为补充。
///
/// ## 为什么不再让两套独立判断并存
///
/// 真实崩溃报告暴露了这个问题的严重性。`crash-2026-08-20_13.43.14-client.txt`
/// 的真实原因是 inotify 实例上限，但堆栈上每一帧都带着 Forge 的 `{re:mixin}`
/// 标记，于是关键词规则报出「Mixin 注入失败」，而因果分析报出「系统资源上限」。
/// 界面上同时显示这两条，等于让用户自己判断该信哪个——而其中一个是错的。
///
/// 现在的规则是：
///
///   1. 因果分析给出确定的类别（非 Unknown）→ 只保留与之**相关**的关键词规则。
///      例如类别是「系统资源上限」时，任何模组类关键词都被丢弃。
///   2. 因果分析是 Unknown → 关键词规则作为兜底全部保留。它们是基于文本的启发式，
///      在没有明确异常时仍然有用。
///
/// 「相关」的映射不是按字面相似，而是按**修复动作是否相同**：一个 inotify 问题
/// 和 mixin 问题需要完全不同的处理，所以它们不相关；而「内存不足」与「堆参数无效」
/// 都指向内存设置，可以并存。
pub fn analyse_log(log: &str, environment: &Environment) -> (Vec<Finding>, LogFacts, Vec<String>) {
    let facts = extract_facts_only(log, environment);
    let warnings = facts.1;

    let cause = causes::analyse_trace(log);
    let raw = rules::evaluate(&facts.2);

    let findings = match &cause {
        /*
         * Forge 的 `-- MOD xxx --` 块是最强证据：它逐条列出了哪个模组缺什么。
         *
         * 这种时候**不能**再保留关键词规则。真实报告里，同一份日志的堆栈也带着
         * `{re:mixin}`（Forge 的改写标记），于是关键词规则报「Mixin 注入失败」，
         * 而真相是 27 个模组缺 tconstruct 前置。让两条结论并存只会误导。
         */
        Some(found) if !found.mod_failures.is_empty() => {
            vec![finding_from_cause(found)]
        }
        Some(found) if found.kind != causes::CauseKind::Unknown => {
            let keep = rules::related_to(found.kind);
            let mut filtered: Vec<Finding> = raw
                .into_iter()
                .filter(|finding| keep.contains(&finding.code.as_str()))
                .collect();
            if filtered.is_empty() {
                /*
                 * 因果分析确定了类别，但关键词规则表里没有对应的可读条目
                 * （系统上限、本地库缺失这两类都是这种情况）。
                 *
                 * 这里不能留空：`findings` 是界面上「是什么问题 + 该怎么办」的
                 * 载体，空数组会让用户看到一个结论框却没有任何可做的动作。
                 * 所以按因果类别合成一条。
                 */
                filtered.push(finding_from_cause(found));
            }
            filtered
        }
        _ => raw,
    };

    (findings, facts.0, warnings)
}

/// 抽出事实与其警告，供上面按需组合。
fn extract_facts_only(log: &str, environment: &Environment) -> (LogFacts, Vec<String>, String) {
    let mut warnings = Vec::new();
    let truncated = log.len() > MAX_LOG_BYTES;
    let body = if truncated {
        warnings.push(format!(
            "日志超过 {} MB，只分析了末尾部分。",
            MAX_LOG_BYTES / 1024 / 1024
        ));
        let start = log.len() - MAX_LOG_BYTES;
        match log[start..].find('\n') {
            Some(offset) => &log[start + offset + 1..],
            None => &log[start..],
        }
    } else {
        log
    };
    let findings = rules::evaluate(body);
    let facts = extract_facts(body, &findings, environment);
    (facts, warnings, body.to_owned())
}

/// Pull the interesting lines out of a log.
///
/// Narrow on purpose: the primary exception with its stack head, lines naming a
/// mod, and — when nothing is recognised — the tail, because the JVM's own
/// complaint is usually the last thing printed.
fn extract_facts(log: &str, findings: &[Finding], environment: &Environment) -> LogFacts {
    let lines: Vec<&str> = log.lines().collect();
    let total_lines = lines.len();

    let mut exception = None;
    let mut detected_java = None;
    let mut detected_game_version = None;
    let mut mods: BTreeSet<String> = BTreeSet::new();
    let mut picked: BTreeSet<usize> = BTreeSet::new();

    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if exception.is_none() && rules::is_primary_exception(trimmed) {
            exception = Some(trimmed.to_owned());
            // The following frames name the responsible classes; 25 lines is
            // enough for the top of a stack without dragging in whole traces.
            for offset in 0..25 {
                if index + offset < lines.len() {
                    picked.insert(index + offset);
                }
            }
        }
        if detected_java.is_none() {
            detected_java = rules::java_version_from_line(trimmed);
        }
        if detected_game_version.is_none() {
            detected_game_version = rules::game_version_from_line(trimmed);
        }
        for name in rules::mods_in_line(trimmed) {
            mods.insert(name);
        }
    }

    /*
     * Mods named by a rule outrank ones merely mentioned: a rule only extracts a
     * name when the surrounding text indicates it is the culprit.
     */
    let mut ranked: Vec<String> = findings.iter().flat_map(|finding| finding.mods.clone()).collect();
    for name in mods.iter() {
        if !ranked.contains(name) {
            ranked.push(name.clone());
        }
    }

    let mut context = String::new();
    for index in &picked {
        if let Some(line) = lines.get(*index) {
            context.push_str(line);
            context.push('\n');
        }
        if context.len() >= MAX_CONTEXT_CHARS {
            break;
        }
    }
    if context.trim().is_empty() {
        let tail: Vec<&str> = lines
            .iter()
            .rev()
            .take(80)
            .copied()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        context = tail.join("\n");
    }
    if context.len() > MAX_CONTEXT_CHARS {
        // Keep the head: the excerpt begins at the exception.
        let cut = context.char_indices().nth(MAX_CONTEXT_CHARS).map(|(i, _)| i);
        context.truncate(cut.unwrap_or(context.len()));
        context.push_str("\n…（日志片段已截断）");
    }

    let _ = environment;
    LogFacts {
        total_lines,
        exception,
        involved_mods: ranked.into_iter().take(8).collect(),
        context,
        detected_java,
        detected_game_version,
    }
}

/// 跑完整流程：因果分析 → 规则 → 硬件 →（可选）模型。
///
/// 顺序是有意的。堆栈因果分析先跑，因为它读的是「实际抛出的异常」，比关键词
/// 匹配可靠得多；规则在它之后作为补充（同一个问题可能有多条建议），硬件判断
/// 放在最后，因为它回答的是另一个问题——「是不是机器本身不行」。
pub async fn analyse(
    log: &str,
    environment: &Environment,
    config: &llm::LlmConfig,
    use_model: bool,
) -> Analysis {
    let (findings, facts, mut warnings) = analyse_log(log, environment);

    /*
     * 堆栈因果分析。
     *
     * 喂给它的是完整日志而不是抽出的片段：`analyse_trace` 只看异常行与 `at `
     * 帧，多余的行会被忽略。这样即使抽取器把 `Caused by:` 链截在中间，也不会
     * 丢失更深的那一层——而更深的那一屋往往才是真正的原因。
     */
    let cause = causes::analyse_trace(log);
    if let Some(found) = &cause
        && found.kind == causes::CauseKind::Unknown
    {
        // 未知类别不往界面上放，否则会显示一个「已分析但什么也没看出来」的框。
        warnings.push("堆栈里没有可归类的异常模式".to_owned());
    }

    /*
     * 硬件判断。
     *
     * 只在调用方给了硬件信息时做。浏览器里粘贴日志的用户没有这个数据，此时
     * 返回 `None` 而不是「判定为够用」——后者是一句没有依据的承诺。
     */
    let capability = environment.hardware.as_ref().map(|hardware| {
        let workload = hardware::Workload {
            game_version: environment
                .game_version
                .clone()
                .or_else(|| facts.detected_game_version.clone()),
            loader: environment.loader.clone(),
            mod_count: environment.mod_count,
            allocated_memory_mb: environment.memory_mb,
        };
        hardware::assess(hardware, &workload)
    });

    let mut narrative = None;
    let mut model_used = false;
    if use_model {
        /*
         * 把因果结论交给模型，而不是只给规则结论。
         *
         * 这是让 1.7B 模型有用的关键：它不需要自己诊断（它做不好），只需要
         * 解释一个已经确定的原因，并读日志里的细节来支撑。
         */
        match llm::narrate(config, &facts, &findings, cause.as_ref(), environment).await {
            Ok(result) => {
                narrative = Some(result);
                model_used = true;
            }
            Err(error) => warnings.push(format!("{error}")),
        }
    } else {
        warnings.push("已跳过模型分析（按请求）".to_owned());
    }

    Analysis {
        findings,
        cause,
        capability,
        narrative,
        facts,
        model_used,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment() -> Environment {
        Environment::default()
    }

    #[test]
    fn facts_capture_the_exception_and_its_stack() {
        let log = "\
[12:00:00] [main/INFO]: Loading 34 mods
java.lang.OutOfMemoryError: Java heap space
\tat net.minecraft.client.Minecraft.<init>(Minecraft.java:500)
\tat net.fabricmc.loader.impl.launch.knot.Knot.init(Knot.java:160)
";
        let (_findings, facts, _warnings) = analyse_log(log, &environment());
        assert!(facts.exception.as_deref().unwrap_or("").contains("OutOfMemoryError"));
        assert!(facts.context.contains("Minecraft.java:500"));
    }

    #[test]
    fn environment_is_backfilled_from_the_log() {
        let log = "openjdk version \"17.0.9\" 2023-10-17\nMinecraft Version: 1.20.1\n";
        let (_findings, facts, _) = analyse_log(log, &environment());
        assert_eq!(facts.detected_java.as_deref(), Some("17.0.9"));
        assert_eq!(facts.detected_game_version.as_deref(), Some("1.20.1"));
    }

    #[test]
    fn log_without_a_recognised_exception_falls_back_to_the_tail() {
        let log = "some noise\nanother line\nthe actual last words\n";
        let (_findings, facts, _) = analyse_log(log, &environment());
        assert!(facts.context.contains("the actual last words"));
    }

    #[test]
    fn excerpt_is_bounded() {
        let mut log = String::new();
        for index in 0..20_000 {
            log.push_str(&format!("line {index} filler filler filler\n"));
        }
        log.push_str("java.lang.OutOfMemoryError: Java heap space\n");
        let (_findings, facts, _) = analyse_log(&log, &environment());
        assert!(
            facts.context.chars().count() <= MAX_CONTEXT_CHARS + 32,
            "excerpt must stay near the cap, got {}",
            facts.context.chars().count()
        );
    }

    #[tokio::test]
    async fn analysis_without_the_model_still_returns_findings() {
        let log = "java.lang.OutOfMemoryError: Java heap space\n";
        let result = analyse(log, &environment(), &llm::LlmConfig::default(), false).await;
        assert!(result.model_used == false);
        assert_eq!(result.findings[0].code, "runtime.outofmemory");
        // The skipped-model notice must be visible, not silent.
        assert!(result.warnings.iter().any(|warning| warning.contains("跳过")));
    }
}

/// 按因果类别合成一条可读的 `Finding`。
///
/// 存在的理由：有些类别（系统资源上限、本地库缺失、缺少图形会话）在关键词规则表
/// 里没有对应条目——因为它们不是「日志里出现了某个词」能识别的，而是堆栈语义。
/// 但这些恰恰是最不该让用户看到空白的情况，所以在这里补上结论与建议。
pub fn finding_from_cause(cause: &causes::Cause) -> Finding {
    use causes::CauseKind as K;
    let (code, title, suggestions): (&str, &str, Vec<&str>) = match cause.kind {
        K::SystemLimit => (
            "system.inotify",
            "系统资源上限（文件监听/句柄数）",
            vec![
                "这是操作系统的限制，不是游戏或模组的问题",
                "临时提高上限：sudo sysctl fs.inotify.max_user_instances=1024 与 fs.inotify.max_user_watches=524288",
                "如果模组里有大量配置文件监视器（Forge 常见），减少常驻后台的程序也会缓解",
                "重启后生效；想永久保存需写入 /etc/sysctl.d/",
            ],
        ),
        K::NativeLibrary => (
            "native.library",
            "缺少系统依赖库",
            vec![
                "这不是模组文件损坏，是系统里缺了某个 .so/.dll",
                "按报错里的库名安装对应的系统包（例如 libcef、libGL、libgbm）",
                "在 Linux 上可用 `apt-file search <库名>` 找到提供该文件的软件包",
            ],
        ),
        K::NoDisplay => (
            "env.display",
            "没有可用的图形会话",
            vec!["在正常的桌面环境中启动游戏，而不是通过 SSH 或无显示的容器"],
        ),
        K::OutOfMemory => (
            "runtime.outofmemory",
            "内存不足",
            vec![
                "在实例设置里提高内存分配（按模组数量可加 1–2 GB）",
                "减少同时启用的模组，或降低视距与材质分辨率",
            ],
        ),
        K::Graphics => (
            "gpu.unclassified",
            "图形栈问题",
            vec!["更新显卡驱动", "确认当前会话有可用的显示与硬件加速"],
        ),
        K::ExternalKill => (
            "runtime.killed",
            "进程被外部终止",
            vec!["降低内存分配或关闭其他大内存程序后重试", "检查系统日志里是否有 OOM killer 记录"],
        ),
        K::FileAccess => (
            "files.access.denied",
            "文件访问被拒绝",
            vec!["关闭可能占用这些文件的程序（云同步、杀毒软件、另一个启动器）"],
        ),
        K::Mods => (
            "mods.dependency",
            "模组依赖缺失或版本不符",
            vec![
                "按下面列出的模组逐个检查：它们缺少前置模组或要求更高的版本",
                "安装或升级对应的前置模组（例如 Tinkers' Construct）",
                "确认前置模组的游戏版本与本整合包一致",
            ],
        ),
        K::JvmArguments => (
            "heap.reserve",
            "JVM 无法按要求分配内存",
            vec![
                "把最大内存调低到物理内存的 70% 左右再试",
                "确认使用的是 64 位 Java（32 位进程最多约 1.5 GB）",
            ],
        ),
        K::JavaVersion => (
            "java.class.version",
            "Java 版本不匹配",
            vec![
                "为这个实例换用匹配的 Java（1.18+ → 17，1.20.5+ → 21）",
                "确认没有混入为其他游戏版本编译的模组",
            ],
        ),
        K::GameFiles => (
            "files.corrupt.jar",
            "游戏文件缺失或损坏",
            vec!["对实例执行「修复」，校验并补全缺失的文件"],
        ),
        K::Unknown => (
            "crash.unclassified",
            "崩溃原因未确定",
            vec![
                "查看日志里的第一个 ERROR 条目",
                "如果日志里有 hs_err_pid 文件，那是 JVM 原生崩溃，需要单独分析",
            ],
        ),
    };

    /*
     * 模组失败的具体条目要进 evidence 与 mods：
     * 「哪个模组、缺什么」比一句「模组加载失败」有用得多，而它对用户是可操作的。
     */
    let mut evidence: Vec<String> = cause
        .root_exception
        .clone()
        .map(|exception| vec![exception])
        .unwrap_or_default();
    let mut mods = Vec::new();
    for failure in &cause.mod_failures {
        evidence.push(failure.message.clone());
        mods.push(failure.mod_id.clone());
    }

    Finding {
        code: code.to_owned(),
        title: title.to_owned(),
        confidence: cause.confidence,
        explanation: cause.reason.clone(),
        suggestions: suggestions.into_iter().map(str::to_owned).collect(),
        evidence,
        mods,
    }
}

#[cfg(test)]
mod cause_regression_tests {
    use super::*;

    /*
     * 这些测试用**真实崩溃报告**的片段。
     *
     * 它们存在的理由：关键词规则在真实日志上产生过一个非常有害的误报——把系统的
     * inotify 上限说成「Mixin 注入失败」。原因是 Forge 给每一帧都加 `{re:mixin}`。
     * 修好之后必须有测试把它钉住，否则下次有人加一条 "mixin" 规则就会复发。
     */

    const INOTIFY_REPORT: &str = r#"
---- Minecraft Crash Report ----
Description: Rendering overlay

java.lang.RuntimeException: java.io.IOException: User limit of inotify instances reached or too many open files
	at com.electronwill.nightconfig.core.file.FileWatcher$WatchedDir.<init>(FileWatcher.java:220) ~[core-3.6.4.jar%237!/:?] {}
	at java.util.Map.computeIfAbsent(Map.java:1054) ~[?:?] {re:mixin}
	at net.minecraftforge.fml.config.ConfigTracker.openConfig(ConfigTracker.java:60) ~[fmlcore-1.18.2-40.2.10.jar%23331!/:?] {}
	at net.minecraftforge.fml.ModLoader.loadMods(ModLoader.java:170) ~[fmlcore.jar] {}
	at net.minecraft.client.renderer.GameRenderer.m_109093_(GameRenderer.java:882) ~[client.jar] {re:mixin,pl:mixin:APP:create.mixins.json:GameRendererMixin,re:classloading}
Caused by: java.io.IOException: User limit of inotify instances reached or too many open files
	at sun.nio.fs.LinuxWatchService.<init>(LinuxWatchService.java:62) ~[?:?] {}
	... 31 more
"#;

    const NATIVE_LIB_REPORT: &str = r#"
---- Minecraft Crash Report ----
Description: Ticking screen

java.lang.UnsatisfiedLinkError: Can't load library: /home/u/.minecraft/mods/mcef-libraries/linux_amd64/libcef.so
	at java.base/java.lang.ClassLoader.loadLibrary(ClassLoader.java:2422) ~[?:?] {}
	at com.cinemamod.mcef.CefApp.startup(CefApp.java:452) ~[mcef.jar] {re:mixin,re:classloading}
	at net.minecraft.client.Minecraft.setScreen(Minecraft.java) ~[client.jar] {re:mixin}
"#;

    #[test]
    fn real_inotify_report_is_not_blamed_on_mixins() {
        let (findings, _facts, _warnings) =
            analyse_log(INOTIFY_REPORT, &Environment::default());
        let codes: Vec<&str> = findings.iter().map(|item| item.code.as_str()).collect();

        assert!(
            !codes.contains(&"mods.mixin"),
            "a system limit must never be reported as a mixin failure, got {codes:?}"
        );
        assert!(
            codes.contains(&"system.inotify"),
            "the real cause must be reported, got {codes:?}"
        );

        let finding = findings
            .iter()
            .find(|item| item.code == "system.inotify")
            .unwrap();
        // 建议必须指向系统限制，而不是「禁用模组」。
        assert!(finding.suggestions.iter().any(|s| s.contains("sysctl") || s.contains("限制")));
    }

    #[test]
    fn real_native_library_report_is_not_blamed_on_mods() {
        let (findings, _facts, _warnings) =
            analyse_log(NATIVE_LIB_REPORT, &Environment::default());
        let codes: Vec<&str> = findings.iter().map(|item| item.code.as_str()).collect();
        assert!(
            !codes.contains(&"mods.mixin"),
            "a missing .so must not be reported as a mod failure, got {codes:?}"
        );
        assert!(codes.contains(&"native.library"), "got {codes:?}");
    }

    #[test]
    fn a_genuine_mixin_failure_still_reports_as_mods() {
        let log = r#"
java.lang.RuntimeException: mixin apply failed
	at org.spongepowered.asm.mixin.transformer.MixinApplicatorStandard.apply(MixinApplicatorStandard.java:380)
Caused by: org.spongepowered.asm.mixin.throwables.MixinApplyError: Mixin apply failed example.mixin.Foo
	at org.spongepowered.asm.mixin.transformer.MixinApplicatorStandard.applyMixin(MixinApplicatorStandard.java:400)
"#;
        let (findings, _facts, _warnings) = analyse_log(log, &Environment::default());
        let codes: Vec<&str> = findings.iter().map(|item| item.code.as_str()).collect();
        assert!(
            codes.contains(&"mods.mixin"),
            "a real mixin apply failure must still be caught, got {codes:?}"
        );
    }

    #[test]
    fn hardware_verdict_is_attached_when_detection_data_exists() {
        let log = "java.lang.OutOfMemoryError: Java heap space\n";
        let environment = Environment {
            hardware: Some(hardware::Hardware {
                total_memory_mb: Some(2048),
                cpu_cores: Some(2),
                ..Default::default()
            }),
            game_version: Some("1.18.2".into()),
            mod_count: Some(300),
            memory_mb: Some(2048),
            ..Default::default()
        };
        let (findings, _facts, _warnings) = analyse_log(log, &environment);
        assert!(!findings.is_empty());

        // 硬件判断走独立通道，由 analyse() 附加；这里直接验证 assess 的结论。
        let capability = hardware::assess(
            environment.hardware.as_ref().unwrap(),
            &hardware::Workload {
                game_version: environment.game_version.clone(),
                mod_count: environment.mod_count,
                allocated_memory_mb: environment.memory_mb,
                ..Default::default()
            },
        );
        assert_eq!(capability.verdict, hardware::Verdict::Hopeless);
    }
}
