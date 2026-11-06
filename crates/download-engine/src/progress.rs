//! 一次安装任务的**整体**进度累计。
//!
//! ## 为什么需要它
//!
//! 单个文件的进度事件只能回答「这一个文件下了多少」。任务卡上如果直接拿最后一个
//! 文件的百分比画进度条，用户看到的是「正在下第 3 个库，进度 12%」——那不是安装
//! 进度，而且会随着文件切换来回跳。这里把整个任务的文件数、总字节和已完成字节
//! 一起累计，任务卡画的才是「这次安装走到哪了」。
//!
//! ## 数值的可信度
//!
//! - `total_bytes` 只有在**所有**参与的文件都声明了大小时才给出。计划里能算出总
//!   量时用计划值（`plan`），算不出来时按已见过的文件大小求和；只要有一个文件大小
//!   未知，就返回 `None`，界面显示不确定态而不是编一个数字。
//! - `completed_bytes` 是「已完成文件的字节 + 在途文件已接收的字节」，并被夹到
//!   `total_bytes` 以内。SHA 校验失败的段落会从在途表里移除，不会把重试前的
//!   字节重复计一次。
//!
//! 这个结构被同一次任务的所有引擎克隆共享（`Arc`），因此并发下载的多个文件各自
//! 上报时看到的是同一份累计值。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

/// 任务整体进度快照，随每条 `DownloadProgress` 一起发出。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskProgressSnapshot {
    /// 计划中的文件数。单文件任务为 1。
    pub files: u32,
    /// 已经下载并校验通过的文件数。
    pub completed_files: u32,
    /// 已完成 + 在途文件的累计已接收字节。
    pub completed_bytes: u64,
    /// 计划总量。任何参与的文件大小未知时为 `None`。
    pub total_bytes: Option<u64>,
}

#[derive(Debug, Default)]
struct Inner {
    planned_files: u32,
    planned_bytes: Option<u64>,
    /// 是否出现过大小未知的文件。一旦为真，`planned_bytes` 永远是 `None`。
    unknown_bytes: bool,
    finished_files: u32,
    finished_bytes: u64,
    /// artifact_id → 当前已接收字节。
    inflight: HashMap<String, u64>,
    /// artifact_id → 声明大小。已完成文件也留在这里，用于推断总量。
    sizes: HashMap<String, u64>,
}

/// 任务级累计器。用 `Arc` 在同一次任务的所有引擎克隆间共享。
#[derive(Debug, Default)]
pub struct TaskProgress {
    inner: Mutex<Inner>,
}

impl TaskProgress {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一批要下载的文件，让进度条从一开始就有正确的分母。
    ///
    /// 一次安装会分多批提交（版本清单里的库/客户端，然后是资源索引里的对象），所以
    /// 这个方法是**累加**的：分批调用会得到「全部文件的合计」，而不是最后一批。
    ///
    /// `total_bytes` 用 `Option`：任何一批里存在缺 size 的条目，整体总量就变成未知
    /// （`None`）且之后不能再被后续批次「修好」——因为缺失的那一批已经在里面了。
    /// 宁可让界面显示不确定态，也不要给出偏小的分母，那会让进度条停在 100% 之后
    /// 继续前进。
    pub fn register(&self, files: u32, total_bytes: Option<u64>) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        inner.planned_files = inner.planned_files.saturating_add(files);
        inner.planned_bytes = match (inner.planned_bytes, total_bytes) {
            // 已经确认有未知大小的文件：总量就是未知的，不再改回「已知」。
            (None, _) if inner.unknown_bytes => None,
            // 第一批就缺 size，或者上一批已经缺了：从此总量未知。
            (None, None) => {
                inner.unknown_bytes = true;
                None
            }
            (None, Some(bytes)) => Some(bytes),
            (Some(previous), Some(bytes)) => Some(previous.saturating_add(bytes)),
            (Some(_), None) => {
                inner.unknown_bytes = true;
                None
            }
        };
    }

    /// 记录一个文件的进度，返回累计后的快照。
    pub(crate) fn observe(
        &self,
        artifact_id: &str,
        received: u64,
        total: Option<u64>,
        finished: bool,
        failed: bool,
    ) -> TaskProgressSnapshot {
        let Ok(mut inner) = self.inner.lock() else {
            return TaskProgressSnapshot {
                files: 1,
                completed_files: 0,
                completed_bytes: received,
                total_bytes: total,
            };
        };
        if let Some(size) = total.filter(|size| *size > 0) {
            inner.sizes.insert(artifact_id.to_owned(), size);
        } else if finished && received > 0 {
            inner
                .sizes
                .entry(artifact_id.to_owned())
                .or_insert(received);
        }
        if failed {
            // 校验或传输失败的文件不计入已完成，也不留在在途表里过夜。
            inner.inflight.remove(artifact_id);
        } else if finished {
            let counted = inner
                .sizes
                .get(artifact_id)
                .copied()
                .unwrap_or(received)
                .max(received);
            inner.inflight.remove(artifact_id);
            inner.finished_bytes = inner.finished_bytes.saturating_add(counted);
            inner.finished_files = inner.finished_files.saturating_add(1);
        } else {
            inner.inflight.insert(artifact_id.to_owned(), received);
        }
        let inflight_bytes: u64 = inner.inflight.values().copied().sum();
        let completed_bytes = inner.finished_bytes.saturating_add(inflight_bytes);
        /*
         * 分母的来源，按可信度排序：
         *
         *   1. 安装器登记的计划总量——最准，且从第一个字节起就稳定；
         *   2. 见过的文件大小之和——同期任务里没有登记计划时用（单文件下载、
         *      散装模组安装）；
         *   3. 都没有 → `None`，界面画不确定态。
         *
         * 只有「见过的文件数 == 计划文件数」时才允许用 2：计划有 300 个文件却只
         * 见过 5 个大小，用这 5 个当分母会让进度条一开始就冲到 90%。
         */
        let seen = inner.sizes.len() as u32;
        let total_bytes = match inner.planned_bytes {
            Some(bytes) => Some(bytes),
            None if !inner.unknown_bytes
                && !inner.sizes.is_empty()
                && seen >= inner.planned_files.max(1) =>
            {
                Some(inner.sizes.values().copied().sum())
            }
            None => None,
        };
        TaskProgressSnapshot {
            files: inner.planned_files.max(seen).max(1),
            completed_files: inner.finished_files.min(inner.planned_files.max(seen)),
            completed_bytes: total_bytes
                .map(|bytes| completed_bytes.min(bytes))
                .unwrap_or(completed_bytes),
            total_bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planned_totals_stay_stable_while_files_report_out_of_order() {
        let progress = TaskProgress::new();
        progress.register(3, Some(300));
        // 三个文件乱序上报：总量必须一直是 300，而不是「已经见过的几个之和」。
        progress.observe("b", 40, Some(100), false, false);
        let snapshot = progress.observe("a", 10, Some(100), false, false);
        assert_eq!(snapshot.total_bytes, Some(300));
        assert_eq!(snapshot.completed_bytes, 50);
        let snapshot = progress.observe("a", 100, Some(100), true, false);
        assert_eq!(snapshot.completed_files, 1);
        assert_eq!(snapshot.completed_bytes, 140);
        assert_eq!(snapshot.total_bytes, Some(300));
    }

    #[test]
    fn batched_registration_accumulates_instead_of_overwriting() {
        let progress = TaskProgress::new();
        // 原版安装分两批：库/客户端，然后是资源对象。
        progress.register(2, Some(200));
        progress.register(3, Some(300));
        let snapshot = progress.observe("a", 10, Some(100), false, false);
        assert_eq!(snapshot.files, 5);
        assert_eq!(snapshot.total_bytes, Some(500));
    }

    #[test]
    fn an_unknown_batch_keeps_the_total_unknown_for_good() {
        let progress = TaskProgress::new();
        progress.register(2, Some(200));
        progress.register(1, None);
        // 后续批次即使大小齐全，也不能把「有未知文件」这件事抹掉。
        progress.register(1, Some(50));
        let snapshot = progress.observe("a", 10, Some(100), false, false);
        assert_eq!(snapshot.total_bytes, None);
        assert_eq!(snapshot.files, 4);
    }

    #[test]
    fn a_failed_artifact_does_not_double_count_its_retry() {
        let progress = TaskProgress::new();
        progress.register(1, Some(100));
        progress.observe("a", 90, Some(100), false, false);
        // 校验失败：在途字节作废，重试从 0 重新计。
        progress.observe("a", 0, Some(100), false, true);
        let snapshot = progress.observe("a", 20, Some(100), false, false);
        assert_eq!(snapshot.completed_bytes, 20);
        assert_eq!(snapshot.completed_files, 0);
    }

    #[test]
    fn unknown_size_keeps_the_total_unknown_instead_of_guessing() {
        let progress = TaskProgress::new();
        progress.register(2, None);
        let snapshot = progress.observe("a", 10, Some(10), true, false);
        // 只见过 1/2 个文件的大小：不许把这 10 字节当成总量。
        assert_eq!(snapshot.total_bytes, None);
        assert_eq!(snapshot.completed_bytes, 10);
        let snapshot = progress.observe("b", 5, None, false, false);
        assert_eq!(snapshot.total_bytes, None);
        assert_eq!(snapshot.completed_bytes, 15);
    }

    #[test]
    fn single_file_tasks_infer_the_total_from_the_reported_size() {
        let progress = TaskProgress::new();
        let snapshot = progress.observe("mod", 512, Some(2048), false, false);
        assert_eq!(snapshot.files, 1);
        assert_eq!(snapshot.total_bytes, Some(2048));
        assert_eq!(snapshot.completed_bytes, 512);
    }
}
