// 桌面后端桥接层。
//
// 这里是界面与 Rust 后端之间唯一的通道。所有 `invoke` 调用都必须经过本文件，
// 原因有两个：
//
// 1. **类型对照**。下面的接口逐一对应 `crates/launcher-core/src/model.rs` 与
//    `src-tauri/src/lib.rs` / `management.rs` 里的 serde 结构。集中在一处，
//    后端加字段时只需要改这一个文件，而不是满项目找 `invoke<string>`。
// 2. **预览模式**。`vite dev` 也能在普通浏览器里打开，此时没有 Tauri 运行时。
//    预览模式没有后端，但界面不能整个白屏——所以本地存储里读一份设置做兜底，
//    而真正需要后端的行为（启动、安装、登录）会明确抛错，而不是假装成功。
//
// 关于命令参数命名：Tauri v2 会把命令的 snake_case 参数在 JS 侧暴露成
// camelCase（`instance_id` → `instanceId`）。这里的实现按 camelCase 传参。

import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { open } from '@tauri-apps/plugin-dialog'
import type { InstanceOverrides as UiInstanceOverrides } from './instanceOverrides'
import type { LauncherPrefs } from './launcherPrefs'
import { PREVIEW_DATA } from './preview'
import { autoAllocateMb, deviceMemoryMbSync, lastKnownUsedMb } from './memory'

// ---------------------------------------------------------------------------
// 与 Rust 结构对应的类型
// ---------------------------------------------------------------------------

export type ThemeMode = 'light' | 'dark' | 'system'
export type ThemePreset = 'moss' | 'ember' | 'lavender' | 'ocean' | 'custom'
export type MemoryMode = 'auto' | 'custom'
export type GarbageCollector = 'auto' | 'g1' | 'serial' | 'parallel'
export type LauncherVisibility = 'keep' | 'hide'
export type ProcessPriority = 'normal' | 'high' | 'low'
export type TitleBarMode = 'default' | 'text' | 'image' | 'none'
export type ModpackMergeMode = 'ask' | 'replace' | 'merge'
export type UpdatePolicy = 'notify' | 'off'
export type DownloadSource = 'official' | 'bmclapi' | 'mixed'
export type LoaderKind = 'fabric' | 'quilt' | 'forge' | 'neoforge'
export type InstanceState = 'not_installed' | 'installing' | 'ready' | 'failed'
export type AccountKind = 'microsoft' | 'offline' | 'skin_server'
export type DownloadTaskState = 'running' | 'paused' | 'completed' | 'cancelled' | 'failed' | 'interrupted'

/** 对应 Rust `DownloadSettings`（嵌套在 `LauncherSettings.download` 里）。 */
export interface DownloadSettings {
  source: DownloadSource
  maxConcurrentDownloads: number
  connectionsPerFile: number
  aria2Enabled: boolean
}

/**
 * 对应 Rust `LauncherSettings`。
 *
 * 这里保留后端会反序列化的全部字段，哪怕是界面暂时不展示的：
 * `save_settings` 接收的是完整结构，少传一个字段就会被 serde 用默认值覆盖，
 * 把用户之前存的值悄悄抹掉。界面不展示的字段原样回传即可。
 */
export interface LauncherSettings {
  schemaVersion: number
  themeMode: ThemeMode
  themePreset: ThemePreset
  customAccent: string
  customBackground: string | null
  beginnerMode: boolean
  showGameLog: boolean
  javaPath: string | null
  memoryMb: number
  memoryMode: MemoryMode
  windowWidth: number
  windowHeight: number
  extraJvmArgs: string
  gameArgs: string
  preLaunchCommand: string
  garbageCollector: GarbageCollector
  optimizeMemory: boolean
  processPriority: ProcessPriority
  fullscreen: boolean
  launcherVisibility: LauncherVisibility
  defaultIsolated: boolean
  closeAfterLaunch: boolean
  speedLimitKbps: number
  downloadDirectory: string | null
  verifySsl: boolean
  communitySource: string
  modpackFilenameFormat: string
  modpackMergeMode: ModpackMergeMode
  showGameHints: boolean
  showDebugHints: boolean
  autoLanguage: boolean
  announcementPolicy: UpdatePolicy
  cacheDirectory: string | null
  anonymousUsage: boolean
  opacity: number
  hiddenTheme: boolean
  showLauncherLogo: boolean
  backgroundImage: string | null
  backgroundMusic: boolean
  titleBar: TitleBarMode
  homeSource: string
  updatePolicy: UpdatePolicy
  download: DownloadSettings
}

/** 对应 Rust `InstanceRecord`。 */
export interface InstanceRecord {
  id: string
  name: string
  gameVersion: string
  loader: string
  instanceDir: string
  gameDir: string
  isolated: boolean
  state: InstanceState
  createdAt: string
  /**
   * 上次成功启动的时间（RFC 3339），从未启动过为 null。
   *
   * 后端按此排序（最近启动的在前，从未启动的按创建时间倒序），所以实例数组的
   * 第一项就是「上次启动的实例」。
   */
  lastLaunchedAt: string | null
}

/**
 * 对应 Rust `InstanceOverrides`：缺省字段表示「继承全局」。
 *
 * 形状必须与后端一致，因为 `save_instance_settings` 会逐字段校验；多传一个
 * 后端不认识的键会被 serde 的 `deny_unknown_fields`（若有）或静默忽略，两种
 * 情况都会让界面显示的覆盖数和实际保存的不一致。
 */
export interface InstanceOverrides {
  memoryMode?: MemoryMode | null
  memoryMb?: number | null
  /**
   * 启动前进行内存优化。
   *
   * 界面一直有这个开关，但类型与 Rust `InstanceOverrides` 都缺这一项——结果
   * 是保存时字段被丢弃、启动时静默使用全局值。两端现已同步。
   */
  optimizeMemory?: boolean | null
  fullscreen?: boolean | null
  showGameLog?: boolean | null
  windowWidth?: number | null
  windowHeight?: number | null
  extraJvmArgs?: string | null
  gameArgs?: string | null
  preLaunchCommand?: string | null
}

/** 对应 Rust `InstanceSettingsView`：原始覆盖表 + 全局值 + 合并后的生效值。 */
export interface InstanceSettingsView {
  overrides: InstanceOverrides
  global: LauncherSettings
  resolved: LauncherSettings
  customized: number
}

/** 对应 Rust `AccountProfile`。凭据本身从不经过 IPC。 */
export interface AccountProfile {
  id: string
  displayName: string
  kind: AccountKind
  authServer: string | null
  selected: boolean
  createdAt: string
}

/** 对应 Rust `Aria2Availability`。 */
export interface Aria2Availability {
  /** 本构建是否为这个平台附带并校验通过了 aria2。 */
  bundled: boolean
  path: string | null
  version: string | null
  /** 不可用的原因；可用时为 null。 */
  problem: string | null
}

/**
 * 查询 aria2 是否真正可用。
 *
 * 用户打开「使用 aria2 加速」后，界面需要能回答「它到底生效了没有」。之前这个
 * 信息只写在后端 stderr 里，用户看不到；开关看起来可用、实际走的是原生下载器。
 * 浏览器预览没有后端，直接返回不可用并说明原因。
 */
export async function aria2Status(): Promise<Aria2Availability> {
  if (!isDesktopShell()) {
    return {
      bundled: false,
      path: null,
      version: null,
      problem: '浏览器预览没有后端进程，无法加载 aria2；桌面版会校验随包附带的二进制。',
    }
  }
  return invoke<Aria2Availability>('aria2_status')
}

/** 对应 Rust `SystemMemoryInfo`。 */
export interface SystemMemoryInfo {
  totalMb: number
  usedMb: number | null
  availableMb: number | null
  recommendedMb: number
  maximumGameMb: number
}

/** 对应 Rust `JavaRuntimeInfo`。 */
export interface JavaRuntimeInfo {
  path: string
  majorVersion: number
  versionText: string
}

/** 对应 Rust `VersionListItem`。 */
export interface VersionListItem {
  id: string
  type: 'release' | 'snapshot' | 'old_beta' | 'old_alpha'
  releaseTime: string
}

/** 对应 Rust `DownloadTask`。 */
export interface DownloadTask {
  id: string
  instanceId: string
  label: string
  source: string | null
  state: DownloadTaskState
  completedBytes: number
  totalBytes: number | null
  createdAt: string
  updatedAt: string
}

/** 对应 Rust `DownloadProgress`（`download-progress` 事件载荷）。 */
export interface DownloadProgress {
  artifactId: string
  label: string
  received: number
  total: number | null
  source: string
  stage: 'connecting' | 'downloading' | 'verifying' | 'completed' | 'failed'
}

/** 对应 Rust `TaskEvent`（`installation-task` 事件载荷）。 */
export interface TaskEvent {
  instanceId: string
  task: DownloadTask
}

/** 对应 Rust `GameLogLine`（`game-log-line` 事件载荷）。 */
export interface GameLogLine {
  instanceId: string
  line: string
}

/** 对应 Rust `GameProcessEvent`（`game-process` 事件载荷）。 */
export interface GameProcessEvent {
  instanceId: string
  status: 'running' | 'stopped' | 'crashed'
  exitCode: number | null
  logPath: string
}

/** 对应 Rust `LaunchProgressEvent`。`progress` 是 0–1 的整体比例。 */
export interface LaunchProgressEvent {
  instanceId: string
  /** `preparing` / `authenticating` / `building` / `starting` / `running`。 */
  stage: string
  progress: number
}

/** 对应 Rust `SkinLoginResult`。只含公开档案信息，不含密码。 */
export interface SkinLoginResult {
  pendingId: string
  profiles: { id: string; name: string }[]
}

/** 对应 Rust `LauncherUpdateStatus`。 */
export interface LauncherUpdateStatus {
  current: string
  latest: string | null
  notes: string
  available: boolean
  url: string | null
  sha256: string | null
  size: number | null
}

/** 对应 Rust `CatalogMod`（Modrinth 搜索结果）。 */
export interface CatalogMod {
  projectId: string
  title: string
  description: string
  author: string
  downloads: number
  follows: number
  iconUrl: string | null
}

/**
 * 一次模组搜索的条件，对应 Rust `catalog::ModQuery`。
 *
 * 所有字段都可省略：省略即「不限制」。空 `query` 是合法的，
 * 表示按下载量浏览该版本与加载器下的模组，而不是报错。
 */
export interface ModQuery {
  query?: string
  /**
   * 内容类型：`mod` / `resourcepack` / `shader` / `datapack` / `modpack`。
   * 省略等价于 `mod`。它决定 Modrinth 的 project_type facet、是否按加载器筛选、
   * 以及文件装到 `mods/` 还是 `resourcepacks/`。
   */
  projectType?: 'mod' | 'resourcepack' | 'shader' | 'datapack' | 'modpack'
  gameVersion?: string
  loader?: string
  category?: string
  index?: 'relevance' | 'downloads' | 'newest' | 'updated' | 'follows'
  offset?: number
}

/** 对应 Rust `CatalogVersion`。`sha1` 在安装时由后端重新校验。 */
export interface CatalogVersion {
  versionId: string
  versionNumber: string
  dateCreated: string
  fileName: string
  url: string
  sha1: string
  size: number
}

/** 对应 Rust `ModFile`。 */
export interface ModFile {
  fileName: string
  enabled: boolean
  size: number
}

/** 对应 Rust `ModIssueSeverity`（serde snake_case）。 */
export type ModIssueSeverity = 'warning' | 'error'

/** 对应 Rust `ModIssue`：只指向具体文件，不推断模组名。 */
export interface ModIssue {
  severity: ModIssueSeverity
  code: string
  message: string
  files: string[]
}

/** 对应 Rust `ModUpdate`：已确认可用的更新目标。 */
export interface ModUpdate {
  fileName: string
  projectId: string
  installedVersion: string
  installedVersionId: string
  newVersion: string
  newVersionId: string
  newFileName: string
  url: string
  sha1: string
  size: number
}

/**
 * 对应 Rust `ModUpdateStatus`。
 *
 * `serde` 用外部标记枚举：`{"up_to_date": null}` /
 * `{"update_available": {...}}` / `"unknown_source"` / `{"unsupported": "..."}`。
 * 这里写成判别联合的前三种 + 字符串式，解析在 `parseModUpdateStatus` 里做，
 * 因为序列化形状不是接口能直接表达的。
 */
export type ModUpdateStatus =
  | { kind: 'upToDate' }
  | { kind: 'updateAvailable'; update: ModUpdate }
  | { kind: 'unknownSource' }
  | { kind: 'unsupported'; reason: string }

/** 对应 Rust `ModUpdateReport`。`status` 在线上是上面那种枚举形状。 */
export interface ModUpdateReport {
  fileName: string
  status: ModUpdateStatus
}

/** 对应 Rust `ClientUpdateStatus`。 */
/**
 * 客户端更新检查结果，对应 Rust `ClientUpdateStatus`。
 *
 * 三个数分开而不是给一个布尔值：用户需要知道「要做多少事」。
 *   - `added`   官方清单新增的文件（多半是 Mojang 补发了依赖）；
 *   - `removed` 本地有、清单里没了；
 *   - `changed` 同一文件的大小或校验值变了。
 */
export interface ClientUpdateStatus {
  upToDate: boolean
  added: number
  removed: number
  changed: number
}

/** 对应 Rust `PackPreview`（整合包预览）。 */
export interface PackPreview {
  name: string
  version: string
  minecraft: string
  loader: string
  requiredFiles: number
  optionalFiles: number
  totalBytes: number
  overrideFiles: number
}

/** 对应 Rust `LoaderVersion`。 */
export interface LoaderVersion {
  version: string
  stable: boolean
}

/** 对应 Rust `BackupPreview`。 */
export interface BackupPreview {
  name: string
  minecraft: string
  createdAt: string
  sha256: string
  files: number
  totalBytes: number
}

/**
 * 一条命中的崩溃原因。
 *
 * 与 Rust `DiagnosticReason` 对应。崩溃往往同时满足多个特征（内存不足会连带触发
 * Mixin 失败），只报一条容易把人引向错误方向，所以后端返回全部命中项。
 */
export interface DiagnosticReason {
  title: string
  /** `high` / `medium`。 */
  confidence: string
  explanation: string
  /** 可执行的下一步；空表示只能人工排查。 */
  suggestions: string[]
  evidence: string[]
  /** 从日志里提取出的模组名。 */
  mods: string[]
}

/** 对应 Rust `DiagnosticSummary`。 */
export interface DiagnosticSummary {
  title: string
  confidence: string
  explanation: string
  evidence: string[]
  filesFound: number
  /** 全部命中的原因，按置信度从高到低。 */
  reasons: DiagnosticReason[]
}

// ---------------------------------------------------------------------------
// 运行时检测
// ---------------------------------------------------------------------------

/**
 * 是否运行在 Tauri 桌面外壳内。
 *
 * `__TAURI_INTERNALS__` 由 Tauri 的初始化脚本注入，比检查 `window.__TAURI__`
 * 可靠：后者取决于 `withGlobalTauri` 配置，而这个标志始终存在。
 */
export function isDesktopShell(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

/** 预览模式下调用的统一错误；带上具体操作名，方便用户知道是哪一步需要桌面版。 */
export class NativeUnavailableError extends Error {
  constructor(operation: string) {
    super(`“${operation}”需要桌面应用后端；浏览器里只能预览界面。`)
    this.name = 'NativeUnavailableError'
  }
}

function requireDesktop(operation: string): void {
  if (!isDesktopShell()) throw new NativeUnavailableError(operation)
}

// ---------------------------------------------------------------------------
// 设置
// ---------------------------------------------------------------------------

export const SETTINGS_FALLBACK_KEY = 'mfmcl-preview-settings'

/**
 * 设置的第一帧兜底值。
 *
 * 数值与 Rust `LauncherSettings::default()` 保持一致（2 GB / 854×480），
 * 这样预览模式和后端首次启动看到的是同一套默认值，不会出现「预览说 4 GB、
 * 真机是 2 GB」。
 */
export function fallbackSettings(): LauncherSettings {
  return {
    schemaVersion: 3,
    themeMode: 'dark',
    themePreset: 'ocean',
    customAccent: '#53b4f6',
    customBackground: null,
    beginnerMode: false,
    showGameLog: true,
    javaPath: null,
    memoryMb: 2048,
    memoryMode: 'custom',
    windowWidth: 854,
    windowHeight: 480,
    extraJvmArgs: '',
    gameArgs: '',
    preLaunchCommand: '',
    garbageCollector: 'auto',
    optimizeMemory: false,
    processPriority: 'normal',
    fullscreen: false,
    launcherVisibility: 'keep',
    defaultIsolated: true,
    closeAfterLaunch: false,
    speedLimitKbps: 0,
    downloadDirectory: null,
    verifySsl: true,
    communitySource: 'official',
    modpackFilenameFormat: '{name}-{version}',
    modpackMergeMode: 'ask',
    showGameHints: true,
    showDebugHints: false,
    autoLanguage: true,
    announcementPolicy: 'notify',
    cacheDirectory: null,
    anonymousUsage: false,
    opacity: 100,
    hiddenTheme: false,
    showLauncherLogo: true,
    backgroundImage: null,
    backgroundMusic: false,
    titleBar: 'default',
    homeSource: 'official',
    updatePolicy: 'notify',
    download: { source: 'mixed', maxConcurrentDownloads: 4, connectionsPerFile: 4, aria2Enabled: true },
  }
}

/** 读取设置。桌面版走后端数据库；预览模式读 localStorage，永不自动写回。 */
export async function getSettings(): Promise<LauncherSettings> {
  if (!isDesktopShell()) return readPreviewSettings()
  return invoke<LauncherSettings>('get_settings')
}

/** 保存设置。后端会做范围校验，非法值会被拒绝并抛出可读错误。 */
export async function saveSettings(settings: LauncherSettings): Promise<void> {
  if (!isDesktopShell()) {
    writePreviewSettings(settings)
    return
  }
  await invoke('save_settings', { settings })
}

/**
 * 界面偏好（`LauncherPrefs`） → 后端设置（`LauncherSettings`）。
 *
 * 为什么需要这个方向：设置面板改的是 `LauncherPrefs`，而启动器真正读取的是
 * `LauncherSettings`。两者字段名与取值域不同，必须显式翻译——之前设置面板只把
 * 偏好写进 localStorage 并往 localStorage 里塞一份「原生设置草稿」，草稿没有任何
 * 代码读取，后端从未收到新值。用户看到「已保存」，重启后一切照旧。
 *
 * `base` 是上一次从后端读到的完整设置。必须基于它展开而不是从默认值构造：
 * 界面不展示的字段（schemaVersion、garbageCollector 等）如果被默认值覆盖，
 * 保存一次就会把这些用户看不见的设置重置掉。
 *
 * 三个值后端会拒绝或没有对应概念，因此固定成唯一允许的取值：
 *   - `processPriority`：后端只接受 normal（高/低优先级尚未实现）；
 *   - `closeAfterLaunch`：与 `launcherVisibility` 重叠，统一由后者表达；
 *   - `launcherVisibility` 的 'close' → 后端用 closeAfterLaunch 表达。
 */
export function prefsToSettings(prefs: LauncherPrefs, base: LauncherSettings): LauncherSettings {
  return {
    ...base,
    themeMode: prefs.theme,
    themePreset: prefs.themePreset as ThemePreset,
    customAccent: prefs.customAccent || base.customAccent,
    customBackground: prefs.backgroundImage || null,
    backgroundImage: prefs.backgroundImage || null,
    showGameLog: prefs.showGameLog,
    javaPath: prefs.javaPath || null,
    memoryMb: prefs.memoryMode === 'auto'
      ? autoAllocateMb({ totalMb: deviceMemoryMbSync(), usedMb: lastKnownUsedMb() })
      : prefs.memoryMb,
    memoryMode: prefs.memoryMode,
    optimizeMemory: prefs.optimizeMemory,
    windowWidth: prefs.windowWidth,
    windowHeight: prefs.windowHeight,
    extraJvmArgs: prefs.extraJvmArgs,
    gameArgs: prefs.gameArgs,
    preLaunchCommand: prefs.preLaunchCommand,
    fullscreen: prefs.fullscreen,
    launcherVisibility: prefs.launcherVisibility === 'close' ? 'keep' : prefs.launcherVisibility,
    closeAfterLaunch: prefs.launcherVisibility === 'close' || prefs.closeAfterLaunch,
    defaultIsolated: prefs.defaultVersionIsolation,
    speedLimitKbps: prefs.speedLimitKbps,
    downloadDirectory: prefs.downloadDirectory || null,
    verifySsl: prefs.verifySsl,
    communitySource: prefs.communitySource,
    modpackFilenameFormat: prefs.modpackFilenameFormat,
    modpackMergeMode: prefs.modpackMergeMode,
    showGameHints: prefs.showGameHints,
    showDebugHints: prefs.showDebugHints,
    announcementPolicy: prefs.announcementPolicy as UpdatePolicy,
    cacheDirectory: prefs.cacheDirectory || null,
    anonymousUsage: prefs.anonymousUsage,
    opacity: prefs.opacity,
    hiddenTheme: prefs.hiddenTheme,
    showLauncherLogo: prefs.showLauncherLogo,
    backgroundMusic: prefs.backgroundMusic,
    titleBar: prefs.titleBar as TitleBarMode,
    homeSource: prefs.homeSource,
    updatePolicy: prefs.checkUpdates ? 'notify' : 'off',
    processPriority: 'normal',
    download: {
      ...base.download,
      // 界面用 'auto' 表示「自动挑选可用源」，后端对应 'mixed'；其余取值同名。
      source: prefs.downloadSource === 'auto' ? 'mixed' : prefs.downloadSource as DownloadSource,
      maxConcurrentDownloads: prefs.maxConcurrentDownloads,
      connectionsPerFile: prefs.connectionsPerFile,
      aria2Enabled: prefs.aria2Enabled,
    },
  }
}

function readPreviewSettings(): LauncherSettings {
  const base = fallbackSettings()
  try {
    const raw = localStorage.getItem(SETTINGS_FALLBACK_KEY)
    if (!raw) return base
    const parsed = JSON.parse(raw) as Partial<LauncherSettings>
    // 逐层合并：老存档可能没有 `download`，直接展开会把整个对象变成 undefined。
    return { ...base, ...parsed, download: { ...base.download, ...parsed.download } }
  } catch {
    return base
  }
}

function writePreviewSettings(settings: LauncherSettings): void {
  try {
    // 背景图可能有几 MB，预览模式没有必要每次保存都重复写一份。
    localStorage.setItem(SETTINGS_FALLBACK_KEY, JSON.stringify({ ...settings, customBackground: null }))
  } catch {
    // 存储被禁用时保持内存中的状态即可。
  }
}

// ---------------------------------------------------------------------------
// 实例
// ---------------------------------------------------------------------------

/**
 * 预览模式的示例实例。
 *
 * 浏览器里没有 Rust 后端，`list_instances` 无从调用。之前这里直接返回空数组，
 * 结果是游戏库永远是「还没有游戏」空状态——而游戏库恰恰是最需要调整外观的
 * 页面之一。给一组覆盖所有加载器的示例实例，让布局能在预览里被看到和修改。
 *
 * 这些是示例数据，不是真实实例：id 带 `preview-` 前缀，任何会改动后端的操作
 * （安装、删除、启动）都会先撞上 `requireDesktop` 并如实报错。
 */
export const PREVIEW_INSTANCES: InstanceRecord[] = [
  {
    id: 'preview-vanilla',
    name: '纯净生存 1.21.1',
    gameVersion: '1.21.1',
    loader: 'vanilla',
    instanceDir: '/preview/instances/vanilla-1.21.1',
    gameDir: '/preview/instances/vanilla-1.21.1/.minecraft',
    isolated: true,
    state: 'ready',
    createdAt: '2026-09-02T10:12:00Z',
    lastLaunchedAt: '2026-10-28T21:40:00Z',
  },
  {
    id: 'preview-fabric',
    name: 'Fabric 优化整合',
    gameVersion: '1.21.1',
    loader: 'fabric',
    instanceDir: '/preview/instances/fabric-1.21.1',
    gameDir: '/preview/instances/fabric-1.21.1/.minecraft',
    isolated: true,
    state: 'ready',
    createdAt: '2026-09-14T08:30:00Z',
    lastLaunchedAt: '2026-10-27T19:12:00Z',
  },
  {
    id: 'preview-neoforge',
    name: 'NeoForge 科技包',
    gameVersion: '1.20.1',
    loader: 'neoforge',
    instanceDir: '/preview/instances/neoforge-1.20.1',
    gameDir: '/preview/instances/neoforge-1.20.1/.minecraft',
    isolated: true,
    state: 'ready',
    createdAt: '2026-09-21T19:05:00Z',
    lastLaunchedAt: '2026-10-25T15:30:00Z',
  },
  {
    id: 'preview-forge',
    name: 'Forge 冒险整合',
    gameVersion: '1.19.2',
    loader: 'forge',
    instanceDir: '/preview/instances/forge-1.19.2',
    gameDir: '/preview/instances/forge-1.19.2/.minecraft',
    isolated: true,
    state: 'ready',
    createdAt: '2026-08-30T14:40:00Z',
    lastLaunchedAt: '2026-09-18T20:05:00Z',
  },
  {
    id: 'preview-quilt',
    name: 'Quilt 试验场',
    gameVersion: '1.20.4',
    loader: 'quilt',
    instanceDir: '/preview/instances/quilt-1.20.4',
    gameDir: '/preview/instances/quilt-1.20.4/.minecraft',
    isolated: true,
    state: 'installing',
    createdAt: '2026-10-26T09:00:00Z',
    lastLaunchedAt: null,
  },
  {
    id: 'preview-broken',
    name: '整合包（安装失败）',
    gameVersion: '1.18.2',
    loader: 'vanilla',
    instanceDir: '/preview/instances/broken-1.18.2',
    gameDir: '/preview/instances/broken-1.18.2/.minecraft',
    isolated: true,
    state: 'failed',
    createdAt: '2026-10-01T16:22:00Z',
    lastLaunchedAt: null,
  },
];

export async function listInstances(): Promise<InstanceRecord[]> {
  if (!isDesktopShell()) {
    // 示例数据只在开发预览里存在；生产构建中 PREVIEW_DATA 为编译期 false，
    // 下面这整个分支（连同 PREVIEW_INSTANCES）会被 tree-shaking 移除。
    if (!PREVIEW_DATA) return []
    // 按后端的排序规则排列：最近启动的在前，从未启动的按创建时间倒序。
    // 不排序的话，预览里「默认目标实例」的取值与真机不一致，这条逻辑就没法在
    // 浏览器里验证。
    return [...PREVIEW_INSTANCES]
      .sort((a, b) => {
        if (a.lastLaunchedAt && !b.lastLaunchedAt) return -1;
        if (!a.lastLaunchedAt && b.lastLaunchedAt) return 1;
        if (a.lastLaunchedAt && b.lastLaunchedAt) {
          return b.lastLaunchedAt.localeCompare(a.lastLaunchedAt);
        }
        return b.createdAt.localeCompare(a.createdAt);
      })
      .map(record => ({ ...record }))
  }
  return invoke<InstanceRecord[]>('list_instances')
}

export async function createInstance(name: string, version: string): Promise<InstanceRecord> {
  requireDesktop('创建实例')
  return invoke<InstanceRecord>('create_instance', { name, version })
}
export async function installInstance(instanceId: string): Promise<void> {
  requireDesktop('安装游戏文件')
  await invoke('install_instance', { instanceId })
}

export async function deleteInstance(instanceId: string, deleteFiles: boolean): Promise<void> {
  requireDesktop('删除实例')
  await invoke('delete_instance', { instanceId, deleteFiles })
}

/**
 * 预览模式的实例覆盖表。
 *
 * 浏览器里没有后端数据库，所以存进 localStorage。键与桌面版无关，只是为了让
 * 实例设置页在预览里能真正看到「已覆盖 / 跟随全局」两种状态——否则页面永远是
 * 一份全继承的清单，覆盖态的样式根本没法调。
 */
const PREVIEW_OVERRIDES_KEY = 'mfmcl-preview-instance-overrides'

function readPreviewOverrides(): Record<string, InstanceOverrides> {
  try {
    const raw = localStorage.getItem(PREVIEW_OVERRIDES_KEY)
    const parsed = raw ? JSON.parse(raw) as unknown : null
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return {}
    return parsed as Record<string, InstanceOverrides>
  } catch {
    return {}
  }
}

function writePreviewOverrides(map: Record<string, InstanceOverrides>): void {
  try {
    localStorage.setItem(PREVIEW_OVERRIDES_KEY, JSON.stringify(map))
  } catch {
    // 存储被禁用时保持内存状态即可。
  }
}

/**
 * 合并全局设置与实例覆盖，得到这次启动真正生效的值。
 *
 * 与 Rust `resolve_settings` 保持同一套字段与优先级。这里只用于预览模式的显示，
 * 桌面版由后端算——但两边必须在「哪些字段可被覆盖」上一致，否则预览里看到的
 * 生效值和真机不同。
 */
function resolvePreview(global: LauncherSettings, overrides: InstanceOverrides): LauncherSettings {
  const resolved: LauncherSettings = { ...global }
  if (overrides.memoryMode != null) resolved.memoryMode = overrides.memoryMode
  if (overrides.memoryMb != null) resolved.memoryMb = overrides.memoryMb
  if (overrides.optimizeMemory != null) resolved.optimizeMemory = overrides.optimizeMemory
  if (overrides.fullscreen != null) resolved.fullscreen = overrides.fullscreen
  if (overrides.showGameLog != null) resolved.showGameLog = overrides.showGameLog
  if (overrides.windowWidth != null) resolved.windowWidth = overrides.windowWidth
  if (overrides.windowHeight != null) resolved.windowHeight = overrides.windowHeight
  if (overrides.extraJvmArgs != null) resolved.extraJvmArgs = overrides.extraJvmArgs
  if (overrides.gameArgs != null) resolved.gameArgs = overrides.gameArgs
  if (overrides.preLaunchCommand != null) resolved.preLaunchCommand = overrides.preLaunchCommand
  return resolved
}

/** 与 Rust `count_instance_overrides` 同一套计数规则（内存两项算一项、空文本不计）。 */
function countPreviewOverrides(overrides: InstanceOverrides): number {
  let count = 0
  if (overrides.memoryMode != null || overrides.memoryMb != null) count += 1
  if (overrides.optimizeMemory != null) count += 1
  if (overrides.fullscreen != null) count += 1
  if (overrides.showGameLog != null) count += 1
  if (overrides.windowWidth != null || overrides.windowHeight != null) count += 1
  if (overrides.extraJvmArgs && overrides.extraJvmArgs.trim()) count += 1
  if (overrides.gameArgs && overrides.gameArgs.trim()) count += 1
  if (overrides.preLaunchCommand && overrides.preLaunchCommand.trim()) count += 1
  return count
}

/**
 * 后端形状（`T | null`）→ 界面形状（`T | undefined`）。
 *
 * 两种形状必须显式转换而不是强转：Rust 的 `Option<T>` 序列化成 `null`，而界面
 * 的覆盖表用 `undefined` 表示「未设置」。混用会让 `Object.keys()` 统计出多余的
 * 键（`null` 也是值），界面就会把「跟随全局」误显示成「已自定义」。
 */
export function toUiOverrides(overrides: InstanceOverrides): UiInstanceOverrides {
  const out: UiInstanceOverrides = {}
  if (overrides.memoryMode != null) out.memoryMode = overrides.memoryMode
  if (overrides.memoryMb != null) out.memoryMb = overrides.memoryMb
  if (overrides.optimizeMemory != null) out.optimizeMemory = overrides.optimizeMemory
  if (overrides.fullscreen != null) out.fullscreen = overrides.fullscreen
  if (overrides.showGameLog != null) out.showGameLog = overrides.showGameLog
  if (overrides.windowWidth != null) out.windowWidth = overrides.windowWidth
  if (overrides.windowHeight != null) out.windowHeight = overrides.windowHeight
  if (overrides.extraJvmArgs != null) out.extraJvmArgs = overrides.extraJvmArgs
  if (overrides.gameArgs != null) out.gameArgs = overrides.gameArgs
  if (overrides.preLaunchCommand != null) out.preLaunchCommand = overrides.preLaunchCommand
  return out
}

/** 界面形状 → 后端形状。`undefined` 在 JSON 里会被省略，正好等于 `None`。 */
export function toNativeOverrides(overrides: UiInstanceOverrides): InstanceOverrides {
  const out: InstanceOverrides = {}
  if (overrides.memoryMode !== undefined) out.memoryMode = overrides.memoryMode
  if (overrides.memoryMb !== undefined) out.memoryMb = overrides.memoryMb
  if (overrides.optimizeMemory !== undefined) out.optimizeMemory = overrides.optimizeMemory
  if (overrides.fullscreen !== undefined) out.fullscreen = overrides.fullscreen
  if (overrides.showGameLog !== undefined) out.showGameLog = overrides.showGameLog
  if (overrides.windowWidth !== undefined) out.windowWidth = overrides.windowWidth
  if (overrides.windowHeight !== undefined) out.windowHeight = overrides.windowHeight
  if (overrides.extraJvmArgs !== undefined) out.extraJvmArgs = overrides.extraJvmArgs
  if (overrides.gameArgs !== undefined) out.gameArgs = overrides.gameArgs
  if (overrides.preLaunchCommand !== undefined) out.preLaunchCommand = overrides.preLaunchCommand
  return out
}

export async function getInstanceSettings(instanceId: string): Promise<InstanceSettingsView> {
  if (!isDesktopShell()) {
    const global = readPreviewSettings()
    const overrides = readPreviewOverrides()[instanceId] ?? {}
    return {
      overrides,
      global,
      resolved: resolvePreview(global, overrides),
      customized: countPreviewOverrides(overrides),
    }
  }
  return invoke<InstanceSettingsView>('get_instance_settings', { instanceId })
}

export async function saveInstanceSettings(instanceId: string, overrides: InstanceOverrides): Promise<number> {
  if (!isDesktopShell()) {
    const map = readPreviewOverrides()
    map[instanceId] = overrides
    writePreviewOverrides(map)
    return countPreviewOverrides(overrides)
  }
  return invoke<number>('save_instance_settings', { instanceId, overrides })
}

export async function resetInstanceSettings(instanceId: string): Promise<void> {
  if (!isDesktopShell()) {
    const map = readPreviewOverrides()
    delete map[instanceId]
    writePreviewOverrides(map)
    return
  }
  await invoke('reset_instance_settings', { instanceId })
}

// ---------------------------------------------------------------------------
// 启动 / 停止
// ---------------------------------------------------------------------------

export async function launchInstance(instanceId: string, accountId: string, offlineConfirmed: boolean): Promise<void> {
  requireDesktop('启动游戏')
  await invoke('launch_instance', { instanceId, accountId, offlineConfirmed })
}

export async function stopGame(instanceId: string): Promise<void> {
  requireDesktop('关闭游戏')
  await invoke('stop_game', { instanceId })
}

// ---------------------------------------------------------------------------
// 账户
// ---------------------------------------------------------------------------

export async function listAccounts(): Promise<AccountProfile[]> {
  if (!isDesktopShell()) return []
  return invoke<AccountProfile[]>('list_accounts')
}

export async function addOfflineAccount(playerName: string): Promise<AccountProfile> {
  requireDesktop('添加离线档案')
  return invoke<AccountProfile>('add_offline_account', { playerName })
}

export async function selectAccount(accountId: string): Promise<void> {
  requireDesktop('切换账户')
  await invoke('select_account', { accountId })
}

export async function removeAccount(accountId: string): Promise<void> {
  requireDesktop('移除账户')
  await invoke('remove_account', { accountId })
}

/** 微软登录会打开系统浏览器完成授权；`clientId` 留空则用构建时内置的 ID。 */
export async function microsoftLogin(clientId = ''): Promise<AccountProfile> {
  requireDesktop('Microsoft 登录')
  return invoke<AccountProfile>('microsoft_login', { clientId })
}

/**
 * 皮肤站（Yggdrasil）登录。
 *
 * `trusted` 必须是用户明确勾选的结果：后端在没有确认时直接拒绝，因为凭据
 * 只会被发送到用户确认过的 HTTPS 地址。
 */
export async function skinLogin(server: string, username: string, password: string, trusted: boolean): Promise<SkinLoginResult> {
  requireDesktop('皮肤站登录')
  return invoke<SkinLoginResult>('skin_login', { server, username, password, trusted })
}

export async function skinSelectProfile(pendingId: string, profileId: string | null): Promise<AccountProfile> {
  requireDesktop('选择角色')
  return invoke<AccountProfile>('skin_select_profile', { pendingId, profileId })
}

// ---------------------------------------------------------------------------
// 系统信息
// ---------------------------------------------------------------------------

export async function getSystemMemory(): Promise<SystemMemoryInfo> {
  if (!isDesktopShell()) {
    // 浏览器拿不到整机内存；用 8 GB 占位并标成估算（见 MemorySection 的文案）。
    return { totalMb: 8192, usedMb: null, availableMb: null, recommendedMb: 2048, maximumGameMb: 6144 }
  }
  return invoke<SystemMemoryInfo>('get_system_memory')
}


// ---------------------------------------------------------------------------
// Java
// ---------------------------------------------------------------------------

export async function listJavaRuntimes(): Promise<JavaRuntimeInfo[]> {
  if (!isDesktopShell()) return []
  return invoke<JavaRuntimeInfo[]>('list_java_runtimes')
}

export async function detectJava(): Promise<JavaRuntimeInfo> {
  requireDesktop('检测 Java')
  return invoke<JavaRuntimeInfo>('detect_java')
}

export async function inspectJavaPath(path: string): Promise<JavaRuntimeInfo> {
  requireDesktop('验证 Java 路径')
  return invoke<JavaRuntimeInfo>('inspect_java_path', { path })
}

// ---------------------------------------------------------------------------
// 版本与内容
// ---------------------------------------------------------------------------

/**
 * 浏览器预览里读取真实的 Mojang 版本清单。
 *
 * 为什么不用一份手写的示例清单：版本号、发布日期、快照命名都是可以被外部事实验证
 * 的东西，编一份假清单既会过期，也会让「预览里看到的」和「桌面版看到的」不一致。
 * Mojang 的清单接口本身允许跨域（`access-control-allow-origin: *`），所以浏览器
 * 可以直接取真实数据，不需要后端代劳。
 *
 * 这与实例列表不同：实例是用户自己的数据，浏览器里根本不存在，只能给示例；而版本
 * 清单是公开事实，可以真的拿到。所以这里没有 `PREVIEW_DATA` 开关——它对预览和正式
 * 版都是真实网络请求，返回的也是真实结果。
 *
 * 限制：
 *   - 只读公开清单，不涉及账号或凭据；
 *   - 8 秒超时，避免网络不通时界面一直转；
 *   - 失败时抛出可读错误，由界面显示重试，而不是退回假数据。
 */
const MANIFEST_URL = 'https://piston-meta.mojang.com/mc/game/version_manifest_v2.json'
const MANIFEST_TIMEOUT_MS = 8000

async function fetchMojangManifest(): Promise<VersionListItem[]> {
  {
    const response = await fetch(MANIFEST_URL, {
      signal: AbortSignal.timeout(MANIFEST_TIMEOUT_MS),
      credentials: 'omit',
      // Mojang 的清单每天才变，缓存一段时间足以避免反复请求，又不会显示过期版本。
      cache: 'default',
    })
    if (!response.ok) {
      throw new Error(`HTTP ${response.status}`)
    }
    const body = (await response.json()) as {
      versions?: Array<{ id?: unknown; type?: unknown; releaseTime?: unknown }>
    }
    if (!Array.isArray(body.versions)) {
      throw new Error('清单格式无效')
    }
    // 逐条校验：单条畸形丢弃，不让它变成列表里的空白行。
    return body.versions
      .filter((entry): entry is { id: string; type: string; releaseTime: string } =>
        typeof entry.id === 'string'
        && entry.id.length > 0
        && typeof entry.type === 'string'
        && typeof entry.releaseTime === 'string')
      .map(entry => ({
        id: entry.id,
        type: entry.type as VersionListItem['type'],
        releaseTime: entry.releaseTime,
      }))
  }
}

export async function listVersions(): Promise<VersionListItem[]> {
  if (!isDesktopShell()) return fetchMojangManifest()
  return invoke<VersionListItem[]>('list_versions')
}

/** 搜索 Modrinth 模组。后端按实例的游戏版本与加载器收窄结果，所以待安装的实例必须有 Fabric/Quilt 加载器。 */
/**
 * 浏览器里直连 Modrinth 的核心请求。
 *
 * 为什么预览能拿真实数据：Modrinth 的公开 API 允许跨域（`access-control-allow-origin: *`），
 * 浏览器可以直接取，不需要后端代劳。版本清单已经这么做了，模组目录同理——
 * 这些是公开事实，编一份假数据既会过期，也让预览与真机不一致。
 *
 * 与 Rust 实现的差别只有一点：这里不能附加自定义 User-Agent（浏览器禁止覆盖），
 * Modrinth 对普通网页请求同样接受。
 */
const MODRINTH_API = 'https://api.modrinth.com/v2'
/**
 * 单次请求的超时。
 *
 * 12 秒对一次搜索足够（实测正常往返不到 1 秒），但设得太短会让网络抖动直接变成
 * 用户可见的错误。真正的容错靠下面的重试，而不是把超时拉长——拉长只会让「确实
 * 连不上」的情况等更久才报错。
 */
const MODRINTH_TIMEOUT_MS = 15000
/** 超时/网络错误时的重试次数。首次之后最多再试两次。 */
const MODRINTH_RETRIES = 2

/** 判断一个错误是否值得重试：超时与网络中断值得，4xx 不值得。 */
function isTransient(error: unknown): boolean {
  if (error instanceof DOMException && error.name === 'TimeoutError') return true
  // fetch 在 DNS 失败/连接被重置时抛 TypeError。
  if (error instanceof TypeError) return true
  return false
}

async function modrinthGet<T>(path: string, params?: Record<string, string>): Promise<T> {
  const url = params
    ? `${MODRINTH_API}${path}?${new URLSearchParams(params).toString()}`
    : `${MODRINTH_API}${path}`
  /*
   * 用 `AbortSignal.timeout` 而不是手写定时器：手写版本在开发环境里出过难查的
   * 问题（句柄被 HMR 干扰导致请求刚发出就被取消）。
   *
   * 外面套一层重试：网络抖动是常态，一次超时就直接把错误抛给用户，体验上等同于
   * 「搜索坏了」。重试只针对超时与网络层错误——HTTP 4xx 说明请求本身有问题，
   * 重试只会重复失败。
   */
  let lastError: unknown
  for (let attempt = 0; attempt <= MODRINTH_RETRIES; attempt += 1) {
    try {
      const response = await fetch(url, {
        signal: AbortSignal.timeout(MODRINTH_TIMEOUT_MS),
        credentials: 'omit',
      })
      if (!response.ok) {
        throw new Error(response.status === 429
          ? 'Modrinth 限流中，请稍后再试。'
          : `Modrinth 请求失败（HTTP ${response.status}）。`)
      }
      return (await response.json()) as T
    } catch (error) {
      lastError = error
      if (!isTransient(error) || attempt === MODRINTH_RETRIES) break
      // 退避：400ms、900ms。给服务端一点恢复时间，也避免连续打满。
      await new Promise(resolve => setTimeout(resolve, 400 * (attempt + 1) + 100 * attempt))
    }
  }
  if (lastError instanceof DOMException && lastError.name === 'TimeoutError') {
    throw new Error(
      `连接 Modrinth 超时（已重试 ${MODRINTH_RETRIES} 次）。请检查网络或代理设置后重试。`,
    )
  }
  if (lastError instanceof TypeError) {
    throw new Error('无法连接 Modrinth（网络不可达）。请检查网络或代理设置后重试。')
  }
  throw lastError
}

/** 在预览实例里找到目标实例，取它的游戏版本与加载器。 */
function previewInstance(instanceId: string): InstanceRecord | undefined {
  return PREVIEW_INSTANCES.find(item => item.id === instanceId)
}

export async function searchMods(
  instanceId: string,
  request: ModQuery,
): Promise<CatalogMod[]> {
  if (!isDesktopShell()) {
    if (!PREVIEW_DATA) throw new NativeUnavailableError('搜索模组')
    const target = previewInstance(instanceId)
    if (!target) throw new Error('找不到所选实例。')
    const projectType = request.projectType ?? 'mod'
    // 与后端同一套 facet 构造。空关键词同样允许：Modrinth 支持只带 facet 的浏览。
    const gameVersion = request.gameVersion?.trim() || target.gameVersion
    /*
     * 资源包/光影包/数据包的版本里 `loaders` 是 `["minecraft"]`，按 `fabric` 筛会
     * 得到空列表。只有模组与整合包才带加载器 facet。
     */
    const usesLoader = projectType === 'mod' || projectType === 'modpack'
    const loader = !usesLoader
      ? ''
      : request.loader?.trim()
        ? (request.loader.trim() === 'any' ? '' : request.loader.trim())
        : (target.loader === 'vanilla' ? '' : target.loader)
    const facets: string[][] = [[`project_type:${projectType}`]]
    if (loader) facets.push([`categories:${loader}`])
    if (gameVersion) facets.push([`versions:${gameVersion}`])
    if (request.category) facets.push([`categories:${request.category}`])
    const params: Record<string, string> = {
      facets: JSON.stringify(facets),
      limit: '20',
      index: request.index || 'relevance',
    }
    if (request.offset) params.offset = String(request.offset)
    const trimmed = (request.query ?? '').trim()
    if (trimmed) params.query = trimmed
    const body = await modrinthGet<{
      hits?: Array<{
        project_id?: unknown; title?: unknown; description?: unknown; author?: unknown;
        downloads?: unknown; follows?: unknown; icon_url?: unknown;
      }>
    }>('/search', params)
    return (body.hits ?? [])
      .filter(hit => typeof hit.project_id === 'string' && typeof hit.title === 'string')
      .map(hit => ({
        projectId: hit.project_id as string,
        title: hit.title as string,
        description: typeof hit.description === 'string' ? hit.description : '',
        author: typeof hit.author === 'string' ? hit.author : '',
        downloads: typeof hit.downloads === 'number' ? hit.downloads : 0,
        follows: typeof hit.follows === 'number' ? hit.follows : 0,
        iconUrl: typeof hit.icon_url === 'string' ? hit.icon_url : null,
      }))
  }
  return invoke<CatalogMod[]>('search_mods', { instanceId, request })
}

export async function listModVersions(
  instanceId: string,
  projectId: string,
  projectType?: 'mod' | 'resourcepack' | 'shader' | 'datapack' | 'modpack',
): Promise<CatalogVersion[]> {
  if (!isDesktopShell()) {
    if (!PREVIEW_DATA) throw new NativeUnavailableError('读取模组版本')
    const target = previewInstance(instanceId)
    if (!target) throw new Error('找不到所选实例。')
    // 只接受 Modrinth 的项目 id 形态：字母数字，通常 8 位。防止把任意串拼进 URL。
    if (!/^[A-Za-z0-9]{6,16}$/.test(projectId)) throw new Error('项目标识无效。')
    const kind = projectType ?? 'mod'
    const params: Record<string, string> = { game_versions: JSON.stringify([target.gameVersion]) }
    // 资源包等不带加载器维度，与后端 `filters_by_loader` 保持一致。
    if ((kind === 'mod' || kind === 'modpack') && target.loader !== 'vanilla') {
      params.loaders = JSON.stringify([target.loader])
    }
    const raw = await modrinthGet<Array<{
      id?: unknown; version_number?: unknown; date_published?: unknown;
      files?: Array<{ url?: unknown; filename?: unknown; size?: unknown; primary?: unknown; hashes?: Record<string, unknown> }>;
    }>>(`/project/${projectId}/version`, params)
    const out: CatalogVersion[] = []
    for (const version of raw.slice(0, 100)) {
      const file = version.files?.find(item => item.primary === true) ?? version.files?.[0]
      if (!file || typeof file.url !== 'string' || typeof file.filename !== 'string') continue
      // 只信任 Modrinth 自己的分发域名，与后端 `parse_versions` 的校验一致。
      let host = ''
      try { host = new URL(file.url).hostname } catch { continue }
      if (host !== 'cdn.modrinth.com') continue
      const sha1 = file.hashes?.sha1
      if (typeof sha1 !== 'string' || sha1.length !== 40) continue
      out.push({
        versionId: typeof version.id === 'string' ? version.id : '',
        versionNumber: typeof version.version_number === 'string' ? version.version_number : '',
        dateCreated: typeof version.date_published === 'string' ? version.date_published : '',
        fileName: file.filename,
        url: file.url,
        sha1,
        size: typeof file.size === 'number' ? file.size : 0,
      })
    }
    return out
  }
  return invoke<CatalogVersion[]>('list_mod_versions', {
    instanceId,
    projectId,
    projectType: projectType ?? 'mod',
  })
}

// ---------------------------------------------------------------------------
// CurseForge
// ---------------------------------------------------------------------------
//
// key 由后端保存在操作系统凭据库里，前端只能查询「有没有设置」，永远拿不到 key
// 本身。所有请求都在 Rust 侧附加 `x-api-key`。

/** CurseForge 内容的分类，对应后端 `ContentKind`。 */
export type CurseForgeKind = 'mod' | 'modpack' | 'resourcepack' | 'world'

/** CurseForge 搜索命中。`follows` 为 null：该 API 不提供关注数。 */
export interface CurseForgeMod {
  projectId: string
  title: string
  description: string
  author: string
  downloads: number
  follows: number | null
  iconUrl: string | null
}

/** CurseForge 的一个文件版本。 */
export interface CurseForgeVersion {
  versionId: string
  displayName: string
  dateCreated: string
  fileName: string
  /**
   * 作者禁止第三方分发时为 null。
   *
   * 这不是错误：文件确实存在，只是启动器不允许代下载。界面必须显示「需手动下载」
   * 并给出项目页地址，而不是重试或换镜像。
   */
  url: string | null
  sha1: string | null
  size: number
  gameVersions: string[]
  loaders: string[]
}

export interface CurseForgeKeyStatus {
  configured: boolean
  /** 凭据库不可用时的原因（例如 Linux 没有 Secret Service）。 */
  problem: string | null
  /**
   * key 的来源：
   * - `user`：用户自己在设置里保存的；
   * - `builtin`：构建期注入的默认 key（用户没填过）；
   * - `null`：两者都没有。
   *
   * 界面用它区分「已配置」与「使用内置 key」：后者不该显示成用户配置过的东西，
   * 「清除」也只应回到内置 key，而不是让 CurseForge 不可用。
   */
  source?: 'user' | 'builtin' | null
}

export async function curseforgeKeyStatus(): Promise<CurseForgeKeyStatus> {
  if (!isDesktopShell()) {
    return {
      configured: false,
      problem: '浏览器预览无法访问系统凭据库；请使用桌面版设置 CurseForge Key。',
      source: null,
    }
  }
  return invoke<CurseForgeKeyStatus>('curseforge_key_status')
}

/** 保存 key。后端会先用一次真实请求验证，无效时不写入。 */
export async function curseforgeSaveKey(key: string): Promise<void> {
  requireDesktop('保存 CurseForge Key')
  await invoke('curseforge_save_key', { key })
}

export async function curseforgeClearKey(): Promise<void> {
  requireDesktop('清除 CurseForge Key')
  await invoke('curseforge_clear_key')
}

export interface CurseForgeQuery {
  kind: CurseForgeKind
  query: string
  gameVersion?: string
  loader?: string
  index?: number
  pageSize?: number
}

export async function curseforgeSearch(options: CurseForgeQuery): Promise<CurseForgeMod[]> {
  requireDesktop('搜索 CurseForge')
  return invoke<CurseForgeMod[]>('curseforge_search', {
    kind: options.kind,
    query: options.query,
    gameVersion: options.gameVersion ?? null,
    loader: options.loader ?? null,
    index: options.index ?? 0,
    pageSize: options.pageSize ?? 20,
  })
}

export async function curseforgeFiles(
  projectId: string,
  gameVersion: string,
  loader?: string,
): Promise<CurseForgeVersion[]> {
  requireDesktop('读取 CurseForge 版本')
  return invoke<CurseForgeVersion[]>('curseforge_files', {
    projectId,
    gameVersion,
    loader: loader ?? null,
  })
}

/**
 * 安装一个 CurseForge 文件。
 *
 * `kind` 决定落盘目录（mods / resourcepacks / saves），必须与搜索时用的类型一致，
 * 否则会把资源包写进 mods 目录，游戏读不到。
 */
export async function curseforgeInstall(
  instanceId: string,
  version: CurseForgeVersion,
  kind: CurseForgeKind,
): Promise<string> {
  requireDesktop('安装 CurseForge 文件')
  return invoke<string>('curseforge_install', { instanceId, version, kind })
}

export async function installModVersion(
  instanceId: string,
  version: CatalogVersion,
  projectType?: 'mod' | 'resourcepack' | 'shader' | 'datapack' | 'modpack',
): Promise<string> {
  requireDesktop('安装内容')
  return invoke<string>('install_mod_version', {
    instanceId,
    version,
    projectType: projectType ?? 'mod',
  })
}

/**
 * 把 CurseForge 文件直接下载到用户选择的目录，不装进实例。
 *
 * 与 `curseforgeInstall` 共用同一套 SHA-1 校验与「不覆盖同名文件」的保护，
 * 区别只有落盘位置。返回最终文件的完整路径。
 */
export async function curseforgeDownload(
  instanceId: string,
  version: CurseForgeVersion,
  kind: CurseForgeKind,
  directory: string,
): Promise<string> {
  requireDesktop('下载 CurseForge 文件')
  return invoke<string>('curseforge_download', { instanceId, version, kind, directory })
}

/** 把 Modrinth 模组直接下载到用户选择的目录，不装进实例。 */
export async function downloadModVersion(
  instanceId: string,
  version: CatalogVersion,
  directory: string,
): Promise<string> {
  requireDesktop('下载模组')
  return invoke<string>('download_mod_version', { instanceId, version, directory })
}

export async function listMods(instanceId: string): Promise<ModFile[]> {
  if (!isDesktopShell()) return PREVIEW_DATA ? previewModsFor(instanceId) : []
  return invoke<ModFile[]>('list_mods', { instanceId })
}

/**
 * 预览模式的示例模组清单。
 *
 * 只给带模组的示例实例造数据：模组数会显示在实例卡片上，全 0 的话「3 个模组」
 * 这段布局在预览里永远不会出现，也就没法调整。原版实例仍然返回空数组，因为
 * 那才是真实情况。
 */
function previewModsFor(instanceId: string): ModFile[] {
  const modded: Record<string, string[]> = {
    'preview-fabric': [
      'fabric-api-0.102.0+1.21.1.jar',
      'sodium-fabric-0.6.0+mc1.21.1.jar',
      'lithium-fabric-0.14.0+mc1.21.1.jar',
    ],
    'preview-neoforge': [
      'jei-1.20.1-forge-15.20.0.106.jar',
      'create-1.20.1-0.5.1.i.jar',
      'curios-forge-5.9.1+1.20.1.jar',
      'patchouli-1.20.1-84-FORGE.jar',
      'cloth-config-11.1.136-forge.jar',
    ],
    'preview-forge': [
      'forge-1.19.2-43.3.5-installer.jar.disabled',
      'twilightforest-1.19.2-4.2.1518.jar',
    ],
    'preview-quilt': ['quilt-loader-0.24.0.jar'],
  };
  return (modded[instanceId] ?? []).map(fileName => ({
    fileName,
    // 以 .disabled 结尾的文件在后端就是「已停用」，这里照同一规则推导。
    enabled: !fileName.endsWith('.disabled'),
    size: 1_200_000,
  }));
}

export async function setModEnabled(instanceId: string, fileName: string, enabled: boolean): Promise<void> {
  if (!isDesktopShell()) {
    // 预览模式：改本地示例清单，让开关有可见反馈；不假装写入了磁盘。
    // 生产构建里这段（连同 previewToggleMod）会被移除，并在此处如实报错。
    if (!PREVIEW_DATA) throw new NativeUnavailableError('启用或停用模组')
    previewToggleMod(instanceId, fileName, enabled)
    return
  }
  await invoke('set_mod_enabled', { instanceId, fileName, enabled })
}

/**
 * 静态检查模组之间的依赖与冲突。
 *
 * 只读 JAR 元数据，不加载类、不执行代码。后端把发现按「严重程度 + 涉及文件」
 * 返回，界面负责解释。
 */
export async function inspectMods(instanceId: string): Promise<ModIssue[]> {
  if (!isDesktopShell()) return []
  return invoke<ModIssue[]>('inspect_mods', { instanceId })
}

/**
 * 向 Modrinth 查询已安装模组是否有新版本。
 *
 * 注意隐私边界：这个查询会把已安装文件的哈希发给 Modrinth 用于反查项目。
 * 界面必须在调用前明确告知，不能默认替用户决定。
 *
 * 后端把状态序列化成外部标记枚举（`{"upToDate": null}` /
 * `{"updateAvailable": {...}}` / `"unknownSource"` / `{"unsupported": "…"}`），
 * 这里统一解析成判别联合，界面就不必再关心 serde 的线上形状。
 */
export async function checkModUpdates(instanceId: string): Promise<ModUpdateReport[]> {
  if (!isDesktopShell()) throw new NativeUnavailableError('检查模组更新')
  const raw = await invoke<Array<{ fileName: string; status: unknown }>>('check_mod_updates', { instanceId })
  return raw.map(row => ({ fileName: row.fileName, status: parseModUpdateStatus(row.status) }))
}

/**
 * 解析 serde 外部标记枚举。
 *
 * 四种合法形状里，`upToDate` 与 `unknownSource` 是「无载荷变体」——serde 默认
 * 写成裸字符串 `"upToDate"`，但带 `rename_all` 的枚举也可能写成单键对象。
 * 两种都接受，避免因后端序列化风格微调而整个页面报错。
 */
function parseModUpdateStatus(raw: unknown): ModUpdateStatus {
  if (typeof raw === 'string') {
    if (raw === 'upToDate' || raw === 'up_to_date') return { kind: 'upToDate' }
    if (raw === 'unknownSource' || raw === 'unknown_source') return { kind: 'unknownSource' }
    return { kind: 'unsupported', reason: raw }
  }
  if (raw && typeof raw === 'object') {
    const entry = Object.entries(raw as Record<string, unknown>)[0]
    if (!entry) return { kind: 'unknownSource' }
    const [tag, payload] = entry
    switch (tag) {
      case 'upToDate':
      case 'up_to_date':
        return { kind: 'upToDate' }
      case 'unknownSource':
      case 'unknown_source':
        return { kind: 'unknownSource' }
      case 'updateAvailable':
      case 'update_available':
        return { kind: 'updateAvailable', update: payload as ModUpdate }
      case 'unsupported':
        return { kind: 'unsupported', reason: typeof payload === 'string' ? payload : '不受支持' }
      default:
        return { kind: 'unsupported', reason: tag }
    }
  }
  return { kind: 'unknownSource' }
}

/** 应用一个模组更新。旧文件保留为 `.old`，不直接删除。 */
export async function updateMod(instanceId: string, update: ModUpdate): Promise<string> {
  if (!isDesktopShell()) throw new NativeUnavailableError('更新模组')
  return invoke<string>('update_mod', { instanceId, update })
}

/** 对照 Mojang 清单检查客户端文件是否有更新。 */
export async function checkClientUpdate(instanceId: string): Promise<ClientUpdateStatus> {
  if (!isDesktopShell()) throw new NativeUnavailableError('检查客户端更新')
  return invoke<ClientUpdateStatus>('check_client_update', { instanceId })
}

/** 预览模式：在内存里翻转模组的启用状态并保存，供界面演示开关效果。 */
function previewToggleMod(instanceId: string, fileName: string, enabled: boolean): void {
  const key = 'mfmcl-preview-mod-toggles'
  try {
    const raw = localStorage.getItem(key)
    const map = raw ? JSON.parse(raw) as Record<string, Record<string, boolean>> : {}
    map[instanceId] = { ...(map[instanceId] ?? {}), [fileName]: enabled }
    localStorage.setItem(key, JSON.stringify(map))
  } catch {
    // 存储不可用时保持界面状态即可。
  }
}

/**
 * 浏览器里直连加载器元数据。
 *
 * Fabric 与 Quilt 的 meta 接口都允许跨域，且返回的是公开事实（可用版本与是否稳定）。
 * 桌面版走 Rust 后端（同样的接口，带额外校验）；预览直连是为了让安装面板在浏览器
 * 里真的能选到版本，而不是永远显示「读取失败」。
 */
const LOADER_META: Record<'fabric' | 'quilt', string> = {
  fabric: 'https://meta.fabricmc.net/v2',
  quilt: 'https://meta.quiltmc.org/v3',
}

/**
 * Forge / NeoForge 的版本列表。
 *
 * ## 预览模式的限制（如实告知，不伪造）
 *
 * `files.minecraftforge.net` 的 `promotions_slim.json` **没有 CORS 头**，浏览器
 * 拿不到它；`maven.neoforged.net` 在当前网络环境下也不稳定。桌面版走 Rust 后端，
 * 不受 CORS 约束，所以这两个加载器只在桌面版可用。
 *
 * 这里抛出明确的说明而不是留一个必然失败的请求——用户需要知道这是预览的限制，
 * 不是启动器坏了。
 */
async function fetchInstallerVersions(kind: 'forge' | 'neoforge', _gameVersion: string): Promise<LoaderVersion[]> {
  throw new Error(
    kind === 'forge'
      ? '浏览器预览无法获取 Forge 版本列表（官方服务不允许跨域请求）；请在桌面版中安装 Forge。'
      : '浏览器预览无法获取 NeoForge 版本列表；请在桌面版中安装 NeoForge。',
  )
}

async function fetchLoaderVersions(kind: LoaderKind, gameVersion: string): Promise<LoaderVersion[]> {
  // Forge / NeoForge 的来源与 Fabric 完全不同，先分派出去。
  if (kind === 'forge' || kind === 'neoforge') {
    return fetchInstallerVersions(kind, gameVersion)
  }
  /*
   * 用 `AbortSignal.timeout()` 而不是手写 `setTimeout + abort()`。
   *
   * 手写版本在开发环境里出过一个难查的问题：定时器句柄被 HMR 模块实例的重新执行
   * 干扰，signal 会在请求刚发出时就被 abort（表现为 `AbortError: signal is
   * aborted without reason`）。`AbortSignal.timeout()` 由浏览器自己管理生命周期，
   * 不依赖我们保存的句柄，也就没有这个问题。
   */
  const response = await fetch(
    `${LOADER_META[kind]}/versions/loader/${encodeURIComponent(gameVersion)}`,
    { signal: AbortSignal.timeout(12000), credentials: 'omit' },
  )
  if (!response.ok) throw new Error(`HTTP ${response.status}`)
  const body = (await response.json()) as Array<{
    loader?: { version?: unknown; stable?: unknown }
  }>
  if (!Array.isArray(body)) throw new Error('响应格式无效')
  return body
    .map(entry => ({
      version: typeof entry.loader?.version === 'string' ? entry.loader.version : '',
      stable: entry.loader?.stable === true,
    }))
    .filter(item => item.version !== '')
}

export async function loaderVersions(kind: LoaderKind, gameVersion: string): Promise<LoaderVersion[]> {
  if (!isDesktopShell()) {
    if (!PREVIEW_DATA) throw new NativeUnavailableError('读取加载器版本')
    return fetchLoaderVersions(kind, gameVersion)
  }
  return invoke<LoaderVersion[]>('loader_versions', { kind, gameVersion })
}

export async function installLoader(instanceId: string, kind: LoaderKind, version: string): Promise<void> {
  requireDesktop('安装加载器')
  await invoke('install_loader', { instanceId, kind, version })
}

export async function previewModpack(path: string): Promise<PackPreview> {
  requireDesktop('预览整合包')
  return invoke<PackPreview>('preview_modpack', { path })
}

export async function importModpack(path: string): Promise<InstanceRecord> {
  requireDesktop('导入整合包')
  return invoke<InstanceRecord>('import_modpack', { path })
}

/**
 * 从 CurseForge 下载整合包并导入为新实例。
 *
 * 与 `importModpack` 的区别只在于文件来源：这里由后端先把包下载到临时目录，
 * 再走完全相同的导入事务（校验结构 → 创建实例 → 下载包内文件 → 失败回滚）。
 * 前端不需要知道临时文件在哪。
 *
 * `instanceHint` 只用于给下载任务一个可见的归属——整合包会创建新实例，
 * 所以没有真正意义上的「当前实例」。
 */
export async function downloadAndImportModpack(
  version: CurseForgeVersion,
  instanceHint?: string,
): Promise<InstanceRecord> {
  requireDesktop('下载并导入整合包')
  return invoke<InstanceRecord>('download_and_import_modpack', { version, instanceHint: instanceHint ?? null })
}

/**
 * 实例里可以被「打开文件夹」定位的子目录。
 *
 * 用固定枚举而不是自由的相对路径：后端只接受这几个值，路径由它自己拼出来。
 * 这样即使前端被注入，也无法让系统文件管理器打开任意位置。
 */
export type InstanceFolder =
  | 'root'
  | 'game'
  | 'mods'
  | 'resource_packs'
  | 'shader_packs'
  | 'saves'
  | 'config'
  | 'logs'
  | 'crash_reports'

/**
 * 在系统文件管理器中打开实例的某个子目录。
 *
 * 返回实际打开的绝对路径（用于提示）。桌面版调用系统命令；
 * 浏览器预览没有文件系统，直接抛出可读错误而不是假装成功。
 */
export async function openInstanceFolder(instanceId: string, folder: InstanceFolder): Promise<string> {
  requireDesktop('打开实例文件夹')
  return invoke<string>('open_instance_folder', { instanceId, folder })
}

/**
 * 在文件管理器中打开「直接下载」用的目录。
 *
 * 路径来自用户通过系统对话框选定的目录，启动器只负责交给文件管理器。
 */
export async function openDownloadFolder(path: string): Promise<void> {
  requireDesktop('打开下载目录')
  await invoke('open_download_folder', { path })
}

export async function exportModpack(instanceId: string, directory: string, rightsConfirmed: boolean): Promise<string> {
  requireDesktop('导出整合包')
  return invoke<string>('export_modpack', { instanceId, directory, rightsConfirmed })
}

// ---------------------------------------------------------------------------
// 下载任务
// ---------------------------------------------------------------------------

/**
 * 预览模式的示例下载任务与文件进度。
 *
 * 为什么需要：浏览器里没有后端，任务列表与文件进度都为空，下载页永远是「暂无
 * 下载任务」，侧栏那颗并行下载气泡也永远不会出现——恰恰是这两处需要调外观。
 *
 * 只用于开发预览：`PREVIEW_DATA` 在生产构建里是编译期 false，这些数据与分支会
 * 被一并移除。且它们只是**展示用的快照**，不会推进：点暂停/取消会如实报错，不会
 * 假装任务在跑。
 */
const PREVIEW_DOWNLOAD_TASKS: DownloadTask[] = [
  {
    id: 'preview-task-1',
    instanceId: 'preview-fabric',
    label: '安装 Fabric 优化整合',
    source: 'official',
    state: 'running',
    completedBytes: 184_320_000,
    totalBytes: 412_000_000,
    createdAt: '2026-10-29T21:02:00Z',
    updatedAt: '2026-10-29T21:05:30Z',
  },
  {
    id: 'preview-task-2',
    instanceId: 'preview-neoforge',
    label: '安装模组 jei-1.20.1-forge.jar',
    source: 'modrinth',
    state: 'paused',
    completedBytes: 1_200_000,
    totalBytes: 1_500_000,
    createdAt: '2026-10-29T20:58:00Z',
    updatedAt: '2026-10-29T21:01:00Z',
  },
  {
    id: 'preview-task-3',
    instanceId: 'preview-vanilla',
    label: '安装 纯净生存 1.21.1',
    source: 'bmclapi',
    state: 'completed',
    completedBytes: 268_000_000,
    totalBytes: 268_000_000,
    createdAt: '2026-10-29T20:40:00Z',
    updatedAt: '2026-10-29T20:52:00Z',
  },
];

/**
 * 预览模式的文件级进度快照。
 *
 * 4 个处于 downloading、1 个 verifying、1 个 completed：这样侧栏气泡应显示 4，
 * 下载页也能同时展示「正在传输」与「已校验完成」两种行，两者都能调外观。
 */
const PREVIEW_FILE_PROGRESS: DownloadProgress[] = [
  { artifactId: 'p-a1', label: 'assets/objects/1a/1a2b3c', received: 1_048_576, total: 2_097_152, source: 'bmclapi', stage: 'downloading' },
  { artifactId: 'p-a2', label: 'libraries/com/mojang/brigadier.jar', received: 512_000, total: 890_000, source: 'official', stage: 'downloading' },
  { artifactId: 'p-a3', label: 'assets/objects/7f/7f8e9d', received: 262_144, total: 1_048_576, source: 'bmclapi', stage: 'downloading' },
  { artifactId: 'p-a4', label: 'natives/lwjgl-3.3.3-linux.jar', received: 786_432, total: 921_600, source: 'official', stage: 'downloading' },
  { artifactId: 'p-a5', label: 'versions/1.21.1/client.jar', received: 24_000_000, total: 24_000_000, source: 'bmclapi', stage: 'verifying' },
  { artifactId: 'p-a6', label: 'libraries/com/google/gson.jar', received: 286_000, total: 286_000, source: 'official', stage: 'completed' },
];

export async function listDownloadTasks(): Promise<DownloadTask[]> {
  if (!isDesktopShell()) {
    if (!PREVIEW_DATA) return []
    return PREVIEW_DOWNLOAD_TASKS.map(task => ({ ...task }))
  }
  return invoke<DownloadTask[]>('list_download_tasks')
}

/**
 * 预览模式的文件进度快照。桌面版由 `download-progress` 事件推送，没有查询命令，
 * 所以这里只服务预览。
 */
export function previewFileProgress(): DownloadProgress[] {
  return PREVIEW_DATA ? PREVIEW_FILE_PROGRESS.map(item => ({ ...item })) : []
}

/** `action` 只接受 pause / resume / cancel；后端对未知动作返回错误。 */
export async function controlDownload(instanceId: string, action: 'pause' | 'resume' | 'cancel'): Promise<void> {
  requireDesktop('控制下载任务')
  await invoke('control_download', { instanceId, action })
}

// ---------------------------------------------------------------------------
// 备份与诊断
// ---------------------------------------------------------------------------

export async function createBackup(instanceId: string, directory: string, privacyConfirmed: boolean): Promise<string> {
  requireDesktop('创建备份')
  return invoke<string>('create_backup', { instanceId, directory, privacyConfirmed })
}

export async function previewBackup(path: string): Promise<BackupPreview> {
  requireDesktop('预览备份')
  return invoke<BackupPreview>('preview_backup', { path })
}

export async function restoreBackup(path: string, archiveSha256: string, name: string, confirmed: boolean): Promise<InstanceRecord> {
  requireDesktop('恢复备份')
  return invoke<InstanceRecord>('restore_backup', { path, archiveSha256, name, confirmed })
}

export async function summarizeDiagnostics(instanceId: string): Promise<DiagnosticSummary> {
  requireDesktop('生成诊断摘要')
  return invoke<DiagnosticSummary>('summarize_diagnostics', { instanceId })
}

export async function exportDiagnostics(instanceId: string, destinationDir: string): Promise<string> {
  requireDesktop('导出诊断包')
  return invoke<string>('export_diagnostics', { instanceId, destinationDir })
}

// ---------------------------------------------------------------------------
// 启动器更新
// ---------------------------------------------------------------------------

export async function checkLauncherUpdate(): Promise<LauncherUpdateStatus> {
  requireDesktop('检查更新')
  return invoke<LauncherUpdateStatus>('check_launcher_update')
}

export async function downloadLauncherUpdate(url: string, sha256: string, directory: string): Promise<string> {
  requireDesktop('下载更新')
  return invoke<string>('download_launcher_update', { url, sha256, directory })
}

// ---------------------------------------------------------------------------
// 系统对话框与事件
// ---------------------------------------------------------------------------

/** 选择一个文件夹；用户取消时返回 null。 */
export async function pickDirectory(title: string): Promise<string | null> {
  if (!isDesktopShell()) return null
  const selected = await open({ directory: true, multiple: false, title })
  return typeof selected === 'string' ? selected : null
}

/** 选择一个文件；`filters` 用于限制扩展名（如 mcpack/zip）。 */
export async function pickFile(title: string, filters?: { name: string; extensions: string[] }[]): Promise<string | null> {
  if (!isDesktopShell()) return null
  const selected = await open({ directory: false, multiple: false, title, filters })
  return typeof selected === 'string' ? selected : null
}

/**
 * 订阅后端事件。
 *
 * 返回 `null` 表示当前不是桌面环境（预览模式没有事件源）。调用方据此跳过
 * 订阅即可，不必分辨「订阅失败」和「后端没发过事件」——后者本来就没有回调。
 */
async function subscribe<T>(name: string, handler: (payload: T) => void): Promise<UnlistenFn | null> {
  if (!isDesktopShell()) return null
  return listen<T>(name, event => handler(event.payload))
}

export const onDownloadProgress = (handler: (payload: DownloadProgress) => void) => subscribe('download-progress', handler)
export const onInstallationTask = (handler: (payload: TaskEvent) => void) => subscribe('installation-task', handler)
export const onGameLogLine = (handler: (payload: GameLogLine) => void) => subscribe('game-log-line', handler)
export const onGameProcess = (handler: (payload: GameProcessEvent) => void) => subscribe('game-process', handler)

/**
 * 启动过程的阶段进度。
 *
 * 与 `onGameProcess` 分开：后者只在进程状态变化时发一次（running/stopped/crashed），
 * 而启动要经历校验、认证、拼参数、拉起 JVM 几步，中间可能等待数秒。用户需要看到
 * 「在动」以及「在等什么」，一个转圈给不了这个信息。
 */
export const onLaunchProgress = (handler: (payload: LaunchProgressEvent) => void) => subscribe('launch-progress', handler)

// ---------------------------------------------------------------------------
// 插件运行时
// ---------------------------------------------------------------------------

/** 一条贡献的类型。与 Rust `ContributionKind` 的 serde 名称一致。 */
export type PluginContributionKind = 'jvmArgs' | 'env' | 'preLaunch' | 'backupBefore'

/** 一条插件贡献。`value` 的含义随 `kind` 变化，见 PLUGIN.md。 */
export interface PluginContribution {
  kind: PluginContributionKind
  value: string
}

/** 后端的插件定义。只含可执行部分，不含文案与图标。 */
export interface PluginDefinition {
  id: string
  version: string
  hooks: string[]
  permissions: string[]
  contributions: PluginContribution[]
  incompatible?: boolean
}

export interface PluginStatePayload {
  custom: PluginDefinition[]
  enabled: Record<string, boolean>
}

export interface AppliedContribution {
  pluginId: string
  kind: PluginContributionKind
  summary: string
}

export interface SkippedContribution {
  pluginId: string
  kind: PluginContributionKind
  reason: string
}

export interface PluginPreview {
  applied: AppliedContribution[]
  skipped: SkippedContribution[]
}

export interface LaunchHistoryEntry {
  id: string
  instanceId: string
  instanceName: string
  startedAt: string
  exitCode: number | null
  outcome: 'running' | 'stopped' | 'crashed'
  contributions: { applied: AppliedContribution[]; skipped: SkippedContribution[] }
}

export interface PluginRuntimeInfo {
  available: boolean
  /** 真正会被执行的贡献类型。界面据此区分「声明了但不会发生」与「会生效」。 */
  kinds: PluginContributionKind[]
}

/**
 * 查询运行时能力。
 *
 * 浏览器预览没有后端，返回 `available: false` 而不是抛错：插件页要在两种环境里
 * 都能打开，只是说明的文字不同。抛错会让整页渲染失败。
 */
export async function pluginRuntimeInfo(): Promise<PluginRuntimeInfo> {
  if (!isDesktopShell()) {
    return { available: false, kinds: [] }
  }
  return invoke<PluginRuntimeInfo>('plugin_runtime_info')
}

/** 读取插件状态（自定义清单 + 启用覆盖）。预览模式下返回空，由本地的 localStorage 状态负责。 */
export async function pluginState(): Promise<PluginStatePayload | null> {
  if (!isDesktopShell()) return null
  return invoke<PluginStatePayload>('plugin_state')
}

/**
 * 保存插件状态。
 *
 * 后端会逐条校验贡献并拒绝非法的那一种；保存失败时抛出的错误要显示给用户——
 * 那正是「这条参数不允许」最有用的提示时机。
 */
export async function pluginSaveState(state: PluginStatePayload): Promise<void> {
  requireDesktop('保存插件设置')
  await invoke('plugin_save_state', { state })
}

/** 预览某个插件实际会贡献什么。 */
export async function pluginPreview(pluginId: string): Promise<PluginPreview | null> {
  if (!isDesktopShell()) return null
  return invoke<PluginPreview>('plugin_preview', { pluginId })
}

export async function launchHistory(): Promise<LaunchHistoryEntry[]> {
  if (!isDesktopShell()) return []
  return invoke<LaunchHistoryEntry[]>('launch_history')
}

export async function clearLaunchHistory(): Promise<number> {
  if (!isDesktopShell()) return 0
  return invoke<number>('clear_launch_history')
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

/** 把后端（或前端校验）抛出的任意错误折成可显示的一行文字。 */
export function errorText(error: unknown, fallback = '操作失败，请重试。'): string {
  if (error instanceof Error && error.message) return error.message
  if (typeof error === 'string' && error.trim()) return error
  return fallback
}

/** 字节数转可读体积，用于下载进度与备份大小。 */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B'
  if (bytes < 1024) return `${Math.round(bytes)} B`
  const units = ['KB', 'MB', 'GB', 'TB']
  let value = bytes / 1024
  let unit = 0
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024
    unit += 1
  }
  return `${value.toFixed(value >= 100 ? 0 : 1)} ${units[unit]}`
}

