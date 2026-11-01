// 本地 AI 模型的预览数据。
//
// 浏览器里没有应用数据目录，也就没有真实文件可读。但「未下载」这一屏恰恰是最需要
// 调整外观的状态（提示文案、下载按钮、源列表）。所以这里提供一份 **未安装** 的
// 状态——与真机上用户第一次打开设置时看到的完全一致。
//
// 不提供「已下载」的示例：那会让预览显示一个真机上不存在的完成态，而且容易让人
// 以为文件真的存在。
//
// ## 为什么数据写在函数内部
//
// 生产构建要求产物里不出现任何示例数据（见 UI_WORKFLOW.md 的验收清单）。把
// 常量放在模块顶层时，只要有一个非死代码的调用方引用它，打包器就无法删除它——
// 实测 `/preview/models` 就因此留在了产物里。
//
// 现在每个数据块都在 `if (!PREVIEW_DATA) return ...` 之后就地构造：`PREVIEW_DATA`
// 在生产构建里是编译期 `false`，整个函数体折叠成常量返回，后面的字面量没有任何
// 引用者，打包器才能把它们连同字符串一起删掉。

import { PREVIEW_DATA } from './preview'

/** 预览里的模型状态；生产构建中不会到达这里。 */
export function previewModelStatus() {
  if (!PREVIEW_DATA) return null
  return {
    id: 'lfm3-1.7b-q4km',
    name: 'LFM3-1.7B (Q4_K_M)',
    fileName: 'LFM3-1.7B-Q4_K_M.gguf',
    path: '/preview/models/LFM3-1.7B-Q4_K_M.gguf',
    directory: '/preview/models',
    expectedBytes: 1_099_627_808,
    installed: false,
    bytesOnDisk: null,
    downloading: false,
    // 与托管方公布的 LFS 元数据一致：预览里显示的也是真实的期望值。
    sha256: '3793e1e6e2266919d6e015287221b41100175a98eddaba92e111aed3b05c93c4',
  }
}

/** 预览里的源列表：标注为「无法测速」，绝不伪造速度数字。 */
export function previewModelSources() {
  if (!PREVIEW_DATA) return []
  return [
    {
      label: 'hf-mirror.com',
      url: 'https://hf-mirror.com/LiquidAI/LFM3-1.7B-GGUF/resolve/main/LFM3-1.7B-Q4_K_M.gguf',
      bytesPerSecond: null,
      problem: '浏览器预览无法测速',
    },
    {
      label: 'ModelScope',
      url: 'https://www.modelscope.cn/models/LiquidAI/LFM3-1.7B-GGUF/resolve/master/LFM3-1.7B-Q4_K_M.gguf',
      bytesPerSecond: null,
      problem: '浏览器预览无法测速',
    },
  ]
}
