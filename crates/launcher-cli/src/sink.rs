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
    /// Whether a task line has been printed for the current task.
    task_running: bool,
    /// The last whole-task file counter, so a completed file is announced once.
    last_task_files: Option<(u32, u32)>,
    /// Phase currently being announced, so a phase change prints one line.
    phase: Option<Phase>,
    /// The artifact label last printed as a permanent line.
    last_artifact: String,
    /// Bytes and time at the start of the task, for the closing summary.
    task_started: Option<(Instant, u64)>,
    /// Previous sample, for a smoothed transfer rate.
    rate_sample: Option<(Instant, u64)>,
    /// Exponentially smoothed rate in bytes per second.
    smoothed_rate: f64,
}

impl State {
    fn reset_task(&mut self) {
        self.task_running = false;
        self.last_task_files = None;
        self.phase = None;
        self.last_artifact.clear();
        self.task_started = None;
        self.rate_sample = None;
        self.smoothed_rate = 0.0;
    }
}

/// Which part of an install is running.
///
/// Derived from the artifact label because the installer emits one stream for
/// three conceptually different jobs: the version's own files, the loader
/// overlay, and (for packs) the imported contents. Splitting them is what lets
/// the output say "获取 mod 依赖" instead of listing raw Maven coordinates
/// under one undifferentiated heading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Client,
    Assets,
    Loader,
    Mods,
    Other,
}

impl Phase {
    /// Classify by the label the installer already emits.
    ///
    /// The prefixes are the contract established in `launcher-core`: "依赖 " for
    /// library coordinates, "Minecraft …" for the client jar, "日志配置" for the
    /// log4j config, "资源索引"/"资源 " for asset files, and "加载器依赖 " for
    /// the loader overlay.
    fn of(label: &str) -> Self {
        if label.starts_with("加载器") {
            Self::Loader
        } else if label.starts_with("资源 ") {
            Self::Assets
        } else if label.starts_with("依赖 ")
            || label.starts_with("日志配置")
            || label.starts_with("Minecraft ")
            // The asset *index* is part of the client's own file set, not one of
            // the thousands of asset objects. Grouping it with the assets made the
            // phase flip back and forth at the start of an install.
            || label.starts_with("资源索引")
        {
            Self::Client
        } else if label.starts_with("模组") {
            Self::Mods
        } else {
            Self::Other
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Client => "获取游戏文件",
            Self::Assets => "获取资源文件",
            Self::Loader => "获取加载器文件",
            Self::Mods => "获取模组",
            Self::Other => "获取文件",
        }
    }
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
                task_running: false,
                last_task_files: None,
                phase: None,
                last_artifact: String::new(),
                task_started: None,
                rate_sample: None,
                smoothed_rate: 0.0,
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

/// One line of live progress, in the shape a package manager uses.
///
/// Kept as a pure function so the format can be asserted without a terminal:
///
/// ```text
/// 已下载  47% [==========>          ] 412.3 MiB / 877.7 MiB  18.4 MiB/s  剩余 25s  71/3984
/// ```
///
/// The percentage is right-aligned in three columns so the bar does not shift as
/// digits are added, and an unknown total prints no percentage at all rather than
/// a fabricated one.
fn progress_line(
    completed: u64,
    total: Option<u64>,
    per_second: f64,
    eta: &str,
    counter: Option<(u32, u32)>,
    palette: &Palette,
) -> String {
    let percent = match total.filter(|total| *total > 0) {
        Some(total) => format!(
            "{:>3.0}%",
            (completed as f64 / total as f64 * 100.0).clamp(0.0, 100.0)
        ),
        None => "  ?%".to_owned(),
    };
    let totals = match total.filter(|total| *total > 0) {
        Some(total) => format!("{} / {}", output::bytes(completed), output::bytes(total)),
        None => output::bytes(completed),
    };
    let speed = output::rate(per_second);
    let mut line = format!(
        "{} {percent} {} {totals}",
        palette.dim("已下载"),
        bar(completed, total, 24),
    );
    /*
     * "剩余" is only meaningful while there is something left. It disappears when
     * the byte count is complete *and* every file has been accounted for; checking
     * only the bytes left "剩余 —" on the last file, which reads like an error next
     * to a full bar.
     */
    let bytes_done = total.is_some_and(|total| total > 0 && completed >= total);
    let files_done = counter.is_none_or(|(done, planned)| done >= planned);
    if !(bytes_done && files_done) {
        line.push_str(&format!(
            "  {}  剩余 {}",
            palette.dim(&speed),
            palette.dim(eta)
        ));
    }
    if let Some((done, planned)) = counter {
        line.push_str(&palette.dim(&format!("  {done}/{planned} 个文件")));
    }
    line
}

/// Build a progress bar of `width` cells.
///
/// The `====>` shape is the one apt and dnf use: a filled run with a leading
/// `>` while work remains, so a bar that is 99% done still looks different from a
/// finished one.
///
/// `None` renders an indeterminate bar rather than a fake percentage: a download
/// whose total size is unknown must not pretend to be 60% done.
fn bar(completed: u64, total: Option<u64>, width: usize) -> String {
    let Some(total) = total.filter(|total| *total > 0) else {
        return format!("[{}]", "·".repeat(width));
    };
    if width == 0 {
        return "[]".to_owned();
    }
    let filled = ((completed as f64 / total as f64) * width as f64).round() as usize;
    let filled = filled.min(width);
    if filled >= width {
        format!("[{}]", "=".repeat(width))
    } else if filled == 0 {
        format!("[{}>{}]", "", " ".repeat(width.saturating_sub(1)))
    } else {
        format!(
            "[{}>{}]",
            "=".repeat(filled.saturating_sub(1)),
            " ".repeat(width.saturating_sub(filled))
        )
    }
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
        if state.task_started.is_none() {
            state.task_started = Some((Instant::now(), completed));
            state.rate_sample = Some((Instant::now(), completed));
        }

        /*
         * Announce a new kind of file with a permanent line, like apt names the
         * package being fetched. Only the *kind* is announced — printing one line
         * per artifact would mean several thousand lines on a full install, which
         * is exactly the noise this replaces.
         */
        let phase = Phase::of(&progress.label);
        let phase_changed = state.phase != Some(phase);
        let is_new_artifact = state.last_artifact != progress.label;
        state.phase = Some(phase);

        /*
         * The rate is smoothed so the line does not flicker between "40 MiB/s" and
         * "0 B/s" as individual files connect and finish. Weighting the new sample
         * lightly keeps it responsive without chasing every gap.
         */
        let (smoothed, eta) = {
            let now = Instant::now();
            let mut rate = state.smoothed_rate;
            if let Some((then, bytes_then)) = state.rate_sample {
                let elapsed = now.duration_since(then).as_secs_f64();
                // Sample every 500 ms: finer than that measures jitter, not speed.
                if elapsed >= 0.5 && completed >= bytes_then {
                    let instant = (completed - bytes_then) as f64 / elapsed;
                    rate = if rate <= 0.0 {
                        instant
                    } else {
                        rate * 0.7 + instant * 0.3
                    };
                    state.smoothed_rate = rate;
                    state.rate_sample = Some((now, completed));
                }
            }
            /*
             * With the byte count complete but files still finishing (verification,
             * atomic rename), there is no meaningful time estimate — but "—" next
             * to a full bar reads like a failure. "收尾" says what is actually
             * happening.
             */
            let eta = match total.filter(|total| *total > 0) {
                Some(total) if completed < total => output::eta(total - completed, rate),
                Some(_) => "收尾".to_owned(),
                None => "—".to_owned(),
            };
            (rate, eta)
        };

        /*
         * Throttle the in-place line to ~8 fps. Anything faster is invisible and
         * costs a syscall per refresh.
         */
        let finished = progress.stage == download_engine::DownloadStage::Completed;
        if !finished && state.last_draw.elapsed() < Duration::from_millis(120) {
            return;
        }
        state.last_draw = Instant::now();

        let palette = self.options.palette;
        let counter = match progress.task {
            Some(task) if task.files > 0 => Some((task.completed_files, task.files)),
            _ => None,
        };
        let line = progress_line(completed, total, smoothed, &eta, counter, &palette);

        if self.options.interactive {
            let width = output::display_width(&line);
            let padding = state.drawn_width.saturating_sub(width);
            let mut stderr = std::io::stderr();
            let _ = write!(stderr, "\r{line}{}", " ".repeat(padding));
            let _ = stderr.flush();
            state.drawn_width = width;
        } else if phase_changed {
            /*
             * A redirected stream cannot rewrite a line, so the apt shape is
             * impossible. Printing one line per file would put several thousand
             * lines in a log; printing one line per *phase*, with the running
             * total and the file count, keeps the log readable and still says
             * where the install is.
             */
            let mut stderr = std::io::stderr();
            let counter = match progress.task {
                Some(task) if task.files > 0 => {
                    format!("（{}/{} 个文件）", task.completed_files, task.files)
                }
                _ => String::new(),
            };
            let totals = match total.filter(|total| *total > 0) {
                Some(total) => format!("{} / {}", output::bytes(completed), output::bytes(total)),
                None => output::bytes(completed),
            };
            let _ = writeln!(
                stderr,
                "{} {}{counter}  {totals}",
                palette.cyan("↓"),
                phase.title()
            );
            let _ = stderr.flush();
        }
        if is_new_artifact {
            state.last_artifact = progress.label.clone();
        }
    }

    fn installation_task(&self, event: &TaskEvent) {
        if !self.verbose() {
            return;
        }
        let task = &event.task;
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return,
        };

        match task.state {
            launcher_core::DownloadTaskState::Running => {
                /*
                 * One heading per task, printed once. Per-file lines are the
                 * download stream's job; printing a file counter here as well
                 * would put two changing lines on the same screen.
                 */
                if !state.task_running {
                    state.task_running = true;
                    state.task_started = Some((Instant::now(), task.completed_bytes));
                    let palette = self.options.palette;
                    drop(state);
                    /*
                     * The label already carries the verb ("安装 1.21.1", "补全 fab"),
                     * so it is printed as-is. Prefixing it printed "安装 安装 1.21.1".
                     */
                    self.line(&palette.bold(&task.label));
                }
            }
            launcher_core::DownloadTaskState::Paused => {
                state.task_running = false;
                drop(state);
                self.line(&format!("{} 任务已暂停", self.options.palette.yellow("‖")));
            }
            launcher_core::DownloadTaskState::Completed => {
                /*
                 * The closing summary, in the shape a package manager prints after
                 * a successful operation: what was transferred and how fast. The
                 * command still prints its own result table afterwards; this answers
                 * "how much did that cost me".
                 */
                let elapsed = state
                    .task_started
                    .map(|(start, _)| start.elapsed().as_secs_f64())
                    .unwrap_or(0.0);
                let bytes = task.completed_bytes;
                let rate = if elapsed > 0.0 {
                    bytes as f64 / elapsed
                } else {
                    0.0
                };
                state.reset_task();
                let palette = self.options.palette;
                drop(state);
                let mut summary =
                    format!("{} 已获取 {}", palette.green("完成"), output::bytes(bytes));
                if let (Some(done), Some(planned)) = (task.completed_files, task.files) {
                    summary.push_str(&format!("，{done}/{planned} 个文件"));
                }
                summary.push_str(&format!(
                    "，用时 {}（平均 {}）",
                    output::duration(elapsed),
                    output::rate(rate)
                ));
                self.line(&summary);
            }
            launcher_core::DownloadTaskState::Failed
            | launcher_core::DownloadTaskState::Cancelled
            | launcher_core::DownloadTaskState::Interrupted => {
                /*
                 * The final verdict belongs to the command, which knows the
                 * instance name and the next step. Only the state is cleared so a
                 * retry starts from a clean slate.
                 */
                state.reset_task();
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
    fn a_known_total_fills_with_an_apt_style_head() {
        // The `>` moves left to right and disappears only when the bar is full, so
        // "almost done" never looks identical to "done".
        assert_eq!(bar(0, Some(100), 4), "[>   ]");
        assert_eq!(bar(50, Some(100), 4), "[=>  ]");
        assert_eq!(bar(75, Some(100), 4), "[==> ]");
        assert_eq!(bar(100, Some(100), 4), "[====]");
        // Rounding must never overflow the bar.
        assert_eq!(bar(101, Some(100), 4), "[====]");
    }

    #[test]
    fn a_zero_width_bar_does_not_panic() {
        assert_eq!(bar(50, Some(100), 0), "[]");
    }

    #[test]
    fn the_progress_line_keeps_a_fixed_shape() {
        let palette = Palette::new(false);
        let line = progress_line(
            412 * 1024 * 1024,
            Some(878 * 1024 * 1024),
            19_400_000.0,
            "25s",
            Some((71, 3984)),
            &palette,
        );
        // Fixed columns: the bar must not move as the percentage gains a digit.
        assert!(line.starts_with("已下载  47% [==========>"), "got {line}");
        assert!(line.contains("412.0 MiB / 878.0 MiB"), "got {line}");
        assert!(line.contains("18.5 MiB/s"), "got {line}");
        assert!(line.ends_with("剩余 25s  71/3984 个文件"), "got {line}");
    }

    #[test]
    fn a_finished_line_drops_the_meaningless_remaining_estimate() {
        let palette = Palette::new(false);
        let line = progress_line(524288, Some(524288), 0.0, "—", Some((3984, 3984)), &palette);
        // A full bar with "剩余 —" reads like a failure; it must not appear.
        assert!(!line.contains("剩余"), "got {line}");
        assert!(line.contains("100%"), "got {line}");
        assert!(line.ends_with("3984/3984 个文件"), "got {line}");
    }

    #[test]
    fn remaining_stays_visible_until_the_last_file_is_counted() {
        let palette = Palette::new(false);
        // All bytes transferred, one file still finishing: still in progress.
        let line = progress_line(
            524288,
            Some(524288),
            0.0,
            "2s",
            Some((3983, 3984)),
            &palette,
        );
        assert!(line.contains("剩余 2s"), "got {line}");
    }

    #[test]
    fn an_unknown_total_shows_no_percentage_instead_of_zero() {
        let palette = Palette::new(false);
        let line = progress_line(2048, None, 0.0, "—", None, &palette);
        // A bar with no denominator is indeterminate, and "0%" would be a claim.
        assert!(line.contains("  ?%"), "got {line}");
        assert!(line.contains('·'), "got {line}");
        assert!(!line.contains("剩余 —  0/"), "got {line}");
    }

    #[test]
    fn the_percentage_column_does_not_shift_between_digits() {
        let palette = Palette::new(false);
        let one = progress_line(9, Some(1000), 0.0, "—", None, &palette);
        let two = progress_line(99, Some(1000), 0.0, "—", None, &palette);
        let three = progress_line(999, Some(1000), 0.0, "—", None, &palette);
        // Every line puts "[", at the same column because the width is padded.
        let col = |s: &str| s.find('[').unwrap();
        assert_eq!(col(&one), col(&two));
        assert_eq!(col(&two), col(&three));
    }

    #[test]
    fn phases_are_derived_from_the_installer_labels() {
        // These prefixes are the contract with `launcher-core`; if a label is
        // renamed there, this test is what notices.
        assert_eq!(Phase::of("Minecraft 1.21.1 客户端"), Phase::Client);
        assert_eq!(Phase::of("依赖 com.ibm.icu:icu4j:73.2"), Phase::Client);
        assert_eq!(Phase::of("日志配置 client-1.12.xml"), Phase::Client);
        assert_eq!(Phase::of("资源索引 17"), Phase::Client);
        assert_eq!(Phase::of("资源 minecraft/sounds/a.ogg"), Phase::Assets);
        assert_eq!(
            Phase::of("加载器依赖 net.fabricmc:fabric-loader"),
            Phase::Loader
        );
        assert_eq!(Phase::of("something else"), Phase::Other);
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
