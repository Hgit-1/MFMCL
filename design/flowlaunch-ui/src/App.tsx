import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { Check, ChevronDown } from 'lucide-react';
import { useTheme } from '../hooks/useTheme';
import { Button } from '../components/Button';
import { Card } from '../components/Card';
import { AccountModal } from '../components/AccountModal';
import { DefaultUserAvatar } from '../components/DefaultUserAvatar';
import { SettingsPanel } from '../components/SettingsPanel';
import { Modal } from '../components/Modal';
import { InstanceSettings } from '../components/InstanceSettings';
import { StoreWorkspace, STORE_CATEGORIES } from '../components/StoreWorkspace';
import { CreateInstanceDialog } from '../components/CreateInstanceDialog';
import { PluginWorkspace } from '../components/PluginWorkspace';
import { LOCALES, useI18n } from '../utils/i18n';
import type { MessageKey } from '../utils/i18n/messages/zh-CN';
import { readPrefs, DEFAULT_PREFS, type LauncherPrefs } from '../utils/launcherPrefs';
import {
  loadPluginStore,
  savePluginStore,
  updatablePluginCount,
  toPluginStatePayload,
  type PluginStore,
} from '../utils/plugins';
import {
  EMPTY_OVERRIDES,
  loadInstanceOverrides,
  type InstanceOverrides,
  type OverrideMap,
} from '../utils/instanceOverrides';
import { BlockIcon } from '../components/BlockIcon';
import { VanillaIcon } from '../components/VanillaIcon';
import { LoaderIcon } from '../components/LoaderIcon';
import { Dashboard } from '../components/Dashboard';
import { BackgroundLayer } from '../components/BackgroundLayer';
import { autoAccentOf } from '../components/BackgroundSection';
import { applyAccent, DEFAULT_ACCENT } from '../utils/accent';
import {
  readBackgroundSetting,
  type BackgroundSetting,
} from '../utils/background';
import { iconForLoader } from '../utils/store/loaders';
import { VANILLA_BLOCK } from '../utils/store/blocks';
import { Image as ImageIcon, Map, Package, Puzzle } from 'lucide-react';
import {
  DEFAULT_INSTANCE_JAVA,
  loadInstanceJavaSettings,
  loadJavaList,
  saveInstanceJavaSettings,
  type InstanceJavaSetting,
  type JavaRuntime,
} from '../utils/java';
import type { GameInstance, Account, AppContext, DownloadTask } from '../types';
import type { StoreCategory } from '../utils/store/types';
import { CATEGORY_KEYS } from '../utils/store/types';
import {
  controlDownload,
  createInstance as createInstanceNative,
  deleteInstance as deleteInstanceNative,
  errorText,
  formatBytes,
  getSettings,
  getInstanceSettings,
  saveInstanceSettings as saveInstanceSettingsNative,
  installInstance as installInstanceNative,
  isDesktopShell,
  launchInstance as launchInstanceNative,
  listAccounts,
  listInstances,
  listDownloadTasks,
  listJavaRuntimes,
  listMods,
  onDownloadProgress,
  onGameLogLine,
  onGameProcess,
  onLaunchProgress,
  onInstallationTask,
  prefsToSettings,
  pluginSaveState,
  previewFileProgress,
  saveSettings as saveSettingsNative,
  selectAccount as selectAccountNative,
  stopGame,
  toNativeOverrides,
  toUiOverrides,
  type AccountProfile,
  type DownloadTask as NativeDownloadTask,
  type InstanceRecord,
  type JavaRuntimeInfo,
  type LauncherSettings,
} from '../utils/native';
import { toAccount, toGameInstance } from '../utils/adapters';
import styles from './App.module.css';
import '../styles/globals.css';

/*
 * 主页默认启动实例的存储键。
 *
 * 放在这里而不是 `launcherPrefs`：那个模块存的是「启动器设置」，由设置面板统一
 * 编辑与保存；这是一个独立的界面状态，没有对应的设置项，混进去会让
 * `prefFromSettings` 的字段映射多出一个后端没有的项。
 */
const DEFAULT_LAUNCH_KEY = 'mfmcl-default-launch-instance';

// 图标组件（简化版）
const PlayIcon = () => <svg width="20" height="20" viewBox="0 0 20 20" fill="currentColor"><path d="M6 4l10 6-10 6V4z"/></svg>;
const PlusIcon = () => <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2"><path d="M10 5v10M5 10h10"/></svg>;
const SettingsIcon = () => <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2"><circle cx="10" cy="10" r="3"/><path d="M10 1v2m0 14v2M1 10h2m14 0h2m-3.5-6.5l1.4-1.4m-11.8 0l-1.4-1.4m12.6 12.6l1.4 1.4m-11.8 0l-1.4 1.4"/></svg>;
const LibraryIcon = () => <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2"><rect x="2" y="3" width="16" height="14" rx="2"/><path d="M2 7h16"/></svg>;
/*
 * 主页图标：房子。
 * 与游戏库的「窗口」轮廓区分开——房子表达「入口/起点」，窗口表达「一个列表」。
 * 两者都是矩形，所以房子特意用三角屋顶把上半部分撑开。
 */
const HomeIcon = () => (
  <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
    <path d="M3 9l7-6 7 6v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V9z" />
    <path d="M8 18v-6h4v6" />
  </svg>
);
const ShopIcon = () => <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2"><path d="M3 6l3-3h8l3 3v11a2 2 0 01-2 2H5a2 2 0 01-2-2V6z"/><path d="M3 6h14v2a3 3 0 01-6 0 3 3 0 01-6 0V6"/></svg>;
/*
 * 图钉：标记「主页按钮启动的就是这个实例」。
 *
 * 用图钉而不是星标：星标在启动器里通常表示「收藏」，而这里表达的是「固定住
 * 一个默认值」——钉住某样东西不会被别的事件挪走，正是这个设置的含义。
 * `filled` 用于已经钉住的卡片，实心与空心在 14px 下也能一眼分辨。
 */
const PinIcon = ({ filled = false }: { filled?: boolean }) => (
  <svg width="14" height="14" viewBox="0 0 24 24" fill={filled ? 'currentColor' : 'none'} stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" focusable="false">
    <path d="M12 17v5" />
    <path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z" />
  </svg>
);
const DownloadIcon = () => <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2"><path d="M10 3v10m-4-4l4 4 4-4M3 17h14"/></svg>;
/*
 * 修复图标：扳手。
 * 与下载箭头区分开——「安装」是从无到有，「修复」是把已有的东西修好，
 * 两者的动作性质不同，图标也该不同。
 */
const WrenchIcon = () => (
  <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
    <path d="M13.5 3a4.5 4.5 0 0 0-4 6.7L3.6 15.6a1.6 1.6 0 0 0 2.2 2.3l5.9-5.9A4.5 4.5 0 1 0 13.5 3Z" />
    <circle cx="13.8" cy="6.2" r="1.5" />
  </svg>
);
/*
 * 插件图标：一枚插座。
 *
 * 插件在中文语境里也常被读成「扩展/接口」，插座比拼图块更直白地传达「插上去
 * 就多一个能力」，而且与商店用的盒子、库用的矩形在轮廓上区分得开——侧栏图标
 * 只有 20px，靠轮廓差异认类比靠细节更重要。
 */
const PluginsIcon = () => (
  <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
    <path d="M7 2v4M13 2v4" />
    <path d="M5 6h10v3a5 5 0 01-5 5 5 5 0 01-5-5V6z" />
    <path d="M10 14v4" />
  </svg>
);
function SidebarAccountAvatar({ account }: { account?: Account }) {
  const { t } = useI18n();
  if (!account?.avatar) return <DefaultUserAvatar className={styles.defaultUserAvatar} />;
  return (
    <img
      className={styles.sidebarAvatar}
      src={account.avatar}
      alt={t('account.avatarAlt', { values: { name: account.username } })}
    />
  );
}

const SunIcon = () => <svg width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2"><circle cx="10" cy="10" r="4"/><path d="M10 2v2m0 12v2M2 10h2m12 0h2m-2.8-5.2l1.4-1.4m-11.2 0L4 4.8m11.2 11.2l1.4 1.4m-11.2 0L4 15.2"/></svg>;
const MoonIcon = () => <svg width="20" height="20" viewBox="0 0 20 20" fill="currentColor"><path d="M17.293 13.293A8 8 0 016.707 2.707a8.001 8.001 0 1010.586 10.586z"/></svg>;

/**
 * 退出登录图标。
 *
 * 用业界通行的「门 + 右向箭头」造型（Material Symbols 的 logout、GitHub 与
 * VS Code 用的都是这一种），而不是「人 + 减号」：后者容易被读成「删除账户」。
 * 描边取 1.9、圆头圆角，与侧栏其它 lucide 图标（20px / strokeWidth 2）的
 * 视觉重量对齐。
 */
const SignOutIcon = () => (
  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.9" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" focusable="false">
    <path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4" />
    <path d="M16 17l5-5-5-5" />
    <path d="M21 12H9" />
  </svg>
);

const TASK_STATUS_KEYS: Record<DownloadTask['status'], MessageKey> = {
  queued: 'app.taskQueued',
  downloading: 'app.taskDownloading',
  verifying: 'app.taskVerifying',
  installing: 'app.taskInstalling',
  completed: 'app.taskCompleted',
  failed: 'app.taskFailed',
  paused: 'app.taskPaused',
  cancelled: 'app.taskCancelled',
};

/** 仍在进行中的任务状态。已完成/失败/取消的任务不再计入侧栏徽章。 */
const ACTIVE_TASK_STATUSES: DownloadTask['status'][] = ['queued', 'downloading', 'verifying', 'installing'];

/**
 * 启动阶段 → 文案键。
 *
 * 阶段键由后端给出（`preparing` / `authenticating` / `building` / `starting`），
 * 这里只做「键 → 可读文案」的映射，未知键回落到 preparing——比显示一个裸的英文
 * 标识符好，也比整条进度条消失好。
 */
const LAUNCH_STAGE_KEYS: Record<string, MessageKey> = {
  preparing: 'app.launchStagePreparing',
  authenticating: 'app.launchStageAuthenticating',
  building: 'app.launchStageBuilding',
  starting: 'app.launchStageStarting',
};

/**
 * 单个文件的下载阶段，对应后端 `DownloadStage`。
 *
 * 借鉴 PCL2 的 NetState：它把「正在连接」与「正在下载」分开，因为两者的等待
 * 体验完全不同——连接阶段可能卡在 DNS 或 TLS 握手，用户需要知道「不是没反应，
 * 是在等服务器」。只显示一个百分比无法表达这件事。
 *
 * `labelKey` / `tagClass` / `fillClass` 都是查表用的键：文案走 i18n，类名走
 * CSS Module 的构建期映射，组件里各自解析一次。
 */
const FILE_STAGE_META: Record<string, { labelKey: MessageKey; tagClass: string; fillClass: string }> = {
  connecting: { labelKey: 'app.stageConnecting', tagClass: 'stageConnecting', fillClass: 'fillConnecting' },
  downloading: { labelKey: 'app.stageDownloading', tagClass: 'stageDownloading', fillClass: 'fillDownloading' },
  verifying: { labelKey: 'app.stageVerifying', tagClass: 'stageVerifying', fillClass: 'fillVerifying' },
  completed: { labelKey: 'app.stageCompleted', tagClass: 'stageCompleted', fillClass: 'fillCompleted' },
  failed: { labelKey: 'app.stageFailed', tagClass: 'stageFailed', fillClass: 'fillFailed' },
};

/**
 * 字节数格式化复用 `utils/native.ts` 的 `formatBytes`。
 *
 * 早先这里另写了一份：两份舍入规则一旦分叉，同一个文件在下载页和商店里会显示成
 * 不同的大小。保持单一来源。
 */

const CATEGORY_ICONS: Record<Exclude<StoreCategory, 'vanilla'>, React.ReactNode> = {
  mod: <Puzzle size={18} strokeWidth={1.8} />,
  modpack: <Package size={18} strokeWidth={1.8} />,
  resourcepack: <ImageIcon size={18} strokeWidth={1.8} />,
  world: <Map size={18} strokeWidth={1.8} />,
};

/**
 * 后端 Java 运行时 → 界面 `JavaRuntime`。
 *
 * 字段几乎一一对应，唯一差别是后端不返回「路径所在的文件夹名」这种派生值——那
 * 是 `runtimeFolder()` 现算的，不需要在转换里重复。
 */
function toJavaRuntime(runtime: JavaRuntimeInfo): JavaRuntime {
  return { path: runtime.path, majorVersion: runtime.majorVersion, versionText: runtime.versionText };
}

/**
 * 后端设置 → 界面侧的 `LauncherPrefs` 只读快照。
 *
 * 实例管理页需要显示「跟随全局时实际是什么值」，它读的是 `LauncherPrefs`。这里
 * 把后端字段翻译过去，只填那些后端真正会读的项：界面不展示、后端也不读的字段
 * （透明度、标题栏样式等）保持默认值即可，反正没有任何代码会看它们。
 */
function prefFromSettings(settings: LauncherSettings): LauncherPrefs {
  return {
    ...DEFAULT_PREFS,
    theme: settings.themeMode,
    themePreset: settings.themePreset,
    customAccent: settings.customAccent,
    showGameLog: settings.showGameLog,
    javaPath: settings.javaPath ?? '',
    memoryMb: settings.memoryMb,
    memoryMode: settings.memoryMode,
    windowWidth: settings.windowWidth,
    windowHeight: settings.windowHeight,
    extraJvmArgs: settings.extraJvmArgs,
    gameArgs: settings.gameArgs,
    preLaunchCommand: settings.preLaunchCommand,
    fullscreen: settings.fullscreen,
    /*
     * 下载目录必须在这里带上：`DEFAULT_PREFS` 里是空串，不映射的话每次从后端
     * 拉设置都会把用户在设置面板里填的目录冲掉，内容商店侧栏刚选的目录也会
     * 在下次刷新时消失。
     */
    downloadDirectory: settings.downloadDirectory ?? '',
    launcherVisibility: settings.launcherVisibility,
    downloadSource: settings.download.source,
    maxConcurrentDownloads: settings.download.maxConcurrentDownloads,
    connectionsPerFile: settings.download.connectionsPerFile,
    aria2Enabled: settings.download.aria2Enabled,
    // 后端拒绝非 normal 优先级与「启动后关闭启动器」，界面固定显示它们唯一允许的值。
    processPriority: 'normal',
    closeAfterLaunch: false,
    checkUpdates: settings.updatePolicy === 'notify',
    announcementPolicy: settings.updatePolicy,
  };
}

/**
 * 后端任务 → 界面任务行。
 *
 * 界面上的 `progress` 是 0–100 的百分比，后端只有已下载/总字节。总量未知时给 0
 * 并让列表显示「…」，而不是假装 100%。
 *
 * 状态映射：后端有 `running` 而没有 `downloading`/`installing`，界面两者都有。
 * `running` 归到 `downloading`——这是用户最常看到的阶段；后端的 `interrupted`
 * 则归到 `failed`，因为用户要做的处理（重试）是一样的。
 */
function taskToUi(task: NativeDownloadTask): DownloadTask {
  const percent = task.totalBytes ? Math.min(100, Math.round((task.completedBytes / task.totalBytes) * 100)) : 0;
  const status: DownloadTask['status'] = task.state === 'running'
    ? 'downloading'
    : task.state === 'interrupted'
      ? 'failed'
      : task.state;
  return {
    id: task.id,
    name: task.label,
    type: 'game',
    status,
    progress: percent,
    total: task.totalBytes ?? undefined,
    /** 后端任务只带 instanceId；下载页据此跳回对应实例。 */
    instanceId: task.instanceId,
    targetInstance: task.instanceId,
  };
}

// 只用来排序/遍历分类，标签由 t(CATEGORY_KEYS[id]) 提供，不在这里存文案。
const storeCategories = STORE_CATEGORIES;


function App() {
  const { theme, setTheme, resolvedTheme } = useTheme();
  const { t, language } = useI18n();
  const locale = LOCALES[language];
  const [context, setContext] = useState<AppContext>('first-run');
  const [instances, setInstances] = useState<GameInstance[]>([]);
  const [selectedInstance, setSelectedInstance] = useState<GameInstance | undefined>();
  /**
   * 主页那个大按钮要启动的实例。
   *
   * 与 `selectedInstance` 分开：后者是「当前正在看/管理的实例」，会随浏览、刷新、
   * 最近启动时间变化；这里是用户**明确固定**的一个选择，不该被别的操作改掉。
   *
   * 存 localStorage 而不是数据库：它是界面偏好，不是业务数据。后端已有
   * `last_launched_at` 用于「最近玩过」排序，那个值是自动变化的、无法被固定，
   * 两者解决的不是同一个问题。
   */
  const [defaultLaunchId, setDefaultLaunchId] = useState<string>(() => {
    try { return localStorage.getItem(DEFAULT_LAUNCH_KEY) ?? ''; } catch { return ''; }
  });
  const [currentAccount, setCurrentAccount] = useState<Account | undefined>();
  /**
   * 自由背景。
   *
   * 由 App 持有而不是设置面板：背景层在面板关闭后仍然要显示，面板只是它的编辑器。
   * 面板打开时通过 `onBackgroundChange` 回写，关闭时状态自然保留。
   */
  const [background, setBackground] = useState<BackgroundSetting>(readBackgroundSetting);
  /**
   * 轮播的当前帧。
   *
   * 放在 App 而不是背景层：强调色也要跟着这一帧走，两处各自计时一定会错位——
   * 界面会用上一张图的颜色配这一张图的背景。
   */
  const [slideIndex, setSlideIndex] = useState(0);
  /**
   * 最近使用的账户列表。
   *
   * 后端只存「哪些账户已添加」和「哪一个被选中」，没有单独的「最近使用」顺序。
   * 所以这里直接用后端账户列表：已选中项由 `currentAccount` 单独表示，列表本身
   * 就是可切换的全部账户。之前的三个演示账户是写死的假数据，现在已移除。
   */
  const [recentAccounts, setRecentAccounts] = useState<Account[]>([]);
  const [showAccountModal, setShowAccountModal] = useState(false);
  const [downloadTasks, setDownloadTasks] = useState<DownloadTask[]>([]);
  /**
   * 侧栏徽章的数字。
   *
   * 统计的是「仍在进行」的任务（排队/下载/校验/安装），而不是只看 downloading：
   * 点击「安装」时任务先进入 queued，若只数 downloading，徽章在排队阶段完全
   * 不出现，用户会以为没生效。
   */
  const activeTaskCount = downloadTasks.filter(task => ACTIVE_TASK_STATUSES.includes(task.status)).length;
  /**
   * 安装失败、需要修复的实例数。
   *
   * 只统计 `error`（后端 `failed`）。`installing` 不算——那是进行中的正常状态；
   * `not_installed` 也不算——那是还没开始装，不是出了问题。
   */
  const brokenCount = instances.filter(instance => instance.status === 'error').length;
  const downloadsLabel = activeTaskCount > 0
    ? t('app.downloadsActive', { values: { count: activeTaskCount } })
    : t('app.downloads');
  /** 某些操作完成后需要让实例列表反映后端的最新状态（例如安装完成）。 */
  const refreshInstances = useCallback(async () => {
    try {
      const records = await listInstances();
      const counts = await Promise.all(records.map(record => listMods(record.id).then(mods => mods.length).catch(() => 0)));
      const mapped = records.map((record, index) => toGameInstance(record, counts[index]));
      setInstances(mapped);
      /*
       * 同步选中的实例对象。
       *
       * `instances` 是新建的数组与对象，而 `selectedInstance` 仍指向刷新前的那个
       * 对象，它的 `status` / `modCount` / `lastPlayed` 都不会更新。结果是安装完成
       * 后卡片显示「已安装」，管理页却还显示「未安装」；模组数也停在旧值。
       *
       * 按 id 重新绑定；选中项已被删除时清空，避免界面停留在一个不存在的实例上。
       */
      setSelectedInstance(current => {
        if (!current) return current;
        return mapped.find(item => item.id === current.id);
      });
    } catch {
      // 刷新失败时保留当前列表；下一次事件或用户操作会再触发一次。
    }
  }, []);

  const [showSettings, setShowSettings] = useState(false);
  const [javaRuntimes, setJavaRuntimes] = useState<JavaRuntime[]>(loadJavaList);
  const [globalJavaPath, setGlobalJavaPath] = useState(() => {
    try { return JSON.parse(localStorage.getItem('flowlaunch-settings') || '{}').javaPath || ''; } catch { return ''; }
  });
  const [instanceJavaSettings, setInstanceJavaSettings] = useState<Record<string, InstanceJavaSetting>>(loadInstanceJavaSettings);
  /**
   * 全局设置的只读快照。
   *
   * 管理页需要显示「跟随全局时实际是什么值」，但全局设置在 SettingsPanel 里
   * 保存。这里存一份，在设置面板保存后同步，避免管理页读到一个陈旧的值。
   */
  const [globalPrefs, setGlobalPrefs] = useState<LauncherPrefs>(() => readPrefs());
  /** 后端返回的原始设置。保存时必须整份回传，否则未展示的字段会被默认值覆盖。 */
  const [naturalSettings, setNaturalSettings] = useState<LauncherSettings | null>(null);
  /**
   * 单文件下载进度，只在下载页作为细节展示。
   *
   * 内存信息不在这里缓存：`MemorySection` 自己会调 `get_system_memory`，再存一份
   * 就会出现两个来源，一个是刷新过的、一个是打开应用时的快照，反而容易不一致。
   */
  const [progress, setProgress] = useState<Array<{ artifactId: string; label: string; received: number; total: number | null; source: string; stage: string }>>([]);
  /**
   * 当前正在传输的文件数（并行下载数）。
   *
   * 与 `activeTaskCount` 是两个不同的量，必须分开：
   *   - 任务数：用户点了几个「安装」。一个任务会下载几十上百个文件，所以它长期
   *     停在 1，看不出下载器有多忙；
   *   - 并行文件数：此刻真正在跑的连接数，会随进度在 1..N 之间起伏。
   *
   * 只数 `downloading` 与 `connecting`：`verifying` 阶段文件已经下载完，不再占用
   * 带宽；把校验算进去会让数字在下载结束时虚高。`completed` / `failed` 同理排除。
   */
  const parallelDownloads = progress.filter(item => item.stage === 'downloading' || item.stage === 'connecting').length;
  const [gameStatus, setGameStatus] = useState<Record<string, { status: string; logPath: string }>>({});
  const [gameLogs, setGameLogs] = useState<Record<string, string>>({});
  /**
   * 启动进度。`null` 表示没有正在启动的实例。
   *
   * 与 `gameStatus` 分开：后者是「进程在不在跑」，这个是「启动到哪一步了」。
   * 一个实例启动成功后前者变 running、后者清空，两者的生命周期不重合。
   */
  const [launchProgress, setLaunchProgress] = useState<{ instanceId: string; stage: string; progress: number } | null>(null);
  const [instanceOverrides, setInstanceOverrides] = useState<OverrideMap>(loadInstanceOverrides);
  const [pluginStore, setPluginStore] = useState<PluginStore>(loadPluginStore);
  /**
   * 把界面里的插件状态推给后端一次。
   *
   * ## 为什么必须有这一步
   *
   * 后端 `plugin_state` 表初始是空的，而它对未出现的 id 一律按「启用」处理。前端
   * 的默认值不是这样：樱花主题、性能监控是 `enabledByDefault: false`。不推送的话，
   * 首次启动时后端会把它们当成已启用——用户看到「未启用」，启动时却按启用处理。
   *
   * 反方向也一样：用户在旧版本里关掉过某个插件，它的状态只在前端的 localStorage 里，
   * 后端不知道。一次推送让两边对齐，之后每次改动各自再同步。
   *
   * 只在桌面版执行：浏览器预览没有后端，`pluginSaveState` 会抛错。
   */
  useEffect(() => {
    if (!isDesktopShell()) return;
    void pluginSaveState(toPluginStatePayload(pluginStore)).catch(() => {
      // 推送失败不影响界面：用户下次改动任何一项时会再同步一次。
    });
    // 只在挂载时推一次；后续变化由 PluginWorkspace 内部同步。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  /** 改过但还没重启的插件（见 PluginWorkspace 的同名 prop）。 */
  const [pendingPluginRestart, setPendingPluginRestart] = useState<string[]>([]);
  /**
   * 可更新的插件数，用于侧栏红色徽章。
   *
   * 与 `attentionPluginCount` 分开：后者还包含实验性提示、不兼容、待重启，
   * 只有「可更新」是点进插件页就能立刻解决的事。徽章指向可执行动作，
   * 否则用户点进去找不到「更新」按钮，下次就忽略这个提示了。
   */
  const updatableCount = updatablePluginCount(pluginStore);
  const [showFirstRun, setShowFirstRun] = useState(true);
  const [storeCategory, setStoreCategory] = useState<StoreCategory>('vanilla');
  const [storeMenuOpen, setStoreMenuOpen] = useState(false);

  /** 新建实例对话框。之前「创建新游戏」只是跳到商店，用户实际无法创建实例。 */
  const [showCreateDialog, setShowCreateDialog] = useState(false);
  /** 待确认删除的实例。删除不可撤销，必须先问清楚。 */
  const [pendingDelete, setPendingDelete] = useState<GameInstance | null>(null);
  /** 删除时是否连带删掉磁盘文件。默认不删：误删记录可以重建，误删存档不能恢复。 */
  const [deleteFilesOnDisk, setDeleteFilesOnDisk] = useState(false);

  /** 界面提示条。原来的实现只在少数分支里用局部变量，接线后大量后端错误需要它。 */
  const [toast, setToast] = useState('');

  /**
   * 从后端重新读取任务列表。
   *
   * 事件只推送「发生变化的那一条」，所以列表的初始内容、以及事件丢失后的兜底
   * 都要靠这个函数。操作完成后主动调一次，避免界面停在一个已经过期的快照上。
   */
  const refreshDownloadTasks = useCallback(async () => {
    try {
      const tasks = await listDownloadTasks();
      setDownloadTasks(tasks.map(taskToUi));
      /*
       * 预览模式补一份文件级进度快照：浏览器里没有 `download-progress` 事件，
       * 不填的话下载页的「正在下载的文件」与侧栏并行数气泡永远是空的，无法调外观。
       * 生产构建中该函数返回空数组，这段等价于空操作。
       */
      const previewed = previewFileProgress();
      if (previewed.length > 0) setProgress(previewed);
    } catch {
      // 任务历史是辅助信息；读取失败时保留当前快照。
    }
  }, []);

  /**
   * 首屏加载：把后端状态灌进界面。
   *
   * 一次并发拉齐实例、账户、设置、Java 列表与内存信息。任何一项失败都不阻断裂
   * 面——`Promise.all` 里每项都带 `catch`，失败的那项给空值，其余照常显示。
   * 只有一个例外：错误的来源要能被看到，所以捕获到的第一条错误会进 toast。
   *
   * 实例的模组数需要额外一次 `list_mods`。这里对每个实例并发取一次；读的是本地
   * 目录，不触网，代价可以接受，换来的是实例卡片上的模组数真实可信。
   */
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const firstError: string[] = [];

      const [records, profiles, settings, runtimes] = await Promise.all([
        listInstances().catch(error => { firstError.push(errorText(error, '无法读取实例列表。')); return [] as InstanceRecord[]; }),
        listAccounts().catch(error => { firstError.push(errorText(error, '无法读取账户列表。')); return [] as AccountProfile[]; }),
        getSettings().catch(error => { firstError.push(errorText(error, '无法读取设置。')); return null as LauncherSettings | null; }),
        listJavaRuntimes().catch(() => [] as JavaRuntimeInfo[]),
      ]);

      // 模组数与实例并发取；读取失败的实例按 0 处理，不影响列表本身。
      const counts = await Promise.all(records.map(record => listMods(record.id).then(mods => mods.length).catch(() => 0)));
      if (cancelled) return;

      const mapped = records.map((record, index) => toGameInstance(record, counts[index]));
      setInstances(mapped);
      /*
       * 默认目标实例 = 上次启动的实例。
       *
       * 后端 `list_instances` 按 `last_launched_at` 降序返回（从未启动的按创建时间
       * 排在后面），所以第一项就是最近玩过的那个。之前这里取的是「列表第一项」但
       * 列表按创建时间排序，新实例总会抢走默认位，与用户的直觉相反。
       *
       * 用 `current ??` 保留用户当前的选择：刷新列表（安装完成、事件触发）时不能
       * 把他刚选中的实例改回默认，否则在商店里选好实例后，一旦有下载进度事件就会
       * 跳回别的实例——安装到错误的实例是真实的数据风险。
       */
      setSelectedInstance(current => current ?? mapped[0]);

      const mappedAccounts = profiles.map(toAccount);
      setRecentAccounts(mappedAccounts);
      // 后端用 `selected` 标记当前账户；没有选中项时保持未登录状态。
      setCurrentAccount(mappedAccounts.find((_, index) => profiles[index].selected));

      if (settings) {
        setGlobalPrefs(prefFromSettings(settings));
        setGlobalJavaPath(settings.javaPath ?? '');
        setNaturalSettings(settings);
      }
      // Java 列表：后端的探测结果覆盖 localStorage 里的旧快照。
      setJavaRuntimes(runtimes.map(toJavaRuntime));

      /*
       * 首屏也要读一次任务列表。
       *
       * 之前只在用户操作后刷新，导致重启启动器时下载页是空的：上一次未完成或
       * 刚完成的任务全都看不见，用户会以为任务丢了——而它们其实还在后端表里。
       * 事件只推送变化，补不上「当前已有什么」。
       */
      void refreshDownloadTasks();

      // 只有桌面版才存在「演示数据」之外的首次运行引导；接后端后直接进主界面。
      setShowFirstRun(false);
      /*
       * 首屏落在主页而不是游戏库。
       *
       * 打开启动器最常见的意图是「进游戏」，主页就是那个大按钮；游戏库是管理
       * 实例的地方。先给人看管理列表，等于把主路径往后推了一步。
       */
      setContext('dashboard');

      if (firstError.length) setToast(firstError[0]);
    })();
    return () => { cancelled = true; };
  }, []);


  /**
   * 订阅后端事件。
   *
   * 三个事件各管一件事，不能合并：
   *   `installation-task` —— 任务级状态，驱动侧栏徽章与下载列表；
   *   `download-progress` —— 单文件进度，只用于「正在下载什么」的细节；
   *   `game-process` / `game-log-line` —— 游戏生命周期与最后一行日志。
   *
   * 事件到达时刷新一次实例列表：安装完成后后端会把状态从 not_installed 改成
   * ready，不刷新的话卡片上的按钮仍然显示「安装」。
   */
  useEffect(() => {
    if (!isDesktopShell()) return;
    let cancelled = false;
    const unlisteners: Array<(() => void) | null> = [];

    void (async () => {
      const subscriptions = await Promise.all([
        onInstallationTask(payload => {
          setDownloadTasks(current => {
            const next = current.filter(task => task.id !== payload.task.id);
            return [taskToUi(payload.task), ...next];
          });
        }),
        onDownloadProgress(payload => {
          // 同一文件只保留最新进度，避免列表被同一个 artifact 刷屏。
          setProgress(current => {
            const next = current.filter(item => item.artifactId !== payload.artifactId);
            return [...next, payload].slice(-60);
          });
        }),
        onGameProcess(payload => {
          setGameStatus(current => ({ ...current, [payload.instanceId]: { status: payload.status, logPath: payload.logPath } }));
          if (payload.status === 'crashed') setToast('游戏异常退出；可在实例管理中查看日志。');
          // 进程结束后实例状态可能已经变化，刷新列表让按钮回到正确状态。
          void refreshInstances();
        }),
        onGameLogLine(payload => {
          setGameLogs(current => ({ ...current, [payload.instanceId]: payload.line }));
        }),
        /*
         * 启动阶段进度。`running` 表示进程已起来，进度到此结束——把状态清掉即可，
         * 不需要保留一条 100% 的进度条（PCL2 也是启动完成后立刻收起）。
         */
        onLaunchProgress(payload => {
          setLaunchProgress(payload.stage === 'running'
            ? null
            : { instanceId: payload.instanceId, stage: payload.stage, progress: payload.progress });
        }),
      ]);
      if (cancelled) subscriptions.forEach(fn => fn?.());
      else unlisteners.push(...(subscriptions as Array<(() => void) | null>));
    })();

    return () => {
      cancelled = true;
      unlisteners.forEach(fn => fn?.());
    };
  }, []);

  const completeFirstRun = () => {
    setShowFirstRun(false);
    // 与首屏加载一致：引导结束后落在主页，而不是游戏库。
    setContext('dashboard');
  };

  /** 新建实例：先创建记录，再跳到商店让用户挑版本并安装。 */
  /**
   * 新建实例。
   *
   * 后端 `create_instance` 只写一条实例记录，不下载文件——下载是随后的
   * `install_instance`。所以这里创建成功后就刷新列表并选中新实例，让用户立刻
   * 能在卡片上点「安装」，而不是被丢回一个看不出发生什么事的列表。
   */
  const handleCreateInstance = useCallback(async (name: string, version: string) => {
    const record = await createInstanceNative(name, version);
    await refreshInstances();
    // 用后端返回的 id 精确定位，而不是按名字猜：同名实例是允许的。
    const mapped = toGameInstance(record);
    setSelectedInstance(mapped);
    setToast(t('create.created', { values: { name: record.name } }));
  }, [refreshInstances, t]);

  /**
   * 删除实例。
   *
   * 两个都必须由用户明确回答的问题：
   *   1. 是否删除文件（后端 `deleteFiles`）——只删记录会让磁盘上的世界留下来，
   *      下次建同名实例时可能撞上旧目录；
   *   2. 确认这一步不可撤销。
   * 先删记录再删文件是后端的顺序，所以文件删除失败时实例记录已经没了；这里如实
   * 报错并刷新列表，不假装成功。
   */
  const handleDeleteInstance = async (instance: GameInstance, deleteFiles: boolean) => {
    try {
      await deleteInstanceNative(instance.id, deleteFiles);
      setPendingDelete(null);
      if (selectedInstance?.id === instance.id) setSelectedInstance(undefined);
      await refreshInstances();
      await refreshDownloadTasks();
      setToast(t('instance.deleted', { values: { name: instance.name } }));
    } catch (error) {
      setToast(errorText(error, '删除实例失败。'));
      await refreshInstances();
    }
  };

  const createNewInstance = () => {
    setShowCreateDialog(true);
  };

  /**
   * 商店里点「原版条目」的安装：交给后端，不再本地伪造任务行。
   *
   * 旧实现是在前端 push 一条 `queued` 任务，界面看起来"接上了"，但后端根本没
   * 收到请求。现在真正的进度由 `installation-task` 事件推回来。
   *
   * `type` 参数保留是为了不改变调用方的签名（商店对所有分类共用同一个回调），
   * 但原版安装走的是 `install_instance`；Mod/整合包的安装入口在商店行内，由
   * StoreWorkspace 直接调用对应命令。
   */
  /**
   * 触发一次实例的原版文件安装/修复。
   *
   * `repairing` 只影响文案：后端是同一条 `install_instance` 路径——下载引擎会先
   * 校验已存在的文件（大小 + SHA-1），通过的跳过、缺失或损坏的重新下载。对
   * `failed` 实例来说这正是「校验并补全」，但用户看到的是一个坏掉的实例，
   * 提示里说「安装」会让他以为全部重来一遍。
   */
  const queueStoreDownload = (
    name: string,
    _type: DownloadTask['type'],
    targetInstance?: string,
    repairing = false,
  ) => {
    if (!isDesktopShell()) {
      setToast(repairing
        ? '修复需要桌面应用后端；浏览器里只能预览界面。'
        : '安装需要桌面应用后端；浏览器里只能预览界面。');
      return;
    }
    // 商店里能直接触发安装的是「原版游戏」条目：它对应一个真实的实例安装。
    // Mod / 整合包的安装入口在商店行内，由 StoreWorkspace 直接调后端命令。
    const target = instances.find(item => item.id === targetInstance) ?? selectedInstance;
    if (!target) {
      setToast('请先创建一个实例，再安装游戏文件。');
      return;
    }
    void (async () => {
      try {
        setToast(repairing ? `正在校验并修复 ${name}…` : `正在安装 ${name}…`);
        await installInstanceNative(target.id);
        await refreshInstances();
        setToast(repairing ? `${name} 已修复完成。` : `${name} 安装完成。`);
      } catch (error) {
        setToast(errorText(error, repairing ? '修复失败。' : '安装失败。'));
        await refreshInstances();
      }
    })();
  };

  const selectStoreInstance = (instanceId: string) => {
    const instance = instances.find(item => item.id === instanceId);
    setSelectedInstance(instance);
  };

  /**
   * 展开/收起内容商店抽屉。
   *
   * 之前是一个 `position: fixed` 的浮层：需要根据按钮位置算坐标、监听 resize 与
   * scroll 来关闭，而且浮在侧栏之上遮住了相邻导航项。抽屉是内联展开的——它把下面
   * 的导航项推开，层级关系由文档流直接表达，因此不需要任何定位计算，也不会在窗口
   * 变化时跑到错误的位置。
   *
   * 收起时机见下面两个 effect：选分类不收起，离开内容商店才收起。
   */
  const toggleStoreMenu = () => setStoreMenuOpen(open => !open);

  /**
   * 选择内容商店的分类。
   *
   * 刻意**不收起抽屉**：分类之间是并列的，用户经常要在「Mod → 资源包 → Mod」
   * 之间来回切换。每选一次就收起，等于每次切换都要再点开一次内容商店。
   * 抽屉只在「离开内容商店」时收起（点游戏库/下载任务/插件等），见下面的 effect。
   */
  const chooseStoreCategory = (category: StoreCategory) => {
    setStoreCategory(category);
    setContext('store');
  };

  /*
   * 抽屉的收起时机。
   *
   * 规则是「选了分类不收起，离开内容商店才收起」：
   *   - 切到其他大导航项（游戏库 / 下载任务 / 插件）→ 收起；
   *   - 点侧栏之外的界面（正文区域）→ **不**收起。
   *
   * 之前是「点任意非抽屉区域就收起」，那个判定把正文点击也算进去了：在商店里
   * 点一下搜索框或结果行，抽屉就被收走，用户每次都要重新点开内容商店。
   * 抽屉是内联的、占文档流，常驻不会遮挡任何东西，所以没有必要为一次无关的
   * 点击而收起它。
   *
   * 退出应用的 `Escape` 仍然收起，键盘用户需要一个不必点鼠标的退出方式。
   */
  useEffect(() => {
    if (!storeMenuOpen) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setStoreMenuOpen(false);
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [storeMenuOpen]);

  // 离开内容商店（切到别的大框）时收起抽屉。
  useEffect(() => {
    if (context !== 'store') setStoreMenuOpen(false);
  }, [context]);

  const persistInstanceJava = (instanceId: string, setting: InstanceJavaSetting) => {
    const next = { ...instanceJavaSettings, [instanceId]: setting };
    setInstanceJavaSettings(next);
    saveInstanceJavaSettings(next);
  };

  /**
   * 保存一个实例的覆盖设置。
   *
   * 每次点击都立即落盘，不跟全局设置共用「保存/关闭」流程：管理页里改动一项
   * 就是一次明确的选择，再要求用户按保存容易漏掉。空对象直接删掉该实例的
   * 记录，避免 localStorage 里堆满空条目。
   */
  /**
   * 插件状态立即落盘。
   *
   * 与实例覆盖同理：开关一个插件是一次明确的选择，再要求按保存容易漏掉；
   * 而且插件与全局设置无关，没有「取消」的语义。
   */
  const persistPlugins = (next: PluginStore) => {
    setPluginStore(next);
    savePluginStore(next);
  };
  /**
   * 保存某个实例的设置覆盖。
   *
   * 必须走后端：启动命令由 Rust 生成，只写 localStorage 的话界面显示「已覆盖」
   * 而实际启动仍按全局值跑——这类偏差极难被用户发现。后端还会做组合校验（例如
   * 覆盖后的内存是否超过本机上限），界面无法替代。
   *
   * 乐观更新：先改本地状态让控件立刻响应，再落库。失败时回滚并提示，避免界面
   * 停在一个后端并不认可的值上。
   */
  const persistOverrides = (instanceId: string, next: InstanceOverrides) => {
    const previous = instanceOverrides;
    const map = { ...previous };
    if (Object.keys(next).length === 0) delete map[instanceId];
    else map[instanceId] = next;
    setInstanceOverrides(map);

    void saveInstanceSettingsNative(instanceId, toNativeOverrides(next))
      .then(count => {
        // 后端会丢弃「无意义」的覆盖（例如清空的文本），它返回的计数才是真相。
        // 与本地键数不一致时回读一次，把界面校正到后端实际保存的内容。
        if (count !== Object.keys(next).length) {
          void getInstanceSettings(instanceId)
            .then(view => {
              const stored = toUiOverrides(view.overrides);
              setInstanceOverrides(current => {
                const corrected = { ...current };
                if (Object.keys(stored).length === 0) delete corrected[instanceId];
                else corrected[instanceId] = stored;
                return corrected;
              });
            })
            .catch(() => undefined);
        }
      })
      .catch(error => {
        setInstanceOverrides(previous);
        setToast(errorText(error, '无法保存实例设置。'));
      });
  };

  /**
   * 实例设置页的初始数据来自后端。
   *
   * 之前这份数据只从 localStorage 读，导致桌面版用户看到的是一份「从未设置过」
   * 的清单，而后端其实存着覆盖值。选中实例时拉一次，覆盖本地缓存。
   */
  const loadInstanceSettings = useCallback(async (instanceId: string) => {
    try {
      const view = await getInstanceSettings(instanceId);
      const stored = toUiOverrides(view.overrides);
      setInstanceOverrides(current => {
        const next = { ...current };
        if (Object.keys(stored).length === 0) delete next[instanceId];
        else next[instanceId] = stored;
        return next;
      });
    } catch {
      // 读取失败时保留本地缓存；用户下次改动会再写一次。
    }
  }, []);

  /** 进入实例详情时同步一次后端值，避免显示一份过期的本地缓存。 */
  useEffect(() => {
    if (context !== 'instance-detail' || !selectedInstance) return;
    void loadInstanceSettings(selectedInstance.id);
  }, [context, loadInstanceSettings, selectedInstance]);

  /**
   * 取消一个下载任务。
   *
   * 必须调用后端的 `control_download`：只把界面上那一行改成「已取消」并不会
   * 停止下载。后端的最终状态会通过 `installation-task` 事件推回来。
   */
  const cancelTask = (taskId: string) => {
    const task = downloadTasks.find(item => item.id === taskId);
    if (!task) return;
    // 后端按实例定位任务；没有 instanceId 的行无法定位，直接说明而不是静默失败。
    if (!task.instanceId) {
      setToast('这条任务没有关联实例，无法取消。');
      return;
    }
    void (async () => {
      try {
        await controlDownload(task.instanceId!, 'cancel');
      } catch (error) {
        setToast(errorText(error, '无法取消此任务。'));
      }
    })();
  };

  /**
   * 主页按钮要启动的实例。
   *
   * 优先级：用户固定的默认实例 → 没有就用最近玩过的（`selectedInstance`，
   * 后端按 `last_launched_at` 排序取第一项）→ 再没有就取列表第一个。
   *
   * 回退链是必要的：用户可能删掉了固定的那个实例，或者首次打开还没有默认值。
   * 任一步失败都不该让主页变成空白，那是把「默认值未设置」显示成「不可用」。
   */
  const dashboardTarget = useMemo(() => {
    const pinned = instances.find(instance => instance.id === defaultLaunchId);
    if (pinned) return pinned;
    if (selectedInstance) return instances.find(instance => instance.id === selectedInstance.id) ?? selectedInstance;
    return instances[0];
  }, [defaultLaunchId, instances, selectedInstance]);

  /** 设置/切换默认启动实例，并写回本地偏好。 */
  const selectDefaultTarget = (instanceId: string) => {
    setDefaultLaunchId(instanceId);
    try {
      localStorage.setItem(DEFAULT_LAUNCH_KEY, instanceId);
    } catch {
      // 存储不可用（隐私模式）时只在本次会话生效，不影响启动本身。
    }
    // 同步到外层选择，让「游戏库」与实例详情页也指向同一个实例。
    setSelectedInstance(instances.find(instance => instance.id === instanceId));
  };

  /**
   * 游戏库卡片上的「主页默认」开关。
   *
   * 与直接点卡片上的「启动」是两件事：
   *   - 启动：这一次跑这个实例，默认值不动；
   *   - 设为默认：以后主页那个按钮跑这个实例，但这次不启动。
   * 把两者分开，用户才能「先设好常用的，再慢慢配模组」，而不是每次都得以
   * 启动的方式去表达偏好。
   *
   * 取消固定的语义是「回到自动」而不是「没有默认」：清掉键之后，主页回落到
   * 后端按 `last_launched_at` 排出来的第一项，也就是最近玩过的那个。删掉键
   * 而不是写空串，是为了让 `localStorage` 里不留无意义的值。
   */
  const toggleDefaultLaunch = (instance: GameInstance) => {
    if (defaultLaunchId === instance.id) {
      setDefaultLaunchId('');
      try {
        localStorage.removeItem(DEFAULT_LAUNCH_KEY);
      } catch {
        // 存储不可用时只影响本次会话之后的行为，界面状态已经改过来了。
      }
      setToast(t('app.defaultCleared'));
      return;
    }
    selectDefaultTarget(instance.id);
    setToast(t('app.defaultSetToast', { values: { name: instance.name } }));
  };

  /** 这个实例是不是主页按钮当前的启动对象（用于卡片上的标记）。 */
  const isDefaultLaunch = (instanceId: string) =>
    defaultLaunchId !== '' && defaultLaunchId === instanceId;

  /**
   * 启动游戏。
   *
   * 三件事按顺序发生：确认有账户、离线档案二次确认、把请求交给后端。离线确认
   * 放在前端是因为这是「知情同意」，必须让用户看到具体是哪个档案；后端同样会
   * 拒绝未确认的离线启动，两层都不能省。
   */
  const launchInstance = (instance: GameInstance) => {
    void (async () => {
      const account = currentAccount;
      if (!account) {
        setShowAccountModal(true);
        setToast('请先添加一个账户。');
        return;
      }
      let offlineConfirmed = false;
      if (account.type === 'offline') {
        offlineConfirmed = window.confirm(
          `你正在使用离线档案「${account.username}」启动。\n\n离线档案不验证 Microsoft 游戏授权，通常只能连接允许离线身份的服务器。仍要继续吗？`,
        );
        if (!offlineConfirmed) return;
      }
      try {
        await launchInstanceNative(instance.id, account.id, offlineConfirmed);
      } catch (error) {
        /*
         * 启动失败要把进度条收掉。
         *
         * 后端只在进程真正起来后才发 `running`；如果它在认证或拼参数阶段就失败，
         * 那条进度条会一直停在中间，看起来像卡死。这里主动清空，把错误交给 toast。
         */
        setLaunchProgress(null);
        setToast(errorText(error, '启动失败；请确认此实例已安装完成。'));
      }
    })();
  };

  /** 停止游戏进程。需要二次确认：未保存的进度会丢失。 */
  const stopInstance = (instanceId: string) => {
    if (!window.confirm('关闭正在运行的 Minecraft 进程？未保存的游戏进度可能会丢失。')) return;
    void stopGame(instanceId).catch(error => setToast(errorText(error, '无法关闭游戏进程。')));
  };

  /**
   * 重新读取账户列表。
   *
   * 登录、切换、移除之后都要走一次：后端用 `selected` 标记当前账户，而移除操作
   * 会连带删掉凭据，选中项可能因此变化。只改本地状态会让侧栏显示一个已不存在的
   * 账户。
   */
  const refreshAccounts = useCallback(async () => {
    try {
      const profiles = await listAccounts();
      const mapped = profiles.map(toAccount);
      setRecentAccounts(mapped);
      setCurrentAccount(mapped.find((_, index) => profiles[index].selected));
    } catch {
      // 读取失败时保留当前显示；下一次操作会再试。
    }
  }, []);

  /**
   * 登录完成后的统一入口。
   *
   * 后端已经把账户写进数据库并标记为选中，这里只需要把它反映到界面。重新拉一次
   * 账户列表而不是手工拼装：`selected` 标记的翻转由后端负责，前端猜容易和真实
   * 状态错位。
   */
  const handleLogin = (account: Account | undefined) => {
    setCurrentAccount(account);
    if (!account) return;
    void listAccounts()
      .then(profiles => {
        const mapped = profiles.map(toAccount);
        setRecentAccounts(mapped);
        setCurrentAccount(mapped.find((_, index) => profiles[index].selected) ?? account);
      })
      .catch(() => {
        // 刷新失败时至少保留刚登录的账户，界面不至于变空。
        setRecentAccounts(prev => [account, ...prev.filter(item => item.id !== account.id)]);
      });
  };

  /**
   * 退出登录。
   *
   * 后端没有「取消选中」命令，所以退出等价于切到另一个账户。只有一个账户时
   * 无法退出——这是后端的模型决定的，界面如实告知，而不是假装清空了状态。
   */
  const signOut = () => {
    if (!currentAccount) return;
    if (!window.confirm(t('app.signOutConfirm', { values: { name: currentAccount.username } }))) return;
    const other = recentAccounts.find(item => item.id !== currentAccount.id);
    if (!other) {
      setToast('这是唯一的账户；请先添加另一个账户再退出。');
      return;
    }
    void (async () => {
      try {
        await selectAccountNative(other.id);
        const profiles = await listAccounts();
        const mapped = profiles.map(toAccount);
        setRecentAccounts(mapped);
        setCurrentAccount(mapped.find((_, index) => profiles[index].selected));
        setToast(t('app.signedOut'));
      } catch (error) {
        setToast(errorText(error, '切换账户失败。'));
      }
    })();
  };

  /**
   * 修改下载源并立即落库。
   *
   * 由插件页的「优先使用镜像源」开关调用。必须真的写后端设置：那个开关的效果就是
   * 改变下载源，如果只改本地状态，用户拨动开关后下载行为不会变——那就是一个装饰品。
   *
   * 与 `toggleTheme` 同一套做法：基于 `naturalSettings`（上次从后端读到的完整设置）
   * 改一个字段再整份保存，避免把界面不展示的字段重置成默认值。
   */
  const persistDownloadSource = async (source: string) => {
    const next = { ...globalPrefs, downloadSource: source };
    setGlobalPrefs(next);
    try {
      const base = naturalSettings ?? (await getSettings());
      const merged = prefsToSettings(next, base);
      await saveSettingsNative(merged);
      setNaturalSettings(merged);
    } catch (error) {
      // 保存失败要回滚显示，否则界面会停在一个后端并不认可的值上。
      setGlobalPrefs(globalPrefs);
      setToast(errorText(error, '无法保存下载源设置。'));
    }
  };

  const toggleTheme = () => {    const newTheme = resolvedTheme === 'light' ? 'dark' : 'light';
    setTheme(newTheme);
    // 主题模式是后端设置的一部分；写回后端才能让下次启动保持一致。
    if (naturalSettings) {
      const next: LauncherSettings = { ...naturalSettings, themeMode: newTheme };
      setNaturalSettings(next);
      void saveSettingsNative(next).catch(() => undefined);
    }
  };

  /*
   * 轮播计时。
   *
   * 只在「轮播 + 至少两张图 + 没有开启减少动效」时启动。三个条件缺一不可：
   * 单张图不需要切换，一张图也切不出变化，而减少动效是用户的明确要求。
   *
   * 用 `setInterval` 而不是递归 `setTimeout`：间隔固定，不需要在每次回调里重设。
   * 依赖里放的是图片张数与间隔，不放进 `slideIndex`——把索引放进依赖会让每次
   * 切换都重建定时器，实际间隔变成「间隔 + 一次渲染」，越切越慢。
   */
  useEffect(() => {
    if (background.mode !== 'slideshow' || background.images.length < 2) return;
    if (typeof window !== 'undefined' && window.matchMedia?.('(prefers-reduced-motion: reduce)').matches) return;
    const timer = window.setInterval(() => {
      setSlideIndex(current => current + 1);
    }, Math.max(3, background.intervalSec) * 1000);
    return () => window.clearInterval(timer);
  }, [background.mode, background.images.length, background.intervalSec]);

  /*
   * 背景换成一另一组图、或切模式时把索引归零。
   *
   * 不归零会出现「切成单张了，但索引还是 5」——此时虽然只有一个元素能显示，
   * 但再切回轮播时不是从第一张开始，用户会以为顺序乱了。
   */
  useEffect(() => {
    setSlideIndex(0);
  }, [background.mode, background.images.length]);

  /**
   * 应用强调色。
   *
   * 三种情况：
   *   1. 跟随背景配色且能取到颜色 → 用背景的颜色；
   *   2. 跟随背景、有素材但还没算出颜色（图片尚在加载）→ 先不动，避免闪一下默认蓝；
   *   3. 跟随背景但已经没有素材（用户清空了背景）→ 回落到手动选的强调色。
   *
   * 第 3 条不能与第 2 条合并：两者都是「auto 模式下取不到颜色」，但含义相反。
   * 合并之后清空背景会把上一个视频/图片的强调色永远留在界面上——用户已经删掉了
   * 那张图，界面却还是它的颜色。
   *
   * 依赖里带上 `resolvedTheme`：`applyAccent` 会按主题算文字色，切主题时需要重算。
   */
  useEffect(() => {
    const manual = globalPrefs.customAccent || DEFAULT_ACCENT;
    const auto = autoAccentOf(background, slideIndex);
    if (background.accentMode === 'auto') {
      if (auto) applyAccent(auto);
      else if (background.mode === 'none') applyAccent(manual);
      // 否则是「有素材但颜色还没算出来」，保持现状，避免闪一下默认色。
      return;
    }
    applyAccent(manual);
  }, [background, slideIndex, globalPrefs.customAccent, resolvedTheme]);

  // 首次运行引导
  if (showFirstRun) {
    return (
      <div className={styles.app}>
        <div className={styles.firstRun}>
          <div className={styles.firstRunContent}>
            <div className={styles.logo}>
              <div className={styles.logoIcon}>MFL</div>
            </div>
            <h1 className={styles.firstRunTitle}>{t('app.firstRunTitle')}</h1>
            <p className={styles.firstRunDescription}>
              {t('app.firstRunTagline')}
              <br />
              {t('app.firstRunSubtagline')}
            </p>
            <div className={styles.firstRunActions}>
              <Button size="lg" onClick={completeFirstRun} icon={<PlayIcon />}>
                {t('app.getStarted')}
              </Button>
            </div>
            <div className={styles.firstRunFeatures}>
              <div className={styles.feature}>
                <ShopIcon />
                <span>{t('app.featureInstall')}</span>
              </div>
              <div className={styles.feature}>
                <SettingsIcon />
                <span>{t('app.featureJava')}</span>
              </div>
              <div className={styles.feature}>
                <LibraryIcon />
                <span>{t('app.featureInstances')}</span>
              </div>
            </div>
          </div>
        </div>
      </div>
    );
  }

  // 主界面
  return (
    <div className={styles.app} data-has-background={background.mode !== 'none' ? 'true' : undefined}>
      {/*
        背景层：铺在一切之下。
        放在 `.app` 内部而不是 body 上，是为了让它与侧栏、内容区共享同一个裁剪
        区域；将来给窗口加圆角时，背景不会从角上漏出去。
      */}
      <BackgroundLayer setting={background} dark={resolvedTheme === 'dark'} slideIndex={slideIndex} />

      {/* 侧边栏 */}
      <aside className={styles.sidebar}>
        <div className={styles.sidebarHeader}>
          <div className={styles.logo}>
            <div className={styles.logoIconSmall}>MFL</div>
            <span className={styles.logoText}>MFMC Launcher</span>
          </div>
        </div>

        <nav className={styles.nav}>
          {/*
            主页排在最前，与游戏库分开。
            两者分工不同：主页只有「启动默认实例」一件事，游戏库是管理实例的地方。
            之前「游戏库」同时承担两者，用户打开启动器想直接进游戏时，得先在一堆
            卡片里找到自己常用的那个再点启动。
          */}
          <button
            className={`${styles.navItem} ${context === 'dashboard' ? styles.active : ''}`}
            onClick={() => setContext('dashboard')}
            aria-label={t('app.dashboard')}
            title={t('app.dashboard')}
          >
            <HomeIcon />
            <span>{t('app.dashboard')}</span>
          </button>
          <button
            className={`${styles.navItem} ${context === 'home' ? styles.active : ''}`}
            onClick={() => setContext('home')}
            aria-label={t('app.library')}
            title={t('app.library')}
          >
            <LibraryIcon />
            <span>{t('app.library')}</span>
            {/*
              两个徽章并列，含义不同：
                - 灰色数字：库里有几个实例（规模信息）；
                - 红色数字：其中几个安装失败、需要修复（待办事项）。
              红色只在有问题时出现，所以它出现本身就说明「有事要做」；
              把两者合并成一个数字会让人以为是同一个意思。
            */}
            {instances.length > 0 && (
              <span className={styles.badge}>{instances.length}</span>
            )}
            {brokenCount > 0 && (
              <span
                className={`${styles.badge} ${styles.badgeProblem}`}
                title={t('app.brokenHint', { count: brokenCount })}
                aria-label={t('app.brokenHint', { count: brokenCount })}
              >
                {brokenCount > 99 ? '99+' : brokenCount}
              </span>
            )}
          </button>
          <div className={styles.storeMenuWrap}>
            <button
              className={`${styles.navItem} ${context === 'store' ? styles.active : ''} ${storeMenuOpen ? styles.menuOpen : ''}`}
              onClick={toggleStoreMenu}
              aria-label={t('app.store')}
              /*
               * 用 `aria-expanded` + `aria-controls` 而不是 `aria-haspopup="menu"`：
               * 抽屉是一组并列的分类（`role="group"`），不是弹出式命令菜单。
               * 声明成 menu 会让读屏期待方向键在菜单项间移动，而这里的交互只是
               * 普通的展开/收起列表。
               */
              aria-expanded={storeMenuOpen}
              aria-controls={storeMenuOpen ? 'store-drawer' : undefined}
              title={t('app.store')}
            >
              <ShopIcon />
              <span>{t('app.store')}</span>
              <ChevronDown className={styles.navChevron} size={14} aria-hidden="true" />
            </button>
            {/*
              内联抽屉：展开时把下方的导航项推开，而不是浮在它们上面。
              `role="group"` 而不是 `menu`：这是一组导航目的地，不是对某项操作的
              命令列表；用 menu 会让读屏期待方向键在菜单项间移动。
            */}
            {storeMenuOpen && (
              <div
                className={styles.storeDrawer}
                id="store-drawer"
                role="group"
                aria-label={t('app.chooseDownloadTypeLabel')}
              >
                <div className={styles.storeDrawerHeader}>{t('app.chooseDownloadType')}</div>
                {storeCategories.map(item => (
                  <button
                    key={item}
                    type="button"
                    aria-current={storeCategory === item ? 'true' : undefined}
                    className={storeCategory === item ? styles.storeDrawerItemActive : styles.storeDrawerItem}
                    onClick={() => chooseStoreCategory(item)}
                  >
                    <span className={styles.storeMenuItemIcon}>
                      {/*
                        原版用重绘的线性图标，与另外四项同一套笔触。
                        之前这里是一张 16×16 像素贴图，和四个 lucide 线稿并排时
                        质感与视觉重量都不一致（详见 VanillaIcon 的头部说明）。
                      */}
                      {item === 'vanilla' ? <VanillaIcon size={18} /> : CATEGORY_ICONS[item]}
                    </span>
                    <span>{t(CATEGORY_KEYS[item])}</span>
                    {storeCategory === item && <Check className={styles.storeMenuCheck} size={15} />}
                  </button>
                ))}
              </div>
            )}
          </div>
          <button
            className={`${styles.navItem} ${context === 'downloads' ? styles.active : ''}`}
            onClick={() => setContext('downloads')}
            aria-label={downloadsLabel}
            title={parallelDownloads > 0
              ? t('app.parallelHint', { values: { count: parallelDownloads } })
              : downloadsLabel}
          >
            <DownloadIcon />
            <span>{t('app.downloads')}</span>
            {/*
              右侧气泡显示「此刻并行下载的文件数」，而不是任务数。
              两者含义不同：任务数长期停在 1（一次安装就是一个任务），看不出下载器
              的繁忙程度；并行文件数会随进度实时起伏，才是「现在有多忙」的答案。
              任务数保留在 aria-label / title 里，供读屏与悬停查看。
            */}
            {parallelDownloads > 0 && (
              <span
                className={`${styles.badge} ${styles.badgeLive}`}
                aria-hidden="true"
              >
                {parallelDownloads > 99 ? '99+' : parallelDownloads}
              </span>
            )}
          </button>
          <button
            className={`${styles.navItem} ${context === 'plugins' ? styles.active : ''}`}
            onClick={() => setContext('plugins')}
            aria-label={t('app.plugins')}
            title={t('app.plugins')}
          >
            <PluginsIcon />
            <span>{t('app.plugins')}</span>
            {/*
              红色徽章只统计「可更新」的插件。
              用红色底而不是灰色：灰色只是计数，红色表示「有事要做」——与游戏库的
              失败实例徽章用同一套语义，用户不必分别记忆两种意思。
              其他需要注意的情况（实验性、不兼容、待重启）仍会在插件页的
              「需要注意」筛选里列出，但不占这个徽章。
            */}
            {updatableCount > 0 && (
              <span
                className={`${styles.badge} ${styles.badgeProblem}`}
                title={t('app.pluginUpdatesHint', { count: updatableCount })}
                aria-label={t('app.pluginUpdatesHint', { count: updatableCount })}
              >
                {updatableCount > 99 ? '99+' : updatableCount}
              </span>
            )}
          </button>
        </nav>

        <div className={styles.sidebarFooter}>
          <button className={styles.navItem} onClick={toggleTheme} aria-label={t('app.switchTheme')} title={t('app.switchTheme')}>
            {resolvedTheme === 'light' ? <MoonIcon /> : <SunIcon />}
            <span>{resolvedTheme === 'light' ? t('app.themeToDark') : t('app.themeToLight')}</span>
          </button>
          <button
            className={styles.navItem}
            onClick={() => setShowAccountModal(true)}
            aria-label={t('app.account')}
            title={t('app.account')}
          >
            <SidebarAccountAvatar account={currentAccount} />
            <span>{currentAccount ? currentAccount.username : t('app.login')}</span>
            {/*
              已登录时在名字右侧放一个「退出登录」按钮。用 <span role="button"> 而
              不是嵌套 <button>：HTML 不允许 button 套 button，浏览器会把内层元素
              拆到外面，点击区域和键盘焦点都会错位。这里用 role + tabIndex 保留
              键盘可达性，并在 onClick 里阻止冒泡，避免点退出时又打开账户弹窗。
            */}
            {currentAccount && (
              <span
                role="button"
                tabIndex={0}
                className={styles.signOut}
                aria-label={t('app.signOut')}
                title={t('app.signOut')}
                onClick={event => { event.stopPropagation(); signOut(); }}
                onKeyDown={event => {
                  if (event.key === 'Enter' || event.key === ' ') {
                    event.preventDefault();
                    event.stopPropagation();
                    signOut();
                  }
                }}
              >
                <SignOutIcon />
              </span>
            )}
          </button>
          <button
            className={styles.navItem}
            onClick={() => setShowSettings(true)}
            aria-label={t('app.settings')}
            title={t('app.settings')}
          >
            <SettingsIcon />
            <span>{t('app.settings')}</span>
          </button>
        </div>
      </aside>

      {/* 主内容区 */}
      <main className={styles.main}>
        {/*
          主页：一个启动按钮 + 默认实例选择。
          与游戏库分开的理由见 Dashboard 的头部注释——启动是高频主路径，
          管理是低频操作，塞在同一页会让主路径变慢。
        */}
        {context === 'dashboard' && (
          <Dashboard
            instances={instances}
            target={dashboardTarget}
            currentAccount={currentAccount}
            launching={launchProgress}
            running={Boolean(dashboardTarget && gameStatus[dashboardTarget.id]?.status === 'running')}
            onLaunch={launchInstance}
            onStop={stopInstance}
            onSelectTarget={selectDefaultTarget}
            onCreate={createNewInstance}
            onOpenLibrary={() => setContext('home')}
            iconForLoader={iconForLoader}
          />
        )}        {/* 空状态 - 无游戏 */}
        {context === 'home' && instances.length === 0 && (
          <div className={styles.emptyState}>
            <div className={styles.emptyStateContent}>
              <LibraryIcon />
              <h2>{t('app.emptyTitle')}</h2>
              <p>{t('app.emptyHint')}</p>
              <Button icon={<PlusIcon />} onClick={createNewInstance}>
                {t('app.createGame')}
              </Button>
            </div>
          </div>
        )}

        {/* 游戏库 - 快速启动中心 */}
        {context === 'home' && instances.length > 0 && (
          <div className={styles.home}>
            <header className={styles.mainHeader}>
              <h1>{t('app.library')}</h1>
              <Button icon={<PlusIcon />} onClick={createNewInstance}>
                {t('app.createGame')}
              </Button>
            </header>

            <div className={styles.instanceGrid}>
              {instances.map(instance => (
                <Card key={instance.id} hoverable padding="lg">
                  <div className={styles.instanceCard}>
                    {/*
                      实例图标用加载器的官方标志：原版草方块、Fabric 织纹、Quilt
                      拼布、Forge 铁砧、NeoForge 狐狸头。之前是 ⛏️ 与 🎮 两个
                      emoji——不同系统字形差别大，而且无法表达「这个是 Forge 还是
                      Fabric」。加载失败时退回草方块，不留破图。

                      状态为 error（后端 `failed`）时整枚图标去色：彩色标志会让
                      人以为实例正常，而灰化 + 右侧置灰的「安装」按钮是同一套
                      「这个实例现在不能用」的视觉语言。
                    */}
                    <div className={styles.instanceIcon}>
                      <LoaderIcon
                        {...iconForLoader(instance.loader)}
                        size={48}
                        muted={instance.status === 'error'}
                        fallback={<BlockIcon {...VANILLA_BLOCK} size={48} />}
                      />
                    </div>
                    <div className={styles.instanceInfo}>
                      {/*
                        标题行：实例名 + 「主页默认」标记 + 图钉开关。
                        图钉就在名字旁边，因为它改的正是「主页那个大按钮启动谁」；
                        放到右侧按钮组里会被读成又一个对实例本身的操作（启动、管理、
                        删除并列），而它其实是整个启动器的默认值设置。
                      */}
                      <div className={styles.instanceTitleRow}>
                        <h3>{instance.name}</h3>
                        {isDefaultLaunch(instance.id) && (
                          <span className={styles.defaultBadge}>{t('app.defaultBadge')}</span>
                        )}
                        <button
                          type="button"
                          className={`${styles.pinButton} ${isDefaultLaunch(instance.id) ? styles.pinActive : ''}`}
                          onClick={() => toggleDefaultLaunch(instance)}
                          aria-pressed={isDefaultLaunch(instance.id)}
                          /*
                           * 未就绪的实例也允许固定。
                           *
                           * 它只是「主页按钮指向谁」，不是「现在能启动」：用户可能刚
                           * 建好一个实例、还没装完就先钉住，装完就能一键进游戏。禁用
                           * 图钉会让这个顺序变成「先装完再设默认」，多一步且没有理由。
                           * 主页那边会照实说明「还没有安装游戏文件」并禁用启动按钮。
                           */
                          aria-label={isDefaultLaunch(instance.id) ? t('app.defaultClear') : t('app.defaultSetLabel')}
                          title={isDefaultLaunch(instance.id) ? t('app.defaultClear') : t('app.defaultSetLabel')}
                        >
                          <PinIcon filled={isDefaultLaunch(instance.id)} />
                        </button>
                      </div>
                      <div className={styles.instanceMeta}>
                        <span>{instance.version}</span>
                        {instance.loader !== 'vanilla' && (
                          <>
                            <span>•</span>
                            <span>{instance.loader}</span>
                          </>
                        )}
                        {instance.modCount > 0 && (
                          <>
                            <span>•</span>
                            <span>{t('app.modCount', { count: instance.modCount })}</span>
                          </>
                        )}
                      </div>
                      {instance.lastPlayed && (
                        <div className={styles.lastPlayed}>
                          {t('app.lastPlayed', { values: { time: new Date(instance.lastPlayed).toLocaleString(locale, {
                            month: 'long',
                            day: 'numeric',
                            hour: '2-digit',
                            minute: '2-digit'
                          }) } })}
                        </div>
                      )}
                      {/*
                        运行状态与最后一行日志。后端通过 `game-process` 与
                        `game-log-line` 事件推过来；这一行是「游戏到底跑起来了没有」
                        最直接的证据，比任何图标都可信。
                      */}
                      {gameStatus[instance.id]?.status === 'running' && (
                        <div className={styles.lastPlayed}>
                          {gameLogs[instance.id] ?? '游戏进程已启动'}
                        </div>
                      )}
                    </div>
                    <div className={styles.instanceActions}>
                      {/*
                        主按钮按实例的真实状态切换：
                          ready + 未运行 → 启动
                          ready + 运行中 → 关闭进程（避免重复启动直接报错）
                          未安装         → 安装（点启动必然失败，不给这个按钮）
                      */}
                      {instance.status === 'error' ? (
                        /*
                          安装失败 → 红色「修复」。
                          为什么单独区分：后端对 failed 实例重跑 `install_vanilla` 是
                          安全的——下载引擎会先校验已存在的文件（大小 + SHA-1），通过
                          的跳过、失败的重新下载，等于「校验并补全」。这与「新安装」
                          语义相同但用户预期不同：他看到的是一个坏掉的实例，按钮应该
                          说「修复」，用危险色提示这里有未解决的问题。
                        */
                        <Button
                          variant="danger"
                          icon={<WrenchIcon />}
                          onClick={() => queueStoreDownload(instance.name, 'game', instance.id, true)}
                        >
                          {t('app.repair')}
                        </Button>
                      ) : instance.status === 'installing' ? (
                        /*
                          正在安装：按钮显示进行中并禁用。
                          之前这里和其他非就绪状态共用「安装」，用户会以为还没开始，
                          点一下等于重复触发一次安装请求。
                        */
                        <Button variant="primary" loading disabled>
                          {t('app.installing')}
                        </Button>
                      ) : instance.status !== 'ready' ? (
                        <Button
                          variant="primary"
                          icon={<DownloadIcon />}
                          onClick={() => queueStoreDownload(instance.name, 'game', instance.id)}
                        >
                          {t('app.install')}
                        </Button>
                      ) : gameStatus[instance.id]?.status === 'running' ? (
                        <Button variant="primary" onClick={() => stopInstance(instance.id)}>
                          {t('app.closeGame')}
                        </Button>
                      ) : (
                        <Button
                          variant="primary"
                          icon={<PlayIcon />}
                          onClick={() => launchInstance(instance)}
                        >
                          {t('app.launch')}
                        </Button>
                      )}
                      <Button
                        variant="ghost"
                        onClick={() => {
                          setSelectedInstance(instance);
                          setContext('instance-detail');
                        }}
                      >
                        {t('app.manage')}
                      </Button>
                    </div>
                  </div>
                </Card>
              ))}
            </div>
          </div>
        )}

        {context === 'instance-detail' && selectedInstance && (
          <InstanceSettings
            instance={selectedInstance}
            global={globalPrefs}
            overrides={instanceOverrides[selectedInstance.id] ?? EMPTY_OVERRIDES}
            onChange={next => persistOverrides(selectedInstance.id, next)}
            onBack={() => setContext('home')}
            onLaunch={() => launchInstance(selectedInstance)}
            onOpenGlobal={() => setShowSettings(true)}
            javaRuntimes={javaRuntimes}
            javaSetting={instanceJavaSettings[selectedInstance.id] ?? { ...DEFAULT_INSTANCE_JAVA }}
            globalJavaPath={globalJavaPath}
            onJavaSettingChange={setting => persistInstanceJava(selectedInstance.id, setting)}
            onManageJava={() => setShowSettings(true)}
            onDelete={() => { setDeleteFilesOnDisk(false); setPendingDelete(selectedInstance); }}
            onModsChanged={() => { void refreshInstances(); }}
            onNotice={setToast}
          />
        )}

        {context === 'store' && (
          <StoreWorkspace
            category={storeCategory}
            instances={instances}
            selectedInstance={selectedInstance}
            onSelectInstance={selectStoreInstance}
            /* 导入整合包后后端会新建实例；刷新一次列表并把新实例选中。 */
            onCreated={async created => {
              await refreshInstances();
              setSelectedInstance(created);
            }}
            onNotice={setToast}
            /* 安装/导入开始后，任务列表与实例状态都会变；两处都刷新。 */
            onTasksChanged={() => { void refreshDownloadTasks(); void refreshInstances(); }}
          />
        )}

        {context === 'plugins' && (
          <PluginWorkspace
            store={pluginStore}
            onChange={persistPlugins}
            pendingRestart={pendingPluginRestart}
            onPendingRestartChange={setPendingPluginRestart}
            onNotice={setToast}
            /*
              「下载镜像助手」的开关要写真实的设置。
              只存选项值会让那个开关没有任何效果——镜像选择看的是 `downloadSource`。
            */
            downloadSource={globalPrefs.downloadSource}
            onDownloadSourceChange={source => { void persistDownloadSource(source); }}
          />
        )}

        {/* 下载任务 */}
        {context === 'downloads' && (
          <div className={styles.downloads}>
            <header className={styles.mainHeader}>
              <div>
                <h1>{t('app.downloads')}</h1>
                {/* 说明这一页展示的是什么：任务级状态，不是文件级流水。 */}
                <p>一个任务包含多个文件；下方「正在下载的文件」只显示当前传输的那几个。</p>
              </div>
            </header>

            {downloadTasks.length === 0 ? (
              <div className={styles.emptyState}>
                <div className={styles.emptyStateContent}>
                  <DownloadIcon />
                  <h2>{t('app.noDownloadTasks')}</h2>
                  <p>{t('app.noDownloadTasksHint')}</p>
                </div>
              </div>
            ) : (
              <div className={styles.taskList}>
                {downloadTasks.map(task => (
                  <Card key={task.id} padding="md">
                    <div className={styles.taskCard}>
                      <div className={styles.taskInfo}>
                        <h3>{task.name}</h3>
                        <div className={styles.taskProgress}>
                          <div className={styles.progressBar}>
                            <div 
                              className={styles.progressFill}
                              style={{ width: `${task.progress}%` }}
                            />
                          </div>
                          <span>{task.progress}%</span>
                        </div>
                            {/* 状态文字：让侧栏徽章的数字在列表里有对应物。 */}
                        <span className={styles.taskStatus}>{t(TASK_STATUS_KEYS[task.status])}</span>
                        {/*
                          实例级的最后一行日志：这是「装完了能不能跑」最直接的证据。
                          只在有日志时才渲染，避免占位空行。
                        */}
                        {task.instanceId && gameLogs[task.instanceId] && (
                          <span className={styles.taskStatus}>{gameLogs[task.instanceId]}</span>
                        )}
                        {task.instanceId && gameStatus[task.instanceId]?.status === 'running' && (
                          <Button variant="ghost" size="sm" onClick={() => stopInstance(task.instanceId!)}>
                            关闭游戏
                          </Button>
                        )}
                      </div>
                      {/*
                        只对仍在进行中的任务显示「取消」。
                        已完成/失败/已取消的任务放一个灰掉的按钮，视觉上仍占位，
                        用户会反复去点它；这些状态下本来就没有可取消的动作，直接
                        不渲染按钮比禁用更清楚。
                      */}
                      {ACTIVE_TASK_STATUSES.includes(task.status) && (
                        <Button
                          variant="ghost"
                          size="sm"
                          onClick={() => cancelTask(task.id)}
                        >
                          {t('app.cancel')}
                        </Button>
                      )}
                    </div>
                  </Card>
                ))}
              </div>
            )}

            {/*
              文件级进度。它不是任务列表的一部分：一个安装任务包含上百个文件，
              放在任务行里会把列表冲垮。单独一段、只留最近几条，回答的是
              「现在具体在下什么」这个问题。

              每行按 PCL2 的 NetState 思路给出三件事：
                - 阶段（连接中 / 下载中 / 校验中 / 已完成 / 失败）；
                - 进度条（总量未知时给不确定态，而不是假装 0 或 100）；
                - 已传/总量，让进度条有可读的数字支撑。
              只写一行「official · 100%」看不出它在干什么，也看不出还剩多少。
            */}
            {progress.length > 0 && (
              <div className={styles.taskList} aria-live="polite">
                <div className={styles.fileProgressHeader}>
                  <span className={styles.fileProgressTitle}>{t('app.fileProgressTitle')}</span>
                  <span className={styles.fileProgressCount}>
                    {t('app.fileProgressParallel', { values: { count: parallelDownloads } })}
                  </span>
                </div>
                {progress.slice(-8).reverse().map(item => {
                  const known = item.total != null && item.total > 0;
                  const pct = known ? Math.min(100, Math.round((item.received / item.total!) * 100)) : null;
                  const stage = FILE_STAGE_META[item.stage] ?? FILE_STAGE_META.downloading;
                  return (
                    <div key={item.artifactId} className={styles.fileProgressRow}>
                      <div className={styles.fileProgressTop}>
                        <span className={styles.fileProgressName} title={item.label}>
                          {item.label}
                        </span>
                        <span className={`${styles.fileStageTag} ${styles[stage.tagClass]}`}>
                          {t(stage.labelKey)}
                        </span>
                      </div>
                      <div className={styles.fileProgressBarRow}>
                        <div
                          className={`${styles.fileProgressBar} ${pct === null && item.stage !== 'completed' ? styles.fileProgressIndeterminate : ''}`}
                        >
                          <div
                            className={`${styles.fileProgressFill} ${styles[stage.fillClass]}`}
                            style={pct === null && item.stage !== 'completed' ? undefined : { width: `${pct ?? 0}%` }}
                          />
                        </div>
                        <span className={styles.fileProgressPct}>
                          {item.stage === 'completed'
                            ? '100%'
                            : pct === null ? '—' : `${pct}%`}
                        </span>
                      </div>
                      <div className={styles.fileProgressMeta}>
                        {item.source && <span>{item.source}</span>}
                        <span>
                          {known
                            ? `${formatBytes(item.received)} / ${formatBytes(item.total!)}`
                            : `${formatBytes(item.received)} / ${t('app.fileProgressUnknownTotal')}`}
                        </span>
                      </div>
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        )}

        {/*
          启动进度条：固定在内容区底部。

          参考 PCL2 的做法——启动时底部一条细进度条 + 当前阶段文字，而不是一个
          居中遮罩或转圈。理由：
            - 不遮挡界面：用户启动后通常还想看看实例列表或点别的；
            - 阶段文字说明「在等什么」：认证可能要等几秒的网络往返，只给一个转圈
              无法区分「在等网络」和「卡死了」；
            - 进度是真实的比例：后端按阶段权重推进（准备 5% → 认证 20~25% →
              拼参数 75% → 拉起 JVM 90% → 就绪 100%），不是假的匀速动画。

          主页例外：那一页有自己的进度条，紧贴在右下角启动按钮上方。两处同时出现
          会一条压在底部操作区上、一条重复同一个数字，所以这里让位。
        */}
        {launchProgress && context !== 'dashboard' && (
          <div className={styles.launchBar} role="status" aria-live="polite">
            <div className={styles.launchBarTrack}>
              <div
                className={styles.launchBarFill}
                style={{ width: `${Math.round(launchProgress.progress * 100)}%` }}
              />
            </div>
            <span className={styles.launchBarLabel}>
              {t(LAUNCH_STAGE_KEYS[launchProgress.stage] ?? 'app.launchStagePreparing')}
            </span>
            <span className={styles.launchBarPct}>
              {Math.round(launchProgress.progress * 100)}%
            </span>
          </div>
        )}
      </main>

      <AccountModal
        isOpen={showAccountModal}
        onClose={() => setShowAccountModal(false)}
        currentAccount={currentAccount}
        onLogin={handleLogin}
        recentAccounts={recentAccounts.filter(a => a.id !== currentAccount?.id)}
        onNotice={setToast}
        /* 账户被移除后需要重新读一次：后端会同时删掉凭据，选中项可能随之变化。 */
        onChanged={() => { void refreshAccounts(); }}
      />

      {showSettings && <SettingsPanel
        theme={theme}
        onThemeChange={setTheme}
        onClose={() => { setShowSettings(false); setGlobalPrefs(readPrefs()); }}
        instance={context === 'instance-detail' ? selectedInstance : undefined}
        initialTab={context === 'instance-detail' ? 'java' : undefined}
        onJavaRuntimesChange={setJavaRuntimes}
        onGlobalJavaChange={setGlobalJavaPath}
        onInstanceJavaChange={persistInstanceJava}
        /* 背景即时生效：面板里每改一项都立刻反映到这一层之后。 */
        onBackgroundChange={setBackground}
        onNotice={setToast}
        dark={resolvedTheme === 'dark'}
        /*
          把界面偏好写入后端。基于 `naturalSettings`（上次从后端读到的完整设置）
          翻译，而不是从默认值重建：否则界面不展示的字段每保存一次就被重置一次。
          保存成功后同步本地的只读快照，让实例管理页的「跟随全局」显示新值。
        */
        onSaveSettings={async nextPrefs => {
          const base = naturalSettings ?? (await getSettings());
          const next = prefsToSettings(nextPrefs, base);
          await saveSettingsNative(next);
          setNaturalSettings(next);
          setGlobalPrefs(prefFromSettings(next));
          setGlobalJavaPath(next.javaPath ?? '');
        }}
      />}

      <CreateInstanceDialog
        isOpen={showCreateDialog}
        onClose={() => setShowCreateDialog(false)}
        onCreated={handleCreateInstance}
      />

      {/*
        删除确认。用一个明确的对话框而不是 window.confirm：除了「要删吗」，
        还必须让用户回答「文件删不删」——这是两个独立决定，塞进一次确认里
        很容易被连点跳过，代价是磁盘上的世界没了。
      */}
      <Modal
        isOpen={pendingDelete !== null}
        onClose={() => setPendingDelete(null)}
        title={t('instance.deleteTitle', { values: { name: pendingDelete?.name ?? '' } })}
        size="sm"
        footer={
          <>
            <Button variant="ghost" onClick={() => setPendingDelete(null)}>{t('common.cancel')}</Button>
            <Button
              variant="danger"
              onClick={() => { if (pendingDelete) void handleDeleteInstance(pendingDelete, deleteFilesOnDisk); }}
            >
              {t('instance.deleteConfirm')}
            </Button>
          </>
        }
      >
        <p className={styles.dialogBody}>{t('instance.deleteBody')}</p>
        <label className={styles.checkRow}>
          <input
            type="checkbox"
            checked={deleteFilesOnDisk}
            onChange={event => setDeleteFilesOnDisk(event.target.checked)}
          />
          <span>
            <strong>{t('instance.deleteFiles')}</strong>
            <small>{t('instance.deleteFilesHint')}</small>
          </span>
        </label>
      </Modal>

      {/*
        提示条。后端错误、安装结果、离线确认的后续都在这里落地。
        放在最外层而不是各页面里：同一个操作可能从一个页面发起、在另一个页面
        看到结果，绑在页面上的提示会在切换页面时丢失。
      */}
      {toast && (
        <div className={`${styles.toast} ${context === 'dashboard' ? styles.toastAboveDock : ''}`} role="status">
          <span>{toast}</span>
          <button type="button" aria-label={t('common.close')} onClick={() => setToast('')}>×</button>
        </div>
      )}
    </div>
  );
}

export default App;
