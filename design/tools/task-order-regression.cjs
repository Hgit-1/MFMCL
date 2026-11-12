// Run: node design/tools/task-order-regression.cjs
// 直接执行 App 用的纯函数，验证任务排序与「校验中」状态的保留规则。
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require('../flowlaunch-ui/node_modules/typescript');

const source = path.resolve(__dirname, '../flowlaunch-ui/src/taskOrder.ts');
const compiled = ts.transpileModule(fs.readFileSync(source, 'utf8'), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 },
}).outputText;

const module_ = { exports: {} };
vm.runInNewContext(compiled, {
  module: module_,
  exports: module_.exports,
  require: () => ({}),
  console,
});

const { ACTIVE_TASK_STATUSES, FINISHED_TASK_STATUSES, mergeTaskUpdate, sortTasksForDisplay, taskDisplayStatus } = module_.exports;

const task = (id, status, extra = {}) => ({
  id, name: id, type: 'game', status, progress: 0, ...extra,
});

/*
 * 模块在独立的 vm context 里执行，它返回的数组属于另一个 realm，
 * `deepStrictEqual` 会因为原型不同而失败。统一转成本地数组再比较。
 */
const ids = list => Array.from(list, item => item.id);
const statuses = list => Array.from(list, item => item.status);

// --- 排序 ---------------------------------------------------------------
assert.deepEqual(
  ids(sortTasksForDisplay([
    task('done', 'completed'),
    task('paused', 'paused'),
    task('running', 'downloading'),
    task('verifying', 'verifying'),
  ])),
  ['running', 'verifying', 'paused', 'done'],
  'active tasks must come first, then paused, then finished',
);

// 同组内保持后端顺序（最近更新的在前），不能因为排序把历史顺序打乱。
assert.deepEqual(
  ids(sortTasksForDisplay([
    task('done-new', 'failed'),
    task('done-old', 'cancelled'),
    task('active-a', 'queued'),
    task('active-b', 'installing'),
  ])),
  ['active-a', 'active-b', 'done-new', 'done-old'],
  'stable ordering inside each group',
);
console.log('PASS task ordering: active → paused → finished, stable within groups');

// --- 校验中状态 ---------------------------------------------------------
/*
 * 下载引擎的 `Verifying` 是单个文件写完后的哈希比对，不是任务级阶段。
 * 任务卡不能因为某个文件在校验就把整条任务标成「校验中」——那会让进度条
 * 停在 88% 却一直显示校验，与实际进度不符。
 */
assert.deepEqual(
  statuses(mergeTaskUpdate([task('t', 'verifying')], task('t', 'downloading'))),
  ['downloading'],
  'a per-file verifying event must not pin the whole task to verifying',
);
console.log('PASS per-file verifying does not become the task status');

/*
 * 暂停后**必须能恢复**。
 *
 * 之前界面把 `paused` 当成「界面已知状态」一直保留：用户点继续，后端已经恢复下载
 * 并广播 `running`，界面却继续显示已暂停——按一次没反应，再按一次也没反应。
 *
 * 现在以后端为准，所以「后端说 downloading」就必须显示 downloading。
 */
assert.deepEqual(
  statuses(mergeTaskUpdate([task('t', 'paused')], task('t', 'downloading'))),
  ['downloading'],
  'resume must take effect: the backend saying downloading wins over a stale paused row',
);
console.log('PASS resuming a paused task actually leaves the paused state');

/*
 * 但用户刚点暂停的那一瞬间，管道里还有在途的 `running` 事件。
 * 不管它们的话任务卡会闪一下「下载中」，所以用一个短暂的本地窗口盖住。
 */
const justPaused = { taskId: 't', at: 1000 };
assert.deepEqual(
  statuses(mergeTaskUpdate([task('t', 'paused')], task('t', 'downloading'), justPaused, 1200)),
  ['paused'],
  'in-flight running events right after the click must not flash the card back to downloading',
);
// 窗口过后必须回到「以后端为准」，否则暂停又会变成永久钉住。
assert.deepEqual(
  statuses(mergeTaskUpdate([task('t', 'paused')], task('t', 'downloading'), justPaused, 5000)),
  ['downloading'],
  'the suppression window must expire so resume can work',
);
// 后端明确回报 paused 之后，窗口立刻作废（由调用方清空 pendingPause）。
assert.deepEqual(
  statuses(mergeTaskUpdate([task('t', 'downloading')], task('t', 'paused'), justPaused, 1200)),
  ['paused'],
  'the backend confirming paused is always honoured',
);
console.log('PASS the pause suppression window is short and self-expiring');

// 已结束的任务可以重新开始：后端再次说 running 就是真的重新跑了，
// 不能让旧状态把它永久钉在「已完成」。
for (const status of ['completed', 'cancelled', 'failed']) {
  assert.deepEqual(
    statuses(mergeTaskUpdate([task('t', status)], task('t', 'downloading'))),
    ['downloading'],
    `${status} must yield when the backend starts the task again`,
  );
}
console.log('PASS finished tasks can restart when the backend reports running');

// 新任务插到最前，且不重复保留旧行。
const merged = mergeTaskUpdate([task('t', 'verifying'), task('other', 'downloading')], task('t', 'downloading'));
assert.deepEqual(ids(merged), ['t', 'other']);
assert.equal(merged.length, 2);
console.log('PASS task update replaces the existing row instead of duplicating it');

// 状态集合本身：verifying 算进行中，不会被当成已结束清掉。
assert.ok(ACTIVE_TASK_STATUSES.includes('verifying'));
assert.ok(!FINISHED_TASK_STATUSES.includes('verifying'));
console.log('PASS verifying counts as an active task');

// --- 校验中：必须等文件全部下完 -----------------------------------------
/*
 * 顺序不能反：Forge 安装先下 3963 个文件，全部落盘后才跑 processor、比对哈希。
 * 只要有文件还在下载，卡片就必须写「下载中」——哪怕此刻正好有个文件在验哈希。
 */
const withCounts = (status, done, total) => task('t', status, { files: total, completedFiles: done });

assert.equal(
  taskDisplayStatus(withCounts('downloading', 3550, 3963)),
  'downloading',
  'still downloading 413 files: must stay "downloading", not "verifying"',
);
assert.equal(
  taskDisplayStatus(withCounts('downloading', 3963, 3963)),
  'verifying',
  'all files on disk but the task still runs: that is the verifying phase',
);
console.log('PASS verifying only starts after every planned file is downloaded');

// 校验期间不能因为「还没结束」就退回下载中：文件数已满，状态要稳在 verifying。
assert.equal(taskDisplayStatus(withCounts('downloading', 3963, 3963)), 'verifying');
console.log('PASS verifying stays put while the task is still running');

// 没有文件计数时不能凭空猜：单文件任务和旧后端都走这条，保持后端状态。
assert.equal(taskDisplayStatus(task('t', 'downloading')), 'downloading');
assert.equal(taskDisplayStatus(withCounts('downloading', 0, 0)), 'downloading');
console.log('PASS without file counts the backend status is left alone');

// 已暂停/已完成的任务不该被派生成「校验中」。
assert.equal(taskDisplayStatus(withCounts('paused', 3963, 3963)), 'paused');
assert.equal(taskDisplayStatus(withCounts('completed', 3963, 3963)), 'completed');
assert.equal(taskDisplayStatus(withCounts('failed', 3963, 3963)), 'failed');
console.log('PASS paused, completed and failed tasks keep their own status');
