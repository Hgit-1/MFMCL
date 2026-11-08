//! Child-process hygiene shared by every crate that spawns a program.
//!
//! ## Why this exists
//!
//! The desktop binary is a `windows`-subsystem application: it has no console of
//! its own. When such a process spawns a console program, Windows gives that child
//! a **new console window**, which appears on screen for as long as the child runs.
//!
//! That is invisible while the parent is a terminal program, because the child
//! inherits the parent's console instead. It became visible only after the desktop
//! build was switched to the GUI subsystem, at which point startup began flashing
//! windows: the frontend asks for the installed Java runtimes on load, and that
//! probe runs `java -version` once per candidate path — up to 128 of them.
//!
//! `CREATE_NO_WINDOW` tells Windows to run the child without any console window.
//! The child keeps working (its stdout and stderr are still captured through pipes),
//! it simply has nowhere to draw a window.
//!
//! Every `Command` created for a short-lived helper must go through [`hide_console`].
//! The game process itself is spawned by `launcher-runtime`, which applies the same
//! rule there because a game must not flash a console either.

use std::process::Command;

/// Windows flag: run the process without creating a console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Keep a child process from opening its own console window.
///
/// A no-op off Windows, where this problem does not exist: a process started from a
/// terminal inherits that terminal, and one started from a desktop session has no
/// console to create.
pub fn hide_console(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// The same, for an async `Command`.
pub fn hide_console_async(command: &mut tokio::process::Command) -> &mut tokio::process::Command {
    #[cfg(windows)]
    {
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wrapped_command_still_runs() {
        /*
         * The flag is only meaningful on Windows, but the wrapper must never change
         * behaviour: this asserts a real process still runs and reports its output on
         * whichever platform the tests are built for.
         */
        let mut command = Command::new(if cfg!(windows) { "cmd" } else { "echo" });
        if cfg!(windows) {
            command.args(["/C", "echo", "ok"]);
        } else {
            command.arg("ok");
        }
        let output = hide_console(&mut command).output().expect("spawn");
        assert!(output.status.success());
    }

    #[tokio::test]
    async fn an_async_wrapped_command_still_runs() {
        let mut command = tokio::process::Command::new(if cfg!(windows) { "cmd" } else { "echo" });
        if cfg!(windows) {
            command.args(["/C", "echo", "ok"]);
        } else {
            command.arg("ok");
        }
        let output = hide_console_async(&mut command)
            .output()
            .await
            .expect("spawn");
        assert!(output.status.success());
    }
}
