// 启动器插件目录与本地状态。
//
// 这一层只处理「有哪些插件、它们声明了什么、用户开关成什么样」，不执行任何
// 插件代码。真实实现里插件运行在启动器主进程，界面通过 IPC 拿到清单和状态；
// 网页预览没有主进程，所以这里用一份内置清单 + localStorage 模拟同一套状态。
//
// 为什么把「扩展点」和「权限」显式建模：
//   用户判断一个插件能不能信任，靠的不是描述文字，而是「它会碰到什么」。
//   所以每个插件必须列出它接入的扩展点（何时被调用）和需要的权限（能访问
//   什么），并且这两项在详情面板里逐条解释，而不是只塞一个图标。
//
// 数据形状：
//   - 文案用 Localized（词典键 + 参数），切换语言后立即跟着变；
//   - 用户状态只有三样：启用与否、每个插件的选项值、手动安装的插件清单；
//   - 全部可 JSON 序列化，存档损坏时逐字段丢弃而不是整体崩溃。

import type { MessageKey } from './i18n/messages/zh-CN';
import { loc, type Localized } from './i18n/localized';
import type { PluginContribution, PluginDefinition, PluginStatePayload } from './native';

export const PLUGIN_STORAGE_KEY = 'flowlaunch-plugins';

/** 插件接入的扩展点。真实执行时对应生命周期钩子。 */
export type PluginHookId =
  | 'launch'
  | 'modInstall'
  | 'instanceCreate'
  | 'theme'
  | 'sidebar'
  | 'background';

/** 插件需要的权限。逐条展示给用户，避免「装了就全权限」的默认信任。 */
export type PluginPermissionId =
  | 'filesystem'
  | 'network'
  | 'launchHook'
  | 'theme'
  | 'uiSlot'
  | 'storage';

/** 插件图标形状。避免直接存 SVG：清单来自外部文件，不能让它注入标记。 */
export type PluginGlyph =
  | 'scroll'
  | 'archive'
  | 'shield'
  | 'palette'
  | 'gauge'
  | 'network'
  | 'image'
  | 'plug';

/**
 * 插件选项。
 *
 * 只支持开关与整数两种：插件配置越自由，界面越难保持一致的密度与可达性。
 * 需要复杂配置的插件应该自己做设置页，而不是把表单塞进启动器的详情面板。
 */
export interface PluginOption {
  id: string;
  type: 'toggle' | 'number';
  label: Localized;
  hint?: Localized;
  /** type === 'toggle' 时的默认值。 */
  defaultOn?: boolean;
  /** type === 'number' 时的默认值与范围。 */
  defaultNumber?: number;
  min?: number;
  max?: number;
}

export interface LauncherPlugin {
  id: string;
  name: Localized;
  version: string;
  author: string;
  description: Localized;
  /** 卡片图标。 */
  glyph: PluginGlyph;
  /** 图标底色，取自插件自己的品牌色。 */
  accent: string;
  hooks: PluginHookId[];
  permissions: PluginPermissionId[];
  options?: PluginOption[];
  /** 首次安装后默认是否启用。 */
  enabledByDefault: boolean;
  /** 内置插件不能被移除；手动安装的可以。 */
  builtin: boolean;
  /** 旧版展示字段；外部清单不能声明，第三方包始终可以移除。 */
  bundled?: boolean;
  /**
   * 第三方来源标记：从插件包或插件源安装的插件。
   *
   * 与 `builtin` 不互斥到只剩一半——`builtin: false` 只说明「不是启动器自带的」，
   * 而手动粘贴的清单同样不是自带。这个字段专门标「代码来自外部、启动器没有审查
   * 过」，界面据此展示第三方提示与权限清单。清单不能自己声明它为 `false`。
   */
  thirdParty?: boolean;
  /** 仅由后端的已安装包记录提供，指向磁盘包根目录，不接受清单中的 URL。 */
  assetBase?: string;
  /**
   * 计划中的插件：清单已公布，但启动器还没有实现对应能力。
   *
   * ## 为什么要有这个状态
   *
   * 插件页的一条硬规矩是「开关必须真的改变行为」（见 `BUILTIN_PLUGINS` 的注释）。
   * 但有些插件值得提前让用户知道存在——它们是路线图的一部分，用户会想知道
   * 「以后会不会有」。把它们直接混进可启用列表，就等于让用户拨一个什么都不做
   * 的开关，那正是这条规矩要防的事。
   *
   * 所以单独建模：卡片照常显示身份、用途、将来会碰到的扩展点与权限，但
   *   - 开关恒为禁用且恒显示「未启用」；
   *   - 不计入「已安装 / 已启用」数量；
   *   - 不参与状态持久化与后端同步（`toPluginStatePayload` 不发送它）；
   *   - 详情里用一段专门的说明交代「计划做什么、现在有什么」。
   *
   * 外部清单无法自行声明这个字段：它也像 `builtin` / `incompatible` 一样由启动器
   * 判定，否则任何人都能装一个永远「只需等待」的假插件。
   */
  planned?: boolean;
  /** 需要重启启动器才真正加载/卸载。 */
  requiresRestart?: boolean;
  /** 需要注意的状态说明，例如实验性或版本不兼容。 */
  status?: Localized;
  statusTone?: 'info' | 'warn' | 'error';
  /** 与当前启动器版本不兼容：开关置灰，也不计入已启用数量。 */
  incompatible?: boolean;
  /**
   * 有新版本可用。
   *
   * 为什么单独建模而不是复用 `statusTone: 'warn'`：侧栏徽章需要区分「需要更新」
   * 与其他「需要注意」。前者是可执行的待办（装个新版本就好），后者（实验性提示、
   * 不兼容、待重启）性质不同，用户看到红色数字时的预期也不同。
   *
   * 当前没有在线插件源，这个字段由内置清单或本地清单自行声明；接入更新检查后
   * 会由检查结果填充。
   */
  updateAvailable?: string;
  /**
   * 插件声明要贡献的动作。
   *
   * 这是「插件能不能真的做事」的入口：只有这里的条目会进入启动链路。内置插件的
   * 贡献定义在后端（`src-tauri/src/plugin_runtime.rs`），用户安装的插件可以把
   * 它们写在清单里——后端会逐条校验，非法的整份拒绝保存。
   *
   * 界面不执行这些条目，所以这里只做「有/没有」的展示（例如详情里列出会应用几条）。
   */
  contributions?: PluginContribution[];
}

/** 保存在 localStorage 里的全部插件状态。 */
export interface PluginStore {
  /** 用户手动安装的插件（内置清单之外的部分）。 */
  custom: LauncherPlugin[];
  /**
   * 从插件包安装的插件（磁盘上的 `plugins/<id>/`）。
   *
   * ## 为什么不并进 `custom`
   *
   * `custom` 是「用户粘了一段 JSON」，它的生命周期完全由 localStorage 决定；
   * 这一组是「磁盘上有一个目录」，它的真值在文件系统，localStorage 只是缓存。
   * 混在一起会出现两种相反的错误：删掉磁盘目录后 `custom` 里的条目还在（幽灵
   * 插件），或者插件被手工放进目录后界面上没有（装了却看不见）。分开存，并在
   * 每次 reload 时用磁盘结果整体替换这一组，两种都不会发生。
   *
   * 运行时它和 `custom` 走同一条渲染与开关路径——用户不需要知道两者的区别。
   */
  installed?: LauncherPlugin[];
  /** 每个插件的启用状态；缺省时用 enabledByDefault。 */
  enabled: Record<string, boolean>;
  /** 每个插件的选项值；缺省时用选项定义里的默认值。 */
  options: Record<string, Record<string, boolean | number>>;
  /**
   * 用户已采纳的更新：插件 id → 已采纳到的版本号。
   *
   * 为什么需要这一层：内置插件的清单是代码里的常量，改了不会持久。如果只把
   * 新版本写进 `custom`，内置条目仍会带着 `updateAvailable` 出现在列表里，
   * 徽章数字也就降不下来——那会变成「点了更新但红点还在」的假成功。
   *
   * 所以单独记录「用户已经接受过这个版本」，`allPlugins` 组装清单时据此清掉
   * 内置插件的待更新标记。这是离线环境下能做到的诚实语义：不下载任何文件，
   * 只记录用户的选择。
   */
  acknowledgedUpdates: Record<string, string>;
}

export const EMPTY_PLUGIN_STORE: PluginStore = { custom: [], installed: [], enabled: {}, options: {}, acknowledgedUpdates: {} };

// ---------------------------------------------------------------------------
// 内置插件目录
// ---------------------------------------------------------------------------

const HOOK_LABEL_KEYS: Record<PluginHookId, MessageKey> = {
  launch: 'plugin.hook.launch',
  modInstall: 'plugin.hook.modInstall',
  instanceCreate: 'plugin.hook.instanceCreate',
  theme: 'plugin.hook.theme',
  sidebar: 'plugin.hook.sidebar',
  background: 'plugin.hook.background',
};

const HOOK_HINT_KEYS: Record<PluginHookId, MessageKey> = {
  launch: 'plugin.hook.launchHint',
  modInstall: 'plugin.hook.modInstallHint',
  instanceCreate: 'plugin.hook.instanceCreateHint',
  theme: 'plugin.hook.themeHint',
  sidebar: 'plugin.hook.sidebarHint',
  background: 'plugin.hook.backgroundHint',
};

/**
 * 哪些扩展点已经真的会触发。
 *
 * 与后端 `plugin_runtime_info().kinds` 是同一件事的两个表述：那边列的是**贡献**
 * 类型（真正能做的事），这里列的是**时机**（什么时候被调用）。两者必须同时满足
 * 才算「会执行」。
 *
 * `launch` 由启动链路触发；`theme` / `background` / `sidebar` 由 `App.tsx` 触发
 * （见 `ariaSeason`、`ariaTint` 与 `applySeasonTheme`），这是 Aria with You 从
 * `planned` 转为可用的前提。`modInstall` / `instanceCreate` 仍然只接受声明。
 *
 * 这里的每一项都必须能指出**一行**读它的代码。加进来只是为了让某个插件的卡片
 * 好看，就等于让界面替一个不存在的行为作证——那正是这份列表存在的理由。
 */
export const EXECUTING_HOOKS: PluginHookId[] = ['launch', 'theme', 'background', 'sidebar'];

const PERMISSION_LABEL_KEYS: Record<PluginPermissionId, MessageKey> = {
  filesystem: 'plugin.perm.filesystem',
  network: 'plugin.perm.network',
  launchHook: 'plugin.perm.launchHook',
  theme: 'plugin.perm.theme',
  uiSlot: 'plugin.perm.uiSlot',
  storage: 'plugin.perm.storage',
};

const PERMISSION_HINT_KEYS: Record<PluginPermissionId, MessageKey> = {
  filesystem: 'plugin.perm.filesystemHint',
  network: 'plugin.perm.networkHint',
  launchHook: 'plugin.perm.launchHookHint',
  theme: 'plugin.perm.themeHint',
  uiSlot: 'plugin.perm.uiSlotHint',
  storage: 'plugin.perm.storageHint',
};

export function hookLabel(id: PluginHookId): Localized {
  return loc(HOOK_LABEL_KEYS[id]);
}

export function hookHint(id: PluginHookId): Localized {
  return loc(HOOK_HINT_KEYS[id]);
}

export function permissionLabel(id: PluginPermissionId): Localized {
  return loc(PERMISSION_LABEL_KEYS[id]);
}

export function permissionHint(id: PluginPermissionId): Localized {
  return loc(PERMISSION_HINT_KEYS[id]);
}

export const BUILTIN_PLUGINS: LauncherPlugin[] = [
  /*
   * 只保留「开关真的改变行为」的插件。
   *
   * 判断标准是能不能指出一行会读这个开关的代码，而不是「听起来有用」：
   *
   *   - `launch-logger` → 启动路径查 `is_enabled(.., "launch-logger")` 决定是否
   *     写启动历史（见 `src-tauri/src/lib.rs`）。
   *   - `download-mirror` → 开关直接映射到 `downloadSource` 设置，下载器读它。
   *
   * 其余几个曾有清单条目但没有对应实现：自动备份的贡献永远为空（因为选项值
   * 只存在前端，后端拿不到目录）、冲突检测在后端无条件执行、主题/悬浮层/旧版
   * 皮肤在界面里从未被读取。它们的开关拨了不会有任何变化，而界面却显示「已启用」。
   * 这种占位条目比缺失更有害：用户会以为自己关掉了某个功能，或者以为启用后
   * 多出了某种保护。功能真正实现时再加回来，并同时补上读取它的代码。
   *
   * `logLimit` 选项去掉了：启动历史目前不按条数裁剪，暴露一个不生效的上限设置
   * 等于承诺一件没做的事。
   */
  {
    id: 'launch-logger',
    name: loc('plugin.launchLogger.name'),
    version: '1.0.0',
    author: 'MFMC Team',
    description: loc('plugin.launchLogger.description'),
    glyph: 'scroll',
    accent: '#53B4F6',
    hooks: ['launch'],
    permissions: ['launchHook', 'storage'],
    enabledByDefault: true,
    builtin: true,
  },
  {
    id: 'download-mirror',
    name: loc('plugin.mirrorHelper.name'),
    version: '2.0.1',
    author: 'MFMC Team',
    description: loc('plugin.mirrorHelper.description'),
    glyph: 'network',
    accent: '#35B9A8',
    hooks: ['launch'],
    permissions: ['network', 'storage'],
    options: [
      { id: 'preferBmclapi', type: 'toggle', label: loc('plugin.option.preferBmclapi'), hint: loc('plugin.option.preferBmclapiHint'), defaultOn: true },
    ],
    enabledByDefault: true,
    builtin: true,
  },
];

/** 第三方插件必须独立安装；保留空导出供旧调用方兼容，不参与已安装列表。 */
export const BUNDLED_PLUGINS: LauncherPlugin[] = [];

/**
 * 计划中的外置插件。
 *
 * ## 为什么它们不放进 `BUILTIN_PLUGINS`
 *
 * 上面那份清单的准入规则是「能指出一行会读这个开关的代码」。这里放的是相反的
 * 东西：能力还没写，先把清单公布出来，让用户知道路线图上有什么。混进同一份数组
 * 会让它们自动获得「可启用」的外观，而那正是准入规则要防的事。
 *
 * 它们的身份也确实是外置的：将来由第三方维护、以普通清单分发，用户选择安装，
 * 而不是随启动器一起编译进去。所以卡片上标「第三方」，并明确说明**当前版本
 * 尚未实现**——用户可以提前看到它会申请什么权限，但拨不动开关。
 *
 * ## 数据形状
 *
 * 与 `LauncherPlugin` 完全一致，只是多了 `planned: true`。用同一份类型而不是
 * 另建一种「预告」结构，是为了让卡片渲染只有一条路径：将来实现之后，只需要把
 * 条目挪进 `custom`（用户安装了它），`planned` 一去掉，其余代码不用改。
 */
export const PLANNED_PLUGINS: LauncherPlugin[] = [];

// ---------------------------------------------------------------------------
// 归一化与持久化
// ---------------------------------------------------------------------------

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}

/** 文案只接受 {key} 或字符串，防止外部清单塞进任意对象当文案渲染。 */
function normalizeLocalized(raw: unknown): Localized | null {
  if (typeof raw === 'string' && raw.trim()) return raw.trim().slice(0, 400);
  if (isPlainObject(raw) && typeof raw.key === 'string') return { key: raw.key as MessageKey };
  return null;
}

const GLYPHS: PluginGlyph[] = ['scroll', 'archive', 'shield', 'palette', 'gauge', 'network', 'image', 'plug'];
const HOOKS: PluginHookId[] = ['launch', 'modInstall', 'instanceCreate', 'theme', 'sidebar', 'background'];
const PERMISSIONS: PluginPermissionId[] = ['filesystem', 'network', 'launchHook', 'theme', 'uiSlot', 'storage'];

/** 只接受 6 位十六进制颜色：清单里的颜色会写进 style，不能放任意字符串。 */
function normalizeAccent(raw: unknown): string {
  return typeof raw === 'string' && /^#[0-9a-fA-F]{6}$/.test(raw) ? raw : '#53B4F6';
}

function normalizeOption(raw: unknown): PluginOption | null {
  if (!isPlainObject(raw)) return null;
  if (typeof raw.id !== 'string' || !raw.id.trim()) return null;
  const label = normalizeLocalized(raw.label);
  if (!label) return null;
  if (raw.type !== 'toggle' && raw.type !== 'number') return null;
  const option: PluginOption = { id: raw.id.trim().slice(0, 40), type: raw.type, label };
  const hint = normalizeLocalized(raw.hint);
  if (hint) option.hint = hint;
  if (raw.type === 'toggle') {
    option.defaultOn = raw.defaultOn === true;
  } else {
    const min = typeof raw.min === 'number' && Number.isFinite(raw.min) ? Math.round(raw.min) : 0;
    const max = typeof raw.max === 'number' && Number.isFinite(raw.max) ? Math.round(raw.max) : 9999;
    option.min = Math.min(min, max);
    option.max = Math.max(min, max);
    const fallback = typeof raw.defaultNumber === 'number' && Number.isFinite(raw.defaultNumber) ? Math.round(raw.defaultNumber) : option.min;
    option.defaultNumber = Math.min(option.max, Math.max(option.min, fallback));
  }
  return option;
}

/**
 * 归一化一个插件清单。
 *
 * 手动安装的清单完全来自用户粘贴的 JSON，所以每个字段都单独校验：宁可丢掉
 * 一个非法字段，也不要让不合法的数据进到渲染路径（例如把对象当 React 子节点）。
 */
export function normalizePlugin(raw: unknown): LauncherPlugin | null {
  if (!isPlainObject(raw)) return null;
  const id = typeof raw.id === 'string' ? raw.id.trim().slice(0, 64) : '';
  const version = typeof raw.version === 'string' ? raw.version.trim().slice(0, 24) : '';
  const author = typeof raw.author === 'string' ? raw.author.trim().slice(0, 48) : '';
  const name = normalizeLocalized(raw.name);
  const description = normalizeLocalized(raw.description);
  if (!id || !version || !name || !description) return null;
  // id 会被用作存储键与 React key，限制成安全字符集。
  if (!/^[a-z0-9][a-z0-9._-]*$/i.test(id)) return null;

  const hooks = Array.isArray(raw.hooks)
    ? HOOKS.filter(hook => (raw.hooks as unknown[]).includes(hook))
    : [];
  const permissions = Array.isArray(raw.permissions)
    ? PERMISSIONS.filter(permission => (raw.permissions as unknown[]).includes(permission))
    : [];
  const options = Array.isArray(raw.options)
    ? raw.options.map(normalizeOption).filter((option): option is PluginOption => option !== null)
    : undefined;
  const glyph = GLYPHS.includes(raw.glyph as PluginGlyph) ? (raw.glyph as PluginGlyph) : 'plug';
  const status = normalizeLocalized(raw.status);
  const contributions = Array.isArray(raw.contributions)
    ? raw.contributions
        .map(normalizeContribution)
        .filter((item): item is PluginContribution => item !== null)
    : undefined;

  return {
    id,
    name,
    version,
    author: author || 'unknown',
    description,
    glyph,
    accent: normalizeAccent(raw.accent),
    hooks,
    permissions,
    ...(options?.length ? { options } : {}),
    ...(contributions?.length ? { contributions } : {}),
    enabledByDefault: raw.enabledByDefault !== false,
    builtin: false,
    requiresRestart: raw.requiresRestart === true,
    ...(status ? { status, statusTone: raw.statusTone === 'warn' || raw.statusTone === 'error' ? raw.statusTone : 'info' } : {}),
  };
}

/**
 * 把一个**已安装的插件包**转成界面用的插件。
 *
 * ## 与 `normalizePlugin` 的区别
 *
 * `normalizePlugin` 校验用户粘贴的清单，任何必填字段缺失就返回 `null`——那是对的，
 * 因为粘贴的东西没有别的来源可以兜底。
 *
 * 这一组不一样：文件已经在磁盘上、包已经通过了解压校验，清单是一个确定存在的
 * 文件。所以这里**不返回 null**，而是给缺失的展示字段补上可读的兜底值。理由：
 * 一个装好的插件因为 `description` 写成对象而不是字符串就整个消失，用户看到的
 * 是「装了但列表里没有」，那是比「描述没显示」严重得多的失败。
 *
 * 真正不可缺的只有 `id` 与 `version`：它们已经是目录名，后端在安装时校验过。
 *
 * ## 素材前缀
 *
 * `assetBase` 由后端给出，这里把它存进 `LauncherPlugin`，供插件自己声明的图片
 * 路径拼接。不对外部清单里的绝对 URL 做任何放行——一个插件要显示图片，图片就得
 * 在它自己的包里。
 */
export function pluginFromInstalled(record: {
  id: string;
  version: string;
  manifest: unknown;
  assetBase?: string;
}): LauncherPlugin | null {
  if (!record.id || !record.version) return null;
  if (!/^[a-z0-9][a-z0-9._-]*$/i.test(record.id)) return null;

  const raw = isPlainObject(record.manifest) ? record.manifest : {};
  /*
   * 缺名字时退回 id。
   *
   * `Localized` 允许一段原文（`raw`），正是为这种「拿不到词条」的情况准备的。
   * 把 id 硬塞进 `loc()` 需要类型断言，运行时则会取到不存在的词条并显示成键名——
   * 比显示 id 更糟。id 至少是用户能在插件目录里核对的东西。
   */
  const name: Localized = normalizeLocalized(raw.name) ?? { key: 'plugin.installedNoDescription', raw: record.id };
  const description: Localized = normalizeLocalized(raw.description) ?? loc('plugin.installedNoDescription');
  const author =
    typeof raw.author === 'string' && raw.author.trim() ? raw.author.trim().slice(0, 48) : 'unknown';

  const hooks = Array.isArray(raw.hooks)
    ? HOOKS.filter(hook => (raw.hooks as unknown[]).includes(hook))
    : [];
  const permissions = Array.isArray(raw.permissions)
    ? PERMISSIONS.filter(permission => (raw.permissions as unknown[]).includes(permission))
    : [];
  const options = Array.isArray(raw.options)
    ? raw.options.map(normalizeOption).filter((option): option is PluginOption => option !== null)
    : undefined;
  const contributions = Array.isArray(raw.contributions)
    ? raw.contributions
        .map(normalizeContribution)
        .filter((item): item is PluginContribution => item !== null)
    : undefined;
  const glyph = GLYPHS.includes(raw.glyph as PluginGlyph) ? (raw.glyph as PluginGlyph) : 'plug';

  return {
    id: record.id,
    name,
    version: record.version,
    author,
    description,
    glyph,
    accent: normalizeAccent(raw.accent),
    hooks,
    permissions,
    ...(options?.length ? { options } : {}),
    ...(contributions?.length ? { contributions } : {}),
    // 新装的包默认启用。这是产品决定，不是疏忽：装一个插件的人已经表达了
    // 「我要用它」，再让他去拨一次开关只是多一步。安全性由安装前的确认弹窗
    // 承担（权限逐条列出），不由开关承担——一个默认关闭的开关挡不住任何东西，
    // 它只会让用户以为自己装了却坏了。
    enabledByDefault: raw.enabledByDefault !== false,
    builtin: false,
    requiresRestart: raw.requiresRestart === true,
    // 来源标记：从包安装的一律是第三方。清单不能自称 `builtin`。
    thirdParty: true,
    ...(record.assetBase ? { assetBase: record.assetBase } : {}),
  };
}

/**
 * 归一化一条贡献。
 *
 * 这里只保证形状（`kind` 是已知枚举、`value` 是字符串），**不做内容校验**——
 * 真正的校验在后端（`launcher_core::plugins::validate_contribution`），它掌握
 * 被禁止的参数形态。前端复制一份规则只会分叉。
 *
 * 空值丢弃：一条没有内容的贡献不会做任何事，留下来只会让「贡献 1 条」的计数说谎。
 */
function normalizeContribution(raw: unknown): PluginContribution | null {
  if (!isPlainObject(raw)) return null;
  const kind = raw.kind;
  if (kind !== 'jvmArgs' && kind !== 'env' && kind !== 'preLaunch' && kind !== 'backupBefore') return null;
  if (typeof raw.value !== 'string') return null;
  const value = raw.value.trim();
  if (!value) return null;
  return { kind, value: value.slice(0, 2048) };
}

export function loadPluginStore(): PluginStore {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(PLUGIN_STORAGE_KEY) || '{}');
    if (!isPlainObject(parsed)) return EMPTY_PLUGIN_STORE;
    const custom = Array.isArray(parsed.custom)
      ? parsed.custom.map(normalizePlugin).filter((plugin): plugin is LauncherPlugin => plugin !== null)
      : [];
    // 缓存不授予安装身份或素材访问权限；启动后必须由后端重新扫描磁盘。
    const installed: LauncherPlugin[] = [];
    const enabled: Record<string, boolean> = {};
    if (isPlainObject(parsed.enabled)) {
      Object.entries(parsed.enabled).forEach(([id, value]) => {
        if (typeof value === 'boolean') enabled[id] = value;
      });
    }
    const options: Record<string, Record<string, boolean | number>> = {};
    if (isPlainObject(parsed.options)) {
      Object.entries(parsed.options).forEach(([id, value]) => {
        if (!isPlainObject(value)) return;
        const entry: Record<string, boolean | number> = {};
        Object.entries(value).forEach(([key, raw]) => {
          if (typeof raw === 'boolean') entry[key] = raw;
          else if (typeof raw === 'number' && Number.isFinite(raw)) entry[key] = raw;
        });
        options[id] = entry;
      });
    }
    const acknowledgedUpdates: Record<string, string> = {};
    if (isPlainObject(parsed.acknowledgedUpdates)) {
      Object.entries(parsed.acknowledgedUpdates).forEach(([id, value]) => {
        if (typeof value === 'string') acknowledgedUpdates[id] = value;
      });
    }
    // 扫描前不能按空的 installed 清掉偏好，否则每次启动都会丢失独立插件的开关。
    return { custom, installed, enabled, options, acknowledgedUpdates };
  } catch {
    return EMPTY_PLUGIN_STORE;
  }
}

export function savePluginStore(store: PluginStore): void {
  try {
    localStorage.setItem(PLUGIN_STORAGE_KEY, JSON.stringify(store));
  } catch { /* storage full or blocked */ }
}

/**
 * 全部插件：内置 → 从包安装 → 手动粘贴 → 计划中。
 *
 * 排序即信任顺序：越靠前越是启动器自己能负责的东西。这不是装饰——用户扫列表时
 * 第一眼看到的是最可信的一组，而要装什么外部代码时，它总在下面。
 *
 * 组装时把「已采纳的更新」应用到内置条目上：版本号换成新值、清掉待更新标记。
 * 这样徽章、卡片与「需要注意」筛选读到的都是同一份已更新的清单，
 * 不会出现各处判断不一致。
 */
export function allPlugins(store: PluginStore): LauncherPlugin[] {
  const acknowledged = store.acknowledgedUpdates ?? {};
  const builtin = BUILTIN_PLUGINS.map(plugin => {
    const accepted = acknowledged[plugin.id];
    if (!accepted || !plugin.updateAvailable) return plugin;
    return { ...plugin, version: accepted, updateAvailable: undefined };
  });
  const claimed = new Set(builtin.map(item => item.id));
  const unique = (plugin: LauncherPlugin) => {
    if (claimed.has(plugin.id)) return false;
    claimed.add(plugin.id);
    return true;
  };
  const installed = (store.installed ?? []).filter(unique);
  const custom = store.custom.filter(unique);
  return [...builtin, ...installed, ...custom, ...PLANNED_PLUGINS.filter(unique)];
}

/** 插件当前是否启用。不兼容的插件永远视为未启用。 */
export function isPluginEnabled(plugin: LauncherPlugin, store: PluginStore): boolean {
  // 计划中的插件恒为未启用：开关置灰，也不该被计入任何「已启用」统计。
  if (plugin.incompatible || plugin.planned) return false;
  return store.enabled[plugin.id] ?? plugin.enabledByDefault;
}

/**
 * 已启用的插件数。
 *
 * 计划中的插件不计入：它会出现在列表里，但既没安装也没启用，「已启用 3 个」里
 * 混进一个永远孵化不出来的条目，会让这个数字失去意义。
 */
export function enabledPluginCount(store: PluginStore): number {
  return allPlugins(store).filter(plugin => isPluginEnabled(plugin, store)).length;
}

/**
 * 已经装好的插件数（含内置，不含计划中的）。
 *
 * 页头用它而不是 `plugins.length`：列表里现在可能有一个「以后会有」的条目，
 * 把它算进「已安装」会让用户以为它已经在机器上了。
 */
export function installedPluginCount(store: PluginStore): number {
  return allPlugins(store).filter(plugin => !plugin.planned).length;
}

/**
 * 丢掉指向已不存在插件的存档条目。
 *
 * 内置插件清单是代码常量，删掉一个插件（比如那些只有清单、没有实现的）之后，
 * 用户存档里仍然留着它的 `enabled` 与 `options` 记录。这不会让界面出错——
 * 渲染只遍历当前清单——但会永久留下无人读取的数据，而且如果以后用同一个 id
 * 重新加回一个语义不同的插件，旧开关会意外生效。
 *
 * 自定义插件不在清理范围内：它们的 id 由清单决定，只在用户卸载时移除。
 */
export function prunePluginStore(store: PluginStore): PluginStore {
  const known = new Set([
    ...BUILTIN_PLUGINS.map(plugin => plugin.id),
    // 已装到磁盘的插件也是「已知」的：它们的开关状态必须活下来，否则每次刷新
    // 都会把用户刚打开的开关重置成默认值。
    ...(store.installed ?? []).map(plugin => plugin.id),
  ]);
  const custom = new Set(store.custom.map(plugin => plugin.id));
  const keep = (id: string) => known.has(id) || custom.has(id);

  const enabled = Object.fromEntries(Object.entries(store.enabled).filter(([id]) => keep(id)));
  const options = Object.fromEntries(Object.entries(store.options).filter(([id]) => keep(id)));
  const acknowledgedUpdates = Object.fromEntries(
    Object.entries(store.acknowledgedUpdates ?? {}).filter(([id]) => keep(id)),
  );

  // 引用没变时返回原对象：调用方可以据此跳过一次无意义的持久化。
  const unchanged =
    Object.keys(enabled).length === Object.keys(store.enabled).length
    && Object.keys(options).length === Object.keys(store.options).length
    && Object.keys(acknowledgedUpdates).length === Object.keys(store.acknowledgedUpdates ?? {}).length;
  return unchanged ? store : { ...store, enabled, options, acknowledgedUpdates };
}

/**
 * 这个插件是否需要用户特别留意。
 *
 * 只把「真的会影响使用」的情况算进来：不兼容（装了也用不了）、以及带警告或
 * 错误状态的插件。像「规则库每天更新」这类纯说明性文字不算——否则侧栏徽章
 * 会长期挂在 3 上，用户很快就学会无视它。
 *
 * `pendingRestart` 由页面传入：某项改动还没重启就生效，也是需要留意的事。
 */
export function pluginNeedsAttention(plugin: LauncherPlugin, pendingRestart: string[] = []): boolean {
  if (plugin.incompatible) return true;
  if (plugin.updateAvailable) return true;
  if (plugin.statusTone === 'warn' || plugin.statusTone === 'error') return true;
  return pendingRestart.includes(plugin.id);
}

/**
 * 有几个插件可以更新。
 *
 * 侧栏红色徽章用它，而不是用 `pluginNeedsAttention` 的总数：那个总数里混着
 * 「实验性提示」「不兼容」「待重启」，只有「可更新」是用户点进插件页就能立刻
 * 解决的事。红色数字应该指向可执行的动作，否则用户点进去发现没有「更新」按钮，
 * 下次就会忽略这个提示。
 */
export function updatablePluginCount(store: PluginStore): number {
  return allPlugins(store).filter(plugin => Boolean(plugin.updateAvailable)).length;
}

/** 读一个选项的当前值，缺省时回落到定义里的默认值。 */
export function optionValue(plugin: LauncherPlugin, optionId: string, store: PluginStore): boolean | number {
  const option = plugin.options?.find(item => item.id === optionId);
  if (!option) return 0;
  const stored = store.options[plugin.id]?.[optionId];
  if (option.type === 'toggle') {
    return typeof stored === 'boolean' ? stored : (option.defaultOn ?? false);
  }
  const fallback = option.defaultNumber ?? option.min ?? 0;
  if (typeof stored !== 'number') return fallback;
  return Math.min(option.max ?? 9999, Math.max(option.min ?? 0, stored));
}

// ---------------------------------------------------------------------------
// 与后端运行时的同步
// ---------------------------------------------------------------------------

/**
 * 把界面状态翻译成后端认识的形状。
 *
 * ## 为什么不再只是一份本地 JSON 草稿
 *
 * 之前插件的启用状态只写在 localStorage 里，后端完全不知道——插件页显示「已启用」，
 * 启动时却没有任何东西被应用。现在后端读自己的 `plugin_state` 表，界面这里负责
 * 把用户的操作同步过去。
 *
 * 只同步两样东西：**启用了哪些**，以及**自定义插件的贡献定义**。文案、图标、
 * 品牌色是纯展示，后端用不上。
 */
export function toPluginStatePayload(store: PluginStore): PluginStatePayload {
  const custom: PluginDefinition[] = [];
  /*
   * 从磁盘装进来的插件也要发给后端。
   *
   * ## 为什么这一条不能省
   *
   * 后端不认识插件目录里的清单：它在启动时读 `plugin_state` 表，把那里的 `custom`
   * 当成「用户装的插件」逐条校验并执行贡献。如果前端只同步粘贴的那些，一个从
   * GitHub 装进来、界面上显示「已启用」的插件，在启动链路里**完全不存在**——
   * 那正是「界面替一个不存在的行为作证」，也正是这份同步函数存在的理由。
   *
   * ## 与粘贴的那些的唯一区别
   *
   * 来源不同，贡献的执行方式相同。两者都在用户可移除的范围内。
   */
  for (const plugin of allPlugins(store)) {
    /*
     * 计划中的插件不发给后端。
     *
     * 后端没有 `planned` 这个概念，收到它就会当成一个正常启用的自定义插件，参与
     * 启动链路——于是「已经公布但还没实现」的插件会真的按默认启用去执行它声明的
     * 贡献，而那些贡献恰恰是我们还没验证过的。前端的置灰只是显示，真正的边界在
     * 这里：压根不发出去。
     */
    if (plugin.builtin || plugin.planned) continue;
    custom.push({
      id: plugin.id,
      version: plugin.version,
      hooks: [...plugin.hooks],
      permissions: [...plugin.permissions],
      contributions: (plugin.contributions ?? []).map(item => ({ kind: item.kind, value: item.value })),
      incompatible: plugin.incompatible === true,
    });
  }
  /*
   * 启用状态整份写入。
   *
   * 后端对未出现的 id 回退「默认启用」，与 `isPluginEnabled` 的规则一致。不能只
   * 写差异：前端不知道后端内置清单的默认值，省掉任何一项都可能让一个本应启用的
   * 插件变成启用（或反之）。整份写入最直白，也不会有一致性缺口。
   *
   * 计划中的插件不在 `store.enabled` 里（开关是禁用的，永远写不进去），所以这里
   * 不需要额外过滤；上面的 `custom` 那一层才是真正的边界。
   */
  const enabled = { ...store.enabled };
  for (const plugin of allPlugins(store)) {
    if (!plugin.planned) enabled[plugin.id] = isPluginEnabled(plugin, store);
  }
  return { custom, enabled };
}
