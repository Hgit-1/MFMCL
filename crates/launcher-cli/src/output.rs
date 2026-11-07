//! Terminal output: colour policy, tables, byte/date formatting, prompts.
//!
//! ## Why every writer goes through here
//!
//! Three rules apply to everything the CLI prints, and keeping them in one module
//! is what makes that enforceable rather than aspirational:
//!
//!   - **Colour is opt-in by terminal.** A pipe, a redirect, or CI must get plain
//!     text. `anstream` decides; we never check `isatty` by hand.
//!   - **`--json` means only JSON on stdout.** Progress bars and colour spoilers
//!     go to stderr, so `mfmcl list --json | jq` cannot be broken by a spinner.
//!   - **Widths come from the terminal.** A 40-column terminal must not get a
//!     120-column table, and a redirected file should not get ANSI escapes.

use std::io::{IsTerminal, Write};

/// Whether stdout should receive colour and cursor control.
///
/// `--color=never`, `NO_COLOR`, and a redirected stdout all mean no. The check is
/// deliberately conservative: it is better to print plain text in a terminal that
/// could have taken colour than to leak escape codes into a log file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColourMode {
    Auto,
    Always,
    Never,
}

impl ColourMode {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "always" | "always-ansi" => Ok(Self::Always),
            "never" => Ok(Self::Never),
            other => Err(format!(
                "无法识别的颜色模式 {other}；可用：auto、always、never。"
            )),
        }
    }

    /// Resolve against the current process environment and stdout.
    pub fn enabled(self) -> bool {
        match self {
            Self::Always => true,
            Self::Never => false,
            Self::Auto => {
                // `NO_COLOR` is a cross-tool convention: if the user set it, they
                // mean it, regardless of what the terminal could do.
                std::env::var_os("NO_COLOR").is_none()
                    && std::env::var("TERM").is_ok_and(|term| term != "dumb")
                    && std::io::stdout().is_terminal()
            }
        }
    }
}

/// Paint one piece of text if colour is on.
///
/// Each helper takes text it does not own and returns a `String` only when it is
/// actually colouring; the plain path hands back the input unchanged.
///
/// `Copy` because it is a single `bool`: commands build derived sinks (for example
/// one with game logging turned off) and threading a reference through them adds
/// lifetimes for no benefit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub on: bool,
}

impl Palette {
    pub fn new(on: bool) -> Self {
        Self { on }
    }

    fn wrap(&self, code: &str, text: &str) -> String {
        if self.on {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }

    pub fn bold(&self, text: &str) -> String {
        self.wrap("1", text)
    }
    pub fn dim(&self, text: &str) -> String {
        self.wrap("2", text)
    }
    pub fn green(&self, text: &str) -> String {
        self.wrap("32", text)
    }
    pub fn yellow(&self, text: &str) -> String {
        self.wrap("33", text)
    }
    pub fn red(&self, text: &str) -> String {
        self.wrap("31", text)
    }
    pub fn cyan(&self, text: &str) -> String {
        self.wrap("36", text)
    }
}

/// Human-readable byte count.
///
/// Binary units (`MiB`) because download totals are compared against file sizes
/// the OS reports in the same units. One decimal is enough to tell 1.4 GiB from
/// 1.6 GiB; more would be noise that changes on every refresh.
pub fn bytes(value: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut size = value as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// Bytes plus a percentage, or just bytes when the total is unknown.
///
/// Printing `100%` for an unknown total would be a lie; printing `?%` would be
/// noise. Withholding the percentage is the honest option.
pub fn progress_text(completed: u64, total: Option<u64>) -> String {
    match total.filter(|total| *total > 0) {
        Some(total) => {
            let percent = (completed as f64 / total as f64 * 100.0).clamp(0.0, 100.0);
            format!("{} / {} ({percent:.0}%)", bytes(completed), bytes(total))
        }
        None => bytes(completed),
    }
}

/// A transfer rate, in the units people read network speeds in.
///
/// `B/s` for slow transfers and `MiB/s` for fast ones, because "0.0 MiB/s" is
/// useless when a small metadata file is trickling in.
pub fn rate(bytes_per_second: f64) -> String {
    if !bytes_per_second.is_finite() || bytes_per_second <= 0.0 {
        return "—".to_owned();
    }
    if bytes_per_second < 1024.0 {
        format!("{bytes_per_second:.0} B/s")
    } else {
        format!("{}/s", bytes(bytes_per_second as u64))
    }
}

/// A countdown, bounded so a stalled transfer does not print a day.
///
/// Clamped at 99 minutes: past that the estimate is derived from a rate that has
/// already decayed, and a precise-looking "1h 47m" would be false confidence.
pub fn eta(remaining_bytes: u64, bytes_per_second: f64) -> String {
    if !bytes_per_second.is_finite() || bytes_per_second <= 0.0 {
        return "—".to_owned();
    }
    let seconds = (remaining_bytes as f64 / bytes_per_second).ceil();
    if !seconds.is_finite() || seconds < 0.0 {
        return "—".to_owned();
    }
    let seconds = seconds.min(99.0 * 60.0) as u64;
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {:02}m", seconds / 3600, seconds % 3600 / 60)
    }
}

/// An elapsed duration, in the compact form a summary line wants.
pub fn duration(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "—".to_owned();
    }
    let seconds = seconds as u64;
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {:02}m", seconds / 3600, seconds % 3600 / 60)
    }
}

/// Strip the timezone from an RFC 3339 timestamp for display.
///
/// Records are stored in UTC with an offset; showing `2026-11-07T05:03:11+00:00`
/// makes a table unreadable. Falls back to the original string when it does not
/// look like a timestamp, so a hand-edited database value still displays.
pub fn timestamp(value: &str) -> String {
    let Some(parsed) = chrono::DateTime::parse_from_rfc3339(value).ok() else {
        return value.to_owned();
    };
    parsed.format("%Y-%m-%d %H:%M").to_string()
}

/// Visible width of a string, ignoring ANSI escape sequences.
///
/// Table alignment has to measure what the user sees. Measuring bytes would pad
/// coloured cells wrongly, and Chinese text is two columns wide per character,
/// which a naive `chars().count()` also gets wrong.
pub fn display_width(text: &str) -> usize {
    let mut width = 0;
    let mut in_escape = false;
    for ch in text.chars() {
        if in_escape {
            if ch.is_ascii_alphabetic() {
                in_escape = false;
            }
            continue;
        }
        if ch == '\x1b' {
            in_escape = true;
            continue;
        }
        width += char_width(ch);
    }
    width
}

/// Terminal columns a single character occupies.
///
/// East Asian wide and fullwidth characters (CJK, most emoji) take two columns.
/// Everything else here is treated as one; this is the subset that matters for
/// our own strings, not a complete Unicode width table.
fn char_width(ch: char) -> usize {
    let code = ch as u32;
    let wide = matches!(code,
        0x1100..=0x115F
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1F64F
        | 0x1F900..=0x1F9FF
        | 0x20000..=0x2FFFD
        | 0x30000..=0x3FFFD
    );
    if wide { 2 } else { 1 }
}

/// Pad `text` with spaces to `width` visible columns.
pub fn pad(text: &str, width: usize) -> String {
    let current = display_width(text);
    if current >= width {
        return text.to_owned();
    }
    format!("{text}{}", " ".repeat(width - current))
}

/// Truncate to `width` visible columns, appending an ellipsis when cut.
///
/// This is what keeps a 200-character Modrinth description from turning one table
/// row into a wall of text. The ellipsis is a single column so the result never
/// exceeds `width`.
pub fn truncate(text: &str, width: usize) -> String {
    if display_width(text) <= width {
        return text.to_owned();
    }
    if width <= 1 {
        return text.chars().take(width).collect();
    }
    let mut result = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let next = char_width(ch);
        if used + next > width - 1 {
            break;
        }
        result.push(ch);
        used += next;
    }
    result.push('…');
    result
}

/// A left-aligned table that adapts to the terminal width.
pub struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    /// Right-align these columns (numbers read better aligned right).
    numeric: Vec<bool>,
}

impl Table {
    pub fn new(headers: &[&str]) -> Self {
        Self {
            headers: headers.iter().map(|item| (*item).to_owned()).collect(),
            rows: Vec::new(),
            numeric: vec![false; headers.len()],
        }
    }

    pub fn numeric(mut self, columns: &[usize]) -> Self {
        for column in columns {
            if let Some(slot) = self.numeric.get_mut(*column) {
                *slot = true;
            }
        }
        self
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.rows.len()
    }
    pub fn row(&mut self, cells: Vec<String>) {
        debug_assert_eq!(
            cells.len(),
            self.headers.len(),
            "a row must have one cell per header"
        );
        self.rows.push(cells);
    }

    /// Render with `gap` spaces between columns.
    ///
    /// When the natural widths exceed the terminal, the widest free-text column is
    /// shrunk and its cells are truncated. Numeric columns are never shrunk: losing
    /// digits turns a size into a wrong number.
    pub fn render(&self, palette: &Palette, max_width: usize) -> String {
        if self.rows.is_empty() {
            return String::new();
        }
        let mut widths: Vec<usize> = self
            .headers
            .iter()
            .map(|header| display_width(header))
            .collect();
        for row in &self.rows {
            for (index, cell) in row.iter().enumerate() {
                widths[index] = widths[index].max(display_width(cell));
            }
        }
        const GAP: usize = 2;
        let total =
            |widths: &[usize]| -> usize { widths.iter().sum::<usize>() + GAP * (widths.len() - 1) };
        if max_width > 0 && total(&widths) > max_width {
            // Shrink the widest non-numeric column, repeatedly if needed. One
            // pass is not enough when several columns are long.
            while total(&widths) > max_width {
                let candidate = (0..widths.len())
                    .filter(|index| !self.numeric[*index] && widths[*index] > 6)
                    .max_by_key(|index| widths[*index]);
                match candidate {
                    Some(index) => widths[index] -= 1,
                    // Everything is already at its floor; accept the overflow
                    // rather than mangling the data.
                    None => break,
                }
            }
        }

        let mut out = String::new();
        let push_row = |cells: &[String], out: &mut String| {
            for (index, cell) in cells.iter().enumerate() {
                let content = truncate(cell, widths[index]);
                if index + 1 == cells.len() {
                    out.push_str(&content);
                } else {
                    out.push_str(&pad(&content, widths[index]));
                    out.push_str("  ");
                }
            }
            out.push('\n');
        };
        // Colour is applied *after* padding: escape sequences must not be counted
        // as content, or every column after the first shifts right.
        for (index, header) in self.headers.iter().enumerate() {
            let text = truncate(header, widths[index]);
            out.push_str(&palette.dim(&pad(&text, widths[index])));
            if index + 1 < self.headers.len() {
                out.push_str("  ");
            }
        }
        out.push('\n');
        for row in &self.rows {
            push_row(row, &mut out);
        }
        out
    }
}

/// Prompt for a yes/no answer.
///
/// Defaults matter: `default` is what an empty line means, and it is also the
/// answer used when stdin is not a terminal. That second part is what lets a
/// script run without hanging, and it is why every destructive prompt's default
/// is `false`.
///
/// ## Why the prompt goes to stderr
///
/// stdout is reserved for the command's actual result. A prompt is interaction,
/// not output, and writing it to stdout broke `--json`: a confirmation question
/// would be interleaved with the JSON document and make it unparsable. It also
/// keeps prompts out of any log that captures stdout.
pub fn confirm(question: &str, default: bool) -> bool {
    if !std::io::stdin().is_terminal() {
        return default;
    }
    let hint = if default { "[Y/n]" } else { "[y/N]" };
    let mut stderr = std::io::stderr();
    let _ = write!(stderr, "{question} {hint} ");
    let _ = stderr.flush();
    drop(stderr);
    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() {
        return default;
    }
    match input.trim().to_ascii_lowercase().as_str() {
        "" => default,
        "y" | "yes" => true,
        "n" | "no" => false,
        _ => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_units_step_at_1024() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1024), "1.0 KiB");
        assert_eq!(bytes(1024 * 1024), "1.0 MiB");
        assert_eq!(bytes(3 * 1024 * 1024 * 1024), "3.0 GiB");
    }

    #[test]
    fn progress_withholds_a_percentage_for_unknown_totals() {
        assert_eq!(progress_text(100, None), "100 B");
        assert_eq!(progress_text(100, Some(0)), "100 B");
        assert!(progress_text(50, Some(100)).contains("50%"));
    }

    #[test]
    fn colour_mode_parsing_rejects_nonsense() {
        assert_eq!(ColourMode::parse("auto").unwrap(), ColourMode::Auto);
        assert_eq!(ColourMode::parse("ALWAYS").unwrap(), ColourMode::Always);
        assert_eq!(ColourMode::parse("never").unwrap(), ColourMode::Never);
        assert!(ColourMode::parse("rainbow").is_err());
    }

    #[test]
    fn display_width_counts_cjk_as_two_columns() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("实例"), 4);
        assert_eq!(display_width("a实例"), 5);
    }

    #[test]
    fn display_width_ignores_ansi_sequences() {
        assert_eq!(display_width("\x1b[32mok\x1b[0m"), 2);
    }

    #[test]
    fn truncate_respects_width_and_marks_the_cut() {
        assert_eq!(truncate("abcdef", 10), "abcdef");
        let cut = truncate("abcdefghij", 5);
        assert_eq!(display_width(&cut), 5);
        assert!(cut.ends_with('…'));
        // A wide character must not overflow the requested width.
        let wide = truncate("实例实例实例", 5);
        assert!(display_width(&wide) <= 5);
    }

    #[test]
    fn table_pads_columns_to_align() {
        let palette = Palette::new(false);
        let mut table = Table::new(&["名称", "大小"]);
        table.row(vec!["a".into(), "1 B".into()]);
        table.row(vec!["longer".into(), "10 B".into()]);
        let rendered = table.render(&palette, 80);
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines.len(), 3);
        // "longer" is the widest cell in column 0; the short cell must be padded.
        assert!(lines[1].starts_with("a     "), "got {:?}", lines[1]);
    }

    #[test]
    fn table_shrinks_the_widest_free_text_column_to_fit() {
        let palette = Palette::new(false);
        let mut table = Table::new(&["name", "description"]);
        table.row(vec!["x".into(), "y".repeat(200)]);
        let rendered = table.render(&palette, 40);
        for line in rendered.lines() {
            assert!(
                display_width(line) <= 40,
                "line overflows 40 columns: {line:?}"
            );
        }
    }

    #[test]
    fn table_never_shrinks_numeric_columns() {
        let palette = Palette::new(false);
        let mut table = Table::new(&["name", "bytes"]).numeric(&[1]);
        table.row(vec!["x".into(), "123456789".into()]);
        let rendered = table.render(&palette, 12);
        assert!(
            rendered.contains("123456789"),
            "a numeric column must keep every digit: {rendered:?}"
        );
    }

    #[test]
    fn timestamp_falls_back_for_unparsable_values() {
        assert_eq!(timestamp("not a date"), "not a date");
        assert_eq!(timestamp("2026-11-07T05:03:11+00:00"), "2026-11-07 05:03");
    }

    #[test]
    fn rates_use_the_unit_that_keeps_the_number_readable() {
        assert_eq!(rate(512.0), "512 B/s");
        assert_eq!(rate(1024.0), "1.0 KiB/s");
        assert_eq!(rate(19_400_000.0), "18.5 MiB/s");
        // A stalled or not-yet-measured rate must not print "0 B/s", which reads
        // as a broken transfer rather than an unknown one.
        assert_eq!(rate(0.0), "—");
        assert_eq!(rate(f64::NAN), "—");
        assert_eq!(rate(-5.0), "—");
    }

    #[test]
    fn etas_are_bounded_and_never_negative() {
        assert_eq!(eta(1000, 1000.0), "1s");
        assert_eq!(eta(90_000, 1000.0), "1m 30s");
        assert_eq!(eta(3_900_000, 1000.0), "1h 05m");
        // An unknown rate has no meaningful estimate.
        assert_eq!(eta(1000, 0.0), "—");
        // A year's worth of remaining bytes clamps instead of printing "8760h".
        assert_eq!(eta(u64::MAX, 1.0), "1h 39m");
        assert_eq!(eta(u64::MAX, f64::INFINITY), "—");
    }

    #[test]
    fn durations_are_compact() {
        assert_eq!(duration(0.0), "0s");
        assert_eq!(duration(59.9), "59s");
        assert_eq!(duration(125.0), "2m 05s");
        assert_eq!(duration(3725.0), "1h 02m");
    }
}
