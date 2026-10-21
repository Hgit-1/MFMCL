mod authentication;
mod backup_management;
mod downloads;
mod management;
mod sidecar;
mod updates;

use chrono::Utc;
use download_engine::{
    DownloadEngine, DownloadProgress, DownloadSettings as EngineDownloadSettings,
    aria2::{Aria2Options, Aria2Session},
};
use launcher_core::{
    AccountKind, AccountProfile, CreateInstanceInput, DiagnosticSummary, InstanceState,
    JavaRuntimeInfo, LauncherSettings, LauncherStore, detect_java_runtime,
    export_instance_diagnostics, install_vanilla, prepare_authenticated_launch,
    prepare_offline_launch, summarize_instance,
};
use mc_metadata::{GameVersionMetadata, SourcePreference, fetch_manifest};
use serde::Serialize;
use std::{
    collections::HashMap,
    process::Stdio,
    sync::{Arc, Mutex},
};
use tauri::{Emitter, Manager, State};
use tokio::{
    io::{AsyncWriteExt, BufReader},
    process::Command,
};
use uuid::Uuid;
use zeroize::Zeroizing;

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
    app_version: String,
}
#[derive(Default)]
struct ProcessRegistry(Arc<Mutex<HashMap<String, tokio::sync::oneshot::Sender<()>>>>);
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
        .map_err(|e| e.to_string())?;
    let manifest = fetch_manifest(&client, SourcePreference::Official)
        .await
        .map_err(|e| format!("读取 Minecraft 官方版本清单失败：{e}"))?;
    Ok(manifest
        .versions
        .into_iter()
        .map(|v| VersionListItem {
            id: v.id,
            version_type: v.version_type,
            release_time: v.release_time,
        })
        .collect())
}
#[tauri::command]
fn get_settings(store: State<'_, LauncherStore>) -> Result<LauncherSettings, String> {
    store.load_settings().map_err(|e| e.to_string())
}
#[tauri::command]
fn save_settings(
    settings: LauncherSettings,
    store: State<'_, LauncherStore>,
) -> Result<(), String> {
    if !(1..=16).contains(&settings.download.max_concurrent_downloads)
        || !(1..=16).contains(&settings.download.connections_per_file)
    {
        return Err("下载并发数必须在 1 到 16 之间。".into());
    }
    if !(512..=32768).contains(&settings.memory_mb)
        || settings
            .java_path
            .as_ref()
            .is_some_and(|p| p.as_os_str().len() > 4096)
    {
        return Err("游戏内存必须在 512–32768 MB 范围，Java 路径不能过长。".into());
    }
    if settings.custom_accent.len() != 7
        || !settings.custom_accent.starts_with('#')
        || !settings.custom_accent[1..]
            .chars()
            .all(|c| c.is_ascii_hexdigit())
    {
        return Err("强调色必须为 #RRGGBB。".into());
    }
    if settings.custom_background.as_ref().is_some_and(|image| {
        image.len() > 3 * 1024 * 1024
            || !(image.starts_with("data:image/png;base64,")
                || image.starts_with("data:image/jpeg;base64,")
                || image.starts_with("data:image/webp;base64,"))
    }) {
        return Err("背景只能是有限大小的本地 PNG、JPEG 或 WebP 图片。".into());
    }
    store.save_settings(&settings).map_err(|e| e.to_string())
}
#[tauri::command]
fn detect_java() -> Result<JavaRuntimeInfo, String> {
    detect_java_runtime(None).map_err(|e| e.to_string())
}
#[tauri::command]
fn get_system_info(app: tauri::AppHandle) -> Result<SystemInfo, String> {
    let root = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(SystemInfo {
        operating_system: std::env::consts::OS.into(),
        architecture: std::env::consts::ARCH.into(),
        app_data_dir: root.to_string_lossy().into_owned(),
        app_version: env!("CARGO_PKG_VERSION").into(),
    })
}
#[tauri::command]
fn list_instances(
    store: State<'_, LauncherStore>,
) -> Result<Vec<launcher_core::InstanceRecord>, String> {
    store.list_instances().map_err(|e| e.to_string())
}
#[tauri::command]
fn create_instance(
    name: String,
    version: String,
    app: tauri::AppHandle,
    store: State<'_, LauncherStore>,
) -> Result<launcher_core::InstanceRecord, String> {
    let root = app.path().app_data_dir().map_err(|e| e.to_string())?;
    store
        .create_instance(
            &CreateInstanceInput {
                name,
                game_version: version,
            },
            &root,
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn install_instance(
    instance_id: String,
    app: tauri::AppHandle,
    store: State<'_, LauncherStore>,
    operations: State<'_, management::Operations>,
    downloads: State<'_, Arc<downloads::Downloads>>,
) -> Result<(), String> {
    let _operation = operations.acquire(&instance_id)?;
    let _shared = operations.acquire("shared-game-files")?;
    let instance = store
        .list_instances()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|i| i.id == instance_id)
        .ok_or("找不到所选实例。")?;
    let settings = store.load_settings().map_err(|e| e.to_string())?;
    let root = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let mut engine = DownloadEngine::new(EngineDownloadSettings {
        source: settings.download.source,
        max_concurrent_downloads: settings.download.max_concurrent_downloads,
        connections_per_file: settings.download.connections_per_file,
        aria2_enabled: settings.download.aria2_enabled,
    })
    .map_err(|e| e.to_string())?;
    let mut aria2_session = None;
    if settings.download.aria2_enabled {
        if let Ok(resource_dir) = app.path().resource_dir() {
            if let Ok(binary) = sidecar::verified_aria2(&resource_dir).await {
                match Aria2Session::start(
                    &binary,
                    &root.join("aria2"),
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
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;
    let control = downloads.start(
        &instance_id,
        &format!("安装 {}", instance.name),
        &store,
        &app,
    )?;
    engine = engine.with_control(control);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<DownloadProgress>();
    let event_app = app.clone();
    let event_id = instance_id.clone();
    let event_store = store.inner().clone();
    let event_downloads = downloads.inner().clone();
    let progress_task = tauri::async_runtime::spawn(async move {
        while let Some(progress) = rx.recv().await {
            event_downloads.progress(&event_id, &progress, &event_store, &event_app);
            let _ = event_app.emit("download-progress", progress);
        }
    });
    let result = install_vanilla(
        &client,
        engine.clone(),
        store.inner(),
        &root,
        &instance,
        Some(tx),
    )
    .await;
    let result = match result {
        Ok(_) => management::install_pending_pack_loader(&root, &instance, &store, &engine).await,
        Err(e) => Err(e.to_string()),
    };
    if result.is_err() {
        let _ = store.set_instance_state(&instance_id, InstanceState::Failed);
    }
    let _ = progress_task.await;
    if let Some(session) = aria2_session {
        let _ = session.shutdown().await;
    }
    downloads.finish(&instance_id, result.is_ok(), &store, &app)?;
    result
}

#[tauri::command]
fn add_offline_account(
    player_name: String,
    store: State<'_, LauncherStore>,
) -> Result<AccountProfile, String> {
    let name = player_name.trim();
    if !(3..=16).contains(&name.len())
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err("离线档案名称需为 3–16 位英文字母、数字或下划线。".into());
    }
    let account = AccountProfile {
        id: Uuid::new_v4().to_string(),
        display_name: name.into(),
        kind: AccountKind::Offline,
        auth_server: None,
        credential_ref: None,
        selected: true,
        created_at: Utc::now().to_rfc3339(),
    };
    store.add_account(&account).map_err(|e| e.to_string())?;
    Ok(account)
}
#[tauri::command]
fn list_accounts(store: State<'_, LauncherStore>) -> Result<Vec<AccountProfile>, String> {
    store.list_accounts().map_err(|e| e.to_string())
}
#[tauri::command]
fn select_account(account_id: String, store: State<'_, LauncherStore>) -> Result<(), String> {
    store.select_account(&account_id).map_err(|e| e.to_string())
}
#[tauri::command]
async fn remove_account(account_id: String, store: State<'_, LauncherStore>) -> Result<(), String> {
    let account = store
        .list_accounts()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|account| account.id == account_id)
        .ok_or("找不到所选账户。")?;
    // Remove the credential-store entry first; a dangling DB row is preferable to a
    // remaining credential whose profile reference was deleted.
    if let Some(key) = account.credential_ref.clone() {
        let _ =
            tauri::async_runtime::spawn_blocking(move || authentication::delete_credential(&key))
                .await;
    }
    store.remove_account(&account_id).map_err(|e| e.to_string())
}
#[tauri::command]
fn summarize_diagnostics(
    instance_id: String,
    store: State<'_, LauncherStore>,
) -> Result<DiagnosticSummary, String> {
    let instance = store
        .list_instances()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|i| i.id == instance_id)
        .ok_or("找不到所选实例。")?;
    summarize_instance(&instance).map_err(|e| e.to_string())
}
#[tauri::command]
fn export_diagnostics(
    instance_id: String,
    destination_dir: String,
    store: State<'_, LauncherStore>,
) -> Result<String, String> {
    let instance = store
        .list_instances()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|i| i.id == instance_id)
        .ok_or("找不到所选实例。")?;
    export_instance_diagnostics(&instance, std::path::Path::new(&destination_dir))
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|e| e.to_string())
}
#[tauri::command]
fn stop_game(instance_id: String, processes: State<'_, ProcessRegistry>) -> Result<(), String> {
    let sender = processes
        .0
        .lock()
        .map_err(|_| "启动器进程状态不可用。")?
        .remove(&instance_id)
        .ok_or("此实例当前没有可停止的游戏进程。")?;
    sender.send(()).map_err(|_| "游戏进程已经退出。".to_owned())
}

#[tauri::command]
async fn launch_instance(
    instance_id: String,
    account_id: String,
    offline_confirmed: bool,
    app: tauri::AppHandle,
    store: State<'_, LauncherStore>,
    processes: State<'_, ProcessRegistry>,
    operations: State<'_, management::Operations>,
) -> Result<(), String> {
    let operation = operations.acquire(&instance_id)?;
    let instance = store
        .list_instances()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|i| i.id == instance_id)
        .ok_or("找不到所选实例。")?;
    if instance.state != InstanceState::Ready {
        return Err("此实例尚未安装完成。".into());
    }
    let account = store
        .list_accounts()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|a| a.id == account_id)
        .ok_or("找不到所选账户。")?;
    if account.kind == AccountKind::Offline && !offline_confirmed {
        return Err("离线账户启动必须先由用户确认。".into());
    }
    let root = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let version_dir = instance
        .instance_dir
        .join("versions")
        .join(&instance.game_version);
    let profile = if instance.loader == "vanilla" {
        version_dir.join(format!("{}.json", instance.game_version))
    } else {
        version_dir.join("launch.json")
    };
    let bytes = tokio::fs::read(&profile)
        .await
        .map_err(|e| format!("无法读取版本配置：{e}"))?;
    let metadata: GameVersionMetadata =
        serde_json::from_slice(&bytes).map_err(|e| format!("版本配置已损坏：{e}"))?;
    let settings = store.load_settings().map_err(|e| e.to_string())?;
    let mut secrets = Vec::new();
    let mut prepared = if account.kind == AccountKind::Offline {
        prepare_offline_launch(&root, &instance, &account, &metadata, settings.java_path)
            .map_err(|e| e.to_string())?
    } else {
        let session = authentication::refresh(&account).await?;
        let identity = launcher_core::LaunchIdentity {
            kind: account.kind.clone(),
            profile_id: &session.profile_id,
            player_name: &session.profile_name,
            access_token: &session.access_token,
            client_id: session.client_id.as_deref().unwrap_or(""),
            xuid: session.xuid.as_deref().unwrap_or(""),
        };
        let prepared = prepare_authenticated_launch(
            &root,
            &instance,
            &identity,
            &metadata,
            settings.java_path,
        )
        .map_err(|e| e.to_string())?;
        secrets.push(session.access_token.clone());
        prepared
    };
    if account.kind == AccountKind::SkinServer {
        let server =
            reqwest::Url::parse(account.auth_server.as_deref().ok_or("皮肤站根地址缺失。")?)
                .map_err(|_| "认证地址无效。")?;
        let agent = authentication::injector(&server, &root).await?;
        prepared.args.insert(
            0,
            format!("-javaagent:{}={}", agent.to_string_lossy(), server),
        );
    }
    prepared
        .args
        .insert(0, format!("-Xmx{}M", settings.memory_mb.clamp(512, 32768)));
    let secrets = Arc::new(secrets);
    let registry = processes.0.clone();
    if let Some(parent) = prepared.log_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| e.to_string())?;
    }
    let log_file = tokio::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&prepared.log_path)
        .await
        .map_err(|e| e.to_string())?;
    let shared_log = Arc::new(tokio::sync::Mutex::new(log_file));
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    {
        let mut running = registry.lock().map_err(|_| "启动器进程状态不可用。")?;
        if running.contains_key(&instance_id) {
            return Err("此实例已经在运行。".into());
        }
        running.insert(instance_id.clone(), stop_tx);
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
    let stdout = child.stdout.take().ok_or("无法读取游戏输出。")?;
    let stderr = child.stderr.take().ok_or("无法读取游戏错误输出。")?;
    let stdout_task = tokio::spawn(consume_game_log(
        stdout,
        app.clone(),
        instance_id.clone(),
        shared_log.clone(),
        secrets.clone(),
    ));
    let stderr_task = tokio::spawn(consume_game_log(
        stderr,
        app.clone(),
        instance_id.clone(),
        shared_log,
        secrets,
    ));
    let log_path = prepared.log_path.to_string_lossy().into_owned();
    let _ = app.emit(
        "game-process",
        GameProcessEvent {
            instance_id: instance_id.clone(),
            status: "running".into(),
            exit_code: None,
            log_path: log_path.clone(),
        },
    );
    tauri::async_runtime::spawn(async move {
        let _operation = operation;
        let mut stopped_by_user = false;
        let status = tokio::select! {
            status = child.wait() => status.ok(),
            _ = stop_rx => { stopped_by_user = true; let _ = child.kill().await; child.wait().await.ok() }
        };
        let _ = stdout_task.await;
        let _ = stderr_task.await;
        let exit_code = status.and_then(|s| s.code());
        if let Ok(mut running) = registry.lock() {
            running.remove(&instance_id);
        }
        let _ = app.emit(
            "game-process",
            GameProcessEvent {
                instance_id,
                status: if stopped_by_user || exit_code == Some(0) {
                    "stopped"
                } else {
                    "crashed"
                }
                .into(),
                exit_code,
                log_path,
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
    secrets: Arc<Vec<Zeroizing<String>>>,
) where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let mut reader = BufReader::new(reader);
    while let Ok(Some(line)) = launcher_core::logs::bounded_log_line(&mut reader).await {
        let known_secrets: Vec<&str> = secrets.iter().map(|s| s.as_str()).collect();
        let line = launcher_core::logs::redact_log(&line, &known_secrets);
        {
            let mut file = log_file.lock().await;
            // Continue draining both pipes after disk errors or the 32 MiB per-launch log cap.
            if file
                .metadata()
                .await
                .is_ok_and(|m| m.len() < 32 * 1024 * 1024)
            {
                let _ = file.write_all(line.as_bytes()).await;
                let _ = file.write_all(b"\n").await;
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

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let root = app.path().app_data_dir()?;
            std::fs::create_dir_all(&root)?;
            app.manage(LauncherStore::open(root.join("mfmcl.sqlite"))?);
            app.manage(ProcessRegistry::default());
            app.manage(management::Operations::default());
            app.manage(Arc::new(downloads::Downloads::default()));
            app.manage(authentication::AuthState::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            authentication::skin_login,
            authentication::skin_select_profile,
            authentication::microsoft_login,
            updates::check_launcher_update,
            updates::download_launcher_update,
            downloads::control_download,
            downloads::list_download_tasks,
            backup_management::create_backup,
            backup_management::preview_backup,
            backup_management::restore_backup,
            management::loader_versions,
            management::install_loader,
            management::list_mods,
            management::set_mod_enabled,
            management::check_mod_updates,
            management::update_mod,
            management::check_client_update,
            management::search_mods,
            management::list_mod_versions,
            management::install_mod_version,
            management::delete_instance,
            management::preview_modpack,
            management::import_modpack,
            management::export_modpack,
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
            remove_account,
            summarize_diagnostics,
            export_diagnostics,
            launch_instance,
            stop_game,
        ])
        .run(tauri::generate_context!())
        .expect("MFMCL desktop application failed");
}
