// 用户协议（EULA）的确认状态。
//
// ## 为什么需要单独记录，且带版本号
//
// PCL2 的做法值得照搬：`SystemEulaVersion` 是个整数，协议内容改动时把它加一，
// 所有用户都会被重新询问一次。只存「已同意」布尔值的版本无法区分「同意的是旧版
// 协议」和「同意的是当前协议」——而协议一旦改动，旧同意在法律上不再有效。
//
// ## 为什么存在 localStorage 而不是后端
//
// 这是「这台机器上的这个用户看过并同意了」的记录，与账户、实例都无关。放后端会
// 让它在多账户场景下互相覆盖，而放 localStorage 恰好对应它的实际语义。
//
// ## 拒绝之后怎么办
//
// 不静默继续，也不删除设置：只是每次启动都会再问一遍。删除数据对一个还没开始用的
// 人没有意义，而静默继续会让这次确认变成摆设。

const EULA_KEY = 'mfmcl-eula-version';

/**
 * 当前用户协议的版本号。
 *
 * 改动协议正文（`LegalNotice` 里的文案）时**必须**同步加一，否则已经同意过的
 * 用户不会重新看到新内容。
 */
export const CURRENT_EULA_VERSION = 1;

/** 用户是否已经同意当前版本的协议。读不到时按「未同意」处理。 */
export function hasAcceptedEula(): boolean {
  try {
    const raw = localStorage.getItem(EULA_KEY);
    if (!raw) return false;
    const accepted = Number(raw);
    return Number.isFinite(accepted) && accepted >= CURRENT_EULA_VERSION;
  } catch {
    return false;
  }
}

/** 记录同意。存版本号而不是时间戳：时间戳无法回答「同意的是哪一版」。 */
export function acceptEula(): void {
  try {
    localStorage.setItem(EULA_KEY, String(CURRENT_EULA_VERSION));
  } catch {
    // 存储不可用时本次会话继续；下次启动会再问一遍。
  }
}
