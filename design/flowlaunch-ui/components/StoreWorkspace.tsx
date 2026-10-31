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
import { VersionPicker } from './VersionPicker';
import { InstallPlan } from './InstallPlan';
import { StoreLogo } from './StoreLogo';
import { blockForVersion, VANILLA_BLOCK } from '../utils/store/blocks';
import { useI18n } from '../utils/i18n';
import type { MessageKey } from '../utils/i18n/messages/zh-CN';
import { kindOfVersion, VERSION_KINDS, VERSION_KIND_KEYS } from '../utils/store/format';
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

const PAGE_SIZE = 20;
/** 与后端 `MAX_RESULTS` 一致：一次搜索最多返回 20 条。 */
const SEARCH_LIMIT = 20;
void SEARCH_LIMIT;

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
  const [modsState, setModsState] = useState<'idle' | 'loading' | 'ready' | 'error'>('idle');
  const [modsError, setModsError] = useState('');
  /** 当前展开版本列表的模组。同一时刻只展开一个，避免整 page 变成版本墙。 */
  const [expandedMod, setExpandedMod] = useState<string | null>(null);
  const [modVersions, setModVersions] = useState<CatalogVersion[]>([]);
  const [modVersionsState, setModVersionsState] = useState<'idle' | 'loading' | 'ready' | 'error'>('idle');
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
  const [sources, setSources] = useState<Array<'modrinth' | 'curseforge'>>(['modrinth']);

  /** CurseForge 是否已配置 key，决定勾选它时是直接搜还是先给设置入口。 */
  const [cfKeyReady, setCfKeyReady] = useState<boolean | null>(null);

  /**
   * 用户是否手动改过来源勾选。
   *
   * 用来判断「自动勾上 CurseForge」还能不能执行：用户一旦自己取消过，就说明他
   * 明确不想要这个来源，之后 key 状态怎么变化都不该再替他勾回去。
   */
  const sourcesTouched = useRef(false);

  /*
   * key 状态查回来后，若 CurseForge 可用且用户没手动改过，自动勾上它。
   *
   * 内置 key 或用户已配 key 时两个来源都真的能用，没有理由让人先手动勾一次。
   */
  useEffect(() => {
    if (cfKeyReady !== true || sourcesTouched.current) return;
    setSources(current => (current.includes('curseforge') ? current : [...current, 'curseforge']));
  }, [cfKeyReady]);

  const toggleSource = (source: 'modrinth' | 'curseforge') => {
    sourcesTouched.current = true;
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
  const [packSearchState, setPackSearchState] = useState<'idle' | 'loading' | 'ready' | 'error'>('idle');
  const [packSearchError, setPackSearchError] = useState('');
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

  // 切分类要回到第一页：原版的页码和模组搜索的页码语义完全不同。
  useEffect(() => { setPage(0); }, [category]);

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
  const runModSearch = useCallback(async (queryOverride?: string) => {
    if (!targetInstanceId) {
      /*
       * 没有目标实例时必须清空旧结果。
       *
       * 之前只设了错误、没清列表：用户在下拉里把实例清空后，界面上同时出现
       * 「上一次的搜索结果」和「请先选择一个实例」——结果区和错误提示互相矛盾，
       * 看起来像是数据没刷新。清空后点击「重试」的语义也才对得上。
       */
      setMods([]);
      setCfMods([]);
      setModsState('error');
      setModsError('请先选择一个实例：模组搜索需要按它的游戏版本与加载器筛选。');
      return;
    }
    if (sources.length === 0) {
      setMods([]);
      setCfMods([]);
      setModsState('error');
      setModsError(t('store.noSourceSelected'));
      return;
    }
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
      gameVersion: modVersion.trim() || undefined,
      loader: modLoader === 'any' ? 'any' : (modLoader.trim() || undefined),
      category: modCategory,
      index: sort,
    };

    const wantsModrinth = sources.includes('modrinth');
    const wantsCurseForge = sources.includes('curseforge');

    const [modrinthResult, curseForgeResult] = await Promise.allSettled([
      wantsModrinth ? searchMods(targetInstanceId, request) : Promise.resolve([]),
      /*
       * CurseForge 现在也支持浏览模式：不传 searchFilter + sortField=6 会返回
       * 该版本/加载器下按总下载量排序的列表（已在真实接口上验证）。
       * 服务端排序是近似降序，所以拿到的这一页在下面会再按下载量排一次。
       */
      wantsCurseForge
        ? curseforgeSearch({
            kind: projectType === 'resourcepack' ? 'resourcepack' : 'mod',
            query: trimmed,
            gameVersion: modVersion.trim() || target?.version,
            // 资源包没有加载器维度，传了会被服务端当成额外的筛选条件而漏结果。
            loader: projectType === 'resourcepack'
              ? undefined
              : (modLoader === 'any' ? undefined : (modLoader.trim() || target?.loader)),
            // 第一页足够：合并视图不做深翻，两站的页码语义不同，深翻会让
            // 「第 3 页」变成两套不同偏移量的混合体。
            index: 0,
            pageSize: 20,
          })
        : Promise.resolve([]),
    ]);

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

    setModsState('ready');
    // 只在一个来源都拿不到时算整体失败；部分失败在各自栏里说明。
    const attempted = [wantsModrinth, wantsCurseForge].filter(Boolean).length;
    if (attempted > 0 && errors.length === attempted) {
      setModsError(errors.join(' '));
      setModsState('error');
    }
  }, [modCategory, modLoader, modSort, modVersion, sources, submittedQuery, t, target, targetInstanceId]);

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
    if (!targetInstanceId) return;
    void searchRef.current();
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
    if (!MODRINTH_CATEGORIES.includes(category) || !targetInstanceId) return;
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
    void searchRef.current();
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
    prevFilters.current = {
      modCategory: '',
      modVersion: '',
      modLoader: '',
      modSort: 'downloads',
      // 重置不动来源，所以这里沿用当前值，避免被误判成「来源变了」多搜一次。
      sourcesKey,
    };
    void searchRef.current('');
  };

  const openModVersions = async (mod: CatalogMod) => {
    if (expandedMod === mod.projectId) { setExpandedMod(null); return; }
    setExpandedMod(mod.projectId);
    setModVersionsState('loading');
    setModVersions([]);
    try {
      // 透传当前分类的项目类型：资源包不能按加载器筛，否则拿到的永远是空列表。
      setModVersions(await listModVersions(targetInstanceId, mod.projectId, projectTypeOf(category)));
      setModVersionsState('ready');
    } catch (error) {
      setModVersionsState('error');
      setModsError(errorText(error, '无法读取该模组的版本列表。'));
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
    if (!target) return;
    setExpandedCfMod(mod.projectId);
    setCfVersionsState('loading');
    setCfVersions([]);
    try {
      setCfVersions(await curseforgeFiles(mod.projectId, target.version, target.loader));
      setCfVersionsState('ready');
    } catch (error) {
      setCfVersionsState('error');
      onNotice(errorText(error, t('cf.versionsFailed')));
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
    // 传入当前输入，避免闭包里读到的还是上一次的提交值。
    void runModSearch(query);
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
  const runPackSearch = async () => {
    setPackSearchState('loading');
    setPackSearchError('');
    setPackHits([]);
    setExpandedPack(null);
    try {
      const hits = await curseforgeSearch({
        kind: 'modpack',
        query: packQuery.trim(),
        // 整合包跨版本可用，不按实例的版本/加载器收窄——否则会漏掉大量实际能装的包。
        index: 0,
        pageSize: 20,
      });
      // 服务端排序是近似降序，本地再排一次，避免「第 4 名下载量比第 3 名高」。
      setPackHits([...hits].sort((a, b) => b.downloads - a.downloads));
      setPackSearchState('ready');
    } catch (error) {
      setPackHits([]);
      setPackSearchError(errorText(error, t('cf.searchFailed')));
      setPackSearchState('error');
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
                >
                  {/*
                    没有实例时才给一个空占位项。
                    之前这个 `<option value="">—</option>` 一直存在，用户可以直接
                    选中它，把目标实例清成空——之后左侧显示「—」、搜索却还留着上
                    一次的结果并报「找不到所选实例」，自相矛盾。把「无实例」表达成
                    一个 disabled 占位项，而不是一个可选择的值。
                  */}
                  {instances.length === 0 && (
                    <option value="" disabled>{t('store.noInstances')}</option>
                  )}
                  {instances.map(instance => (
                    <option key={instance.id} value={instance.id}>
                      {instance.name} · {instance.version}
                    </option>
                  ))}
                </select>
                {category === 'mod' && target && target.loader === 'vanilla' && (
                  <p className={styles.needKey}>
                    该实例是原版；模组目录需要 Fabric 或 Quilt 实例。
                  </p>
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

              {sources.includes('curseforge') && cfKeyReady === false && (
                <div className={styles.cfKeySlot}>
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

                    {expandedMod === mod.projectId && (
                      <div className={styles.versionListInner}>
                        {modVersionsState === 'loading' && (
                          <div className={styles.versionMeta}>{t('store.loading')}</div>
                        )}
                        {modVersionsState === 'ready' && modVersions.length === 0 && (
                          <div className={styles.versionMeta}>{t('cf.noVersion')}</div>
                        )}
                        {modVersions.map(version => (
                          <div key={version.versionId} className={styles.versionRow}>
                            <Package size={22} className={styles.logoPlaceholder} />
                            <div className={styles.versionMain}>
                              <div className={styles.versionTitle}>
                                <strong>{version.versionNumber}</strong>
                              </div>
                              <span className={styles.versionMeta}>
                                {version.fileName} · {formatBytes(version.size)} ·{' '}
                                {new Date(version.dateCreated).toLocaleDateString(locale)}
                              </span>
                            </div>
                            <div className={styles.versionActions}>
                              <Button
                                size="sm"
                                disabled={installing === version.versionId}
                                icon={<Download size={13} />}
                                onClick={() => void installVersion(version)}
                              >
                                {installing === version.versionId ? t('store.loading') : t('store.install')}
                              </Button>
                              <Button
                                size="sm"
                                variant="secondary"
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
                      </div>
                    )}
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
                    <h2>CurseForge 整合包</h2>
                    <span>
                      {packSearchState === 'loading'
                        ? t('store.loading')
                        : packSearchState === 'ready'
                          ? t('store.resultCount', { count: packHits.length })
                          : '—'}
                    </span>
                  </div>

                  {packSearchState === 'error' && (
                    <div className={styles.errorRow} role="alert"><span>{packSearchError}</span></div>
                  )}

                  {packSearchState === 'ready' && packHits.length === 0 && (
                    <div className={styles.empty}>没有找到匹配的整合包。</div>
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
                  </div>
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
