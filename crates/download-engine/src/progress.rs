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
    /// 未登记计划时，仅在收到文件事件后保留单文件任务的分母。
    observed_file: bool,
    /// 是否出现过大小未知的文件。一旦为真，`planned_bytes` 永远是 `None`。
    unknown_bytes: bool,
    finished_files: u32,
    finished_bytes: u64,
    /// 已经计入 `finished_files`/`finished_bytes` 的 artifact。用于避免同一文件
    /// 被重复上报时累加两次。
    finished: std::collections::HashSet<String>,
    /// artifact_id → 当前已接收字节。
    inflight: HashMap<String, u64>,
    /// artifact_id → 声明大小。已完成文件也留在这里，用于推断总量。
    sizes: HashMap<String, u64>,
}

impl Inner {
    fn snapshot(&self) -> TaskProgressSnapshot {
        let inflight_bytes: u64 = self.inflight.values().copied().sum();
        let completed_bytes = self.finished_bytes.saturating_add(inflight_bytes);
        /*
         * 分母的来源，按可信度排序：
         *
         *   1. 安装器登记的计划总量——最准，且从第一个字节起就稳定；
         *   2. 见过的文件大小之和——同期任务里没有登记计划时用（单文件下载、
         *      散装模组安装）；
         *   3. 都没有 → `None`，界面画不确定态。
         *
         * 只有「见过的文件数 >= 计划文件数」时才允许用 2：计划有 300 个文件却只
         * 见过 5 个大小，用这 5 个当分母会让进度条一开始就冲到 90%。
         */
        let seen = self.sizes.len() as u32;
        let total_bytes = match self.planned_bytes {
            Some(bytes) => Some(bytes),
            None if !self.unknown_bytes
                && !self.sizes.is_empty()
                && seen >= self.planned_files.max(1) =>
            {
                Some(self.sizes.values().copied().sum())
            }
            None => None,
        };
        TaskProgressSnapshot {
            files: self
                .planned_files
                .max(seen)
                .max(u32::from(self.observed_file)),
            completed_files: self.finished_files.min(self.planned_files.max(seen)),
            completed_bytes: total_bytes
                .map(|bytes| completed_bytes.min(bytes))
                .unwrap_or(completed_bytes),
            total_bytes,
        }
    }
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

    /// 读取共享累计器的当前值，不改变在途文件或已完成文件。
    ///
    /// 并发生产者可能在累计后、发送前被抢占，导致事件快照乱序到达；生产者停止且
    /// 事件通道排空后，应以此处的当前值做最终刷新，而不是使用最后收到的事件快照。
    pub fn snapshot(&self) -> TaskProgressSnapshot {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .snapshot()
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
        inner.observed_file = true;
        if let Some(size) = total.filter(|size| *size > 0) {
            inner.sizes.insert(artifact_id.to_owned(), size);
        } else if finished && received > 0 {
            inner
                .sizes
                .entry(artifact_id.to_owned())
                .or_insert(received);
        }
        if failed {
            // 失败同时撤销在途字节和可能已存在的完成记录，重试不重复累计。
            inner.inflight.remove(artifact_id);
            if inner.finished.remove(artifact_id) {
                let counted = inner
                    .sizes
                    .get(artifact_id)
                    .copied()
                    .unwrap_or(received)
                    .max(received);
                inner.finished_bytes = inner.finished_bytes.saturating_sub(counted);
                inner.finished_files = inner.finished_files.saturating_sub(1);
            }
        } else if finished {
            let counted = inner
                .sizes
                .get(artifact_id)
                .copied()
                .unwrap_or(received)
                .max(received);
            inner.inflight.remove(artifact_id);
            /*
             * 同一个 artifact 只计一次。
             *
             * 资源索引按名称组织、对象按 hash 落盘，重复 hash 会生成多条指向同一
             * destination 的请求：第一条真正下载并上报 `Completed`，后面的走
             * 「已存在且校验通过」的跳过分支，也会上报一次 `Completed`。
             * 无条件累加会让 `completed_files` 超过唯一文件数，界面上的
             * 「x / 总数」和百分比就对不上。
             */
            if inner.finished.insert(artifact_id.to_owned()) {
                inner.finished_bytes = inner.finished_bytes.saturating_add(counted);
                inner.finished_files = inner.finished_files.saturating_add(1);
            }
        } else {
            inner.inflight.insert(artifact_id.to_owned(), received);
        }
        inner.snapshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_snapshot_has_no_files_or_total() {
        let progress = TaskProgress::new();
        let expected = TaskProgressSnapshot {
            files: 0,
            completed_files: 0,
            completed_bytes: 0,
            total_bytes: None,
        };
        assert_eq!(progress.snapshot(), expected);
        assert_eq!(progress.snapshot(), expected);
    }

    #[test]
    fn snapshot_matches_observe_for_known_batches() {
        let progress = TaskProgress::new();
        progress.register(2, Some(200));
        progress.register(1, Some(50));
        assert_eq!(
            progress.snapshot(),
            TaskProgressSnapshot {
                files: 3,
                completed_files: 0,
                completed_bytes: 0,
                total_bytes: Some(250),
            }
        );
        for (id, received, total, finished, failed) in [
            ("a", 40, Some(100), false, false),
            ("b", 100, Some(100), true, false),
            ("a", 0, Some(100), false, true),
            ("a", 100, Some(100), true, false),
            ("c", 60, Some(50), true, false),
        ] {
            let observed = progress.observe(id, received, total, finished, failed);
            assert_eq!(progress.snapshot(), observed);
        }
        assert_eq!(
            progress.snapshot(),
            TaskProgressSnapshot {
                files: 3,
                completed_files: 3,
                completed_bytes: 250,
                total_bytes: Some(250),
            }
        );
    }

    #[test]
    fn snapshot_matches_observe_with_an_unknown_batch() {
        let progress = TaskProgress::new();
        progress.register(1, Some(100));
        progress.register(1, None);
        progress.register(1, Some(50));
        assert_eq!(progress.snapshot().files, 3);
        assert_eq!(progress.snapshot().total_bytes, None);
        for (id, received, total, finished) in [
            ("a", 100, Some(100), true),
            ("b", 40, None, false),
            ("c", 50, Some(50), true),
            ("b", 60, None, true),
        ] {
            let observed = progress.observe(id, received, total, finished, false);
            assert_eq!(progress.snapshot(), observed);
        }
        assert_eq!(
            progress.snapshot(),
            TaskProgressSnapshot {
                files: 3,
                completed_files: 3,
                completed_bytes: 210,
                total_bytes: None,
            }
        );
    }

    #[test]
    fn snapshot_preserves_unplanned_observation_semantics() {
        for (received, total, finished, failed) in [
            (0, None, false, true),
            (0, None, false, false),
            (10, None, false, false),
            (10, Some(100), false, false),
            (100, None, true, false),
        ] {
            let progress = TaskProgress::new();
            let observed = progress.observe("a", received, total, finished, failed);
            assert_eq!(observed.files, 1);
            assert_eq!(progress.snapshot(), observed);
        }
    }

    #[test]
    fn snapshot_does_not_finish_verifying_files_or_change_inflight_bytes() {
        let progress = TaskProgress::new();
        progress.register(3, Some(300));
        progress.observe("finished", 100, Some(100), true, false);
        progress.observe("downloading", 40, Some(100), false, false);
        let observed = progress.observe("verifying", 100, Some(100), false, false);
        assert_eq!(observed.completed_files, 1);
        assert_eq!(observed.completed_bytes, 240);
        assert_eq!(progress.snapshot(), observed);
        assert_eq!(progress.snapshot(), observed);
        {
            let inner = progress.inner.lock().unwrap();
            assert_eq!(inner.finished_files, 1);
            assert_eq!(inner.finished_bytes, 100);
            assert_eq!(inner.finished.len(), 1);
            assert!(inner.finished.contains("finished"));
            assert_eq!(inner.inflight.len(), 2);
            assert_eq!(inner.inflight.get("verifying"), Some(&100));
            assert_eq!(inner.inflight.get("downloading"), Some(&40));
        }
        let failed = progress.observe("verifying", 100, Some(100), false, true);
        assert_eq!(failed.completed_files, 1);
        assert_eq!(failed.completed_bytes, 140);
        assert_eq!(progress.snapshot(), failed);
    }

    #[test]
    fn final_snapshot_is_current_after_older_event_snapshots_are_drained() {
        let progress = TaskProgress::new();
        progress.register(2, Some(200));
        let older = progress.observe("a", 20, Some(100), false, false);
        progress.observe("a", 100, Some(100), true, false);
        let latest = progress.observe("b", 100, Some(100), true, false);
        let (tx, rx) = std::sync::mpsc::channel();
        // observe 与发送之间可能被抢占，因此较旧快照可以最后入队。
        tx.send(latest).unwrap();
        tx.send(older).unwrap();
        drop(tx);
        let drained: Vec<_> = rx.into_iter().collect();
        assert_eq!(drained, vec![latest, older]);
        assert_ne!(drained.last().copied(), Some(latest));
        assert_eq!(progress.snapshot(), latest);
        assert_eq!(
            progress.snapshot(),
            TaskProgressSnapshot {
                files: 2,
                completed_files: 2,
                completed_bytes: 200,
                total_bytes: Some(200),
            }
        );
    }

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

    /*
     * 资源索引按名称组织、对象按 hash 落盘。1.21.1 的索引有 3913 个名称但只有
     * 3890 个唯一 hash：重复 hash 会生成多条指向同一 destination 的请求，后面的
     * 走「已存在且校验通过」的跳过分支，也会上报一次 Completed。
     *
     * 无条件累加会让 completed_files 超过唯一文件数，界面上的「x / 总数」和
     * 百分比就对不上——用户看到「文件都下完了，进度却停在 88%」。
     */
    #[test]
    fn a_repeated_completion_is_counted_once() {
        let progress = TaskProgress::new();
        progress.register(2, Some(200));
        progress.observe("a", 100, Some(100), true, false);
        // 同一个 artifact 因为重复 hash 又上报了一次完成。
        let snapshot = progress.observe("a", 100, Some(100), true, false);
        assert_eq!(snapshot.completed_files, 1);
        assert_eq!(snapshot.completed_bytes, 100);
        // 另一个文件完成后才应该是 2。
        let snapshot = progress.observe("b", 100, Some(100), true, false);
        assert_eq!(snapshot.completed_files, 2);
        assert_eq!(snapshot.completed_bytes, 200);
    }

    /// 失败后重试成功，仍然只计一次。
    #[test]
    fn a_failure_then_success_counts_the_file_once() {
        let progress = TaskProgress::new();
        progress.register(1, Some(100));
        progress.observe("a", 100, Some(100), true, false);
        progress.observe("a", 0, Some(100), false, true);
        let snapshot = progress.observe("a", 100, Some(100), true, false);
        assert_eq!(snapshot.completed_files, 1);
        assert_eq!(snapshot.completed_bytes, 100);
    }

    /// 字节收齐不代表完成；此处只覆盖 observe 的布尔契约，不验证 emit 的阶段映射。
    #[test]
    fn an_unfinished_file_is_not_counted_as_completed() {
        let progress = TaskProgress::new();
        progress.register(2, Some(200));
        // Verifying：字节齐了但还没过哈希，按「在途」上报。
        let snapshot = progress.observe("a", 100, Some(100), false, false);
        assert_eq!(snapshot.completed_files, 0);
        // 字节仍然计入进度（文件内容确实已经在盘上），只是不算「完成」。
        assert_eq!(snapshot.completed_bytes, 100);
        assert_eq!(snapshot.total_bytes, Some(200));
        // 校验通过后紧跟 Completed，这时才算完成。
        let snapshot = progress.observe("a", 100, Some(100), true, false);
        assert_eq!(snapshot.completed_files, 1);
        assert_eq!(snapshot.completed_bytes, 100);
    }

    /// 失败也能撤销之前已经计入的完成记录。
    #[test]
    fn a_failure_revokes_a_previous_completion() {
        let progress = TaskProgress::new();
        progress.register(2, Some(200));
        let snapshot = progress.observe("a", 100, Some(100), true, false);
        assert_eq!(snapshot.completed_files, 1);
        let snapshot = progress.observe("a", 100, Some(100), false, true);
        assert_eq!(snapshot.completed_files, 0);
        assert_eq!(snapshot.completed_bytes, 0);
        // 重试成功后重新计入，且只计一次。
        let snapshot = progress.observe("a", 100, Some(100), true, false);
        assert_eq!(snapshot.completed_files, 1);
        assert_eq!(snapshot.completed_bytes, 100);
    }

    #[test]
    fn an_unfinished_report_then_completion_does_not_double_count() {
        let progress = TaskProgress::new();
        progress.register(1, Some(100));
        progress.observe("a", 100, Some(100), false, false);
        let snapshot = progress.observe("a", 100, Some(100), true, false);
        assert_eq!(snapshot.completed_files, 1);
        assert_eq!(snapshot.completed_bytes, 100);
    }
}
