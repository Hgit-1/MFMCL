use chrono::Utc;
use download_engine::{
    DownloadEngine, DownloadProgress, DownloadSettings as EngineDownloadSettings,
    aria2::{Aria2Options, Aria2Session, find_aria2_binary},
};
use launcher_core::{
    AccountKind, AccountProfile, CreateInstanceInput, DiagnosticSummary, InstanceState,
    JavaRuntimeInfo, LauncherSettings, LauncherStore, detect_java_runtime,
    export_instance_diagnostics, install_vanilla, prepare_offline_launch, summarize_instance,
};
use mc_metadata::{GameVersionMetadata, SourcePreference, fetch_manifest};
use serde::Serialize;
use std::collections::HashSet;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager, State};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct VersionListItem {
    id: String,
    #[serde(rename = "type")]
    version_type: String,
    release_time: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SystemInfo {
    operating_system: String,
    architecture: String,
    app_data_dir: String,
}

#[derive(Default)]
struct ProcessRegistry(Arc<Mutex<HashSet<String>>>);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GameLogLine {
    instance_id: String,
    line: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GameProcessEvent {
    instance_id: String,
    status: String,
    exit_code: Option<i32>,
    log_path: String,
}

#[tauri::command]
async fn list_versions() -> Result<Vec<VersionListItem>, String> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("MFMCL/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| error.to_string())?;
    let manifest = fetch_manifest(&client, SourcePreference::Official)
        .await
        .map_err(|error| format!("读取 Minecraft 官方版本清单失败：{error}"))?;
    Ok(manifest
        .versions
        .into_iter()
        .map(|version| VersionListItem {
            id: version.id,
            version_type: version.version_type,
            release_time: version.release_time,
        })
        .collect())
}

#[tauri::command]
fn get_settings(store: State<'_, LauncherStore>) -> Result<LauncherSettings, String> {
    store.load_settings().map_err(|error| error.to_string())
}

#[tauri::command]
fn save_settings(
    settings: LauncherSettings,
    store: State<'_, LauncherStore>,
) -> Result<(), String> {
    if !(1..=16).contains(&settings.download.max_concurrent_downloads)
        || !(1..=16).contains(&settings.download.connections_per_file)
    {
        return Err("下载并发数必须在 1 到 16 之间。".to_owned());
    }
    store
        .save_settings(&settings)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn detect_java() -> Result<JavaRuntimeInfo, String> {
    detect_java_runtime(None).map_err(|error| error.to_string())
}

#[tauri::command]
fn get_system_info(app: tauri::AppHandle) -> Result<SystemInfo, String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    Ok(SystemInfo {
        operating_system: std::env::consts::OS.to_owned(),
        architecture: std::env::consts::ARCH.to_owned(),
        app_data_dir: app_data_dir.to_string_lossy().into_owned(),
    })
}

#[tauri::command]
fn list_instances(
    store: State<'_, LauncherStore>,
) -> Result<Vec<launcher_core::InstanceRecord>, String> {
    store.list_instances().map_err(|error| error.to_string())
}

#[tauri::command]
fn create_instance(
    name: String,
    version: String,
    app: tauri::AppHandle,
    store: State<'_, LauncherStore>,
) -> Result<launcher_core::InstanceRecord, String> {
    let data_root = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let input = CreateInstanceInput {
        name,
        game_version: version,
    };
    store
        .create_instance(&input, &data_root)
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn install_instance(
    instance_id: String,
    app: tauri::AppHandle,
    store: State<'_, LauncherStore>,
) -> Result<(), String> {
    let instance = store
        .list_instances()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|instance| instance.id == instance_id)
        .ok_or_else(|| "找不到所选实例。".to_owned())?;
    if instance.loader != "vanilla" {
        return Err("当前安装引擎只支持原版实例；模组加载器适配器正在开发。".to_owned());
    }
    let settings = store.load_settings().map_err(|error| error.to_string())?;
    let data_root = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let engine_settings = EngineDownloadSettings {
        source: settings.download.source,
        max_concurrent_downloads: settings.download.max_concurrent_downloads,
        connections_per_file: settings.download.connections_per_file,
        aria2_enabled: settings.download.aria2_enabled,
    };
    let mut engine = DownloadEngine::new(engine_settings).map_err(|error| error.to_string())?;
    let mut aria2_session = None;
    if settings.download.aria2_enabled {
        if let Ok(resource_dir) = app.path().resource_dir() {
            let binary = find_aria2_binary(&resource_dir);
            if binary.is_file() {
                match Aria2Session::start(
                    &binary,
                    &data_root.join("aria2"),
                    Aria2Options {
                        max_concurrent_downloads: settings.download.max_concurrent_downloads,
                        connections_per_file: settings.download.connections_per_file,
                    },
                )
                .await
                {
                    Ok(session) => {
                        engine = engine.with_aria2(session.clone());
                        aria2_session = Some(session);
                    }
                    Err(error) => {
                        eprintln!("aria2 unavailable; using verified HTTP fallback: {error}")
                    }
                }
            }
        }
    }

    let client = reqwest::Client::builder()
        .user_agent(concat!("MFMCL/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| error.to_string())?;
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel::<DownloadProgress>();
    let event_app = app.clone();
    let progress_task = tauri::async_runtime::spawn(async move {
        while let Some(progress) = progress_rx.recv().await {
            let _ = event_app.emit("download-progress", progress);
        }
    });
    let result = install_vanilla(
        &client,
        engine,
        store.inner(),
        &data_root,
        &instance,
        Some(progress_tx),
    )
    .await;
    let _ = progress_task.await;
    if let Some(session) = aria2_session {
        let _ = session.shutdown().await;
    }
    result.map(|_| ()).map_err(|error| error.to_string())
}

#[tauri::command]
fn add_offline_account(
    player_name: String,
    store: State<'_, LauncherStore>,
) -> Result<AccountProfile, String> {
    let name = player_name.trim();
    if !(3..=16).contains(&name.len())
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return Err("离线档案名称需为 3–16 位英文字母、数字或下划线。".to_owned());
    }
    let account = AccountProfile {
        id: Uuid::new_v4().to_string(),
        display_name: name.to_owned(),
        kind: AccountKind::Offline,
        auth_server: None,
        credential_ref: None,
        selected: true,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    store
        .add_account(&account)
        .map_err(|error| error.to_string())?;
    Ok(account)
}

#[tauri::command]
fn list_accounts(store: State<'_, LauncherStore>) -> Result<Vec<AccountProfile>, String> {
    store.list_accounts().map_err(|error| error.to_string())
}

#[tauri::command]
fn select_account(account_id: String, store: State<'_, LauncherStore>) -> Result<(), String> {
    store
        .select_account(&account_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn summarize_diagnostics(
    instance_id: String,
    store: State<'_, LauncherStore>,
) -> Result<DiagnosticSummary, String> {
    let instance = store
        .list_instances()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|instance| instance.id == instance_id)
        .ok_or_else(|| "找不到所选实例。".to_owned())?;
    summarize_instance(&instance).map_err(|error| error.to_string())
}

#[tauri::command]
fn export_diagnostics(
    instance_id: String,
    destination_dir: String,
    store: State<'_, LauncherStore>,
) -> Result<String, String> {
    let instance = store
        .list_instances()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|instance| instance.id == instance_id)
        .ok_or_else(|| "找不到所选实例。".to_owned())?;
    export_instance_diagnostics(&instance, std::path::Path::new(&destination_dir))
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn launch_instance(
    instance_id: String,
    account_id: String,
    offline_confirmed: bool,
    app: tauri::AppHandle,
    store: State<'_, LauncherStore>,
    processes: State<'_, ProcessRegistry>,
) -> Result<(), String> {
    let instance = store
        .list_instances()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|instance| instance.id == instance_id)
        .ok_or_else(|| "找不到所选实例。".to_owned())?;
    if instance.state != InstanceState::Ready {
        return Err("此实例尚未安装完成。".to_owned());
    }
    let account = store
        .list_accounts()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|account| account.id == account_id)
        .ok_or_else(|| "找不到所选账户。".to_owned())?;
    if account.kind == AccountKind::Offline && !offline_confirmed {
        return Err("离线账户启动必须先由用户确认。".to_owned());
    }
    if account.kind != AccountKind::Offline {
        return Err("此版本暂时只支持离线档案启动；Microsoft 与皮肤站认证正在接入。".to_owned());
    }

    let data_root = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let metadata_path = instance
        .instance_dir
        .join("versions")
        .join(&instance.game_version)
        .join(format!("{}.json", instance.game_version));
    let metadata_bytes = tokio::fs::read(&metadata_path)
        .await
        .map_err(|error| format!("无法读取版本配置：{error}"))?;
    let metadata: GameVersionMetadata = serde_json::from_slice(&metadata_bytes)
        .map_err(|error| format!("版本配置已损坏：{error}"))?;
    let prepared = prepare_offline_launch(&data_root, &instance, &account, &metadata, None)
        .map_err(|error| error.to_string())?;

    let registry = processes.0.clone();
    if let Some(parent) = prepared.log_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| error.to_string())?;
    }
    let log_file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&prepared.log_path)
        .await
        .map_err(|error| error.to_string())?;
    let shared_log = Arc::new(tokio::sync::Mutex::new(log_file));
    {
        let mut running = registry
            .lock()
            .map_err(|_| "启动器进程状态不可用。".to_owned())?;
        if !running.insert(instance_id.clone()) {
            return Err("此实例已经在运行。".to_owned());
        }
    }

    let mut child = match Command::new(&prepared.java_path)
        .args(&prepared.args)
        .current_dir(&prepared.working_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            if let Ok(mut running) = registry.lock() {
                running.remove(&instance_id);
            }
            return Err(format!("无法启动 Java：{error}"));
        }
    };

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "无法读取游戏输出。".to_owned())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "无法读取游戏错误输出。".to_owned())?;
    let stdout_task = tokio::spawn(consume_game_log(
        stdout,
        app.clone(),
        instance_id.clone(),
        shared_log.clone(),
    ));
    let stderr_task = tokio::spawn(consume_game_log(
        stderr,
        app.clone(),
        instance_id.clone(),
        shared_log,
    ));
    let _ = app.emit(
        "game-process",
        GameProcessEvent {
            instance_id: instance_id.clone(),
            status: "running".to_owned(),
            exit_code: None,
            log_path: prepared.log_path.to_string_lossy().into_owned(),
        },
    );

    let monitor_app = app.clone();
    let monitor_registry = registry.clone();
    let monitor_id = instance_id.clone();
    let monitor_log = prepared.log_path.to_string_lossy().into_owned();
    tauri::async_runtime::spawn(async move {
        let status = child.wait().await.ok();
        let _ = stdout_task.await;
        let _ = stderr_task.await;
        let exit_code = status.and_then(|status| status.code());
        if let Ok(mut running) = monitor_registry.lock() {
            running.remove(&monitor_id);
        }
        let _ = monitor_app.emit(
            "game-process",
            GameProcessEvent {
                instance_id: monitor_id,
                status: if exit_code == Some(0) {
                    "stopped"
                } else {
                    "crashed"
                }
                .to_owned(),
                exit_code,
                log_path: monitor_log,
            },
        );
    });
    Ok(())
}

async fn consume_game_log<R>(
    reader: R,
    app: tauri::AppHandle,
    instance_id: String,
    log_file: Arc<tokio::sync::Mutex<tokio::fs::File>>,
) where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let mut lines = BufReader::new(reader).lines();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            _ => break,
        };
        let line = redact_sensitive_log(&line);
        {
            let mut file = log_file.lock().await;
            if file.write_all(line.as_bytes()).await.is_err()
                || file.write_all(b"\n").await.is_err()
            {
                break;
            }
        }
        let _ = app.emit(
            "game-log-line",
            GameLogLine {
                instance_id: instance_id.clone(),
                line,
            },
        );
    }
}

fn redact_sensitive_log(line: &str) -> String {
    static SECRET_PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let pattern = SECRET_PATTERN.get_or_init(|| {
        regex::Regex::new(r#"(?i)[\"']?(access[_-]?token|client[_-]?token|authorization)[\"']?\s*[:=]\s*(?:\"[^\"]*\"|'[^']*'|Bearer\s+[^\s,;]+|[^\s,;]+)"#)
            .expect("valid credential redaction expression")
    });
    let safe = pattern.replace_all(line, "$1=[REDACTED]");
    safe.chars().take(8_000).collect()
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&app_data_dir)?;
            let store = LauncherStore::open(app_data_dir.join("mfmcl.sqlite"))?;
            app.manage(store);
            app.manage(ProcessRegistry::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_versions,
            get_settings,
            save_settings,
            detect_java,
            get_system_info,
            list_instances,
            create_instance,
            install_instance,
            add_offline_account,
            list_accounts,
            select_account,
            summarize_diagnostics,
            export_diagnostics,
            launch_instance,
        ])
        .run(tauri::generate_context!())
        .expect("MFMCL desktop application failed");
}
