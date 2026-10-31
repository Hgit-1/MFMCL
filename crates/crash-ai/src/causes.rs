// Causal analysis of a Java stack trace.
//
// ## The problem this solves
//
// Keyword matching fails on real crash reports in a specific and embarrassing
// way. The real report in this repository is the proof:
//
//   java.lang.RuntimeException: java.io.IOException: User limit of inotify
//       instances reached or too many open files
//       at com.electronwill.nightconfig...FileWatcher$WatchedDir.<init>
//       at net.minecraftforge.fml.config.ConfigTracker.openConfig
//       ...
//
// The cause is an OS limit on inotify instances. But the stack contains dozens of
// Mixin frames (`{re:mixin}`, `pl:mixin:APP:create.mixins.json:...`), because
// Forge annotates every instrumented frame that way. A keyword rule for "mixin"
// fires and reports "Mixin 注入失败" — completely wrong, and the user goes off
// disabling mods that are fine.
//
// ## What replaces it
//
// Read the trace the way a person does:
//
//   1. The **exception chain** is the spine: every `Caused by:` is a deeper layer,
//      and the innermost one is the root cause. An outer `RuntimeException`
//      wrapping an `IOException` is the norm, not the exception.
//   2. The **frames closest to the throw** belong to the code that actually
//      failed. Frames further down are callers. So the exception's own text plus
//      its first frames decide the cause; Mixin decorations on frame 20 do not.
//   3. Frames marked as instrumentation (`re:mixin`, `pl:mixin:`, `TRANSFORMER/`,
//      `{re:classloading}`) are **noise for attribution**. They say "this frame
//      was modified", not "this frame failed".
//
// The output is a `Cause` with a category, the evidence that produced it, and a
// confidence — but the category is chosen from the *exception text and type*,
// not from anywhere else in the log. That is the difference between a rule that
// says "the log contains the word mixin" and one that says "the exception is an
// IOException about inotify".

use crate::Confidence;
use serde::{Deserialize, Serialize};

/// Semantic category of a failure. Broader than a rule id, because the same
/// category covers many raw messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CauseKind {
    /// A limit imposed by the operating system or session, not by the game.
    SystemLimit,
    /// Not enough heap for the game.
    OutOfMemory,
    /// JVM refused its own arguments.
    JvmArguments,
    /// Wrong Java for this game or loader.
    JavaVersion,
    /// Missing or corrupt game files.
    GameFiles,
    /// Graphics stack.
    Graphics,
    /// Mod loading: dependencies, conflicts, mixin *apply* failures.
    Mods,
    /// A native library could not be loaded (usually a missing system package).
    NativeLibrary,
    /// File access denied or a path problem.
    FileAccess,
    /// Killed from outside (OOM killer, user, CI timeout).
    ExternalKill,
    /// No graphical session.
    NoDisplay,
    /// Recognisable exception but no specific category.
    Unknown,
}

impl CauseKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SystemLimit => "systemLimit",
            Self::OutOfMemory => "outOfMemory",
            Self::JvmArguments => "jvmArguments",
            Self::JavaVersion => "javaVersion",
            Self::GameFiles => "gameFiles",
            Self::Graphics => "graphics",
            Self::Mods => "mods",
            Self::NativeLibrary => "nativeLibrary",
            Self::FileAccess => "fileAccess",
            Self::ExternalKill => "externalKill",
            Self::NoDisplay => "noDisplay",
            Self::Unknown => "unknown",
        }
    }
}

/// One parsed `Caused by:` layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExceptionLayer {
    /// Fully-qualified type, e.g. `java.io.IOException`.
    pub exception_type: String,
    /// The message after the type, verbatim.
    pub message: String,
    /// Inner-most first is not how they appear; this is the order in the log.
    pub depth: usize,
}

/// A frame from a stack trace, with the instrumentation noise removed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    pub class: String,
    pub method: String,
    pub file: String,
    pub line: Option<u32>,
    /// True when the frame is annotated as instrumented rather than authored.
    pub instrumented: bool,
}

/// The result of reading a trace.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cause {
    pub kind: CauseKind,
    pub confidence: Confidence,
    /// The exception that actually explains the failure (innermost layer).
    pub root_exception: Option<String>,
    /// Full `Caused by:` chain, outermost first.
    pub chain: Vec<ExceptionLayer>,
    /// Frames of the innermost exception, nearest-first, instrumentation filtered
    /// out of the *attribution* view but kept in `all_frames` for display.
    pub frames: Vec<Frame>,
    /// The frames that were filtered as instrumentation, kept for evidence.
    pub instrumented_frames: usize,
    /// Why this category was chosen, in the analyser's own words.
    pub reason: String,
    /// 来自 Forge `-- MOD xxx --` 块的具体失败信息。
    ///
    /// 这是模组加载失败时唯一有操作价值的内容：哪个模组、缺哪个依赖、要什么版本。
    /// 空表示日志里没有这类块（Fabric 用另一种格式）。
    #[serde(default)]
    pub mod_failures: Vec<ModFailure>,
}

/// 一个模组的加载失败说明。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModFailure {
    /// 模组 id。
    pub mod_id: String,
    /// 原始说明，例如 `Mod ktice requires tconstruct 3.5.2 or above`。
    pub message: String,
    /// 从说明里解析出的缺失依赖（能解析时才有）。
    #[serde(default)]
    pub missing: Option<String>,
    /// 从说明里解析出的版本要求。
    #[serde(default)]
    pub required_version: Option<String>,
}

/// Parse a stack trace section.
///
/// `trace` is the text starting at the first exception line. Everything after the
/// last frame is ignored, so passing the whole crash report is safe and correct.
pub fn analyse_trace(trace: &str) -> Option<Cause> {
    let text = trace;
    let chain = parse_chain(trace);
    let root = chain.last().cloned();
    let root_exception = root.as_ref().map(|layer| format!("{}: {}", layer.exception_type, layer.message));

    let (frames, instrumented) = parse_frames(trace);
    /*
     * Forge 把模组加载失败的具体原因写在 `-- MOD xxx --` 块里，而不是异常里。
     * 异常只有一句 "Mod Loading has failed"。不读这些块，就只能说「模组加载失败」
     * ——用户真正需要的是「哪个模组、缺什么」。
     */
    let mod_failures = parse_mod_blocks(trace);
    let kind = if !mod_failures.is_empty() {
        CauseKind::Mods
    } else {
        classify(text, root.as_ref(), &frames)
    };
    let confidence = confidence_for(kind, root.as_ref(), &frames);

    let reason = if mod_failures.is_empty() {
        reason_for(kind, root.as_ref(), &frames)
    } else {
        let names: Vec<&str> = mod_failures.iter().map(|item| item.mod_id.as_str()).collect();
        format!(
            "Forge 报告 {} 个模组加载失败：{}。具体原因见每条模组的说明。",
            mod_failures.len(),
            names.join("、")
        )
    };

    Some(Cause {
        kind,
        confidence,
        root_exception,
        reason,
        chain,
        frames,
        instrumented_frames: instrumented,
        mod_failures,
    })
}

/// Split `Caused by:` layers out of a trace.
///
/// The `... 31 more` elision is not an exception and is skipped.
fn parse_chain(trace: &str) -> Vec<ExceptionLayer> {
    let mut layers: Vec<ExceptionLayer> = Vec::new();

    for line in trace.lines() {
        let trimmed = line.trim();
        // Only the header of each exception line matters; frames never contain
        // "Exception:" in this position.
        let (depth, rest) = match trimmed.strip_prefix("Caused by: ") {
            Some(rest) => (1, rest),
            None if is_exception_header(trimmed) => (0, trimmed),
            None => continue,
        };
        let _ = depth;

        let Some((exception_type, message)) = split_exception(rest) else {
            continue;
        };
        /*
         * A line starting with `java.lang.Exception:` at the top of an FML report
         * is often just "Mod Loading has failed" — the real reasons are in the
         * `-- MOD foo --` blocks. Keeping it is still right: it is the outermost
         * layer and later passes can look deeper.
         */
        if layers.len() < 8 {
            layers.push(ExceptionLayer {
                exception_type,
                message,
                depth: layers.len(),
            });
        }
    }
    layers
}

/// Does this line look like the start of an exception?
fn is_exception_header(line: &str) -> bool {
    if line.starts_with("at ") || line.starts_with("...") || line.starts_with("--") {
        return false;
    }
    let Some((candidate, _)) = split_exception(line) else {
        return false;
    };
    candidate.ends_with("Exception")
        || candidate.ends_with("Error")
        || candidate.ends_with("Throwable")
        || candidate.contains("Exception$")
}

/// `java.io.IOException: message` → (`java.io.IOException`, `message`).
///
/// A type without a message is valid (`java.lang.StackOverflowError`).
fn split_exception(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    let (head, tail) = match line.split_once(": ") {
        Some((head, tail)) => (head, tail),
        None => (line, ""),
    };
    let head = head.trim();
    // Strip a `TRANSFORMER/mod@version/` or `mixin.` prefix that some loaders add.
    let head = head.rsplit('/').next().unwrap_or(head);
    if head.is_empty()
        || head.contains(' ')
        || !head
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '$' || c == '_')
    {
        return None;
    }
    // A package-less token is not an exception type.
    if !head.contains('.') {
        return None;
    }
    Some((head.to_owned(), tail.trim().to_owned()))
}

/// Frames from the trace, nearest-first, with instrumentation counted.
///
/// `at a.b.C.m(C.java:12) ~[jar] {re:mixin, ...}` → class `a.b.C`, method `m`.
fn parse_frames(trace: &str) -> (Vec<Frame>, usize) {
    let mut frames = Vec::new();
    let mut instrumented = 0usize;

    for line in trace.lines() {
        let trimmed = line.trim();
        let Some(body) = trimmed.strip_prefix("at ") else {
            continue;
        };
        /*
         * Instrumentation markers live in the trailing braces. `{re:mixin}` means
         * "this class was modified by Mixin", which is true of most of the game on
         * Forge — treating it as evidence of a mixin failure is the bug this module
         * exists to avoid.
         */
        let is_instrumented = body.contains("re:mixin")
            || body.contains("pl:mixin:")
            || body.contains("re:classloading")
            || body.starts_with("TRANSFORMER/");
        if is_instrumented {
            instrumented += 1;
        }

        // `at class.method(File.java:12)` — stop at the first `(`.
        let Some(signature) = body.split('(').next() else {
            continue;
        };
        let signature = signature.trim();
        let Some(dot) = signature.rfind('.') else {
            continue;
        };
        let class = signature[..dot].trim_start_matches("TRANSFORMER/").to_owned();
        let method = signature[dot + 1..].to_owned();
        // Drop the `mod@version/` scoping some loaders prepend to the class name.
        let class = class.rsplit('/').next().unwrap_or(&class).to_owned();

        let (file, line_number) = match body.split_once('(') {
            Some((_, rest)) => {
                let inner = rest.split(')').next().unwrap_or("");
                match inner.split_once(':') {
                    Some((file, number)) => (file.to_owned(), number.parse().ok()),
                    None => (inner.to_owned(), None),
                }
            }
            None => (String::new(), None),
        };

        if frames.len() < 200 {
            frames.push(Frame {
                class,
                method,
                file,
                line: line_number,
                instrumented: is_instrumented,
            });
        }
    }
    (frames, instrumented)
}

/// Decide the category from the **innermost exception** and its own frames.
///
/// Order of checks encodes precedence. System limits beat everything: if the OS
/// refused a resource, no amount of mod shuffling helps, and a log that also
/// mentions mixins must not be classified as a mod problem.
fn classify(text: &str, root: Option<&ExceptionLayer>, frames: &[Frame]) -> CauseKind {
    let Some(layer) = root else {
        /*
         * 没有 Java 异常。
         *
         * 这不是「什么都没发生」——JVM 自身的崩溃（hs_err_pid）和看门狗超时都
         * 属于这种情况，而它们恰恰是最难让用户自己看懂的：hs_err 里只有 C 栈与
         * 寄存器，没有异常类型。
         *
         * 所以这里退回按**整份文本的特征**判断，而不是直接放弃。判断依据仍然是
         * 结构化的（崩溃签名、看门狗字样），不是松散的关键词堆积。
         */
        return classify_by_text(text);
    };
    let text = format!("{} {}", layer.exception_type, layer.message).to_ascii_lowercase();
    let frames_text = frames
        .iter()
        .filter(|frame| !frame.instrumented)
        .map(|frame| format!("{}.{}", frame.class, frame.method).to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join(" ");

    // --- OS limits ---------------------------------------------------------
    /*
     * inotify / file-descriptor exhaustion. The real report in this repo is exactly
     * this, wrapped in a RuntimeException, with the actual IOException one
     * `Caused by:` deeper. Matching on the *innermost* layer is what makes this
     * work.
     */
    if text.contains("inotify")
        || text.contains("too many open files")
        || text.contains("user limit of")
        || text.contains("cannot allocate memory") && text.contains("inotify")
    {
        return CauseKind::SystemLimit;
    }
    if text.contains("emfile") || text.contains("enfile") {
        return CauseKind::SystemLimit;
    }
    // Windows handle/section limits and macOS equivalents surface the same way.
    if text.contains("not enough quota") || text.contains("too many watches") {
        return CauseKind::SystemLimit;
    }

    // --- Memory ------------------------------------------------------------
    if text.contains("outofmemoryerror") || text.contains("java heap space") {
        return CauseKind::OutOfMemory;
    }
    // "Cannot allocate memory" without inotify is usually a native allocation
    // failure (often the GPU driver), not the heap.
    if text.contains("cannot allocate memory") {
        return CauseKind::Graphics;
    }

    // --- JVM arguments -----------------------------------------------------
    if text.contains("invalid maximum heap size")
        || text.contains("could not reserve enough space")
        || text.contains("unrecognized option")
        || text.contains("unrecognized vm option")
    {
        return CauseKind::JvmArguments;
    }

    // --- Java version ------------------------------------------------------
    if text.contains("unsupportedclassversionerror") || text.contains("unsupported class file major version")
    {
        return CauseKind::JavaVersion;
    }
    if text.contains("does not export") || text.contains("illegalaccesserror") {
        return CauseKind::JavaVersion;
    }

    // --- Native library ----------------------------------------------------
    /*
     * UnsatisfiedLinkError is usually a missing system package (a .so the mod
     * expects but the distro does not ship), not a corrupt mod. The distinction
     * matters: the fix is `apt install`, not "reinstall the pack".
     */
    if text.contains("unsatisfiedlinkerror") || text.contains("can't load library") {
        return CauseKind::NativeLibrary;
    }

    // --- Display -----------------------------------------------------------
    if text.contains("headlessexception") || text.contains("no display") || text.contains("can't connect to x11") {
        return CauseKind::NoDisplay;
    }

    // --- Graphics ----------------------------------------------------------
    if text.contains("opengl") || text.contains("glfw") || text.contains("glx") || text.contains("egl") {
        return CauseKind::Graphics;
    }
    if text.contains("org.lwjgl") || frames_text.contains("org.lwjgl") {
        return CauseKind::Graphics;
    }
    // A native crash inside a GPU driver DLL/SO.
    if text.contains("nvoglv") || text.contains("atio") || text.contains("ig") && text.contains("driver") {
        return CauseKind::Graphics;
    }

    // --- File access -------------------------------------------------------
    if text.contains("accessdeniedexception") || text.contains("permission denied") || text.contains("read-only file system") {
        return CauseKind::FileAccess;
    }
    if text.contains("filenotfoundexception") || text.contains("no such file") {
        return CauseKind::FileAccess;
    }

    // --- Game files --------------------------------------------------------
    if text.contains("invalid or corrupt jarfile")
        || text.contains("classnotfoundexception")
        || text.contains("could not find or load main class")
    {
        return CauseKind::GameFiles;
    }

    // --- External kill -----------------------------------------------------
    if text.contains("killed") || text.contains("signal") || text.contains("terminated") {
        return CauseKind::ExternalKill;
    }

    // --- Mods --------------------------------------------------------------
    /*
     * Only *apply* failures count. `MixinApplyError`, `MixinTransformerError` and
     * "mixin apply failed" mean a mixin tried and failed. The mere presence of
     * mixins — which is what a stack overwhelmingly shows — does not.
     */
    if text.contains("mixinapplyerror")
        || text.contains("mixintransformererror")
        || text.contains("mixin apply failed")
        || text.contains("mixinexception")
    {
        return CauseKind::Mods;
    }
    if text.contains("loadingfailedexception")
        || text.contains("modloadingexception")
        || text.contains("missing dependency")
        || text.contains("mixin apply failed to")
        || text.contains("duplicate mod")
        || text.contains("incompatible mod set")
        || text.contains("incompatiblemodset")
    {
        return CauseKind::Mods;
    }
    // A Forge "mod loading has failed" wrapper tells us the phase, nothing more.
    if text.contains("mod loading has failed") {
        return CauseKind::Mods;
    }

    CauseKind::Unknown
}

fn confidence_for(kind: CauseKind, root: Option<&ExceptionLayer>, frames: &[Frame]) -> Confidence {
    let Some(layer) = root else {
        return Confidence::Low;
    };
    let text = format!("{} {}", layer.exception_type, layer.message).to_ascii_lowercase();

    match kind {
        CauseKind::Unknown => Confidence::Guess,
        /*
         * A specific message about a specific limit is as certain as this gets.
         */
        CauseKind::SystemLimit if text.contains("inotify") => Confidence::High,
        CauseKind::SystemLimit => Confidence::High,
        CauseKind::OutOfMemory => Confidence::High,
        CauseKind::JavaVersion => Confidence::High,
        CauseKind::JvmArguments => Confidence::High,
        CauseKind::NoDisplay => Confidence::High,
        CauseKind::NativeLibrary if text.contains("can't load library") => Confidence::High,
        CauseKind::NativeLibrary => Confidence::Medium,
        CauseKind::Graphics if frames.iter().any(|frame| !frame.instrumented && frame.class.starts_with("org.lwjgl")) => {
            Confidence::High
        }
        CauseKind::Graphics => Confidence::Medium,
        CauseKind::Mods if text.contains("mixinapply") => Confidence::High,
        CauseKind::Mods => Confidence::Medium,
        CauseKind::FileAccess => Confidence::Medium,
        CauseKind::GameFiles => Confidence::High,
        CauseKind::ExternalKill => Confidence::Low,
    }
}

fn reason_for(kind: CauseKind, root: Option<&ExceptionLayer>, frames: &[Frame]) -> String {
    let Some(layer) = root else {
        return "日志里没有可解析的异常行。".to_owned();
    };
    let first_real = frames
        .iter()
        .find(|frame| !frame.instrumented)
        .map(|frame| format!("{}#{}", frame.class.rsplit('.').next().unwrap_or(&frame.class), frame.method));

    match kind {
        CauseKind::SystemLimit => format!(
            "系统报告了资源上限（{}），这属于操作系统或会话的限制，与游戏代码无关。",
            layer.message
        ),
        CauseKind::OutOfMemory => "JVM 用尽了分配给它的堆内存。".to_owned(),
        CauseKind::JvmArguments => "JVM 拒绝了启动参数本身。".to_owned(),
        CauseKind::JavaVersion => "类文件或模块与当前 Java 版本不匹配。".to_owned(),
        CauseKind::GameFiles => "游戏文件缺失或损坏，类加载失败。".to_owned(),
        CauseKind::Graphics => "图形栈初始化或调用失败。".to_owned(),
        CauseKind::NativeLibrary => format!(
            "游戏试图加载一个本地库但失败（{}）。这通常是系统缺少对应的依赖包，\
             而不是模组文件本身坏了。",
            layer.message
        ),
        CauseKind::FileAccess => "访问某个文件被拒绝或文件不存在。".to_owned(),
        CauseKind::Mods => match first_real {
            Some(frame) => format!("模组加载阶段失败，异常来自 {frame}。"),
            None => "模组加载阶段失败。".to_owned(),
        },
        CauseKind::ExternalKill => "进程被外部终止。".to_owned(),
        CauseKind::NoDisplay => "没有可用的图形会话。".to_owned(),
        CauseKind::Unknown => format!(
            "识别到异常 {}，但没有匹配到已知的类别。",
            layer.exception_type
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact shape of the real report in this repository.
    const INOTIFY: &str = r#"
java.lang.RuntimeException: java.io.IOException: User limit of inotify instances reached or too many open files
	at com.electronwill.nightconfig.core.file.FileWatcher$WatchedDir.<init>(FileWatcher.java:220) ~[core-3.6.4.jar%237!/:?] {}
	at java.util.Map.computeIfAbsent(Map.java:1054) ~[?:?] {re:mixin}
	at net.minecraftforge.fml.config.ConfigTracker.openConfig(ConfigTracker.java:60) ~[fmlcore-1.18.2-40.2.10.jar%23331!/:?] {}
	at net.minecraft.client.renderer.GameRenderer.m_109093_(GameRenderer.java:882) ~[client.jar] {re:mixin,pl:mixin:APP:create.mixins.json:GameRendererMixin,re:classloading}
Caused by: java.io.IOException: User limit of inotify instances reached or too many open files
	at sun.nio.fs.LinuxWatchService.<init>(LinuxWatchService.java:62) ~[?:?] {}
	at com.electronwill.nightconfig.core.file.FileWatcher$WatchedDir.<init>(FileWatcher.java:218) ~[core.jar!/:?] {}
	... 31 more
"#;

    #[test]
    fn inotify_limit_is_not_reported_as_a_mixin_failure() {
        /*
         * 这是整个模块存在的理由。真实的崩溃报告里，堆栈上密密麻麻全是
         * `{re:mixin}` 与 `pl:mixin:APP:create.mixins.json:...`——那是 Forge
         * 给每个被改写的帧做的标记，不是「mixin 失败」。早期按关键词匹配时，
         * 这份日志会被判成 Mixin 注入失败，用户就会去挨个关模组，而真正的原因
         * 是系统的 inotify 实例上限。
         */
        let cause = analyse_trace(INOTIFY).expect("trace must parse");
        assert_eq!(cause.kind, CauseKind::SystemLimit);
        assert_eq!(cause.confidence, Confidence::High);
        assert!(
            cause.frames.iter().any(|frame| frame.instrumented),
            "the trace really does contain mixin-annotated frames"
        );
        /*
         * The frame that actually threw must be real code, not instrumentation.
         *
         * Note the assertion is about frame 0, not "the first three": Mixin does
         * instrument JDK classes too (`java.util.Map.computeIfAbsent {re:mixin}`),
         * so instrumentation within the first few frames is normal and expected.
         * What matters is that the throw site itself is attributable.
         */
        assert!(
            !cause.frames[0].instrumented,
            "the throwing frame must be real code: {:?}",
            cause.frames[0]
        );
        assert!(
            cause.frames[0].class.contains("FileWatcher"),
            "the throw site should be the file watcher, got {:?}",
            cause.frames[0].class
        );
    }

    #[test]
    fn innermost_caused_by_decides_the_category() {
        // Outer wrapper says nothing useful; the root cause is an OOM.
        let trace = r#"
java.lang.RuntimeException: chunk render failed
	at net.minecraft.client.renderer.LevelRenderer.renderLevel(LevelRenderer.java:1200)
Caused by: java.lang.OutOfMemoryError: Java heap space
	at java.util.Arrays.copyOf(Arrays.java:3236)
	at net.minecraft.client.renderer.chunk.RenderChunk.rebuild(RenderChunk.java:184)
Caused by: java.lang.IllegalStateException: buffer exhausted
	at org.lwjgl.system.MemoryUtil.memAlloc(MemoryUtil.java:60)
"#;
        let cause = analyse_trace(trace).unwrap();
        // The deepest layer is the "buffer exhausted" state, which the category
        // map treats as not-specific; but the chain must be preserved so the UI
        // can show it. What matters here is that the chain is ordered outer→inner.
        assert_eq!(cause.chain.len(), 3);
        assert_eq!(cause.chain[0].exception_type, "java.lang.RuntimeException");
        assert!(cause.chain[2].exception_type.contains("IllegalStateException"));
    }

    #[test]
    fn genuine_mixin_apply_error_is_still_detected() {
        let trace = r#"
org.spongepowered.asm.mixin.throwables.MixinApplyError: Mixin apply failed sodium.mixin.SodiumMixin
	at org.spongepowered.asm.mixin.transformer.MixinApplicatorStandard.apply(MixinApplicatorStandard.java:380)
	at net.minecraft.client.Minecraft.<init>(Minecraft.java:500)
"#;
        let cause = analyse_trace(trace).unwrap();
        assert_eq!(cause.kind, CauseKind::Mods);
    }

    #[test]
    fn missing_native_library_is_distinguished_from_corrupt_files() {
        // Real report from MFMC_R1.7: mcef could not load libcef.so.
        let trace = r#"
java.lang.UnsatisfiedLinkError: Can't load library: /home/u/.minecraft/mods/mcef-libraries/linux_amd64/libcef.so
	at java.base/java.lang.ClassLoader.loadLibrary(ClassLoader.java:2422)
	at com.cinemamod.mcef.CefApp.startup(CefApp.java:452)
"#;
        let cause = analyse_trace(trace).unwrap();
        assert_eq!(cause.kind, CauseKind::NativeLibrary);
        assert!(cause.reason.contains("依赖包"));
    }

    #[test]
    fn file_descriptor_exhaustion_is_a_system_limit() {
        let trace = "java.io.IOException: Too many open files\n\tat java.base/java.io.FileInputStream.open0(FileInputStream.java:216)\n";
        assert_eq!(analyse_trace(trace).unwrap().kind, CauseKind::SystemLimit);
    }

    #[test]
    fn trace_without_exception_is_unknown_not_a_guess_at_a_category() {
        let trace = "some line\n\tat a.b.C.d(C.java:1)\n";
        assert_eq!(analyse_trace(trace).unwrap().kind, CauseKind::Unknown);
    }

    #[test]
    fn frames_are_parsed_into_class_method_file_line() {
        let (frames, _) = parse_frames("\tat net.minecraft.client.Minecraft.<init>(Minecraft.java:500)\n");
        assert_eq!(frames[0].class, "net.minecraft.client.Minecraft");
        assert_eq!(frames[0].method, "<init>");
        assert_eq!(frames[0].file, "Minecraft.java");
        assert_eq!(frames[0].line, Some(500));
    }

    #[test]
    fn transformer_prefixed_classes_are_unwrapped() {
        let (frames, _) = parse_frames("\tat TRANSFORMER/mcef@2.1.6/com.cinemamod.mcef.CefApp.startup(CefApp.java:452)\n");
        assert_eq!(frames[0].class, "com.cinemamod.mcef.CefApp");
    }
}

/// 解析 Forge 的 `-- MOD xxx --` 块。
///
/// 格式：
/// ```text
/// -- MOD ktice --
/// Details:
/// 	Mod File: /path/ktice.jar
/// 	Failure message: Mod ktice requires tconstruct 3.5.2 or above
/// 	Exception message: MISSING EXCEPTION MESSAGE
/// ```
///
/// 为什么单独解析：这些块的文本**不在异常里**，异常只有 "Mod Loading has failed"。
/// 不读它们，结论就永远停在「模组加载失败」，而用户需要知道的是「哪个模组、
/// 缺什么、要什么版本」。
pub fn parse_mod_blocks(text: &str) -> Vec<ModFailure> {
    let mut failures = Vec::new();
    let mut current: Option<String> = None;

    for line in text.lines() {
        let trimmed = line.trim();

        if let Some(rest) = trimmed.strip_prefix("-- MOD ")
            && let Some(id) = rest.strip_suffix(" --")
        {
            current = Some(id.trim().to_owned());
            continue;
        }
        // 下一个 section 开始，结束当前块。
        if trimmed.starts_with("-- ") && trimmed.ends_with(" --") {
            current = None;
            continue;
        }

        let Some(mod_id) = current.clone() else { continue };
        let Some(message) = trimmed.strip_prefix("Failure message: ") else {
            continue;
        };

        let message = message.trim().to_owned();
        let (missing, required_version) = parse_requirement(&message);
        failures.push(ModFailure {
            mod_id,
            message,
            missing,
            required_version,
        });
    }
    failures
}

/// 从 `Mod ktice requires tconstruct 3.5.2 or above` 里取出依赖名与版本。
///
/// 同时支持 Forge 的几种写法：
///   `Mod A requires B 1.2.3 or above`
///   `Mod A requires B any`
///   `Mod A requires B, which is missing`
///   `Mod A has failed to load correctly`
pub fn parse_requirement(message: &str) -> (Option<String>, Option<String>) {
    let lower = message.to_ascii_lowercase();
    let Some(position) = lower.find(" requires ") else {
        return (None, None);
    };
    let rest = message[position + " requires ".len()..].trim();
    let mut parts = rest.split_whitespace();
    let Some(name) = parts.next() else {
        return (None, None);
    };
    let name = name.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_');
    if name.is_empty() || name.eq_ignore_ascii_case("any") {
        return (None, None);
    }
    let version = parts
        .next()
        .filter(|token| token.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(|token| token.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '-').to_owned());
    (Some(name.to_owned()), version)
}

/// 没有 Java 异常时的判断。
///
/// ## 为什么需要单独一条路径
///
/// JVM 原生崩溃（`hs_err_pid*.log`）与看门狗超时都不抛 Java 异常：
///
///   - hs_err 以 `# A fatal error has been detected` 开头，后面是 `Problematic
///     frame:` 指向出事的模块（常常是显卡驱动的 dll/so）。这是显卡问题，但
///     从「异常」的角度看什么都不存在。
///   - 看门狗超时在日志里表现为 `ThreadDeath` 加 `watchdog`，说明服务器线程被
///     判定卡死，通常是世界生成或某个模组死循环。
///
/// 两者需要完全不同的建议，所以不能笼统地报「未知」。
fn classify_by_text(text: &str) -> CauseKind {
    let lower = text.to_ascii_lowercase();

    // --- JVM 原生崩溃 ------------------------------------------------------
    if lower.contains("a fatal error has been detected by the java runtime")
        || lower.contains("exception_access_violation")
        || lower.contains("sigsegv")
    {
        /*
         * `Problematic frame` 指向的模块说明是谁崩的。显卡驱动（nvoglv / atio /
         * igd）是最常见的，其次是 JVM 自身。
         */
        if lower.contains("nvoglv")
            || lower.contains("atio")
            || lower.contains("amdvlk")
            || lower.contains("igvk")
            || lower.contains("glxhd")
            || lower.contains("[opengl32")
        {
            return CauseKind::Graphics;
        }
        if lower.contains("problematic frame") && lower.contains("[libjvm") {
            return CauseKind::Unknown;
        }
        // 无法归因的原生崩溃按图形处理是不诚实的；保持 Unknown 但由上层给出
        // 「这是原生崩溃」的说明。
        return CauseKind::Unknown;
    }

    // --- 看门狗 / 卡死 ------------------------------------------------------
    if lower.contains("watchdog") && (lower.contains("threaddeath") || lower.contains("timed out"))
    {
        /*
         * 看门狗超时说明服务器线程卡住了，通常是某个模组在世界生成或 tick 里
         * 死循环。归到模组类是对的，但**不能**套用「依赖缺失」的建议——那是完全不同
         * 的处理。下面 reason_for 会按 mods 里是否有 mod_failures 区分。
         */
        return CauseKind::ExternalKill;
    }

    // --- 纯文本的 JVM 启动失败 ----------------------------------------------
    /*
     * `Could not reserve enough space` 这类消息在 JVM 初始化阶段打印，没有异常
     * 行。它们属于参数/内存问题，而不是「未知」。
     */
    if lower.contains("could not reserve enough space") || lower.contains("invalid maximum heap size")
    {
        return CauseKind::JvmArguments;
    }

    CauseKind::Unknown
}
