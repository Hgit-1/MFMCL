//! Crash analysis for MFMCL: deterministic rules plus a local LFM3 model.
//!
//! Split out of `launcher-core` so the analysis service can be built and shipped
//! on its own (`cargo run -p crash-ai --bin crash-ai-server`), while the launcher
//! links the same library and therefore behaves identically.

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

/// Full analysis result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Analysis {
    pub findings: Vec<Finding>,
    #[serde(default)]
    pub narrative: Option<Narrative>,
    pub facts: LogFacts,
    /// Whether the model stage actually ran. Without this the UI cannot tell a
    /// rules-only result from a complete one.
    pub model_used: bool,
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Rule pass plus fact extraction. No network, no model.
pub fn analyse_log(log: &str, environment: &Environment) -> (Vec<Finding>, LogFacts, Vec<String>) {
    let mut warnings = Vec::new();

    let truncated = log.len() > MAX_LOG_BYTES;
    // Trim from the front: the crash is at the end. Cut on a newline so the first
    // context line is not a half token.
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
    (findings, facts, warnings)
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

/// Run the whole pipeline: rules, then the model if it is reachable.
///
/// The model is optional by design. A launcher that refuses to explain a crash
/// because a 1.7B model is not running would be worse than one that shows the
/// rule match on its own.
pub async fn analyse(
    log: &str,
    environment: &Environment,
    config: &llm::LlmConfig,
    use_model: bool,
) -> Analysis {
    let (findings, facts, mut warnings) = analyse_log(log, environment);

    let mut narrative = None;
    let mut model_used = false;
    if use_model {
        match llm::narrate(config, &facts, &findings, environment).await {
            Ok(result) => {
                narrative = Some(result);
                model_used = true;
            }
            Err(error) => {
                /*
                 * A model failure never fails the analysis. It is reported so the
                 * user knows the explanation is missing and why, but the rule
                 * findings stand on their own.
                 */
                warnings.push(format!("{error}"));
            }
        }
    } else {
        warnings.push("已跳过模型分析（按请求）".to_owned());
    }

    Analysis {
        findings,
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
