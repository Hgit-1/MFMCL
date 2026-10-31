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
    /**
     * 期望命中的规则 id。
     *
     * `None` 表示这类日志**没有**关键词规则能覆盖（hs_err 原生崩溃、看门狗超时），
     * 结论完全来自堆栈结构分析。这种日志仍然必须通过 `expected_cause` 断言，
     * 否则「什么都没识别出来」会被当成通过。
     */
    expected: Option<&'static str>,
    /// 期望的因果类别；用于那些没有关键词规则、只能靠堆栈判断的日志。
    expected_cause: Option<&'static str>,
}

fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            name: "heap-too-large",
            log: include_str!("../fixtures/heap-too-large.log"),
            expected: Some("heap.reserve"),
            expected_cause: Some("jvmArguments"),
        },
        Fixture {
            name: "old-java",
            log: include_str!("../fixtures/old-java.log"),
            expected: Some("java.class.version"),
            expected_cause: Some("javaVersion"),
        },
        Fixture {
            name: "out-of-memory",
            log: include_str!("../fixtures/out-of-memory.log"),
            expected: Some("runtime.outofmemory"),
            expected_cause: Some("outOfMemory"),
        },
        Fixture {
            name: "missing-mod-dependency",
            log: include_str!("../fixtures/missing-mod-dependency.log"),
            expected: Some("mods.missing.dependency"),
            expected_cause: Some("mods"),
        },
        Fixture {
            name: "opengl-failure",
            log: include_str!("../fixtures/opengl-failure.log"),
            expected: Some("gpu.opengl"),
            expected_cause: Some("graphics"),
        },
        Fixture {
            name: "clean-launch",
            log: include_str!("../fixtures/clean-launch.log"),
            expected: None,
            expected_cause: None,
        },
        /*
         * 没有 Java 异常的情况：JVM 原生崩溃（hs_err）与看门狗超时。两者以前都会
         * 落到「未知」，而它们恰恰是最难让用户自己看懂的两类。
         */
        Fixture {
            name: "hs-err-gpu",
            log: include_str!("../fixtures/hs-err-gpu.log"),
            expected: None,
            expected_cause: Some("graphics"),
        },
        Fixture {
            name: "watchdog-timeout",
            log: include_str!("../fixtures/watchdog-timeout.log"),
            expected: None,
            expected_cause: Some("externalKill"),
        },
        Fixture {
            name: "duplicate-mod",
            log: include_str!("../fixtures/duplicate-mod.log"),
            expected: Some("mods.duplicate"),
            expected_cause: Some("mods"),
        },
        Fixture {
            name: "permission-denied",
            log: include_str!("../fixtures/permission-denied.log"),
            expected: Some("files.access.denied"),
            expected_cause: Some("fileAccess"),
        },
    ]
}

fn self_test() -> Result<(), Box<dyn std::error::Error>> {
    let mut failures = 0;
    for fixture in fixtures() {
        let (findings, facts, _warnings) =
            crash_ai::analyse_log(fixture.log, &Environment::default());
        let codes: Vec<&str> = findings.iter().map(|item| item.code.as_str()).collect();

        if fixture.expected.is_none() && fixture.expected_cause.is_none() {
            if !findings.is_empty() {
                eprintln!("FAIL {}: expected no findings, got {codes:?}", fixture.name);
                failures += 1;
            } else {
                println!("ok   {} (no findings, {} lines)", fixture.name, facts.total_lines);
            }
            continue;
        }

        if let Some(expected) = fixture.expected
            && !codes.contains(&expected)
        {
            eprintln!(
                "FAIL {}: expected rule {expected}, got {codes:?}",
                fixture.name
            );
            failures += 1;
            continue;
        }
        /*
         * 因果类别单独校验。
         *
         * 有些日志（hs_err、看门狗）没有任何关键词规则能命中——它们的信息全在
         * 结构里，不在某个词里。只看规则会把这类日志当成「什么都没发现」。
         */
        if let Some(expected) = fixture.expected_cause {
            let cause = crash_ai::causes::analyse_trace(fixture.log);
            let actual = cause.map(|item| item.kind.as_str().to_owned());
            if actual.as_deref() != Some(expected) {
                eprintln!(
                    "FAIL {}: expected cause {expected}, got {actual:?}",
                    fixture.name
                );
                failures += 1;
                continue;
            }
        }
        let label = match fixture.expected {
            Some(expected) => expected.to_owned(),
            None => fixture
                .expected_cause
                .map(|cause| format!("通过堆栈判定为 {cause}"))
                .unwrap_or_default(),
        };
        println!("ok   {} → {}", fixture.name, label);
    }

    if failures > 0 {
        return Err(format!("{failures} fixture(s) failed").into());
    }
    println!("\nall fixtures passed");
    Ok(())
}
