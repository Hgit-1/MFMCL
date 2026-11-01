// 首次引导的完成状态。
//
// ## 为什么单独存一个标记
//
// 引导问的都是「设置」，而设置本身已经各有归宿：主题在 `theme` 键、强调色与
// 内存在后端 `LauncherSettings`、语言在 `flowlaunch-language`、Java 在
// `flowlaunch-java-runtimes`。但它们都不回答「这个人走完引导没有」——一个把
// 主题手动改成深色的新用户，和一个在引导里选了深色的用户，从这里看不出区别。
//
// 所以只记一个「引导已完成」的时间戳。它不改动任何既有设置，只是决定下次启动
// 还要不要挡住主界面。
//
// ## 为什么允许重跑
//
// `resetOnboarding` 给「设置 → 高级 → 重新运行引导」用。重跑时**不重置**已经
// 选好的值：那些值就在各自的存储里，引导只是把它们再展示一遍。把它做成「恢复
// 默认」会让一个只想复习设置的用户意外丢掉自己的配置。

const ONBOARDING_KEY = 'mfmcl-onboarding';

/** 引导是否还没走完。存档损坏或读不到时按「未完成」处理（宁可多问一次）。 */
export function isOnboardingPending(): boolean {
  try {
    const raw = localStorage.getItem(ONBOARDING_KEY);
    if (!raw) return true;
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== 'object') return true;
    const record = parsed as { completedAt?: unknown };
    return typeof record.completedAt !== 'number';
  } catch {
    return true;
  }
}

/** 记录完成时间。时间戳本身没有界面用途，但排查「为什么又弹出来了」时很有用。 */
export function markOnboardingComplete(): void {
  try {
    localStorage.setItem(ONBOARDING_KEY, JSON.stringify({ completedAt: Date.now() }));
  } catch {
    // 存储不可用时本次会话仍算完成；下次启动会再问一遍，代价只是多几步点击。
  }
}

/** 让引导下次启动时重新出现（「重新运行引导」）。 */
export function resetOnboarding(): void {
  try {
    localStorage.removeItem(ONBOARDING_KEY);
  } catch {
    // 同上：删不掉也不会让当前这次重跑失败，调用方紧接着就会显示引导。
  }
}
