//! Installation-scoped controls and persistent, truthful task status.
use download_engine::{DownloadControl, DownloadProgress};
use launcher_core::{DownloadTask, DownloadTaskState, LauncherStore};
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, State};

struct ActiveJob {
    control: DownloadControl,
    task: DownloadTask,
    last_saved: Instant,
}
#[derive(Default)]
pub struct Downloads(Mutex<HashMap<String, ActiveJob>>);
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskEvent {
    pub instance_id: String,
    pub task: DownloadTask,
}

impl Downloads {
    pub fn start(
        &self,
        id: &str,
        label: &str,
        store: &LauncherStore,
        app: &AppHandle,
    ) -> Result<DownloadControl, String> {
        let now = chrono::Utc::now().to_rfc3339();
        let task = DownloadTask {
            id: uuid::Uuid::new_v4().to_string(),
            instance_id: id.into(),
            label: label.into(),
            source: None,
            state: DownloadTaskState::Running,
            completed_bytes: 0,
            total_bytes: None,
            created_at: now.clone(),
            updated_at: now,
        };
        let mut jobs = self.0.lock().map_err(|_| "下载任务状态不可用。")?;
        if let Some(existing) = jobs.get(id) {
            if !existing.control.is_cancelled() {
                return Err("此实例已有下载任务。".into());
            }
            return Err("正在取消旧任务，请稍候重试。".into());
        }
        store.save_download_task(&task).map_err(|e| e.to_string())?;
        let control = DownloadControl::new();
        jobs.insert(
            id.into(),
            ActiveJob {
                control: control.clone(),
                task: task.clone(),
                last_saved: Instant::now(),
            },
        );
        let _ = app.emit(
            "installation-task",
            TaskEvent {
                instance_id: id.into(),
                task,
            },
        );
        Ok(control)
    }
    pub fn progress(
        &self,
        id: &str,
        progress: &DownloadProgress,
        store: &LauncherStore,
        app: &AppHandle,
    ) {
        let Ok(mut jobs) = self.0.lock() else {
            return;
        };
        let Some(job) = jobs.get_mut(id) else {
            return;
        };
        // These are the current artifact's bytes, never a fabricated installation percentage.
        job.task.label = progress.label.clone();
        job.task.completed_bytes = progress.received;
        job.task.total_bytes = progress.total;
        job.task.source = Some(progress.source.clone());
        if job.last_saved.elapsed() >= Duration::from_millis(250) {
            job.task.updated_at = chrono::Utc::now().to_rfc3339();
            let _ = store.save_download_task(&job.task);
            let _ = app.emit(
                "installation-task",
                TaskEvent {
                    instance_id: id.into(),
                    task: job.task.clone(),
                },
            );
            job.last_saved = Instant::now();
        }
    }
    pub fn finish(
        &self,
        id: &str,
        success: bool,
        store: &LauncherStore,
        app: &AppHandle,
    ) -> Result<(), String> {
        let mut jobs = self.0.lock().map_err(|_| "下载任务状态不可用。")?;
        if let Some(mut job) = jobs.remove(id) {
            job.task.state = if success {
                DownloadTaskState::Completed
            } else if job.control.is_cancelled() {
                DownloadTaskState::Cancelled
            } else {
                DownloadTaskState::Failed
            };
            job.task.updated_at = chrono::Utc::now().to_rfc3339();
            store
                .save_download_task(&job.task)
                .map_err(|e| e.to_string())?;
            let _ = app.emit(
                "installation-task",
                TaskEvent {
                    instance_id: id.into(),
                    task: job.task,
                },
            );
        }
        Ok(())
    }
}

#[tauri::command]
pub fn list_download_tasks(store: State<'_, LauncherStore>) -> Result<Vec<DownloadTask>, String> {
    store.list_download_tasks().map_err(|e| e.to_string())
}
#[tauri::command]
pub fn control_download(
    instance_id: String,
    action: String,
    downloads: State<'_, Arc<Downloads>>,
    store: State<'_, LauncherStore>,
    app: AppHandle,
) -> Result<(), String> {
    let mut jobs = downloads.0.lock().map_err(|_| "下载任务状态不可用。")?;
    let job = jobs
        .get_mut(&instance_id)
        .ok_or("下载任务已结束；请使用安装按钮重试。")?;
    if job.control.is_cancelled() {
        return Err("正在取消，请等待任务结束。".into());
    }
    let state = match action.as_str() {
        "pause" => DownloadTaskState::Paused,
        "resume" => DownloadTaskState::Running,
        "cancel" => DownloadTaskState::Cancelled,
        _ => return Err("未知下载操作。".into()),
    };
    let mut task = job.task.clone();
    task.state = state;
    task.updated_at = chrono::Utc::now().to_rfc3339();
    store.save_download_task(&task).map_err(|e| e.to_string())?;
    match action.as_str() {
        "pause" => job.control.pause(),
        "resume" => job.control.resume(),
        "cancel" => job.control.cancel(),
        _ => unreachable!(),
    }
    job.task = task.clone();
    let _ = app.emit("installation-task", TaskEvent { instance_id, task });
    Ok(())
}
