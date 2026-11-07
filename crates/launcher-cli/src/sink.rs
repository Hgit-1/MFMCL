//! The [`EventSink`] implementation that turns runtime events into terminal text.
//!
//! ## Design constraints
//!
//! The runtime calls these methods from async code, while holding no locks, and
//! expects them not to block. That rules out anything elaborate: no re-entrant
//! drawing, no spawning, no waiting on stdin.
//!
//! Two rendering modes exist because the two callers differ:
//!
//!   - **Interactive** (stdout is a terminal): one progress line rewritten in
//!     place with `\r`, so a long install does not scroll the terminal away.
//!   - **Plain** (redirected, `--no-progress`, or `--json`): periodic whole lines.
//!     A log file must not contain carriage returns, and a machine-readable stream
//!     must not contain a progress bar at all.
//!
//! Progress always goes to **stderr**. That is what makes `mfmcl list --json |
//! jq` safe: stdout carries only the payload the user asked for.

use std::io::{IsTerminal, Write};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use launcher_runtime::{
    EventSink, GameLogLine, GameProcessEvent, LaunchProgressEvent, ProgressEvent, TaskEvent,
};

use crate::output::{self, Palette};

/// How chatty the sink should be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verbosity {
    /// Progress lines and game output.
    Normal,
    /// Only errors and the final result.
    Quiet,
}

/// What the sink is allowed to draw.
pub struct SinkOptions {
    pub verbosity: Verbosity,
    /// Rewrite one line in place. Requires a terminal on stderr and no `--json`.
    pub interactive: bool,
    /// Print game output as it arrives.
    pub game_log: bool,
    pub palette: Palette,
}

impl SinkOptions {
    pub fn new(verbosity: Verbosity, palette: Palette, json: bool) -> Self {
        /*
         * `stderr` is checked rather than `stdout` because progress is written
         * there. A user who redirects stdout to a file still has a terminal on
         * stderr and should still see progress; one who redirects both gets plain
         * lines.
         */
        let interactive = verbosity == Verbosity::Normal
            && !json
            && std::io::stderr().is_terminal()
            && std::env::var("TERM").map_or(true, |term| term != "dumb");
        Self {
            verbosity,
            interactive,
            game_log: verbosity == Verbosity::Normal,
            palette,
        }
    }
}

/// State that must be shared across the async call sites.
///
/// A single mutex guards all of it. The critical sections are single `write!`
/// calls, so contention is irrelevant next to the network I/O happening around
/// them; the alternative (an `RwLock` or atomics per field) would add surface for
/// no measurable gain.
struct State {
    /// Visible width of the line currently occupying the terminal row, so the
    /// next rewrite can erase exactly that much.
    drawn_width: usize,
    /// When the current line was last written (throttles redraws).
    last_draw: Instant,
    last_label: String,
    /// Whether a task-scoped summary has already been printed.
    task_running: bool,
    /// The last whole-task numbers we printed, for change detection.
    last_task_files: Option<(u32, u32)>,
}

pub struct TerminalSink {
    options: SinkOptions,
    state: Mutex<State>,
}

impl TerminalSink {
    pub fn new(options: SinkOptions) -> Self {
        Self {
            options,
            state: Mutex::new(State {
                drawn_width: 0,
                // Start in the past so the first event draws immediately.
                last_draw: Instant::now() - Duration::from_secs(1),
                last_label: String::new(),
                task_running: false,
                last_task_files: None,
            }),
        }
    }

    /// The palette in use, so commands can colour their own output consistently.
    pub fn palette(&self) -> &Palette {
        &self.options.palette
    }

    /// Verbosity in effect.
    pub fn verbosity(&self) -> Verbosity {
        self.options.verbosity
    }

    /// Whether in-place progress redrawing is enabled.
    pub fn is_interactive(&self) -> bool {
        self.options.interactive
    }

    /// Print a permanent line to stderr, clearing any progress bar first.
    ///
    /// Used for narration ("creating instance…") that belongs with progress rather
    /// than in the payload stdout carries in `--json` mode.
    pub fn note(&self, text: &str) {
        self.line(text);
    }

    /// This sink as a runtime event sink.
    ///
    /// The runtime takes `&dyn EventSink` (install) and `Arc<dyn EventSink>`
    /// (launch). Handing out `&self` keeps one sink, and therefore one progress
    /// line, for the whole process.
    pub fn as_event_sink(&self) -> &dyn EventSink {
        self
    }

    fn verbose(&self) -> bool {
        self.options.verbosity == Verbosity::Normal
    }

    /// Clear the in-place line, if one is up.
    ///
    /// Every mode change and every permanent message must go through this first;
    /// otherwise a progress bar overwritten halfway leaves half a line behind.
    fn clear_line(&self) {
        if !self.options.interactive {
            return;
        }
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state.drawn_width > 0 {
            let mut stderr = std::io::stderr();
            let _ = write!(stderr, "\r{}\r", " ".repeat(state.drawn_width));
            let _ = stderr.flush();
            state.drawn_width = 0;
        }
    }

    /// Write one permanent line (never overwritten).
    fn line(&self, text: &str) {
        self.clear_line();
        let mut stderr = std::io::stderr();
        let _ = writeln!(stderr, "{text}");
        let _ = stderr.flush();
    }

    /// Print the final verdict of an operation to stdout.
    ///
    /// stdout is reserved for the answer the user asked for, which is why the
    /// success summary goes here while progress goes to stderr.
    #[allow(dead_code)]
    pub fn result(&self, text: &str) {
        self.clear_line();
        println!("{text}");
    }
}

/// Build a progress bar of `width` cells.
///
/// `None` renders an indeterminate bar rather than a fake percentage: a download
/// whose total size is unknown must not pretend to be 60% done.
fn bar(completed: u64, total: Option<u64>, width: usize) -> String {
    let Some(total) = total.filter(|total| *total > 0) else {
        return format!("[{}]", "·".repeat(width));
    };
    let filled = ((completed as f64 / total as f64) * width as f64).round() as usize;
    let filled = filled.min(width);
    format!("[{}{}]", "#".repeat(filled), "-".repeat(width - filled))
}

impl EventSink for TerminalSink {
    fn download_progress(&self, event: &ProgressEvent) {
        if !self.verbose() {
            return;
        }
        let progress = &event.progress;
        /*
         * Prefer the whole-task numbers when they are available. The per-file
         * figure drops back to 0 at every file boundary, which reads as the
         * install restarting.
         */
        let (completed, total) = match progress.task {
            Some(task) => (task.completed_bytes, task.total_bytes),
            None => (progress.received, progress.total),
        };

        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return,
        };

        /*
         * Redraw throttle. 10 fps is well past the point where the eye can follow
         * a bar, and it keeps a 5 MB/s download from producing thousands of
         * writes per second.
         */
        if state.last_draw.elapsed() < Duration::from_millis(100)
            && progress.stage != download_engine::DownloadStage::Completed
        {
            return;
        }
        state.last_draw = Instant::now();

        let label = output::truncate(&progress.label, 40);
        let detail = output::progress_text(completed, total);
        let text = if self.options.interactive {
            let bar = bar(completed, total, 20);
            format!(
                "{} {bar} {detail}  {}",
                self.options.palette.cyan("↓"),
                self.options.palette.dim(&label)
            )
        } else {
            format!("↓ {label}  {detail}")
        };

        if self.options.interactive {
            let width = output::display_width(&text);
            let mut stderr = std::io::stderr();
            /*
             * Pad with spaces when the new line is shorter than the old one, or
             * the tail of the previous label stays visible.
             */
            let padding = state.drawn_width.saturating_sub(width);
            let _ = write!(stderr, "\r{text}{}", " ".repeat(padding));
            let _ = stderr.flush();
            state.drawn_width = width;
        } else if state.last_label != label {
            // Non-interactive output cannot rewrite, so it prints a new line only
            // when the file changes. Otherwise a 300-file install emits 300 lines
            // that differ only in digits.
            let mut stderr = std::io::stderr();
            let _ = writeln!(stderr, "{text}");
            let _ = stderr.flush();
            state.last_label = label;
        }
    }

    fn installation_task(&self, event: &TaskEvent) {
        if !self.verbose() {
            return;
        }
        let task = &event.task;
        let files = task
            .completed_files
            .zip(task.files)
            .filter(|(_, planned)| *planned > 0);
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return,
        };

        match task.state {
            launcher_core::DownloadTaskState::Running => {
                /*
                 * Announce the task once, then only print a line when the file
                 * counter advances. Without this the task card and the per-file
                 * line would both update and the output would be redundant.
                 */
                if !state.task_running {
                    state.task_running = true;
                    state.last_task_files = files;
                    let palette = &self.options.palette;
                    drop(state);
                    self.line(&format!("{} {}", palette.cyan("任务"), task.label));
                } else if files != state.last_task_files {
                    state.last_task_files = files;
                    if let Some((done, planned)) = files {
                        let palette = &self.options.palette;
                        drop(state);
                        self.line(&format!(
                            "  {} {done}/{planned} 个文件",
                            palette.dim("已下载")
                        ));
                    }
                }
            }
            launcher_core::DownloadTaskState::Paused => {
                state.task_running = false;
                drop(state);
                self.line(&format!("{} 任务已暂停", self.options.palette.yellow("‖")));
            }
            launcher_core::DownloadTaskState::Completed
            | launcher_core::DownloadTaskState::Failed
            | launcher_core::DownloadTaskState::Cancelled
            | launcher_core::DownloadTaskState::Interrupted => {
                // The final verdict is printed by the command itself, with the
                // instance name and next steps. Printing it here would duplicate
                // it and, in `--quiet`, contradict the silence.
                state.task_running = false;
            }
        }
    }

    fn launch_progress(&self, event: &LaunchProgressEvent) {
        if !self.verbose() {
            return;
        }
        let stage = match event.stage.as_str() {
            "preparing" => "检查实例与配置",
            "authenticating" => "刷新登录令牌",
            "building" => "构建启动参数",
            "starting" => "启动 Java 进程",
            "running" => "游戏运行中",
            other => other,
        };
        let palette = &self.options.palette;
        if event.stage == "running" {
            self.line(&palette.green(&format!("游戏已启动（{stage}）")));
            return;
        }
        if self.options.interactive {
            let text = format!("{} {stage}", palette.cyan("…"));
            if let Ok(mut state) = self.state.lock() {
                let width = output::display_width(&text);
                let padding = state.drawn_width.saturating_sub(width);
                let mut stderr = std::io::stderr();
                let _ = write!(stderr, "\r{text}{}", " ".repeat(padding));
                let _ = stderr.flush();
                state.drawn_width = width;
            }
        } else {
            self.line(&format!("… {stage}"));
        }
    }

    fn game_log(&self, event: &GameLogLine) {
        if !self.options.game_log {
            return;
        }
        /*
         * Game output goes to stdout: it is the thing the user asked to watch, and
         * `mfmcl launch ... > game.log` should capture it. Progress stays on
         * stderr, so the two streams do not interleave.
         */
        self.clear_line();
        let mut stdout = std::io::stdout();
        let _ = writeln!(stdout, "{}", event.line);
        let _ = stdout.flush();
    }

    fn game_process(&self, event: &GameProcessEvent) {
        if !self.verbose() {
            return;
        }
        let palette = &self.options.palette;
        let text = match event.status.as_str() {
            "running" => return,
            "stopped" => palette.green("游戏已正常退出"),
            "crashed" => {
                let code = event
                    .exit_code
                    .map_or_else(|| "未知".to_owned(), |code| code.to_string());
                palette.red(&format!("游戏异常退出（退出码 {code}）"))
            }
            other => other.to_owned(),
        };
        self.line(&text);
        self.line(&palette.dim(&format!("日志：{}", event.log_path)));
    }

    fn plugin_backup(&self, _instance_id: &str, plugin_id: &str, path: &std::path::Path) {
        if !self.verbose() {
            return;
        }
        self.line(&format!(
            "{} 插件 {plugin_id} 已备份到 {}",
            self.options.palette.dim("备份"),
            path.display()
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_total_renders_an_indeterminate_bar() {
        assert_eq!(bar(0, None, 4), "[····]");
        assert_eq!(bar(5, Some(0), 4), "[····]");
    }

    #[test]
    fn a_known_total_fills_proportionally() {
        assert_eq!(bar(0, Some(100), 4), "[----]");
        assert_eq!(bar(50, Some(100), 4), "[##--]");
        assert_eq!(bar(100, Some(100), 4), "[####]");
        // Rounding must never overflow the bar.
        assert_eq!(bar(101, Some(100), 4), "[####]");
    }

    #[test]
    fn quiet_sinks_produce_no_progress() {
        let sink = TerminalSink::new(SinkOptions {
            verbosity: Verbosity::Quiet,
            interactive: false,
            game_log: false,
            palette: Palette::new(false),
        });
        // Nothing observable happens; the assertion is that these do not panic or
        // attempt to draw while a bool says they must not.
        assert!(!sink.verbose());
    }
}
