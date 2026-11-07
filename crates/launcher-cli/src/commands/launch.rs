//! `mfmcl launch` — start the game and stream its output.
//!
//! ## Blocking by default
//!
//! `mfmcl launch` waits for the game to exit, so a script can `&&` a follow-up
//! step and a person sees the exit code. `--detach` covers the other case (start
//! it and walk away), which cannot be the default: the process would otherwise be
//! orphaned with nothing to report when it crashes.
//!
//! ## Offline confirmation
//!
//! An offline profile proves nothing about Minecraft ownership. The runtime refuses
//! to launch one unless the caller confirms, and the CLI makes that confirmation
//! explicit (`--offline-ok` or an interactive prompt). Making it implicit would
//! turn "just run it" into a silent ownership bypass.

use std::sync::Arc;

use crate::cli::LaunchArgs;
use crate::commands::{Failure, Outcome, Session, resolve_account, resolve_instance};
use crate::output::Palette;
use launcher_core::AccountKind;
use launcher_runtime::launch::{LaunchRequest, ProcessRegistry};

pub async fn run(session: &Session<'_>, args: &LaunchArgs) -> Outcome {
    let palette = Palette::new(session.sink.palette().on);

    let instance = match (&args.instance, args.last) {
        (Some(target), _) => resolve_instance(&session.store, target)?,
        (None, true) => crate::commands::last_instance(&session.store)?,
        (None, false) => {
            return Err(Failure::message(
                "请用 --instance <名称> 指定实例，或用 --last 启动上次使用的实例。",
            ));
        }
    };
    let account = resolve_account(&session.store, args.account.as_deref())?;

    /*
     * The runtime requires an explicit offline confirmation on every launch. Ask
     * once here rather than in the runtime: the wording is interface-specific, and
     * a GUI shows a dialog for the same flag.
     */
    if account.kind == AccountKind::Offline && !args.offline_ok {
        let name = &account.display_name;
        let accepted = session.confirm(&format!(
            "离线档案「{name}」不验证 Minecraft 正版所有权，仅适用于单人或已授权的服务器。继续启动？"
        ));
        if !accepted {
            return Err(Failure::Declined(
                "已取消启动。确认后可用 --offline-ok 跳过这个提示。".to_owned(),
            ));
        }
    }

    if !session.cli.quiet {
        session.sink.note(&format!(
            "启动「{}」（Minecraft {}，{}）",
            instance.name, instance.game_version, instance.loader
        ));
    }

    let registry = ProcessRegistry::default();
    let sink: Arc<dyn launcher_runtime::EventSink> = if args.no_log {
        // The log still goes to disk; only terminal forwarding is off.
        Arc::new(crate::sink::TerminalSink::new(crate::sink::SinkOptions {
            verbosity: session.sink.verbosity(),
            interactive: session.sink.is_interactive(),
            game_log: false,
            palette: *session.sink.palette(),
        }))
    } else {
        Arc::new(crate::sink::TerminalSink::new(crate::sink::SinkOptions {
            verbosity: session.sink.verbosity(),
            interactive: session.sink.is_interactive(),
            game_log: true,
            palette: *session.sink.palette(),
        }))
    };

    let running = launcher_runtime::launch::launch(
        &session.context,
        &session.store,
        &registry,
        sink,
        LaunchRequest {
            instance_id: instance.id.clone(),
            account_id: account.id.clone(),
            offline_confirmed: args.offline_ok || account.kind != AccountKind::Offline,
            java_path: args.java.clone(),
        },
    )
    .await
    .map_err(Failure::from)?;

    if args.detach {
        /*
         * Detaching still leaves the watcher task running inside this process, so
         * exiting immediately would kill it — and with it the log drain. Spawning
         * the wait on a background task and returning would do the same thing once
         * `main` returns.
         *
         * The honest behaviour is to say so rather than pretend the game survives a
         * closed shell: the game process itself is independent, and only the log
         * forwarding ends.
         */
        session.sink.note(&format!(
            "游戏已在后台启动；本进程退出后将不再转发日志。完整日志：{}",
            running.log_path.display()
        ));
        return Ok(());
    }

    let event = running.wait().await;
    match event.status.as_str() {
        "stopped" => {
            if !session.cli.quiet {
                session.sink.note(&palette.green(&format!(
                    "游戏已退出（退出码 {}）",
                    event.exit_code.unwrap_or(0)
                )));
            }
            Ok(())
        }
        _ => Err(Failure::message(format!(
            "游戏异常退出（退出码 {}）。\n日志：{}",
            event
                .exit_code
                .map_or_else(|| "未知".to_owned(), |code| code.to_string()),
            event.log_path
        ))),
    }
}
