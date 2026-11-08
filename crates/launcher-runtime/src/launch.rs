//! Launch orchestration: settings merge, argument build, process spawn, log drain.
//!
//! ## What this replaces
//!
//! `src-tauri/src/lib.rs` had a ~440-line `launch_instance` command. Almost all of
//! it is frontend-independent; only four things were not:
//!
//!   - `app.path().app_data_dir()` → [`RuntimeContext`]
//!   - `app.emit(...)` → [`EventSink`]
//!   - `window.hide()` / `window.show()` → not done here at all
//!   - the tray-registry handedness that came with the window
//!
//! Everything else — merging instance overrides, resolving the instance's Java,
//! refreshing an online session, inserting the skin-server agent, applying memory
//! and GC settings, building plugin contributions, running pre-launch commands,
//! spawning the JVM, draining both pipes with redaction, and recording launch
//! history — is the same work regardless of who asked. It lives here so that the
//! GUI and the CLI cannot drift apart on "how does MFMCL actually start the game".

use crate::events::{EventSink, GameLogLine, GameProcessEvent, LaunchProgressEvent};
use crate::{Result, RuntimeContext, RuntimeError};
use launcher_core::{
    AccountKind, AccountProfile, InstanceRecord, InstanceState, LauncherStore, SkippedContribution,
    apply_launch_settings, prepare_authenticated_launch, prepare_offline_launch, resolve_settings,
    system_memory_info, validate_settings,
};
use mc_metadata::GameVersionMetadata;
use std::{
    collections::HashMap,
    process::Stdio,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncWriteExt, BufReader},
    process::Command,
};
use zeroize::Zeroizing;

/// Running game processes, keyed by instance id. Sending `()` asks it to stop.
#[derive(Default, Clone)]
pub struct ProcessRegistry(Arc<Mutex<HashMap<String, tokio::sync::oneshot::Sender<()>>>>);

impl ProcessRegistry {
    /// Register a stop signal for an instance. Fails if it is already running.
    pub fn register(&self, instance_id: &str) -> Result<tokio::sync::oneshot::Receiver<()>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut running = self
            .0
            .lock()
            .map_err(|_| RuntimeError::message("启动器进程状态不可用。"))?;
        if running.contains_key(instance_id) {
            return Err(RuntimeError::message("此实例已经在运行。"));
        }
        running.insert(instance_id.to_owned(), tx);
        Ok(rx)
    }

    /// Signal a running instance to stop.
    pub fn stop(&self, instance_id: &str) -> Result<()> {
        let sender = self
            .0
            .lock()
            .map_err(|_| RuntimeError::message("启动器进程状态不可用。"))?
            .remove(instance_id)
            .ok_or_else(|| RuntimeError::message("此实例当前没有可停止的游戏进程。"))?;
        sender
            .send(())
            .map_err(|_| RuntimeError::message("游戏进程已经退出。"))
    }

    fn release(&self, instance_id: &str) {
        if let Ok(mut running) = self.0.lock() {
            running.remove(instance_id);
        }
    }

    /// Undo a registration whose spawn failed.
    ///
    /// Without this a failed spawn leaves the instance permanently "already
    /// running" — the user's next attempt is refused with a message that has
    /// nothing to do with the real failure.
    pub fn clear(&self, instance_id: &str) {
        self.release(instance_id);
    }

    pub fn is_running(&self, instance_id: &str) -> bool {
        self.0
            .lock()
            .map(|running| running.contains_key(instance_id))
            .unwrap_or(false)
    }
}

/// Everything needed to launch one instance.
///
/// `java_path` is resolved by the caller when it knows the instance's Java rule
/// (the desktop UI evaluates "version range"/"folder" settings client-side).
/// `None` falls back to the global setting and then to auto-detection.
pub struct LaunchRequest {
    pub instance_id: String,
    pub account_id: String,
    /// Offline profiles must be confirmed explicitly on every launch.
    pub offline_confirmed: bool,
    pub java_path: Option<String>,
}

/// A launch that has spawned and is being watched.
///
/// Returned by [`launch`] so a caller that wants to block (the CLI) can await
/// [`RunningGame::wait`]; a caller that does not (the desktop UI) can drop it,
/// because the watcher task owns the child process.
pub struct RunningGame {
    pub log_path: std::path::PathBuf,
    exit: tokio::sync::oneshot::Receiver<GameProcessEvent>,
}

impl RunningGame {
    /// Wait for the game to exit and return the final process event.
    pub async fn wait(self) -> GameProcessEvent {
        self.exit.await.unwrap_or_else(|_| GameProcessEvent {
            instance_id: String::new(),
            status: "crashed".into(),
            exit_code: None,
            log_path: self.log_path.to_string_lossy().into_owned(),
        })
    }
}

/// Launch one instance and start watching it.
///
/// Returns as soon as the JVM process exists. The returned [`RunningGame`] can be
/// awaited for the exit status.
pub async fn launch(
    context: &RuntimeContext,
    store: &LauncherStore,
    registry: &ProcessRegistry,
    sink: Arc<dyn EventSink>,
    request: LaunchRequest,
) -> Result<RunningGame> {
    let LaunchRequest {
        instance_id,
        account_id,
        offline_confirmed,
        java_path,
    } = request;

    let report = {
        let sink = sink.clone();
        let instance_id = instance_id.clone();
        move |stage: &str, progress: f64| {
            sink.launch_progress(&LaunchProgressEvent {
                instance_id: instance_id.clone(),
                stage: stage.to_owned(),
                progress,
            });
        }
    };
    report("preparing", 0.05);

    let instance = crate::install::find_instance(store, &instance_id)?;
    if instance.state != InstanceState::Ready {
        return Err(RuntimeError::message("此实例尚未安装完成。"));
    }
    let account = store
        .list_accounts()?
        .into_iter()
        .find(|account| account.id == account_id)
        .ok_or_else(|| RuntimeError::message("找不到所选账户。"))?;
    if account.kind == AccountKind::Offline && !offline_confirmed {
        return Err(RuntimeError::message("离线账户启动必须先由用户确认。"));
    }

    /*
     * Apply the instance's game-directory override.
     *
     * The override lives in `InstanceOverrides` rather than `InstanceRecord`
     * because the record's `game_dir` is the value chosen at creation time, and
     * changing a record would imply moving files. Here the directory is only
     * what gets passed to the JVM, so overriding it takes effect immediately and
     * cannot lose data.
     */
    let mut instance = instance;
    if let Some(dir) = store
        .load_instance_overrides(&instance_id)
        .ok()
        .and_then(|overrides| overrides.game_dir)
        .filter(|raw| !raw.trim().is_empty())
    {
        instance.game_dir = std::path::PathBuf::from(dir.trim());
    }

    let metadata = read_profile(&instance).await?;
    let settings = store.load_settings()?;
    validate_settings(&settings, Some(system_memory_info().maximum_game_mb))
        .map_err(RuntimeError::message)?;
    /*
     * Merge instance overrides exactly once, before anything downstream reads a
     * setting. Everything after this point works with the values this instance
     * will really run with.
     */
    let overrides = store.load_instance_overrides(&instance_id)?;
    let settings = resolve_settings(&settings, &overrides);

    let effective_java = java_path
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| settings.java_path.clone());

    let mut secrets: Vec<Zeroizing<String>> = Vec::new();
    let mut prepared = if account.kind == AccountKind::Offline {
        report("preparing", 0.25);
        prepare_offline_launch(
            context.data_root(),
            &instance,
            &account,
            &metadata,
            effective_java.clone(),
        )
        .map_err(|error| RuntimeError::message(error.to_string()))?
    } else {
        report("authenticating", 0.2);
        let session = crate::account::refresh(&account).await?;
        let identity = launcher_core::LaunchIdentity {
            kind: account.kind.clone(),
            profile_id: &session.profile_id,
            player_name: &session.profile_name,
            access_token: &session.access_token,
            client_id: session.client_id.as_deref().unwrap_or(""),
            xuid: session.xuid.as_deref().unwrap_or(""),
        };
        let prepared = prepare_authenticated_launch(
            context.data_root(),
            &instance,
            &identity,
            &metadata,
            effective_java.clone(),
        )
        .map_err(|error| RuntimeError::message(error.to_string()))?;
        secrets.push(session.access_token.clone());
        prepared
    };

    if account.kind == AccountKind::SkinServer {
        let server = reqwest::Url::parse(
            account
                .auth_server
                .as_deref()
                .ok_or_else(|| RuntimeError::message("皮肤站根地址缺失。"))?,
        )
        .map_err(|_| RuntimeError::message("认证地址无效。"))?;
        let agent = crate::authlib_injector::ensure(&server, context.data_root()).await?;
        prepared.args.insert(
            0,
            format!("-javaagent:{}={}", agent.to_string_lossy(), server),
        );
    }

    apply_launch_settings(
        &mut prepared,
        &settings,
        system_memory_info().recommended_mb,
        metadata
            .main_class
            .as_deref()
            .ok_or_else(|| RuntimeError::message("Minecraft 主类缺失。"))?,
    )
    .map_err(|error| RuntimeError::message(error.to_string()))?;
    report("building", 0.75);

    let secrets = Arc::new(secrets);
    if let Some(parent) = prepared.log_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| RuntimeError::Io(error.to_string()))?;
    }
    let log_file = tokio::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&prepared.log_path)
        .await
        .map_err(|error| RuntimeError::Io(error.to_string()))?;

    /*
     * Plugin contributions. The plan is computed after `apply_launch_settings`
     * because that call reorders arguments and inserts `-Xmx`; the insertion
     * point for plugin JVM flags must agree with the final ordering.
     */
    let plugin_state = store.load_plugin_state().unwrap_or_default();
    let plugin_definitions = crate::plugins::catalog(&plugin_state);
    let plan = launcher_core::build_plan(&plugin_definitions, &plugin_state, &instance, &settings);
    if !plan.jvm_args.is_empty() {
        launcher_core::insert_jvm_args(
            &mut prepared.args,
            &plan.jvm_args,
            metadata.main_class.as_deref().unwrap_or_default(),
        );
    }

    /*
     * Pre-launch backups happen before the process touches any file. A failure
     * here does not stop the launch — a broken backup directory should not make
     * the game unplayable — but it is recorded in the launch history.
     */
    let mut backup_failures: Vec<String> = Vec::new();
    for (plugin_id, directory) in &plan.backups {
        let target = instance.clone();
        let dest = directory.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            launcher_core::backups::create_backup(&target, &dest)
        })
        .await;
        match outcome {
            Ok(Ok(path)) => sink.plugin_backup(&instance_id, plugin_id, &path),
            Ok(Err(error)) => backup_failures.push(format!("{plugin_id}: {error}")),
            Err(_) => backup_failures.push(format!("{plugin_id}: 备份任务未能完成")),
        }
    }

    // Pre-launch commands (global setting, then plugin contributions). Either
    // failing aborts: a user's setup step failing means the environment is not
    // ready, and continuing produces a more confusing error later.
    if !settings.pre_launch_command.trim().is_empty() {
        run_pre_launch_command(&settings.pre_launch_command, &prepared.working_directory).await?;
    }
    for (plugin_id, command) in &plan.pre_launch {
        run_pre_launch_command(command, &prepared.working_directory)
            .await
            .map_err(|error| {
                RuntimeError::message(format!("插件 {plugin_id} 的启动前命令失败：{error}"))
            })?;
    }

    let shared_log = Arc::new(tokio::sync::Mutex::new(log_file));
    let stop_rx = registry.register(&instance_id)?;

    report("starting", 0.9);
    let mut command = Command::new(&prepared.java_path);
    command
        .args(&prepared.args)
        .current_dir(&prepared.working_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    /*
     * The game draws its own window, but the JVM console must not appear beside it.
     * From a `windows`-subsystem parent that console would be a new window; from a
     * terminal it would be the terminal itself, so the flag is a no-op there only
     * because Windows ignores it when a console is inherited.
     */
    launcher_core::process::hide_console_async(&mut command);
    /*
     * Plugin-contributed environment variables. Applied after the base command is
     * built so they layer over the parent environment rather than replacing it.
     */
    for (key, value) in &plan.env {
        command.env(key, value);
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            registry.clear(&instance_id);
            return Err(RuntimeError::message(format!("无法启动 Java：{error}")));
        }
    };

    /*
     * Record the launch only now that the JVM exists. A launch that failed to
     * spawn is not "played", and should not take over the "last used instance"
     * slot.
     */
    let _ = store.mark_instance_launched(&instance_id);

    let history_id = uuid::Uuid::new_v4().to_string();
    let history_enabled = crate::plugins::is_enabled(&plugin_state, "launch-logger");
    let mut history_report = plan.report.clone();
    for failure in &backup_failures {
        history_report.skipped.push(SkippedContribution {
            plugin_id: failure.split(':').next().unwrap_or("plugin").to_owned(),
            kind: launcher_core::ContributionKind::BackupBefore,
            reason: "备份失败".into(),
        });
    }
    if history_enabled {
        let _ = store.record_launch_start(&launcher_core::LaunchHistoryEntry {
            id: history_id.clone(),
            instance_id: instance_id.clone(),
            instance_name: instance.name.clone(),
            started_at: chrono::Utc::now().to_rfc3339(),
            exit_code: None,
            outcome: "running".into(),
            contributions: history_report,
        });
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| RuntimeError::message("无法读取游戏输出。"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| RuntimeError::message("无法读取游戏错误输出。"))?;
    let stdout_task = tokio::spawn(consume_game_log(
        stdout,
        sink.clone(),
        instance_id.clone(),
        shared_log.clone(),
        secrets.clone(),
    ));
    let stderr_task = tokio::spawn(consume_game_log(
        stderr,
        sink.clone(),
        instance_id.clone(),
        shared_log,
        secrets,
    ));

    let log_path = prepared.log_path.clone();
    report("running", 1.0);
    sink.game_process(&GameProcessEvent {
        instance_id: instance_id.clone(),
        status: "running".into(),
        exit_code: None,
        log_path: log_path.to_string_lossy().into_owned(),
    });

    let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
    let watcher_registry = registry.clone();
    let watcher_store = store.clone();
    let watcher_sink = sink.clone();
    let watcher_instance = instance_id.clone();
    let history_id_for_exit = if history_enabled {
        history_id
    } else {
        // An empty id matches no row, making `finish_launch` a no-op — exactly
        // what "logging is off" should mean.
        String::new()
    };
    let log_path_for_event = log_path.clone();

    tokio::spawn(async move {
        let mut stopped_by_user = false;
        let status = tokio::select! {
            status = child.wait() => status.ok(),
            _ = stop_rx => {
                stopped_by_user = true;
                let _ = child.kill().await;
                child.wait().await.ok()
            }
        };
        let _ = stdout_task.await;
        let _ = stderr_task.await;
        let exit_code = status.and_then(|status| status.code());
        watcher_registry.release(&watcher_instance);

        /*
         * One verdict for both the history row and the event. Two independent
         * tests would eventually disagree about whether a run "crashed".
         */
        let outcome = if stopped_by_user || exit_code == Some(0) {
            "stopped"
        } else {
            "crashed"
        };
        let _ = watcher_store.finish_launch(&history_id_for_exit, outcome, exit_code);
        let event = GameProcessEvent {
            instance_id: watcher_instance,
            status: outcome.into(),
            exit_code,
            log_path: log_path_for_event.to_string_lossy().into_owned(),
        };
        watcher_sink.game_process(&event);
        let _ = exit_tx.send(event);
    });

    Ok(RunningGame {
        log_path,
        exit: exit_rx,
    })
}

async fn read_profile(instance: &InstanceRecord) -> Result<GameVersionMetadata> {
    let version_dir = instance.version_dir();
    let profile = if instance.loader == "vanilla" {
        version_dir.join(format!("{}.json", instance.game_version))
    } else {
        version_dir.join("launch.json")
    };
    let bytes = tokio::fs::read(&profile)
        .await
        .map_err(|error| RuntimeError::message(format!("无法读取版本配置：{error}")))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| RuntimeError::message(format!("版本配置已损坏：{error}")))
}

async fn run_pre_launch_command(command: &str, working_directory: &std::path::Path) -> Result<()> {
    let parts = shlex::split(command)
        .ok_or_else(|| RuntimeError::message("启动前命令格式无效，请检查引号是否成对。"))?;
    let (program, args) = parts
        .split_first()
        .ok_or_else(|| RuntimeError::message("启动前命令不能为空。"))?;
    if program.contains("..") {
        return Err(RuntimeError::message("启动前命令不能使用包含 .. 的路径。"));
    }
    let mut pre_launch = Command::new(program);
    pre_launch
        .args(args)
        .current_dir(working_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    /*
     * A user's pre-launch command is typically a shell script or batch file, which
     * is a console program. Without this it flashed a window on every launch from
     * the desktop app.
     */
    launcher_core::process::hide_console_async(&mut pre_launch);
    let status = pre_launch
        .status()
        .await
        .map_err(|error| RuntimeError::message(format!("无法执行启动前命令：{error}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(RuntimeError::message(format!(
            "启动前命令退出码异常：{}",
            status
                .code()
                .map_or_else(|| "unknown".to_owned(), |code| code.to_string())
        )))
    }
}

/// Drain one pipe, redacting known secrets before writing or emitting.
///
/// Both the file and the event get the **redacted** line. Redacting only one of
/// them would leave the token in whichever path the user is less likely to
/// inspect.
async fn consume_game_log<R>(
    reader: R,
    sink: Arc<dyn EventSink>,
    instance_id: String,
    log_file: Arc<tokio::sync::Mutex<tokio::fs::File>>,
    secrets: Arc<Vec<Zeroizing<String>>>,
) where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let mut reader = BufReader::new(reader);
    while let Ok(Some(line)) = launcher_core::logs::bounded_log_line(&mut reader).await {
        let known: Vec<&str> = secrets.iter().map(|secret| secret.as_str()).collect();
        let line = launcher_core::logs::redact_log(&line, &known);
        {
            let mut file = log_file.lock().await;
            // Keep draining both pipes after disk errors or the per-launch cap.
            if file
                .metadata()
                .await
                .is_ok_and(|meta| meta.len() < 32 * 1024 * 1024)
            {
                let _ = file.write_all(line.as_bytes()).await;
                let _ = file.write_all(b"\n").await;
            }
        }
        sink.game_log(&GameLogLine {
            instance_id: instance_id.clone(),
            line,
        });
    }
}

/// Convenience: launch and wait, for callers that have nothing else to do.
pub async fn launch_and_wait(
    context: &RuntimeContext,
    store: &LauncherStore,
    registry: &ProcessRegistry,
    sink: Arc<dyn EventSink>,
    request: LaunchRequest,
) -> Result<GameProcessEvent> {
    let running = launch(context, store, registry, sink, request).await?;
    Ok(running.wait().await)
}

/// Resolve which account to use when the caller did not name one.
///
/// "The selected account" is what the GUI shows as current; the CLI uses the
/// same rule so `mfmcl launch --instance x` without `--account` behaves the way
/// the user expects.
pub fn selected_account(store: &LauncherStore) -> Result<Option<AccountProfile>> {
    Ok(store
        .list_accounts()?
        .into_iter()
        .find(|account| account.selected))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_registered_instance_cannot_be_started_twice() {
        let registry = ProcessRegistry::default();
        let _first = registry.register("a").unwrap();
        let error = registry.register("a").unwrap_err();
        assert!(error.to_string().contains("已经在运行"), "got {error}");
        assert!(registry.is_running("a"));
    }

    #[test]
    fn clearing_a_failed_spawn_allows_a_retry() {
        // The bug this prevents: a failed spawn leaves the key in the map, and
        // every later attempt is refused as "already running".
        let registry = ProcessRegistry::default();
        let _rx = registry.register("a").unwrap();
        registry.clear("a");
        assert!(!registry.is_running("a"));
        assert!(registry.register("a").is_ok());
    }

    #[test]
    fn stopping_an_instance_that_is_not_running_is_an_error() {
        let registry = ProcessRegistry::default();
        assert!(registry.stop("a").is_err());
    }

    #[test]
    fn stopping_consumes_the_registration() {
        let registry = ProcessRegistry::default();
        let _rx = registry.register("a").unwrap();
        registry.stop("a").unwrap();
        assert!(!registry.is_running("a"), "stop must release the slot");
    }
}
