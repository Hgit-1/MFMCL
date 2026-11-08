//! The MFMCL launcher binary.
//!
//! ## One binary, two frontends (Unix)
//!
//! On Linux and macOS this executable opens the window when run with no arguments
//! and serves `mfmcl cli <function>` when the first argument is `cli`. Those
//! platforms have no subsystem distinction, so a single executable can do both —
//! which is why the `.deb`, `.rpm` and AppImage ship only this file.
//!
//! ## Windows is GUI-only
//!
//! A Windows `windows`-subsystem process has no console, and no amount of
//! `AttachConsole` plus handle rebinding makes `println!` reach a ConPTY: both were
//! measured while building this. Shipping the CLI inside the Windows binary would
//! mean either a silent CLI (no progress, no errors, no `--json`) or a console
//! window flashing on every desktop launch. Neither is acceptable, so the Windows
//! build keeps `mfmcl.exe` (this, GUI) and a separate `mfmcl-cli.exe` (terminal).
//!
//! The subsystem attribute below is therefore Windows-only: on Unix a console
//! subsystem is what makes terminal output work, and there is no window to hide.

// Windows: no console window for a desktop app.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    mfmcl_desktop_lib::run();
}
