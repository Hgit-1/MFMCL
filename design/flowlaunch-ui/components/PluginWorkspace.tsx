// 启动器插件页。
//
// 页面要回答的两个问题：现在装了什么、它们能做什么。所以每张卡片先给出身份
// （名称、作者、版本）与一句话用途，再显式列出「扩展点」和「权限」的数量；
// 展开后才逐条解释每一项。把权限默认展开会让列表变成一堵字墙，默认收起又
// 等于藏起来——折中成「数量可见、详情一点即开」。
//
// 与 utils/plugins.ts 的分工：那边只管数据形状、校验与持久化，这边只管渲染与
// 交互。插件代码本身不在这里执行，预览环境没有主进程可执行；这里改动的是
// 「下一个启动会话要加载哪些插件」的配置。

import { useEffect, useMemo, useRef, useState } from 'react';
import {
  AlertTriangle,
  ArrowUpCircle,
  ArchiveRestore,
  Ban,
  ChevronDown,
  Gauge,
  Image as ImageIcon,
  Network,
  Palette,
  Plug,
  RotateCcw,
  ScrollText,
  Search,
  ShieldCheck,
  Trash2,
} from 'lucide-react';
import { Modal } from './Modal';
import { useI18n } from '../utils/i18n';
import { renderLocalized, type Localized } from '../utils/i18n/localized';
import {
  allPlugins,
  enabledPluginCount,
  hookHint,
  hookLabel,
  isPluginEnabled,
  normalizePlugin,
  optionValue,
  permissionHint,
  permissionLabel,
  pluginNeedsAttention,
  toPluginStatePayload,
  type LauncherPlugin,
  type PluginGlyph,
  type PluginOption,
  type PluginStore,
} from '../utils/plugins';
import {
  clearLaunchHistory,
  errorText,
  launchHistory,
  pluginPreview,
  pluginRuntimeInfo,
  pluginSaveState,
  type LaunchHistoryEntry,
  type PluginPreview,
  type PluginRuntimeInfo,
  type PluginContributionKind,
} from '../utils/native';
import styles from './PluginWorkspace.module.css';

const GLYPH_ICONS: Record<PluginGlyph, typeof Plug> = {
  scroll: ScrollText,
  archive: ArchiveRestore,
  shield: ShieldCheck,
  palette: Palette,
  gauge: Gauge,
  network: Network,
  image: ImageIcon,
  plug: Plug,
};

type FilterId = 'all' | 'enabled' | 'disabled' | 'attention';

/** 贡献类型 → 展示文案。与 Rust `ContributionKind` 一一对应。 */
function kindLabel(kind: PluginContributionKind, t: ReturnType<typeof useI18n>['t']): string {
  switch (kind) {
    case 'jvmArgs': return t('plugin.contribution.jvmArgs');
    case 'env': return t('plugin.contribution.env');
    case 'preLaunch': return t('plugin.contribution.preLaunch');
    case 'backupBefore': return t('plugin.contribution.backupBefore');
  }
}

const FILTER_KEYS = {
  all: 'plugin.filterAll',
  enabled: 'plugin.filterEnabled',
  disabled: 'plugin.filterDisabled',
  attention: 'plugin.filterAttention',
} as const;

interface PluginWorkspaceProps {
  store: PluginStore;
  onChange: (next: PluginStore) => void;
  /**
   * 本次会话里改过、但还没重启的插件。
   *
   * 由 App 持有而不是页面自己存：切成别的栏目再回来时页面会卸载，若状态留在
   * 这里，「X 项改动待重启」的提示就丢了；侧栏徽章也需要同一份数据才能与
   * 「需要注意」筛选保持一致。
   */
  pendingRestart: string[];
  onPendingRestartChange: (ids: string[]) => void;
  /** 操作结果提示，与其他页面共用同一条提示条。 */
  onNotice: (message: string) => void;
  /**
   * 当前的下载源偏好，以及修改它的回调。
   *
   * 为什么插件页需要它：内置的「下载镜像助手」有一个「优先使用镜像源」开关，
   * 而镜像选择的真实开关是设置里的 `downloadSource`。如果插件页只把这个值存进
   * 自己的选项表，那个开关就是**没有效果**的——用户拨动它，下载行为不会改变。
   *
   * 之前正是这样：选项存下来了，但没有任何代码读它。现在它直接写真实的设置。
   */
  downloadSource?: string;
  onDownloadSourceChange?: (source: string) => void;
}

export function PluginWorkspace({ store, onChange, pendingRestart, onPendingRestartChange, onNotice, downloadSource, onDownloadSourceChange }: PluginWorkspaceProps) {
  const { t, language } = useI18n();
  /** 日期本地化用。与内容商店保持一致的三语映射。 */
  const locale = language === 'zh-CN' ? 'zh-CN' : language === 'ja' ? 'ja-JP' : 'en-US';
  const [query, setQuery] = useState('');
  const [filter, setFilter] = useState<FilterId>('all');
  const [expanded, setExpanded] = useState<string | null>(null);
  const [installOpen, setInstallOpen] = useState(false);
  const [manifestText, setManifestText] = useState('');
  const [installError, setInstallError] = useState<Localized | null>(null);
  /**
   * 后端运行时是否可用，以及它支持哪些贡献类型。
   *
   * 这个值决定页头那段说明说什么。之前是一句写死的「没有运行时」，现在要按实际
   * 情况给：桌面版有运行时，就说清楚「启用的插件会在启动时执行它的贡献」；浏览器
   * 预览没有后端，就如实说明只保存配置。
   */
  const [runtime, setRuntime] = useState<PluginRuntimeInfo | null>(null);
  /** 每个插件的贡献预览。只对展开的那一个查，避免打开页面就发 N 个请求。 */
  const [preview, setPreview] = useState<{ id: string; data: PluginPreview } | null>(null);
  /** 启动记录。与插件预览同一理由：这是运行时存在的证据，不是装饰。 */
  const [history, setHistory] = useState<LaunchHistoryEntry[]>([]);

  useEffect(() => {
    let alive = true;
    void pluginRuntimeInfo()
      .then(info => {
        if (!alive) return;
        setRuntime(info);
        // 只有真的有运行时才去读历史：预览模式读不到，多一次请求没有意义。
        if (info.available) {
          void launchHistory()
            .then(entries => { if (alive) setHistory(entries); })
            .catch(() => undefined);
        }
      })
      .catch(() => { if (alive) setRuntime({ available: false, kinds: [] }); });
    return () => { alive = false; };
  }, []);

  const clearHistory = async () => {
    try {
      await clearLaunchHistory();
      setHistory([]);
    } catch (error) {
      onNotice(errorText(error, t('plugin.syncFailed', { values: { reason: '' } })));
    }
  };

  /*
   * 展开某个插件时拉一次它的贡献预览。
   *
   * 为什么值得一次请求：它回答的是「这个插件到底会不会做事」。没有它，用户只能
   * 通过启动游戏后观察副作用来猜——而那正是这次要修的问题。
   */
  useEffect(() => {
    if (!expanded || !runtime?.available) { setPreview(null); return; }
    let alive = true;
    void pluginPreview(expanded)
      .then(data => { if (alive && data) setPreview({ id: expanded, data }); })
      .catch(() => { if (alive) setPreview(null); });
    return () => { alive = false; };
  }, [expanded, runtime?.available]);

  /**
   * 状态变化时同步到后端。
   *
   * 同步是「尽力而为」而不是阻塞式：后端拒绝时把原因提示出来，但界面不等它。
   * 原因很简单——后端校验的是**贡献内容**，而用户此刻可能只是在开关一个没有贡献
   * 的插件。把每个开关都串行等一次 IPC 会让界面变慢，收益却只有那一小部分场景。
   *
   * 失败时提示而不回滚：本地状态已经是用户看到的样子，回滚会让界面闪烁；而再次
   * 改任一项都会重新同步，坏的条目下次保存时仍会被拒并再次提示。
   */
  const syncToBackend = (next: PluginStore) => {
    if (!runtime?.available) return;
    void pluginSaveState(toPluginStatePayload(next)).catch(error => {
      onNotice(errorText(error, t('plugin.syncFailed')));
    });
  };
  const updateStore = (next: PluginStore) => {
    onChange(next);
    syncToBackend(next);
  };
  const manifestRef = useRef<HTMLTextAreaElement>(null);

  /*
   * 打开安装弹窗后把焦点放进文本框。
   *
   * Modal 自带的自动聚焦会落到第一个可聚焦元素上，也就是标题栏的关闭按钮；
   * 用户的目的明明是粘贴清单，先落到「关闭」既多余又容易误触。
   */
  useEffect(() => {
    if (!installOpen) return;
    const id = window.setTimeout(() => manifestRef.current?.focus(), 0);
    return () => window.clearTimeout(id);
  }, [installOpen]);

  const plugins = useMemo(() => allPlugins(store), [store]);
  const enabledCount = useMemo(() => enabledPluginCount(store), [store]);
  const text = (value: Localized) => renderLocalized(language, value);

  const visible = useMemo(() => {
    const needle = query.trim().toLowerCase();
    return plugins.filter(plugin => {
      const enabled = isPluginEnabled(plugin, store);
      if (filter === 'enabled' && !enabled) return false;
      if (filter === 'disabled' && enabled) return false;
      if (filter === 'attention' && !pluginNeedsAttention(plugin, pendingRestart)) return false;
      if (!needle) return true;
      // 搜索同时匹配名称、作者和描述：只匹配名称时，用户想不起插件叫什么都找不到。
      const haystack = `${text(plugin.name)} ${text(plugin.description)} ${plugin.author} ${plugin.id}`.toLowerCase();
      return haystack.includes(needle);
    });
  }, [plugins, store, filter, query, pendingRestart, language]);

  const setEnabled = (plugin: LauncherPlugin, next: boolean) => {
    const enabled = { ...store.enabled, [plugin.id]: next };
    updateStore({ ...store, enabled });
    if (plugin.requiresRestart && !pendingRestart.includes(plugin.id)) {
      onPendingRestartChange([...pendingRestart, plugin.id]);
    }
    // 停用时收起详情：卡片变成灰底，展开区还留着会显得没生效。
    if (!next && expanded === plugin.id) setExpanded(null);
  };

  /**
   * 采纳插件的新版本。
   *
   * 本地没有在线插件源，所以这个动作的真实含义是「记录用户已接受清单声明的
   * 新版本」——不下载任何文件，只把待更新标记清掉。这样徽章数字才会减少，
   * 不会出现「点了更新但红点还在」的假成功。
   *
   * 内置插件通过 `acknowledgedUpdates` 记录（它的清单是代码常量，改对象不持久）；
   * 自定义插件直接改写 `custom` 里的条目。两条路都会落盘。
   *
   * 接入在线源后，这里会改为下载 + 校验 + 替换文件，届时才真正安装新版本。
   */
  const acknowledgeUpdate = (plugin: LauncherPlugin) => {
    if (!plugin.updateAvailable) return;
    const target = plugin.updateAvailable;
    if (plugin.builtin) {
      updateStore({
        ...store,
        acknowledgedUpdates: { ...(store.acknowledgedUpdates ?? {}), [plugin.id]: target },
      });
      onNotice(t('plugin.updated', { values: { name: text(plugin.name), version: target } }));
    } else {
      updateStore({
        ...store,
        custom: store.custom.map(item =>
          item.id === plugin.id ? { ...item, version: target, updateAvailable: undefined } : item,
        ),
      });
      onNotice(t('plugin.updated', { values: { name: text(plugin.name), version: target } }));
    }
  };

  const setOption = (plugin: LauncherPlugin, optionId: string, value: boolean | number) => {
    const perPlugin = { ...(store.options[plugin.id] ?? {}), [optionId]: value };
    updateStore({ ...store, options: { ...store.options, [plugin.id]: perPlugin } });
    /*
     * 镜像开关直接改真实设置。
     *
     * `preferBmclapi` 不是一个独立的偏好——「优先使用镜像源」在启动器里就对应
     * `downloadSource: 'bmclapi'`（或关闭时回到 `auto`）。只把值存进选项表会让这个
     * 开关变成装饰品：拨了它，下载源不变。这里把它映射到真正的设置上。
     */
    if (plugin.id === 'download-mirror' && optionId === 'preferBmclapi' && onDownloadSourceChange) {
      onDownloadSourceChange(value ? 'bmclapi' : 'auto');
    }
  };

  const remove = (plugin: LauncherPlugin) => {
    if (!window.confirm(t('plugin.removeConfirm', { values: { name: text(plugin.name) } }))) return;
    const enabled = { ...store.enabled };
    const options = { ...store.options };
    delete enabled[plugin.id];
    delete options[plugin.id];
    updateStore({ ...store, custom: store.custom.filter(item => item.id !== plugin.id), enabled, options });
    if (expanded === plugin.id) setExpanded(null);
    onPendingRestartChange(pendingRestart.filter(id => id !== plugin.id));
  };

  const closeInstall = () => {
    setInstallOpen(false);
    setManifestText('');
    setInstallError(null);
  };

  const submitInstall = () => {
    let raw: unknown;
    try {
      raw = JSON.parse(manifestText);
    } catch {
      setInstallError({ key: 'plugin.installInvalid' });
      return;
    }
    // 保留原始声明，用来发现「声明了扩展点但一个都不认识」的情况：只看归一化
    // 结果的话，拼错的 hooks 与没写 hooks 无法区分。
    const declaredHooks = Array.isArray((raw as { hooks?: unknown })?.hooks)
      ? ((raw as { hooks: unknown[] }).hooks.length)
      : 0;
    const declaredPermissions = Array.isArray((raw as { permissions?: unknown })?.permissions)
      ? ((raw as { permissions: unknown[] }).permissions.length)
      : 0;
    const plugin = normalizePlugin(raw);
    if (!plugin) {
      setInstallError({ key: 'plugin.installInvalid' });
      return;
    }
    if (plugins.some(item => item.id === plugin.id)) {
      setInstallError({ key: 'plugin.installDuplicate', values: { id: plugin.id } });
      return;
    }
    /*
     * 声明了扩展点却一个都没被识别 → 这个插件装上也不会做任何事。
     *
     * 规范允许未知的 hooks/permissions 被静默丢弃（这样新插件在旧启动器上仍能
     * 安装），但如果**全部**都丢了，静默安装一份「什么都不做」的插件会误导用户：
     * 他以为装成功了，实际上拼写错误让插件完全失效。这里明确拒绝，并指出问题。
     */
    if (declaredHooks > 0 && plugin.hooks.length === 0) {
      setInstallError({ key: 'plugin.installUnknownHooks' });
      return;
    }
    if (declaredPermissions > 0 && plugin.permissions.length === 0) {
      setInstallError({ key: 'plugin.installUnknownPermissions' });
      return;
    }
    updateStore({ ...store, custom: [...store.custom, plugin] });
    closeInstall();
  };

  const renderOption = (plugin: LauncherPlugin, option: PluginOption) => {
    /*
     * 镜像开关读真实设置而不是选项表。
     *
     * 这个开关的效果就是「下载源是不是 bmclapi」，所以它的显示状态必须由那个设置
     * 决定。读选项表会让两处对同一个事实给出不同答案——用户在设置页切了下载源，
     * 插件页的开关还停在旧值。
     */
    const mirrorsSetting = plugin.id === 'download-mirror' && option.id === 'preferBmclapi';
    const value = mirrorsSetting && downloadSource !== undefined
      ? (downloadSource === 'bmclapi' ? true : false)
      : optionValue(plugin, option.id, store);
    return (
      <div className={styles.option} key={option.id}>
        <div className={styles.optionCopy}>
          <strong>{text(option.label)}</strong>
          {option.hint && <span>{text(option.hint)}</span>}
        </div>
        {option.type === 'toggle' ? (
          <button
            type="button"
            role="switch"
            aria-checked={value === true}
            aria-label={text(option.label)}
            className={`${styles.switch} ${value === true ? styles.switchOn : ''}`}
            onClick={() => setOption(plugin, option.id, value !== true)}
          ><i /></button>
        ) : (
          <input
            type="number"
            min={option.min}
            max={option.max}
            value={Number(value)}
            aria-label={text(option.label)}
            onChange={event => {
              const next = Number(event.target.value);
              if (!Number.isFinite(next)) return;
              setOption(plugin, option.id, Math.min(option.max ?? 9999, Math.max(option.min ?? 0, Math.round(next))));
            }}
          />
        )}
      </div>
    );
  };

  return (
    <div className={styles.page}>
      <header className={styles.header}>
        <div className={styles.headerCopy}>
          <h1>{t('plugin.title')}</h1>
          <p>{t('plugin.subtitle')}</p>
          <div className={styles.counts}>
            <span>{t('plugin.installedCount', { count: plugins.length })}</span>
            <span>{t('plugin.enabledCount', { count: enabledCount })}</span>
          </div>
        </div>
      </header>

      {/*
        运行时状态。

        这段文字必须如实反映「插件会不会真的做事」：
          - 桌面版：插件声明的贡献会在启动时执行（JVM 参数、环境变量、启动前命令、
            启动前备份）。页头说明这一点，并把支持的贡献类型列出来。
          - 浏览器预览：没有后端，开关只保存配置。如实说明，而不是让用户以为它生效了。

        之前这里是一句写死的「没有插件运行时」——那在当时是对的，现在运行时已经
        实现，继续留着就是错误的说明。
      */}
      <div className={styles.capabilityNote} role="note">
        {runtime?.available ? (
          <ShieldCheck size={15} strokeWidth={1.9} aria-hidden="true" />
        ) : (
          <AlertTriangle size={15} strokeWidth={1.9} aria-hidden="true" />
        )}
        <span>
          {runtime === null
            ? t('plugin.runtimeChecking')
            : runtime.available
              ? t('plugin.runtimeActive', { values: { kinds: runtime.kinds.length } })
              : t('plugin.runtimeUnavailable')}
        </span>
      </div>

      <div className={styles.toolbar}>
        <label className={styles.search}>
          <Search size={14} />
          <input
            value={query}
            onChange={event => setQuery(event.target.value)}
            placeholder={t('plugin.searchPlaceholder')}
            aria-label={t('plugin.searchPlaceholder')}
          />
        </label>
        <div className={styles.filters} role="radiogroup" aria-label={t('plugin.filterAll')}>
          {(Object.keys(FILTER_KEYS) as FilterId[]).map(id => (
            <button
              key={id}
              type="button"
              role="radio"
              aria-checked={filter === id}
              className={`${styles.filter} ${filter === id ? styles.filterActive : ''}`}
              onClick={() => setFilter(id)}
            >
              {t(FILTER_KEYS[id])}
            </button>
          ))}
        </div>
        <button
          type="button"
          className={styles.install}
          aria-label={t('plugin.installAria')}
          onClick={() => { setInstallOpen(true); setInstallError(null); }}
        >
          <Plug size={13} /> {t('plugin.install')}
        </button>
      </div>

      <div className={styles.list}>
        {pendingRestart.length > 0 && (
          <p className={styles.notice} role="status">
            <RotateCcw size={13} />
            {t('plugin.restartHint', { count: pendingRestart.length })}
          </p>
        )}

        {visible.length === 0 ? (
          <div className={styles.empty}>
            <Plug size={40} strokeWidth={1.4} />
            <h2>{plugins.length === 0 ? t('plugin.emptyTitle') : t('plugin.filterEmpty')}</h2>
            <p>{plugins.length === 0 ? t('plugin.emptyHint') : t('plugin.filterEmptyHint')}</p>
          </div>
        ) : (
          <div className={styles.cards}>
            {visible.map(plugin => {
              const enabled = isPluginEnabled(plugin, store);
              const Glyph = GLYPH_ICONS[plugin.glyph] ?? Plug;
              const open = expanded === plugin.id;
              const name = text(plugin.name);
              return (
                <article key={plugin.id} className={`${styles.card} ${enabled ? '' : styles.cardOff}`}>
                  <div className={styles.cardHead}>
                    <span
                      className={`${styles.glyph} ${enabled ? '' : styles.glyphOff}`}
                      /* 插件品牌色只做图标底，不参与文字对比度计算。 */
                      style={{ background: plugin.accent }}
                      aria-hidden="true"
                    >
                      <Glyph size={19} strokeWidth={1.9} />
                    </span>
                    <div className={styles.headCopy}>
                      <div className={styles.nameLine}>
                        <span className={styles.name} title={name}>{name}</span>
                        <span className={styles.tag}>{t(plugin.builtin ? 'plugin.builtin' : 'plugin.thirdParty')}</span>
                        {plugin.incompatible && (
                          <span className={`${styles.tag} ${styles.tagError}`}>{t('plugin.incompatible')}</span>
                        )}
                        {plugin.requiresRestart && pendingRestart.includes(plugin.id) && (
                          <span className={`${styles.tag} ${styles.tagWarn}`}>{t('plugin.restartNeeded')}</span>
                        )}
                      </div>
                      <div className={styles.meta}>
                        {t('plugin.version', { values: { version: plugin.version } })} · {t('plugin.byAuthor', { values: { author: plugin.author } })}
                      </div>
                    </div>
                  </div>

                  <p className={styles.description}>{text(plugin.description)}</p>

                  {/*
                    可更新提示放在状态行之前：它是可执行的事，比说明性文字更该先被看到。
                    用独立样式而不是复用 statusLine——后者是「这个插件有什么情况」，
                    这里是「你可以做一件事」，性质不同。
                  */}
                  {plugin.updateAvailable && (
                    <p className={styles.updateLine}>
                      <ArrowUpCircle size={11} />
                      <span>{t('plugin.updateAvailable', { values: { version: plugin.updateAvailable } })}</span>
                    </p>
                  )}

                  {plugin.status && (
                    <p className={`${styles.statusLine} ${plugin.incompatible || plugin.statusTone === 'error' ? styles.statusLineError : ''}`}>
                      <AlertTriangle size={11} />
                      <span>{text(plugin.status)}</span>
                    </p>
                  )}

                  <div className={styles.summary}>
                    <span><Plug size={11} /> {t('plugin.hooksCount', { count: plugin.hooks.length })}</span>
                    <span><ShieldCheck size={11} /> {plugin.permissions.length ? t('plugin.permCount', { count: plugin.permissions.length }) : t('plugin.noPermissions')}</span>
                  </div>

                  <div className={styles.cardFoot}>
                    <span className={`${styles.state} ${enabled ? styles.stateOn : ''}`}>
                      <i className={styles.stateDot} />
                      {plugin.incompatible
                        ? t('plugin.incompatible')
                        : enabled ? t('plugin.enabled') : t('plugin.disabled')}

                    </span>
                    <div className={styles.footActions}>
                      {/*
                        更新按钮只在确实有新版本时出现。
                        本地没有在线插件源，所以点击「更新」表示采纳清单里声明的新版本号
                        （清除 updateAvailable 并写入新版本）——这是离线环境下能做到的
                        诚实行为：不下载任何东西，只把「待更新」这个状态消掉。
                        接入在线源后这里会改成真正的下载 + 替换。
                      */}
                      {plugin.updateAvailable && (
                        <button
                          type="button"
                          className={`${styles.ghostButton} ${styles.updateButton}`}
                          onClick={() => acknowledgeUpdate(plugin)}
                        >
                          <ArrowUpCircle size={12} />
                          {t('plugin.update')}
                        </button>
                      )}
                      {!plugin.builtin && (
                        <button
                          type="button"
                          className={`${styles.ghostButton} ${styles.dangerButton}`}
                          onClick={() => remove(plugin)}
                          title={t('plugin.remove')}
                          aria-label={`${t('plugin.remove')} ${name}`}
                        >
                          <Trash2 size={12} />
                        </button>
                      )}
                      <button
                        type="button"
                        className={`${styles.ghostButton} ${open ? styles.ghostButtonOpen : ''}`}
                        onClick={() => setExpanded(open ? null : plugin.id)}
                        aria-expanded={open}
                        aria-label={t('plugin.detailsAria', { values: { name } })}
                      >
                        {open ? t('plugin.collapse') : t('plugin.expand')}
                        <ChevronDown size={12} />
                      </button>
                      <button
                        type="button"
                        role="switch"
                        aria-checked={enabled}
                        aria-label={t('plugin.toggleAria', { values: { name } })}
                        className={`${styles.switch} ${enabled ? styles.switchOn : ''}`}
                        disabled={plugin.incompatible}
                        onClick={() => setEnabled(plugin, !enabled)}
                      ><i /></button>
                    </div>
                  </div>

                  {open && (
                    <div className={styles.details}>
                      <div className={styles.block}>
                        <div className={styles.blockTitle}>
                          {t('plugin.hooks')}
                          <small>{t('plugin.hooksHint')}</small>
                        </div>
                        <div className={styles.chips}>
                          {plugin.hooks.length === 0
                            ? <span className={styles.chip}>{t('common.none')}</span>
                            : plugin.hooks.map(hook => (
                              <span className={`${styles.chip} ${styles.chipStrong}`} key={hook} title={text(hookHint(hook))}>
                                {text(hookLabel(hook))}
                              </span>
                            ))}
                        </div>
                      </div>

                      <div className={styles.block}>
                        <div className={styles.blockTitle}>
                          {t('plugin.permissions')}
                          <small>{t('plugin.permissionsHint')}</small>
                        </div>
                        {plugin.permissions.length === 0 ? (
                          <p className={styles.summary}><Ban size={11} /> {t('plugin.noPermissions')}</p>
                        ) : plugin.permissions.map(permission => (
                          <div className={styles.permissionRow} key={permission}>
                            <ShieldCheck size={12} />
                            <div>
                              <strong>{text(permissionLabel(permission))}</strong>
                              <span>{text(permissionHint(permission))}</span>
                            </div>
                          </div>
                        ))}
                      </div>

                      {plugin.options && plugin.options.length > 0 && (
                        <div className={styles.block}>
                          <div className={styles.blockTitle}>{t('plugin.options')}</div>
                          {plugin.options.map(option => renderOption(plugin, option))}
                        </div>
                      )}

                      {/*
                        贡献预览。
                        这是「这个插件会不会真的做事」的直接答案：展开就能看到本次启动
                        会应用几条、跳过几条，以及跳过原因（缺权限、缺扩展点、参数不被
                        允许）。

                        为什么不看 `plugin.contributions` 是否有值：内置插件的贡献定义
                        在后端（前端清单只带展示字段），用它做条件会让内置插件永远不显示
                        这一块。真正的来源是后端的 `plugin_preview`——它对「没有贡献」也
                        会返回空列表，那就如实显示「启动时不贡献任何东西」。
                      */}
                      {runtime?.available && (
                        <div className={styles.block}>
                          <div className={styles.blockTitle}>
                            {t('plugin.contributions')}
                            <small>{t('plugin.contributionsHint')}</small>
                          </div>
                          {preview?.id === plugin.id ? (
                            preview.data.applied.length === 0 && preview.data.skipped.length === 0 ? (
                              <p className={styles.summary}>
                                <Ban size={11} />
                                {t('plugin.contributionsNone')}
                              </p>
                            ) : (
                              <>
                                {preview.data.applied.map((item, index) => (
                                  <div className={styles.contributionRow} key={`a-${index}`}>
                                    <ShieldCheck size={12} />
                                    <div>
                                      <strong>{kindLabel(item.kind, t)}</strong>
                                      <span>{item.summary}</span>
                                    </div>
                                  </div>
                                ))}
                                {preview.data.skipped.map((item, index) => (
                                  <div className={`${styles.contributionRow} ${styles.contributionSkipped}`} key={`s-${index}`}>
                                    <Ban size={12} />
                                    <div>
                                      <strong>{kindLabel(item.kind, t)}</strong>
                                      <span>{item.reason}</span>
                                    </div>
                                  </div>
                                ))}
                              </>
                            )
                          ) : (
                            <p className={styles.summary}>{t('common.loading')}</p>
                          )}
                        </div>
                      )}

                      {/*
                        插件声明了贡献，但当前环境读不到后端。
                        说明它们已随插件保存、会在桌面版启动时应用，并把内容列出来——
                        只说「不会被执行」读起来像插件坏了，而实际上它安装得好好的。
                      */}
                      {!runtime?.available && plugin.contributions && plugin.contributions.length > 0 && (
                        <div className={styles.block}>
                          <div className={styles.blockTitle}>{t('plugin.contributions')}</div>
                          {plugin.contributions.map((item, index) => (
                            <div className={styles.contributionRow} key={index}>
                              <ShieldCheck size={12} />
                              <div>
                                <strong>{kindLabel(item.kind, t)}</strong>
                                <span>{item.value}</span>
                              </div>
                            </div>
                          ))}
                          <p className={styles.summary}>
                            <AlertTriangle size={11} />
                            {t('plugin.contributionsPreviewOnly', { count: plugin.contributions.length })}
                          </p>
                        </div>
                      )}
                    </div>
                  )}
                </article>
              );
            })}
          </div>
        )}

        <p className={`${styles.notice} ${styles.noticeFooter}`}>
          <ShieldCheck size={13} />
          {t('plugin.notice')}
        </p>

        {/*
          启动记录。
          这是运行时「真的生效了」最直接的证据：每次启动时哪个插件贡献了什么、
          哪些被跳过、跳过的原因。没有它，用户只能靠观察副作用来猜插件是否工作。
          只在有运行时的环境显示——预览模式没有后端，读不到历史。
        */}
        {runtime?.available && (
          <section className={styles.history}>
            <div className={styles.historyHead}>
              <div>
                <h2>{t('plugin.historyTitle')}</h2>
                <p>{t('plugin.historyHint')}</p>
              </div>
              {history.length > 0 && (
                <button
                  type="button"
                  className={styles.historyClear}
                  onClick={() => void clearHistory()}
                >
                  <Trash2 size={12} />{t('common.delete')}
                </button>
              )}
            </div>
            {history.length === 0 ? (
              <p className={styles.historyEmpty}>{t('plugin.historyEmpty')}</p>
            ) : (
              <ul className={styles.historyList}>
                {history.slice(0, 8).map(entry => {
                  const applied = entry.contributions?.applied?.length ?? 0;
                  const skipped = entry.contributions?.skipped?.length ?? 0;
                  const outcomeKey = entry.outcome === 'crashed'
                    ? 'plugin.historyOutcome.crashed'
                    : entry.outcome === 'running'
                      ? 'plugin.historyOutcome.running'
                      : 'plugin.historyOutcome.stopped';
                  return (
                    <li key={entry.id} className={styles.historyRow}>
                      <span className={`${styles.historyDot} ${entry.outcome === 'crashed' ? styles.historyDotBad : ''}`} aria-hidden="true" />
                      <div className={styles.historyCopy}>
                        <strong>{entry.instanceName}</strong>
                        <span>
                          {new Date(entry.startedAt).toLocaleString(locale, {
                            month: 'short',
                            day: 'numeric',
                            hour: '2-digit',
                            minute: '2-digit',
                          })}
                          {' · '}
                          {t(outcomeKey)}
                          {entry.exitCode !== null && ` (${entry.exitCode})`}
                        </span>
                      </div>
                      <span className={styles.historyCounts}>
                        {t('plugin.historyContributions', { values: { applied, skipped } })}
                      </span>
                    </li>
                  );
                })}
              </ul>
            )}
          </section>
        )}
      </div>

      <Modal
        isOpen={installOpen}
        onClose={closeInstall}
        title={t('plugin.installTitle')}
        size="md"
        footer={(
          <div className={styles.modalActions}>
            <button type="button" className={styles.modalButton} onClick={closeInstall}>{t('common.cancel')}</button>
            <button type="button" className={`${styles.modalButton} ${styles.modalPrimary}`} onClick={submitInstall}>
              {t('plugin.installConfirm')}
            </button>
          </div>
        )}
      >
        <p className={styles.installHint}>{t('plugin.installDescription')}</p>
        <textarea
          ref={manifestRef}
          className={styles.manifest}
          value={manifestText}
          placeholder={t('plugin.installPlaceholder')}
          aria-label={t('plugin.installPlaceholder')}
          spellCheck={false}
          onChange={event => { setManifestText(event.target.value); if (installError) setInstallError(null); }}
        />
        {installError && (
          <p className={styles.error} role="alert">
            <AlertTriangle size={13} /> {renderLocalized(language, installError)}
          </p>
        )}
      </Modal>
    </div>
  );
}

export default PluginWorkspace;
