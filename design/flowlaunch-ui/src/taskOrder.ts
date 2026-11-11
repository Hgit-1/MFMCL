// 下载任务列表的纯函数：排序与状态合并。
//
// 单独一个模块是为了能在 Node 里直接测：这些规则决定「任务卡显示什么状态」，
// 而 App 里的实现依赖 React 与后端事件，无法在回归脚本中直接驱动。

import type { DownloadTask } from '../types';

/** 仍在进行中的任务状态。已完成/失败/取消的任务不再计入侧栏徽章。 */
export const ACTIVE_TASK_STATUSES: DownloadTask['status'][] = ['queued', 'downloading', 'verifying', 'installing'];
export const FINISHED_TASK_STATUSES: DownloadTask['status'][] = ['completed', 'failed', 'cancelled'];

/**
 * 下载页的显示顺序：进行中 → 暂停 → 已结束。
 *
 * 后端只按 `updated_at` 排序，所以刚装完的任务会插到仍在跑的任务上方。列表里
 * 真正需要盯着的是还在动的那些，把已结束的压到下面才不会把进行中的挤出视野。
 * 同组内保持后端顺序（最近更新的在前）。
 */
export function taskRank(task: DownloadTask): number {
  if (ACTIVE_TASK_STATUSES.includes(task.status)) return 0;
  if (task.status === 'paused') return 1;
  return 2;
}

export function sortTasksForDisplay(tasks: DownloadTask[]): DownloadTask[] {
  return tasks
    .map((task, index) => ({ task, index }))
    .sort((a, b) => taskRank(a.task) - taskRank(b.task) || a.index - b.index)
    .map(entry => entry.task);
}

/**
 * 任务卡上要显示的状态。
 *
 * 后端任务只有 `running`，分不出「还在下」和「下完了在收尾」——Forge 的文件全部
 * 落盘之后，processor 还要打补丁、逐个比对哈希，这段时间任务记录仍然是 running。
 * 这里按文件计数补一个派生态：**计划里的文件全部下完、任务却还在跑**，就是校验
 * （或安装）阶段。
 *
 * ## 为什么判据不是「有文件在校验」
 *
 * 单文件哈希校验发生在每个文件写完的瞬间，而队列里通常还有几百个文件在下载。
 * 拿它当任务状态，卡片会在 86% 就写「校验中」并一直停在那里——文件明明还在下，
 * 标签却说在验。判据必须是**全部文件都下完**：先下载、后校验，这个顺序不能反。
 */
export function taskDisplayStatus(task: DownloadTask): DownloadTask['status'] {
  const allFilesDone = task.files != null
    && task.files > 0
    && (task.completedFiles ?? 0) >= task.files;
  return task.status === 'downloading' && allFilesDone ? 'verifying' : task.status;
}

/**
 * 把一条新的后端任务合并进列表。
 *
 * ## 为什么不能无条件相信「界面本地状态」
 *
 * 任务事件每 250ms 重写整条记录。如果界面自己缓存了一个「暂停」状态并坚持不放，
 * 用户点「继续」之后后端已经恢复下载，界面却永远停在「已暂停」——按一次恢复键
 * 没有任何反应，再按一次也没有。这正是用户报的「暂停后无法恢复」。
 *
 * 所以默认**后端说什么就是什么**。暂停与恢复都由后端在 `control_download` 里
 * 立刻写库并广播。
 *
 * ## 唯一需要界面撑住的窗口
 *
 * 用户刚点「暂停」、后端的 `paused` 事件还在路上时，会先收到几条在途的 `running`
 * 事件。不管它们的话，任务卡会在「已暂停」和「下载中」之间闪一下。
 *
 * `pendingPause` 显式表达这个窗口（值是被点下暂停的时间戳），而不是靠状态残留
 * 去猜：窗口一过，或者后端明确说 `paused`，就立刻回到「以后端为准」。
 */
export function mergeTaskUpdate(
  tasks: DownloadTask[],
  task: DownloadTask,
  pendingPause?: { taskId: string; at: number } | null,
  now: number = Date.now(),
): DownloadTask[] {
  const next = tasks.filter(item => item.id !== task.id);
  /*
   * 用户刚点暂停后的 1.5 秒内，忽略在途的 `running` 事件。
   *
   * 取 1.5 秒而不是更长：后端把 `paused` 写库并广播通常只要几十毫秒，这个窗口
   * 只是为了盖住已经在管道里的那几条。窗口越长，用户点「继续」时被旧状态盖住的
   * 风险越大——而那正是要修的病。
   */
  const suppressRunning = pendingPause?.taskId === task.id
    && now - pendingPause.at < 1500
    && task.status === 'downloading';
  if (suppressRunning) {
    const previous = tasks.find(item => item.id === task.id);
    return [previous ? { ...task, status: 'paused' } : task, ...next];
  }
  return [task, ...next];
}
