// 二次确认。
//
// ## 为什么不能直接用 `window.confirm`
//
// `tauri-plugin-dialog` 初始化时会**替换全局的 `window.alert` / `window.confirm`**
// （见 crate 内的 `init-iife.js`）：
//
//     window.confirm = async function (message) {
//       return "Ok" === await invoke("plugin:dialog|message", { ... })
//     }
//
// 这个替换没有同步的类型声明，TypeScript 仍认为它返回 `boolean`，运行时却是
// **Promise**。后果有两层：
//
//   1. `if (!window.confirm(...)) return` 的判断永远为假（Promise 是 truthy），
//      用户点「取消」也会继续执行——所有二次确认形同虚设。
//   2. 把它当参数传给后端时，Promise 被序列化成 `{}`，后端收到一个 map 而非
//      布尔，报 `invalid type: map, expected a boolean`。`launch_instance` 的
//      `offlineConfirmed` 就是这样失败的。
//
// 所以所有确认都必须走这里：它 await 插件的异步版本，拿到真正的布尔值。
// 同时 `src-tauri/capabilities/default.json` 必须授予 `dialog:allow-message`，
// 否则命令会被权限拒绝（表现为确认框根本不出现）。

import { confirm as dialogConfirm } from '@tauri-apps/plugin-dialog';

import { isDesktopShell } from './native';

/**
 * 显示一个确认框，返回用户是否点了「确定」。
 *
 * 三档回退：
 *   1. 桌面版 → 插件对话框（系统原生外观，与窗口一致）；
 *   2. 浏览器预览 → 原生 `window.confirm`（同步，浏览器里没有被替换）；
 *   3. 任何一步抛错 → `false`。
 *
 * 失败时返回 `false` 是唯一安全的方向：一个「没能确认」的破坏性操作必须中止，
 * 而不是被当成「用户同意」继续执行。
 */
export async function confirmAction(message: string): Promise<boolean> {
  if (!isDesktopShell()) {
    // 预览环境没有 Tauri，插件的 confirm 会因为缺少 IPC 抛错。
    return typeof window !== 'undefined' && typeof window.confirm === 'function'
      ? window.confirm(message)
      : false;
  }
  try {
    return await dialogConfirm(message, { title: 'MFMCL', kind: 'warning' });
  } catch {
    return false;
  }
}
