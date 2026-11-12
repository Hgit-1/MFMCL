use download_engine::DownloadControl;
use launcher_core::{
    InstanceRecord, JavaRuntimeInfo, LauncherStore,
    java_runtime::{
        JavaPreferences, JavaPreferencesUpdate, JavaSelectionRequest, JavaSelectionResult,
    },
};
use mc_metadata::GameVersionMetadata;
use std::{collections::HashMap, path::Path, sync::Mutex};
use tauri::{Emitter, Manager, State};

#[derive(Default)]
pub struct JavaInstalls(Mutex<HashMap<String, DownloadControl>>);

struct InstallGuard<'a> {
    registry: &'a JavaInstalls,
    instance_id: String,
}
impl Drop for InstallGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut installs) = self.registry.0.lock() {
            installs.remove(&self.instance_id);
        }
    }
}

pub fn selection_request(
    store: &LauncherStore,
    instance_id: &str,
    selection: Option<JavaSelectionRequest>,
    legacy_path: Option<String>,
    global_path: Option<&Path>,
) -> Result<JavaSelectionRequest, String> {
    store
        .resolve_java_selection(instance_id, selection, legacy_path, global_path)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_java_preferences(store: State<'_, LauncherStore>) -> Result<JavaPreferences, String> {
    store.load_java_preferences().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_java_preferences(
    update: JavaPreferencesUpdate,
    settings: Option<launcher_core::LauncherSettings>,
    store: State<'_, LauncherStore>,
) -> Result<JavaPreferences, String> {
    if let Some(settings) = &settings {
        launcher_core::validate_settings(
            settings,
            Some(launcher_core::system_memory_info().maximum_game_mb),
        )?;
    }
    store
        .save_java_preferences(&update, settings.as_ref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn migrate_legacy_java_preferences(
    legacy: JavaPreferences,
    store: State<'_, LauncherStore>,
) -> Result<JavaPreferences, String> {
    store
        .migrate_legacy_java_preferences(&legacy)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_preferred_java_runtimes(
    app: tauri::AppHandle,
    store: State<'_, LauncherStore>,
) -> Result<Vec<JavaRuntimeInfo>, String> {
    let preferences = store.load_java_preferences().map_err(|e| e.to_string())?;
    let root = app.path().app_data_dir().map_err(|e| e.to_string())?;
    tokio::task::spawn_blocking(move || {
        let mut runtimes: Vec<_> = preferences
            .ordered_paths
            .iter()
            .filter_map(|path| launcher_core::inspect_java_path(path.into()).ok())
            .collect();
        runtimes.extend(launcher_core::java_runtime::managed_java_runtimes(&root));
        runtimes.extend(launcher_core::discover_java_runtimes());
        let mut seen = std::collections::HashSet::new();
        runtimes.retain(|runtime| {
            seen.insert(if cfg!(windows) {
                runtime.path.replace('/', "\\").to_lowercase()
            } else {
                runtime.path.clone()
            })
        });
        runtimes
    })
    .await
    .map_err(|e| e.to_string())
}

async fn profile(
    store: &LauncherStore,
    instance_id: &str,
) -> Result<(InstanceRecord, GameVersionMetadata), String> {
    let instance = store
        .list_instances()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|i| i.id == instance_id)
        .ok_or("找不到所选实例。")?;
    let path = if instance.loader == "vanilla" {
        instance
            .version_dir()
            .join(format!("{}.json", instance.game_version))
    } else {
        instance.version_dir().join("launch.json")
    };
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| format!("无法读取版本配置：{e}"))?;
    let metadata = serde_json::from_slice(&bytes).map_err(|e| format!("版本配置已损坏：{e}"))?;
    Ok((instance, metadata))
}

#[tauri::command]
pub async fn select_instance_java(
    instance_id: String,
    selection: Option<JavaSelectionRequest>,
    app: tauri::AppHandle,
    store: State<'_, LauncherStore>,
) -> Result<JavaSelectionResult, String> {
    let (instance, metadata) = profile(&store, &instance_id).await?;
    let settings = store.load_settings().map_err(|e| e.to_string())?;
    let request = selection_request(
        &store,
        &instance_id,
        selection,
        None,
        settings.java_path.as_deref(),
    )?;
    let root = app.path().app_data_dir().map_err(|e| e.to_string())?;
    tokio::task::spawn_blocking(move || {
        launcher_core::java_runtime::select_java(&root, &instance, &metadata, &request)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

pub async fn ensure_for_launch(
    app: &tauri::AppHandle,
    registry: &JavaInstalls,
    root: &Path,
    instance: &InstanceRecord,
    metadata: &GameVersionMetadata,
    request: &JavaSelectionRequest,
) -> Result<JavaRuntimeInfo, String> {
    let (select_root, select_instance, select_metadata, select_request) = (
        root.to_owned(),
        instance.clone(),
        metadata.clone(),
        request.clone(),
    );
    let selected = tokio::task::spawn_blocking(move || {
        launcher_core::java_runtime::select_java(
            &select_root,
            &select_instance,
            &select_metadata,
            &select_request,
        )
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    if let Some(runtime) = selected.runtime {
        return Ok(runtime);
    }
    if !selected.can_auto_install {
        return Err("找不到符合设置的 Java。".into());
    }
    let control = DownloadControl::new();
    {
        let mut installs = registry.0.lock().map_err(|_| "Java 安装状态不可用。")?;
        if installs.contains_key(&instance.id) {
            return Err("Java 正在安装。".into());
        }
        installs.insert(instance.id.clone(), control.clone());
    }
    let _guard = InstallGuard {
        registry,
        instance_id: instance.id.clone(),
    };
    let _ = app.emit(
        "launch-progress",
        super::LaunchProgressEvent {
            instance_id: instance.id.clone(),
            stage: "java_installing".into(),
            progress: 0.12,
        },
    );
    let network = app
        .state::<LauncherStore>()
        .load_settings()
        .map_err(|e| e.to_string())?
        .download;
    let runtime = launcher_core::java_runtime::install_official_java_with_settings(
        root,
        &selected.requirement,
        &control,
        &network,
    )
    .await
    .map_err(|e| e.to_string())?;
    let _ = app.emit("java-runtime-installed", &runtime);
    Ok(runtime)
}

#[tauri::command]
pub async fn install_instance_java(
    instance_id: String,
    selection: Option<JavaSelectionRequest>,
    app: tauri::AppHandle,
    store: State<'_, LauncherStore>,
    installs: State<'_, JavaInstalls>,
    operations: State<'_, super::management::Operations>,
) -> Result<JavaRuntimeInfo, String> {
    let _operation = operations.acquire(&instance_id)?;
    let (instance, metadata) = profile(&store, &instance_id).await?;
    let settings = store.load_settings().map_err(|e| e.to_string())?;
    let request = selection_request(
        &store,
        &instance_id,
        selection,
        None,
        settings.java_path.as_deref(),
    )?;
    let root = app.path().app_data_dir().map_err(|e| e.to_string())?;
    ensure_for_launch(&app, &installs, &root, &instance, &metadata, &request).await
}

#[tauri::command]
pub fn cancel_java_install(
    instance_id: String,
    installs: State<'_, JavaInstalls>,
) -> Result<(), String> {
    let installs = installs.0.lock().map_err(|_| "Java 安装状态不可用。")?;
    if let Some(control) = installs.get(&instance_id) {
        control.cancel();
    }
    Ok(())
}
