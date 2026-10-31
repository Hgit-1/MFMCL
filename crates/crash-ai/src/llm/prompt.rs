// Prompt construction.
//
// ## The constraint that shapes everything here
//
// A 1.7B model has a small effective working memory. Long, hedge-laden prompts
// make it lose the thread and start inventing causes. So the prompt is:
//
//   - **short**: the system message is a few rules, not an essay;
//   - **grounded**: the findings are given as facts and the model is told not to
//     contradict them;
//   - **bounded**: it may only recommend actions from a supplied vocabulary, so
//     it cannot invent a menu path that does not exist;
//   - **explicitly uncertain** when the rules found nothing, with the required
//     output marked as a guess.
//
// ## Why the output format is rigid
//
// `说明：` and `步骤：` are parsed by `llm::parse_answer`. Free-form prose would
// need a second model to extract structure. Demanding two labelled sections is
// something even a small model follows most of the time, and when it does not,
// the whole answer still degrades to an explanation rather than being lost.

use crate::causes::{Cause, CauseKind};
use crate::{Environment, Finding, LogFacts};

/// The system message. Kept under ~120 words on purpose.
pub const SYSTEM: &str = "\
你是 Minecraft 启动器的崩溃分析助手。你的读者是不懂技术的玩家。

规则：
1. 只根据给出的证据回答，不要编造日志里没有的内容。
2. 如果证据不足以确定原因，直接说“无法确定”，并说明还需要什么信息。
3. 不要说“可能是驱动、内存或模组”这种罗列。给出你认为最可能的那一个。
4. 建议必须是玩家能直接做的操作，不要提修改代码。
5. 用简体中文，语气平实，不要用感叹号。

输出格式（严格遵守）：
说明：一段话，2-4 句，讲清为什么崩溃。
步骤：
1. 第一个操作
2. 第二个操作
置信度：高/中/低";

/// Build the user message.
pub fn build(
    facts: &LogFacts,
    findings: &[Finding],
    cause: Option<&Cause>,
    environment: &Environment,
) -> String {
    let mut out = String::with_capacity(2048);

    out.push_str("## 环境\n");
    let java = environment
        .java_version
        .as_deref()
        .or(facts.detected_java.as_deref())
        .unwrap_or("未知");
    let game = environment
        .game_version
        .as_deref()
        .or(facts.detected_game_version.as_deref())
        .unwrap_or("未知");
    out.push_str(&format!("- 游戏版本：{game}\n"));
    out.push_str(&format!("- Java：{java}\n"));
    if let Some(loader) = environment.loader.as_deref() {
        out.push_str(&format!("- 加载器：{loader}\n"));
    }
    if let Some(memory) = environment.memory_mb {
        out.push_str(&format!("- 分配内存：{memory} MB\n"));
    }
    if let Some(java_bits) = environment.java_bits {
        out.push_str(&format!("- Java 位数：{java_bits} 位\n"));
    }
    if let Some(count) = environment.mod_count {
        out.push_str(&format!("- 模组数量：{count}\n"));
    }
    out.push('\n');

    /*
     * 因果结论放在最前。
     *
     * 它是从堆栈里读出来的（抛出的异常 + 最近的帧），比关键词可靠。模型拿到
     * 「原因已确定」这个前提后，任务就简化成「解释并给建议」，而不是自己诊断。
     * 实测这一条对 1.7B 的输出质量影响很大。
     */
    if let Some(cause) = cause
        && cause.kind != CauseKind::Unknown
    {
        out.push_str("## 已确定的根本原因（从堆栈分析得出，不要推翻）\n");
        out.push_str(&format!("类别：{}\n", kind_label(cause.kind)));
        if let Some(exception) = &cause.root_exception {
            out.push_str(&format!("最内层异常：{exception}\n"));
        }
        out.push_str(&format!("判断依据：{}\n\n", cause.reason));
    }

    /*
     * Findings are stated as established facts.
     *
     * This is the guardrail against a small model wandering: if the rules already
     * determined "Java 版本过低", the model's job is to explain and advise, not
     * to re-diagnose from scratch.
     */
    if findings.is_empty() {
        out.push_str("## 已有的规则分析\n规则没有匹配到已知的崩溃特征。下面是日志片段，请判断最可能的原因。");
        out.push_str("你没有把握时要把置信度标成“低”。\n\n");
    } else {
        out.push_str("## 已有的规则分析（这些结论已经确认，不要反驳）\n");
        for (index, finding) in findings.iter().enumerate().take(4) {
            out.push_str(&format!("{}. {}\n", index + 1, finding.title));
            if !finding.evidence.is_empty() {
                // One evidence line is enough; more crowds out the log excerpt.
                out.push_str(&format!("   依据：{}\n", clip(&finding.evidence[0], 200)));
            }
        }
        out.push('\n');
    }

    if !facts.involved_mods.is_empty() {
        out.push_str("## 日志里出现的模组\n");
        out.push_str(&facts.involved_mods.join("、"));
        out.push_str("\n\n");
    }

    out.push_str("## 日志片段\n```\n");
    out.push_str(&facts.context);
    out.push_str("\n```\n");

    out
}

/// 类别 → 给模型看的中文名。
fn kind_label(kind: CauseKind) -> &'static str {
    match kind {
        CauseKind::SystemLimit => "操作系统资源上限",
        CauseKind::OutOfMemory => "游戏内存不足",
        CauseKind::JvmArguments => "JVM 启动参数无效",
        CauseKind::JavaVersion => "Java 版本不匹配",
        CauseKind::GameFiles => "游戏文件缺失或损坏",
        CauseKind::Graphics => "图形栈/显卡问题",
        CauseKind::Mods => "模组加载失败",
        CauseKind::NativeLibrary => "本地依赖库缺失",
        CauseKind::FileAccess => "文件访问被拒绝",
        CauseKind::ExternalKill => "进程被外部终止",
        CauseKind::NoDisplay => "缺少图形会话",
        CauseKind::Unknown => "未确定",
    }
}

fn clip(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let cut = text.char_indices().nth(limit).map(|(i, _)| i).unwrap_or(text.len());
    format!("{}…", &text[..cut])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Confidence;

    fn facts() -> LogFacts {
        LogFacts {
            total_lines: 10,
            exception: Some("java.lang.OutOfMemoryError".into()),
            involved_mods: vec!["sodium".into()],
            context: "java.lang.OutOfMemoryError: Java heap space".into(),
            detected_java: Some("17.0.9".into()),
            detected_game_version: Some("1.21.1".into()),
        }
    }

    #[test]
    fn confirmed_findings_are_marked_as_not_to_be_contradicted() {
        let finding = Finding {
            code: "runtime.outofmemory".into(),
            title: "内存不足".into(),
            confidence: Confidence::High,
            explanation: String::new(),
            suggestions: vec![],
            evidence: vec!["java.lang.OutOfMemoryError".into()],
            mods: vec![],
        };
        let prompt = build(&facts(), &[finding], None, &Environment::default());
        assert!(prompt.contains("已经确认，不要反驳"));
        assert!(prompt.contains("内存不足"));
    }

    #[test]
    fn empty_findings_ask_for_a_low_confidence_guess() {
        let prompt = build(&facts(), &[], None, &Environment::default());
        assert!(prompt.contains("规则没有匹配"));
        assert!(prompt.contains("“低”"));
    }

    #[test]
    fn environment_values_are_included_when_present() {
        let environment = Environment {
            memory_mb: Some(2048),
            java_bits: Some(32),
            loader: Some("Fabric".into()),
            ..Default::default()
        };
        let prompt = build(&facts(), &[], None, &environment);
        assert!(prompt.contains("2048 MB"));
        assert!(prompt.contains("32 位"));
        assert!(prompt.contains("Fabric"));
    }

    #[test]
    fn log_excerpt_is_fenced() {
        let prompt = build(&facts(), &[], None, &Environment::default());
        assert!(prompt.contains("```\njava.lang.OutOfMemoryError"));
    }
}
