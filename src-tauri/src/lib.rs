mod authentication;
mod backup_management;
mod crash_ai;
mod curseforge;
mod downloads;
mod management;
mod plugin_runtime;
mod sidecar;
mod updates;

use chrono::Utc;
use download_engine::{
    DownloadEngine, DownloadProgress, DownloadSettings as EngineDownloadSettings,
    aria2::{Aria2Options, Aria2Session},
};
use launcher_core::{
    AccountKind, AccountProfile, CreateInstanceInput, DiagnosticSummary, InstanceOverrides,
    InstanceState, JavaRuntimeInfo, LauncherSettings, LauncherStore, SystemMemoryInfo,
    apply_launch_settings, count_instance_overrides, detect_java_runtime, discover_java_runtimes,
    export_instance_diagnostics, install_vanilla, prepare_authenticated_launch,
    prepare_offline_launch, resolve_settings, summarize_instance, system_memory_info,
    validate_instance_overrides, validate_settings,
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

/// 启动过程的阶段进度。
///
/// PCL2 在启动时显示一条带阶段文字的进度条而不是一个转圈：启动其实分成好几步
/// （校验实例、读配置、刷新登录、拼参数、拉起 JVM），其中「刷新登录」可能等几秒
/// 的网络往返。只给一个不确定的转圈，用户无法区分「在等网络」和「卡死了」。
///
/// `progress` 是 0–1 的整体比例，由各阶段的固定权重累加得到；`stage` 是当前
/// 阶段的展示文案（界面按语言自己映射，后端只给稳定的键）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LaunchProgressEvent {
    instance_id: String,
    /// 稳定的阶段键：`preparing` / `authenticating` / `building` / `starting`。
    stage: String,
    /// 0.0–1.0。
    progress: f64,
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
    let settings = store.load_settings().map_err(|e| e.to_string())?;
    validate_settings(&settings, Some(system_memory_info().maximum_game_mb))?;
    Ok(settings)
}
#[tauri::command]
fn get_system_memory() -> SystemMemoryInfo {
    system_memory_info()
}
/// 报告本构建附带的 aria2 状态。
///
/// 用户开关打开但二进制缺失或校验失败时，界面要能看到原因；否则「已开启加速」
/// 是一个无法验证的承诺。
#[tauri::command]
async fn aria2_status(app: tauri::AppHandle) -> Result<sidecar::Aria2Availability, String> {
    let resource_dir = app.path().resource_dir().map_err(|e| e.to_string())?;
    Ok(sidecar::aria2_availability(&resource_dir).await)
}
#[tauri::command]
fn save_settings(
    settings: LauncherSettings,
    store: State<'_, LauncherStore>,
) -> Result<(), String> {
    validate_settings(&settings, Some(system_memory_info().maximum_game_mb))?;
    store.save_settings(&settings).map_err(|e| e.to_string())
}
#[tauri::command]
fn detect_java() -> Result<JavaRuntimeInfo, String> {
    detect_java_runtime(None).map_err(|e| e.to_string())
}

/// Everything the instance settings page needs in one round trip.
///
/// Returning the resolved values alongside the raw overrides means the frontend
/// never has to re-implement the merge rules. If it did, the page could claim a
/// value that the launch path disagrees with — exactly the mismatch this feature
/// exists to prevent.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct InstanceSettingsView {
    overrides: InstanceOverrides,
    /// Global settings exactly as stored, so the page can label "global is X"
    /// with the real global value instead of the instance's own override.
    global: LauncherSettings,
    /// Global settings with this instance's overrides applied.
    resolved: LauncherSettings,
    /// Number of user-visible settings this instance overrides.
    customized: usize,
}

#[tauri::command]
fn get_instance_settings(
    instance_id: String,
    store: State<'_, LauncherStore>,
) -> Result<InstanceSettingsView, String> {
    let instances = store.list_instances().map_err(|e| e.to_string())?;
    if !instances.iter().any(|instance| instance.id == instance_id) {
        return Err("找不到所选实例。".into());
    }
    let global = store.load_settings().map_err(|e| e.to_string())?;
    let overrides = store
        .load_instance_overrides(&instance_id)
        .map_err(|e| e.to_string())?;
    // Validate the persisted state as well. A row written by an older build, or
    // edited by hand, must surface here instead of failing at launch time.
    validate_instance_overrides(&overrides)?;
    let resolved = resolve_settings(&global, &overrides);
    validate_settings(&resolved, Some(system_memory_info().maximum_game_mb))?;
    let customized = count_instance_overrides(&overrides);
    Ok(InstanceSettingsView {
        overrides,
        global,
        resolved,
        customized,
    })
}

#[tauri::command]
fn save_instance_settings(
    instance_id: String,
    overrides: InstanceOverrides,
    store: State<'_, LauncherStore>,
) -> Result<usize, String> {
    validate_instance_overrides(&overrides)?;
    // Field-level validation is not enough: a combination can be individually
    // legal and still exceed this machine's memory ceiling. Checking the merged
    // result is exactly what the launch path will hand to the JVM.
    let global = store.load_settings().map_err(|e| e.to_string())?;
    let resolved = resolve_settings(&global, &overrides);
    validate_settings(&resolved, Some(system_memory_info().maximum_game_mb))?;
    store
        .save_instance_overrides(&instance_id, &overrides)
        .map_err(|e| e.to_string())?;
    Ok(count_instance_overrides(&overrides))
}

#[tauri::command]
fn reset_instance_settings(
    instance_id: String,
    store: State<'_, LauncherStore>,
) -> Result<(), String> {
    store
        .save_instance_overrides(&instance_id, &InstanceOverrides::default())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn list_java_runtimes() -> Vec<JavaRuntimeInfo> {
    discover_java_runtimes()
}
#[tauri::command]
fn inspect_java_path(path: String) -> Result<JavaRuntimeInfo, String> {
    launcher_core::inspect_java_path(path.into()).map_err(|e| e.to_string())
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

/// 实例里可以被「打开文件夹」定位的子目录。
///
/// 用枚举而不是接收任意相对路径：这个命令最终会把路径交给系统文件管理器，
/// 若接受自由字符串，前端一旦被注入就能让它打开任意位置。只允许这几项，
/// 路径由后端自己拼出来，前端无法越界。
#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceFolder {
    /// 实例根目录（`.minecraft` 同级）。
    Root,
    /// 游戏目录，通常是 `<instance>/game`。
    Game,
    Mods,
    ResourcePacks,
    ShaderPacks,
    Saves,
    Config,
    Logs,
    /// 崩溃报告。
    CrashReports,
}

impl InstanceFolder {
    fn subpath(self) -> Option<&'static str> {
        match self {
            Self::Root | Self::Game => None,
            Self::Mods => Some("mods"),
            Self::ResourcePacks => Some("resourcepacks"),
            Self::ShaderPacks => Some("shaderpacks"),
            Self::Saves => Some("saves"),
            Self::Config => Some("config"),
            Self::Logs => Some("logs"),
            Self::CrashReports => Some("crash-reports"),
        }
    }

    /// 「创建后打开」的目录：游戏首次启动前它们可能不存在，
    /// 直接打开一个不存在的路径在多数系统上会静默失败。
    fn create_on_open(self) -> bool {
        !matches!(self, Self::Root | Self::Game)
    }
}

/// 在系统文件管理器中打开实例的某个子目录。
///
/// 为什么需要这个命令：玩家管理模组、存档、资源包时，最自然的方式是在文件管理器里
/// 操作，而不是让启动器为每种文件都做一套 UI。PCL2 与 HMCL 都有「打开版本文件夹」，
/// 这是实际使用频率很高的入口。
///
/// 安全边界：
///   - 目标目录必须由后端从实例记录拼出，不接受前端传入的路径；
///   - 拼好之后再校验一次它确实位于启动器管理的实例目录内（防止实例记录被篡改）。
#[tauri::command]
fn open_instance_folder(
    instance_id: String,
    folder: InstanceFolder,
    store: tauri::State<'_, LauncherStore>,
) -> Result<String, String> {
    let target = management::instance(&store, &instance_id)?;

    let path = match folder.subpath() {
        // 根目录是实例目录本身；Game 是游戏目录（可能是 `<instance>/game`）。
        None => match folder {
            InstanceFolder::Root => target.instance_dir.clone(),
            _ => target.game_dir.clone(),
        },
        Some(sub) => target.game_dir.join(sub),
    };

    if folder.create_on_open() && !path.exists() {
        std::fs::create_dir_all(&path)
            .map_err(|error| format!("无法创建目录 {}：{error}", path.display()))?;
    }
    if !path.is_dir() {
        return Err(format!("目录不存在：{}", path.display()));
    }

    // 纵深防御：解析真实路径后确认它仍在实例目录内。
    // 实例目录本身可能是符号链接（用户自定义位置），所以两边都 canonicalize
    // 后再比较，而不是比较字面字符串。
    let real_root = std::fs::canonicalize(&target.instance_dir)
        .map_err(|error| format!("无法解析实例目录：{error}"))?;
    let real_path =
        std::fs::canonicalize(&path).map_err(|error| format!("无法解析目录：{error}"))?;
    if !real_path.starts_with(&real_root) {
        return Err("目标目录不在实例目录内，已拒绝打开。".into());
    }

    open_in_file_manager(&real_path)?;
    Ok(real_path.to_string_lossy().into_owned())
}

/// 调用系统默认的文件管理器打开一个目录。
///
/// 直接调用各平台的命令而不是引入 `tauri-plugin-opener`：只需要「打开目录」这一个
/// 能力，引一个插件会顺带获得「打开任意路径/URL」的权限面，反而更难审计。
fn open_in_file_manager(path: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("explorer");
        // explorer 对退出码不可靠（打开成功也可能返回非 0），所以忽略状态。
        command.arg(path);
        return command
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("无法打开文件管理器：{error}"));
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg(path);
        command
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(path);
        command
    };
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("无法打开文件管理器（{error}）。可在设置里手动打开该目录。"))
}
/// 在系统文件管理器中打开「直接下载」保存到的目录。
///
/// 与 `open_instance_folder` 分开是因为安全边界不同：那个只允许实例内的目录，
/// 而这个的路径由用户自己选过（来自系统的文件夹选择对话框），启动器不该、也无法
/// 把它限制在某个根目录里。
///
/// 仍然做了必要的约束：
///   - 必须是绝对路径，避免相对路径被当前工作目录解释成意外位置；
///   - 必须存在且是目录，避免把一个文件路径交给文件管理器（不同平台行为不一）。
#[tauri::command]
fn open_download_folder(path: String) -> Result<(), String> {
    let target = std::path::PathBuf::from(&path);
    if !target.is_absolute() {
        return Err("下载目录必须是绝对路径。".into());
    }
    if !target.is_dir() {
        return Err(format!("目录不存在：{}", target.display()));
    }
    open_in_file_manager(&target)
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
    if settings.download.aria2_enabled
        && let Ok(resource_dir) = app.path().resource_dir()
        && let Ok(binary) = sidecar::verified_aria2(&resource_dir).await
    {
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
    let client = reqwest::Client::builder()
        .user_agent(concat!("MFMCL/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;
    let control = downloads.start(
        &instance_id,
        // 已是 ready 的实例走这条命令就是「补全」；任务标题如实反映，
        // 否则用户会以为自己在重装整个游戏。
        &if instance.state == InstanceState::Ready {
            format!("补全 {}", instance.name)
        } else {
            format!("安装 {}", instance.name)
        },
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
        Ok(_) => {
            /*
             * 官方文件补齐之后，接着处理加载器。
             *
             * 两步都不能省，因为它们的文件来源不同：`install_vanilla` 只碰 Mojang
             * 那一侧（客户端 jar、官方库、资源、日志配置），而加载器的 jar 来自
             * Fabric / Quilt / Forge 各自的 maven。用户误删或磁盘出错时，官方文件
             * 可能完好、加载器的文件却已经不在了——那种情况下补全「成功」、游戏
             * 仍然起不来，报的是「找不到主类」这种和缺失文件看起来无关的错误。
             *
             * 顺序有意义：先补官方文件（加载器配置依赖原版 profile），再诊断加载器。
             */
            let mut failure: Option<String> = None;
            if let Err(error) =
                management::install_pending_pack_loader(&root, &instance, &store, &engine).await
            {
                failure = Some(error);
            }
            if failure.is_none() {
                match launcher_core::loaders::diagnose_loader(&instance, &root.join("libraries")) {
                    launcher_core::loaders::LoaderHealth::Broken(reason) => {
                        if let Err(error) =
                            management::repair_loader(&root, &instance, &store, &engine).await
                        {
                            failure = Some(format!(
                                "加载器文件需要重装，但重装失败：{error}\n（诊断：{reason}）"
                            ));
                        }
                    }
                    launcher_core::loaders::LoaderHealth::Healthy
                    | launcher_core::loaders::LoaderHealth::NotInstalled => {}
                }
            }
            match failure {
                Some(error) => Err(error),
                None => Ok(()),
            }
        }
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
    /*
     * 启动进度上报。
     *
     * 各阶段的权重按「用户实际要等多久」分配，而不是步骤数量：认证通常是最慢的
     * 一步（一次或多跳网络往返），参数拼装几乎瞬时。权重固定，所以进度条的推进
     * 速度在一台机器上是一致的，不会忽快忽慢。
     */
    let report_launch = |stage: &str, progress: f64| {
        let _ = app.emit(
            "launch-progress",
            LaunchProgressEvent {
                instance_id: instance_id.clone(),
                stage: stage.to_owned(),
                progress,
            },
        );
    };
    report_launch("preparing", 0.05);
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
    validate_settings(&settings, Some(system_memory_info().maximum_game_mb))?;
    // Instance overrides are merged here, once, before anything downstream reads
    // a setting. Everything below therefore works with the values this instance
    // will actually run with, and `apply_launch_settings` stays instance-agnostic.
    let overrides = store
        .load_instance_overrides(&instance_id)
        .map_err(|e| e.to_string())?;
    let settings = resolve_settings(&settings, &overrides);
    let mut secrets = Vec::new();
    let mut prepared = if account.kind == AccountKind::Offline {
        // 离线档案不联网，这一步很快；仍给一个明确的阶段，避免进度条停在原地。
        report_launch("preparing", 0.25);
        prepare_offline_launch(
            &root,
            &instance,
            &account,
            &metadata,
            settings.java_path.clone(),
        )
        .map_err(|e| e.to_string())?
    } else {
        // 认证是整条链路上最可能耗时的一步：需要向认证服务器刷新令牌。
        report_launch("authenticating", 0.2);
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
            settings.java_path.clone(),
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
    apply_launch_settings(
        &mut prepared,
        &settings,
        system_memory_info().recommended_mb,
        metadata
            .main_class
            .as_deref()
            .ok_or("Minecraft 主类缺失。")?,
    )
    .map_err(|error| error.to_string())?;
    report_launch("building", 0.75);
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
    /*
     * 插件贡献。
     *
     * 计划必须在 `apply_launch_settings` 之后才算得出来：那条路径会重排参数、插入
     * `-Xmx`，我们要插在主类之前的位置必须与它对齐。所以这里（参数已定、进程未起）
     * 是唯一正确的时机。
     *
     * 每个被拒绝的贡献都记进 `plan.report.skipped`，随后写进启动历史。用户点开
     * 「启动记录」就能看到「这个插件声明了 jvmArgs，但缺少 launchHook 权限」——
     * 没有这份记录，「已启用却什么也没发生」就只能靠猜。
     */
    let plugin_state = store.load_plugin_state().unwrap_or_default();
    let plugin_definitions = crate::plugin_runtime::catalog(&plugin_state);
    let plan = launcher_core::build_plan(&plugin_definitions, &plugin_state, &instance, &settings);
    /*
     * 插入 JVM 参数。失败只在「找不到主类」时发生，此时参数已经坏了，
     * 少几个插件参数比让启动直接报错好——所以只记录、不中断。
     */
    if !plan.jvm_args.is_empty() {
        launcher_core::insert_jvm_args(
            &mut prepared.args,
            &plan.jvm_args,
            metadata.main_class.as_deref().unwrap_or_default(),
        );
    }
    /*
     * 启动前备份。
     *
     * 放在这里而不是 spawn 之后：备份的意义是「万一这次启动把存档写坏了还能回去」，
     * 必须在游戏进程接触文件之前完成。失败不阻止启动——一个坏掉的备份目录不该
     * 让用户玩不了游戏，但要在历史里留下记录。
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
            Ok(Ok(path)) => {
                let _ = app.emit(
                    "plugin-backup",
                    serde_json::json!({
                        "instanceId": instance_id,
                        "pluginId": plugin_id,
                        "path": path.to_string_lossy(),
                    }),
                );
            }
            Ok(Err(error)) => backup_failures.push(format!("{plugin_id}: {error}")),
            Err(_) => backup_failures.push(format!("{plugin_id}: 备份任务未能完成")),
        }
    }
    // 启动前命令（全局设置 + 插件贡献）。任何一条失败都中断启动：
    // 用户配置的「启动前命令」失败意味着环境没准备好，继续启动只会得到更奇怪的错误。
    if !settings.pre_launch_command.trim().is_empty() {
        run_pre_launch_command(&settings.pre_launch_command, &prepared.working_directory).await?;
    }
    for (plugin_id, command) in &plan.pre_launch {
        run_pre_launch_command(command, &prepared.working_directory)
            .await
            .map_err(|error| format!("插件 {plugin_id} 的启动前命令失败：{error}"))?;
    }
    let shared_log = Arc::new(tokio::sync::Mutex::new(log_file));
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    {
        let mut running = registry.lock().map_err(|_| "启动器进程状态不可用。")?;
        if running.contains_key(&instance_id) {
            return Err("此实例已经在运行。".into());
        }
        running.insert(instance_id.clone(), stop_tx);
    }
    // JVM 从 spawn 到窗口出现有一段可感知的等待（加载几千个类），给个明确阶段。
    report_launch("starting", 0.9);
    let mut command = Command::new(&prepared.java_path);
    command
        .args(&prepared.args)
        .current_dir(&prepared.working_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    /*
     * 插件贡献的环境变量。
     *
     * 在 `spawn` 之前逐个 `env` 设置，而不是先读环境再合并：`Command` 会以父进程
     * 的环境为基底，这里只需要追加（同名覆盖）。键与值都已在校验阶段限制过字符集
     * 与长度，不会注入意外的进程属性。
     */
    for (key, value) in &plan.env {
        command.env(key, value);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            if let Ok(mut running) = registry.lock() {
                running.remove(&instance_id);
            }
            return Err(format!("无法启动 Java：{error}"));
        }
    };
    let launcher_hidden = settings.launcher_visibility == launcher_core::LauncherVisibility::Hide;
    if launcher_hidden && let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
    /*
     * 记录启动时间。
     *
     * 位置很关键：必须在 Java 进程真的跑起来之后，而不是在参数准备好时。用户点了
     * 启动但 JVM 起不来（缺内存、Java 路径失效）的情况不算「玩过」，那些实例不该
     * 抢走「上次启动的实例」这个默认位。
     *
     * 写入失败不影响启动：这只是一个便利信息，不值得让已经跑起来的游戏失败。
     */
    let _ = store.mark_instance_launched(&instance_id);
    /*
     * 登记启动历史。
     *
     * 与 `mark_instance_launched` 同一时机（进程真的起来了）。`contributions` 记录
     * 本次每个插件做了什么、以及哪些被拒绝——这是「插件能不能用」最直接的证据。
     * 备份失败也写进条目，让用户在历史里看到它，而不是只在一次性 toast 里闪过。
     *
     * 受「启动记录」插件开关控制：关掉它就不写历史。这不是可选项——用户关掉一个
     * 叫「启动记录」的插件，唯一合理的理解就是「不要记录」，如果照写不误，这个开关
     * 就是假的。
     */
    let history_id = uuid::Uuid::new_v4().to_string();
    let history_enabled = crate::plugin_runtime::is_enabled(&plugin_state, "launch-logger");
    let mut history_report = plan.report.clone();
    for failure in &backup_failures {
        history_report.skipped.push(launcher_core::SkippedContribution {
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
    /*
     * 进程结束时的回填需要自己的 store 句柄：`store` 是 `State` 借用，不能带进 spawn。
     * 历史被关掉时 `history_id` 是空串，`finish_launch` 不会命中任何行，等价于空操作。
     */
    let history_store = store.inner().clone();
    let history_id_for_exit = if history_enabled { history_id } else { String::new() };
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
    // 进程已起来：进度到 100%，界面据此收起进度条。
    report_launch("running", 1.0);
    let _ = app.emit(
        "game-process",
        GameProcessEvent {
            instance_id: instance_id.clone(),
            status: "running".into(),
            exit_code: None,
            log_path: log_path.clone(),
        },
    );
    let _spawned = tauri::async_runtime::spawn(async move {
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
        if launcher_hidden && let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.set_focus();
        }
        /*
         * 回填启动历史的结果。
         *
         * 与接下来的 `game-process` 事件使用同一个判定（用户主动停止或退出码为 0
         * 都算正常结束），避免两处对「这次算不算崩溃」给出不同答案。
         */
        let outcome = if stopped_by_user || exit_code == Some(0) {
            "stopped"
        } else {
            "crashed"
        };
        let _ = history_store.finish_launch(&history_id_for_exit, outcome, exit_code);        let _ = app.emit(
            "game-process",
            GameProcessEvent {
                instance_id,
                status: outcome.into(),
                exit_code,
                log_path,
            },
        );
    });
    Ok(())
}

async fn run_pre_launch_command(
    command: &str,
    working_directory: &std::path::Path,
) -> Result<(), String> {
    let parts = shlex::split(command)
        .ok_or_else(|| "启动前命令格式无效，请检查引号是否成对。".to_owned())?;
    let (program, args) = parts
        .split_first()
        .ok_or_else(|| "启动前命令不能为空。".to_owned())?;
    if program.contains("..") {
        return Err("启动前命令不能使用包含 .. 的路径。".into());
    }
    let status = Command::new(program)
        .args(args)
        .current_dir(working_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .status()
        .await
        .map_err(|error| format!("无法执行启动前命令：{error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "启动前命令退出码异常：{}",
            status
                .code()
                .map_or_else(|| "unknown".to_owned(), |code| code.to_string())
        ))
    }
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
            app.manage(crash_ai::CrashAiState::default());
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
            management::inspect_mods,
            management::set_mod_enabled,
            management::check_mod_updates,
            management::update_mod,
            management::check_client_update,
            curseforge::curseforge_key_status,
            curseforge::curseforge_save_key,
            curseforge::curseforge_clear_key,
            curseforge::curseforge_search,
            curseforge::curseforge_files,
            curseforge::curseforge_install,
            curseforge::curseforge_download,
            management::search_mods,
            management::list_mod_versions,
            management::install_mod_version,
            management::download_mod_version,
            management::delete_instance,
            management::preview_modpack,
            management::import_modpack,
            management::download_and_import_modpack,
            management::export_modpack,
            list_versions,
            get_settings,
            save_settings,
            get_instance_settings,
            save_instance_settings,
            reset_instance_settings,
            get_system_memory,
            aria2_status,
            detect_java,
            list_java_runtimes,
            inspect_java_path,
            get_system_info,
            open_instance_folder,
            open_download_folder,
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
            plugin_runtime::plugin_runtime_info,
            plugin_runtime::plugin_state,
            plugin_runtime::plugin_save_state,
            plugin_runtime::plugin_preview,
            plugin_runtime::launch_history,
            plugin_runtime::clear_launch_history,
            crash_ai::crash_ai_status,
            crash_ai::crash_ai_analyse,
            crash_ai::crash_ai_instance_report,
        ])
        .run(tauri::generate_context!())
        .expect("MFMCL desktop application failed");
}
