import { DEFAULT_SKIN_SETTING, type SkinSetting } from '../components/SkinSection';
import type { LauncherSettings } from './native';

export const SETTINGS_STORAGE_KEY = 'flowlaunch-settings';

export type ThemeMode = 'light' | 'dark' | 'system';

export interface LauncherPrefs {
  defaultVersionIsolation: boolean; gameResourceLocation: string; skin: SkinSetting; launcherVisibility: 'keep' | 'hide' | 'close'; processPriority: 'normal' | 'high' | 'low'; windowPreset: 'default' | 'custom';
  fullscreen: boolean; gameArgs: string; preLaunchCommand: string; memoryMode: 'auto' | 'custom'; optimizeMemory: boolean; launchWrapperDisabled: boolean; lwjglUnsafeAgentDisabled: boolean; hardwareAcceleration: boolean;
  proxyMode: 'none' | 'system' | 'http'; proxyUrl: string; downloadTimeoutSec: number; downloadRetries: number; speedLimitKbps: number; verifySsl: boolean; downloadDirectory: string;
  debugMode: boolean; showCommandPreview: boolean; safeMode: boolean; logLevel: 'error' | 'warn' | 'info' | 'debug'; environmentVariables: string; clearFinishedTasks: boolean;
  communitySource: string; modpackFilenameFormat: string; modpackMergeMode: 'ask' | 'replace' | 'merge'; showGameHints: boolean; showDebugHints: boolean;
  theme: ThemeMode; themePreset: string; customAccent: string; showGameLog: boolean;
  javaPath: string; memoryMb: number; windowWidth: number; windowHeight: number; extraJvmArgs: string;
  downloadSource: string; maxConcurrentDownloads: number; connectionsPerFile: number; aria2Enabled: boolean;
  closeAfterLaunch: boolean; checkUpdates: boolean; gameDirectory: string; announcementPolicy: 'notify' | 'off'; cacheDirectory: string; launcherLogEnabled: boolean; llamaServerPath: string; opacity: number; hiddenTheme: boolean; showLauncherLogo: boolean; backgroundImage: string; backgroundMusic: boolean; titleBar: 'default' | 'text' | 'image' | 'none'; homeSource: string;
}

export const DEFAULT_PREFS: LauncherPrefs = {
  defaultVersionIsolation: true, gameResourceLocation: 'default', skin: { ...DEFAULT_SKIN_SETTING }, launcherVisibility: 'keep', processPriority: 'normal', windowPreset: 'default',
  fullscreen: false, gameArgs: '', preLaunchCommand: '', memoryMode: 'custom', optimizeMemory: false, launchWrapperDisabled: false, lwjglUnsafeAgentDisabled: false, hardwareAcceleration: true,
  debugMode: false, showCommandPreview: false, safeMode: false, logLevel: 'info', environmentVariables: '', clearFinishedTasks: true,
  proxyMode: 'none', proxyUrl: '', downloadTimeoutSec: 60, downloadRetries: 3, speedLimitKbps: 0, verifySsl: true, downloadDirectory: '',
  communitySource: 'official', modpackFilenameFormat: '{name}-{version}', modpackMergeMode: 'ask', showGameHints: true, showDebugHints: false,
  theme: 'dark', themePreset: 'ocean', customAccent: '', showGameLog: true,
  javaPath: '', memoryMb: 4096, windowWidth: 1280, windowHeight: 720, extraJvmArgs: '',
  // 'auto' → 后端 `mixed`：镜像优先、官方回退（详见 native.ts 的 prefsToSettings）。
  // 默认选它是因为官方 CDN 在部分地区很慢，而镜像只在有校验值时才用，不牺牲完整性。
  downloadSource: 'auto', maxConcurrentDownloads: 16, connectionsPerFile: 256, aria2Enabled: true,
  closeAfterLaunch: false, checkUpdates: true, gameDirectory: '', announcementPolicy: 'notify', cacheDirectory: '', launcherLogEnabled: false, llamaServerPath: '', opacity: 100, hiddenTheme: false, showLauncherLogo: true, backgroundImage: '', backgroundMusic: false, titleBar: 'default', homeSource: 'official',
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
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(SETTINGS_STORAGE_KEY) || '{}');
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) saved = parsed as StoredPrefs;
  } catch { /* use defaults */ }
  // 旧版本存过一个没有实际含义的皮肤下拉字段。它已经被离线皮肤取代，这里
  // 显式丢弃，避免把废弃字段继续写回 localStorage。
  const { customSkin: legacyFieldToDrop, ...rest } = saved;
  void legacyFieldToDrop;
  return {
    ...DEFAULT_PREFS,
    ...rest,
    ...(!saved.customAccent && currentAccent ? { customAccent: currentAccent } : {}),
    skin: normalizeSkin(rest.skin),
  };
}

/** 将已确认的偏好复制为独立草稿，避免嵌套皮肤对象共享引用。 */
export function copyPrefs(prefs: LauncherPrefs): LauncherPrefs {
  return { ...prefs, skin: normalizeSkin(prefs.skin) };
}

export function prefsEqual(left: LauncherPrefs, right: LauncherPrefs): boolean {
  return (Object.keys(DEFAULT_PREFS) as (keyof LauncherPrefs)[]).every(key => key === 'skin'
    ? JSON.stringify(normalizeSkin(left.skin)) === JSON.stringify(normalizeSkin(right.skin))
    : Object.is(left[key], right[key]));
}

/** 只应在后端确认后调用；存储失败交给调用方显示，不能伪装为保存成功。 */
export function savePrefs(prefs: LauncherPrefs): void {
  localStorage.setItem(SETTINGS_STORAGE_KEY, JSON.stringify(copyPrefs(prefs)));
}

/** 后端字段覆盖本地缓存；仅前端持有的字段（如皮肤）保留已确认的值。 */
export function prefsFromSettings(settings: LauncherSettings, current: LauncherPrefs = readStoredPrefs()): LauncherPrefs {
  return {
    ...copyPrefs(current),
    theme: settings.themeMode,
    themePreset: settings.themePreset,
    customAccent: settings.customAccent,
    backgroundImage: settings.backgroundImage ?? settings.customBackground ?? '',
    showGameLog: settings.showGameLog,
    javaPath: settings.javaPath ?? '',
    memoryMb: settings.memoryMb,
    memoryMode: settings.memoryMode,
    optimizeMemory: settings.optimizeMemory,
    windowWidth: settings.windowWidth,
    windowHeight: settings.windowHeight,
    extraJvmArgs: settings.extraJvmArgs,
    gameArgs: settings.gameArgs,
    preLaunchCommand: settings.preLaunchCommand,
    fullscreen: settings.fullscreen,
    processPriority: settings.processPriority,
    launcherVisibility: settings.launcherVisibility,
    closeAfterLaunch: settings.closeAfterLaunch,
    defaultVersionIsolation: settings.defaultIsolated,
    speedLimitKbps: settings.speedLimitKbps,
    downloadDirectory: settings.downloadDirectory ?? '',
    verifySsl: settings.verifySsl,
    communitySource: settings.communitySource,
    modpackFilenameFormat: settings.modpackFilenameFormat,
    modpackMergeMode: settings.modpackMergeMode,
    showGameHints: settings.showGameHints,
    showDebugHints: settings.showDebugHints,
    announcementPolicy: settings.announcementPolicy,
    cacheDirectory: settings.cacheDirectory ?? '',
    gameDirectory: settings.gameRoot ?? '',
    launcherLogEnabled: settings.launcherLogEnabled,
    clearFinishedTasks: settings.clearFinishedTasks,
    llamaServerPath: settings.llamaServerPath ?? '',
    opacity: settings.opacity,
    hiddenTheme: settings.hiddenTheme,
    showLauncherLogo: settings.showLauncherLogo,
    backgroundMusic: settings.backgroundMusic,
    titleBar: settings.titleBar,
    homeSource: settings.homeSource,
    checkUpdates: settings.updatePolicy === 'notify',
    downloadSource: settings.download.source === 'mixed' ? 'auto' : settings.download.source,
    maxConcurrentDownloads: settings.download.maxConcurrentDownloads,
    connectionsPerFile: settings.download.connectionsPerFile,
    aria2Enabled: settings.download.aria2Enabled,
    proxyMode: settings.download.proxyMode,
    proxyUrl: settings.download.proxyUrl,
    downloadTimeoutSec: settings.download.timeoutSec,
    downloadRetries: settings.download.retries,
  };
}

/** 从存档里读出全局设置，不访问 getComputedStyle（可在任何环境调用）。 */
export function readStoredPrefs(): LauncherPrefs {
  return readPrefs('');
}
