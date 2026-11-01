use crate::InstanceRecord;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 24 * 1024 * 1024;
const MAX_FILES: usize = 10;

#[derive(Debug, Error)]
pub enum DiagnosticError {
    #[error("file operation failed: {0}")]
    Io(#[from] io::Error),
    #[error("could not create diagnostic archive: {0}")]
    Archive(String),
    #[error("selected export directory does not exist")]
    InvalidDestination,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticSummary {
    /// 最可能的原因（多条命中时取置信度最高的）。
    pub title: String,
    /// `high` / `medium` / `low` / `none` / `unknown`。
    pub confidence: String,
    pub explanation: String,
    pub evidence: Vec<String>,
    pub files_found: usize,
    /// 全部命中的原因，按置信度从高到低。
    ///
    /// 为什么返回多条而不是一条：崩溃往往同时满足多个特征（例如内存不足会连带
    /// 触发 Mixin 失败），只报一条容易把人引向错误的方向。PCL 的 CrashAnalyzer
    /// 同样收集多个原因并逐个列出。
    #[serde(default)]
    pub reasons: Vec<DiagnosticReason>,
}

/// 一条命中的崩溃原因。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticReason {
    pub title: String,
    pub confidence: String,
    pub explanation: String,
    /// 可执行的下一步。空表示只能人工排查。
    pub suggestions: Vec<String>,
    /// 证据：命中的关键字与所在文件。
    pub evidence: Vec<String>,
    /// 从日志里提取出的模组名（能提取到时才有）。
    #[serde(default)]
    pub mods: Vec<String>,
}

/// 一条诊断规则。
struct Rule {
    /// 触发关键字。全部出现才算命中（与关系），用于需要组合特征的规则。
    needles: &'static [&'static str],
    /// 排除关键字。任一出现则本规则不成立——例如「内存不足」要排除
    /// 「32 位 Java 无法分配」，两者指向完全不同的处理方式。
    unless: &'static [&'static str],
    title: &'static str,
    confidence: &'static str,
    explanation: &'static str,
    suggestions: &'static [&'static str],
    /// 命中后从日志里提取模组名的正则，取第一个捕获组。
    mod_pattern: Option<&'static str>,
}

/// 诊断规则表。
///
/// ## 设计来源
///
/// 规则本身参考 PCL2 的 `ModCrash.vb`（`AnalyzeCrit1`/`AnalyzeCrit2`）——它对每个
/// 已知崩溃模式列出触发关键字，并给出中文原因描述。这里按同一思路重写，但做了
/// 三点调整：
///
/// 1. **全部改为结构化规则**，而不是一长串 if。规则表能一眼看出覆盖了哪些模式，
///    加规则时也不会漏改相邻分支。
/// 2. **每条规则带建议**。PCL 的原因枚举不含建议，用户在界面上看到「内存不足」
///    之后仍不知道该怎么办。这里把「怎么改」也写进规则。
/// 3. **带 `unless` 排除项**。PCL 有些规则靠 if/else 分支区分（例如
///    `Could not reserve enough space` 在 32 位 Java 与普通内存不足下含义不同），
///    这里用排除项表达同一层判断，保持规则表是扁平的。
///
/// 关键字尽量取**同时适用于 Forge/Fabric/原版**的形态：加载器各自的日志格式不同，
/// 只匹配某一种会让另一半用户得不到诊断。
static RULES: &[Rule] = &[
    // --- 内存 ---------------------------------------------------------------
    Rule {
        needles: &["Invalid maximum heap size"],
        unless: &[],
        title: "内存参数超出 Java 可分配范围",
        confidence: "high",
        explanation: "启动参数里的最大堆内存超过了 JVM 能分配的上限，通常是数值写错或使用了 32 位 Java。",
        suggestions: &[
            "把最大内存调到 4096 MB 以下再试",
            "确认使用的是 64 位 Java（32 位进程最多只能用约 1.5 GB）",
        ],
        mod_pattern: None,
    },
    Rule {
        needles: &["Could not reserve enough space"],
        unless: &["Invalid maximum heap size"],
        title: "内存分配失败",
        confidence: "high",
        explanation: "JVM 无法向系统申请到设定的内存量。可能是物理内存不足、被其他程序占用，或数值超过了 32 位 Java 的上限。",
        suggestions: &[
            "降低分配的内存，或在启动前关闭占用内存较多的程序",
            "如果分配值本身很小却仍失败，检查是否误用了 32 位 Java",
        ],
        mod_pattern: None,
    },
    Rule {
        needles: &[
            "OutOfMemoryError",
            "an out of memory error",
            "Out of Memory Error",
        ],
        unless: &[],
        title: "内存不足",
        confidence: "high",
        explanation: "游戏运行时耗尽了分配给它的堆内存。模组越多、材质越高，需要的内存越多。",
        suggestions: &[
            "在实例设置里提高内存分配（建议按模组数量增加 1–2 GB）",
            "减少同时启用的模组，或降低材质/视距",
        ],
        mod_pattern: None,
    },
    // --- Java 与 JVM --------------------------------------------------------
    Rule {
        needles: &["Unrecognized option:"],
        unless: &[],
        title: "JVM 参数无效",
        confidence: "high",
        explanation: "启动参数里有一个当前 Java 不认识的选项。可能是手工添加的 JVM 参数，或从其他启动器迁移时带过来的。",
        suggestions: &[
            "检查实例设置里的「额外 JVM 参数」，逐个删掉后重试",
            "确认这些参数与你使用的 Java 版本兼容",
        ],
        mod_pattern: None,
    },
    Rule {
        needles: &[
            "UnsupportedClassVersionError",
            "Unsupported class file major version",
            "Unsupported major.minor version",
        ],
        unless: &[],
        title: "Java 版本不匹配",
        confidence: "high",
        explanation: "有类文件是用更高版本的 Java 编译的，当前 Java 识别不了。常见于模组要求更高的 Java，或游戏版本与 Java 版本不对应。",
        suggestions: &[
            "为该实例改用更高版本的 Java（1.18+ 建议 Java 17，1.20.5+ 建议 Java 21）",
            "确认没有混入为其他游戏版本编译的模组",
        ],
        mod_pattern: None,
    },
    Rule {
        needles: &[
            "because module java.base does not export",
            "java.lang.NoSuchFieldException: ucp",
        ],
        unless: &[],
        title: "Java 版本过高",
        confidence: "high",
        explanation: "模组或加载器使用了被新版 Java 移除或限制的接口。它们通常是为更旧的 Java 编写的。",
        suggestions: &[
            "为这个实例改用较低版本的 Java",
            "或把模组/加载器升级到支持当前 Java 的版本",
        ],
        mod_pattern: None,
    },
    Rule {
        needles: &[
            "Open J9 is not supported",
            "OpenJ9 is incompatible",
            ".J9VMInternals.",
        ],
        unless: &[],
        title: "不支持 OpenJ9 虚拟机",
        confidence: "high",
        explanation: "当前使用的是 OpenJ9 而非 HotSpot，Minecraft 与多数模组都不支持。",
        suggestions: &["改用 HotSpot 版 Java（Oracle JDK / Temurin / Adoptium 等）"],
        mod_pattern: None,
    },
    Rule {
        needles: &[
            "java.lang.ClassCastException: java.base/jdk",
            "java.lang.ClassCastException: class jdk.",
        ],
        unless: &[],
        title: "使用了 JDK 而非 JRE 以外的问题",
        confidence: "medium",
        explanation: "日志里出现 JDK 内部类的类型转换异常，通常与 Java 发行版或版本不兼容有关。",
        suggestions: &["尝试换成 Temurin / Adoptium 的标准 JDK 或 JRE"],
        mod_pattern: None,
    },
    // --- 图形 ---------------------------------------------------------------
    Rule {
        needles: &["The driver does not appear to support OpenGL"],
        unless: &[],
        title: "显卡驱动不支持 OpenGL",
        confidence: "high",
        explanation: "系统报告的 OpenGL 能力不足，游戏无法创建渲染上下文。",
        suggestions: &[
            "更新显卡驱动",
            "确认不是在没有硬件加速的远程桌面/虚拟机里启动",
        ],
        mod_pattern: None,
    },
    Rule {
        needles: &["Couldn't set pixel format", "Pixel format not accelerated"],
        unless: &[],
        title: "显卡驱动无法设置像素格式",
        confidence: "high",
        explanation: "OpenGL 初始化失败，通常是驱动不完整或缺少硬件加速。",
        suggestions: &["更新或重装显卡驱动", "检查是否在远程桌面环境运行"],
        mod_pattern: None,
    },
    Rule {
        needles: &["EXCEPTION_ACCESS_VIOLATION"],
        unless: &[],
        title: "原生组件异常退出",
        confidence: "medium",
        explanation: "JVM 在原生代码中崩溃。显卡驱动、模组里的原生库、或 Java 本身都可能相关。",
        suggestions: &["更新显卡驱动", "逐个停用带原生库的模组（光影、优化类）排查"],
        mod_pattern: None,
    },
    Rule {
        needles: &["GL_OUT_OF_MEMORY"],
        unless: &[],
        title: "图形内存不足",
        confidence: "medium",
        explanation: "显卡显存不足，常见于高分辨率材质包或光影。",
        suggestions: &["降低材质包分辨率", "关闭光影或降低光影质量"],
        mod_pattern: None,
    },
    Rule {
        needles: &["Maybe try a lower resolution resourcepack?"],
        unless: &[],
        title: "材质包过大",
        confidence: "high",
        explanation: "游戏明确建议降低材质分辨率，说明显存或内存被材质占满。",
        suggestions: &["换用更低分辨率的材质包", "关闭光影"],
        mod_pattern: None,
    },
    Rule {
        needles: &["1282: Invalid operation"],
        unless: &[],
        title: "OpenGL 1282 错误",
        confidence: "medium",
        explanation: "渲染时出现无效操作，多与光影或材质包有关。",
        suggestions: &["暂时移除光影与材质包后重试"],
        mod_pattern: None,
    },
    // --- 模组与加载器 -------------------------------------------------------
    Rule {
        needles: &[
            "Mixin apply failed",
            "MixinApplyError",
            "MixinTransformerError",
            "Mixin prepare failed",
        ],
        unless: &[],
        title: "模组注入失败（Mixin）",
        confidence: "medium",
        explanation: "模组修改游戏代码时失败，通常是模组与游戏版本不匹配，或两个模组改动了同一处。",
        suggestions: &[
            "确认所有模组都支持当前游戏版本",
            "逐个停用最近添加的模组排查",
        ],
        mod_pattern: Some(r"from mod ([^\r\n\] ]+)"),
    },
    Rule {
        needles: &["caught exception from", "Caught exception from"],
        unless: &[],
        title: "确定是某个模组导致崩溃",
        confidence: "high",
        explanation: "加载器明确指出是哪个模组在初始化时抛出异常。",
        suggestions: &[
            "先停用该模组，确认游戏能启动",
            "检查它是否缺少前置模组或版本不匹配",
        ],
        mod_pattern: Some(r"[Cc]aught exception from ([^\r\n(]+)"),
    },
    Rule {
        needles: &["Missing or unsupported mandatory dependencies"],
        unless: &[],
        title: "模组缺少前置",
        confidence: "high",
        explanation: "有模组声明了必需的依赖，但依赖没有安装或版本不符。",
        suggestions: &[
            "按日志列出的依赖名安装对应模组",
            "确认依赖的版本范围与当前游戏版本匹配",
        ],
        mod_pattern: None,
    },
    Rule {
        needles: &["Incompatible mods found!", "ModResolutionException"],
        unless: &[],
        title: "模组之间互不兼容",
        confidence: "high",
        explanation: "加载器检测到无法共存的模组组合。",
        suggestions: &[
            "按日志点名的模组逐个停用排查",
            "检查是否有同一功能的两份模组同时安装",
        ],
        mod_pattern: None,
    },
    Rule {
        needles: &[
            "DuplicateModsFoundException",
            "Found a duplicate mod",
            "Found duplicate mods",
        ],
        unless: &[],
        title: "模组重复安装",
        confidence: "high",
        explanation: "同一个模组存在多份文件（常见于手动复制、或同时装了新旧版本）。",
        suggestions: &[
            "在实例文件 → 模组里删掉重复的 jar",
            "留意 .jar.disabled 之类的重复残留",
        ],
        mod_pattern: None,
    },
    Rule {
        needles: &[
            "The directories below appear to be extracted jar files",
            "Extracted mod jars found",
        ],
        unless: &[],
        title: "模组文件被解压",
        confidence: "high",
        explanation: "mods 目录里出现了被解压的模组文件夹。模组必须以 .jar 形式存在。",
        suggestions: &["把解压出来的模组目录删掉，只保留 .jar 文件"],
        mod_pattern: None,
    },
    Rule {
        needles: &["Invalid module name: '' is not a Java identifier"],
        unless: &[],
        title: "模组名包含特殊字符",
        confidence: "high",
        explanation: "模组文件名里有 Java 模块系统不接受的字符合（例如方括号、中文标点）。",
        suggestions: &["把模组文件名改成只含字母、数字、点、横线、下划线的形式"],
        mod_pattern: None,
    },
    Rule {
        needles: &["maximum id range exceeded"],
        unless: &[],
        title: "模组过多导致 ID 超出上限",
        confidence: "high",
        explanation: "旧版本 Forge 的方块/物品 ID 数量有上限，装太多模组会用尽。",
        suggestions: &["减少模组数量", "或改用支持更多 ID 的新版本"],
        mod_pattern: None,
    },
    Rule {
        needles: &["ClassNotFoundException: org.spongepowered.asm.launch.MixinTweaker"],
        unless: &[],
        title: "缺少 MixinBootstrap",
        confidence: "high",
        explanation: "旧版本 Forge 需要额外的 Mixin 引导模组，但没有安装。",
        suggestions: &["安装对应版本的 MixinBootstrap"],
        mod_pattern: None,
    },
    Rule {
        needles: &[
            "Cannot find launch target fmlclient",
            "Invalid paths argument, contained no existing paths",
        ],
        unless: &[],
        title: "Forge 安装不完整",
        confidence: "high",
        explanation: "加载器文件缺失，通常是安装中断或被手动删除。",
        suggestions: &[
            "在实例设置里重新安装 Forge（当前版本后端尚未支持）",
            "或重建实例",
        ],
        mod_pattern: None,
    },
    Rule {
        needles: &["Failed loading config file"],
        unless: &[],
        title: "模组配置文件损坏",
        confidence: "high",
        explanation: "某个模组的配置文件无法解析，模组在读取配置时崩溃。",
        suggestions: &["在实例文件 → 配置里找到对应模组的配置并删除，让游戏重新生成"],
        mod_pattern: Some(r"for modid ([^\r\n ]+)"),
    },
    Rule {
        needles: &["NightConfig", "ParsingException: Not enough data available"],
        unless: &[],
        title: "NightConfig 解析失败",
        confidence: "medium",
        explanation: "Forge 使用的配置库读取到不完整的文件，通常是上次写入时被中断。",
        suggestions: &["删除实例配置目录下损坏的文件后重新启动"],
        mod_pattern: None,
    },
    Rule {
        needles: &[
            "has been compiled by a more recent version of the Java Runtime (class file version 55.0)",
        ],
        unless: &[],
        title: "模组需要 Java 11",
        confidence: "high",
        explanation: "该模组用 Java 11 编译，当前 Java 版本更低。",
        suggestions: &["为该实例改用 Java 11 或更高版本"],
        mod_pattern: None,
    },
    // --- 其他 ---------------------------------------------------------------
    Rule {
        needles: &["signer information does not match"],
        unless: &[],
        title: "文件签名校验失败",
        confidence: "medium",
        explanation: "同一个包内的类来自不同签名来源，通常是被其他工具改动过 jar。",
        suggestions: &["重新下载该模组", "不要手工往 jar 里塞文件"],
        mod_pattern: None,
    },
    Rule {
        needles: &["Manually triggered debug crash"],
        unless: &[],
        title: "玩家手动触发调试崩溃",
        confidence: "high",
        explanation: "游戏是通过 F3+C 快捷键主动崩溃的，不是真实故障。",
        suggestions: &["无需处理"],
        mod_pattern: None,
    },
];

/// 分析实例日志，给出可能的原因。
///
/// ## 与 PCL 的差别
///
/// PCL 有三级判定（Crit1 命中即停止），这里改为**收集全部命中**后按置信度排序。
/// 原因：
///   - 停止在第一条会把「内存不足」这类上游原因之外的表象（如连带的 Mixin 失败）
///     掩盖掉，用户可能按错误的线索去改；
///   - 多条并列时用户能看到完整的因果链，更容易判断先改哪一个。
///
/// 但保持 PCL 的关键做法：**只报告命中的规则，不猜测**。没有命中就明说「无法确定」，
/// 而不是硬凑一个最接近的原因。
pub fn summarize_instance(instance: &InstanceRecord) -> Result<DiagnosticSummary, DiagnosticError> {
    let files = discover_logs(instance);
    let mut reasons: Vec<DiagnosticReason> = Vec::new();

    for file in files.iter().take(5) {
        let text = match read_bounded(&file.path, MAX_FILE_BYTES) {
            Ok(text) => text,
            // 单个日志读不出来不该让整次分析失败：其余文件仍有价值。
            Err(_) => continue,
        };
        for rule in RULES {
            if reasons.iter().any(|existing| existing.title == rule.title) {
                continue;
            }
            if !rule.needles.iter().any(|needle| text.contains(needle)) {
                continue;
            }
            if rule.unless.iter().any(|needle| text.contains(needle)) {
                continue;
            }
            let mut evidence = vec![format!(
                "{}：检测到 {}",
                file.archive_name,
                rule.needles
                    .iter()
                    .find(|needle| text.contains(*needle))
                    .copied()
                    .unwrap_or_default()
            )];
            let mods = rule
                .mod_pattern
                .map(|pattern| extract_mods(&text, pattern))
                .unwrap_or_default();
            if !mods.is_empty() {
                evidence.push(format!("涉及的模组：{}", mods.join("、")));
            }
            reasons.push(DiagnosticReason {
                title: rule.title.to_owned(),
                confidence: rule.confidence.to_owned(),
                explanation: rule.explanation.to_owned(),
                suggestions: rule.suggestions.iter().map(|s| (*s).to_owned()).collect(),
                evidence,
                mods,
            });
        }
    }

    // 置信度排序：high → medium。同级保持规则表顺序（规则表是按危害从大到小排的）。
    reasons.sort_by_key(|reason| match reason.confidence.as_str() {
        "high" => 0,
        _ => 1,
    });

    let (title, confidence, explanation) = if let Some(first) = reasons.first() {
        (
            first.title.clone(),
            first.confidence.clone(),
            format!(
                "{}{}",
                first.explanation,
                if reasons.len() > 1 {
                    format!(
                        "（另有 {} 条可能相关的原因，见下方列表。）",
                        reasons.len() - 1
                    )
                } else {
                    String::new()
                }
            ),
        )
    } else if files.is_empty() {
        (
            "没有找到可分析的日志".to_owned(),
            "none".to_owned(),
            "游戏目录里没有 logs/latest.log 或 crash-reports。先启动一次游戏再分析，或确认实例已经安装完成。".to_owned(),
        )
    } else {
        (
            "暂时无法确定原因".to_owned(),
            "unknown".to_owned(),
            format!(
                "已检查 {} 个日志文件，但都没有命中已知的崩溃特征。这类问题通常需要人工看日志：可以在下方导出诊断包，或直接打开实例文件 → 日志。",
                files.len()
            ),
        )
    };

    Ok(DiagnosticSummary {
        title,
        confidence,
        explanation,
        evidence: reasons
            .iter()
            .flat_map(|reason| reason.evidence.iter().cloned())
            .take(6)
            .collect(),
        files_found: files.len(),
        reasons,
    })
}

/// 用正则提取模组名，去重并限长。
///
/// 单独成函数是因为 PCL 的同一套提取逻辑散落在十几处正则里；集中一份便于在
/// 日志格式变化时只改一处。
fn extract_mods(text: &str, pattern: &str) -> Vec<String> {
    let Ok(regex) = regex::Regex::new(pattern) else {
        return Vec::new();
    };
    let mut mods: Vec<String> = regex
        .captures_iter(text)
        .filter_map(|captures| captures.get(1).map(|m| m.as_str().trim().to_owned()))
        .filter(|name| !name.is_empty() && name.chars().count() <= 64)
        .collect();
    mods.sort();
    mods.dedup();
    mods.truncate(8);
    mods
}

#[derive(Debug, Clone)]
struct DiagnosticFile {
    path: PathBuf,
    archive_name: String,
    modified: Option<std::time::SystemTime>,
}

pub fn export_instance_diagnostics(
    instance: &InstanceRecord,
    destination_dir: &Path,
) -> Result<PathBuf, DiagnosticError> {
    let canonical_destination = destination_dir
        .canonicalize()
        .map_err(|_| DiagnosticError::InvalidDestination)?;
    if !canonical_destination.is_dir() {
        return Err(DiagnosticError::InvalidDestination);
    }
    let files = discover_logs(instance);
    let archive_path = unique_archive_path(&canonical_destination);
    let file = File::create(&archive_path)?;
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o600);
    let mut total = 0_u64;
    for item in files.iter().take(MAX_FILES) {
        let metadata = match std::fs::metadata(&item.path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
            continue;
        }
        if total.saturating_add(metadata.len()) > MAX_TOTAL_BYTES {
            break;
        }
        let content = read_bounded(&item.path, MAX_FILE_BYTES)?;
        let redacted = redact(&content);
        archive
            .start_file(&item.archive_name, options)
            .map_err(|error| DiagnosticError::Archive(error.to_string()))?;
        archive
            .write_all(redacted.as_bytes())
            .map_err(DiagnosticError::Io)?;
        total = total.saturating_add(redacted.len() as u64);
    }
    archive
        .start_file("diagnostic-summary.txt", options)
        .map_err(|error| DiagnosticError::Archive(error.to_string()))?;
    let summary = summarize_instance(instance)?;
    writeln!(archive, "MFMCL diagnostic summary")?;
    writeln!(archive, "Instance: {}", safe_name(&instance.name))?;
    writeln!(archive, "Minecraft: {}", safe_name(&instance.game_version))?;
    writeln!(
        archive,
        "Finding: {} ({})",
        summary.title, summary.confidence
    )?;
    writeln!(archive, "{}", summary.explanation)?;
    for item in summary.evidence {
        writeln!(archive, "- {}", item)?;
    }
    writeln!(
        archive,
        "\nOnly local log snippets are included. Credentials are redacted."
    )?;
    archive
        .finish()
        .map_err(|error| DiagnosticError::Archive(error.to_string()))?;
    Ok(archive_path)
}

fn discover_logs(instance: &InstanceRecord) -> Vec<DiagnosticFile> {
    let mut files = Vec::new();
    add_candidate(
        &mut files,
        &instance.game_dir.join("logs").join("latest.log"),
        "game-latest.log",
    );
    let crash_dir = instance.game_dir.join("crash-reports");
    add_directory_files(&mut files, &crash_dir, "crash-reports", |name| {
        name.ends_with(".txt")
    });
    add_directory_files(&mut files, &instance.game_dir, "game", |name| {
        name.starts_with("hs_err_pid") && name.ends_with(".log")
    });
    add_candidate(
        &mut files,
        &instance.game_dir.join("debug.log"),
        "debug.log",
    );
    add_directory_files(
        &mut files,
        &instance.instance_dir.join("logs"),
        "launcher",
        |name| name.ends_with(".log"),
    );
    files.sort_by_key(|file| std::cmp::Reverse(file.modified));
    files
}

fn add_candidate(files: &mut Vec<DiagnosticFile>, path: &Path, archive_name: &str) {
    let metadata = std::fs::symlink_metadata(path).ok();
    let modified = metadata
        .as_ref()
        .and_then(|metadata| metadata.modified().ok());
    if metadata.is_some_and(|metadata| metadata.file_type().is_file()) {
        files.push(DiagnosticFile {
            path: path.to_path_buf(),
            archive_name: archive_name.to_owned(),
            modified,
        });
    }
}

fn add_directory_files<F>(
    files: &mut Vec<DiagnosticFile>,
    directory: &Path,
    archive_prefix: &str,
    predicate: F,
) where
    F: Fn(&str) -> bool,
{
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        if files.len() >= MAX_FILES * 2 {
            break;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let path = entry.path();
        let Some(name) = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
        else {
            continue;
        };
        if !predicate(&name) {
            continue;
        }
        let modified = std::fs::metadata(&path)
            .ok()
            .and_then(|metadata| metadata.modified().ok());
        files.push(DiagnosticFile {
            path,
            archive_name: format!("{archive_prefix}/{name}"),
            modified,
        });
    }
}

fn read_bounded(path: &Path, max_bytes: u64) -> Result<String, DiagnosticError> {
    let file = File::open(path)?;
    let mut buffer = Vec::new();
    file.take(max_bytes).read_to_end(&mut buffer)?;
    Ok(String::from_utf8_lossy(&buffer).into_owned())
}

fn redact(text: &str) -> String {
    crate::logs::redact_log(text, &[])
}

fn unique_archive_path(directory: &Path) -> PathBuf {
    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let mut candidate = directory.join(format!("MFMCL-diagnostics-{timestamp}.zip"));
    let mut suffix = 1;
    while candidate.exists() {
        candidate = directory.join(format!("MFMCL-diagnostics-{timestamp}-{suffix}.zip"));
        suffix += 1;
    }
    candidate
}

fn safe_name(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(100)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::ZipArchive;

    #[test]
    fn summarizes_and_exports_bounded_redacted_logs() {
        let root =
            std::env::temp_dir().join(format!("mfmcl-diagnostic-test-{}", uuid::Uuid::new_v4()));
        let game_dir = root.join("game");
        let destination = root.join("export");
        std::fs::create_dir_all(game_dir.join("logs")).unwrap();
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::write(
            game_dir.join("logs/latest.log"),
            "java.lang.OutOfMemoryError: Java heap space\naccessToken=secret-token\n",
        )
        .unwrap();
        let instance = InstanceRecord {
            id: "test-instance".to_owned(),
            name: "Test".to_owned(),
            game_version: "1.21.4".to_owned(),
            loader: "vanilla".to_owned(),
            instance_dir: root.join("instance"),
            game_dir,
            isolated: true,
            state: crate::InstanceState::Failed,
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            last_launched_at: None,
        };
        let summary = summarize_instance(&instance).unwrap();
        assert_eq!(summary.title, "内存不足");
        let archive_path = export_instance_diagnostics(&instance, &destination).unwrap();
        let file = File::open(&archive_path).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
        let mut log = String::new();
        archive
            .by_name("game-latest.log")
            .unwrap()
            .read_to_string(&mut log)
            .unwrap();
        assert!(!log.contains("secret-token"));
        assert!(log.contains("[REDACTED]"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn redacts_common_credentials() {
        let result = redact("accessToken=secret123 username=Alex");
        assert!(!result.contains("secret123"));
        assert!(result.contains("[REDACTED]"));
        assert!(result.contains("username=Alex"));
        let json_result = redact(r#"{"accessToken":"secret-json", "name":"Alex"}"#);
        assert!(!json_result.contains("secret-json"));
        let bearer_result = redact("Authorization: Bearer secret-bearer");
        assert!(!bearer_result.contains("secret-bearer"));
    }
    /// 建一个临时实例，`logs/latest.log` 写入给定内容。
    fn instance_with_log(lines: &str) -> (InstanceRecord, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("mfmcl-diag-{}", uuid::Uuid::new_v4()));
        let game_dir = root.join("game");
        std::fs::create_dir_all(game_dir.join("logs")).unwrap();
        std::fs::write(game_dir.join("logs/latest.log"), lines).unwrap();
        let instance = InstanceRecord {
            id: "diag".to_owned(),
            name: "Diag".to_owned(),
            game_version: "1.20.1".to_owned(),
            loader: "fabric".to_owned(),
            instance_dir: root.join("instance"),
            game_dir,
            isolated: true,
            state: crate::InstanceState::Failed,
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            last_launched_at: None,
        };
        (instance, root)
    }

    #[test]
    fn recognises_common_crash_patterns_with_confidence() {
        let cases = [
            ("java.lang.OutOfMemoryError: Java heap space", "内存不足"),
            (
                "java.lang.UnsupportedClassVersionError: foo",
                "Java 版本不匹配",
            ),
            (
                "Missing or unsupported mandatory dependencies:",
                "模组缺少前置",
            ),
            ("GL_OUT_OF_MEMORY", "图形内存不足"),
            (
                "The directories below appear to be extracted jar files.",
                "模组文件被解压",
            ),
            ("Unrecognized option: -XX:+Bogus", "JVM 参数无效"),
        ];
        for (log, expected) in cases {
            let (instance, root) = instance_with_log(log);
            let summary = summarize_instance(&instance).unwrap();
            assert_eq!(summary.title, expected, "日志 {log} 应识别为 {expected}");
            assert!(
                summary.confidence == "high" || summary.confidence == "medium",
                "命中规则时置信度不应是 {}",
                summary.confidence
            );
            // 每条命中都必须带建议，否则用户知道原因也不知道怎么修。
            assert!(
                !summary.reasons[0].suggestions.is_empty(),
                "{expected} 缺少可执行建议"
            );
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn reports_multiple_reasons_sorted_by_confidence() {
        // 真实崩溃常常同时满足多条：内存不足会连带触发 Mixin 失败。
        let (instance, root) = instance_with_log(
            "java.lang.OutOfMemoryError: Java heap space\nMixin apply failed: mixin.foo from mod sodium",
        );
        let summary = summarize_instance(&instance).unwrap();
        assert!(summary.reasons.len() >= 2, "应收集到多条原因");
        assert_eq!(summary.reasons[0].confidence, "high", "高置信度排在最前");
        assert_eq!(summary.reasons[0].title, "内存不足");
        assert!(summary.explanation.contains("另有"), "说明存在多条原因");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extracts_mod_name_from_loader_message() {
        let (instance, root) = instance_with_log(
            "[Render thread/ERROR]: Caught exception from create (com.simibubi.create.Create)",
        );
        let summary = summarize_instance(&instance).unwrap();
        let reason = summary
            .reasons
            .iter()
            .find(|r| r.title == "确定是某个模组导致崩溃")
            .expect("应命中模组导致的崩溃");
        assert_eq!(reason.mods.first().map(String::as_str), Some("create"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unless_clause_separates_memory_rules() {
        // 「32 位 Java 上限」与「普通内存不足」指向不同的处理方式，不能混淆。
        let (instance, root) =
            instance_with_log("Could not reserve enough space for 1048576KB object heap");
        let summary = summarize_instance(&instance).unwrap();
        assert_eq!(summary.title, "内存分配失败");

        let (instance2, root2) = instance_with_log(
            "Invalid maximum heap size: -Xmx99999999999\nCould not reserve enough space",
        );
        let summary2 = summarize_instance(&instance2).unwrap();
        assert_eq!(
            summary2.title, "内存参数超出 Java 可分配范围",
            "同时出现时以更具体的那条为准"
        );
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(root2).unwrap();
    }

    #[test]
    fn unknown_log_says_so_instead_of_guessing() {
        let (instance, root) = instance_with_log("Starting Minecraft\nLoaded 12 mods\nStopping");
        let summary = summarize_instance(&instance).unwrap();
        assert_eq!(summary.confidence, "unknown");
        assert!(summary.reasons.is_empty(), "没有命中就不该编造原因");
        assert!(summary.explanation.contains("导出诊断包"), "应给出下一步");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_logs_report_none_not_unknown() {
        let root = std::env::temp_dir().join(format!("mfmcl-diag-{}", uuid::Uuid::new_v4()));
        let game_dir = root.join("game");
        std::fs::create_dir_all(&game_dir).unwrap();
        let instance = InstanceRecord {
            id: "diag".to_owned(),
            name: "Diag".to_owned(),
            game_version: "1.20.1".to_owned(),
            loader: "vanilla".to_owned(),
            instance_dir: root.join("instance"),
            game_dir,
            isolated: true,
            state: crate::InstanceState::NotInstalled,
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            last_launched_at: None,
        };
        let summary = summarize_instance(&instance).unwrap();
        // 「没有日志」和「有日志但认不出」是两件事，用户要采取的行动不同。
        assert_eq!(summary.confidence, "none");
        assert_eq!(summary.files_found, 0);
        std::fs::remove_dir_all(root).unwrap();
    }
}
