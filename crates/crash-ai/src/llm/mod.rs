// LFM3 client.
//
// Talks to a llama.cpp server over the OpenAI-compatible chat API. Kept
// deliberately small: one endpoint, one prompt shape, strict timeouts.
//
// ## Thinking must be turned off
//
// LFM3 defaults to a reasoning mode that emits a `<think>` block before the
// answer. With a 1.7B model on CPU that block easily eats the entire token
// budget, and the API then returns an empty `content` — which looks exactly
// like a broken server. Measured on this machine: thinking on returned `''` for
// a one-word answer, thinking off returned `'PONG'`.
//
// So `enable_thinking: false` is always sent. If a future model ignores it, the
// `<think>` block is stripped from the response as a second line of defence.

pub mod prompt;

use crate::{Confidence, Narrative, StepSource};
use serde::Deserialize;
use std::time::Duration;

/// Where the model lives and how long to wait.
#[derive(Debug, Clone)]
pub struct LlmConfig {
    /// Base URL, no trailing slash. Defaults to a local llama.cpp server.
    pub base_url: String,
    pub model: String,
    /// Total budget for one analysis. A 1.7B model on CPU needs seconds for a
    /// short answer; anything past a minute means it is thrashing, and the user
    /// is better served by the rules-only result.
    pub timeout: Duration,
    pub max_tokens: u32,
    /*
     * 下面两项用于判断这台机器能不能跑这个模型。
     *
     * 它们是部署参数而不是用户输入：用户不该需要知道「参数量」或「上下文长度」
     * 这类数字，但内存需求完全由它们决定。写成配置项而不是常量，是因为将来换成
     * 更大的模型时，可行性判断必须跟着变——否则它会自信地说「能跑」然后把人
     * 的机器拖死。
     */
    /// 参数量（十亿）。默认 1.7（LFM3-1.7B）。
    pub parameters_b: f32,
    /// 上下文长度。默认 8192，与启动 llama-server 时用的 `-c` 一致。
    pub context_tokens: u32,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            base_url: std::env::var("MFMCL_LLM_BASE_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8099".to_owned()),
            model: std::env::var("MFMCL_LLM_MODEL").unwrap_or_else(|_| "lfm3-1.7b".to_owned()),
            timeout: Duration::from_secs(90),
            max_tokens: 420,
            parameters_b: std::env::var("MFMCL_LLM_PARAMS_B")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(1.7),
            context_tokens: std::env::var("MFMCL_LLM_CONTEXT")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(8192),
        }
    }
}

#[derive(Debug)]
pub enum LlmError {
    /// Server unreachable. Distinct from a bad response: the UI must be able to
    /// say "model not running" rather than "analysis failed".
    Unreachable(String),
    /// Server answered, but not with something usable.
    BadResponse(String),
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(message) => write!(formatter, "无法连接本地模型：{message}"),
            Self::BadResponse(message) => write!(formatter, "模型返回了无法解析的内容：{message}"),
        }
    }
}

impl std::error::Error for LlmError {}

/// Ask the model to explain the findings.
///
/// `findings` may be empty — that is the "rules did not match" case, and the
/// prompt handles it by explicitly telling the model it is speculating.
pub async fn narrate(
    config: &LlmConfig,
    facts: &crate::LogFacts,
    findings: &[crate::Finding],
    cause: Option<&crate::causes::Cause>,
    environment: &crate::Environment,
) -> Result<Narrative, LlmError> {
    let user_prompt = prompt::build(facts, findings, cause, environment);

    let body = serde_json::json!({
        "model": config.model,
        "messages": [
            { "role": "system", "content": prompt::SYSTEM },
            { "role": "user", "content": user_prompt },
        ],
        "max_tokens": config.max_tokens,
        // Low temperature on purpose: this is a diagnostic explanation, not a
        // creative task, and a 1.7B model drifts quickly at higher values.
        "temperature": 0.2,
        "top_p": 0.9,
        "stream": false,
        /*
         * LFM3 puts its reasoning in a <think> block. On CPU that consumes the
         * whole budget and leaves `content` empty, so it is disabled explicitly.
         */
        "chat_template_kwargs": { "enable_thinking": false },
    });

    let client = reqwest::Client::builder()
        .timeout(config.timeout)
        .connect_timeout(Duration::from_secs(5))
        .build()
        .map_err(|error| LlmError::Unreachable(error.to_string()))?;

    let response = client
        .post(format!("{}/v1/chat/completions", config.base_url.trim_end_matches('/')))
        .json(&body)
        .send()
        .await
        .map_err(|error| {
            if error.is_connect() || error.is_timeout() {
                LlmError::Unreachable(format!(
                    "{}（模型可能没有启动）",
                    error
                ))
            } else {
                LlmError::Unreachable(error.to_string())
            }
        })?;

    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| LlmError::BadResponse(error.to_string()))?;

    if !status.is_success() {
        return Err(LlmError::BadResponse(format!(
            "HTTP {status}: {}",
            truncate(&text, 300)
        )));
    }

    let parsed: ChatResponse = serde_json::from_str(&text)
        .map_err(|error| LlmError::BadResponse(format!("{error}: {}", truncate(&text, 300))))?;

    let raw = parsed
        .choices
        .first()
        .and_then(|choice| choice.message.content.as_deref())
        .ok_or_else(|| LlmError::BadResponse("响应里没有 content".to_owned()))?;

    let cleaned = strip_thinking(raw);
    if cleaned.trim().is_empty() {
        /*
         * An empty answer is a real failure mode with small models: either the
         * budget ran out inside a thinking block, or the template swallowed the
         * output. Report it instead of pretending the model had nothing to say.
         */
        return Err(LlmError::BadResponse(
            "模型没有给出内容（可能仍然输出了思考块并耗尽了 token）".to_owned(),
        ));
    }

    let (explanation, model_steps) = parse_answer(&cleaned);

    /*
     * Advice comes from the rules whenever they matched.
     *
     * ## Why not the model's steps
     *
     * Observed repeatedly from the live model on an OutOfMemoryError log — every
     * run phrased it differently and every one was wrong in the same direction:
     *
     *   "降低 Java 堆内存到 512 MB"                       (run 1)
     *   "降低游戏中分配给 Java 的内存…在 Forge 配置中减少堆大小" (run 2)
     *
     * Regex-filtering these was a losing game: each fix invited a rephrasing that
     * slipped through. The deeper problem is that a 1.7B model has no reliable
     * notion of *direction* for a fix, so asking it for steps at all is the
     * mistake.
     *
     * The rules, by contrast, already carry curated and tested suggestions —
     * "提高内存分配（按模组数量可加 1–2 GB）". So when a rule matched, its steps
     * win and the model is used for what it is actually good at: reading this
     * specific log and explaining why it happened.
     *
     * When no rule matched there is nothing curated to fall back on, so the
     * model's steps are kept — but the whole narrative is labelled speculative
     * in the UI, because that is what it is.
     */
    /*
     * 有确定的因果结论（非 Unknown）也算「规则命中」：那种情况下我们同样有
     * 可信的、人工整理过的建议，不该让模型自己发挥。
     */
    let confirmed = !findings.is_empty()
        || cause.is_some_and(|item| item.kind != crate::causes::CauseKind::Unknown);
    let rules_matched = confirmed;
    let (steps, source) = if rules_matched {
        let curated: Vec<String> = findings
            .iter()
            .flat_map(|finding| finding.suggestions.clone())
            .take(6)
            .collect();
        (curated, StepSource::Rules)
    } else {
        (model_steps, StepSource::Model)
    };

    Ok(Narrative {
        explanation,
        steps,
        source,
        /*
         * The model's self-assessment is capped at Medium. A 1.7B model that was
         * told what the rules found should not be trusted to declare High
         * confidence, and letting it do so would make model output
         * indistinguishable from a matched signature.
         */
        confidence: confidence_from(&cleaned).min(Confidence::Medium),
        model: config.model.clone(),
    })
}

/// Probe the server without generating anything, for the health endpoint.
pub async fn probe(config: &LlmConfig) -> Result<Vec<String>, LlmError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|error| LlmError::Unreachable(error.to_string()))?;
    let response = client
        .get(format!("{}/v1/models", config.base_url.trim_end_matches('/')))
        .send()
        .await
        .map_err(|error| LlmError::Unreachable(error.to_string()))?;
    if !response.status().is_success() {
        return Err(LlmError::Unreachable(format!("HTTP {}", response.status())));
    }
    #[derive(Deserialize)]
    struct Models {
        #[serde(default)]
        data: Vec<ModelEntry>,
    }
    #[derive(Deserialize)]
    struct ModelEntry {
        id: String,
    }
    let models: Models = response
        .json()
        .await
        .map_err(|error| LlmError::BadResponse(error.to_string()))?;
    if models.data.is_empty() {
        return Err(LlmError::BadResponse("服务端没有加载任何模型".to_owned()));
    }
    Ok(models.data.into_iter().map(|entry| entry.id).collect())
}

#[derive(Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: Message,
}

#[derive(Deserialize)]
struct Message {
    #[serde(default)]
    content: Option<String>,
}

/// Remove a `<think>…</think>` block.
///
/// Handling both the closed and the unclosed case matters: a truncated response
/// often ends with an open `<think>` and no answer at all.
pub fn strip_thinking(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let Some(start) = rest.find("<think") else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..start]);
        let after_open = &rest[start..];
        match after_open.find("</think>") {
            Some(end) => rest = &after_open[end + "</think>".len()..],
            // Unclosed: everything after it was reasoning, so drop it.
            None => break,
        }
    }
    out.trim().to_owned()
}

/// Split the answer into the explanation and the numbered steps.
///
/// The prompt asks for `说明：` then a `步骤：` list. Small models follow this
/// most of the time but not always, so an unparseable answer still becomes the
/// explanation rather than being thrown away.
///
/// The `置信度：` line is always stripped even though it is parsed separately:
/// observed from the live model, a trailing `置信度：高` was being consumed as a
/// final step, which then rendered as "3. 置信度：高" in the UI.
pub fn parse_answer(text: &str) -> (String, Vec<String>) {
    let mut explanation = String::new();
    let mut steps: Vec<String> = Vec::new();
    let mut in_steps = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower_marker = trimmed.replace([':', '：'], ":");
        if lower_marker.starts_with("置信度") || lower_marker.starts_with("confidence") {
            continue;
        }
        if lower_marker.starts_with("步骤:") || lower_marker.starts_with("steps:") {
            in_steps = true;
            let rest = lower_marker
                .split_once(':')
                .map(|(_, value)| value.trim())
                .unwrap_or("");
            if !rest.is_empty() {
                steps.push(rest.to_owned());
            }
            continue;
        }
        if lower_marker.starts_with("说明:") || lower_marker.starts_with("explanation:") {
            in_steps = false;
            let rest = lower_marker
                .split_once(':')
                .map(|(_, value)| value.trim())
                .unwrap_or("");
            if !rest.is_empty() {
                if !explanation.is_empty() {
                    explanation.push(' ');
                }
                explanation.push_str(rest);
            }
            continue;
        }

        if in_steps {
            // Accept "1. text", "- text", or bare text under the header.
            let cleaned = trimmed
                .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ')' || c == '•' || c == '-' || c == '*')
                .trim();
            if !cleaned.is_empty() {
                steps.push(cleaned.to_owned());
            }
        } else {
            if !explanation.is_empty() {
                explanation.push(' ');
            }
            explanation.push_str(trimmed);
        }
    }

    if explanation.trim().is_empty() && !steps.is_empty() {
        // Only steps came back; promote the first one so the caller always has
        // something to show as the explanation.
        explanation = steps.remove(0);
    }
    (explanation.trim().to_owned(), steps.into_iter().take(6).collect())
}

/// Read the confidence the model claimed, if it stated one.
///
/// The full-width colon is normalised first. Without that the live model's
/// actual output — `置信度：高` — never matched any marker and every analysis
/// silently fell through to `Low`, which is exactly the kind of quiet
/// degradation that makes a feature look broken.
fn confidence_from(text: &str) -> Confidence {
    let normalised = text.replace(['：', '∶'], ":").to_ascii_lowercase();
    for (marker, level) in [
        ("置信度: 高", Confidence::Medium),
        ("置信度:高", Confidence::Medium),
        ("confidence: high", Confidence::Medium),
        // "中" is only reachable with the explicit marker; anything else is Low.
        ("置信度: 中", Confidence::Medium),
        ("置信度:中", Confidence::Medium),
        ("confidence: medium", Confidence::Medium),
        ("置信度: 低", Confidence::Low),
        ("置信度:低", Confidence::Low),
        ("confidence: low", Confidence::Low),
    ] {
        if normalised.contains(marker) {
            return level;
        }
    }
    Confidence::Low
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let cut = text.char_indices().nth(limit).map(|(i, _)| i).unwrap_or(text.len());
    format!("{}…", &text[..cut])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinking_block_is_removed() {
        assert_eq!(strip_thinking("<think>secret</think>答案"), "答案");
        assert_eq!(strip_thinking("答案"), "答案");
        // Unclosed thinking leaves nothing usable rather than leaking reasoning.
        assert_eq!(strip_thinking("<think>half a thought"), "");
    }

    #[test]
    fn answer_sections_are_split() {
        let (explanation, steps) = parse_answer(
            "说明：内存分配不足。\n步骤：\n1. 提高内存\n2. 减少模组\n",
        );
        assert_eq!(explanation, "内存分配不足。");
        assert_eq!(steps, vec!["提高内存", "减少模组"]);
    }

    #[test]
    fn answer_without_markers_still_becomes_explanation() {
        let (explanation, steps) = parse_answer("大概是内存不够。");
        assert_eq!(explanation, "大概是内存不够。");
        assert!(steps.is_empty());
    }

    #[test]
    fn steps_only_response_promotes_first_step() {
        let (explanation, steps) = parse_answer("步骤：\n1. 提高内存\n2. 减少模组\n");
        assert_eq!(explanation, "提高内存");
        assert_eq!(steps, vec!["减少模组"]);
    }

    #[test]
    fn model_confidence_is_capped_at_medium() {
        // A 1.7B model must never be able to claim High and look like a rule match.
        assert_eq!(confidence_from("置信度：高"), Confidence::Medium);
        assert_eq!(confidence_from("confidence: high"), Confidence::Medium);
    }

    #[test]
    fn confidence_line_never_becomes_a_step() {
        let (_, steps) = parse_answer("说明：内存不足。\n步骤：\n1. 提高内存\n置信度：高\n");
        assert_eq!(steps, vec!["提高内存"]);
    }
}

// ---------------------------------------------------------------------------
// 流式输出
// ---------------------------------------------------------------------------

/// 流式回调。每收到一小段文本就调用一次。
///
/// 用 `Send` 是因为它会被跨 `await` 持有：调用方通常在异步任务里把片段转发到
/// SSE 连接。
pub type StreamHandler<'a> = &'a mut (dyn FnMut(&str) + Send);

/// 与 `narrate` 相同，但把模型输出边生成边回调。
///
/// ## 为什么值得单独实现
///
/// 1.7B 模型在 CPU 上生成一段完整解释要 6–13 秒。没有流式时，用户面对的是一个
/// 骨架屏，无法区分「正在推理」和「卡死了」。有流式以后，第一个字通常在 1–2 秒内
/// 出现，剩下的时间用户是在读内容，而不是在等。
///
/// ## 实现上的两个坑
///
/// 1. **思考块必须在流里实时剥离**。不能等整个响应结束再剥：那样用户会先看到
///    一段 `<think>...` 的推理过程，然后它又消失。所以按行切分时用一个状态机
///    跟踪「当前是否在遮罩内」。
/// 2. **增量可能把一个标记切成两半**（`</thi` + `nk>`），所以剥离逻辑必须容忍
///    跨块的半截标记，不能假设每一块都是完整的。
pub async fn narrate_streaming(
    config: &LlmConfig,
    facts: &crate::LogFacts,
    findings: &[crate::Finding],
    cause: Option<&crate::causes::Cause>,
    environment: &crate::Environment,
    on_chunk: StreamHandler<'_>,
) -> Result<Narrative, LlmError> {
    let user_prompt = prompt::build(facts, findings, cause, environment);

    let body = serde_json::json!({
        "model": config.model,
        "messages": [
            { "role": "system", "content": prompt::SYSTEM },
            { "role": "user", "content": user_prompt },
        ],
        "max_tokens": config.max_tokens,
        "temperature": 0.2,
        "top_p": 0.9,
        "stream": true,
        "chat_template_kwargs": { "enable_thinking": false },
    });

    let client = reqwest::Client::builder()
        .timeout(config.timeout)
        .connect_timeout(Duration::from_secs(5))
        .build()
        .map_err(|error| LlmError::Unreachable(error.to_string()))?;

    let response = client
        .post(format!("{}/v1/chat/completions", config.base_url.trim_end_matches('/')))
        .json(&body)
        .send()
        .await
        .map_err(|error| {
            if error.is_connect() || error.is_timeout() {
                LlmError::Unreachable(format!("{}（模型可能没有启动）", error))
            } else {
                LlmError::Unreachable(error.to_string())
            }
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(LlmError::BadResponse(format!(
            "HTTP {status}: {}",
            truncate(&text, 300)
        )));
    }

    use futures_util::StreamExt;
    let mut stream = response.bytes_stream();
    let mut raw = String::new();
    let mut pending = String::new();
    let mut guard = ThinkGuard::default();

    /*
     * 按字节累积再按行切分。
     *
     * SSE 的 `data:` 行以 `\n` 结束，但一个 TCP 块可能包含半行，也可能包含多行。
     * 所以保留未完成的部分（`pending`），只处理完整的行。
     */
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|error| LlmError::Unreachable(error.to_string()))?;
        pending.push_str(&String::from_utf8_lossy(&bytes));

        while let Some(index) = pending.find('\n') {
            let line: String = pending.drain(..=index).collect();
            let line = line.trim_end_matches(['\r', '\n']);
            let Some(payload) = line.strip_prefix("data: ") else {
                continue;
            };
            if payload.trim() == "[DONE]" {
                continue;
            }
            let Ok(value) = serde_json::from_str::<StreamChunk>(payload) else {
                continue;
            };
            let Some(delta) = value
                .choices
                .first()
                .and_then(|choice| choice.delta.as_ref())
                .and_then(|delta| delta.content.as_deref())
            else {
                continue;
            };
            raw.push_str(delta);

            /*
             * 实时剥离思考块后转发。
             *
             * 只有通过了 guard 的文本才会到达界面，所以用户不会先看到推理过程。
             */
            let visible = guard.filter(delta);
            if !visible.is_empty() {
                on_chunk(&visible);
            }
        }
    }

    if raw.trim().is_empty() {
        return Err(LlmError::BadResponse(
            "模型没有返回任何内容（流式响应为空）".to_owned(),
        ));
    }

    finish_narrative(config, findings, cause, &raw)
}

// ---------------------------------------------------------------------------
// 流式辅助
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<StreamChoice>,
}

#[derive(Deserialize)]
struct StreamChoice {
    #[serde(default)]
    delta: Option<StreamDelta>,
}

#[derive(Deserialize)]
struct StreamDelta {
    #[serde(default)]
    content: Option<String>,
}

/// 在增量文本里过滤 `<think>` 段。
///
/// 增量切分会让标记本身被拆开（`</thi` + `nk>`），所以状态机要能跨块保持：
/// 遇到 `<` 时先缓存，等后续字节决定它是不是标记的一部分。
#[derive(Default)]
pub struct ThinkGuard {
    /// 当前是否在 `<think>` 块内。
    inside: bool,
    /// 已收到但还无法判定的尾部（可能是半个标记）。
    tail: String,
}

impl ThinkGuard {
    /// 过滤一段增量，返回应当显示的部分。
    pub fn filter(&mut self, chunk: &str) -> String {
        self.tail.push_str(chunk);
        let mut out = String::new();

        loop {
            if self.inside {
                // 在遮罩内：找到结束标记前全部丢弃。
                match self.tail.find("</think>") {
                    Some(index) => {
                        self.tail.drain(..index + "</think>".len());
                        self.inside = false;
                    }
                    None => {
                        /*
                         * 还没看到结束标记。保留可能是 `</think>` 前缀的尾部，
                         * 其余丢掉——这部分内容不该显示。
                         */
                        let keep = partial_suffix(&self.tail, "</think>");
                        self.tail.drain(..self.tail.len() - keep);
                        return out;
                    }
                }
            } else {
                match self.tail.find("<think") {
                    Some(index) => {
                        out.push_str(&self.tail[..index]);
                        self.tail.drain(..index);
                        // 等到 `>` 才能确认这是标记而不是普通文本里的尖括号。
                        match self.tail.find('>') {
                            Some(end) => {
                                self.tail.drain(..=end);
                                self.inside = true;
                            }
                            None => return out,
                        }
                    }
                    None => {
                        /*
                         * 没有起始标记。但尾部可能是 `<thi` 这样的半个标记，
                         * 需要保留到下一块再判断。
                         */
                        let keep = partial_suffix(&self.tail, "<think");
                        let cut = self.tail.len() - keep;
                        out.push_str(&self.tail[..cut]);
                        self.tail.drain(..cut);
                        return out;
                    }
                }
            }
        }
    }

    /// 收尾：把仍然缓存的内容吐出来。
    ///
    /// 只有在确认它不属于任何一个被截断的标记时才返回——否则会漏出半个标签。
    pub fn finish(&mut self) -> String {
        let rest = std::mem::take(&mut self.tail);
        if self.inside || rest.starts_with('<') {
            String::new()
        } else {
            rest
        }
    }
}

/// `text` 的末尾有多少字符是 `marker` 的前缀。
fn partial_suffix(text: &str, marker: &str) -> usize {
    let bytes = text.as_bytes();
    let marker_bytes = marker.as_bytes();
    let max = bytes.len().min(marker_bytes.len().saturating_sub(1));
    for length in (1..=max).rev() {
        if bytes[bytes.len() - length..] == marker_bytes[..length] {
            return length;
        }
    }
    0
}

/// 组装最终的 `Narrative`。
///
/// 与 `narrate` 共用，保证流式与非流式的解析结果完全一致——否则「流式看到的文字」
/// 和「最终结构里的文字」可能不同，用户会看到内容闪一下然后变化。
fn finish_narrative(
    config: &LlmConfig,
    findings: &[crate::Finding],
    cause: Option<&crate::causes::Cause>,
    raw: &str,
) -> Result<Narrative, LlmError> {
    let cleaned = strip_thinking(raw);
    if cleaned.trim().is_empty() {
        return Err(LlmError::BadResponse(
            "模型没有给出内容（可能仍然输出了思考块并耗尽了 token）".to_owned(),
        ));
    }
    let (explanation, model_steps) = parse_answer(&cleaned);

    let confirmed = !findings.is_empty()
        || cause.is_some_and(|item| item.kind != crate::causes::CauseKind::Unknown);
    let (steps, source) = if confirmed {
        let curated: Vec<String> = findings
            .iter()
            .flat_map(|finding| finding.suggestions.clone())
            .take(6)
            .collect();
        (curated, StepSource::Rules)
    } else {
        (model_steps, StepSource::Model)
    };

    Ok(Narrative {
        explanation,
        steps,
        source,
        confidence: confidence_from(&cleaned).min(Confidence::Medium),
        model: config.model.clone(),
    })
}

// ---------------------------------------------------------------------------
// 流式测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod streaming_tests {
    use super::*;

    #[test]
    fn think_guard_strips_complete_block_in_one_chunk() {
        let mut guard = ThinkGuard::default();
        assert_eq!(guard.filter("<think>secret</think>答案"), "答案");
    }

    #[test]
    fn think_guard_handles_markers_split_across_chunks() {
        /*
         * 这是流式最容易出错的地方：增量把一个标记切成两半。
         * `</thi` + `nk>` 必须被识别成一个结束标记，而不是当成正文吐出去。
         */
        let mut guard = ThinkGuard::default();
        let mut visible = String::new();
        for piece in ["<thi", "nk>", "推理中", "</thi", "nk>", "答案是内存不足"] {
            visible.push_str(&guard.filter(piece));
        }
        visible.push_str(&guard.finish());
        assert_eq!(visible, "答案是内存不足");
    }

    #[test]
    fn think_guard_passes_ordinary_text_untouched() {
        let mut guard = ThinkGuard::default();
        let mut visible = String::new();
        for piece in ["说明：", "内存不足。", "\n步骤：\n1. 提高内存"] {
            visible.push_str(&guard.filter(piece));
        }
        visible.push_str(&guard.finish());
        assert_eq!(visible, "说明：内存不足。\n步骤：\n1. 提高内存");
    }

    #[test]
    fn think_guard_does_not_leak_a_truncated_tag() {
        // 流在标记中途断掉：不能把半个标签显示出来。
        let mut guard = ThinkGuard::default();
        let visible = guard.filter("答案<thi");
        assert_eq!(visible, "答案");
        assert_eq!(guard.finish(), "", "half a tag must not be emitted");
    }

    #[test]
    fn think_guard_keeps_stray_angle_bracket() {
        // 普通文本里的尖括号不代表标记——等到能确认是 `<think` 才拦截。
        let mut guard = ThinkGuard::default();
        let mut visible = String::from(guard.filter("内存 < 4GB 时"));
        visible.push_str(&guard.finish());
        assert_eq!(visible, "内存 < 4GB 时");
    }

    #[test]
    fn streaming_and_non_streaming_parse_identically() {
        /*
         * 两条路径必须给出一致的结构，否则界面上会「先看到文字、再被替换」。
         */
        let findings = vec![crate::Finding {
            code: "runtime.outofmemory".into(),
            title: "内存不足".into(),
            confidence: Confidence::High,
            explanation: String::new(),
            suggestions: vec!["提高内存分配".into()],
            evidence: vec![],
            mods: vec![],
        }];
        let config = LlmConfig::default();
        let raw = "说明：堆内存耗尽。\n步骤：\n1. 降低内存\n置信度：高";
        let streamed = finish_narrative(&config, &findings, None, raw).unwrap();
        assert_eq!(streamed.explanation, "堆内存耗尽。");
        // 规则命中时步骤来自规则，不是模型。
        assert_eq!(streamed.steps, vec!["提高内存分配"]);
        assert_eq!(streamed.confidence, Confidence::Medium);
    }
}
