//! Command-line frontend for MFMCL, as a library plus a thin binary.
//!
//! ## Why this is a library
//!
//! The desktop build ships a single executable. On Linux and macOS that binary
//! also answers `mfmcl cli <function>`, because those platforms have no subsystem
//! distinction: one binary can open a window or write a terminal depending only on
//! its arguments.
//!
//! Windows cannot do that. A `windows`-subsystem process has no console attached,
//! and I verified that neither `println!` nor an explicit `AttachConsole` plus
//! handle rebinding delivers a single byte to a ConPTY. That is why the Windows
//! build ships the GUI only and this crate's binary stays a separate executable
//! there.
//!
//! Everything below is frontend code: argument parsing, validation, progress
//! rendering, prompts, tables, and exit codes. Nothing here downloads, installs,
//! or launches anything — that is [`launcher_runtime`], which the desktop shell
//! also uses.
//!
//! ## Exit codes
//!
//! Scripts branch on these, so they are part of the interface:
//!
//!   0  success
//!   1  the command failed (network, disk, missing file, refused by policy)
//!   2  the command line itself was wrong (unknown flag, bad value)
//!   3  the user declined a confirmation, or a destructive action needed `--yes`

#![deny(unsafe_code)]

pub mod cli;
pub mod commands;
pub mod console;
pub mod output;
pub mod sink;

use std::process::ExitCode;

use clap::Parser;

use cli::{Cli, Command};
use output::{ColourMode, Palette};
use sink::{SinkOptions, TerminalSink, Verbosity};

/// Exit code for a failed command.
pub const EXIT_FAILURE: u8 = 1;
/// Exit code for an invalid command line (clap uses 2 natively).
pub const EXIT_USAGE: u8 = 2;
/// Exit code for "the user said no" / "confirmation missing".
pub const EXIT_DECLINED: u8 = 3;
/// Exit code when the platform cannot run the CLI in this binary.
pub const EXIT_UNSUPPORTED: u8 = 4;

/// Run the CLI using the process's own arguments.
///
/// This is what the standalone `mfmcl` binary calls, and what the desktop binary
/// calls on Linux and macOS when it sees `cli` as the first argument.
pub fn run_from_args() -> ExitCode {
    ExitCode::from(exit_code_of(std::env::args().collect()))
}

/// Run the CLI with an explicit argument list, returning the numeric exit code.
///
/// The first element is treated as the program name, as `clap` expects. Callers
/// that are not the process entry point (the desktop shell forwarding `cli …`, or
/// a test) use this; returning a `u8` keeps them off the `ExitCode` conversions,
/// which are only stable in one direction.
pub fn exit_code_of(args: Vec<String>) -> u8 {
    run(args)
}

/// Run the CLI with an explicit argument list and return an `ExitCode`.
pub fn entrypoint(args: Vec<String>) -> ExitCode {
    ExitCode::from(run(args))
}

fn run(args: Vec<String>) -> u8 {
    // Before anything is printed: clap's own help is Chinese too.
    console::use_utf8_output();

    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            // clap renders help and version to stdout; a real error goes to stderr.
            // Letting clap print keeps its formatting and its exit code (2).
            let _ = error.print();
            return if error.use_stderr() { EXIT_USAGE } else { 0 };
        }
    };

    let colour = match ColourMode::parse(&cli.colour) {
        Ok(mode) => mode,
        Err(message) => {
            eprintln!("错误：{message}");
            return EXIT_USAGE;
        }
    };
    let palette = Palette::new(colour.enabled() && !cli.json);
    let verbosity = if cli.quiet {
        Verbosity::Quiet
    } else {
        Verbosity::Normal
    };

    /*
     * Documentation is answered before any I/O. `mfmcl cli help` must work on a
     * machine with no launcher data directory, no network, and no Java — asking
     * for documentation should never fail for environmental reasons.
     */
    if let Command::Help { function } = &cli.command {
        commands::help::run(function.as_deref(), &palette);
        return 0;
    }

    let options = SinkOptions::new(verbosity, palette, cli.json);
    let sink = TerminalSink::new(options);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build();
    let runtime = match runtime {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("错误：无法启动异步运行时：{error}");
            return EXIT_FAILURE;
        }
    };

    let outcome = runtime.block_on(commands::dispatch(&cli, &sink));

    /*
     * Tear the runtime down without waiting for outstanding tasks.
     *
     * Dropping a `Runtime` blocks until every spawned task finishes. `launch`
     * spawns a watcher that outlives the command on purpose (it owns the child
     * process and drains its log), so a plain drop made the process hang at the
     * very end — after all output had been printed — until the user pressed
     * Ctrl+C. `--detach` hit this on every invocation.
     *
     * `shutdown_background` is the documented way to say "do not wait": pending
     * tasks are dropped. Anything that must survive the command is already a
     * separate OS process, and anything that must be flushed has been written
     * before this point.
     */
    runtime.shutdown_background();

    match outcome {
        Ok(()) => 0,
        Err(commands::Failure::Declined(message)) => {
            sink_note(&message);
            EXIT_DECLINED
        }
        Err(commands::Failure::Message(message)) => {
            sink_note(&message);
            EXIT_FAILURE
        }
    }
}

/// Print an error to stderr.
///
/// Errors never go to stdout, not even with `--json`: a caller parsing stdout must
/// be able to tell "the command failed" from "the command returned this".
fn sink_note(message: &str) {
    eprintln!("错误：{message}");
}
