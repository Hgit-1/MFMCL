//! Bounded game-output handling shared by the desktop host and diagnostic export.
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

/// Drain a complete line without allocating beyond 8 KiB. Oversized lines are
/// omitted in full so a truncated credential can never bypass exact redaction.
pub async fn bounded_log_line<R: AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
) -> std::io::Result<Option<String>> {
    const MAX_LINE: usize = 8 * 1024;
    let mut bytes = Vec::new();
    let mut oversized = false;
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            if bytes.is_empty() && !oversized {
                return Ok(None);
            }
            break;
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |index| index + 1);
        let content_length = newline.unwrap_or(available.len());
        if !oversized {
            if bytes.len().saturating_add(content_length) <= MAX_LINE {
                bytes.extend_from_slice(&available[..content_length]);
            } else {
                oversized = true;
                bytes.clear();
            }
        }
        reader.consume(consumed);
        if newline.is_some() {
            break;
        }
    }
    if oversized {
        Ok(Some("[超长日志行已省略]".to_owned()))
    } else {
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
        Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
    }
}

pub fn redact_log(text: &str, secrets: &[&str]) -> String {
    static SECRET_PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let pattern = SECRET_PATTERN.get_or_init(|| {
        regex::Regex::new(r#"(?i)[\"']?(access[_-]?token|refresh[_-]?token|client[_-]?token|identity[_-]?token|xsts[_-]?token|authorization|password)[\"']?(?:\s*[:=]\s*|\s+)(?:\"[^\"]*\"|'[^']*'|Bearer\s+[^\s,;]+|[^\s,;]+)"#).expect("valid credential redaction expression")
    });
    let mut safe = text.to_owned();
    for secret in secrets {
        if !secret.is_empty() {
            safe = safe.replace(secret, "[REDACTED]");
        }
    }
    pattern.replace_all(&safe, "$1=[REDACTED]").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn drains_oversized_line_and_keeps_following_output() {
        let mut input = vec![b'x'; 2 * 1024 * 1024];
        input.extend_from_slice(b"\r\nnext\r\nlast");
        let mut reader = BufReader::new(input.as_slice());
        assert_eq!(
            bounded_log_line(&mut reader).await.unwrap().as_deref(),
            Some("[超长日志行已省略]")
        );
        assert_eq!(
            bounded_log_line(&mut reader).await.unwrap().as_deref(),
            Some("next")
        );
        assert_eq!(
            bounded_log_line(&mut reader).await.unwrap().as_deref(),
            Some("last")
        );
        assert!(bounded_log_line(&mut reader).await.unwrap().is_none());
    }
    #[test]
    fn removes_known_tokens_json_headers_and_jvm_command_arguments() {
        let text = "unlabelled actual-session-token\n--accessToken command-secret --username Alex\n{\"refreshToken\":\"refresh-secret\"}\nAuthorization: Bearer header-secret\npassword='password with spaces'";
        let safe = redact_log(text, &["actual-session-token"]);
        for secret in [
            "actual-session-token",
            "command-secret",
            "refresh-secret",
            "header-secret",
            "password with spaces",
        ] {
            assert!(!safe.contains(secret));
        }
        assert!(safe.contains("--username Alex"));
    }
}
