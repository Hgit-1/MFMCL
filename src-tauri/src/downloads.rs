//! Installation-scoped controls and persistent, truthful task status.
use download_engine::{DownloadControl, DownloadProgress, TaskProgress};
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

/// `download-progress` 事件载荷。
///
/// ## 为什么要在单文件进度上附带任务标识
///
/// 引擎只知道「哪个文件下到多少」，不知道自己在为哪个任务服务——那层映射在
/// `Downloads` 里（按实例 id 索引）。界面要把文件行归到对应的任务卡片下，就必须
/// 知道这条进度属于谁。
///
/// 加两个字段而不是改引擎：`DownloadProgress` 是下载引擎的通用类型，让它携带
/// 界面概念（任务 id、任务标题）会把两层耦合起来。这里包装一层即可。
///
/// `task_id` 是**任务记录 id**（`DownloadTask::id`），不是实例 id：同一个实例
/// 可以先后有不同的任务，按实例分组会把它们的文件混成一份。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressEvent {
    pub task_id: String,
    pub instance_id: String,
    #[serde(flatten)]
    pub progress: DownloadProgress,
}

/// 一次任务需要交给下载引擎的两样东西。
///
/// 为什么合成一个返回值：控制句柄和进度累计器必须属于**同一个任务**。以前只返回
/// 控制句柄，每个引擎克隆各自新建累计器，结果任务卡上的进度永远只是「最后一个
/// 文件的进度」。
pub struct TaskHandle {
    pub control: DownloadControl,
    /// 整次任务共用的累计器，交给 `DownloadEngine::with_task_progress`。
    pub progress: Arc<TaskProgress>,
}

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
    ) -> Result<TaskHandle, String> {
        let now = chrono::Utc::now().to_rfc3339();
        let task = DownloadTask {
            id: uuid::Uuid::new_v4().to_string(),
            instance_id: id.into(),
            /*
             * 任务名称只在 `start` 时确定。
             *
             * 它描述的是「用户在装什么」（实例名、整合包名、模组全名），不是某个
             * 具体文件。以前每来一条文件进度就把它覆盖成文件名，于是任务卡标题在
             * 安装过程中不断变成 `gson-2.10.1.jar` 之类的路径片段——那既是错的
             * 信息层级，也让用户认不出自己点的是哪个任务。
             */
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
        let mut jobs = self.0.lock().map_err(|_| "下载任务状态不可用。")?;
        if let Some(existing) = jobs.get(id) {
            if !existing.control.is_cancelled() {
                return Err("此实例已有下载任务。".into());
            }
            return Err("正在取消旧任务，请稍候重试。".into());
        }
        store.save_download_task(&task).map_err(|e| e.to_string())?;
        let control = DownloadControl::new();
        let progress = Arc::new(TaskProgress::new());
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
        Ok(TaskHandle { control, progress })
    }
    pub fn progress(
        &self,
        id: &str,
        progress: &DownloadProgress,
        store: &LauncherStore,
        app: &AppHandle,
    ) -> Option<String> {
        let Ok(mut jobs) = self.0.lock() else {
            return None;
        };
        let Some(job) = jobs.get_mut(id) else {
            return None;
        };
        /*
         * 写进任务行的必须是**整体**进度，不是这一条事件里那个文件的进度。
         *
         * `progress.received / progress.total` 是当前文件自己的计量：拿它当任务
         * 进度，用户会看到进度条随着文件切换从 100% 跳回 3%。`progress.task`
         * 是下载引擎对整次任务的累计（文件数、已完成字节、总量），只有它才是
         * 「这次安装走到哪了」。
         *
         * 拿不到累计值时（旧引擎、或调用方没接 `with_task_progress`）保留上一次
         * 已知的数值，而不是用一个更小的单文件值把进度条往回拨。
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
            let _ = app.emit(
                "installation-task",
                TaskEvent {
                    instance_id: id.into(),
                    task: job.task.clone(),
                },
            );
            job.last_saved = Instant::now();
        }
        /*
         * 把任务记录 id 交回调用方，供 `ProgressEvent` 标注归属。
         *
         * 界面按它把文件行挂到对应的任务卡片下；没有它就只能全局混排，而一次
         * 并发安装里两个任务的文件会互相穿插，看起来像同一个任务在下别人的文件。
         */
        Some(job.task.id.clone())
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

/// 清掉已结束的任务记录。
///
/// 正在进行和已暂停的任务不动：前者还在写文件，后者可以继续。只返回删除的行数，
/// 界面据此提示具体清掉了几条——「已清理」这种没有数量的说法，用户无法判断
/// 是不是真的生效了。
#[tauri::command]
pub fn clear_finished_download_tasks(store: State<'_, LauncherStore>) -> Result<usize, String> {
    store
        .clear_finished_download_tasks()
        .map_err(|e| e.to_string())
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
