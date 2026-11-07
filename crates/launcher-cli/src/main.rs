//! Command-line frontend for MFMCL.
//!
//! ## Where the logic lives
//!
//! Nothing in this crate downloads, installs, or launches anything itself. Every
//! command is a thin adapter over [`launcher_runtime`], which the desktop shell is
//! also meant to use. That is deliberate: two copies of "how to launch" drift, and
//! the drift shows up as "works in the GUI, fails in the CLI".
//!
//! What *is* here is everything that makes a terminal pleasant to use: argument
//! parsing and validation, progress rendering, prompts, tables, and exit codes.
//!
//! ## Exit codes
//!
//! Scripts branch on these, so they are part of the interface:
//!
//!   0  success
//!   1  the command failed (network, disk, missing file, refused by policy)
//!   2  the command line itself was wrong (unknown flag, bad value)
//!   3  the user declined a confirmation, or a destructive action needed `--yes`
//!
//! ## The one `unsafe` block
//!
//! `deny` rather than `forbid`, because setting the Windows console output code
//! page has no safe wrapper in Rust's standard library and it is the only way to
//! keep Chinese output readable on a default Chinese Windows console. There is
//! exactly one call site (see `console.rs`), it is marked `#[allow(unsafe_code)]`
//! with a safety comment, and it is compiled out on every other platform.

#![deny(unsafe_code)]

mod cli;
mod commands;
mod console;
mod output;
mod sink;

use clap::Parser;
use std::process::ExitCode;

use cli::{Cli, Command};
use output::{ColourMode, Palette};
use sink::{SinkOptions, TerminalSink, Verbosity};

/// Exit code for a failed command.
const EXIT_FAILURE: u8 = 1;
/// Exit code for an invalid command line (clap uses 2 natively).
const EXIT_USAGE: u8 = 2;
/// Exit code for "the user said no" / "confirmation missing".
const EXIT_DECLINED: u8 = 3;

fn main() -> ExitCode {
    // Before anything is printed: clap's own help is Chinese too.
    console::use_utf8_output();

    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            // clap renders help and version to stdout; a real error goes to stderr.
            // Letting clap print keeps its formatting and its exit code (2).
            let _ = error.print();
            return ExitCode::from(if error.use_stderr() { EXIT_USAGE } else { 0 });
        }
    };

    let colour = match ColourMode::parse(&cli.colour) {
        Ok(mode) => mode,
        Err(message) => {
            eprintln!("错误：{message}");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    let palette = Palette::new(colour.enabled() && !cli.json);
    let verbosity = if cli.quiet {
        Verbosity::Quiet
    } else {
        Verbosity::Normal
    };

    /*
     * Documentation is answered before any I/O. `mfmcl explain download` must work
     * on a machine with no launcher data directory, no network, and no Java —
     * asking for documentation should never fail for environmental reasons.
     *
     * `explain` rather than `help` because `help` is clap's own subcommand and
     * shadowing it would break `mfmcl help download`.
     */
    if let Command::Help { function } = &cli.command {
        commands::help::run(function.as_deref(), &palette);
        return ExitCode::SUCCESS;
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
            return ExitCode::from(EXIT_FAILURE);
        }
    };

    let outcome = runtime.block_on(commands::dispatch(&cli, &sink));
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(commands::Failure::Declined(message)) => {
            sink_note(&message);
            ExitCode::from(EXIT_DECLINED)
        }
        Err(commands::Failure::Message(message)) => {
            sink_note(&message);
            ExitCode::from(EXIT_FAILURE)
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
