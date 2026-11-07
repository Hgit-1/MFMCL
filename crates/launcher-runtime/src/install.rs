//! Installation-scoped download tasks and the install orchestration.
//!
//! ## Why task tracking lives here
//!
//! The desktop version kept it in `src-tauri/src/downloads.rs` and took an
//! `AppHandle` purely to emit events. The tracking itself — task rows, the
//! 250 ms persist cadence, whole-task progress — is frontend-independent. Moving
//! it here means the CLI writes the same task history the GUI shows, instead of
//! leaving a database that says "nothing was ever downloaded".

use crate::events::{EventSink, ProgressEvent, TaskEvent};
use crate::{Result, RuntimeContext, RuntimeError};
use download_engine::{
    DownloadControl, DownloadEngine, DownloadProgress, TaskProgress,
    aria2::{Aria2Options, Aria2Session},
};
use launcher_core::{
    CreateInstanceInput, DownloadTask, DownloadTaskState, InstanceRecord, InstanceState,
    LauncherSettings, LauncherStore,
};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// One in-flight download task.
struct ActiveJob {
    control: DownloadControl,
    task: DownloadTask,
    last_saved: Instant,
}

/// Registry of running tasks, keyed by instance id.
///
/// At most one task per instance: two concurrent installs of the same instance
/// would write the same files.
#[derive(Default)]
pub struct Downloads(Mutex<HashMap<String, ActiveJob>>);

/// What an install needs to hand the download engine.
pub struct TaskHandle {
    pub control: DownloadControl,
    pub progress: Arc<TaskProgress>,
}

/*
 * Hand-written because `TaskProgress` holds a mutex and internal bookkeeping that
 * would be useless -- and misleading -- in a panic message. `unwrap_err()` in the
 * tests needs some form of `Debug` on the success type, and this is the smallest
 * one that stays honest.
 */
impl std::fmt::Debug for TaskHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskHandle")
            .field("control", &self.control)
            .finish_non_exhaustive()
    }
}

impl Downloads {
    pub fn start(
        &self,
        instance_id: &str,
        label: &str,
        store: &LauncherStore,
        sink: &dyn EventSink,
    ) -> Result<TaskHandle> {
        let now = chrono::Utc::now().to_rfc3339();
        let task = DownloadTask {
            id: uuid::Uuid::new_v4().to_string(),
            instance_id: instance_id.into(),
            label: label.into(),
            source: None,
            state: DownloadTaskState::Running,
            completed_bytes: 0,
            total_bytes: None,
            files: None,
            completed_files: None,
            created_at: now.clone(),
            updated_at: now,
        };
        let mut jobs = self
            .0
            .lock()
            .map_err(|_| RuntimeError::message("下载任务状态不可用。"))?;
        /*
         * A cancelled job stays in the map until its task is removed, so the
         * user can press "install" again immediately. Reporting the *reason*
         * matters: "already running" and "still cancelling" call for different
         * actions from the user.
         */
        if let Some(existing) = jobs.get(instance_id) {
            if !existing.control.is_cancelled() {
                return Err(RuntimeError::message("此实例已有下载任务。"));
            }
            return Err(RuntimeError::message("正在取消旧任务，请稍候重试。"));
        }
        store.save_download_task(&task)?;
        let control = DownloadControl::new();
        let progress = Arc::new(TaskProgress::new());
        jobs.insert(
            instance_id.into(),
            ActiveJob {
                control: control.clone(),
                task: task.clone(),
                last_saved: Instant::now(),
            },
        );
        sink.installation_task(&TaskEvent {
            instance_id: instance_id.into(),
            task,
        });
        Ok(TaskHandle { control, progress })
    }

    /// Record a file progress event against its task.
    ///
    /// Returns the task record id so the caller can attribute the file line to
    /// the right card. `None` means the task already finished.
    pub fn progress(
        &self,
        instance_id: &str,
        progress: &DownloadProgress,
        store: &LauncherStore,
        sink: &dyn EventSink,
    ) -> Option<String> {
        let Ok(mut jobs) = self.0.lock() else {
            return None;
        };
        let job = jobs.get_mut(instance_id)?;
        /*
         * Persist whole-task numbers, never this file's. Taking the single-file
         * figure would make the bar jump back to a few percent whenever the
         * download moves to the next file.
         */
        if let Some(snapshot) = progress.task {
            job.task.completed_bytes = snapshot.completed_bytes;
            job.task.total_bytes = snapshot.total_bytes;
            job.task.files = Some(snapshot.files);
            job.task.completed_files = Some(snapshot.completed_files);
        }
        job.task.source = Some(progress.source.clone());
        if job.last_saved.elapsed() >= Duration::from_millis(250) {
            job.task.updated_at = chrono::Utc::now().to_rfc3339();
            let _ = store.save_download_task(&job.task);
            sink.installation_task(&TaskEvent {
                instance_id: instance_id.into(),
                task: job.task.clone(),
            });
            job.last_saved = Instant::now();
        }
        Some(job.task.id.clone())
    }

    pub fn finish(
        &self,
        instance_id: &str,
        success: bool,
        store: &LauncherStore,
        sink: &dyn EventSink,
    ) -> Result<()> {
        let mut jobs = self
            .0
            .lock()
            .map_err(|_| RuntimeError::message("下载任务状态不可用。"))?;
        if let Some(mut job) = jobs.remove(instance_id) {
            job.task.state = if success {
                DownloadTaskState::Completed
            } else if job.control.is_cancelled() {
                DownloadTaskState::Cancelled
            } else {
                DownloadTaskState::Failed
            };
            job.task.updated_at = chrono::Utc::now().to_rfc3339();
            store.save_download_task(&job.task)?;
            sink.installation_task(&TaskEvent {
                instance_id: instance_id.into(),
                task: job.task,
            });
        }
        Ok(())
    }

    pub fn control(&self, instance_id: &str) -> Option<DownloadControl> {
        self.0
            .lock()
            .ok()?
            .get(instance_id)
            .map(|job| job.control.clone())
    }

    /// Pause/resume/cancel, updating both the engine and the persisted row.
    pub fn set_control(
        &self,
        instance_id: &str,
        action: &str,
        store: &LauncherStore,
        sink: &dyn EventSink,
    ) -> Result<()> {
        let mut jobs = self
            .0
            .lock()
            .map_err(|_| RuntimeError::message("下载任务状态不可用。"))?;
        let job = jobs
            .get_mut(instance_id)
            .ok_or_else(|| RuntimeError::message("下载任务已结束；请使用安装按钮重试。"))?;
        if job.control.is_cancelled() {
            return Err(RuntimeError::message("正在取消，请等待任务结束。"));
        }
        let state = match action {
            "pause" => DownloadTaskState::Paused,
            "resume" => DownloadTaskState::Running,
            "cancel" => DownloadTaskState::Cancelled,
            _ => return Err(RuntimeError::message("未知下载操作。")),
        };
        let mut task = job.task.clone();
        task.state = state;
        task.updated_at = chrono::Utc::now().to_rfc3339();
        store.save_download_task(&task)?;
        match action {
            "pause" => job.control.pause(),
            "resume" => job.control.resume(),
            "cancel" => job.control.cancel(),
            _ => unreachable!("validated above"),
        }
        job.task = task.clone();
        sink.installation_task(&TaskEvent {
            instance_id: instance_id.into(),
            task,
        });
        Ok(())
    }
}

/// Build a download engine from the user's stored settings.
///
/// `aria2_enabled` is forced off: this crate has no resource directory to verify
/// a pinned binary against, and the manifest ships with no pins anyway. The
/// desktop path that *can* use aria2 keeps passing its own engine in.
pub fn engine_from_settings(settings: &LauncherSettings) -> Result<DownloadEngine> {
    DownloadEngine::new(download_engine::DownloadSettings {
        source: settings.download.source,
        max_concurrent_downloads: settings.download.max_concurrent_downloads,
        connections_per_file: settings.download.connections_per_file,
        aria2_enabled: false,
    })
    .map_err(|error| RuntimeError::message(format!("无法创建下载器：{error}")))
}

/// Start the pinned aria2 sidecar when one is present and verified.
///
/// Returns `None` rather than an error when the binary is missing: the Rust
/// downloader is the supported default, and a build without a pinned binary is
/// the normal case for this repository.
pub async fn maybe_aria2(
    context: &RuntimeContext,
    settings: &LauncherSettings,
) -> Option<(Aria2Session, std::path::PathBuf)> {
    if !settings.download.aria2_enabled {
        return None;
    }
    let resource_dir = context.resource_dir()?;
    let workdir = context.data_root().join("aria2");
    let options = Aria2Options {
        max_concurrent_downloads: settings.download.max_concurrent_downloads,
        connections_per_file: settings.download.connections_per_file,
    };
    let binary = launcher_sidecar_path(resource_dir);
    let session = Aria2Session::start(&binary, &workdir, options).await.ok()?;
    Some((session, binary))
}

/// Where a pinned aria2 would live. Mirrors `src-tauri/src/sidecar.rs` so the
/// two agree on the layout if a build ever ships one.
fn launcher_sidecar_path(resource_dir: &Path) -> std::path::PathBuf {
    let executable = if cfg!(windows) {
        "aria2c.exe"
    } else {
        "aria2c"
    };
    resource_dir
        .join("aria2")
        .join(std::env::consts::OS)
        .join(std::env::consts::ARCH)
        .join(executable)
}

/// Create an instance record without installing it.
///
/// The loader request is written as a marker file rather than installed here, so
/// that a multi-minute Forge processor run does not happen inside "create".
pub fn create_instance(
    context: &RuntimeContext,
    store: &LauncherStore,
    name: &str,
    game_version: &str,
    loader: Option<&str>,
) -> Result<InstanceRecord> {
    let record = store.create_instance(
        &CreateInstanceInput {
            name: name.to_owned(),
            game_version: game_version.to_owned(),
        },
        context.data_root(),
    )?;
    if let Some(request) = loader.map(str::trim).filter(|value| !value.is_empty()) {
        /*
         * Roll back on failure. A marker that cannot be written means the
         * instance directory is not writable, so nothing will ever install into
         * it; leaving the record behind only produces an instance that can never
         * work.
         */
        if let Err(error) = crate::install_support::request_pack_loader(&record, request) {
            let _ = store.discard_uninstalled_instance(&record.id);
            let _ = std::fs::remove_dir_all(&record.instance_dir);
            return Err(error);
        }
    }
    Ok(record)
}

/// Install (or complete) one instance.
///
/// This is the whole sequence the desktop "install" button ran: create the
/// download engine, start a task, download vanilla files, then consume any
/// pending loader request and repair a broken loader overlay. Order matters —
/// the loader overlay depends on the vanilla profile existing.
pub async fn install_instance(
    context: &RuntimeContext,
    store: &LauncherStore,
    downloads: &Downloads,
    sink: &dyn EventSink,
    instance_id: &str,
) -> Result<()> {
    let instance = find_instance(store, instance_id)?;
    let settings = store.load_settings()?;
    let mut engine = engine_from_settings(&settings)?;

    let aria2_session = match maybe_aria2(context, &settings).await {
        Some((session, _)) => {
            engine = engine.with_aria2(session.clone());
            Some(session)
        }
        None => None,
    };

    let handle = downloads.start(
        instance_id,
        // "Complete" vs "install": pressing install on a ready instance is a
        // repair, and the title should say so.
        &if instance.state == InstanceState::Ready {
            format!("补全 {}", instance.name)
        } else {
            format!("安装 {}", instance.name)
        },
        store,
        sink,
    )?;
    engine = engine
        .with_control(handle.control)
        .with_task_progress(handle.progress);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<DownloadProgress>();
    /*
     * The progress pump needs its own references: it outlives this borrow while
     * the install runs. `store` is behind `Arc<Mutex<_>>` internally, so cloning
     * is cheap and shares the same connection.
     */
    let progress_store = store.clone();
    let progress_result: Arc<Mutex<Option<Result<()>>>> = Arc::new(Mutex::new(None));
    let progress_outcome = progress_result.clone();
    let instance_for_progress = instance_id.to_owned();
    let pump = async move {
        while let Some(progress) = rx.recv().await {
            let task_id =
                downloads.progress(&instance_for_progress, &progress, &progress_store, sink);
            sink.download_progress(&ProgressEvent {
                task_id: task_id.unwrap_or_default(),
                instance_id: instance_for_progress.clone(),
                progress,
            });
        }
        // The receiver loop ending is the normal case; nothing to report.
        if let Ok(mut slot) = progress_outcome.lock() {
            slot.replace(Ok(()));
        }
    };

    let client = reqwest::Client::builder()
        .user_agent(concat!("MFMCL/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|error| RuntimeError::message(format!("无法创建网络客户端：{error}")))?;

    // Run the pump concurrently with the install so progress is live, not
    // replayed after the fact.
    let (install_result, _) = tokio::join!(
        async {
            launcher_core::install_vanilla(
                &client,
                engine.clone(),
                store,
                context.data_root(),
                &instance,
                Some(tx),
            )
            .await
        },
        pump,
    );

    let result = match install_result {
        Ok(_) => {
            let mut failure: Option<String> = None;
            if let Err(error) = crate::install_support::install_pending_pack_loader(
                context, &instance, store, &engine,
            )
            .await
            {
                failure = Some(error.to_string());
            }
            if failure.is_none() {
                match launcher_core::loaders::diagnose_loader(
                    &instance,
                    &context.data_root().join("libraries"),
                ) {
                    launcher_core::loaders::LoaderHealth::Broken(reason) => {
                        if let Err(error) = crate::install_support::repair_loader(
                            context, &instance, store, &engine,
                        )
                        .await
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
                Some(error) => Err(RuntimeError::message(error)),
                None => Ok(()),
            }
        }
        Err(error) => Err(RuntimeError::message(error.to_string())),
    };

    if result.is_err() {
        let _ = store.set_instance_state(instance_id, InstanceState::Failed);
    }
    if let Some(session) = aria2_session {
        let _ = session.shutdown().await;
    }
    downloads.finish(instance_id, result.is_ok(), store, sink)?;
    result
}

pub fn find_instance(store: &LauncherStore, id: &str) -> Result<InstanceRecord> {
    store
        .list_instances()?
        .into_iter()
        .find(|instance| instance.id == id)
        .ok_or_else(|| RuntimeError::message("找不到所选实例。"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::VecSink;

    fn temp_root(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("mfmcl-runtime-{tag}-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn a_second_install_of_the_same_instance_is_refused() {
        let root = temp_root("dup");
        let store = LauncherStore::open(root.join("mfmcl.sqlite")).unwrap();
        let downloads = Downloads::default();
        let sink = VecSink::new();
        let first = downloads.start("inst", "安装", &store, &sink).unwrap();
        assert!(!first.control.is_cancelled());
        let second = downloads.start("inst", "安装", &store, &sink);
        assert!(second.is_err(), "concurrent installs must be refused");
        assert_eq!(sink.tasks.lock().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cancel_then_retry_reports_the_right_reason() {
        // The message differs on purpose: one tells the user to wait, the other
        // tells them a task exists. Getting them backwards sends the user to the
        // wrong remedy.
        let root = temp_root("cancel");
        let store = LauncherStore::open(root.join("mfmcl.sqlite")).unwrap();
        let downloads = Downloads::default();
        let sink = VecSink::new();
        let handle = downloads.start("inst", "安装", &store, &sink).unwrap();
        handle.control.cancel();
        let error = downloads.start("inst", "安装", &store, &sink).unwrap_err();
        assert!(error.to_string().contains("取消"), "got {error}");
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn finishing_records_cancelled_rather_than_failed() {
        let root = temp_root("finish");
        let store = LauncherStore::open(root.join("mfmcl.sqlite")).unwrap();
        let downloads = Downloads::default();
        let sink = VecSink::new();
        let handle = downloads.start("inst", "安装", &store, &sink).unwrap();
        handle.control.cancel();
        downloads.finish("inst", false, &store, &sink).unwrap();
        let tasks = sink.tasks.lock().unwrap();
        assert_eq!(
            tasks.last().unwrap().task.state,
            DownloadTaskState::Cancelled
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn create_instance_rolls_back_when_the_loader_request_is_invalid() {
        let root = temp_root("rollback");
        let store = LauncherStore::open(root.join("mfmcl.sqlite")).unwrap();
        let context = RuntimeContext::new(&root);
        // A loader string without `@version` is rejected before anything is
        // written, which is exactly the case that used to leave a broken record.
        let error =
            create_instance(&context, &store, "probe", "1.21.4", Some("neoforge")).unwrap_err();
        assert!(error.to_string().contains("加载器"), "got {error}");
        assert!(
            store.list_instances().unwrap().is_empty(),
            "a failed create must not leave an instance behind"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn engine_is_built_from_stored_settings() {
        let settings = LauncherSettings::default();
        let engine = engine_from_settings(&settings);
        assert!(engine.is_ok());
    }
}
