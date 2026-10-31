// 全局设置的读取。
//
// 为什么单独抽一个模块：实例管理页也要显示「跟随全局时实际是什么值」，而全局
// 设置原本只在 SettingsPanel.tsx 内部可见（一个 interface + 一个 readPrefs）。
// 把读取逻辑放在这里，两处用同一份默认值和同一套归一化，避免「管理页说全局是
// 4 GB、设置页显示 2 GB」这类不一致。
//
// 写入仍然由 SettingsPanel 负责（它还要处理保存/取消与原生载荷），这里只读。

import { DEFAULT_SKIN_SETTING, type SkinSetting } from '../components/SkinSection';

export const SETTINGS_STORAGE_KEY = 'flowlaunch-settings';

export type ThemeMode = 'light' | 'dark' | 'system';

export interface LauncherPrefs {
  defaultVersionIsolation: boolean; gameResourceLocation: string; skin: SkinSetting; launcherVisibility: 'keep' | 'hide' | 'close'; processPriority: 'normal' | 'high' | 'low'; windowPreset: 'default' | 'custom';
  fullscreen: boolean; gameArgs: string; preLaunchCommand: string; memoryMode: 'auto' | 'custom'; optimizeMemory: boolean; launchWrapperDisabled: boolean; lwjglUnsafeAgentDisabled: boolean; hardwareAcceleration: boolean;
  proxyMode: 'none' | 'system' | 'http'; proxyUrl: string; downloadTimeoutSec: number; downloadRetries: number; speedLimitKbps: number; verifySsl: boolean; downloadDirectory: string;
  debugMode: boolean; showCommandPreview: boolean; safeMode: boolean; logLevel: 'error' | 'warn' | 'info' | 'debug'; environmentVariables: string;
  communitySource: string; modpackFilenameFormat: string; modpackMergeMode: 'ask' | 'replace' | 'merge'; showGameHints: boolean; showDebugHints: boolean;
  theme: ThemeMode; themePreset: string; customAccent: string; showGameLog: boolean;
  javaPath: string; memoryMb: number; windowWidth: number; windowHeight: number; extraJvmArgs: string;
  downloadSource: string; maxConcurrentDownloads: number; connectionsPerFile: number; aria2Enabled: boolean;
  closeAfterLaunch: boolean; checkUpdates: boolean; gameDirectory: string; announcementPolicy: 'notify' | 'off'; cacheDirectory: string; anonymousUsage: boolean; opacity: number; hiddenTheme: boolean; showLauncherLogo: boolean; backgroundImage: string; backgroundMusic: boolean; titleBar: 'default' | 'text' | 'image' | 'none'; homeSource: string;
}

export const DEFAULT_PREFS: LauncherPrefs = {
  defaultVersionIsolation: true, gameResourceLocation: 'default', skin: { ...DEFAULT_SKIN_SETTING }, launcherVisibility: 'keep', processPriority: 'normal', windowPreset: 'default',
  fullscreen: false, gameArgs: '', preLaunchCommand: '', memoryMode: 'custom', optimizeMemory: false, launchWrapperDisabled: false, lwjglUnsafeAgentDisabled: false, hardwareAcceleration: true,
  debugMode: false, showCommandPreview: false, safeMode: false, logLevel: 'info', environmentVariables: '',
  proxyMode: 'none', proxyUrl: '', downloadTimeoutSec: 60, downloadRetries: 3, speedLimitKbps: 0, verifySsl: true, downloadDirectory: '',
  communitySource: 'official', modpackFilenameFormat: '{name}-{version}', modpackMergeMode: 'ask', showGameHints: true, showDebugHints: false,
  theme: 'dark', themePreset: 'ocean', customAccent: '', showGameLog: true,
  javaPath: '', memoryMb: 4096, windowWidth: 1280, windowHeight: 720, extraJvmArgs: '',
  // 'auto' → 后端 `mixed`：镜像优先、官方回退（详见 native.ts 的 prefsToSettings）。
  // 默认选它是因为官方 CDN 在部分地区很慢，而镜像只在有校验值时才用，不牺牲完整性。
  downloadSource: 'auto', maxConcurrentDownloads: 4, connectionsPerFile: 4, aria2Enabled: true,
  closeAfterLaunch: false, checkUpdates: true, gameDirectory: '', announcementPolicy: 'notify', cacheDirectory: '', anonymousUsage: false, opacity: 100, hiddenTheme: false, showLauncherLogo: true, backgroundImage: '', backgroundMusic: false, titleBar: 'default', homeSource: 'official',
};

/** 存档里的皮肤设置可能来自旧版本或只有部分字段，逐项兜底。 */
export function normalizeSkin(raw: unknown): SkinSetting {
  if (!raw || typeof raw !== 'object') return { ...DEFAULT_SKIN_SETTING };
  const value = raw as Partial<SkinSetting>;
  const type: SkinSetting['type'] = value.type === 'steve' || value.type === 'alex' || value.type === 'premium' || value.type === 'custom'
    ? value.type
    : 'random';
  return {
    type,
    premiumName: typeof value.premiumName === 'string' ? value.premiumName.slice(0, 16) : '',
    slim: value.slim === true,
    customDataUrl: typeof value.customDataUrl === 'string' ? value.customDataUrl : '',
    customFileName: typeof value.customFileName === 'string' ? value.customFileName : '',
  };
}

/**
 * 读取全局设置。
 *
 * `currentAccent` 用于「从未保存过强调色」时回落到屏幕上实际生效的那个值——
 * 只有浏览器里才有 getComputedStyle，所以它是可选参数，服务端渲染时传空。
 */
export function readPrefs(currentAccent = ''): LauncherPrefs {
  type StoredPrefs = Partial<LauncherPrefs> & { customSkin?: unknown };
  let saved: StoredPrefs = {};
  try { saved = JSON.parse(localStorage.getItem(SETTINGS_STORAGE_KEY) || '{}'); } catch { /* use defaults */ }
  // 旧版本存过一个没有实际含义的皮肤下拉字段。它已经被离线皮肤取代，这里
  // 显式丢弃，避免把废弃字段继续写回 localStorage。
  const { customSkin: legacyFieldToDrop, ...rest } = saved;
  void legacyFieldToDrop;
  return {
    ...DEFAULT_PREFS,
    ...(!saved.customAccent && currentAccent ? { customAccent: currentAccent } : {}),
    ...rest,
    skin: normalizeSkin(rest.skin),
  };
}

/** 从存档里读出全局设置，不访问 getComputedStyle（可在任何环境调用）。 */
export function readStoredPrefs(): LauncherPrefs {
  return readPrefs('');
}
