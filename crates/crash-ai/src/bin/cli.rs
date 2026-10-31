//! Command-line analysis, for testing the pipeline without a browser.
//!
//! Two uses: `cargo run -p crash-ai --bin crash-ai-cli -- <log>` to analyse a
//! file, and `--self-test` to run every fixture through the rules and assert the
//! expected code fires. The self-test exists because the fixtures are the only
//! realistic crash logs available in this repository — a rule that stops
//! matching one of them is a regression nobody would notice otherwise.

use crash_ai::{Environment, llm::LlmConfig};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|arg| arg == "--self-test") {
        return self_test();
    }

    let no_model = args.iter().any(|arg| arg == "--no-model");
    let Some(path) = args.iter().find(|arg| !arg.starts_with("--")) else {
        eprintln!("用法: crash-ai-cli <日志路径> [--no-model]");
        eprintln!("      crash-ai-cli --self-test");
        return Ok(());
    };

    let log = std::fs::read_to_string(PathBuf::from(path))?;
    let config = LlmConfig::default();
    let analysis = crash_ai::analyse(&log, &Environment::default(), &config, !no_model).await;

    println!("{}", serde_json::to_string_pretty(&analysis)?);
    Ok(())
}

/// Fixtures paired with the rule that must fire for them.
///
/// Kept here rather than in `tests/` so the expected mapping is visible next to
/// the fixture names, and so `--self-test` works from a release binary.
struct Fixture {
    name: &'static str,
    log: &'static str,
    expected: &'static str,
}

fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            name: "heap-too-large",
            log: include_str!("../fixtures/heap-too-large.log"),
            expected: "heap.reserve",
        },
        Fixture {
            name: "old-java",
            log: include_str!("../fixtures/old-java.log"),
            expected: "java.class.version",
        },
        Fixture {
            name: "out-of-memory",
            log: include_str!("../fixtures/out-of-memory.log"),
            expected: "runtime.outofmemory",
        },
        Fixture {
            name: "missing-mod-dependency",
            log: include_str!("../fixtures/missing-mod-dependency.log"),
            expected: "mods.missing.dependency",
        },
        Fixture {
            name: "opengl-failure",
            log: include_str!("../fixtures/opengl-failure.log"),
            expected: "gpu.opengl",
        },
        Fixture {
            name: "clean-launch",
            log: include_str!("../fixtures/clean-launch.log"),
            expected: "",
        },
    ]
}

fn self_test() -> Result<(), Box<dyn std::error::Error>> {
    let mut failures = 0;
    for fixture in fixtures() {
        let (findings, facts, _warnings) =
            crash_ai::analyse_log(fixture.log, &Environment::default());
        let codes: Vec<&str> = findings.iter().map(|item| item.code.as_str()).collect();

        if fixture.expected.is_empty() {
            if !findings.is_empty() {
                eprintln!("FAIL {}: expected no findings, got {codes:?}", fixture.name);
                failures += 1;
            } else {
                println!("ok   {} (no findings, {} lines)", fixture.name, facts.total_lines);
            }
            continue;
        }

        if codes.contains(&fixture.expected) {
            println!("ok   {} → {}", fixture.name, fixture.expected);
        } else {
            eprintln!(
                "FAIL {}: expected {}, got {codes:?}",
                fixture.name, fixture.expected
            );
            failures += 1;
        }
    }

    if failures > 0 {
        return Err(format!("{failures} fixture(s) failed").into());
    }
    println!("\nall fixtures passed");
    Ok(())
}
