// CurseForge 搜索面板。
//
// ## 为什么单独成一个组件，而不是塞进 StoreWorkspace 的 mod 分支
//
// 两个来源的模型差别不小：Modrinth 用「游戏版本 + 加载器」的 facet 字符串，失败
// 时返回结构化错误；CurseForge 用数字枚举、按 `index/pageSize` 分页、key 缺失与
// 无效都是同一个空响应体的 403。把它们写进同一个组件会让每个分支都要判来源，
// 单独成块反而更容易维持两边都正确。
//
// ## key 的处理
//
// key 由后端存进系统凭据库，这个组件只查询「有没有设置」。输入框里的值只在点
// 「保存」时上传一次，之后立刻从组件状态里清掉——不在内存里留一份不必要的副本。

import { useCallback, useEffect, useState } from 'react';
import { Download, RefreshCw, Search } from 'lucide-react';
import { Button } from './Button';
import { CurseForgeKeyCard } from './CurseForgeKeyCard';
import { useI18n } from '../utils/i18n';
import {
  curseforgeFiles,
  curseforgeInstall,
  curseforgeKeyStatus,
  curseforgeSearch,
  errorText,
  formatBytes,
  type CurseForgeKeyStatus,
  type CurseForgeKind,
  type CurseForgeMod,
  type CurseForgeVersion,
} from '../utils/native';
import type { GameInstance } from '../types';
// 布局类名与内容商店共用一份 CSS Module：搜索框、结果行、分页器这套结构与
// Modrinth 完全一致，复制一遍会在两边开始漂移。只有 CurseForge 特有的部分
// （key 设置卡片、受限文件提示）写在本组件自己的样式表里。
import styles from './StoreWorkspace.module.css';
import own from './CurseForgePanel.module.css';

interface CurseForgePanelProps {
  /**
   * 内容类型。决定搜索的 classId 与安装目录：
   * mod→mods、resourcepack→resourcepacks、world→saves。
   * 整合包不走这里（要创建新实例，由整合包流程处理）。
   */
  kind: Exclude<CurseForgeKind, 'modpack'>;
  instance?: GameInstance;
  language: string;
  onNotice: (message: string) => void;
  onChanged: () => void;
}

/** 每页条数。服务端上限 50，20 是一屏能读完的量。 */
const PAGE_SIZE = 20;

export function CurseForgePanel({ kind, instance, language, onNotice, onChanged }: CurseForgePanelProps) {
  const { t } = useI18n();

  const [keyStatus, setKeyStatus] = useState<CurseForgeKeyStatus | null>(null);

  const [query, setQuery] = useState('');
  const [submitted, setSubmitted] = useState('');
  const [index, setIndex] = useState(0);
  const [results, setResults] = useState<CurseForgeMod[]>([]);
  const [searchState, setSearchState] = useState<'idle' | 'loading' | 'ready' | 'error'>('idle');
  const [searchError, setSearchError] = useState('');

  const [expanded, setExpanded] = useState<string | null>(null);
  const [versions, setVersions] = useState<CurseForgeVersion[]>([]);
  const [versionsState, setVersionsState] = useState<'idle' | 'loading' | 'ready' | 'error'>('idle');
  const [installing, setInstalling] = useState('');

  const locale = language === 'zh-CN' ? 'zh-CN' : language === 'ja' ? 'ja-JP' : 'en-US';

  /**
   * 当前类型的名称与搜索示例词。
   *
   * 三类的措辞不同：地图不该说「搜索模组」，资源包也不需要提「加载器」。
   * 示例词按类型给，避免提示用户输入一个该类型里不存在的名字。
   */
  const kindLabel = t(
    kind === 'resourcepack' ? 'cf.kindResourcePack' : kind === 'world' ? 'cf.kindWorld' : 'cf.kindMod',
  );
  const kindExample = kind === 'resourcepack' ? 'faithful' : kind === 'world' ? 'parkour' : 'jei';

  useEffect(() => {
    void curseforgeKeyStatus().then(setKeyStatus).catch(() => setKeyStatus({ configured: false, problem: null, source: null }));
  }, []);



  /*
   * 只依赖真正参与请求的字段，而不是 `instance` 对象本身。
   *
   * 父组件用 `instances.find(...)` 取实例，每次渲染都是新对象；把它放进依赖会让
   * `runSearch` 每帧变成新函数，下面的 effect 随之重跑并再发一次请求，`setSearchState`
   * 又触发渲染——搜索过一次之后就是自激循环。改用 `id/version/loader` 三个标量，
   * 值不变就不重建。
   */
  const instanceId = instance?.id;
  const instanceVersion = instance?.version;
  const instanceLoader = instance?.loader;

  const runSearch = useCallback(async (page: number) => {
    const query = submitted.trim();
    if (!query) return;
    if (!instanceId) {
      setSearchState('error');
      setSearchError(t('cf.needInstance'));
      return;
    }
    setSearchState('loading');
    setSearchError('');
    try {
      const hits = await curseforgeSearch({
        kind,
        query,
        gameVersion: instanceVersion,
        loader: instanceLoader,
        index: page * PAGE_SIZE,
        pageSize: PAGE_SIZE,
      });
      setResults(hits);
      setSearchState('ready');
    } catch (error) {
      setResults([]);
      setSearchError(errorText(error, t('cf.searchFailed')));
      setSearchState('error');
    }
  }, [instanceId, instanceLoader, instanceVersion, kind, submitted, t]);

  useEffect(() => {
    if (!keyStatus?.configured || !submitted) return;
    void runSearch(index);
  }, [index, keyStatus?.configured, runSearch, submitted]);

  const openVersions = async (mod: CurseForgeMod) => {
    if (expanded === mod.projectId) { setExpanded(null); return; }
    if (!instance) return;
    setExpanded(mod.projectId);
    setVersionsState('loading');
    setVersions([]);
    try {
      setVersions(await curseforgeFiles(mod.projectId, instance.version, instance.loader));
      setVersionsState('ready');
    } catch (error) {
      setVersionsState('error');
      onNotice(errorText(error, t('cf.versionsFailed')));
    }
  };

  const install = async (version: CurseForgeVersion) => {
    if (!instance) return;
    setInstalling(version.versionId);
    try {
      const name = await curseforgeInstall(instance.id, version, kind);
      onNotice(t('cf.installedKind', { values: { kind: kindLabel, name } }));
      onChanged();
    } catch (error) {
      onNotice(errorText(error, `安装${kindLabel}失败。`));
    } finally {
      setInstalling('');
    }
  };

  /*
   * 没有任何可用 key：只显示设置入口。
   *
   * 用共享的 CurseForgeKeyCard 而不是本组件自己再写一份表单：模组分类现在也
   * 需要就地配 key（勾选 CurseForge 但没配时），两处共用同一个组件才不会出现
   * 「一处能配、另一处不能」的不一致。
   *
   * 判断的是「有没有内置 key 兜底」，而不是 `configured`：内置 key 时
   * `configured` 已经是 true，如果只按它判断，用户在这条路径上就永远看不到
   * 「换用自己的 key」的入口（模组分类里有，这里没有，两处行为不一致）。
   */
  const hasBuiltinOnly = keyStatus?.configured && keyStatus.source === 'builtin';
  if (!keyStatus?.configured) {
    return (
      <CurseForgeKeyCard
        /*
         * 用 `setKeyStatus` 直接作为回调，而不是再包一层箭头函数。
         *
         * `setKeyStatus` 是 React 保证引用稳定的函数，而内联箭头每次渲染都是新
         * 引用。虽然 `CurseForgeKeyCard` 现在用 ref 持有回调、不再因此循环，但
         * 传稳定引用可以让这层依赖关系本来就不存在，避免以后有人改回去。
         */
        onStatusChange={setKeyStatus}
        onNotice={onNotice}
      />
    );
  }

  // --- 已设置 key：搜索 -----------------------------------------------
  return (
    <>
      {/* 用内置 key 时给出可收起的覆盖入口，与模组分类里的行为保持一致。 */}
      {hasBuiltinOnly && (
        <div className={styles.filterCard}>
          <CurseForgeKeyCard
            compact
            onStatusChange={setKeyStatus}
            onNotice={onNotice}
          />
        </div>
      )}
      <div className={styles.filterCard}>
        <div className={styles.filterTitle}>
          <h2>{t('cf.searchTitleKind', { values: { kind: kindLabel } })}</h2>
          <span>
            {searchState === 'loading'
              ? t('store.loading')
              : searchState === 'ready'
                ? t('store.resultCount', { count: results.length })
                : '—'}
          </span>
        </div>
        <div className={styles.filterGrid}>
          <label className={styles.field}>
            <span>{t('store.name')}</span>
            <div className={styles.fieldInput}>
              <Search size={15} />
              <input
                value={query}
                onChange={event => setQuery(event.target.value)}
                onKeyDown={event => {
                  if (event.key !== 'Enter') return;
                  setSubmitted(query);
                  setIndex(0);
                  if (index === 0) void runSearch(0);
                }}
                placeholder={t('cf.placeholderKind', { values: { kind: kindLabel, example: kindExample } })}
              />
            </div>
          </label>
        </div>
        <div className={styles.filterActions}>
          <Button onClick={() => { setSubmitted(query); setIndex(0); void runSearch(0); }} icon={<Search size={15} />}>
            {t('store.search')}
          </Button>
        </div>
        <p className={styles.mixedNote}>{t('cf.hintForKind')}</p>
      </div>

      <div className={styles.resultsCard}>
        <div className={styles.resultsHeader}>
          <h2>{t('store.searchResults')}</h2>
        </div>

        {searchState === 'error' && (
          /*
           * 这里用 `styles.*` 而不是 `own.*`：错误行与重试链接的结构、配色是内容商店
           * 通用的，规则定义在 StoreWorkspace.module.css 里（与 Modrinth 那侧共用）。
           * `own` 指向本组件自己的样式表，那里没有这两条，写成 `own.` 会解析成
           * undefined，错误行的边框和链接的配色都会静默丢失。
           */
          <div className={styles.errorRow} role="alert">
            <span>{searchError}</span>
            <button type="button" className={styles.linkButton} onClick={() => void runSearch(index)}>
              <RefreshCw size={12} />{t('common.retry')}
            </button>
          </div>
        )}

        {searchState === 'idle' && (
          <div className={styles.empty}>{t('cf.idle')}</div>
        )}

        {searchState === 'loading' && (
          <div className={styles.skeletonList} aria-busy="true">
            {[0, 1, 2, 3].map(row => (
              <div key={row} className={styles.skeletonRow}>
                <span className={styles.skeletonIcon} />
                <span className={styles.skeletonLines}><i /><i /></span>
              </div>
            ))}
          </div>
        )}

        {searchState === 'ready' && results.length === 0 && (
          <div className={styles.empty}>{t('cf.noResultsKind', { values: { kind: kindLabel } })}</div>
        )}

        <div className={styles.versionList}>
          {results.map(mod => (
            <div key={mod.projectId}>
              <div className={styles.contentRow}>
                {mod.iconUrl ? (
                  <img className={own.logo} src={mod.iconUrl} alt="" width={36} height={36} loading="lazy" />
                ) : (
                  <span className={own.logoFallback} aria-hidden="true">CF</span>
                )}
                <div className={styles.contentMain}>
                  <div className={styles.contentTitleLine}>
                    <strong>{mod.title}</strong>
                    <span className={`${styles.sourceBadge} ${own.badgeCurseForge}`}>CurseForge</span>
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
                <Button size="sm" variant="ghost" onClick={() => void openVersions(mod)}>
                  {expanded === mod.projectId ? t('cf.collapse') : t('cf.chooseVersion')}
                </Button>
              </div>

              {expanded === mod.projectId && (
                <div className={own.versionListInner}>
                  {versionsState === 'loading' && <div className={styles.versionMeta}>{t('store.loading')}</div>}
                  {versionsState === 'ready' && versions.length === 0 && (
                    <div className={styles.versionMeta}>{t('cf.noVersion')}</div>
                  )}
                  {versions.map(version => (
                    <div key={version.versionId} className={styles.versionRow}>
                      <div className={styles.versionMain}>
                        <div className={styles.versionTitle}>
                          <strong>{version.displayName || version.fileName}</strong>
                        </div>
                        <span className={styles.versionMeta}>
                          {version.fileName}
                          {version.size > 0 ? ` · ${formatBytes(version.size)}` : ''}
                          {version.dateCreated
                            ? ` · ${new Date(version.dateCreated).toLocaleDateString(locale)}`
                            : ''}
                        </span>
                        {/*
                          受限文件：作者禁止第三方启动器分发。这里不给「安装」按钮，
                          而是明确说明需要去网页手动下载——点一个必然失败的按钮比
                          没有按钮更糟。
                        */}
                        {version.url === null && (
                          <span className={own.restricted}>{t('cf.restricted')}</span>
                        )}
                      </div>
                      <Button
                        size="sm"
                        disabled={version.url === null || installing === version.versionId}
                        icon={<Download size={13} />}
                        onClick={() => void install(version)}
                      >
                        {version.url === null
                          ? t('cf.manual')
                          : installing === version.versionId ? t('store.loading') : t('store.install')}
                      </Button>
                    </div>
                  ))}
                </div>
              )}
            </div>
          ))}
        </div>

        {(index > 0 || results.length === PAGE_SIZE) && (
          <nav className={styles.pager} aria-label={t('store.pageNavLabel')}>
            <button
              type="button"
              className={styles.pagerButton}
              disabled={index === 0 || searchState === 'loading'}
              onClick={() => setIndex(current => Math.max(0, current - 1))}
            >
              {t('store.pagePrev')}
            </button>
            <span className={styles.pagerInfo}>{t('cf.page', { values: { page: index + 1 } })}</span>
            <button
              type="button"
              className={styles.pagerButton}
              disabled={results.length < PAGE_SIZE || searchState === 'loading'}
              onClick={() => setIndex(current => current + 1)}
            >
              {t('store.pageNext')}
            </button>
          </nav>
        )}
      </div>
    </>
  );
}
