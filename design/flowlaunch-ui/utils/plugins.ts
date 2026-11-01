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

export const EMPTY_PLUGIN_STORE: PluginStore = { custom: [], enabled: {}, options: {}, acknowledgedUpdates: {} };

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
    options: [
      { id: 'logLimit', type: 'number', label: loc('plugin.option.logLimit'), hint: loc('plugin.option.logLimitHint'), defaultNumber: 200, min: 20, max: 2000 },
    ],
    enabledByDefault: true,
    builtin: true,
  },
  {
    id: 'auto-backup',
    name: loc('plugin.autoBackup.name'),
    version: '1.2.0',
    author: 'MFMC Team',
    description: loc('plugin.autoBackup.description'),
    glyph: 'archive',
    accent: '#41C98B',
    hooks: ['launch'],
    permissions: ['filesystem', 'launchHook'],
    options: [
      { id: 'keepCount', type: 'number', label: loc('plugin.option.keepCount'), hint: loc('plugin.option.keepCountHint'), defaultNumber: 5, min: 1, max: 30 },
      { id: 'onlyWhenChanged', type: 'toggle', label: loc('plugin.option.onlyWhenChanged'), hint: loc('plugin.option.onlyWhenChangedHint'), defaultOn: true },
    ],
    enabledByDefault: true,
    builtin: true,
  },
  {
    id: 'mod-conflict-detector',
    name: loc('plugin.conflictDetector.name'),
    version: '0.9.3',
    author: 'MFMC Team',
    description: loc('plugin.conflictDetector.description'),
    glyph: 'shield',
    accent: '#F19B52',
    hooks: ['modInstall'],
    permissions: ['network', 'uiSlot'],
    enabledByDefault: true,
    builtin: true,
    status: loc('plugin.conflictDetector.status'),
    statusTone: 'info',
    // 模拟一次可用更新：这是内置清单里的示例，用于演示徽章与更新按钮。
    updateAvailable: '1.0.0',
  },
  {
    id: 'download-mirror',
    name: loc('plugin.mirrorHelper.name'),
    version: '2.0.1',
    author: 'MirrorKit',
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
  {
    id: 'pink-theme',
    name: loc('plugin.sakuraTheme.name'),
    version: '1.0.0',
    author: 'Sakura',
    description: loc('plugin.sakuraTheme.description'),
    glyph: 'palette',
    accent: '#E76B8A',
    hooks: ['theme'],
    permissions: ['theme'],
    enabledByDefault: false,
    builtin: true,
    requiresRestart: true,
  },
  {
    id: 'performance-monitor',
    name: loc('plugin.monitorName'),
    version: '1.1.2',
    author: 'MFMC Team',
    description: loc('plugin.monitorDescription'),
    glyph: 'gauge',
    accent: '#8B7CFF',
    hooks: ['launch', 'sidebar'],
    permissions: ['launchHook', 'uiSlot'],
    options: [
      { id: 'showOverlay', type: 'toggle', label: loc('plugin.option.showOverlay'), hint: loc('plugin.option.showOverlayHint'), defaultOn: false },
    ],
    enabledByDefault: false,
    builtin: true,
    status: loc('plugin.monitorStatus'),
    statusTone: 'info',
  },
  {
    id: 'legacy-skin-patch',
    name: loc('plugin.legacySkin.name'),
    version: '0.3.1',
    author: 'OldCraft',
    description: loc('plugin.legacySkin.description'),
    glyph: 'image',
    accent: '#94A3B8',
    hooks: ['launch'],
    permissions: ['filesystem', 'launchHook'],
    enabledByDefault: false,
    builtin: true,
    status: loc('plugin.legacySkin.status'),
    statusTone: 'error',
    incompatible: true,
  },
];

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
    return { custom, enabled, options, acknowledgedUpdates };
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
 * 全部插件：内置清单在前，手动安装的在后。
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
  return [...builtin, ...store.custom];
}

/** 插件当前是否启用。不兼容的插件永远视为未启用。 */
export function isPluginEnabled(plugin: LauncherPlugin, store: PluginStore): boolean {
  if (plugin.incompatible) return false;
  return store.enabled[plugin.id] ?? plugin.enabledByDefault;
}

export function enabledPluginCount(store: PluginStore): number {
  return allPlugins(store).filter(plugin => isPluginEnabled(plugin, store)).length;
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
  for (const plugin of store.custom) {
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
   */
  return { custom, enabled: { ...store.enabled } };
}
