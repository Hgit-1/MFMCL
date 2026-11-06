// 内容下载工作区（接后端版）。
//
// 与原型最大的区别：这里不再直接 fetch Modrinth / CurseForge / Mojang。
//
// 原因不是「为了接后端而接后端」，而是三个实际差别：
//   1. 安装必须走 Rust 的下载引擎，它做哈希校验、断点续传、aria2 回退，并把进度
//      写进持久化任务表。浏览器里 fetch 下来一个文件是写不进游戏目录的。
//   2. 模组搜索需要按实例的「游戏版本 + 加载器」收窄，这一步后端已经在做
//      （`search_mods` 的 facets）；前端自己拼参数迟早会和启动路径不一致。
//   3. CSP 里 `connect-src` 只放行了少数域名，前端直连会有一条永远失败的路径。
//
// 因此本页只负责：
//   - 原版版本列表：来自 `list_versions`（后端读 Mojang 清单）。
//   - 模组搜索：`search_mods` → 选版本 → `install_mod_version`。
//   - 整合包：选本地 .zip/.mrpack → `preview_modpack` 显示内容 → `import_modpack`。
//
// 资源包与地图分类后端没有对应命令（既不能搜索也不能安装），所以这两个分类在
// 界面上明确标注为暂不可用，而不是给一个点了没反应的按钮。

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  Download,
  FolderDown,
  Package,
  RefreshCw,
  Search,
} from 'lucide-react';
import { Button } from './Button';
import { BlockIcon } from './BlockIcon';
import { VanillaIcon } from './VanillaIcon';
import { CurseForgePanel } from './CurseForgePanel';
import { CurseForgeKeyCard } from './CurseForgeKeyCard';
import { ALL_VERSIONS, VersionPicker } from './VersionPicker';
import { InstallPlan } from './InstallPlan';
import { StoreLogo } from './StoreLogo';
import { blockForVersion, VANILLA_BLOCK } from '../utils/store/blocks';
import { useI18n } from '../utils/i18n';
import type { MessageKey } from '../utils/i18n/messages/zh-CN';
import { fitInstanceLabel, kindOfVersion, VERSION_KINDS, VERSION_KIND_KEYS } from '../utils/store/format';
import type { VersionKind } from '../utils/store/types';
import type { GameInstance } from '../types';
import {
  errorText,
  formatBytes,
  downloadAndImportModpack,
  importModpack,
  curseforgeDownload,
  curseforgeFiles,
  curseforgeInstall,
  curseforgeKeyStatus,
  curseforgeSearch,
  downloadModVersion,
  installModVersion,
  isDesktopShell,
  listModVersions,
  searchModsUnscoped,
  listProjectVersions,
  downloadAndImportModrinthPack,
  listVersions,
  openDownloadFolder,
  pickDirectory,
  pickFile,
  previewModpack,
  searchMods,
  type CatalogMod,
  type CatalogVersion,
  type CurseForgeMod,
  type CurseForgeVersion,
  type ModQuery,
  type PackPreview,
  type VersionListItem,
} from '../utils/native';
import { SETTINGS_STORAGE_KEY, readStoredPrefs } from '../utils/launcherPrefs';
import styles from './StoreWorkspace.module.css';

export const STORE_CATEGORIES = ['vanilla', 'mod', 'modpack', 'resourcepack', 'world'] as const;
export type StoreCategory = (typeof STORE_CATEGORIES)[number];

/**
 * 有 Modrinth 项目类型的分类。
 *
 * Modrinth 没有地图（`world`）这种类型，所以它只能走 CurseForge。资源包有
 * （`project_type: resourcepack`），因此两个来源都能提供——之前把资源包也锁死
 * 在 CurseForge 上，用户必须配 key 才能下资源包，这是没必要的门槛。
 */
const MODRINTH_CATEGORIES: StoreCategory[] = ['mod', 'resourcepack'];

/** 分类 → Modrinth 的 project_type。 */
function projectTypeOf(category: StoreCategory): 'mod' | 'resourcepack' | 'modpack' | undefined {
  switch (category) {
    case 'mod':
      return 'mod';
    case 'resourcepack':
      return 'resourcepack';
    case 'modpack':
      return 'modpack';
    default:
      return undefined;
  }
}

/**
 * 每页条数。
 *
 * 原版版本列表与模组搜索共用：两处都是「一屏放得下的条目数」，用同一个值让
 * 翻页控件在两处的位置与语义完全一致。后端 `MAX_RESULTS` 也是 20，三者对齐。
 */
/**
 * 去掉压缩包扩展名。
 *
 * 与 PCL2 的 `.BeforeLastOfAny({ ".zip", ".jar", ".mrpack", ".litemod" })` 同一个
 * 做法：版本列表里「fabric-api-0.115.1+1.21.1.jar」显示成
 * 「fabric-api-0.115.1+1.21.1」。扩展名对玩家没有信息量，而它占的位置常常正好
 * 把版本号挤进省略号。
 *
 * 只剥这四种已知后缀，不用通用的 `lastIndexOf('.')`：版本号自身就带点
 * （`0.165.1+26.4`），按最后一个点切会把 `26.4` 当扩展名切掉。
 */
function stripArchiveExtension(fileName: string): string {
  const lower = fileName.toLowerCase();
  for (const ext of ['.mrpack', '.litemod', '.jar', '.zip']) {
    if (lower.endsWith(ext)) return fileName.slice(0, -ext.length);
  }
  return fileName;
}

/**
 * 组装版本行的元信息行。
 *
 * 顺序即优先级，参考 PCL2 的 `ToListItem`：加载器 → 游戏版本 → 大小 →
 * 前置数 → 下载量 → 更新时间。
 *
 * - **加载器排第一**：装错加载器是最常见的失败原因（Forge 的 jar 放进 Fabric
 *   实例会直接崩），它比其他任何信息都更该先被看到。
 * - **游戏版本只在「文件与文件名不一致」时显示**：`0.165.1+26.4` 里的 `26.4`
 *   已经说明了 MC 版本，再写一遍是噪音。这条判断也来自 PCL2。
 * - **下载量用「万」而不是完整数字**：`29462万` 比 `294,620,270` 更快被读出
 *   量级，而这个数字在版本选择里只用于「哪个更常用」的排序感。
 */
function buildVersionMeta(
  version: CatalogVersion,
  locale: string,
  t: (key: MessageKey, options?: { values?: Record<string, string | number>; count?: number }) => string,
  /**
   * 是否显示加载器。
   *
   * 由调用方按「整页是否有多个加载器」决定——单个加载器的列表里它是纯噪音，
   * 而多加载器时它是防装错的关键信息。
   */
  showLoader: boolean,
): string {
  const parts: string[] = [];

  /*
   * 加载器：只在「这一页里有多个加载器」时才显示。
   *
   * 单个模组的版本列表里，加载器几乎总是同一个值（`fabric-api` 的每一版都是
   * `fabric`）。把它写 36 遍不提供任何区分度，反而挤掉了后面真正有用的信息
   * （大小、下载量、更新时间）——实测一屏 16 行，每行都以同一个 `fabric` 开头。
   *
   * 反过来，一个同时发布 Forge 与 Fabric 版本的模组，这里的差异就是用户最需要
   * 看到的东西：装错加载器会直接崩。所以判据是「整页是否只有一种加载器」，
   * 由 `showLoader` 传入，而不是无条件显示。
   *
   * PCL2 的 `ToListItem` 同样无条件输出加载器，但它的列表通常同时混有多个来源
   * （CurseForge 的文件可能跨加载器），前提不同。
   */
  if (showLoader && version.loaders && version.loaders.length > 0) {
    // Modrinth 对资源包返回 `["minecraft"]`，那不是加载器，显示出来只会让人困惑。
    const real = version.loaders.filter(loader => loader !== 'minecraft');
    if (real.length > 0) parts.push(real.join('、'));
  }

  const gameVersions = version.gameVersions ?? [];
  if (gameVersions.length > 0) {
    const name = stripArchiveExtension(version.fileName);
    // 文件名里已经包含该版本号时不再重复（PCL2 的 `Title <> FileName` 判断）。
    if (!gameVersions.every(v => name.includes(v))) {
      parts.push(gameVersions.join('、'));
    }
  }

  if (version.size > 0) parts.push(formatBytes(version.size));
  if ((version.dependencyCount ?? 0) > 0) {
    parts.push(t('store.depsCount', { count: version.dependencyCount!, values: { count: version.dependencyCount! } }));
  }
  if ((version.downloads ?? 0) > 0) {
    const n = version.downloads!;
    const shown = n >= 100_000 ? `${(n / 10_000).toLocaleString(locale, { maximumFractionDigits: 0 })}万` : n.toLocaleString(locale);
    parts.push(t('store.downloadsTimes', { values: { value: shown } }));
  }
  if (version.dateCreated) {
    parts.push(t('store.updatedOn', {
      values: { date: new Date(version.dateCreated).toLocaleDateString(locale) },
    }));
  }
  return parts.join(' · ');
}

const PAGE_SIZE = 20;

/**
 * 一个整合包最多展开多少个版本。
 *
 * 30 个足够覆盖「最近几个大版本」，再多就不是在挑版本而是在读变更史了。
 */
const MR_PACK_VERSION_LIMIT = 30;

/**
 * 模组/资源包版本列表的渲染上限。
 *
 * 有实例时后端已按版本与加载器收窄，通常只有几十条；直连模式不筛，实测可到 100。
 * 40 条足够覆盖「最近几个版本」，再多也不会有人逐条读完。
 */
const VERSION_PREVIEW_LIMIT = 40;

/**
 * 搜索面板的「类型」选项。
 *
 * `slug` 直接作为 Modrinth 的 categories facet，`labelKey` 是界面文案键——
 * 两者分开是因为分类名要跟着语言走，而请求参数不能。只列出 Modrinth 确实
 * 存在的分类，不照搬 PCL2 那份以 CurseForge 分类 ID 为主的清单。
 */
const MOD_CATEGORIES: { slug: string; labelKey: MessageKey }[] = [
  { slug: 'optimization', labelKey: 'store.category.optimization' },
  { slug: 'technology', labelKey: 'store.category.technology' },
  { slug: 'magic', labelKey: 'store.category.magic' },
  { slug: 'adventure', labelKey: 'store.category.adventure' },
  { slug: 'decoration', labelKey: 'store.category.decoration' },
  { slug: 'mobs', labelKey: 'store.category.mobs' },
  { slug: 'utility', labelKey: 'store.category.utility' },
  { slug: 'equipment', labelKey: 'store.category.equipment' },
  { slug: 'food', labelKey: 'store.category.food' },
  { slug: 'storage', labelKey: 'store.category.storage' },
  { slug: 'transportation', labelKey: 'store.category.transportation' },
  { slug: 'worldgen', labelKey: 'store.category.worldgen' },
  { slug: 'library', labelKey: 'store.category.library' },
  { slug: 'game-mechanics', labelKey: 'store.category.gameMechanics' },
  { slug: 'social', labelKey: 'store.category.social' },
];

interface StoreWorkspaceProps {
  category: StoreCategory;
  instances: GameInstance[];
  selectedInstance?: GameInstance;
  onSelectInstance: (instanceId: string) => void;
  onCreated: (instance: GameInstance) => void;
  onNotice: (message: string) => void;
  /** 安装/导入开始后刷新任务列表与实例状态。 */
  onTasksChanged: () => void;
}

/** 原版列表里的一个版本行。类型与后端 `VersionListItem` 一致。 */
interface VanillaRow {
  id: string;
  releaseTime: string;
  type: VersionListItem['type'];
}

export function StoreWorkspace({
  category,
  instances,
  selectedInstance,
  onSelectInstance,
  onCreated,
  onNotice,
  onTasksChanged,
}: StoreWorkspaceProps) {
  const { t, language } = useI18n();
  /** 日期本地化用。两处列表都要显示发布日期，统一在这里算一次。 */
  const locale = language === 'zh-CN' ? 'zh-CN' : language === 'ja' ? 'ja-JP' : 'en-US';

  // 目标实例：安装模组必须有实例，因为后端要按它的版本与加载器筛选。
  const [targetInstanceId, setTargetInstanceId] = useState(selectedInstance?.id ?? '');
  const target = instances.find(instance => instance.id === targetInstanceId);
  /**
   * 直连下载模式：没有目标实例。
   *
   * 组件层的这一份用于渲染（隐藏「安装」按钮、显示直连提示）；搜索函数内部还会
   * 各自算一次，因为那个闭包需要与请求参数保持一致。两处判据同一个表达式，
   * 不会分叉。
   */
  const directMode = !targetInstanceId;

  /**
   * 「版本」筛选的全量清单。
   *
   * 单独一份状态而不是复用原版列表的 `versions`：那个只在「原版游戏」分类下才
   * 加载，而模组搜索在别的分类也要用版本清单。分开之后进入模组页时按需拉一次，
   * 之后整个会话共用；`ready` 之后不再请求，因为 Mojang 的清单一天才更新几次。
   */
  const [versionList, setVersionList] = useState<VersionListItem[]>([]);
  const [versionListState, setVersionListState] = useState<'idle' | 'loading' | 'ready' | 'error'>('idle');

  const loadVersionList = useCallback(async () => {
    setVersionListState('loading');
    try {
      setVersionList(await listVersions());
      setVersionListState('ready');
    } catch {
      setVersionListState('error');
    }
  }, []);

  /**
   * 「版本」筛选器第一次展开时才加载。
   *
   * 不在进入模组页时立刻拉：多数用户直接用「跟随实例」，从不打开版本列表，
   * 那个默认值不需要这份清单就能工作。失败后允许重试（`error` 也算可再次请求），
   * 否则一次网络抖动会让这个筛选器整场会话不可用。
   */
  const requestVersionList = useCallback(() => {
    if (versionListState === 'loading' || versionListState === 'ready') return;
    void loadVersionList();
  }, [versionListState, loadVersionList]);

  /** 「跟随实例」那一项的文字。没有目标实例时用破折号，与之前的下拉一致。 */
  const followVersionLabel = t('store.versionFollowInstance', { values: { version: target?.version ?? '—' } });

  /**
   * 「直接下载」的保存目录。
   *
   * 放在侧栏而不是每个版本的按钮上：用户选一次目录，之后所有「直接下载」都用它。
   * 之前每次点下载都要弹一次文件夹对话框，选同一个目录十次就要点十次确认。
   *
   * 与全局设置里的 `downloadDirectory` 共用同一个存储键，所以在设置面板里改过
   * 的目录会在这里直接显示，不需要两处各配一遍。
   */
  const [downloadDir, setDownloadDir] = useState(() => readStoredPrefs().downloadDirectory);

  /**
   * 最近一次「直接下载」成功落盘的文件名。
   *
   * 只显示一条 transient toast 不够：用户点完下载往往过一会儿才想起来「刚才那个
   * jar 放哪了」，而 toast 早就消失了。这里记住最近一次的结果，在侧栏的下载目录
   * 下面给一个「刚刚保存：xxx」的可点击行，点它就打开目录。
   *
   * 只记名字不记完整路径：完整路径放在 title 里，避免侧栏被长路径撑开。
   */
  const [lastSaved, setLastSaved] = useState<{ name: string; path: string } | null>(null);

  /**
   * 「直接下载」区块是否展开。
   *
   * 初始值取决于是否已配过目录：配过就展开（那是有效配置，藏起来会让人以为没配
   * 过），没配过就收起（那是次要路径，常驻会占掉侧栏近 200px）。
   */
  const [directOpen, setDirectOpen] = useState(() => readStoredPrefs().downloadDirectory !== '');

  /*
   * 每次进入模组分类时重读一次目录。
   *
   * 组件在分类之间不会卸载，只在挂载时读一次的话，用户去设置面板改了下载目录、
   * 再切回内容商店，这里仍然显示旧值。重读的代价只是一次 localStorage 读取。
   */
  useEffect(() => {
    if (!MODRINTH_CATEGORIES.includes(category)) return;
    setDownloadDir(readStoredPrefs().downloadDirectory);
  }, [category]);

  /**
   * 取路径的最后一段作为摘要。
   *
   * 侧栏只有 183px 宽，整条路径必然被省略成 `/home/user/...`，那样连「哪个目录」
   * 都看不出来。显示末段（例如 `mods`）才有信息量；完整路径仍在 `title` 里。
   */
  const folderName = (path: string) => {
    const trimmed = path.replace(/[\\/]+$/, '');
    const last = trimmed.split(/[\\/]/).pop();
    return last && last !== trimmed ? last : trimmed;
  };

  const chooseDownloadDir = async () => {
    if (!isDesktopShell()) {
      onNotice(t('store.directDownloadDesktopOnly'));
      return;
    }
    const picked = await pickDirectory(t('store.directDownloadPickTitle'));
    if (!picked) return;
    setDownloadDir(picked);
    // 选完立刻展开：否则用户点了一个整宽按钮，界面没有任何变化。
    setDirectOpen(true);
    // 写回同一份偏好，让设置面板与下次打开都看到同一个目录。
    try {
      const current = readStoredPrefs();
      localStorage.setItem(
        SETTINGS_STORAGE_KEY,
        JSON.stringify({ ...current, downloadDirectory: picked }),
      );
    } catch {
      // localStorage 不可用（隐私模式等）时只在本次会话生效，不影响下载本身。
    }
  };

  /** 在文件管理器里打开下载目录；目录已被删除时如实报告而不是静默失败。 */
  const openDownloadDir = async () => {
    if (!downloadDir.trim()) return;
    try {
      await openDownloadFolder(downloadDir.trim());
    } catch (error) {
      onNotice(errorText(error, t('store.directDownloadOpenFailed')));
    }
  };

  // -------------------------------------------------------------------------
  // 原版版本列表
  // -------------------------------------------------------------------------
  const [versions, setVersions] = useState<VersionListItem[]>([]);
  const [versionsState, setVersionsState] = useState<'idle' | 'loading' | 'ready' | 'error'>('idle');
  const [versionsError, setVersionsError] = useState('');
  const [query, setQuery] = useState('');
  const [submittedQuery, setSubmittedQuery] = useState('');
  const [kinds, setKinds] = useState<VersionKind[]>(['release']);
  const [page, setPage] = useState(0);

  const loadVersions = useCallback(async () => {
    setVersionsState('loading');
    setVersionsError('');
    try {
      const list = await listVersions();
      setVersions(list);
      setVersionsState('ready');
    } catch (error) {
      setVersionsError(errorText(error, t('store.manifestFailed')));
      setVersionsState('error');
    }
  }, [t]);

  useEffect(() => {
    if (category !== 'vanilla') return;
    if (versionsState !== 'idle') return;
    void loadVersions();
  }, [category, versionsState, loadVersions]);

  /**
   * 切回「原版游戏」时重新拉一次清单。
   *
   * 首次加载由上面的 effect 负责；但用户在切走再切回时，`versionsState` 已经是
   * `ready`，effect 不会再跑。如果期间清单拉取失败或被别的操作清空，列表就会
   * 停在空的状态而没有任何自动恢复的机会。这里在每次进入原版分类时补一次
   * 「加载失败则重试」。
   */
  useEffect(() => {
    if (category !== 'vanilla') return;
    if (versionsState === 'error') void loadVersions();
  }, [category, versionsState, loadVersions]);

  // -------------------------------------------------------------------------
  // 模组搜索
  // -------------------------------------------------------------------------
  const [mods, setMods] = useState<CatalogMod[]>([]);
  /**
   * 模组结果的页码（从 0 开始）。
   *
   * 与「原版版本列表」的 `page` 分开：两者属于不同分类，共用一个页码会让切分类时
   * 停在上一个列表的页码上，打开就是空白。
   *
   * 翻页是**服务端**的：Modrinth 的 `/search` 只返回一页，`offset` 决定从第几条
   * 开始。早期这里只发一次请求、把返回的 20 条直接当成全部结果，界面上既没有翻页
   * 也没有「还有更多」的提示——用户想找第 21 个模组时无从下手。
   */
  const [modPage, setModPage] = useState(0);
  /**
   * 每个来源是否可能还有下一页。
   *
   * 用「本页是否满额」判断而不是读总数：Modrinth 的 `total_hits` 与 CurseForge 的
   * `pagination.totalCount` 语义不同（前者是去重后的项目数，后者含分页元数据），
   * 混用会出现「最后一页永远有下一页」这种循环。满额即可能还有下一页，是两站都
   * 成立的保守判断。
   */
  const [modHasMore, setModHasMore] = useState(false);
  const [modsState, setModsState] = useState<'idle' | 'loading' | 'ready' | 'error'>('idle');
  const [modsError, setModsError] = useState('');
  /** 当前展开版本列表的模组。同一时刻只展开一个，避免整 page 变成版本墙。 */
  const [expandedMod, setExpandedMod] = useState<string | null>(null);
  const [modVersions, setModVersions] = useState<CatalogVersion[]>([]);
  const [modVersionsState, setModVersionsState] = useState<'idle' | 'loading' | 'ready' | 'error'>('idle');
  /** 版本列表的失败原因。与 `modsError`（搜索结果横幅）分开，两者互不覆盖。 */
  const [modVersionsError, setModVersionsError] = useState('');
  const [installing, setInstalling] = useState('');

  /*
   * 搜索条件。三组单选/多选对应 PCL2 搜索面板的「版本」「类型」「排序」：
   * 版本与加载器默认跟随目标实例，可以在这里单独覆盖（例如想提前看别的版本）。
   * 分类与排序改变后立刻重搜——这些是一次点击就能看到结果的低代价条件；
   * 名称仍然需要按回车或点搜索，避免边输入边请求。
   */
  const [modCategory, setModCategory] = useState('');
  const [modSort, setModSort] = useState<'relevance' | 'downloads' | 'newest' | 'updated' | 'follows'>('downloads');
  /** 版本筛选；空表示跟随目标实例。 */
  const [modVersion, setModVersion] = useState('');
  /** 加载器筛选；`any` 表示不限制，空表示跟随目标实例。 */
  const [modLoader, setModLoader] = useState('');

  /** 正在为哪个版本配置安装。null 表示面板关闭。 */
  const [installTarget, setInstallTarget] = useState<{ version: string } | null>(null);

  /**
   * 模组来源（多选）。
   *
   * 之前做成单选切换，理由是「两个来源的枚举与错误语义不同，混排后无法解释计数」。
   * 但实际使用中这个代价换来的好处更小：用户想的是「搜到这个模组」，而不是
   * 「先在哪个站搜」。折中方案是**合并结果但保留来源标记**——每条结果上标出它
   * 来自哪个站，计数写明各站条数，这样既能一次搜全，也不会出现无法解释的数字。
   *
   * 默认只勾 Modrinth，等 key 状态查回来后再补上 CurseForge：默认勾上会让首次
   * 打开（还没确认 key 是否可用时）出现半个错误状态。构建里带了内置 key、
   * 或用户自己配过 key 时，下面那个 effect 会把 CurseForge 一并勾上——那种情况下
   * 两个来源都真的能用，没有理由让用户先手动勾一次。
   */
  /*
   * 默认勾上两个来源。
   *
   * 之前的做法是「先只勾 Modrinth，等 key 状态查回来再补上 CurseForge」——那是
   * 因为当时没有 key 就搜不了。现在无 key 会走 MCIM 镜像，CurseForge 从第一次
   * 打开就是可用的，没有理由让用户先看到一个缺一半的商店再等它自己补齐。
   */
  const [sources, setSources] = useState<Array<'modrinth' | 'curseforge'>>([
    'modrinth',
    'curseforge',
  ]);

  /**
   * CurseForge 的凭据状态。
   *
   * `configured: false` 不再是阻断条件——它只表示「走镜像」。这个状态现在的用途
   * 是决定要不要给出「填入自己的 key」的建议入口，而不是决定能不能搜。
   */
  const [cfKeyReady, setCfKeyReady] = useState<boolean | null>(null);

  const toggleSource = (source: 'modrinth' | 'curseforge') => {
    setSources(current =>
      current.includes(source)
        ? current.filter(item => item !== source)
        : [...current, source]);
  };

  /*
   * 进入模组分类时查一次 key 状态。
   *
   * 不查的话 `cfKeyReady` 永远是 null，勾上 CurseForge 既不显示设置入口也不会
   * 真的去搜——界面看起来有选项、实际什么也不会发生。放在分类切换时查而不是只在
   * 勾选时查，是为了让用户勾上之前就能看到「需要配置」。
   */
  useEffect(() => {
    if (!MODRINTH_CATEGORIES.includes(category)) return;
    let alive = true;
    void curseforgeKeyStatus()
      .then(status => { if (alive) setCfKeyReady(status.configured); })
      .catch(() => { if (alive) setCfKeyReady(false); });
    return () => { alive = false; };
  }, [category]);

  /** CurseForge 的搜索结果与错误，与 Modrinth 分开保存以便各自报错。 */
  const [cfMods, setCfMods] = useState<CurseForgeMod[]>([]);
  const [cfSearchError, setCfSearchError] = useState('');
  /** 当前展开版本列表的 CurseForge 项目。与 Modrinth 的展开状态互不影响。 */
  const [expandedCfMod, setExpandedCfMod] = useState<string | null>(null);
  const [cfVersions, setCfVersions] = useState<CurseForgeVersion[]>([]);
  const [cfVersionsState, setCfVersionsState] = useState<'idle' | 'loading' | 'ready' | 'error'>('idle');
  /** 展开失败时的原因（缺少 key、没有目标实例、网络等）。 */
  const [cfVersionsError, setCfVersionsError] = useState('');

  // -------------------------------------------------------------------------
  // 整合包导入
  // -------------------------------------------------------------------------
  const [packPath, setPackPath] = useState('');
  const [packPreview, setPackPreview] = useState<PackPreview | null>(null);
  const [packState, setPackState] = useState<'idle' | 'loading' | 'ready' | 'error'>('idle');
  const [packError, setPackError] = useState('');
  const [importing, setImporting] = useState(false);

  /*
   * 站内整合包搜索。
   *
   * 与本地导入是两条独立路径：这里查 CurseForge 的 `classId=4471`（整合包），
   * 选中文件后由后端「下载 + 导入」一步完成，前端不接触临时文件。
   *
   * 不复用模组页那套搜索状态：整合包不按加载器/版本筛选实例，而且结果区的
   * 「包内容」属于本地导入，两批数据混在一个 state 里会互相覆盖。
   */
  const [packQuery, setPackQuery] = useState('');
  const [packHits, setPackHits] = useState<CurseForgeMod[]>([]);
  /*
   * Modrinth 的整合包结果，与 CurseForge 分开存。
   *
   * 两个来源的条目形状不同（`CurseForgeMod` 有 projectId/downloads，
   * `CatalogMod` 有 projectId/follows），合并成一个数组需要先定义一个两边都能
   * 表达的中间类型，再在渲染时按来源分支——那等于把两份类型定义拼成一个更差的
   * 第三个。分开存 + 分栏渲染，代价只是多一个 state。
   */
  const [mrPackHits, setMrPackHits] = useState<CatalogMod[]>([]);
  /** 当前展开文件列表的 Modrinth 整合包项目 id。 */
  const [expandedMrPack, setExpandedMrPack] = useState<string | null>(null);
  const [mrPackVersions, setMrPackVersions] = useState<CatalogVersion[]>([]);
  const [mrPackVersionsState, setMrPackVersionsState] = useState<'idle' | 'loading' | 'ready' | 'error'>('idle');
  const [packSearchState, setPackSearchState] = useState<'idle' | 'loading' | 'ready' | 'error'>('idle');
  const [packSearchError, setPackSearchError] = useState('');
  /*
   * 整合包的页码与「是否还有下一页」。
   *
   * 与模组页的 `modPage` 分开：两页的筛选条件完全不同（整合包不按实例的版本与
   * 加载器收窄），共用一个页码会在切换分类时把用户丢到某个无关列表的中段。
   */
  const [packPage, setPackPage] = useState(0);
  const [packHasMore, setPackHasMore] = useState(false);
  /** 记住上一次实际执行的查询，翻页时复用，避免把输入框的实时内容带进去。 */
  const lastPackQuery = useRef('');
  /** 当前展开文件列表的整合包项目。 */
  const [expandedPack, setExpandedPack] = useState<string | null>(null);
  const [packVersions, setPackVersions] = useState<CurseForgeVersion[]>([]);
  const [packVersionsState, setPackVersionsState] = useState<'idle' | 'loading' | 'ready' | 'error'>('idle');
  /** 正在下载/导入的整合包项目 id，用于禁用按钮。 */
  const [installingPack, setInstallingPack] = useState('');

  /*
   * 把目标实例与外壳选中的实例对齐。
   *
   * `selectedInstance` 是异步加载的：首屏渲染时它还是 `undefined`，上面的
   * `useState` 初始化只能拿到空串，所以首个真实实例到达时这里补上一次。
   *
   * 反方向要保留用户在本页的选择：用户在下拉里挑了别的实例后，外层会同步更新
   * `selectedInstance`，这时不能再把本地值覆盖回去。所以只在「本地为空」或
   * 「本地指向的实例已不存在」时才跟随外层。
   *
   * 依赖用 `instanceIds` 这个字符串而不是 `instances` 数组：父组件只在刷新时
   * 换新数组，但字符串在内容不变时相等，可以少一次无意义的 effect 执行。
   */
  const instanceIds = instances.map(item => item.id).join(',');
  useEffect(() => {
    const external = selectedInstance?.id ?? '';
    if (!external) return;
    setTargetInstanceId(current => {
      if (!current) return external;
      const known = instanceIds.split(',').includes(current);
      return known ? current : external;
    });
  }, [instanceIds, selectedInstance?.id]);

  /*
   * 切分类 = 换上下文，不只是换一个筛选条件。
   *
   * 需要重置三样东西，否则会带着上一个分类的语义去查新分类：
   *
   *  1. **页码**：原版的页码和模组搜索的页码语义完全不同；模组第 3 页的 offset
   *     对资源包毫无意义，继续用会让新分类从第 41 条开始取，看起来像「没有结果」。
   *  2. **加载器筛选**：`loader` facet 只对模组/整合包成立。资源包的 `loaders`
   *     是 `["minecraft"]`，带着 `categories:fabric` 去查会得到空列表——而界面
   *     不显示「你被加载器过滤了」，用户只会以为资源包不存在。
   *  3. **分类 facet**：Modrinth 的 category slug 是按项目类型分的（模组有
   *     `optimization`、资源包有 `16x`），跨类型使用同样查不到东西。
   *
   * 用「上一次的 category」比较，跳过首次挂载：挂载时这些值本来都是空的。
   */
  const prevCategory = useRef(category);
  useEffect(() => {
    setPage(0);
    setModPage(0);
    setExpandedMod(null);
    setExpandedCfMod(null);
    setModVersions([]);
    setCfVersions([]);
    if (prevCategory.current === category) return;
    prevCategory.current = category;
    setModLoader('');
    setModCategory('');
  }, [category]);

  /**
   * 搜索模组，按勾选的来源并行请求。
   *
   * 为什么并行并各自记录错误：两个站互相独立，一个挂了不该让另一个的结果也消失。
   * 所以用 `Promise.allSettled`，成功的照常显示，失败的只把该站的错误挂在那一栏，
   * 并在结果区顶部提示——用户仍能看到另一半结果。
   */
  /**
   * 执行一次模组搜索。
   *
   * `queryOverride` 用于「刚按下搜索」这一帧：`setSubmittedQuery` 是异步的，
   * 直接调用会读到上一次的闭包值，导致第一次搜索用旧关键词、空关键词时甚至
   * 搜不出东西。显式传入当前输入即可避开这个时序问题。
   */
  /**
   * 搜索请求的序号，用来丢弃「过期的响应」。
   *
   * ## 为什么必须有它
   *
   * 切分类时会有**两个** effect 同时触发搜索：
   *   - `[category, targetInstanceId]` —— 进入分类要搜一次；
   *   - `[category, modFilters..., sourcesKey]` —— 筛选条件（含分类）变化要搜一次。
   *
   * 两者都会调用 `runModSearch`，于是同时发出两个请求。它们返回顺序不确定，
   * 谁后到谁写入 state——如果旧分类的请求后到，就会出现「侧栏与标题都是
   * Resource packs，结果列表却是 Fabric API / Sodium」。
   *
   * ## 做法
   *
   * 每次发起时自增一个序号并记下本次的号；响应回来时如果号已经不是最新的，
   * 直接丢弃、不写任何 state。这比「加锁串行化」简单，也比「取消请求」可靠：
   * 后者需要 AbortController 贯穿到 `searchMods` 与 CurseForge 两条调用链，
   * 而 `invoke` 的取消语义在各平台并不一致。
   */
  const searchSeq = useRef(0);

  const runModSearch = useCallback(async (queryOverride?: string, pageOverride?: number) => {
    /*
     * 页码从参数取，缺省用当前页。
     *
     * 显式传参而不是把 `modPage` 放进依赖：翻页处理器要能「先算出目标页再请求」，
     * 而用 state 需要等一次重渲染才能拿到新值，那样会先请求旧页再请求新页。
     */
    const page = pageOverride ?? modPage;
    /*
     * 没有目标实例 = 直连下载模式，**仍然搜索**。
     *
     * 之前这里直接设成一个错误并返回，于是「不装进任何实例、只把文件存到磁盘」
     * 这种用法在界面上根本走不通：用户能把下拉清空，然后结果区报「请先选择一个
     * 实例」。而搜索本身并不需要实例——需要实例的只是「装到哪」那一步。
     *
     * 直连模式下调用的后端路径不加版本/加载器 facet（见 search_mods_unscoped），
     * 也就是「不限版本与加载器」，由用户在版本列表里自己挑。
     */
    // 复用组件层那份判据（同一个表达式，不存在分叉）。
    if (sources.length === 0) {
      setMods([]);
      setCfMods([]);
      setModsState('error');
      setModsError(t('store.noSourceSelected'));
      return;
    }
    /*
     * 领一个序号。从这里开始的所有写入都要先确认自己仍是最新的一次搜索。
     */
    const seq = ++searchSeq.current;
    /** 本次结果是否还应当写入界面。 */
    const fresh = () => searchSeq.current === seq;

    setModsState('loading');
    setModsError('');
    setCfMods([]);
    setCfSearchError('');

    const trimmed = (queryOverride ?? submittedQuery).trim();
    // 关键词为空时按下载量浏览；有关键词时尊重用户选的排序（默认仍是下载量）。
    const sort = trimmed ? modSort : (modSort === 'relevance' ? 'downloads' : modSort);
    // 当前分类决定请求哪种项目：资源包要 `project_type: resourcepack`，
    // 否则搜出来的是模组。
    const projectType = projectTypeOf(category) ?? 'mod';
    const request: ModQuery = {
      query: trimmed,
      projectType,
      /*
       * 「全部版本」用独立字段表达，而不是把 `gameVersion` 传空。
       *
       * 传空的含义是「跟随实例」，后端会自动补上实例的版本——那正是用户选
       * 「全部版本」时想要避开的行为。
       */
      allVersions: modVersion === ALL_VERSIONS,
      gameVersion: modVersion === ALL_VERSIONS ? undefined : (modVersion.trim() || undefined),
      loader: modLoader === 'any' ? 'any' : (modLoader.trim() || undefined),
      category: modCategory,
      index: sort,
      offset: page * PAGE_SIZE,
    };

    const wantsModrinth = sources.includes('modrinth');
    const wantsCurseForge = sources.includes('curseforge');

    const [modrinthResult, curseForgeResult] = await Promise.allSettled([
      wantsModrinth
        ? (directMode
            // 直连模式：后端不加版本/加载器 facet，等于「不限」。
            ? searchModsUnscoped(request)
            : searchMods(targetInstanceId, request))
        : Promise.resolve([]),
      /*
       * CurseForge 现在也支持浏览模式：不传 searchFilter + sortField=6 会返回
       * 该版本/加载器下按总下载量排序的列表（已在真实接口上验证）。
       * 服务端排序是近似降序，所以拿到的这一页在下面会再按下载量排一次。
       */
      wantsCurseForge
        ? curseforgeSearch({
            kind: projectType === 'resourcepack' ? 'resourcepack' : 'mod',
            query: trimmed,
            /*
             * 直连模式不能用实例的值兜底：没有实例。此时只保留用户显式选的版本/
             * 加载器，都没选就不传——与 Modrinth 那条路径的语义保持一致，否则两站
             * 在同一个模式下会按不同条件过滤，结果无法互相解释。
             */
            gameVersion: modVersion === ALL_VERSIONS
              ? undefined
              : (modVersion.trim() || (directMode ? undefined : target?.version)),
            // 资源包没有加载器维度，传了会被服务端当成额外的筛选条件而漏结果。
            loader: projectType === 'resourcepack'
              ? undefined
              : (modLoader === 'any'
                  ? undefined
                  : (modLoader.trim() || (directMode ? undefined : target?.loader))),
            /*
             * CurseForge 的 `index` 是**偏移量**，不是页码——传页码会让每一页都
             * 从第 1 条开始取，翻页看起来完全没生效。
             *
             * 依据是后端自己的分页上限检查：`index + pageSize <= 10000`，只有当
             * `index` 是条目偏移时这个式子才成立（页号乘页大小才换算成偏移）。
             *
             * 与 Modrinth 的 `offset` 是同一个量，所以两站用同一个乘积。
             */
            index: page * PAGE_SIZE,
            pageSize: PAGE_SIZE,
          })
        : Promise.resolve([]),
    ]);

    /*
     * 过期响应一律丢弃。
     *
     * 这是修复「分类切了、结果没切」的最后一道闸：即使两个请求都成功返回，
     * 也只有序号最新的那次能写界面。放在这里而不是每个 setState 前判断，
     * 是因为从这里往下全是「本次请求的结果」——漏判一处就会重新出现旧数据。
     */
    if (!fresh()) return;

    const errors: string[] = [];
    if (modrinthResult.status === 'fulfilled') {
      setMods(modrinthResult.value);
    } else {
      setMods([]);
      errors.push(errorText(modrinthResult.reason, t('store.modrinthUnavailable')));
    }
    if (curseForgeResult.status === 'fulfilled') {
      /*
       * 浏览模式下对本地这一页再排一次。
       *
       * CurseForge 的 sortField=6 只是近似降序（实测约 1.9% 的相邻对逆序，
       * 单页最大偏差近 29%），直接显示会出现「第 4 名下载量比第 3 名高」这种
       * 一眼可见的矛盾。只对已拿到的这一页排序，不声称跨页的全局名次。
       * 有关键词时不动顺序——那里用户要的是相关度，不是下载量。
       */
      const hits = curseForgeResult.value;
      setCfMods(
        trimmed
          ? hits
          : [...hits].sort((a, b) => b.downloads - a.downloads),
      );
    } else {
      setCfMods([]);
      setCfSearchError(errorText(curseForgeResult.reason, t('cf.searchFailed')));
      errors.push(errorText(curseForgeResult.reason, t('cf.searchFailed')));
    }

    /*
     * 是否还有下一页：任一来源的当前页是满的，就认为可能还有。
     *
     * 满额判断在两个来源上都成立，而读取各自的 total 字段需要两套不同的解析，
     * 且 CurseForge 在缺 key 时根本不返回分页元数据。保守判断的代价只是最后
     * 一页可能多一次「翻过去发现是空的」，比「明明还有却翻不到」好得多。
     */
    setModHasMore(
      (modrinthResult.status === 'fulfilled' && modrinthResult.value.length >= PAGE_SIZE)
      || (curseForgeResult.status === 'fulfilled' && curseForgeResult.value.length >= PAGE_SIZE),
    );
    setModsState('ready');
    // 只在一个来源都拿不到时算整体失败；部分失败在各自栏里说明。
    const attempted = [wantsModrinth, wantsCurseForge].filter(Boolean).length;
    if (attempted > 0 && errors.length === attempted) {
      setModsError(errors.join(' '));
      setModsState('error');
    }
    /*
     * `category` 必须在依赖里。
     *
     * 这是一个**陈旧闭包**陷阱：`projectTypeOf(category)` 在函数体内读取
     * `category`，而 memo 缓存的是「定义时那一帧」的闭包。少写这一项，
     * `searchRef.current` 在分类切换后仍指向旧闭包，于是界面上标题变成
     * 「Resource packs」，请求却还在查 `project_type:mod`——实测切到资源包后
     * 结果里仍是 Fabric API / Sodium / Iris Shaders。
     *
     * `directMode` 同理：它由 `targetInstanceId` 推导，而那个已在依赖里。
     */
  }, [category, modCategory, modLoader, modSort, modVersion, sources, submittedQuery, t, target, targetInstanceId]);

  /*
   * 进入模组分类就自动搜索一次。
   *
   * 关键词可以为空——这时是「按热度浏览当前实例能装的模组」。之前必须输入关键词
   * 才发请求，导致第一次打开面板永远停在「输入关键词后按搜索」，看起来像没有接
   * 数据。PCL2 也是进页面即为列表。
   *
   * 依赖里刻意不放 `runModSearch`：它随 `submittedQuery` 与筛选条件变化，放进来
   * 会让「点搜索」之后再自动跑一次，同一份结果请求两遍。这里只在「进入分类」和
   * 「换目标实例」时触发，筛选与关键词的变化由各自的处理器显式调用。
   */
  const searchRef = useRef(runModSearch);
  searchRef.current = runModSearch;
  useEffect(() => {
    if (!MODRINTH_CATEGORIES.includes(category)) return;
    /*
     * 不要在这里判断 `targetInstanceId`。
     *
     * 直连模式（不选实例）同样需要搜索，而 `runModSearch` 已经自己区分了两条
     * 路径。之前这里有一个 `if (!targetInstanceId) return`，效果是：把下拉切到
     * 「不安装到实例」时**一次请求都不发**，界面停在切之前的实例结果上，而提示
     * 已经改成「直连下载模式」——两边互相矛盾。
     */
    void searchRef.current();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [category, targetInstanceId]);
  /**
   * CurseForge key 状态变化的处理。
   *
   * 必须用 `useCallback` 固定引用：`CurseForgeKeyCard` 内部把它放进 `refresh`
   * 的依赖，若这里每次渲染都新建闭包，会与那边的 `refresh` 形成渲染循环。
   */
  const handleCfKeyStatus = useCallback((status: { configured: boolean }) => {
    setCfKeyReady(status.configured);
    // 刚配好 key 时若已有搜索词，立刻重跑一次，省掉再点一次搜索。
    if (status.configured && submittedQuery.trim()) void searchRef.current();
  }, [submittedQuery]);

  /*
   * 筛选条件（含来源勾选）变化后立刻重搜。
   *
   * 这些是一次点击就能看到结果的条件，等待用户再按一次「搜索」只会让人以为没生效。
   * 用「上一次的值」比较来跳过首次挂载：挂载时由上面的 effect 负责，这里再跑一次
   * 就是重复请求。
   *
   * `sources` 必须在内：取消勾选一个来源后它的结果要立刻消失，否则勾选框看起来
   * 像没生效，得再点一次搜索。
   */
  const sourcesKey = [...sources].sort().join(',');
  const prevFilters = useRef({
    modCategory,
    modVersion,
    modLoader,
    modSort,
    sourcesKey,
  });
  useEffect(() => {
    // 同上：直连模式也要重搜。实例 id 只影响后端如何收窄，不影响「要不要搜」。
    if (!MODRINTH_CATEGORIES.includes(category)) return;
    const previous = prevFilters.current;
    if (
      previous.modCategory === modCategory
      && previous.modVersion === modVersion
      && previous.modLoader === modLoader
      && previous.modSort === modSort
      && previous.sourcesKey === sourcesKey
    ) {
      return;
    }
    prevFilters.current = { modCategory, modVersion, modLoader, modSort, sourcesKey };
    /*
     * 筛选条件变了就回到第一页。
     *
     * 换了个加载器或版本之后，后续页大概率是空的——继续停在第 3 页会让用户以为
     * 「这个筛选没有结果」，而首页其实有。
     */
    setModPage(0);
    void searchRef.current(undefined, 0);
  }, [category, modCategory, modSort, modVersion, modLoader, sourcesKey, targetInstanceId]);

  /**
   * 重置全部搜索条件并立刻重搜。
   *
   * 这里显式调用而不是依赖上面的 effect：如果原来的筛选已经是默认值、只有关键词
   * 非空，清空关键词不会让「筛选条件变化」的 effect 触发，列表就会继续显示旧关键
   * 词的结果，看起来像重置没生效。
   */
  const resetModFilters = () => {
    setQuery('');
    setSubmittedQuery('');
    setModCategory('');
    setModVersion('');
    setModLoader('');
    setModSort('downloads');
    setModPage(0);
    prevFilters.current = {
      modCategory: '',
      modVersion: '',
      modLoader: '',
      modSort: 'downloads',
      // 重置不动来源，所以这里沿用当前值，避免被误判成「来源变了」多搜一次。
      sourcesKey,
    };
    void searchRef.current('', 0);
  };

  const openModVersions = async (mod: CatalogMod) => {
    if (expandedMod === mod.projectId) { setExpandedMod(null); return; }
    setExpandedMod(mod.projectId);
    setModVersionsState('loading');
    setModVersions([]);
    setModVersionsError('');
    try {
      /*
       * 直连模式下列出**全部**版本，不按实例过滤。
       *
       * `listModVersions` 会用实例的游戏版本与加载器筛；没有实例时它无从筛起，
       * 而用任何默认值代替都等于静默收窄——用户在界面上看不到「你正在被过滤」。
       * 直连模式的语义就是「我自己挑」，所以给出完整列表。
       *
       * 资源包没有加载器维度，两条路径都透传当前分类的项目类型。
       */
      /*
       * 原版实例也允许列出全部版本，而不是报错。
       *
       * 版本清单是公开信息：用户点开一个模组想看它有哪些版本，这件事与「当前实例
       * 是否装了加载器」无关。之前原版实例会直接收到「在线模组目录需要 Fabric 或
       * Quilt 实例」，界面又把这个错误丢进搜索横幅——最终表现是卡片展开了但下面
       * 一片空白，看起来完全没反应。
       *
       * 仍然不给「安装」按钮（后端会按实例的加载器再校验一次），只是把版本列出来。
       */
      const isVanillaInstance = target !== undefined && target.loader === 'vanilla';
      setModVersions(await (targetInstanceId && !isVanillaInstance
        ? listModVersions(targetInstanceId, mod.projectId, projectTypeOf(category))
        : listProjectVersions(mod.projectId, projectTypeOf(category) === 'modpack' ? 'modpack' : 'mod')));
      setModVersionsState('ready');
    } catch (error) {
      setModVersionsState('error');
      /*
       * 只写进版本区的状态，不碰 `modsError`。
       *
       * 之前这里复用 `modsError`（搜索结果那条横幅）：一次展开失败会把
       * 「搜索失败」显示在顶部，而版本区仍然空白——两处都指向不了真正的原因。
       */
      setModVersionsError(errorText(error, t('cf.versionsFailed')));
    }
  };

  const installVersion = async (version: CatalogVersion) => {
    setInstalling(version.versionId);
    try {
      // 后端会在下载后按清单里的 SHA-1 校验；返回的是安装到的文件路径。
      // 项目类型决定落盘目录：资源包进 `resourcepacks/`，模组进 `mods/`。
      const path = await installModVersion(targetInstanceId, version, projectTypeOf(category));
      onNotice(`已安装 ${version.fileName} → ${path}`);
      onTasksChanged();
    } catch (error) {
      onNotice(errorText(error, '安装失败。'));
    } finally {
      setInstalling('');
    }
  };

  /**
   * 确认「直接下载」的落盘目录。
   *
   * 关键改动：没选过目录时**直接拉起选择对话框**，而不是先弹一句「请先选目录」。
   * 用户点了下载，意图很明确——他想把文件存下来。让他在两个地方之间来回找设置，
   * 是一次没有必要的往返。这里就地把目录问清楚，选完继续下载。
   *
   * 返回 null 表示这次下载不该继续（浏览器预览，或用户取消了选择）。
   */
  const requireDownloadDirectory = async (): Promise<string | null> => {
    if (!isDesktopShell()) {
      onNotice(t('store.directDownloadDesktopOnly'));
      return null;
    }
    const existing = downloadDir.trim();
    if (existing) return existing;
    // 还没选过：就地询问，选完写回偏好并继续。
    const picked = await pickDirectory(t('store.directDownloadPickTitle'));
    if (!picked) return null;
    setDownloadDir(picked);
    try {
      const current = readStoredPrefs();
      localStorage.setItem(
        SETTINGS_STORAGE_KEY,
        JSON.stringify({ ...current, downloadDirectory: picked }),
      );
    } catch { /* 仅本次会话生效 */ }
    return picked;
  };

  const downloadVersion = async (version: CatalogVersion) => {
    const directory = await requireDownloadDirectory();
    if (!directory) return;
    setInstalling(version.versionId);
    try {
      const path = await downloadModVersion(targetInstanceId, version, directory);
      setLastSaved({ name: version.fileName, path });
      onNotice(t('store.directDownloadDone', { values: { name: version.fileName, path } }));
      onTasksChanged();
    } catch (error) {
      onNotice(errorText(error, t('store.directDownloadFailed')));
    } finally {
      setInstalling('');
    }
  };

  /** 展开/收起某个 CurseForge 项目的文件列表。 */
  const openCfVersions = async (mod: CurseForgeMod) => {
    if (expandedCfMod === mod.projectId) { setExpandedCfMod(null); return; }
    setExpandedCfMod(mod.projectId);
    setCfVersionsState('loading');
    setCfVersions([]);
    setCfVersionsError('');
    /*
     * 没有目标实例时也要展开，并说明原因。
     *
     * 原来这里直接 `return`：按钮点下去毫无反应，用户无法判断是「没反应」还是
     * 「加载失败」。而 CurseForge 的查询必须带游戏版本与加载器，没有实例就无从
     * 得知该按什么筛选——所以正确做法不是静默失败，而是把这件事写出来，
     * 并给一个「去选择实例」的去处。
     */
    if (!target) {
      setCfVersionsState('error');
      setCfVersionsError(t('cf.needInstance'));
      return;
    }
    try {
      setCfVersions(await curseforgeFiles(mod.projectId, target.version, target.loader));
      setCfVersionsState('ready');
    } catch (error) {
      /*
       * 失败原因留在行内，不只弹一条会消失的 toast。
       *
       * 缺少/失效 API Key 是最常见的原因，而后端的错误文案已经写清了怎么办
       * （「请在内容商店里重新填写」）。用 toast 呈现意味着用户读完就没了，
       * 而列表区是一片空白——看起来像「这个模组没有文件」。
       */
      setCfVersionsState('error');
      setCfVersionsError(errorText(error, t('cf.versionsFailed')));
    }
  };

  /** 安装一个 CurseForge 文件。受限文件由后端拒绝并给出手动下载提示。 */
  const installCfVersion = async (version: CurseForgeVersion) => {
    if (!target) return;
    setInstalling(version.versionId);
    try {
      const name = await curseforgeInstall(target.id, version, 'mod');
      onNotice(t('cf.installedKind', { values: { kind: t('cf.kindMod'), name } }));
      onTasksChanged();
    } catch (error) {
      onNotice(errorText(error, t('cf.searchFailed')));
    } finally {
      setInstalling('');
    }
  };

  /** 把 CurseForge 文件直接下载到侧栏选定的目录，不装进实例。 */
  const downloadCfVersion = async (version: CurseForgeVersion) => {
    if (!target) return;
    const directory = await requireDownloadDirectory();
    if (!directory) return;
    setInstalling(version.versionId);
    try {
      const path = await curseforgeDownload(target.id, version, 'mod', directory);
      setLastSaved({ name: version.fileName, path });
      onNotice(t('store.directDownloadDone', { values: { name: version.fileName, path } }));
      onTasksChanged();
    } catch (error) {
      onNotice(errorText(error, t('store.directDownloadFailed')));
    } finally {
      setInstalling('');
    }
  };

  // -------------------------------------------------------------------------
  // 原版筛选与分页（清单全量在前端，筛选与分页都是本地计算）
  // -------------------------------------------------------------------------
  const vanillaRows = useMemo<VanillaRow[]>(() => {
    const needle = submittedQuery.trim().toLowerCase();
    return versions
      .map(version => ({ id: version.id, releaseTime: version.releaseTime, type: version.type }))
      .filter(row => kinds.includes(kindOfVersion(row.id, row.type)))
      .filter(row => !needle || row.id.toLowerCase().includes(needle));
  }, [kinds, submittedQuery, versions]);

  const pageCount = Math.max(1, Math.ceil(vanillaRows.length / PAGE_SIZE));
  const pageRows = vanillaRows.slice(page * PAGE_SIZE, page * PAGE_SIZE + PAGE_SIZE);

  useEffect(() => {
    if (page > pageCount - 1) setPage(pageCount - 1);
  }, [page, pageCount]);

  /**
   * 切换版本类型筛选。
   *
   * 至少要保留一类：允许全部取消的话，列表会变空并显示「至少选择一种版本类型」，
   * 但用户往往没意识到是自己点掉了最后一类，看起来就像「版本列表不见了」。
   * 这里直接拒绝取消最后一类，界面状态始终有效。
   */
  const toggleKind = (kind: VersionKind) => {
    setKinds(current => {
      const next = current.includes(kind) ? current.filter(item => item !== kind) : [...current, kind];
      return next.length === 0 ? current : next;
    });
    setPage(0);
  };

  const submitSearch = () => {
    setSubmittedQuery(query);
    setPage(0);
    /*
     * 换关键词必然回到第一页。
     *
     * 不重置的话，在第 3 页改成新关键词会从第 41 条开始取——那个新关键词可能
     * 根本没那么多结果，用户看到的是空白，而首页其实有内容。
     */
    setModPage(0);
    // 传入当前输入，避免闭包里读到的还是上一次的提交值。
    void runModSearch(query, 0);
  };

  /** 翻到模组结果的上一页/下一页。页码是服务端的，请求时带 offset。 */
  const goToModPage = (next: number) => {
    if (next < 0) return;
    setModPage(next);
    setExpandedMod(null);
    // 展开的版本列表属于上一页的项目，翻页后应当收起。
    setModVersions([]);
    void runModSearch(submittedQuery, next);
  };

  const pickPack = async () => {
    const path = await pickFile('选择整合包', [{ name: 'Minecraft 整合包', extensions: ['zip', 'mrpack'] }]);
    if (!path) return;
    setPackPath(path);
    setPackState('loading');
    setPackError('');
    try {
      setPackPreview(await previewModpack(path));
      setPackState('ready');
    } catch (error) {
      setPackError(errorText(error, '无法读取这个整合包。'));
      setPackState('error');
    }
  };

  const doImport = async () => {
    if (!packPath) return;
    setImporting(true);
    try {
      const record = await importModpack(packPath);
      onNotice(`整合包已导入为实例「${record.name}」。`);
      // 导入会创建一个新实例并触发安装任务；刷新后跳到游戏库查看结果。
      onTasksChanged();
      onCreated({
        id: record.id,
        name: record.name,
        version: record.gameVersion,
        loader: 'vanilla',
        javaVersion: '',
        playTime: 0,
        modCount: 0,
        status: 'ready',
      });
      setPackPath('');
      setPackPreview(null);
      setPackState('idle');
    } catch (error) {
      onNotice(errorText(error, '导入整合包失败。'));
    } finally {
      setImporting(false);
    }
  };

  // -------------------------------------------------------------------------
  // 站内整合包搜索与「下载并导入」
  // -------------------------------------------------------------------------

  /**
   * 搜索 CurseForge 的整合包。
   *
   * 关键词为空时按下载量浏览（后端已支持不传 searchFilter 的浏览模式），
   * 所以进入整合包页可以先点一次搜索看到热门列表，不必先想关键词。
   */
  const runPackSearch = async (pageOverride?: number) => {
    const page = pageOverride ?? 0;
    setPackSearchState('loading');
    setPackSearchError('');
    setPackHits([]);
    setMrPackHits([]);
    setExpandedPack(null);
    setExpandedMrPack(null);
    setPackPage(page);

    // 翻页复用上次的查询条件，而不是读输入框的实时内容：用户可能改了输入框但没点搜索。
    const query = pageOverride === undefined ? packQuery.trim() : lastPackQuery.current;
    lastPackQuery.current = query;
    /*
     * 两个来源并发查。
     *
     * Modrinth 需要一个实例来做 facet 构造（后端签名如此），但它只需要游戏版本与
     * 加载器；整合包不按这些筛，所以传当前目标实例即可，拿到的结果里后端**不**加
     * 版本/加载器 facet（整合包走的是 `project_versions` 与不加版本 facet 的搜索）。
     *
     * 没有目标实例时仍然查 Modrinth：整合包是「创建新实例」，与当前选中的实例
     * 无关。之前这里要求必须有实例，导致全新安装（一个实例都没有）时整合包页
     * 一片空白——而那恰恰是最需要它的时刻。
     */
    const wantsCf = sources.includes('curseforge');
    const [cfResult, mrResult] = await Promise.allSettled([
      wantsCf
        ? curseforgeSearch({
            kind: 'modpack',
            query,
            // 整合包跨版本可用，不按实例的版本/加载器收窄——否则会漏掉大量实际能装的包。
            index: page * PAGE_SIZE,
            pageSize: PAGE_SIZE,
          })
        : Promise.resolve([] as CurseForgeMod[]),
      searchModsUnscoped({
        query,
        projectType: 'modpack',
        // 不传 gameVersion / loader：整合包自带它们，按实例筛会漏掉大量能装的包。
        gameVersion: undefined,
        loader: 'any',
        // 排序与分页对齐模组页：按下载量，offset 用同一套页码。
        index: 'downloads',
        offset: page * PAGE_SIZE,
      }),
    ]);

    const errors: string[] = [];
    if (cfResult.status === 'fulfilled') {
      // 服务端排序是近似降序，本地再排一次，避免「第 4 名下载量比第 3 名高」。
      setPackHits([...cfResult.value].sort((a, b) => b.downloads - a.downloads));
    } else {
      // CurseForge 缺 key 是常态，不该让整页变成错误——Modrinth 的结果仍然有用。
      errors.push(errorText(cfResult.reason, t('cf.searchFailed')));
    }
    if (mrResult.status === 'fulfilled') {
      setMrPackHits([...mrResult.value].sort((a, b) => b.downloads - a.downloads));
    } else {
      errors.push(errorText(mrResult.reason, t('store.modrinthUnavailable')));
    }

    /*
     * 是否还有下一页。
     *
     * 用「本页是否满额」判断，与模组页同一套做法：两站都不返回可靠的总数
     * （CurseForge 的 pagination 与 Modrinth 的 total_hits 语义不同），读它们
     * 会让「最后一页之后还有一页」这种 bug 反复出现。
     */
    const fullPage =
      (cfResult.status === 'fulfilled' && cfResult.value.length >= PAGE_SIZE)
      || (mrResult.status === 'fulfilled' && mrResult.value.length >= PAGE_SIZE);
    setPackHasMore(fullPage && page < 40);

    // 两个来源都失败才算整页失败；只缺一个时另一栏仍然有用。
    if (errors.length === 2) {
      setPackSearchState('error');
      setPackSearchError(errors.join(' '));
      return;
    }
    // 部分失败在各自栏里说明，不把整页标成错误。
    setPackSearchError(errors.join(' '));
    setPackSearchState('ready');
  };

  /*
   * 进入整合包页就列出一批（按下载量）。
   *
   * 之前必须先输入关键词再点「搜索」——而用户往往不知道有哪些整合包，空着搜索框
   * 就不知道下一步该做什么。模组页早就是「空关键词 = 浏览热门」的行为，两页
   * 保持一致。
   *
   * 依赖只放 `category`：切到整合包页时跑一次。不把 `sources` 放进去是因为
   * 用户勾选来源后由「搜索」按钮触发重查，自动重跑会把他正在看的列表清空。
   */
  const packAutoLoaded = useRef(false);
  useEffect(() => {
    if (category !== 'modpack') { packAutoLoaded.current = false; return; }
    if (packAutoLoaded.current) return;
    packAutoLoaded.current = true;
    void runPackSearch(0);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [category]);

  /** 展开一个 Modrinth 整合包的文件列表。 */
  const openMrPackVersions = async (hit: CatalogMod) => {
    if (expandedMrPack === hit.projectId) { setExpandedMrPack(null); return; }
    setExpandedMrPack(hit.projectId);
    setMrPackVersionsState('loading');
    setMrPackVersions([]);
    try {
      setMrPackVersions(await listProjectVersions(hit.projectId, 'modpack'));
      setMrPackVersionsState('ready');
    } catch (error) {
      setMrPackVersionsState('error');
      setPackSearchError(errorText(error, '无法读取该整合包的版本列表。'));
    }
  };

  /** 下载并导入一个 Modrinth 整合包。 */
  const installMrPack = async (version: CatalogVersion) => {    setInstallingPack(version.versionId);
    try {
      const record = await downloadAndImportModrinthPack(version, targetInstanceId || undefined);
      onNotice(`整合包已导入为实例「${record.name}」。`);
      // 与 CurseForge 那条路径一致：导入会创建实例并触发安装任务，刷新后跳过去看。
      onTasksChanged();
      onCreated({
        id: record.id,
        name: record.name,
        version: record.gameVersion,
        loader: 'vanilla',
        javaVersion: '',
        playTime: 0,
        modCount: 0,
        status: 'ready',
      });
    } catch (error) {
      onNotice(errorText(error, '下载并导入整合包失败。'));
    } finally {
      setInstallingPack('');
    }
  };

  const openPackVersions = async (hit: CurseForgeMod) => {
    if (expandedPack === hit.projectId) { setExpandedPack(null); return; }
    setExpandedPack(hit.projectId);
    setPackVersionsState('loading');
    setPackVersions([]);
    try {
      // 不传实例的版本/加载器：整合包自带它要用的那套。
      setPackVersions(await curseforgeFiles(hit.projectId, '', undefined));
      setPackVersionsState('ready');
    } catch (error) {
      setPackVersionsState('error');
      onNotice(errorText(error, t('cf.versionsFailed')));
    }
  };

  /**
   * 把整合包文件「另存为」到用户选择的目录，不导入。
   *
   * 对应 PCL2 版本行上的保存按钮。与 `installPack` 共用后端的安全下载路径
   * （SHA-1 校验 + 不覆盖同名文件），区别只是不进入导入流程。
   */
  const savePack = async (file: CurseForgeVersion) => {
    if (!isDesktopShell()) {
      onNotice(t('store.directDownloadDesktopOnly'));
      return;
    }
    /*
     * `curseforge_download` 需要 instance_id 来登记下载任务与串行化。
     * 整合包不依赖某个具体实例，但传空串会让所有「另存为」共用一个空 key、
     * 互相阻塞，任务也会挂在空 id 下。这里退回当前选中的实例；一个都没有时
     * 明确报错，而不是发一个会留下怪数据的请求。
     */
    if (!targetInstanceId) {
      onNotice(t('store.saveAsNeedsInstance'));
      return;
    }
    const directory = await pickDirectory(t('store.directDownloadPickTitle'));
    if (!directory) return;
    setInstallingPack(file.versionId);
    try {
      const path = await curseforgeDownload(targetInstanceId, file, 'modpack', directory);
      onNotice(t('store.directDownloadDone', { values: { name: file.fileName, path } }));
      onTasksChanged();
    } catch (error) {
      onNotice(errorText(error, t('store.directDownloadFailed')));
    } finally {
      setInstallingPack('');
    }
  };

  /**
   * 把整合包文件按游戏版本分组。
   *
   * 参考 PCL2 的详情页：它把版本列表按 MC 版本分成多张卡片，而不是平铺几十行。
   * 一个成熟的整合包动辄有 50+ 个文件，平铺时用户很难找到「我这个 1.20.1 该用哪个」。
   *
   * 后端已把 CurseForge 混合数组里的加载器名拆出去（`gameVersions` 现在只含
   * 真正的版本号），所以这里直接取最大值作为归属键即可。取最新版本是因为一个
   * 文件可能声明支持多个版本，归到最新的那组更符合「这个包主要给哪个版本用」。
   */
  const groupedPackVersions = useMemo(() => {
    const groups = new Map<string, CurseForgeVersion[]>();
    for (const file of packVersions) {
      const key = file.gameVersions.length > 0
        // 数值序比较：字面比较会把 "1.9" 排在 "1.10" 之后。
        ? [...file.gameVersions].sort((a, b) => a.localeCompare(b, undefined, { numeric: true })).pop()!
        : '';
      const bucket = groups.get(key);
      if (bucket) bucket.push(file);
      else groups.set(key, [file]);
    }
    for (const files of groups.values()) {
      files.sort((a, b) => b.dateCreated.localeCompare(a.dateCreated));
    }
    // 有具体版本的组按版本号倒序在前，未声明版本的固定排最后。
    return [...groups.entries()].sort((a, b) => {
      if (a[0] === '') return 1;
      if (b[0] === '') return -1;
      return b[0].localeCompare(a[0], undefined, { numeric: true });
    });
  }, [packVersions]);

  /**
   * 下载并导入一个整合包文件。
   *
   * 后端负责「下载到临时目录 → 校验 SHA-1 → 按包内声明创建实例 → 下载包内文件」，
   * 失败会整条回滚。这里只需要提示结果并把新实例交给外层刷新列表。
   */
  const installPack = async (file: CurseForgeVersion) => {
    setInstallingPack(file.versionId);
    try {
      const record = await downloadAndImportModpack(file, targetInstanceId || undefined);
      onNotice(`整合包「${record.name}」已导入为实例。`);
      onTasksChanged();
      onCreated({
        id: record.id,
        name: record.name,
        version: record.gameVersion,
        loader: 'vanilla',
        javaVersion: '',
        playTime: 0,
        modCount: 0,
        status: 'ready',
      });
    } catch (error) {
      onNotice(errorText(error, '下载或导入整合包失败。'));
    } finally {
      setInstallingPack('');
    }
  };

  const categoryLabel = t(`category.${category}` as MessageKey);

  // 后端没有搜索/安装能力的分类，明确说明原因，而不是给一个失效的界面。
  /*
   * 资源包与地图现在走 CurseForge。
   *
   * 为什么只能走 CurseForge：Modrinth 没有地图这种项目类型，资源包虽然有
   * （`project_type: resourcepack`）但当前后端只实现了模组搜索。硬把地图塞进
   * Modrinth 搜索会得到一个永久的错误提示——这正是之前那句
   * 「Modrinth has no "world" project type」的来源。
   *
   * 所以这两类直接进入 CurseForge 面板，界面上不再出现来源切换：给一个切过去
   * 就会报错的选项，比不给更糟。
   */

  return (
    <div className={styles.store}>
      <header className={styles.header}>
        <div>
          <h1>{t('store.title')}</h1>
          <p className={styles.headerHint}>
            {category === 'vanilla'
              ? t('store.hintVanilla')
              : category === 'mod'
                // 副标题跟着勾选的来源走：只勾一个时说明那个站，两个都勾时说"两个来源"。
                ? sources.length === 2
                  ? t('store.hintBothSources')
                  : sources[0] === 'curseforge'
                    ? t('cf.headerHint')
                    : t('store.hintModrinth')
                : category === 'modpack'
                  ? '从本地选择一个整合包文件并导入为新实例'
                  : category === 'resourcepack'
                    // 资源包与地图固定走 CurseForge，副标题必须说明来源，否则用户
                    // 会以为是本地导入（这句之前对所有非 mod/vanilla 分类都显示
                    // 「选择整合包」）。
                    ? t('store.hintResourcePack')
                    : t('store.hintWorld')}
          </p>
        </div>
      </header>

      <div className={styles.workspace}>
        <aside className={styles.nav} aria-label={t('store.downloadSettings')}>
          <div className={styles.navTitle}>{t('store.downloadSettings')}</div>
          <div className={styles.currentType}>
            <span className={styles.categoryIcon}>
              {/*
                与抽屉菜单用同一枚线性图标。
                这一格宽 24px、颜色是 `--color-primary-text`，旁边没有其他图标可对照；
                若放彩色像素贴图，它会成为侧栏里唯一的彩色元素，而它的职责只是
                说明「现在在挑游戏版本」。
              */}
              {category === 'vanilla'
                ? <VanillaIcon size={22} />
                : <span className={styles.categoryFallback}>{categoryLabel.slice(0, 1)}</span>}
            </span>
            <div>
              {/*
                这里显示的是「当前在下载什么」，不是「内容分类」。
                原版分类在抽屉里叫「原版游戏」，但在这个位置它实际指的是
                「游戏版本列表」——用户在这里要的是版本号，不是内容类型。
                沿用 `category.vanilla` 会让这一格写「原版游戏」，与右侧的
                「搜索原版游戏 / 版本列表」重复且含糊。
              */}
              <strong>{category === 'vanilla' ? t('store.versionPicker') : categoryLabel}</strong>
              <small>{t('store.switchTypeHint')}</small>
            </div>
          </div>
          <div className={styles.navDivider} />
          {/*
            「当前实例」只在需要它的分类下显示：
              - 原版：版本清单是全局的（Mojang 官方），与实例无关，给选择器会误导
                用户以为版本列表被实例过滤；
              - 整合包：导入直接创建新实例，不需要先选目标。
            之前除整合包外都显示，原版页因此有一个点了没有任何作用的控件。
          */}
          {category !== 'modpack' && category !== 'vanilla' && (
            <div className={styles.navFooter}>
              {/*
                「要装到哪」与「直接下载存到哪」是两件事，视觉上必须分开：
                  - 目标实例决定「安装」按钮把文件写进哪个 mods/resourcepacks 目录；
                  - 下载目录只影响「直接下载」，与实例无关。
                之前两块在同一个容器里紧挨着，中间没有任何分隔或小标题，看起来像
                同一个设置的两个子项，用户会以为改下载目录会改变安装位置。
              */}
              <div className={styles.targetBlock}>
                <span className={styles.blockHeading}>{t('store.installTarget')}</span>
                <label htmlFor="store-target">{t('store.currentInstance')}</label>
                <select
                  id="store-target"
                  value={targetInstanceId}
                  /*
                   * 必须同时更新本地 state 与外层选择。
                   *
                   * 这个 `<select>` 的 `value` 绑定的是组件内的 `targetInstanceId`，
                   * 只调用 `onSelectInstance` 的话，外层更新 `selectedInstance`
                   * 之后本地的值仍是旧的，React 会把下拉弹回上一项——表现就是
                   * 「点选后不切换」。外层的 `onSelectInstance` 负责把选择同步给
                   * 应用其他部分（例如实例详情页），两者都要更新。
                   */
                  onChange={event => {
                    const next = event.target.value;
                    setTargetInstanceId(next);
                    onSelectInstance(next);
                  }}
                  /*
                   * 标签可能因为宽度不够被截断（见 `fitInstanceLabel`），
                   * 悬停时给完整名称，截断就不算丢信息。
                   */
                  title={target ? `${target.name} · ${target.version}` : undefined}
                >
                  {/*
                    「不安装到实例」是一个**可选项**，不是一个禁用占位。

                    之前它在没有实例时才出现、且是 disabled：用户看到的是一个
                    灰色选项，既不知道它有什么用，也不能选它。而它对应的正是
                    「直连下载」这个真实用法——不装进任何实例，只把文件存到磁盘。

                    放在第一项：选它之后整页会切到直连模式（搜索不再按实例的版本
                    与加载器收窄，版本列表也不过滤），这是最彻底的语义变化，放在
                    顶部才不会让人误以为是某个实例的别名。
                  */}
                  <option value="">{t('store.noInstanceOption')}</option>
                  {instances.length === 0 && (
                    <option value="" disabled>{t('store.noInstances')}</option>
                  )}
                  {instances.map(instance => (
                    <option key={instance.id} value={instance.id}>
                      {fitInstanceLabel(instance.name, instance.version)}
                    </option>
                  ))}
                </select>
                {/*
                  两种提示，二选一：
                    - 没选实例 → 说明现在是直连模式（什么被改变、什么没被改变）；
                    - 选了原版实例 → 说明模组需要带加载器的实例。

                  都是 i18n 键。原来这一句是硬编码中文，而其余界面文案已经本地化，
                  切换语言时它会单独留在中文。
                */}
                {!target && (
                  <p className={styles.needKey}>{t('store.directModeHint')}</p>
                )}
                {category === 'mod' && target && target.loader === 'vanilla' && (
                  <p className={styles.needKey}>{t('store.needModLoader')}</p>
                )}
              </div>

              {/*
                「直接下载」的保存目录。
                只在模组页显示：这是唯一有「直接下载」按钮的分类。

                默认**收起**，只留一行标题 + 当前状态摘要。原因：这一块包含三行
                说明加一个整宽按钮，常驻会占掉侧栏近 200px，而绝大多数用户在这里
                是「装进实例」，直接下载是次要路径。收起后侧栏回到「安装目标」
                一件事，需要时才展开，符合「先选目标，要用再配下载」的顺序。

                已设置过目录时默认展开：那种情况下这一块是有效配置，藏起来会让人
                以为自己没配过。
              */}
              {MODRINTH_CATEGORIES.includes(category) && (
                <div className={`${styles.targetBlock} ${styles.directBlock}`}>
                  <button
                    type="button"
                    className={styles.directToggle}
                    aria-expanded={directOpen}
                    onClick={() => setDirectOpen(current => !current)}
                  >
                    <span className={styles.blockHeading}>{t('store.directDownloadBlock')}</span>
                    {/*
                      收起时给出「现在是什么状态」：已设置显示目录名，未设置明确说
                      「未设置」。只收一个标题会让用户不知道点开是不是有用。
                    */}
                    <span className={styles.directSummary} title={downloadDir || undefined}>
                      {downloadDir ? folderName(downloadDir) : t('store.directDownloadNotSet')}
                    </span>
                    <ChevronDown
                      size={14}
                      className={`${styles.directChevron} ${directOpen ? styles.directChevronOpen : ''}`}
                      aria-hidden="true"
                    />
                  </button>

                  {directOpen && (
                    <div className={styles.directBody}>
                      <p className={styles.blockHint}>{t('store.directDownloadBlockHint')}</p>

                      {downloadDir ? (
                        <>
                          {/*
                            已选目录：路径本身就是一个「打开文件夹」按钮。
                            之前这里只是个静态文本框，用户下载完看着路径却点不了——
                            「文件在哪」这个问题在界面上无法闭环。
                          */}
                          <div className={styles.dirRow}>
                            <button
                              id="store-download-dir"
                              type="button"
                              className={styles.dirPicker}
                              onClick={() => void openDownloadDir()}
                              title={`${downloadDir}\n${t('store.directDownloadOpenHint')}`}
                            >
                              <span className={styles.dirPath}>{downloadDir}</span>
                              <FolderDown size={14} />
                            </button>
                            <button
                              type="button"
                              className={styles.dirChange}
                              onClick={() => void chooseDownloadDir()}
                              title={t('store.directDownloadChangeDir')}
                            >
                              {t('common.edit')}
                            </button>
                          </div>
                          {/*
                            最近一次保存的文件。toast 会消失，但用户往往过一会儿才想起
                            「刚才那个 jar 放哪了」——这一行让答案一直留在界面上。
                          */}
                          {lastSaved && (
                            <button
                              type="button"
                              className={styles.lastSaved}
                              onClick={() => void openDownloadDir()}
                              title={lastSaved.path}
                            >
                              <span className={styles.lastSavedLabel}>
                                {t('store.directDownloadLastSaved')}
                              </span>
                              <span className={styles.lastSavedName}>{lastSaved.name}</span>
                            </button>
                          )}
                        </>
                      ) : (
                        /*
                          未选目录时不只显示「未选择」：那是一个死状态，用户得自己猜
                          要先去点哪里。做成一句可直接执行的话 + 按钮。
                        */
                        <>
                          <p className={styles.dirEmptyHint}>
                            {t('store.directDownloadNoDirHint')}
                          </p>
                          <button
                            id="store-download-dir"
                            type="button"
                            className={styles.dirPickCta}
                            onClick={() => void chooseDownloadDir()}
                          >
                            <FolderDown size={14} />
                            {t('store.directDownloadPickCta')}
                          </button>
                        </>
                      )}
                    </div>
                  )}
                </div>
              )}
            </div>
          )}
        </aside>

        <section className={styles.panel}>
          {category === 'vanilla' && (
            <>
              <div className={styles.filterCard}>
                <div className={styles.filterTitle}>
                  <h2>{t('store.searchCategory', { values: { category: categoryLabel } })}</h2>
                  <span>
                    {versionsState === 'ready'
                      ? t('store.versionCount', { count: vanillaRows.length })
                      : versionsState === 'loading'
                        ? t('store.loading')
                        : '—'}
                  </span>
                </div>
                <div className={styles.filterGrid}>
                  <label className={`${styles.field} ${styles.fieldWide}`}>
                    <span>{t('store.name')}</span>
                    <div className={styles.fieldInput}>
                      <Search size={15} />
                      <input
                        value={query}
                        onChange={event => setQuery(event.target.value)}
                        onKeyDown={event => { if (event.key === 'Enter') submitSearch(); }}
                        placeholder={t('store.searchPlaceholderVersion')}
                      />
                    </div>
                  </label>
                  <div className={`${styles.field} ${styles.fieldWide}`}>
                    <span>{t('store.versionType')}</span>
                    <div className={styles.checkGroup}>
                      {VERSION_KINDS.map(kind => (
                        <label key={kind} className={styles.checkOption}>
                          <input type="checkbox" checked={kinds.includes(kind)} onChange={() => toggleKind(kind)} />
                          <span>{t(VERSION_KIND_KEYS[kind])}</span>
                        </label>
                      ))}
                    </div>
                  </div>
                </div>
                <div className={styles.filterActions}>
                  <Button onClick={submitSearch} icon={<Search size={15} />}>{t('store.search')}</Button>
                  <Button
                    variant="ghost"
                    onClick={() => { setQuery(''); setSubmittedQuery(''); void loadVersions(); }}
                    icon={<RefreshCw size={14} />}
                  >
                    {t('store.resetFilters')}
                  </Button>
                </div>
              </div>

              <div className={styles.resultsCard}>
                <div className={styles.resultsHeader}>
                  <h2>{t('store.versionList')}</h2>
                </div>

                {/*
                  安装面板放在列表上方：用户点了某一行的「安装」，面板就地展开，
                  仍能看到自己点的是哪个版本。见 InstallPlan 的头部注释。
                */}
                <InstallPlan
                  target={installTarget}
                  onClose={() => setInstallTarget(null)}
                  onNotice={onNotice}
                  onDone={async () => {
                    setInstallTarget(null);
                    onTasksChanged();
                  }}
                />

                {versionsState === 'error' && (
                  <div className={styles.errorRow} role="alert">
                    <span>{versionsError}</span>
                    <div className={styles.errorActions}>
                      <button type="button" className={styles.linkButton} onClick={() => void loadVersions()}>
                        <RefreshCw size={12} />{t('common.retry')}
                      </button>
                    </div>
                  </div>
                )}

                {versionsState === 'loading' && (
                  <div className={styles.skeletonList} aria-busy="true">
                    {[0, 1, 2, 3].map(index => (
                      <div key={index} className={styles.skeletonRow}>
                        <span className={styles.skeletonIcon} />
                        <span className={styles.skeletonLines}><i /><i /></span>
                      </div>
                    ))}
                  </div>
                )}

                {/*
                  空状态要区分两种完全不同的原因，并各自给出可执行的下一步：
                    - 清单本身为空或加载失败 → 显示重试，而不是让用户去猜；
                    - 有清单但被筛选条件排除 → 提示调整类型/关键词。
                  之前两种情况共用一句「至少选择一种版本类型，且确认清单已加载」，
                  用户既不知道是哪一种，也没有可以点的动作。
                */}
                {versionsState === 'ready' && pageRows.length === 0 && (
                  <div className={styles.empty}>
                    {versions.length === 0 ? (
                      <>
                        <span>{t('store.manifestEmpty')}</span>
                        <button type="button" className={styles.linkButton} onClick={() => void loadVersions()}>
                          <RefreshCw size={12} />{t('common.retry')}
                        </button>
                      </>
                    ) : (
                      <span>{t('store.emptyNoKind')}</span>
                    )}
                  </div>
                )}

                <div className={styles.versionList}>
                  {pageRows.map(row => {
                    const kind = kindOfVersion(row.id, row.type);
                    const block = blockForVersion(kind) ?? VANILLA_BLOCK;
                    return (
                      <div key={row.id} className={styles.versionRow}>
                        <BlockIcon {...block} size={36} />
                        <div className={styles.versionMain}>
                          <div className={styles.versionTitle}>
                            <strong>Minecraft {row.id}</strong>
                            <span className={kind === 'special' ? styles.kindSpecial : styles.kindBadge}>
                              {t(VERSION_KIND_KEYS[kind])}
                            </span>
                          </div>
                          <span className={styles.versionMeta}>
                            {new Date(row.releaseTime).toLocaleDateString(
                              language === 'zh-CN' ? 'zh-CN' : language === 'ja' ? 'ja-JP' : 'en-US',
                            )}
                          </span>
                        </div>
                        {/*
                          点「安装」展开安装面板，而不是弹一句提示。
                          之前这里只提示「去游戏库新建实例」，用户被指去另一个页面
                          手动走完整流程；现在就地完成：填名称、选加载器、开始安装。
                        */}
                        <Button
                          size="sm"
                          onClick={() => setInstallTarget({ version: row.id })}
                        >
                          {t('store.install')}
                        </Button>
                      </div>
                    );
                  })}
                </div>

                {pageCount > 1 && (
                  <nav className={styles.pager} aria-label={t('store.pageNavLabel')}>
                    <span className={styles.pagerRange}>
                      {t('store.pageRange', {
                        values: {
                          from: page * PAGE_SIZE + 1,
                          to: page * PAGE_SIZE + pageRows.length,
                          total: vanillaRows.length,
                        },
                      })}
                    </span>
                    <div className={styles.pagerControls}>
                      <button
                        type="button"
                        className={styles.pagerButton}
                        disabled={page === 0}
                        onClick={() => setPage(current => Math.max(0, current - 1))}
                      >
                        <ChevronLeft size={15} />{t('store.pagePrev')}
                      </button>
                      <span className={styles.pagerInfo}>
                        {t('store.pageInfo', { values: { page: page + 1, pages: pageCount } })}
                      </span>
                      <button
                        type="button"
                        className={styles.pagerButton}
                        disabled={page >= pageCount - 1}
                        onClick={() => setPage(current => Math.min(pageCount - 1, current + 1))}
                      >
                        {t('store.pageNext')}<ChevronRight size={15} />
                      </button>
                    </div>
                  </nav>
                )}
              </div>
            </>
          )}

          {/*
            来源与关键词放在同一张筛选卡里，来源做成多选勾选。
            把「查哪些站」和「查什么」放在一起，是因为它们共同构成一次搜索；
            之前把来源单独放在外面做成单选切换，用户得先切站再搜，想两边都看
            就必须来回切换、丢一次结果。
          */}
          {MODRINTH_CATEGORIES.includes(category) && (
            <div className={styles.filterCard}>
              <div className={styles.filterTitle}>
                <h2>{t('store.searchMods')}</h2>
                <span>
                  {modsState === 'loading'
                    ? t('store.loading')
                    : modsState === 'ready'
                      ? t('store.combinedCount', { values: { modrinth: mods.length, curseforge: cfMods.length } })
                      : '—'}
                </span>
              </div>
              <div className={styles.filterGrid}>
                <label className={`${styles.field} ${styles.fieldWide}`}>
                  <span>{t('store.name')}</span>
                  <div className={styles.fieldInput}>
                    <Search size={15} />
                    <input
                      value={query}
                      onChange={event => setQuery(event.target.value)}
                      onKeyDown={event => { if (event.key === 'Enter') submitSearch(); }}
                      placeholder={t('store.queryPlaceholder')}
                    />
                  </div>
                </label>
              </div>
              <div className={styles.filterActions}>
                <Button onClick={submitSearch} icon={<Search size={15} />}>{t('store.search')}</Button>
                <Button variant="ghost" onClick={resetModFilters}>{t('store.resetFilters')}</Button>
              </div>
              <p className={styles.mixedNote}>{t('store.searchDisclosure')}</p>

              {/*
                版本 / 类型 / 排序。
                这三项对应 PCL2 搜索面板的「版本」「类型」与结果排序：改一次就该看到新
                结果，因此它们不经过「搜索」按钮，直接在 useEffect 里重跑。
                版本留空表示跟随目标实例，留空时 hint 会写明实际使用的版本。
              */}
              <div className={`${styles.filterGrid} ${styles.filterGridEven}`}>
                <div className={`${styles.field} ${styles.fieldVersion}`}>
                  <span>{t('store.filterVersion')}</span>
                  <VersionPicker
                    value={modVersion}
                    onChange={setModVersion}
                    followLabel={followVersionLabel}
                    versions={versionList}
                    state={versionListState}
                    onRequestVersions={requestVersionList}
                    label={t('store.filterVersion')}
                    placeholder={t('store.versionPlaceholder')}
                    /* 「全部版本」只在按版本筛选有意义的地方出现。 */
                    allLabel={t('store.versionAll')}
                  />
                </div>
                {/*
                  加载器筛选只对模组有意义。资源包的版本里 `loaders` 是
                  `["minecraft"]`，给它一个 Fabric/Forge 的下拉只会让用户选出一个
                  必然为空的结果——搜索时已经被忽略，界面上就不该出现。
                */}
                {category === 'mod' && (
                  <label className={styles.field}>
                    <span>{t('store.filterLoader')}</span>
                    <select
                      value={modLoader}
                      onChange={event => setModLoader(event.target.value)}
                    >
                      <option value="">{t('store.loaderFollowInstance', { values: { loader: target?.loader ?? '—' } })}</option>
                      <option value="any">{t('store.loaderAny')}</option>
                      <option value="fabric">Fabric</option>
                      <option value="quilt">Quilt</option>
                      <option value="forge">Forge</option>
                      <option value="neoforge">NeoForge</option>
                    </select>
                  </label>
                )}
                <label className={styles.field}>
                  <span>{t('store.filterCategory')}</span>
                  <select
                    value={modCategory}
                    onChange={event => setModCategory(event.target.value)}
                  >
                    <option value="">{t('store.categoryAll')}</option>
                    {MOD_CATEGORIES.map(item => (
                      <option key={item.slug} value={item.slug}>{t(item.labelKey)}</option>
                    ))}
                  </select>
                </label>
                <label className={styles.field}>
                  <span>{t('store.filterSort')}</span>
                  <select
                    value={modSort}
                    onChange={event => setModSort(event.target.value as typeof modSort)}
                  >
                    <option value="downloads">{t('store.sortDownloads')}</option>
                    <option value="relevance">{t('store.sortRelevance')}</option>
                    <option value="newest">{t('store.sortNewest')}</option>
                    <option value="updated">{t('store.sortUpdated')}</option>
                    <option value="follows">{t('store.sortFollows')}</option>
                  </select>
                </label>
              </div>

              {/*
                来源复选。CurseForge 需要用户自己的 key，所以勾上它但没配 key 时，
                就地给出设置入口，而不是让一次搜索以错误结束。
              */}
              <div className={styles.sourcePick}>
                <span className={styles.sourcePickLabel}>{t('cf.source')}</span>
                <label className={styles.sourceOption}>
                  <input
                    type="checkbox"
                    checked={sources.includes('modrinth')}
                    onChange={() => toggleSource('modrinth')}
                  />
                  <span>Modrinth</span>
                </label>
                <label className={styles.sourceOption}>
                  <input
                    type="checkbox"
                    checked={sources.includes('curseforge')}
                    onChange={() => toggleSource('curseforge')}
                  />
                  <span>CurseForge</span>
                </label>
              </div>

              {/*
                没有用户 key 时给一条中性说明，而不是把整张设置卡片顶出来。

                CurseForge 现在无 key 也能用（走 MCIM 镜像），所以在筛选区展开一个
                「请填写 API Key」的表单会传达错误前提：用户会以为不填就用不了。
                这里只在状态查回来确实「没有 key」时给一行可选入口。
              */}
              {sources.includes('curseforge') && cfKeyReady === false && (
                <div className={styles.cfKeySlot}>
                  <p className={styles.mirrorNote}>{t('cf.mirrorNotice')}</p>
                  <CurseForgeKeyCard
                    compact
                    onStatusChange={handleCfKeyStatus}
                    onNotice={onNotice}
                  />
                </div>
              )}
            </div>
          )}

          {/*
            合并结果区。
            两站的条目混在同一列表里，但每条都带来源标记；顶部分别写明各站条数，
            这样「为什么这个数字是这样」是可解释的。之前把来源做成互斥切换，正是
            为了回避这个计数问题——用来源标记 + 分项计数解决它，比强迫用户切换更好。
          */}
          {MODRINTH_CATEGORIES.includes(category) && (
            <div className={styles.resultsCard}>
              <div className={styles.resultsHeader}>
                <h2>{t('store.searchResults')}</h2>
                {modsState === 'ready' && (mods.length > 0 || cfMods.length > 0) && (
                  <span className={styles.resultsBreakdown}>
                    {sources.includes('modrinth') && `Modrinth ${mods.length}`}
                    {sources.includes('modrinth') && sources.includes('curseforge') && ' · '}
                    {sources.includes('curseforge') && `CurseForge ${cfMods.length}`}
                  </span>
                )}
              </div>

              {modsState === 'error' && (
                <div className={styles.errorRow} role="alert">
                  <span>{modsError}</span>
                  <div className={styles.errorActions}>
                    <button type="button" className={styles.linkButton} onClick={() => void runModSearch()}>
                      <RefreshCw size={12} />{t('common.retry')}
                    </button>
                  </div>
                </div>
              )}

              {/*
                搜索失败时给出可执行的下一步。
                之前错误行下面直接是空白结果区：用户看到一句「超时」，既不知道是
                网络问题还是站点问题，也看不出该等一等还是改设置。这里补上具体的
                排查方向，并说明重试已经自动做过（见 `modrinthGet` 的重试）。
              */}
              {modsState === 'error' && (
                <ul className={styles.errorHints}>
                  <li>{t('store.errorHintNetwork')}</li>
                  <li>{t('store.errorHintRetry')}</li>
                </ul>
              )}

              {/* 部分失败：一个站挂了，另一个的结果仍然显示。 */}
              {modsState === 'ready' && cfSearchError && (
                <div className={styles.errorRow} role="alert">
                  <span>{cfSearchError}</span>
                </div>
              )}

              {/* 浏览模式的说明：不解释的话「只有 20 条」看起来像 bug。 */}
              {modsState === 'ready' && !submittedQuery.trim() && (
                <p className={styles.mixedNote}>{t('store.browseModeNote')}</p>
              )}

              {modsState === 'idle' && (
                <div className={styles.empty}>
                  {targetInstanceId ? t('store.searchIdle') : t('store.needInstanceFirst')}
                </div>
              )}
              {modsState === 'loading' && (
                <div className={styles.skeletonList} aria-busy="true">
                  {[0, 1, 2, 3].map(index => (
                    <div key={index} className={styles.skeletonRow}>
                      <span className={styles.skeletonIcon} />
                      <span className={styles.skeletonLines}><i /><i /></span>
                    </div>
                  ))}
                </div>
              )}

              {modsState === 'ready' && mods.length === 0 && cfMods.length === 0 && !cfSearchError && (
                <div className={styles.empty}>{t('store.noModResults')}</div>
              )}

              <div className={styles.versionList}>
                {mods.map(mod => (
                  <div key={`mr-${mod.projectId}`}>
                    <div className={styles.contentRow}>
                      <StoreLogo src={mod.iconUrl ?? undefined} alt={mod.title} category="mod" />
                      <div className={styles.contentMain}>
                        <div className={styles.contentTitleLine}>
                          <strong>{mod.title}</strong>
                          <span className={`${styles.sourceBadge} ${styles.badgeModrinth}`}>Modrinth</span>
                        </div>
                        <p>{mod.description || t('store.noSummary')}</p>
                        <span className={styles.contentMeta}>
                          {mod.author ? `${mod.author} · ` : ''}
                          {t('store.downloadsShort', {
                            values: { value: mod.downloads.toLocaleString(locale) },
                            count: mod.downloads,
                          })}
                          {' · '}
                          {t('store.followsShort', {
                            values: { value: mod.follows.toLocaleString(locale) },
                            count: mod.follows,
                          })}
                        </span>
                      </div>
                      <Button size="sm" variant="ghost" onClick={() => void openModVersions(mod)}>
                        {expandedMod === mod.projectId ? t('cf.collapse') : t('cf.chooseVersion')}
                      </Button>
                    </div>

                    {expandedMod === mod.projectId && (() => {
                      /*
                       * 这一页的版本是否跨多个加载器。
                       *
                       * 用 IIFE 而不是在组件顶层算：它只在这一个模组展开时才需要，
                       * 而展开状态随时变化；放在这里不必为「未展开」的情况维护
                       * 一份空的派生值。
                       */
                      const loaders = new Set(
                        modVersions
                          .flatMap(version => (version.loaders ?? []).filter(l => l !== 'minecraft'))
                          .map(l => l.toLowerCase()),
                      );
                      const pagesHaveMultipleLoaders = loaders.size > 1;
                      return (
                      <div className={styles.versionListInner}>
                        {modVersionsState === 'loading' && (
                          <div className={styles.versionMeta}>{t('store.loading')}</div>
                        )}
                        {modVersionsState === 'ready' && modVersions.length === 0 && (
                          <div className={styles.versionMeta}>{t('cf.noVersion')}</div>
                        )}
                        {/*
                          失败要就地说明。
                          之前 error 只写进 `setModsError`（搜索结果区那条横幅），
                          版本区没有任何分支——于是点开一个模组后，卡片显示「收起」
                          却下面是空的，看起来像「这个项目没有版本」。
                          Modrinth 的版本请求失败（网络、限流、该项目无文件）都会
                          走到这里。
                        */}
                        {modVersionsState === 'error' && (
                          <div className={styles.versionError} role="alert">
                            <span>{modVersionsError || t('cf.versionsFailed')}</span>
                          </div>
                        )}
                        {/*
                          一行说明：这一列到底是「装进实例」还是「只下载」。
                          用户看到的按钮不同（有实例时两个、直连时一个），不解释的话
                          会以为功能少了一个。
                        */}
                        {modVersionsState === 'ready' && modVersions.length > 0 && (
                          <p className={styles.versionListHint}>{t('store.versionListHint')}</p>
                        )}
                        {/*
                          直连模式的版本列表不按实例过滤，条数可能上百（实测 100）。
                          只渲染前 40 个并说明剩余数量：全铺开会让页面长达数千像素，
                          而用户要找的是最近几个版本。
                        */}
                        {/*
                          整页是否含多种加载器。用于决定元信息里要不要写加载器
                          （见 buildVersionMeta 的说明）。在 map 外算一次，
                          避免对 36 个版本各扫一遍。
                        */}
                        {modVersions.slice(0, VERSION_PREVIEW_LIMIT).map(version => (
                          /*
                           * 版本行：紧凑单行，参考 PCL2 的 ResourceVersion.ToListItem。
                           *
                           * PCL2 的版本项固定 42px 高、单行展示，标题去掉扩展名，
                           * 右侧一行逗号分隔的元信息（加载器、前置数、下载量、更新
                           * 时间、发布类型）。关键是**一屏能扫过十几个版本**——
                           * 之前这里是「大图标 + 两行文字 + 两个按钮」的卡车式行，
                           * 每个占 120px 以上，一个模组的版本列表就吞掉整页。
                           *
                           * 现在值本身也带上来了（versionType/loaders/downloads），
                           * 所以那行元信息是真信息，而不是只有文件名和大小。
                           */
                          <div key={version.versionId} className={styles.versionRowCompact}>
                            <div className={styles.versionMain}>
                              <div className={styles.versionTitle}>
                                {/*
                                  标题按 PCL2 的做法去掉 `.jar` 后缀：扩展名对玩家
                                  没有信息量，而它占掉的那 4 个字符常常正好把版本号
                                  挤到省略号里。
                                */}
                                <strong>{stripArchiveExtension(version.fileName)}</strong>
                                {/*
                                  发布类型标记。只在非 release 时出现——给每个正式版
                                  都挂一个「正式」标签等于没有标签。
                                */}
                                {(version.versionType === 'beta' || version.versionType === 'alpha') && (
                                  <span
                                    className={`${styles.releaseTag} ${version.versionType === 'alpha' ? styles.releaseAlpha : styles.releaseBeta}`}
                                    title={t(version.versionType === 'alpha' ? 'store.releaseAlpha' : 'store.releaseBeta')}
                                  >
                                    {version.versionType === 'alpha' ? 'alpha' : 'beta'}
                                  </span>
                                )}
                              </div>
                              {/*
                                元信息：逗号分隔的一行，与 PCL2 一致。
                                顺序按「用户判断时最关心的在前」：加载器 → 游戏版本 →
                                大小 → 前置数 → 下载量 → 更新时间。加载器排第一，
                                因为它是最常见的「装错版本」原因。
                              */}
                              <span className={styles.versionMeta}>
                                {buildVersionMeta(version, locale, t, pagesHaveMultipleLoaders)}
                              </span>
                            </div>
                            <div className={styles.versionActions}>
                              {/*
                                直连模式不显示「安装」。
                                没有目标实例，`installModVersion` 无从写入——留一个
                                点了必然报错的按钮，比不显示更糟。此时只剩「仅下载」，
                                而这正是直连模式的定义。

                                原版实例 + 模组同理：后端会拒绝（模组在原版客户端里不会
                                被加载），所以这里也不给按钮，避免点了才看到错误。
                                资源包与光影在原版里生效，因此只对「模组」这一类收窄。
                              */}
                              {!directMode && !(target?.loader === 'vanilla' && category === 'mod') && (
                                <Button
                                  size="sm"
                                  disabled={installing === version.versionId}
                                  icon={<Download size={13} />}
                                  onClick={() => void installVersion(version)}
                                >
                                  {installing === version.versionId ? t('store.loading') : t('store.install')}
                                </Button>
                              )}
                              <Button
                                size="sm"
                                variant={directMode ? undefined : 'secondary'}
                                disabled={installing === version.versionId}
                                icon={<FolderDown size={13} />}
                                title={t('store.directDownloadHint')}
                                onClick={() => void downloadVersion(version)}
                              >
                                {t('store.directDownload')}
                              </Button>
                            </div>
                          </div>
                        ))}
                        {modVersions.length > VERSION_PREVIEW_LIMIT && (
                          <div className={styles.versionMeta}>
                            {t('store.moreVersions', {
                              count: modVersions.length - VERSION_PREVIEW_LIMIT,
                              values: { count: modVersions.length - VERSION_PREVIEW_LIMIT },
                            })}
                          </div>
                        )}
                      </div>
                      );
                    })()}
                  </div>
                ))}

                {cfMods.map(mod => (
                  <div key={`cf-${mod.projectId}`}>
                    <div className={styles.contentRow}>
                      <StoreLogo src={mod.iconUrl ?? undefined} alt={mod.title} category="mod" />
                      <div className={styles.contentMain}>
                        <div className={styles.contentTitleLine}>
                          <strong>{mod.title}</strong>
                          <span className={`${styles.sourceBadge} ${styles.badgeCurseforge}`}>CurseForge</span>
                        </div>
                        <p>{mod.description || t('store.noSummary')}</p>
                        <span className={styles.contentMeta}>
                          {mod.author ? `${mod.author} · ` : ''}
                          {t('store.downloadsShort', {
                            values: { value: mod.downloads.toLocaleString(locale) },
                            count: mod.downloads,
                          })}
                        </span>
                      </div>
                      <Button size="sm" variant="ghost" onClick={() => void openCfVersions(mod)}>
                        {expandedCfMod === mod.projectId ? t('cf.collapse') : t('cf.chooseVersion')}
                      </Button>
                    </div>

                    {expandedCfMod === mod.projectId && (
                      <div className={styles.versionListInner}>
                        {cfVersionsState === 'loading' && (
                          <div className={styles.versionMeta}>{t('store.loading')}</div>
                        )}
                        {cfVersionsState === 'ready' && cfVersions.length === 0 && (
                          <div className={styles.versionMeta}>{t('cf.noVersion')}</div>
                        )}
                        {/*
                          失败时把原因写在这里。
                          之前只有一条 toast，列表区留空——用户会以为这个模组没有
                          可下载文件，而实际原因是网络或镜像不可用。
                        */}
                        {cfVersionsState === 'error' && (
                          <div className={styles.versionError} role="alert">
                            <span>{cfVersionsError}</span>
                            {/*
                              没有 key 时给一个直达入口：滚动到页面顶部那张 Key 卡片。
                              这不是「必须修复」的提示——没有 key 平时也能用，只有
                              镜像不可用时才值得换成自己的 key。
                            */}
                            {cfKeyReady === false && (
                              <Button
                                size="sm"
                                variant="ghost"
                                onClick={() => {
                                  document.querySelector('[class*="cfKeySlot"]')
                                    ?.scrollIntoView({ behavior: 'smooth', block: 'center' });
                                }}
                              >
                                {t('cf.openKeySettings')}
                              </Button>
                            )}
                          </div>
                        )}
                        {cfVersions.map(version => (
                          <div key={version.versionId} className={styles.versionRow}>
                            <div className={styles.versionMain}>
                              <div className={styles.versionTitle}>
                                <strong>{version.displayName || version.fileName}</strong>
                              </div>
                              <span className={styles.versionMeta}>
                                {version.fileName}
                                {version.size > 0 ? ` · ${formatBytes(version.size)}` : ''}
                                {version.dateCreated ? ` · ${new Date(version.dateCreated).toLocaleDateString(locale)}` : ''}
                              </span>
                              {version.url === null && (
                                <span className={styles.restricted}>{t('cf.restricted')}</span>
                              )}
                            </div>
                            <div className={styles.versionActions}>
                              <Button
                                size="sm"
                                disabled={version.url === null || installing === version.versionId}
                                icon={<Download size={13} />}
                                onClick={() => void installCfVersion(version)}
                              >
                                {version.url === null
                                  ? t('cf.manual')
                                  : installing === version.versionId ? t('store.loading') : t('store.install')}
                              </Button>
                              <Button
                                size="sm"
                                variant="secondary"
                                disabled={version.url === null || installing === version.versionId}
                                icon={<FolderDown size={13} />}
                                title={t('store.directDownloadHint')}
                                onClick={() => void downloadCfVersion(version)}
                              >
                                {t('store.directDownload')}
                              </Button>
                            </div>
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                ))}
              </div>

              {/*
                翻页。
                只在「有结果」且（不是第一页或还有下一页）时出现：只有一页结果时
                摆一对永远禁用的按钮，等于告诉用户「这里本该有更多」。
              */}
              {modsState === 'ready'
                && (mods.length > 0 || cfMods.length > 0)
                && (modPage > 0 || modHasMore) && (
                  <nav className={styles.pager} aria-label={t('store.pageNavLabel')}>
                    <span className={styles.pagerRange}>
                      {t('store.pageRange', {
                        values: {
                          from: modPage * PAGE_SIZE + 1,
                          // 「到第几项」按本页实际条数算，而不是写死 PAGE_SIZE——
                          // 最后一页通常不满，写死会报一个不存在的位置。
                          to: modPage * PAGE_SIZE + Math.max(mods.length, cfMods.length),
                          /*
                           * 「共 N 项」用 `store.pageRange` 的两种形态表达：
                           * 还有下一页时 N 后面带一个「+」，表示至少这么多。
                           * 不去读两站各自的总数——它们的语义不同，混用会出现
                           * 「最后一页永远有下一页」。
                           */
                          total: `${modPage * PAGE_SIZE + Math.max(mods.length, cfMods.length)}${modHasMore ? '+' : ''}`,
                        },
                      })}
                    </span>
                    <div className={styles.pagerControls}>
                      <button
                        type="button"
                        className={styles.pagerButton}
                        disabled={modPage === 0}
                        onClick={() => goToModPage(modPage - 1)}
                      >
                        <ChevronLeft size={15} />{t('store.pagePrev')}
                      </button>
                      <span className={styles.pagerInfo}>
                        {t('store.pageInfo', { values: { page: modPage + 1, pages: modHasMore ? '…' : modPage + 1 } })}
                      </span>
                      <button
                        type="button"
                        className={styles.pagerButton}
                        disabled={!modHasMore}
                        onClick={() => goToModPage(modPage + 1)}
                      >
                        {t('store.pageNext')}<ChevronRight size={15} />
                      </button>
                    </div>
                  </nav>
                )}
            </div>
          )}

          {/*
            地图固定走 CurseForge：Modrinth 没有 `project_type: world` 这种类型，
            给它一个「Modrinth」选项只会得到必然失败的结果。

            资源包不在这里：Modrinth 也有资源包（`project_type: resourcepack`），
            所以它走上面的合并搜索界面，用户不必为了下资源包先去配 CurseForge key。
            只有在用户主动勾选 CurseForge 时，才走 CurseForgePanel 的完整面板。
          */}
          {category === 'world' && (
            <CurseForgePanel
              kind="world"
              instance={instances.find(item => item.id === targetInstanceId)}
              language={language}
              onNotice={onNotice}
              onChanged={onTasksChanged}
            />
          )}


          {category === 'modpack' && (
            <>
              {/*
                两条获取途径并列：站内搜索下载 与 本地文件导入。
                之前只有后者，用户得先去网站上找到整合包、手动下载、再回来选文件；
                PCL2 的整合包页是直接搜索下载的，本地导入只是它的补充。
                这里把两条路放在同一张卡片里，用户不需要先猜「该用哪个入口」。
              */}
              <div className={styles.filterCard}>
                <div className={styles.filterTitle}>
                  <h2>获取整合包</h2>
                  <span>从 CurseForge 搜索，或导入本地文件</span>
                </div>
                <p className={styles.mixedNote}>
                  两种方式都会创建新实例。从站内下载会先校验 SHA-1 再导入。
                  本地文件支持 Modrinth 的 .mrpack 与 CurseForge 的 .zip；后者会按清单里的
                  文件引用联网查询下载地址（需要 CurseForge Key），查不到或被作者限制分发的
                  文件会被跳过。
                </p>
                <div className={styles.filterGrid}>
                  <label className={`${styles.field} ${styles.fieldWide}`}>
                    <span>{t('store.name')}</span>
                    <div className={styles.fieldInput}>
                      <Search size={15} />
                      <input
                        value={packQuery}
                        onChange={event => setPackQuery(event.target.value)}
                        onKeyDown={event => { if (event.key === 'Enter') void runPackSearch(); }}
                        placeholder="例如 all the mods、better mc"
                      />
                    </div>
                  </label>
                </div>
                <div className={styles.filterActions}>
                  <Button onClick={() => void runPackSearch()} icon={<Search size={15} />}>
                    {t('store.search')}
                  </Button>
                  <Button variant="ghost" onClick={() => void pickPack()} icon={<Package size={15} />}>
                    导入本地文件
                  </Button>
                  {packPath && (
                    <Button
                      variant="ghost"
                      onClick={() => { setPackPath(''); setPackPreview(null); setPackState('idle'); setPackError(''); }}
                    >
                      {t('common.cancel')}
                    </Button>
                  )}
                </div>
                {packPath && <p className={styles.versionMeta} title={packPath}>{packPath}</p>}
              </div>

              {/* 站内搜索结果：与本地导入的「包内容」分开两张卡，避免两类信息混在一起。 */}
              {(packSearchState !== 'idle' || packHits.length > 0) && (
                <div className={styles.resultsCard}>
                  <div className={styles.resultsHeader}>
                    <h2>{t('store.modpackResults')}</h2>
                    <span className={styles.resultsBreakdown}>
                      {packSearchState === 'loading'
                        ? t('store.loading')
                        : packSearchState === 'ready'
                          ? [
                              sources.includes('curseforge') ? `CurseForge ${packHits.length}` : '',
                              `Modrinth ${mrPackHits.length}`,
                            ].filter(Boolean).join(' · ')
                          : '—'}
                    </span>
                  </div>

                  {packSearchState === 'error' && (
                    <div className={styles.errorRow} role="alert"><span>{packSearchError}</span></div>
                  )}

                  {packSearchState === 'ready' && packHits.length === 0 && mrPackHits.length === 0 && (
                    <div className={styles.empty}>{t('store.noModpackResults')}</div>
                  )}

                  <div className={styles.versionList}>
                    {packHits.map(hit => (
                      <div key={hit.projectId}>
                        <div className={styles.contentRow}>
                          {hit.iconUrl
                            ? <img className={styles.blockIcon} src={hit.iconUrl} alt="" width={36} height={36} loading="lazy" />
                            : <span className={styles.versionMeta}>CF</span>}
                          <div className={styles.contentMain}>
                            <div className={styles.contentTitleLine}>
                              <strong>{hit.title}</strong>
                              <span className={`${styles.sourceBadge} ${styles.badgeCurseforge}`}>CurseForge</span>
                            </div>
                            <p>{hit.description || t('store.noSummary')}</p>
                            <span className={styles.contentMeta}>
                              {hit.downloads.toLocaleString(locale)} 次下载
                            </span>
                          </div>
                          <Button
                            size="sm"
                            disabled={installingPack !== ''}
                            onClick={() => void openPackVersions(hit)}
                          >
                            {expandedPack === hit.projectId ? t('cf.collapse') : t('cf.chooseVersion')}
                          </Button>
                        </div>

                        {expandedPack === hit.projectId && (
                          <div className={styles.versionListInner}>
                            {packVersionsState === 'loading' && <div className={styles.versionMeta}>{t('store.loading')}</div>}
                            {packVersionsState === 'ready' && packVersions.length === 0 && (
                              <div className={styles.versionMeta}>这个整合包没有可用文件。</div>
                            )}
                            {/*
                              按游戏版本分组展示（参考 PCL2 详情页的多卡片结构）。
                              一个成熟的整合包常有几十个文件，平铺列表里找「我这个版本该用
                              哪个」非常费劲；分组后每组标题直接就是版本号。
                            */}
                            {groupedPackVersions.map(([versionKey, files]) => (
                              <div key={versionKey} className={styles.packVersionGroup}>
                                <div className={styles.packVersionGroupTitle}>
                                  {versionKey === ''
                                    ? t('store.packVersionUnknown')
                                    : `Minecraft ${versionKey}`}
                                  <span className={styles.packVersionGroupCount}>{files.length}</span>
                                </div>
                                {files.map(file => (
                                  <div key={file.versionId} className={styles.versionRow}>
                                    {/*
                                      版本行的信息结构参考 PCL2 的 ResourceVersion.ToListItem：
                                      一行里给全「用什么加载器 / 多大 / 什么时候发的」，而不是
                                      只写文件名。游戏版本已经由分组标题表达，行内不再重复。
                                    */}
                                    <div className={styles.versionMain}>
                                      <div className={styles.versionTitle}>
                                        <strong>{file.displayName || file.fileName}</strong>
                                      </div>
                                      <span className={styles.versionMeta}>
                                        {[
                                          file.loaders.length > 0 ? file.loaders.join('、') : '',
                                          file.size > 0 ? formatBytes(file.size) : '',
                                          file.dateCreated ? new Date(file.dateCreated).toLocaleDateString(locale) : '',
                                        ].filter(Boolean).join(' · ')}
                                      </span>
                                      {/* 受限整合包：作者禁止第三方分发，只能给网页入口。 */}
                                      {file.url === null && (
                                        <span className={styles.restricted}>{t('cf.restricted')}</span>
                                      )}
                                    </div>
                                    <div className={styles.versionActions}>
                                      {/*
                                        两个动作对应 PCL2 的 ToListItem：主按钮 = 安装，
                                        次按钮 = 另存为。
                                        「只下载不导入」是真实需求——用户可能想先备份、或放进
                                        别的启动器。之前整合包只有「下载并导入」一条路，想留个
                                        原包就得先导入再导出。
                                      */}
                                      <Button
                                        size="sm"
                                        variant="secondary"
                                        disabled={file.url === null || installingPack !== ''}
                                        title={t('store.directDownloadHint')}
                                        onClick={() => void savePack(file)}
                                      >
                                        {t('store.saveAs')}
                                      </Button>
                                      <Button
                                        size="sm"
                                        disabled={file.url === null || installingPack !== ''}
                                        icon={<Download size={13} />}
                                        onClick={() => void installPack(file)}
                                      >
                                        {file.url === null
                                          ? t('cf.manual')
                                          : installingPack === file.versionId ? t('store.loading') : t('store.downloadAndImport')}
                                      </Button>
                                    </div>
                                  </div>
                                ))}
                              </div>
                            ))}
                          </div>
                        )}
                      </div>
                    ))}

                    {/*
                      Modrinth 整合包。
                      与上面的 CurseForge 条目分开渲染，因为两者的数据形状不同：
                      Modrinth 的下载量字段是 `downloads`、文件列表走
                      `listProjectVersions`（不按实例过滤），CurseForge 走
                      `curseforgeFiles`。合并成一个数组要先把两边归一化成第三种
                      形状，那比多一段渲染更差。
                    */}
                    {mrPackHits.map(hit => (
                      <div key={`mr-${hit.projectId}`}>
                        <div className={styles.contentRow}>
                          {hit.iconUrl
                            ? <img className={styles.blockIcon} src={hit.iconUrl} alt="" width={36} height={36} loading="lazy" />
                            : <span className={styles.versionMeta}>MR</span>}
                          <div className={styles.contentMain}>
                            <div className={styles.contentTitleLine}>
                              <strong>{hit.title}</strong>
                              <span className={`${styles.sourceBadge} ${styles.badgeModrinth}`}>Modrinth</span>
                            </div>
                            <p>{hit.description || t('store.noSummary')}</p>
                            <span className={styles.contentMeta}>
                              {t('store.downloadsShort', {
                                values: { value: hit.downloads.toLocaleString(locale) },
                                count: hit.downloads,
                              })}
                              {hit.author ? ` · ${hit.author}` : ''}
                            </span>
                          </div>
                          <Button
                            size="sm"
                            disabled={installingPack !== ''}
                            onClick={() => void openMrPackVersions(hit)}
                          >
                            {expandedMrPack === hit.projectId ? t('cf.collapse') : t('cf.chooseVersion')}
                          </Button>
                        </div>

                        {expandedMrPack === hit.projectId && (
                          <div className={styles.versionListInner}>
                            {mrPackVersionsState === 'loading' && (
                              <div className={styles.versionMeta}>{t('store.loading')}</div>
                            )}
                            {mrPackVersionsState === 'ready' && mrPackVersions.length === 0 && (
                              <div className={styles.versionMeta}>{t('cf.noVersion')}</div>
                            )}
                            {/*
                              只渲染前 30 个版本。
                              一个成熟整合包常有上百个历史版本（Fabulously Optimized
                              实测 100 个），全铺开会把页面撑成几千像素，而用户真正
                              想装的是最近那几个。超出的部分明确说明，而不是静默截断——
                              静默截断会让人以为「这个包没有我要的版本」。
                            */}
                            {mrPackVersions.slice(0, MR_PACK_VERSION_LIMIT).map(version => (
                              <div key={version.versionId} className={styles.versionRow}>
                                <div className={styles.versionMain}>
                                  <div className={styles.versionTitle}>
                                    <strong>{version.versionNumber || version.fileName}</strong>
                                  </div>
                                  <span className={styles.versionMeta}>
                                    {[
                                      version.fileName,
                                      version.size > 0 ? formatBytes(version.size) : '',
                                      version.dateCreated
                                        ? new Date(version.dateCreated).toLocaleDateString(locale)
                                        : '',
                                    ].filter(Boolean).join(' · ')}
                                  </span>
                                </div>
                                <div className={styles.versionActions}>
                                  <Button
                                    size="sm"
                                    disabled={installingPack !== ''}
                                    icon={<Download size={13} />}
                                    onClick={() => void installMrPack(version)}
                                  >
                                    {installingPack === version.versionId
                                      ? t('store.loading')
                                      : t('store.downloadAndImport')}
                                  </Button>
                                </div>
                              </div>
                            ))}
                            {mrPackVersions.length > MR_PACK_VERSION_LIMIT && (
                              <div className={styles.versionMeta}>
                                {t('store.moreVersions', {
                                  count: mrPackVersions.length - MR_PACK_VERSION_LIMIT,
                                  values: { count: mrPackVersions.length - MR_PACK_VERSION_LIMIT },
                                })}
                              </div>
                            )}
                          </div>
                        )}
                      </div>
                    ))}
                  </div>

                  {/*
                    翻页。
                    与模组页同一套：只在「有结果」且（不在第一页或还有下一页）时出现。
                    整合包的列表页数通常比模组少，但按下载量浏览时同样需要往后翻。
                  */}
                  {packSearchState === 'ready'
                    && (packHits.length > 0 || mrPackHits.length > 0)
                    && (packPage > 0 || packHasMore) && (
                      <nav className={styles.pager} aria-label={t('store.pageNavLabel')}>
                        <span className={styles.pagerRange}>
                          {t('store.pageRange', {
                            values: {
                              from: packPage * PAGE_SIZE + 1,
                              to: packPage * PAGE_SIZE + Math.max(packHits.length, mrPackHits.length),
                              total: `${packPage * PAGE_SIZE + Math.max(packHits.length, mrPackHits.length)}${packHasMore ? '+' : ''}`,
                            },
                          })}
                        </span>
                        <div className={styles.pagerControls}>
                          <button
                            type="button"
                            disabled={packPage === 0}
                            onClick={() => void runPackSearch(packPage - 1)}
                          >
                            {t('store.pagePrev')}
                          </button>
                          <button
                            type="button"
                            disabled={!packHasMore}
                            onClick={() => void runPackSearch(packPage + 1)}
                          >
                            {t('store.pageNext')}
                          </button>
                        </div>
                      </nav>
                    )}
                </div>
              )}

              <div className={styles.resultsCard}>
                <div className={styles.resultsHeader}>
                  <h2>包内容</h2>
                </div>

                {packState === 'error' && (
                  <div className={styles.errorRow} role="alert">
                    <span>{packError}</span>
                  </div>
                )}

                {packPreview && (
                  <>
                    <div className={styles.versionList}>
                      <div className={styles.versionRow}>
                        <BlockIcon {...VANILLA_BLOCK} size={36} />
                        <div className={styles.versionMain}>
                          <div className={styles.versionTitle}>
                            <strong>{packPreview.name}</strong>
                            <span className={styles.kindBadge}>{packPreview.version}</span>
                          </div>
                          <span className={styles.versionMeta}>
                            Minecraft {packPreview.minecraft} · {packPreview.loader === 'vanilla' ? t('app.vanilla') : packPreview.loader}
                          </span>
                        </div>
                      </div>
                    </div>
                    <div className={styles.resultsMeta}>
                      <span>
                        必须文件 {packPreview.requiredFiles} · 可选 {packPreview.optionalFiles} · 覆盖 {packPreview.overrideFiles} · 合计 {formatBytes(packPreview.totalBytes)}
                      </span>
                    </div>
                    <div className={styles.filterActions}>
                      <Button
                        disabled={importing}
                        icon={<Download size={15} />}
                        onClick={() => void doImport()}
                      >
                        {importing ? t('store.loading') : '确认并导入'}
                      </Button>
                    </div>
                  </>
                )}

                {packState === 'idle' && !packPreview && (
                  <div className={styles.empty}>先选择一个整合包文件。</div>
                )}
              </div>
            </>
          )}
        </section>
      </div>
    </div>
  );
}

export default StoreWorkspace;
