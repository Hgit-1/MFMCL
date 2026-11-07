//! Frontend-agnostic orchestration for MFMCL.
//!
//! ## Why this crate exists
//!
//! Everything the launcher *does* — resolving settings, laying out an instance,
//! downloading verified files, building a JVM command line, spawning Java,
//! draining its output, recording launch history — used to live inside Tauri
//! command handlers in `src-tauri`. That made the desktop shell the only way to
//! run the launcher, and it meant a second frontend (a command line, a TUI, a
//! test harness) would have to either link Tauri or copy the logic.
//!
//! Copying was the real risk. Two copies of "how to launch" drift, and the drift
//! shows up as "it works in the GUI but not in the CLI", which is the worst
//! possible bug to diagnose for a user.
//!
//! This crate is that logic, with exactly two seams where the frontends differ:
//!
//!   - [`RuntimeContext`] supplies the data root and resource directory instead
//!     of `tauri::AppHandle::path()`.
//!   - [`EventSink`] receives progress, log lines, and lifecycle events instead
//!     of `app.emit(...)`.
//!
//! Neither seam knows what a window is. The desktop shell implements `EventSink`
//! by emitting Tauri events; the CLI implements it by printing. Both call the
//! same functions below.
//!
//! ## What is deliberately *not* here
//!
//! window management, the tray icon, the file-picker dialogs, and the Microsoft
//! browser login flow. Those are genuinely GUI concerns; a command line has no
//! window to hide and no tray to click. The Microsoft flow lives in
//! `launcher-auth` and is driven by each frontend as appropriate.

#![forbid(unsafe_code)]

pub mod account;
pub mod authlib_injector;
pub mod backup;
pub mod events;
pub mod install;
pub mod install_support;
pub mod launch;
pub mod plugins;

pub use events::{
    EventSink, GameLogLine, GameProcessEvent, LaunchProgressEvent, NoopSink, ProgressEvent,
    TaskEvent, VecSink,
};
pub use install::Downloads;

use std::path::{Path, PathBuf};

/// Where the runtime reads and writes.
///
/// `data_root` is the launcher's own directory (the desktop app uses
/// `%APPDATA%\dev.mfmcl.launcher`, the CLI defaults to the same place so both
/// see the same instances and accounts). `resource_dir` holds bundled sidecars;
/// it only matters for the pinned-aria2 path.
#[derive(Debug, Clone)]
pub struct RuntimeContext {
    data_root: PathBuf,
    resource_dir: Option<PathBuf>,
}

impl RuntimeContext {
    pub fn new(data_root: impl Into<PathBuf>) -> Self {
        Self {
            data_root: data_root.into(),
            resource_dir: None,
        }
    }

    /// Attach the directory that holds bundled resources (`aria2-manifest.json`).
    ///
    /// Without it the runtime silently uses the Rust downloader, which is the
    /// correct behaviour for a source build: the manifest ships with no binary
    /// pins, so aria2 would not be used anyway.
    pub fn with_resource_dir(mut self, resource_dir: impl Into<PathBuf>) -> Self {
        self.resource_dir = Some(resource_dir.into());
        self
    }

    pub fn data_root(&self) -> &Path {
        &self.data_root
    }

    pub fn resource_dir(&self) -> Option<&Path> {
        self.resource_dir.as_deref()
    }

    /// Create the data root if it does not exist yet.
    ///
    /// Every entry point calls this before touching SQLite: `LauncherStore::open`
    /// creates the parent directory too, but doing it here keeps a clearer error
    /// ("cannot create data directory") when the path is unwritable.
    pub fn prepare(&self) -> Result<()> {
        std::fs::create_dir_all(&self.data_root).map_err(|error| {
            RuntimeError::Io(format!(
                "无法创建数据目录 {}：{error}",
                self.data_root.display()
            ))
        })
    }
}

/// Errors surfaced to a frontend.
///
/// The desktop shell turns these into the same Chinese strings it used to
/// produce inline; keeping them in one enum means the CLI and the GUI cannot
/// disagree about what a failure means.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("{0}")]
    Message(String),
    #[error("{0}")]
    Io(String),
    #[error("{0}")]
    Store(String),
    #[error("{0}")]
    Auth(String),
}

impl RuntimeError {
    pub fn message(text: impl Into<String>) -> Self {
        Self::Message(text.into())
    }
}

impl From<launcher_core::StoreError> for RuntimeError {
    fn from(error: launcher_core::StoreError) -> Self {
        Self::Store(error.to_string())
    }
}

pub type Result<T> = std::result::Result<T, RuntimeError>;
