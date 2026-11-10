// 插件源浏览面板：列出源里的插件，并让用户安装其中一个。
//
// ## 设计取舍：为什么源里的插件必须逐条展示权限
//
// 用户在这里看到的是一份**别人写的清单**。如果只显示「名字 + 描述 + 安装按钮」，
// 那么「这个插件能改我的启动参数」这件事就被藏在安装之后了——而安装之后，用户
// 已经提交了自己的信任。所以每一条都带权限摘要，`launchHook` / `filesystem` 这类
// 会真正改变机器状态的项单独标红。
//
// ## 为什么源不能改变插件的实际权限
//
// 索引里的 `permissions` 是**告知性**的，真正的权限来自包内清单（后端独立校验）。
// 一个被篡改的索引可以少写权限，让用户以为它很安全；但装进来之后，插件实际能做
// 什么仍然取决于包里的清单，而界面在安装后展示的是包里那份。这个不对称是刻意的：
// 索引骗到的只是「点击前的那一刻」，不是「安装后的能力」。
//
// ## 只读 HTTPS
//
// 索引与包地址都由后端强制 HTTPS（含重定向后再检查一次）。前端不重复实现这条
// 规则，只在添加源时先挡一次，让用户立刻得到反馈。

import { useCallback, useEffect, useState } from 'react';
import { AlertTriangle, Download, Loader2, Plus, RefreshCw, Trash2 } from 'lucide-react';
import { useI18n } from '../utils/i18n';
import { errorText, isDesktopShell } from '../utils/native';
import {
  BUILTIN_PLUGIN_SOURCE_URL,
  fetchPluginIndex,
  installPluginFromUrl,
  normalizeSourceUrl,
  readPluginSources,
  savePluginSources,
  validateSourceUrl,
  type PluginSource,
  type PluginSourceIndex,
  type PluginIndexEntry,
  type InstalledPluginRecord,
} from '../utils/pluginInstall';
import styles from './PluginWorkspace.module.css';

interface PluginSourcePanelProps {
  onNotice: (message: string) => void;
  onInstalled: (record: InstalledPluginRecord) => void;
  installedIds: Set<string>;
}

/** 一个源加载后的状态。三态并列，避免 `index === null && loading === false` 的歧义。 */
type SourceState =
  | { status: 'loading' }
  | { status: 'ready'; index: PluginSourceIndex }
  | { status: 'failed'; reason: string };

export function PluginSourcePanel({ onNotice, onInstalled, installedIds }: PluginSourcePanelProps) {
  const { t } = useI18n();
  const [sources, setSources] = useState<PluginSource[]>(readPluginSources);
  const [states, setStates] = useState<Record<string, SourceState>>({});
  const [addOpen, setAddOpen] = useState(false);
  const [draftUrl, setDraftUrl] = useState('');
  const [draftName, setDraftName] = useState('');
  const [installing, setInstalling] = useState<string | null>(null);
  const desktop = isDesktopShell();

  const allSources: PluginSource[] = [
    { url: BUILTIN_PLUGIN_SOURCE_URL, name: t('plugin.sourceBuiltin'), custom: false },
    ...sources,
  ];

  const load = useCallback(async (source: PluginSource) => {
    setStates(current => ({ ...current, [source.url]: { status: 'loading' } }));
    try {
      const index = await fetchPluginIndex(source.url);
      setStates(current => ({ ...current, [source.url]: { status: 'ready', index } }));
    } catch (error) {
      setStates(current => ({
        ...current,
        [source.url]: { status: 'failed', reason: errorText(error, '') },
      }));
    }
  }, []);

  /*
   * 打开面板时拉一次全部源。
   *
   * 每个源独立失败：一个源挂了不该让整页显示成「读不到」。这也是 `states` 按
   * URL 分开存的原因——共享一个 loading 标志会把「A 好了、B 还在读」压成一种状态。
   */
  useEffect(() => {
    if (!desktop) {
      // 预览模式没有主进程，`fetchPluginIndex` 会抛错。这里直接给出明确状态，
      // 而不是停在“正在读取…”——一个永远转圈的加载态比一句「需要桌面版」更糟。
      setStates(
        Object.fromEntries(
          allSources.map(source => [
            source.url,
            { status: 'failed' as const, reason: t('plugin.sourceNeedsDesktop') },
          ]),
        ) as Record<string, SourceState>,
      );
      return;
    }
    for (const source of allSources) void load(source);
    // `allSources` 每次渲染都是新数组，依赖它会导致无限拉取；用长度与内容签名。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [desktop, sources.length, load]);

  const addSource = () => {
    const candidate = normalizeSourceUrl(draftUrl);
    const problem = validateSourceUrl(candidate);
    if (problem) {
      onNotice(t('plugin.sourceInvalid', { values: { reason: problem } }));
      return;
    }
    if (allSources.some(source => source.url === candidate)) {
      onNotice(t('plugin.sourceExists'));
      return;
    }
    const name = draftName.trim() || candidate.replace(/^https:\/\//, '').slice(0, 48);
    const next = [...sources, { url: candidate, name, custom: true }];
    setSources(next);
    savePluginSources(next);
    onNotice(t('plugin.sourceAdded', { values: { name } }));
    setDraftUrl('');
    setDraftName('');
    setAddOpen(false);
  };

  const removeSource = (url: string) => {
    const next = sources.filter(source => source.url !== url);
    setSources(next);
    savePluginSources(next);
    setStates(current => {
      const copy = { ...current };
      delete copy[url];
      return copy;
    });
  };

  const install = async (entry: PluginIndexEntry) => {
    setInstalling(entry.id);
    try {
      onInstalled(await installPluginFromUrl(entry.download, entry.sha256 ?? null));
    } catch (error) {
      onNotice(errorText(error, t('plugin.installInvalid')));
    } finally {
      setInstalling(null);
    }
  };

  return (
    <div className={styles.sourcePanel}>
      <header className={styles.sourceHeader}>
        <div>
          <h2>{t('plugin.sourceSection')}</h2>
          <p>{t('plugin.sourceAddHint')}</p>
        </div>
        <button type="button" className={styles.importAction} onClick={() => setAddOpen(open => !open)}>
          <Plus size={13} /> {t('plugin.sourceAdd')}
        </button>
      </header>

      {addOpen && (
        <div className={styles.sourceAdd}>
          <label>
            <span>{t('plugin.sourceAddTitle')}</span>
            <input
              value={draftUrl}
              onChange={event => setDraftUrl(event.target.value)}
              placeholder={t('plugin.sourceUrlPlaceholder')}
              spellCheck={false}
              autoFocus
            />
          </label>
          <label>
            <span>{t('plugin.sourceCustom')}</span>
            <input
              value={draftName}
              onChange={event => setDraftName(event.target.value)}
              placeholder={t('plugin.sourceCustom')}
              maxLength={64}
            />
          </label>
          <button type="button" onClick={addSource} disabled={!draftUrl.trim()}>
            {t('plugin.sourceAdd')}
          </button>
        </div>
      )}

      {!desktop && (
        <p className={styles.sourceWarning}>
          <AlertTriangle size={13} /> {t('plugin.thirdPartyNoSandbox')}
        </p>
      )}

      {allSources.map(source => {
        const state = states[source.url];
        return (
          <section key={source.url} className={styles.sourceGroup}>
            <header className={styles.sourceGroupHeader}>
              <div>
                <strong>{source.name}</strong>
                <code>{source.url}</code>
              </div>
              <div className={styles.sourceGroupActions}>
                <button type="button" onClick={() => void load(source)} title={t('plugin.sourceRefresh')}>
                  <RefreshCw size={12} />
                </button>
                {source.custom && (
                  <button
                    type="button"
                    onClick={() => removeSource(source.url)}
                    title={t('plugin.sourceRemove')}
                  >
                    <Trash2 size={12} />
                  </button>
                )}
              </div>
            </header>

            {(!state || state.status === 'loading') && (
              <p className={styles.sourceLoading}>
                <Loader2 size={13} className={styles.spin} /> {t('plugin.sourceLoading')}
              </p>
            )}
            {state?.status === 'failed' && (
              <p className={styles.sourceError}>
                {t('plugin.sourceFailed', { values: { reason: state.reason } })}
              </p>
            )}

            {state?.status === 'ready' && state.index.plugins.length === 0 && (
              <p className={styles.sourceLoading}>{t('plugin.sourceEmpty')}</p>
            )}

            {state?.status === 'ready' && state.index.plugins.length > 0 && (
              <ul className={styles.sourceList}>
                {state.index.plugins.map(entry => {
                  const installed = installedIds.has(entry.id);
                  return (
                    <li key={entry.id}>
                      <div className={styles.sourceItemCopy}>
                        <strong>{entry.name || entry.id}</strong>
                        <span>
                          {entry.version} · {entry.author || 'unknown'}
                        </span>
                        {entry.description && <p>{entry.description}</p>}
                        {/*
                          权限摘要。空数组要说「不需要特殊权限」而不是留白：
                          留白会让用户以为「还没加载出来」，而实际上它什么都没要。
                        */}
                        <em>
                          {entry.permissions.length > 0
                            ? t('plugin.sourcePermissions', { values: { list: entry.permissions.join('、') } })
                            : t('plugin.sourceNoPermissions')}
                        </em>
                      </div>
                      <button
                        type="button"
                        className={styles.sourceInstallButton}
                        disabled={installed || installing === entry.id}
                        onClick={() => void install(entry)}
                      >
                        {installing === entry.id ? (
                          <Loader2 size={12} className={styles.spin} />
                        ) : (
                          <Download size={12} />
                        )}
                        {installed ? t('plugin.sourceInstalled') : t('plugin.sourceInstall')}
                      </button>
                    </li>
                  );
                })}
              </ul>
            )}
          </section>
        );
      })}
    </div>
  );
}
