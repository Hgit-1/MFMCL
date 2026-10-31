// The deterministic half of crash analysis.
//
// Every rule is a signature: a set of needles that must all appear, optional
// text that disqualifies the rule, and the explanation to attach when it
// matches. This is the part that must be right — the model only ever explains
// what these rules found.
//
// ## Where the signatures come from
//
// They are the failure modes that actually show up in Minecraft launcher bug
// reports, grouped by the layer that fails:
//
//   - the JVM refusing to start (heap, invalid flags, wrong bitness),
//   - the wrong Java for the game or loader (class version, missing exports),
//   - missing or corrupted game files,
//   - the graphics driver / OpenGL,
//   - mod-loading failures (mixin, duplicate mods, missing dependencies),
//   - runs that were killed by something external (OOM killer, no display).
//
// The set is not meant to be exhaustive. It is meant to be *honest*: each rule
// here corresponds to a concrete, verifiable fix.

use crate::Confidence;
use crate::Finding;

/// One signature.
pub struct Rule {
    /// Stable id. Tests assert on this, never on the prose.
    pub code: &'static str,
    /// All of these must be present (case-insensitive).
    pub needles: &'static [&'static str],
    /// Any of these present disqualifies the rule.
    pub unless: &'static [&'static str],
    pub title: &'static str,
    pub confidence: Confidence,
    pub explanation: &'static str,
    pub suggestions: &'static [&'static str],
    /// Regex whose first capture group is a mod name, when the rule identifies one.
    pub mod_pattern: Option<&'static str>,
}

/*
 * Order matters only for readability: every rule is evaluated and all matches
 * are returned. Duplicate causes are common (running out of heap triggers a
 * mixin error too), and reporting both is more useful than picking one.
 */
pub static RULES: &[Rule] = &[
    // --- The JVM refused to start -------------------------------------------
    Rule {
        code: "heap.invalid",
        needles: &["invalid maximum heap size"],
        unless: &[],
        title: "内存上限参数无效",
        confidence: Confidence::High,
        explanation:
            "JVM 拒绝了 -Xmx 的取值。通常是数字写错、带了单位以外的字符，或这个值超过了当前 Java 能分配的上限。",
        suggestions: &[
            "在实例设置里把最大内存改回 2048–4096 MB 之间再试",
            "确认最大内存不小于初始内存",
        ],
        mod_pattern: None,
    },
    Rule {
        code: "heap.reserve",
        needles: &["could not reserve enough space"],
        unless: &["invalid maximum heap size"],
        title: "无法申请到设定内存",
        confidence: Confidence::High,
        explanation:
            "系统没有足够的连续内存给 JVM。可能是设定的值接近物理内存总量，也可能是用了 32 位 Java（最多约 1.5 GB）。",
        suggestions: &[
            "降低分配的内存，或先关掉占用内存较多的程序",
            "如果分配值很小仍然失败，检查是否误用了 32 位 Java",
        ],
        mod_pattern: None,
    },
    Rule {
        code: "jvm.option.invalid",
        needles: &["unrecognized option"],
        unless: &[],
        title: "JVM 参数不被识别",
        confidence: Confidence::High,
        explanation: "启动参数里有一个当前 Java 不认识的选项，通常来自手工添加的额外 JVM 参数。",
        suggestions: &[
            "清空实例设置里的「额外 JVM 参数」，再逐条加回以定位是哪一条",
            "确认这些参数与当前 Java 版本匹配（例如 tuned 参数常只对特定版本有效）",
        ],
        mod_pattern: None,
    },
    Rule {
        code: "jvm.option.space",
        needles: &["unrecognized vm option"],
        unless: &[],
        title: "JVM 调优参数不被识别",
        confidence: Confidence::High,
        explanation: "某一项 -XX 参数在当前 Java 上不存在。常见于从旧版本复制过来的调优参数。",
        suggestions: &["删掉这一条 -XX 参数，或改用当前 Java 支持的写法"],
        mod_pattern: None,
    },
    // --- The wrong Java for this game ---------------------------------------
    Rule {
        code: "java.class.version",
        needles: &["unsupportedclassversionerror"],
        unless: &[],
        title: "Java 版本过低",
        confidence: Confidence::High,
        explanation:
            "有类文件是更高版本的 Java 编译的，当前 Java 读不了。Minecraft 1.18+ 需要 Java 17，1.20.5+ 需要 Java 21。",
        suggestions: &[
            "为这个实例换用更高版本的 Java（1.18+ → 17，1.20.5+ → 21）",
            "确认没有混入为更新的游戏版本编译的模组",
        ],
        mod_pattern: None,
    },
    Rule {
        code: "java.class.version.mismatch",
        needles: &["unsupported class file major version"],
        unless: &[],
        title: "Java 版本与游戏不匹配",
        confidence: Confidence::High,
        explanation: "类文件的版本号高于当前 Java 支持的上限，说明游戏或模组要求的 Java 比现在这个新。",
        suggestions: &["提高这个实例使用的 Java 版本"],
        mod_pattern: None,
    },
    Rule {
        code: "java.too.new",
        needles: &["does not export", "java.base"],
        unless: &[],
        title: "Java 版本过高",
        confidence: Confidence::High,
        explanation:
            "模组或加载器访问了被新版 Java 收紧的模块。它们是为更旧的 Java 编写的，在 Java 17+ 上常需要加 --add-opens。",
        suggestions: &[
            "把这个实例换成较低版本的 Java",
            "或升级模组/加载器到支持当前 Java 的版本",
        ],
        mod_pattern: None,
    },
    Rule {
        code: "java.openj9",
        needles: &["j9vm"],
        unless: &[],
        title: "不支持 OpenJ9 虚拟机",
        confidence: Confidence::High,
        explanation: "当前用的是 OpenJ9 而非 HotSpot。Minecraft 与绝大多数模组都只支持 HotSpot。",
        suggestions: &["改用 HotSpot 版 Java：Temurin、Adoptium、Zulu、Oracle JDK 皆可"],
        mod_pattern: None,
    },
    // --- Game files ---------------------------------------------------------
    Rule {
        code: "files.missing.assets",
        needles: &["failed to download", "assets"],
        unless: &[],
        title: "游戏资源下载不完整",
        confidence: Confidence::Medium,
        explanation: "启动时缺少资源文件，说明上一次安装没有完成，或磁盘上的文件被删除了。",
        suggestions: &["在游戏库里对实例点「修复」，让它校验并补全缺失文件"],
        mod_pattern: None,
    },
    Rule {
        code: "files.corrupt.jar",
        needles: &["invalid or corrupt jarfile"],
        unless: &[],
        title: "核心 jar 损坏",
        confidence: Confidence::High,
        explanation: "客户端 jar 无法读取，通常是下载中断或磁盘写入出错导致的半截文件。",
        suggestions: &["对实例执行「修复」以重新校验并下载客户端"],
        mod_pattern: None,
    },
    Rule {
        code: "files.class.notfound",
        needles: &["classnotfoundexception"],
        unless: &[],
        title: "缺少必需的类",
        confidence: Confidence::Medium,
        explanation:
            "某个类在类路径上找不到。可能是模组文件缺失、加载器安装不完整，或模组与游戏版本不对应。",
        suggestions: &[
            "对实例执行「修复」，确认加载器与官方文件齐全",
            "检查是否有模组要求的前置库没有安装",
        ],
        mod_pattern: None,
    },
    Rule {
        code: "files.no.main.class",
        needles: &["could not find or load main class"],
        unless: &[],
        title: "找不到主类",
        confidence: Confidence::High,
        explanation: "启动命令里的主类不存在，通常是加载器没装好，或启动配置与本实例不匹配。",
        suggestions: &["重新安装这个实例使用的加载器", "对实例执行「修复」"],
        mod_pattern: None,
    },
    Rule {
        code: "files.access.denied",
        needles: &["accessdeniedexception"],
        unless: &[],
        title: "文件访问被拒绝",
        confidence: Confidence::Medium,
        explanation:
            "游戏无法写入某个文件。可能是文件被其他程序占用，或当前用户对游戏目录没有写权限。",
        suggestions: &[
            "关闭可能正在占用这些文件的程序（云同步盘、杀毒软件、另一个启动器）",
            "确认游戏目录不在需要管理员权限的位置",
        ],
        mod_pattern: None,
    },
    // --- Graphics -----------------------------------------------------------
    Rule {
        code: "gpu.opengl",
        needles: &["org.lwjgl.opengl"],
        unless: &[],
        title: "显卡/OpenGL 初始化失败",
        confidence: Confidence::High,
        explanation:
            "创建 OpenGL 上下文失败。常见原因是显卡驱动过旧、没有可用的显示设备，或在远程桌面/无头环境里启动。",
        suggestions: &[
            "更新显卡驱动",
            "如果在远程桌面或虚拟机里运行，改用本机桌面会话",
            "确认没有强制使用软件渲染的选项",
        ],
        mod_pattern: None,
    },
    Rule {
        code: "gpu.gl.function",
        needles: &["glfw error"],
        unless: &[],
        title: "窗口系统初始化失败",
        confidence: Confidence::High,
        explanation: "GLFW 无法创建窗口或初始化图形上下文，通常与驱动或显示环境有关。",
        suggestions: &["更新显卡驱动", "确认当前会话有可用的桌面环境"],
        mod_pattern: None,
    },
    Rule {
        code: "gpu.driver",
        needles: &["nvoglv", "ig"],
        unless: &[],
        title: "显卡驱动崩溃",
        confidence: Confidence::Medium,
        explanation: "崩溃发生在显卡驱动的模块里，说明问题来自驱动或它的某次调用，而不是 Java 层。",
        suggestions: &[
            "更新或回退显卡驱动",
            "关闭超频，或在显卡控制面板里为 Java 指定独显",
        ],
        mod_pattern: None,
    },
    // --- Mod loading --------------------------------------------------------
    Rule {
        code: "mods.mixin",
        /*
         * `mixin` alone is far too loose: every Fabric/Forge log prints a
         * "SpongePowered MIXIN Subsystem Version=..." banner on a perfectly
         * healthy launch, so matching that word alone reported a mixin failure
         * on a clean run (caught by the clean-launch fixture).
         *
         * Requiring both halves of a failure message — the mixin subsystem AND a
         * failure word — keeps the banner from matching while still catching
         * `MixinApplyError`, `MixinTransformerError`, `mixin apply failed`, and
         * the "Mixin config ... failed to apply" variants. The `unless` clause
         * then filters the reverse case: a log that merely *mentions* a failure
         * while reporting it recovered.
         */
        needles: &["mixin", "failed"],
        unless: &["mixin subsystem version"],
        title: "Mixin 注入失败",
        confidence: Confidence::Medium,
        explanation:
            "有模组通过 Mixin 修改游戏代码时失败。原因可能是该模组与当前游戏/加载器版本不兼容，或两个模组改写了同一处。",
        suggestions: &[
            "确认失败的这个模组与游戏版本、加载器版本匹配",
            "逐个禁用最近安装的模组以定位冲突",
        ],
        mod_pattern: Some(r"mixin\s+([A-Za-z0-9_.$]+)"),
    },
    Rule {
        code: "mods.duplicate",
        needles: &["duplicate mod"],
        unless: &[],
        title: "同一模组存在多份",
        confidence: Confidence::High,
        explanation: "mods 目录里找到了同一个模组的多个副本，加载器不知道该用哪个。",
        suggestions: &["在实例的模组管理里删掉重复的版本，只保留一个"],
        mod_pattern: None,
    },
    Rule {
        code: "mods.missing.dependency",
        needles: &["missing dependency"],
        unless: &[],
        title: "缺少模组前置",
        confidence: Confidence::High,
        explanation: "某个模组要求另一个模组，但它不在 mods 目录里。",
        suggestions: &["按提示安装缺失的前置模组", "确认前置模组与本模组的游戏版本一致"],
        mod_pattern: None,
    },
    Rule {
        code: "mods.incompatible",
        needles: &["incompatible mod set"],
        unless: &[],
        title: "模组集合不兼容",
        confidence: Confidence::High,
        explanation: "加载器检出了互相排斥的模组，或模组声明的版本范围互不重叠。",
        suggestions: &["按报错里列出的模组逐个排查", "在实例的模组管理里查看冲突提示"],
        mod_pattern: None,
    },
    Rule {
        code: "mods.forge.dependency",
        needles: &["mod loading has failed", "forge"],
        unless: &[],
        title: "Forge 模组加载失败",
        confidence: Confidence::Medium,
        explanation: "Forge 在加载阶段报告了模组错误。具体原因通常在日志更早的位置列出。",
        suggestions: &["查看崩溃报告里第一个 ERROR 级别的模组条目", "逐个禁用可疑模组验证"],
        mod_pattern: None,
    },
    // --- Killed from outside ------------------------------------------------
    Rule {
        code: "runtime.killed",
        needles: &["killed", "signal"],
        unless: &[],
        title: "进程被外部终止",
        confidence: Confidence::Low,
        explanation:
            "游戏进程收到了终止信号。可能是系统的内存不足保护（OOM killer）杀掉了它，也可能是被其他程序结束。",
        suggestions: &[
            "降低内存分配或关闭其他大内存程序后重试",
            "检查系统日志里是否有 OOM killer 的记录",
        ],
        mod_pattern: None,
    },
    Rule {
        code: "runtime.headless",
        needles: &["headlessexception"],
        unless: &[],
        title: "没有可用的图形界面",
        confidence: Confidence::High,
        explanation: "程序需要一个图形显示环境，但当前会话没有。",
        suggestions: &["在正常的桌面会话中启动游戏，而不是通过纯 SSH 或无显示的容器"],
        mod_pattern: None,
    },
    // --- A catch-all for the most common real-world crash --------------------
    Rule {
        code: "runtime.outofmemory",
        needles: &["outofmemoryerror"],
        unless: &[],
        title: "内存不足",
        confidence: Confidence::High,
        explanation: "游戏用尽了分配给它的堆内存。模组越多、材质越高，需要的内存越多。",
        suggestions: &[
            "在实例设置里提高内存分配（按模组数量可加 1–2 GB）",
            "减少同时启用的模组，或降低视距与材质分辨率",
        ],
        mod_pattern: None,
    },
];

/// Lines that start a fatal exception.
///
/// Deliberately narrow: `Exception` alone appears in benign log lines
/// (`Catching exception from ...`) that would send the model down the wrong
/// path. Requiring a known fatal wrapper keeps the excerpt meaningful.
pub fn is_primary_exception(line: &str) -> bool {
    const MARKERS: &[&str] = &[
        "Exception in thread",
        "Caused by:",
        "java.lang.",
        "net.minecraft.",
        "org.spongepowered.",
        "java.lang.RuntimeException",
        "Fatal error",
        "A fatal error has been detected",
        "Uncaught exception",
    ];
    MARKERS.iter().any(|marker| line.starts_with(marker) || line.contains(marker))
        && !line.contains("Catching exception")
}

/// Mod names mentioned on a line.
///
/// Fragments a name out of the three places Minecraft logs put them: an explicit
/// `modid`, a mixin package, and a `mods.toml` style `Mod File:` line. False
/// positives are cheap here — the name is only used to point the model at a
/// likely culprit and to show the user which mod to look at first.
pub fn mods_in_line(line: &str) -> Vec<String> {
    let mut found = Vec::new();
    let lower = line.to_ascii_lowercase();

    if let Some(index) = lower.find("mod file:") {
        let rest = line[index + "mod file:".len()..].trim();
        if let Some(name) = rest.split_whitespace().next() {
            found.push(name.trim_matches(|c| c == '「' || c == '」' || c == '"').to_owned());
        }
    }
    for marker in ["modid:", "mod id:"] {
        if let Some(index) = lower.find(marker) {
            let rest = line[index + marker.len()..].trim();
            if let Some(name) = rest.split_whitespace().next() {
                let name = name.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_');
                if !name.is_empty() {
                    found.push(name.to_owned());
                }
            }
        }
    }
    // Mixin classes are `package.mixin.ClassName`; the package before `.mixin`
    // is usually the mod name.
    if lower.contains("mixin") {
        for token in line.split_whitespace() {
            if let Some(position) = token.find(".mixin") {
                let package = &token[..position];
                if let Some(name) = package.split('.').next()
                    && name.len() > 2
                    && !name.eq_ignore_ascii_case("org")
                    && !name.eq_ignore_ascii_case("com")
                    && !name.eq_ignore_ascii_case("net")
                {
                    found.push(name.to_owned());
                }
            }
        }
    }
    found.retain(|item| {
        item.len() >= 2
            && item.len() <= 64
            && item.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    });
    found.sort();
    found.dedup();
    found
}

/// Java version from a `java version "17.0.9"` style line.
pub fn java_version_from_line(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    if !(lower.contains("java version") || lower.contains("openjdk version")) {
        return None;
    }
    let start = line.find('"')? + 1;
    let end = line[start..].find('"')? + start;
    let version = line[start..end].trim();
    if version.is_empty() {
        None
    } else {
        Some(version.to_owned())
    }
}

/// Game version from the usual `Minecraft Version: 1.21.1` line, or the
/// `--version 1.21.1` argument echoed in a crash report.
pub fn game_version_from_line(line: &str) -> Option<String> {
    let markers = ["minecraft version:", "version:", "--version"];
    let lower = line.to_ascii_lowercase();
    for marker in markers {
        if let Some(index) = lower.find(marker) {
            let rest = line[index + marker.len()..].trim();
            let token = rest.split_whitespace().next()?;
            let cleaned = token.trim_matches(|c: char| !c.is_ascii_digit() && c != '.' && c != '-');
            if cleaned.starts_with('1') || cleaned.contains('.') {
                return Some(cleaned.to_owned());
            }
        }
    }
    None
}

/// Evaluate every rule against the log body.
pub fn evaluate(log: &str) -> Vec<Finding> {
    let lower = log.to_ascii_lowercase();
    let mut findings: Vec<Finding> = Vec::new();

    for rule in RULES {
        let matched = rule.needles.iter().all(|needle| lower.contains(needle));
        if !matched {
            continue;
        }
        if rule.unless.iter().any(|needle| lower.contains(needle)) {
            continue;
        }
        /*
         * Two rules claiming the same code would mean a copy-paste error; skip
         * the duplicate rather than showing the user the same cause twice.
         */
        if findings.iter().any(|item| item.code == rule.code) {
            continue;
        }

        let evidence = evidence_lines(log, rule);
        let mods = match rule.mod_pattern {
            Some(pattern) => extract_mods(&evidence, pattern),
            None => Vec::new(),
        };
        findings.push(Finding {
            code: rule.code.to_owned(),
            title: rule.title.to_owned(),
            confidence: rule.confidence,
            explanation: rule.explanation.to_owned(),
            suggestions: rule.suggestions.iter().map(|item| (*item).to_owned()).collect(),
            evidence,
            mods,
        });
    }

    // Strongest first; stable so equal-confidence rules keep their table order.
    findings.sort_by(|a, b| b.confidence.cmp(&a.confidence));
    findings
}

/// The log lines that made a rule fire, capped so the UI stays readable.
fn evidence_lines(log: &str, rule: &Rule) -> Vec<String> {
    let mut out = Vec::new();
    for line in log.lines() {
        let lower = line.to_ascii_lowercase();
        if rule.needles.iter().any(|needle| lower.contains(needle)) {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let clipped = if trimmed.chars().count() > 300 {
                let cut = trimmed.char_indices().nth(300).map(|(i, _)| i).unwrap_or(trimmed.len());
                format!("{}…", &trimmed[..cut])
            } else {
                trimmed.to_owned()
            };
            out.push(clipped);
            if out.len() >= 4 {
                break;
            }
        }
    }
    out
}

fn extract_mods(evidence: &[String], pattern: &str) -> Vec<String> {
    let Ok(regex) = regex::Regex::new(pattern) else {
        return Vec::new();
    };
    let mut mods = Vec::new();
    for line in evidence {
        if let Some(captures) = regex.captures(&line.to_ascii_lowercase())
            && let Some(name) = captures.get(1)
        {
            let name = name.as_str().to_owned();
            if !mods.contains(&name) {
                mods.push(name);
            }
        }
    }
    mods
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heap_error_is_recognised() {
        let log = "Error: Could not reserve enough space for 8388608KB object heap\n";
        let findings = evaluate(log);
        assert_eq!(findings[0].code, "heap.reserve");
        assert_eq!(findings[0].confidence, Confidence::High);
    }

    #[test]
    fn invalid_heap_wins_over_reserve() {
        // The reserve rule excludes itself when the heap value itself is invalid:
        // they need opposite fixes.
        let log = "Invalid maximum heap size: -Xmx9999999999m\nCould not reserve enough space\n";
        let findings = evaluate(log);
        assert!(findings.iter().any(|f| f.code == "heap.invalid"));
        assert!(
            !findings.iter().any(|f| f.code == "heap.reserve"),
            "reserve must be suppressed when the heap value is invalid"
        );
    }

    #[test]
    fn class_version_surfaces_as_low_java() {
        let log = "java.lang.UnsupportedClassVersionError: net/fabricmc/loader/FabricLoader \
                   has been compiled by a more recent version of the Java Runtime";
        let findings = evaluate(log);
        assert_eq!(findings[0].code, "java.class.version");
        assert!(findings[0].suggestions.iter().any(|s| s.contains("17")));
    }

    #[test]
    fn java_version_and_game_version_are_extracted() {
        assert_eq!(
            java_version_from_line(r#"openjdk version "21.0.2" 2024-01-16"#).as_deref(),
            Some("21.0.2")
        );
        assert_eq!(
            game_version_from_line("Minecraft Version: 1.21.1").as_deref(),
            Some("1.21.1")
        );
    }

    #[test]
    fn mods_are_extracted_from_a_mixin_failure() {
        let log = "org.spongepowered.asm.mixin.throwables.MixinApplyError: \
                   Mixin apply failed sodium.mixin.SodiumMixin";
        let findings = evaluate(log);
        let mixin = findings
            .iter()
            .find(|f| f.code == "mods.mixin")
            .expect("mixin rule must fire");
        assert!(!mixin.mods.is_empty(), "the offending mod should be named");
    }

    #[test]
    fn benign_exception_mentions_are_not_primary() {
        assert!(!is_primary_exception("Catching exception from worker thread"));
        assert!(is_primary_exception("java.lang.OutOfMemoryError: Java heap space"));
    }

    #[test]
    fn multiple_independent_causes_are_all_returned() {
        let log = "java.lang.OutOfMemoryError: Java heap space\n\
                   org.spongepowered.asm.mixin.MixinApplyError: failed\n";
        let findings = evaluate(log);
        let codes: Vec<&str> = findings.iter().map(|f| f.code.as_str()).collect();
        assert!(codes.contains(&"runtime.outofmemory"));
        assert!(codes.contains(&"mods.mixin"));
    }

    #[test]
    fn clean_log_produces_no_findings() {
        let log = "[12:00:00] [main/INFO]: Loading 34 mods\n[12:00:01] [main/INFO]: Done\n";
        assert!(evaluate(log).is_empty());
    }

    #[test]
    fn stronger_confidence_sorts_first() {
        let log = "Unrecognized VM option 'UseG1GC'\nMixin apply failed\n";
        let findings = evaluate(log);
        assert_eq!(findings[0].confidence, Confidence::High);
    }
}

/// 哪些关键词规则与一个确定的因果类别是同一个问题。
///
/// ## 判断标准是「修复动作是否相同」，不是字面相似
///
/// 一个 inotify 上限和 mixin 注入失败需要完全不同的处理，所以它们互不相关；
/// 而「内存不足」与「堆参数无效」都指向内存设置，可以并存。
///
/// 这张表存在的直接原因：真实报告
/// `crash-2026-08-20_13.43.14-client.txt` 里，堆栈每一帧都带 `{re:mixin}`
/// （Forge 的改写标记），关键词规则因此报「Mixin 注入失败」；而真实原因是
/// inotify 实例上限。两者同时出现时，用户只会看到两条互相矛盾的结论。
pub fn related_to(kind: crate::causes::CauseKind) -> &'static [&'static str] {
    use crate::causes::CauseKind as K;
    match kind {
        // 系统资源上限：只有「内存参数」类可能相关（EAGAIN/ENOMEM 常与内存同现），
        // 模组、图形、Java 版本全部无关。
        K::SystemLimit => &["heap.reserve", "heap.invalid"],
        K::OutOfMemory => &["runtime.outofmemory", "heap.reserve", "heap.invalid"],
        K::JvmArguments => &["heap.invalid", "jvm.option.invalid", "jvm.option.space"],
        K::JavaVersion => &["java.class.version", "java.class.version.mismatch", "java.too.new", "java.openj9"],
        K::GameFiles => &[
            "files.missing.assets",
            "files.corrupt.jar",
            "files.class.notfound",
            "files.no.main.class",
        ],
        K::Graphics => &["gpu.opengl", "gpu.gl.function", "gpu.driver"],
        K::Mods => &[
            "mods.mixin",
            "mods.duplicate",
            "mods.missing.dependency",
            "mods.incompatible",
            "mods.forge.dependency",
        ],
        // 本地库缺失是「装系统包」的问题，不是「模组坏了」。
        K::NativeLibrary => &[],
        K::FileAccess => &["files.access.denied"],
        K::ExternalKill => &["runtime.killed"],
        K::NoDisplay => &["runtime.headless"],
        // 未知类别不筛选：交给关键词兜底。
        K::Unknown => &[
            "heap.invalid",
            "heap.reserve",
            "jvm.option.invalid",
            "jvm.option.space",
            "java.class.version",
            "java.class.version.mismatch",
            "java.too.new",
            "java.openj9",
            "files.missing.assets",
            "files.corrupt.jar",
            "files.class.notfound",
            "files.no.main.class",
            "files.access.denied",
            "gpu.opengl",
            "gpu.gl.function",
            "gpu.driver",
            "mods.mixin",
            "mods.duplicate",
            "mods.missing.dependency",
            "mods.incompatible",
            "mods.forge.dependency",
            "runtime.killed",
            "runtime.headless",
            "runtime.outofmemory",
        ],
    }
}
