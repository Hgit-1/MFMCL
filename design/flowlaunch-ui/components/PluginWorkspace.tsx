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
  type LauncherPlugin,
  type PluginGlyph,
  type PluginOption,
  type PluginStore,
} from '../utils/plugins';
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
}

export function PluginWorkspace({ store, onChange, pendingRestart, onPendingRestartChange }: PluginWorkspaceProps) {
  const { t, language } = useI18n();
  const [query, setQuery] = useState('');
  const [filter, setFilter] = useState<FilterId>('all');
  const [expanded, setExpanded] = useState<string | null>(null);
  const [installOpen, setInstallOpen] = useState(false);
  const [manifestText, setManifestText] = useState('');
  const [installError, setInstallError] = useState<Localized | null>(null);
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
    onChange({ ...store, enabled });
    if (plugin.requiresRestart && !pendingRestart.includes(plugin.id)) {
      onPendingRestartChange([...pendingRestart, plugin.id]);
    }
    // 停用时收起详情：卡片变成灰底，展开区还留着会显得没生效。
    if (!next && expanded === plugin.id) setExpanded(null);
  };

  const setOption = (plugin: LauncherPlugin, optionId: string, value: boolean | number) => {
    const perPlugin = { ...(store.options[plugin.id] ?? {}), [optionId]: value };
    onChange({ ...store, options: { ...store.options, [plugin.id]: perPlugin } });
  };

  const remove = (plugin: LauncherPlugin) => {
    if (!window.confirm(t('plugin.removeConfirm', { values: { name: text(plugin.name) } }))) return;
    const enabled = { ...store.enabled };
    const options = { ...store.options };
    delete enabled[plugin.id];
    delete options[plugin.id];
    onChange({ ...store, custom: store.custom.filter(item => item.id !== plugin.id), enabled, options });
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
    const plugin = normalizePlugin(raw);
    if (!plugin) {
      setInstallError({ key: 'plugin.installInvalid' });
      return;
    }
    if (plugins.some(item => item.id === plugin.id)) {
      setInstallError({ key: 'plugin.installDuplicate', values: { id: plugin.id } });
      return;
    }
    onChange({ ...store, custom: [...store.custom, plugin] });
    closeInstall();
  };

  const renderOption = (plugin: LauncherPlugin, option: PluginOption) => {
    const value = optionValue(plugin, option.id, store);
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
