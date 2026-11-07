//! The seam between orchestration and a user interface.
//!
//! Every event here mirrors a Tauri event name that already existed, so the
//! desktop shell keeps emitting exactly what the React frontend listens for. The
//! names are preserved as string constants for that reason — renaming them would
//! silently break the existing UI.

use download_engine::DownloadProgress;
use launcher_core::DownloadTask;
use serde::{Deserialize, Serialize};

/// Event names, unchanged from the desktop implementation.
pub mod names {
    pub const DOWNLOAD_PROGRESS: &str = "download-progress";
    pub const INSTALLATION_TASK: &str = "installation-task";
    pub const LAUNCH_PROGRESS: &str = "launch-progress";
    pub const GAME_LOG_LINE: &str = "game-log-line";
    pub const GAME_PROCESS: &str = "game-process";
    pub const PLUGIN_BACKUP: &str = "plugin-backup";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressEvent {
    pub task_id: String,
    pub instance_id: String,
    #[serde(flatten)]
    pub progress: DownloadProgress,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskEvent {
    pub instance_id: String,
    pub task: DownloadTask,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchProgressEvent {
    pub instance_id: String,
    /// Stable stage key: `preparing` / `authenticating` / `building` / `starting` / `running`.
    pub stage: String,
    pub progress: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameLogLine {
    pub instance_id: String,
    pub line: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameProcessEvent {
    pub instance_id: String,
    /// `running` / `stopped` / `crashed`.
    pub status: String,
    pub exit_code: Option<i32>,
    pub log_path: String,
}

/// Receives everything a frontend needs to display.
///
/// Implementations must not block: the launch path calls these from async code
/// while holding no locks, and a slow sink would stall downloads. A CLI sink
/// that writes to stdout satisfies this; one that pops a dialog would not, which
/// is exactly the sort of thing that belongs in the frontend, not here.
pub trait EventSink: Send + Sync {
    fn download_progress(&self, event: &ProgressEvent);
    fn installation_task(&self, event: &TaskEvent);
    fn launch_progress(&self, event: &LaunchProgressEvent);
    fn game_log(&self, event: &GameLogLine);
    fn game_process(&self, event: &GameProcessEvent);

    /// A plugin's pre-launch backup finished. Default: ignore.
    fn plugin_backup(&self, _instance_id: &str, _plugin_id: &str, _path: &std::path::Path) {}
}

/// Discards every event. Useful in tests and for `--quiet`.
pub struct NoopSink;

impl EventSink for NoopSink {
    fn download_progress(&self, _event: &ProgressEvent) {}
    fn installation_task(&self, _event: &TaskEvent) {}
    fn launch_progress(&self, _event: &LaunchProgressEvent) {}
    fn game_log(&self, _event: &GameLogLine) {}
    fn game_process(&self, _event: &GameProcessEvent) {}
}

/// Records events for assertions.
///
/// This is what lets the tests below prove that the orchestration actually emits
/// the events the frontends listen for, rather than merely compiling.
#[derive(Default)]
pub struct VecSink {
    pub tasks: std::sync::Mutex<Vec<TaskEvent>>,
    pub progress: std::sync::Mutex<Vec<ProgressEvent>>,
    pub launches: std::sync::Mutex<Vec<LaunchProgressEvent>>,
    pub logs: std::sync::Mutex<Vec<GameLogLine>>,
    pub processes: std::sync::Mutex<Vec<GameProcessEvent>>,
}

impl VecSink {
    pub fn new() -> Self {
        Self::default()
    }

    fn push<T>(slot: &std::sync::Mutex<Vec<T>>, value: T) {
        if let Ok(mut guard) = slot.lock() {
            guard.push(value);
        }
    }
}

impl EventSink for VecSink {
    fn download_progress(&self, event: &ProgressEvent) {
        Self::push(&self.progress, event.clone());
    }
    fn installation_task(&self, event: &TaskEvent) {
        Self::push(&self.tasks, event.clone());
    }
    fn launch_progress(&self, event: &LaunchProgressEvent) {
        Self::push(&self.launches, event.clone());
    }
    fn game_log(&self, event: &GameLogLine) {
        Self::push(&self.logs, event.clone());
    }
    fn game_process(&self, event: &GameProcessEvent) {
        Self::push(&self.processes, event.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_sink_methods_are_inert() {
        // `plugin_backup` has a default body; adding it must not force every
        // implementor to write an empty method.
        struct Minimal;
        impl EventSink for Minimal {
            fn download_progress(&self, _event: &ProgressEvent) {}
            fn installation_task(&self, _event: &TaskEvent) {}
            fn launch_progress(&self, _event: &LaunchProgressEvent) {}
            fn game_log(&self, _event: &GameLogLine) {}
            fn game_process(&self, _event: &GameProcessEvent) {}
        }
        Minimal.plugin_backup("a", "b", std::path::Path::new("/tmp"));
    }

    #[test]
    fn event_names_match_the_frontend_contract() {
        // The React side listens on these exact strings. Changing one here
        // without changing the frontend silently stops that UI from updating.
        assert_eq!(names::DOWNLOAD_PROGRESS, "download-progress");
        assert_eq!(names::INSTALLATION_TASK, "installation-task");
        assert_eq!(names::LAUNCH_PROGRESS, "launch-progress");
        assert_eq!(names::GAME_LOG_LINE, "game-log-line");
        assert_eq!(names::GAME_PROCESS, "game-process");
        assert_eq!(names::PLUGIN_BACKUP, "plugin-backup");
    }

    #[test]
    fn vec_sink_records_every_event_type() {
        let sink = VecSink::new();
        sink.launch_progress(&LaunchProgressEvent {
            instance_id: "i".into(),
            stage: "preparing".into(),
            progress: 0.25,
        });
        sink.game_log(&GameLogLine {
            instance_id: "i".into(),
            line: "hello".into(),
        });
        sink.game_process(&GameProcessEvent {
            instance_id: "i".into(),
            status: "running".into(),
            exit_code: None,
            log_path: "x".into(),
        });
        assert_eq!(sink.launches.lock().unwrap().len(), 1);
        assert_eq!(sink.logs.lock().unwrap()[0].line, "hello");
        assert_eq!(sink.processes.lock().unwrap()[0].status, "running");
    }
}
