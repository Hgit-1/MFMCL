// 内容下载工作区：真实数据源版。
//
// 三个来源各管一摊（PCL2 里也是分开的：原版走 version_manifest，Mod/整合包
// 走 CurseForge 或 Modrinth）：
//
//   原版    Mojang version_manifest_v2.json —— 一次拉全量，本地筛选/排序
//   Mod 等  Modrinth v2/search + CurseForge v1/mods/search —— 按请求分页
//
// 交互约定：
//   - 搜索是**用户触发**的（点搜索、回车、切排序），不做输入即请求；
//     输入本身只做 350ms 防抖，避免每敲一个字都打一次 API。
//   - 两个源并发请求，一个失败不影响另一个；错误按源分行显示。
//   - 合并排序只在「两边都有真实值」的键上生效，其余标为混合排序。
//
// 全部文案都走词典：分类名、版本种类、排序键来自 `category.*` / `versionKind.*`
// / `store.sort*`，数据源抛出的错误则带的是词典键（见 utils/i18n/localized.ts），
// 所以切换语言后连已显示的错误也会跟着变。

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ArrowDownWideNarrow, ArrowUpNarrowWide, ChevronLeft, ChevronRight, KeyRound, RefreshCw, Search } from 'lucide-react';
import { Button } from './Button';
import { BlockIcon } from './BlockIcon';
import { StoreLogo } from './StoreLogo';
import { blockForVersion, VANILLA_BLOCK } from '../utils/store/blocks';
import { readCurseForgeKey, saveCurseForgeKey, searchCurseForge } from '../utils/store/curseforge';
import { formatCount, formatDate, formatRelative, VERSION_KIND_KEYS } from '../utils/store/format';
import { loc, localizedTextOf, renderLocalized, type Localized } from '../utils/i18n/localized';
import { useI18n } from '../utils/i18n';
import type { MessageKey } from '../utils/i18n/messages/zh-CN';
import { isMixed, mergeResults, sortEntries } from '../utils/store/sort';
import { clampPage, mergedPageCount, PAGE_SIZE, slicePage } from '../utils/store/pagination';
import { searchModrinth } from '../utils/store/modrinth';
import { fetchManifest, queryVersions, releaseOptions } from '../utils/store/vanilla';
import type {
  SortDirection,
  SortKey,
  StoreCategory,
  StoreEntry,
  StoreSource,
  VersionKind,
} from '../utils/store/types';
import type { DownloadTask, GameInstance } from '../types';
import styles from './StoreWorkspace.module.css';

/** 分类只保留 id；显示名统一走 t('category.*')。 */
export const STORE_CATEGORIES: StoreCategory[] = ['vanilla', 'mod', 'modpack', 'resourcepack', 'world'];

const VERSION_KINDS: VersionKind[] = ['release', 'snapshot', 'old', 'special'];

const SORT_LABEL_KEYS: Record<SortKey, MessageKey> = {
  popular: 'store.sortPopular',
  downloads: 'store.sortDownloads',
  follows: 'store.sortFollows',
  newest: 'store.sortNewest',
  updated: 'store.sortUpdated',
  name: 'store.sortName',
};

const MOD_SORT_KEYS: SortKey[] = ['popular', 'downloads', 'follows', 'newest', 'updated', 'name'];
const VANILLA_SORT_KEYS: SortKey[] = ['newest', 'name'];

/**
 * 每个分类的默认排序。
 *
 * 原版清单只有「发布时间 / 名称」两个可用键（Mojang 不提供下载量），其余分类
 * 默认「热度」。之所以要按分类区分，是因为这些键不是共通的：从原版切到整合包
 * 时必须回到热度，而不是把原版的「发布时间」继承过去。
 */
const DEFAULT_SORT: Record<StoreCategory, SortKey> = {
  vanilla: 'newest',
  mod: 'popular',
  modpack: 'popular',
  resourcepack: 'popular',
  world: 'popular',
};

const DEFAULT_DIRECTION: SortDirection = 'desc';



interface StoreWorkspaceProps {
  category: StoreCategory;
  instances: GameInstance[];
  selectedInstance?: GameInstance;
  downloadTasks: DownloadTask[];
  onSelectInstance: (instanceId: string) => void;
  onQueueDownload: (name: string, type: DownloadTask['type'], targetInstance?: string) => void;
}

type LoadState = 'idle' | 'loading' | 'ready' | 'error';

interface SourceResult {
  state: LoadState;
  entries: StoreEntry[];
  /** 错误保持「键 + 参数」形式，渲染时才落到当前语言。 */
  error?: Localized;
  actionable?: boolean;
  /** 该源报告的总条数，用于算总页数。 */
  total?: number;
}

const EMPTY_RESULT: SourceResult = { state: 'idle', entries: [] };

/** 把抛出的异常折成可渲染的文案：优先用数据源给出的键，否则用兜底键。 */
function errorText(error: unknown, fallback: MessageKey): Localized {
  return localizedTextOf(error) ?? loc(fallback);
}

/**
 * 在一条译文里嵌入链接。
 *
 * 词典是纯字符串，不能直接放 JSX，所以约定用占位符表示链接位置：这里把渲染
 * 好的文案切开，中间插入 <a>。这样词序仍由各语言自己决定（日语把链接放在句
 * 末、英语放在句中都能正常工作），而不是把链接硬编码在句子中间。
 */
const LINK_SLOT = '\u0001';

function withLink(text: string, link: React.ReactNode): React.ReactNode {
  const [before, after = ''] = text.split(LINK_SLOT);
  return <>{before}{link}{after}</>;
}

export function StoreWorkspace({
  category,
  instances,
  selectedInstance,
  downloadTasks,
  onSelectInstance,
  onQueueDownload,
}: StoreWorkspaceProps) {
  const { t, language } = useI18n();
  const [query, setQuery] = useState('');
  const [submittedQuery, setSubmittedQuery] = useState('');
  const [versionKinds, setVersionKinds] = useState<VersionKind[]>(['release']);
  const [sources, setSources] = useState<StoreSource[]>(['Modrinth', 'CurseForge']);
  const [gameVersion, setGameVersion] = useState('');
  const [sortKey, setSortKey] = useState<SortKey>(() => DEFAULT_SORT[category]);
  const [sortDirection, setSortDirection] = useState<SortDirection>(DEFAULT_DIRECTION);
  /**
   * 每个分类上一次选过的排序。
   *
   * 单独记一份而不是共用一个 state：各分类可用的排序键不同（原版没有热度），
   * 共用一个值会让「原版的发布时间」泄漏到整合包上——那正是默认显示不正确的
   * 原因。这里记住每个分类自己的选择，切回去时还原用户的偏好。
   */
  const sortMemoryRef = useRef<Partial<Record<StoreCategory, { key: SortKey; direction: SortDirection }>>>({});
  const [targetInstanceId, setTargetInstanceId] = useState(selectedInstance?.id ?? '');
  /** 当前页（从 0 开始）。换搜索条件、换分类、换排序时都回到第一页。 */
  const [page, setPage] = useState(0);

  const [vanilla, setVanilla] = useState<SourceResult>(EMPTY_RESULT);
  const [modrinth, setModrinth] = useState<SourceResult>(EMPTY_RESULT);
  const [curseforge, setCurseforge] = useState<SourceResult>(EMPTY_RESULT);

  const [keyOpen, setKeyOpen] = useState(false);
  const [keyDraft, setKeyDraft] = useState(() => readCurseForgeKey());
  const [hasKey, setHasKey] = useState(() => Boolean(readCurseForgeKey()));

  const abortRef = useRef<AbortController | null>(null);
  const debounceRef = useRef<number | null>(null);
  /** 各源最近一次报告的总条数，用来判断是否已经翻到底（不进依赖数组）。 */
  const totalsRef = useRef<{ modrinth?: number; curseforge?: number }>({});
  /** 结果面板的滚动容器，翻页后要滚回顶部。 */
  const panelRef = useRef<HTMLElement | null>(null);

  const categoryLabel = t(`category.${category}`);
  const versionKindLabel = (kind: VersionKind) => t(VERSION_KIND_KEYS[kind]);

  useEffect(() => { setTargetInstanceId(selectedInstance?.id ?? ''); }, [selectedInstance?.id]);

  // 分类由父组件（侧栏菜单）控制，切换后必须回到第一页：原版和 Mod 的页码
  // 语义完全不同，停在旧页码会直接看到空列表。
  useEffect(() => { setPage(0); }, [category]);

  // 切分类时切换排序：优先还原该分类上次的选择，没有记录就用它的默认值。
  // 原版没有「热度」，所以必须回落到「发布时间」；反向也必须处理——否则从
  // 原版切到整合包时会带着「发布时间」，用户看到的默认排序就不是热度了。
  useEffect(() => {
    const remembered = sortMemoryRef.current[category];
    const nextKey = remembered?.key ?? DEFAULT_SORT[category];
    const allowed = category === 'vanilla' ? VANILLA_SORT_KEYS : MOD_SORT_KEYS;
    setSortKey(allowed.includes(nextKey) ? nextKey : DEFAULT_SORT[category]);
    setSortDirection(remembered?.direction ?? DEFAULT_DIRECTION);
  }, [category]);

  // 用户主动改排序时记到这个分类名下，切走再切回时能还原。
  const changeSort = (key: SortKey) => {
    setSortKey(key);
    sortMemoryRef.current[category] = { key, direction: sortDirection };
    setPage(0);
  };

  const changeSortDirection = (direction: SortDirection) => {
    setSortDirection(direction);
    sortMemoryRef.current[category] = { key: sortKey, direction };
    setPage(0);
  };

  // ---------------------------------------------------------------------
  // Mojang 清单：任何时候都需要（原版列表 + 游戏版本下拉）
  // ---------------------------------------------------------------------
  const loadManifest = useCallback(async (fresh = false) => {
    setVanilla(current => ({ ...current, state: 'loading', error: undefined }));
    try {
      const { entries } = await fetchManifest(undefined, fresh);
      setVanilla({ state: 'ready', entries, total: entries.length });
    } catch (error) {
      if (error instanceof DOMException && error.name === 'AbortError') return;
      setVanilla({ state: 'error', entries: [], error: errorText(error, 'store.manifestFailed') });
    }
  }, []);

  useEffect(() => { void loadManifest(); }, [loadManifest]);

  // ---------------------------------------------------------------------
  // 第三方源搜索
  // ---------------------------------------------------------------------
  const runSearch = useCallback(async (options: { fresh?: boolean } = {}) => {
    if (category === 'vanilla') return;

    abortRef.current?.abort();
    const controller = new AbortController();
    abortRef.current = controller;

    const wantsModrinth = sources.includes('Modrinth');
    const wantsCurseForge = sources.includes('CurseForge');
    const offset = page * PAGE_SIZE;
    const shared = {
      category,
      query: submittedQuery,
      gameVersion: gameVersion || undefined,
      sort: sortKey,
      direction: sortDirection,
      limit: PAGE_SIZE,
      offset,
      signal: controller.signal,
      fresh: options.fresh,
    };

    const tasks: Promise<void>[] = [];

    if (wantsModrinth) {
      // 该源已经翻到底了就不再请求：offset 超过 total 只会拿到空数组，白占一次
      // API 配额（Modrinth 有 300 次/分钟的限流）。总数存在 ref 里，避免把
      // modrinth 放进依赖数组——那会让每次结果更新都触发一次新搜索。
      const known = totalsRef.current.modrinth;
      if (page > 0 && known !== undefined && offset >= known) {
        setModrinth({ state: 'ready', entries: [], total: known });
      } else {
        // 清空旧条目：否则加载期间会显示上一页的行，而序号标签已经是新页的了。
        setModrinth(current => ({ state: 'loading', entries: [], total: current.total }));
        tasks.push(searchModrinth(shared).then(
          pageResult => {
            if (controller.signal.aborted) return;
            totalsRef.current.modrinth = pageResult.total;
            setModrinth({ state: 'ready', entries: pageResult.entries, total: pageResult.total });
          },
          error => {
            if (controller.signal.aborted || (error instanceof DOMException && error.name === 'AbortError')) return;
            setModrinth({
              state: 'error',
              entries: [],
              error: errorText(error, 'store.modrinthUnavailable'),
              actionable: Boolean(error && typeof error === 'object' && 'actionable' in error && error.actionable),
            });
          },
        ));
      }
    } else {
      setModrinth(EMPTY_RESULT);
      totalsRef.current.modrinth = undefined;
    }

    if (wantsCurseForge) {
      if (!readCurseForgeKey()) {
        // 缺 key 不是「失败」，是一个需要用户操作的待办状态。
        setCurseforge({
          state: 'error',
          entries: [],
          error: loc('store.curseforgeNeedsKey'),
          actionable: true,
        });
      } else {
        const known = totalsRef.current.curseforge;
        // 与 Modrinth 同理：已知翻到底就不再发空请求。
        if (page > 0 && known !== undefined && offset >= known) {
          setCurseforge({ state: 'ready', entries: [], total: known });
        } else {
          setCurseforge(current => ({ state: 'loading', entries: [], total: current.total }));
          tasks.push(searchCurseForge(shared).then(
            pageResult => {
              if (controller.signal.aborted) return;
              totalsRef.current.curseforge = pageResult.total;
              setCurseforge({ state: 'ready', entries: pageResult.entries, total: pageResult.total });
            },
            error => {
              if (controller.signal.aborted || (error instanceof DOMException && error.name === 'AbortError')) return;
              setCurseforge({
                state: 'error',
                entries: [],
                error: errorText(error, 'store.curseforgeUnavailable'),
                actionable: Boolean(error && typeof error === 'object' && 'actionable' in error && error.actionable),
              });
            },
          ));
        }
      }
    } else {
      setCurseforge(EMPTY_RESULT);
      totalsRef.current.curseforge = undefined;
    }

    await Promise.all(tasks);
  }, [category, gameVersion, page, sortDirection, sortKey, sources, submittedQuery]);

  // 分类 / 来源 / 排序 / 已提交查询变化时重新搜索。
  useEffect(() => {
    if (category === 'vanilla') {
      abortRef.current?.abort();
      return;
    }
    void runSearch();
    return () => { abortRef.current?.abort(); };
  }, [category, runSearch]);

  // 卸载时取消在途请求。
  useEffect(() => () => { abortRef.current?.abort(); }, []);

  // 输入防抖：只更新「已提交」的查询，真正的请求由上面的 effect 发起。
  const onQueryChange = (value: string) => {
    setQuery(value);
    if (debounceRef.current) window.clearTimeout(debounceRef.current);
    debounceRef.current = window.setTimeout(() => {
      setSubmittedQuery(value);
      // 换了关键词就从第一页开始，否则会停在旧查询的深页码上。
      setPage(0);
    }, 350);
  };

  const toggleVersionKind = (kind: VersionKind) => {
    setVersionKinds(current => current.includes(kind) ? current.filter(item => item !== kind) : [...current, kind]);
    setPage(0);
  };

  const toggleSource = (source: StoreSource) => {
    if (category === 'vanilla') return;
    setSources(current => current.includes(source) ? current.filter(item => item !== source) : [...current, source]);
    setPage(0);
  };

  const resetFilters = () => {
    setQuery('');
    setSubmittedQuery('');
    setGameVersion('');
    setVersionKinds(['release']);
    setSources(['Modrinth', 'CurseForge']);
    setSortKey(DEFAULT_SORT[category]);
    setSortDirection(DEFAULT_DIRECTION);
    // 重置条件意味着「这个分类回到初始状态」，所以连记住的排序一起清掉。
    delete sortMemoryRef.current[category];
    setPage(0);
  };

  // ---------------------------------------------------------------------
  // 结果
  // ---------------------------------------------------------------------
  const releaseList = useMemo(() => (vanilla.state === 'ready' ? releaseOptions(vanilla.entries) : []), [vanilla]);

  /** 原版清单是一次性拉全量的，所以筛选/排序在本地做完，再按页切片。 */
  const vanillaEntries = useMemo(() => {
    if (vanilla.state !== 'ready') return [];
    const filtered = queryVersions(vanilla.entries, { kinds: versionKinds, query: submittedQuery, language });
    return sortEntries(filtered.entries, sortKey === 'name' ? 'name' : 'newest', sortDirection, language);
  }, [vanilla, versionKinds, submittedQuery, sortKey, sortDirection, language]);

  /**
   * 原版列表在本地分页（清单是全量的）；第三方源由 API 按 offset 分页，
   * results 已经是当前页。
   */
  const vanillaSlice = useMemo(() => slicePage(vanillaEntries, page), [vanillaEntries, page]);

  const results = useMemo(() => {
    if (category === 'vanilla') return vanillaSlice.entries;
    const groups: { source: string; entries: StoreEntry[] }[] = [];
    if (modrinth.state === 'ready') groups.push({ source: 'Modrinth', entries: modrinth.entries });
    if (curseforge.state === 'ready') groups.push({ source: 'CurseForge', entries: curseforge.entries });
    return mergeResults(groups, sortKey, sortDirection, language);
  }, [category, vanillaSlice, modrinth, curseforge, sortKey, sortDirection, language]);

  /**
   * 总页数。
   * 原版：由本地筛选结果算出。第三方源：取各源报告的 total 中较大者，因为用户
   * 看到的是合并列表，页数必须覆盖条目更多的那个源，否则另一个源的可翻条目会被
   * 截断。上限 MAX_PAGE，避免用户一直点到几十万页之外。
   */
  const pageCount = useMemo(() => {
    if (category === 'vanilla') return vanillaSlice.pages;
    return mergedPageCount([modrinth.total, curseforge.total]);
  }, [category, modrinth.total, curseforge.total, vanillaSlice.pages]);

  /**
   * 当前页在整体结果中的序号范围，用于「第 21–40 项，共 920 项」。
   * 原版用本地切片的精确值；第三方源用 API 报告的 total（本页条数可能不足一页）。
   */
  const totalShown = category === 'vanilla'
    ? vanillaSlice.total
    : Math.max(modrinth.total ?? 0, curseforge.total ?? 0) || results.length;
  const shownFrom = page * PAGE_SIZE + 1;
  const shownTo = page * PAGE_SIZE + results.length;

  // 换了分类或筛选后总页数可能变少（例如从 mod 切到 world），停在越界页会
  // 显示空白却还能继续点。这里把它夹回最后一页。
  useEffect(() => {
    if (page > pageCount - 1) setPage(pageCount - 1);
  }, [page, pageCount]);

  const goToPage = (next: number) => {
    const clamped = clampPage(next, pageCount);
    if (clamped === page) return;
    setPage(clamped);
    // 翻页后把结果区滚回顶部，否则会停在新一页的中间位置。
    panelRef.current?.scrollTo({ top: 0, behavior: 'smooth' });
  };

  /**
   * 任何会改变结果集的操作都从第一页重新开始。
   *
   * 单独调用某个 setter 时很容易漏掉 setPage(0)（排序、分类切换都曾如此），
   * 于是用户会停在一个旧页码上，看着「第 4 / 2 页」这种矛盾状态。把「改条件」
   * 和「回首页」绑在一起，新加的筛选条件就不会再漏。
   */
  const resetToFirstPage = <T,>(setter: (value: T) => void) => (value: T) => {
    setter(value);
    setPage(0);
  };

  const activeResults: SourceResult[] = category === 'vanilla' ? [vanilla] : [modrinth, curseforge].filter(
    result => result.state !== 'idle',
  );
  const isLoading = activeResults.some(result => result.state === 'loading');
  const errors = activeResults.filter(result => result.state === 'error');
  const mixed = category !== 'vanilla' && isMixed(sortKey, sources);

  const isQueued = (name: string) => downloadTasks.some(
    task => task.name === name && task.status !== 'cancelled' && task.status !== 'failed',
  );

  const saveKey = () => {
    saveCurseForgeKey(keyDraft);
    const next = Boolean(keyDraft.trim());
    setHasKey(next);
    setKeyOpen(false);
    if (next) void runSearch();
  };

  const clearKey = () => {
    saveCurseForgeKey('');
    setKeyDraft('');
    setHasKey(false);
    setCurseforge({ state: 'idle', entries: [] });
  };

  const sortKeys = category === 'vanilla' ? VANILLA_SORT_KEYS : MOD_SORT_KEYS.filter(
    key => key !== 'follows' || sources.includes('Modrinth'),
  );

  // 取消勾选 Modrinth 后「关注数」会从选项里消失。此时若它正是当前选中项，
  // <select> 会没有匹配的 option，浏览器显示成空白。这里回落到该分类默认值。
  // 依赖用 join 出来的字符串：sortKeys 每次渲染都是新数组，直接放进去会每帧重跑。
  const sortKeysSignature = sortKeys.join(',');
  useEffect(() => {
    if (!sortKeysSignature.split(',').includes(sortKey)) {
      setSortKey(DEFAULT_SORT[category]);
      setSortDirection(DEFAULT_DIRECTION);
      delete sortMemoryRef.current[category];
    }
  }, [category, sortKey, sortKeysSignature]);

  const submitSearch = () => {
    if (debounceRef.current) window.clearTimeout(debounceRef.current);
    setSubmittedQuery(query);
    // 重新搜索是「新的查询」，从第一页开始。
    setPage(0);
  };

  const sourceText = sources.length
    ? sources.join(t('store.sourceSeparator'))
    : t('store.hintNoSource');

  return (
    <div className={styles.store}>
      <header className={styles.header}>
        <div>
          <h1>{t('store.title')}</h1>
          <p className={styles.headerHint}>
            {category === 'vanilla'
              ? t('store.hintVanilla')
              : t('store.hintSource', { values: { sources: sourceText, category: categoryLabel } })}
          </p>
        </div>
      </header>

      <div className={styles.workspace}>
        <aside className={styles.nav} aria-label={t('store.downloadSettings')}>
          <div className={styles.navTitle}>{t('store.downloadSettings')}</div>
          <div className={styles.currentType}>
            <span className={styles.categoryIcon}>
              {category === 'vanilla'
                ? <BlockIcon {...VANILLA_BLOCK} size={24} />
                : <span className={styles.categoryFallback}>{categoryLabel.slice(0, 1)}</span>}
            </span>
            <div>
              <strong>{categoryLabel}</strong>
              <small>{t('store.switchTypeHint')}</small>
            </div>
          </div>
          <div className={styles.navDivider} />
          <div className={styles.targetBlock}>
            <label htmlFor="store-target">{t('store.currentInstance')}</label>
            <select id="store-target" value={targetInstanceId} onChange={event => onSelectInstance(event.target.value)}>
              <option value="">{t('store.downloadTaskTarget')}</option>
              {instances.map(instance => <option key={instance.id} value={instance.id}>{instance.name}</option>)}
            </select>
          </div>
        </aside>

        <section className={styles.panel} ref={panelRef}>
          <div className={styles.filterCard}>
            <div className={styles.filterTitle}>
              <h2>{t('store.searchCategory', { values: { category: categoryLabel } })}</h2>
              <span>
                {category === 'vanilla'
                  ? vanilla.state === 'ready'
                    ? t('store.versionCount', { count: vanillaEntries.length })
                    : '—'
                  : isLoading
                    ? t('store.loading')
                    : t('store.resultCount', { count: totalShown })}
              </span>
            </div>

            <div className={styles.filterGrid}>
              <label className={`${styles.field} ${styles.fieldWide}`}>
                <span>{t('store.name')}</span>
                <div className={styles.fieldInput}>
                  <Search size={15} />
                  <input
                    value={query}
                    onChange={event => onQueryChange(event.target.value)}
                    onKeyDown={event => { if (event.key === 'Enter') submitSearch(); }}
                    placeholder={category === 'vanilla'
                      ? t('store.searchPlaceholderVersion')
                      : t('store.searchPlaceholder', { values: { category: categoryLabel } })}
                  />
                </div>
              </label>

              {category === 'vanilla' ? (
                <div className={`${styles.field} ${styles.fieldWide}`}>
                  <span>{t('store.versionType')}</span>
                  <div className={styles.checkGroup}>
                    {VERSION_KINDS.map(kind => (
                      <label key={kind} className={styles.checkOption}>
                        <input type="checkbox" checked={versionKinds.includes(kind)} onChange={() => toggleVersionKind(kind)} />
                        <span>{versionKindLabel(kind)}</span>
                      </label>
                    ))}
                  </div>
                </div>
              ) : (
                <>
                  <div className={styles.field}>
                    <span>{t('store.source')}</span>
                    <div className={styles.checkGroup}>
                      {(['Modrinth', 'CurseForge'] as const).map(source => (
                        <label key={source} className={styles.checkOption}>
                          <input type="checkbox" checked={sources.includes(source)} onChange={() => toggleSource(source)} />
                          <span className={styles.sourceName}>
                            <i className={source === 'Modrinth' ? styles.dotModrinth : styles.dotCurseforge} aria-hidden="true" />
                            {source}
                            {source === 'CurseForge' && !hasKey ? <em className={styles.needKey}>{t('store.needKey')}</em> : null}
                          </span>
                        </label>
                      ))}
                    </div>
                  </div>
                  <label className={styles.field}>
                    <span>{t('store.gameVersion')}</span>
                    <select value={gameVersion} onChange={event => resetToFirstPage(setGameVersion)(event.target.value)}>
                      <option value="">{t('store.allVersions')}</option>
                      {releaseList.map(version => <option key={version} value={version}>{version}</option>)}
                    </select>
                  </label>
                </>
              )}
            </div>

            <div className={styles.filterActions}>
              <Button onClick={submitSearch} icon={<Search size={15} />}>{t('store.search')}</Button>
              <Button variant="ghost" onClick={resetFilters}>{t('store.resetFilters')}</Button>
            </div>
          </div>

          <div className={styles.resultsCard}>
            <div className={styles.resultsHeader}>
              <h2>{category === 'vanilla' ? t('store.versionList') : t('store.searchResults')}</h2>
              <div className={styles.sortBar}>
                {mixed && <span className={styles.mixedNote}>{t('store.mixedSort')}</span>}
                <label className={styles.sortField}>
                  <span className={styles.sortLabel}>{t('store.sort')}</span>
                  <select
                    value={sortKey}
                    onChange={event => changeSort(event.target.value as SortKey)}
                    aria-label={t('store.sortLabel')}
                  >
                    {sortKeys.map(key => <option key={key} value={key}>{t(SORT_LABEL_KEYS[key])}</option>)}
                  </select>
                </label>
                <button
                  type="button"
                  className={styles.sortToggle}
                  onClick={() => changeSortDirection(sortDirection === 'desc' ? 'asc' : 'desc')}
                  aria-label={sortDirection === 'desc' ? t('store.sortToAsc') : t('store.sortToDesc')}
                  aria-pressed={sortDirection === 'asc'}
                  title={sortDirection === 'desc' ? t('store.sortDesc') : t('store.sortAsc')}
                >
                  {sortDirection === 'desc' ? <ArrowDownWideNarrow size={16} /> : <ArrowUpNarrowWide size={16} />}
                </button>
              </div>
            </div>

            <div className={styles.resultsMeta}>
              <span>{targetInstanceId ? t('store.installToInstance') : t('store.installToTasks')}</span>
              {category !== 'vanilla' && (
                <button
                  type="button"
                  className={styles.keyButton}
                  onClick={() => { setKeyDraft(readCurseForgeKey()); setKeyOpen(open => !open); }}
                  aria-expanded={keyOpen}
                >
                  <KeyRound size={13} />{hasKey ? t('store.keySet') : t('store.keyFill')}
                </button>
              )}
            </div>

            {keyOpen && category !== 'vanilla' && (
              <div className={styles.keyPanel}>
                <label htmlFor="cf-key">{t('store.keyLabel')}</label>
                <div className={styles.keyRow}>
                  <input
                    id="cf-key"
                    type="password"
                    value={keyDraft}
                    onChange={event => setKeyDraft(event.target.value)}
                    onKeyDown={event => { if (event.key === 'Enter') saveKey(); }}
                    placeholder={t('store.keyPlaceholder')}
                    autoComplete="off"
                    spellCheck={false}
                  />
                  <Button size="sm" onClick={saveKey}>{t('store.keySave')}</Button>
                  {hasKey && <Button size="sm" variant="ghost" onClick={clearKey}>{t('store.keyClear')}</Button>}
                </div>
                <p>
                  {withLink(
                    t('store.keyHelp', { values: { link: LINK_SLOT } }),
                    <a href="https://console.curseforge.com/" target="_blank" rel="noreferrer">{t('store.keyConsole')}</a>,
                  )}
                </p>
              </div>
            )}

            {errors.map((result, index) => (
              <div key={`${index}-${typeof result.error === 'string' ? result.error : result.error?.key ?? 'error'}`} className={styles.errorRow} role="alert">
                <span>{renderLocalized(language, result.error)}</span>
                <div className={styles.errorActions}>
                  {result.actionable && (
                    <button type="button" className={styles.linkButton} onClick={() => { setKeyDraft(readCurseForgeKey()); setKeyOpen(true); }}>
                      {t('store.fillKey')}
                    </button>
                  )}
                  <button
                    type="button"
                    className={styles.linkButton}
                    onClick={() => { if (category === 'vanilla') void loadManifest(true); else void runSearch({ fresh: true }); }}
                  >
                    <RefreshCw size={12} />{t('common.retry')}
                  </button>
                </div>
              </div>
            ))}

            {isLoading && results.length === 0 && (
              <div className={styles.skeletonList} aria-live="polite" aria-busy="true">
                {[0, 1, 2, 3].map(index => (
                  <div key={index} className={styles.skeletonRow}>
                    <span className={styles.skeletonIcon} />
                    <span className={styles.skeletonLines}>
                      <i /><i />
                    </span>
                  </div>
                ))}
                <span className={styles.srOnly}>{t('store.loadingAria')}</span>
              </div>
            )}

            {!isLoading && results.length === 0 && errors.length === 0 && (
              <div className={styles.empty}>
                {category !== 'vanilla' && sources.length === 0
                  ? t('store.emptyNoSource')
                  : category === 'vanilla'
                    ? t('store.emptyNoKind')
                    : t('store.emptyNoMatch')}
              </div>
            )}

            {category === 'vanilla' ? (
              <div className={styles.versionList}>
                {results.map(entry => {
                  const kind = entry.version?.kind ?? 'release';
                  const block = blockForVersion(kind) ?? VANILLA_BLOCK;
                  return (
                    <div key={`${kind}-${entry.id}`} className={styles.versionRow}>
                      <BlockIcon {...block} size={36} />
                      <div className={styles.versionMain}>
                        <div className={styles.versionTitle}>
                          <strong>Minecraft {entry.name}</strong>
                          {entry.version?.latest && <span className={styles.latestBadge}>{t('store.latest')}</span>}
                          <span className={kind === 'special' ? styles.kindSpecial : styles.kindBadge}>
                            {versionKindLabel(kind)}
                          </span>
                        </div>
                        <span className={styles.versionMeta}>
                          {formatDate(entry.modified, language)} · {t('store.javaEstimate', { values: { major: entry.version?.javaMajor ?? 8 } })}
                          {kind !== 'release' ? t('store.estimated') : ''}
                        </span>
                        {entry.version?.foolNote && <span className={styles.foolNote}>{t(entry.version.foolNote)}</span>}
                      </div>
                      <Button
                        size="sm"
                        onClick={() => onQueueDownload(`Minecraft ${entry.name}`, 'game', targetInstanceId || undefined)}
                        disabled={isQueued(`Minecraft ${entry.name}`)}
                      >
                        {isQueued(`Minecraft ${entry.name}`) ? t('store.installed') : t('store.install')}
                      </Button>
                    </div>
                  );
                })}
              </div>
            ) : (
              <div className={styles.contentList}>
                {results.map(entry => {
                  const taskType: DownloadTask['type'] = entry.category === 'mod' ? 'mod'
                    : entry.category === 'modpack' ? 'modpack' : 'resource';
                  return (
                    <div key={`${entry.source}-${entry.id}`} className={styles.contentRow}>
                      <StoreLogo
                        src={entry.iconUrl}
                        alt={t('store.itemIconAlt', { values: { name: entry.name } })}
                        category={entry.category}
                      />
                      <div className={styles.contentMain}>
                        <div className={styles.contentTitleLine}>
                          <strong>{entry.name}</strong>
                          <span className={`${styles.sourceBadge} ${entry.source === 'Modrinth' ? styles.badgeModrinth : styles.badgeCurseforge}`}>
                            {entry.source}
                          </span>
                        </div>
                        <p>{entry.summary || t('store.noSummary')}</p>
                        <span className={styles.contentMeta}>
                          {entry.author ? `${entry.author} · ` : ''}
                          {entry.downloads !== undefined
                            ? t('store.downloadsShort', { values: { value: formatCount(entry.downloads, language) }, count: entry.downloads })
                            : t('store.downloadsUnknown')}
                          {entry.follows !== undefined
                            ? ` · ${t('store.followsShort', { values: { value: formatCount(entry.follows, language) }, count: entry.follows })}`
                            : ''}
                          {entry.modified
                            ? ` · ${t('store.updatedAt', { values: { time: renderLocalized(language, formatRelative(entry.modified)) } })}`
                            : ''}
                        </span>
                      </div>
                      <Button
                        size="sm"
                        onClick={() => onQueueDownload(entry.name, taskType, targetInstanceId || undefined)}
                        disabled={isQueued(entry.name)}
                      >
                        {isQueued(entry.name) ? t('store.installed') : t('store.install')}
                      </Button>
                    </div>
                  );
                })}
              </div>
            )}

            {/*
              只要不止一页就一直显示分页条，包括加载中的那一帧：条件里带上
              results.length 会让它在翻页时消失又出现，整个列表上下跳动。
              翻页期间按钮由 isLoading 禁用，不会重复触发。
            */}
            {pageCount > 1 && (
              <nav className={styles.pager} aria-label={t('store.pageNavLabel')}>
                <span className={styles.pagerRange}>
                  {t('store.pageRange', { values: { from: shownFrom, to: shownTo, total: totalShown } })}
                </span>
                <div className={styles.pagerControls}>
                  <button
                    type="button"
                    className={styles.pagerButton}
                    onClick={() => goToPage(page - 1)}
                    disabled={page === 0 || isLoading}
                    aria-label={t('store.pagePrev')}
                  >
                    <ChevronLeft size={15} />
                    {t('store.pagePrev')}
                  </button>
                  <span className={styles.pagerInfo}>
                    {t('store.pageInfo', { values: { page: page + 1, pages: pageCount } })}
                  </span>
                  <button
                    type="button"
                    className={styles.pagerButton}
                    onClick={() => goToPage(page + 1)}
                    disabled={page >= pageCount - 1 || isLoading}
                    aria-label={t('store.pageNext')}
                  >
                    {t('store.pageNext')}
                    <ChevronRight size={15} />
                  </button>
                </div>
              </nav>
            )}
          </div>
        </section>
      </div>
    </div>
  );
}

/** 分类菜单里「原版游戏」要用金块图，其余用 lucide 图标。 */
export function StoreCategoryIcon({ id }: { id: StoreCategory }) {
  if (id === 'vanilla') return <BlockIcon {...VANILLA_BLOCK} size={20} />;
  return null;
}

export default StoreWorkspace;
