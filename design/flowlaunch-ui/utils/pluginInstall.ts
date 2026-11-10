// 插件包与插件源的桥接层。
//
// ## 为什么单独一个模块，而不并进 `plugins.ts`
//
// `plugins.ts` 是插件的**数据模型**：清单长什么样、怎么归一化、怎么持久化。
// 这一层是**渠道**：从磁盘装一个包、从 URL 装一个包、拉一个源索引、让后端重新
// 对账。两者的依赖方向不同——模型不知道渠道存在，渠道要把模型喂给 Rust——混在
// 一起会让「我改了一个内置插件」和「我改了一条网络策略」变成同一次改动。
//
// ## 预览模式的行为
//
// 浏览器预览没有主进程：没有插件目录、没有 ZIP 解压、没有 asset 协议。所以这里
// 每个函数都会先问 `isDesktopShell()`，在预览里返回空列表或抛出可读的错误，而
// **不是**伪造一个「安装成功」。伪造会让设计评审看到一个不存在的功能，那正是
// 本仓库反复记下的那类失败（见 PLUGIN.md 的准入规则）。
//
// ## 安全边界读一遍
//
// 后端（`src-tauri/src/plugin_package.rs`）负责：ZIP 路径穿越、绝对路径、保留
// 设备名、控制字符、解压炸弹（条目数/单条大小/总量/压缩比）、重复条目、符号
// 链接位、清单大小。这一层负责：让用户**看见**它装了哪些权限、来自哪里、有没有
// 被沙箱化。两边合起来才是完整的说法——单独任何一半都会夸大安全性。

import { invoke } from '@tauri-apps/api/core';
import { isDesktopShell, NativeUnavailableError } from './native';

/**
 * 需要桌面后端，否则抛出与 `native.ts` 同一个错误类型。
 *
 * 复制的理由只有一个：`native.ts` 没导出它的 `requireDesktop`，而这里需要
 * 与它**完全相同**的错误，而不是一句相似的话——设置页与插件页同时弹错误时，
 * 用户应该看到两种操作都指向同一个原因（「需要桌面应用后端」）。
 */
function requireDesktop(operation: string): void {
  if (!isDesktopShell()) throw new NativeUnavailableError(operation);
}

/** 一个已安装插件包的后端记录。 */
export interface InstalledPluginRecord {
  id: string;
  version: string;
  /** 插件根下的目录名，等于 id。 */
  directory: string;
  /** 原始清单 JSON，交给 `plugins.ts::normalizePlugin` 解释。 */
  manifest: unknown;
  /** 包目录的绝对路径，供「打开文件夹」使用。 */
  path: string;
  /**
   * 该插件素材的可访问前缀（`asset://localhost/...` 或 `http://asset.localhost/...`）。
   *
   * 由后端按平台拼好，前端不要自己构造：两个平台的 scheme 不同，而且路径里可能
   * 有空格或非 ASCII 字符（Windows 用户目录形如 `C:\Users\张三`），编码规则只有
   * 一处才不会分叉。
   */
  assetBase: string;
}

/** 插件源索引里的一条插件。 */
export interface PluginIndexEntry {
  id: string;
  name: string;
  description: string;
  author: string;
  version: string;
  /** 包归档的 HTTPS 地址。 */
  download: string;
  homepage?: string | null;
  /**
   * 包的 SHA-256（小写十六进制）。
   *
   * 索引与哈希来自同一份文档，所以它防不住「整份索引被篡改」；它防的是更常见的
   * 那几种：release 资产被替换、URL 现在指向了新构建、代理塞回了一个 HTML 错误页。
   * 这几种都会让字节对不上，安装会拒绝而不是解压一个来路不明的东西。
   */
  sha256?: string | null;
  permissions: string[];
  hooks: string[];
  /** 低于这个启动器版本时界面会标为不兼容。仅供参考，不阻止安装。 */
  minLauncherVersion?: string | null;
}

/** 一个插件源（`index.json`）的内容。 */
export interface PluginSourceIndex {
  schema: number;
  name: string;
  plugins: PluginIndexEntry[];
}

/**
 * 已添加的插件源。
 *
 * 只存 URL 与用户给它的显示名；索引内容每次刷新都重新拉。缓存索引会得到
 * 「源更新了但启动器还装着旧版本」的状态，而这类不一致在插件系统里就是装错东西。
 */
export interface PluginSource {
  url: string;
  name: string;
  /** 用户自己加的源。内置源不在这个列表里。 */
  custom: boolean;
}

export const PLUGIN_SOURCES_KEY = 'mfmcl-plugin-sources';

/**
 * 内置源。
 *
 * ## 为什么内置源也走同一个 `index.json` 格式
 *
 * 它本可以写死在代码里，省掉一次网络请求。但它不写死有一个实际好处：内置源的
 * 内容改动不需要发启动器版本。插件列表会长期变化，把「新增一个插件」和「发一个
 * 启动器更新」绑在一起，等于让列表永远落后于实际。
 *
 * ## 源的信任
 *
 * 内置源由本项目维护，但它交付的仍然是**第三方代码**。所以内置源不会改变它的
 * 条目在界面上的标记：来自源的插件一律标为第三方、一律展示权限、一律在安装前
 * 弹确认。源的「内置」只表示「这个 URL 出厂时就填好了」，不表示「它给的插件可以
 * 少问一句」。
 */
export const BUILTIN_PLUGIN_SOURCE_URL =
  'https://raw.githubusercontent.com/Hgit-1/MFMCL-Plugins/main/index.json';

/** 支持的包扩展名。与后端 `plugin_install_file` 的白名单一致。 */
export const PLUGIN_PACKAGE_EXTENSIONS = ['.zip', '.mflp', '.mfplugin'] as const;

export function isPluginPackage(path: string): boolean {
  const lower = path.toLowerCase();
  return PLUGIN_PACKAGE_EXTENSIONS.some(extension => lower.endsWith(extension));
}

// ---------------------------------------------------------------------------
// 已安装
// ---------------------------------------------------------------------------

export async function listInstalledPlugins(): Promise<InstalledPluginRecord[]> {
  if (!isDesktopShell()) return [];
  return invoke<InstalledPluginRecord[]>('plugin_list_installed');
}

/**
 * 重新读取插件目录，并对账后端状态。
 *
 * 后端在同一次调用里做两件事：重新扫描磁盘、把已经不存在的插件的启用状态与贡献
 * 清掉。分成两次调用会出现「列表刷新了、启动链路还在用旧贡献」的中间态。
 */
export async function reloadPlugins(): Promise<InstalledPluginRecord[]> {
  if (!isDesktopShell()) return [];
  return invoke<InstalledPluginRecord[]>('plugin_reload');
}

export async function installPluginFromFile(path: string): Promise<InstalledPluginRecord> {
  requireDesktop('安装插件包');
  return invoke<InstalledPluginRecord>('plugin_install_file', { path });
}

export async function installPluginFromUrl(
  url: string,
  expectedSha256?: string | null,
): Promise<InstalledPluginRecord> {
  requireDesktop('从网址安装插件');
  return invoke<InstalledPluginRecord>('plugin_install_url', {
    url,
    // 显式传 null 而不是省略：后端用 `Option<String>` 接收，省略与 null 在
    // serde 里都映射到 None，但写出来能让读代码的人一眼看到「没有钉哈希」。
    expectedSha256: expectedSha256 ?? null,
  });
}

export async function removeInstalledPlugin(id: string): Promise<void> {
  requireDesktop('移除插件');
  await invoke('plugin_remove_installed', { id });
}

export async function pluginDirectory(): Promise<string> {
  requireDesktop('查看插件目录');
  return invoke<string>('plugin_directory');
}

// ---------------------------------------------------------------------------
// 插件源
// ---------------------------------------------------------------------------

export async function fetchPluginIndex(url: string): Promise<PluginSourceIndex> {
  requireDesktop('读取插件源');
  return invoke<PluginSourceIndex>('plugin_fetch_index', { url });
}

/**
 * 读取用户添加的源。
 *
 * 逐字段校验的理由与 `plugins.ts::loadPluginSet` 相同：这个对象会随版本变化，
 * 而一个 `undefined` 的 `custom` 会让界面把「全部源」显示成「没有源」。
 */
export function readPluginSources(): PluginSource[] {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(PLUGIN_SOURCES_KEY) || '[]');
    if (!Array.isArray(parsed)) return [];
    return parsed.flatMap(entry => {
      if (!entry || typeof entry !== 'object') return [];
      const value = entry as Partial<PluginSource>;
      if (typeof value.url !== 'string' || typeof value.name !== 'string') return [];
      // 只接受 HTTPS：与后端的源校验同一条规则。这里先挡一次，是为了让用户在
      // 添加的那一刻就得到错误，而不是等到刷新时才失败。
      if (!value.url.startsWith('https://')) return [];
      return [{ url: value.url, name: value.name.slice(0, 64), custom: true }];
    });
  } catch {
    return [];
  }
}

export function savePluginSources(sources: PluginSource[]): void {
  try {
    localStorage.setItem(PLUGIN_SOURCES_KEY, JSON.stringify(sources));
  } catch {
    // 隐私模式下只在本次会话生效。
  }
}

/**
 * 校验用户输入的一个源地址。
 *
 * 返回错误文案而不是布尔值：添加源失败时用户需要知道为什么，而「格式不正确」对
 * 一个把 GitHub 网页地址粘进来的人毫无帮助。
 */
export function validateSourceUrl(raw: string): string | null {
  const text = raw.trim();
  if (!text) return '请输入插件源地址。';
  let parsed: URL;
  try {
    parsed = new URL(text);
  } catch {
    return '地址格式不正确；需要完整的 HTTPS 网址。';
  }
  if (parsed.protocol !== 'https:') {
    return '插件源必须使用 HTTPS，不能是 http 或其他协议。';
  }
  if (parsed.username || parsed.password) {
    return '地址里不能包含账号或密码。';
  }
  if (!parsed.pathname.toLowerCase().endsWith('.json')) {
    // GitHub 的 raw 地址以 .json 结尾；网页地址不是，直接指向仓库首页也不行。
    // 这不是安全规则，是可用性规则：与其让用户等等再看到「格式错误」，不如现在说。
    return '地址应指向源的 index.json 文件（以 .json 结尾）。';
  }
  return null;
}

/**
 * 把 GitHub 的仓库或网页地址转成 raw 索引地址。
 *
 * ## 为什么要做这件事
 *
 * 用户看到 README 里写着 `github.com/owner/repo`，粘过来的就是那个地址。要求他
 * 自己知道 `raw.githubusercontent.com/owner/repo/main/index.json` 是一种不必要的
 * 门槛，而且粘错的可能性很高（分支名、`/blob/` 段）。这里把能识别的几种形态都
 * 归一化，识别不了就原样返回，让 `validateSourceUrl` 去报错。
 */
export function normalizeSourceUrl(raw: string): string {
  const text = raw.trim();
  const repo = /^https?:\/\/github\.com\/([^/]+)\/([^/?#]+)\/?$/i.exec(text);
  if (repo) {
    return `https://raw.githubusercontent.com/${repo[1]}/${repo[2]}/main/index.json`;
  }
  const blob = /^https?:\/\/github\.com\/([^/]+)\/([^/]+)\/blob\/([^/]+)\/(.+)$/i.exec(text);
  if (blob) {
    return `https://raw.githubusercontent.com/${blob[1]}/${blob[2]}/${blob[3]}/${blob[4]}`;
  }
  return text;
}
