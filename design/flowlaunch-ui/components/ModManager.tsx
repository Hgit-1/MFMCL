// 实例的模组管理页。
//
// 这一页回答三个问题：装了哪些、哪些停用了、哪些有问题/能升级。
//
// 设计上的取舍：
//   - 列表是主视图，一屏能看到全部文件。停用项不隐藏（灰化 + 标记），因为
//     「我明明装了却看不到」比「看到但灰着」更让人困惑。
//   - 兼容性检查（`inspect_mods`）需要读 JAR 元数据，代价不小，所以不自动跑，
//     由用户点「检查问题」触发，结果按文件归并展示。
//   - 检查更新（`check_mod_updates`）会把文件哈希发给 Modrinth。这是隐私决定，
//     必须由用户明确点击，并在按钮旁写清后果——不能因为「更有用」就默认执行。
//   - 更新是逐条的：一次只更新一个模组，成功后刷新列表。批量更新会让失败时
//     难以判断是哪一条坏了，也无法在中途停下。

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import {
  AlertTriangle,
  Download,
  Fingerprint,
  RefreshCw,
  ShieldAlert,
  ShieldCheck,
  ToggleLeft,
  ToggleRight,
} from 'lucide-react';
import { Button } from './Button';
import { useI18n } from '../utils/i18n';
import {
  checkModUpdates,
  curseforgeIdentifyMods,
  errorText,
  formatBytes,
  inspectMods,
  listMods,
  setModEnabled,
  updateMod,
  type IdentifiedMod,
  type ModFile,
  type ModIssue,
  type ModUpdateReport,
} from '../utils/native';
import type { GameInstance } from '../types';
import styles from './ModManager.module.css';

interface ModManagerProps {
  instance: GameInstance;
  /** 操作结果与错误统一走父组件的提示条。 */
  onNotice: (message: string) => void;
  /** 模组增删后实例卡片上的计数需要刷新。 */
  onChanged: () => void;
}

type LoadState = 'loading' | 'ready' | 'error';

export function ModManager({ instance, onNotice, onChanged }: ModManagerProps) {
  const { t } = useI18n();
  const [files, setFiles] = useState<ModFile[]>([]);
  const [state, setState] = useState<LoadState>('loading');
  const [loadError, setLoadError] = useState('');
  /** 正在切换的单个文件；用于禁用该行开关，避免连点重复提交。 */
  const [toggling, setToggling] = useState('');
  const [issues, setIssues] = useState<ModIssue[] | null>(null);
  const [inspecting, setInspecting] = useState(false);
  const [updates, setUpdates] = useState<ModUpdateReport[] | null>(null);
  const [checking, setChecking] = useState(false);
  const [updating, setUpdating] = useState('');
  /** 用户是否已确认「把哈希发给 Modrinth」。点过之后本次页面内不再重复询问。 */
  const [hashConsent, setHashConsent] = useState(false);
  /**
   * 本地文件 → CurseForge 工程。`null` 表示还没查过。
   *
   * 与「检查更新」分开：识别只把哈希发给 CurseForge，且结果能直接改进列表的
   * 可读性（文件名 → 工程名），所以由用户点一次「识别来源」触发，不自动跑——
   * 一个几百模组的实例要读上 GB 的文件才算得出指纹。
   */
  const [sources, setSources] = useState<IdentifiedMod[] | null>(null);
  const [identifying, setIdentifying] = useState(false);

  const scope = useRef({ instanceId: instance.id, revision: 0 });
  const listRequest = useRef(0);
  const mutation = useRef<object | null>(null);

  useLayoutEffect(() => {
    scope.current = { instanceId: instance.id, revision: 0 };
    mutation.current = null;
    setFiles([]);
    setIssues(null);
    setUpdates(null);
    setSources(null);
    setHashConsent(false);
    setToggling('');
    setUpdating('');
    setInspecting(false);
    setChecking(false);
    setIdentifying(false);
    return () => { scope.current = { instanceId: '', revision: 0 }; };
  }, [instance.id]);

  const refresh = useCallback(async (quiet = false) => {
    const owner = scope.current;
    const request = ++listRequest.current;
    if (!quiet) setState('loading');
    setLoadError('');
    try {
      const found = await listMods(instance.id);
      if (scope.current !== owner || listRequest.current !== request) return;
      setFiles(found);
      setState('ready');
    } catch (error) {
      if (scope.current !== owner || listRequest.current !== request) return;
      setLoadError(errorText(error, ''));
      setState('error');
    }
  }, [instance.id]);

  useEffect(() => { void refresh(); }, [refresh]);

  const invalidateReports = () => {
    scope.current.revision += 1;
    setIssues(null);
    setUpdates(null);
    setSources(null);
    setInspecting(false);
    setChecking(false);
    setIdentifying(false);
  };

  const enabledCount = useMemo(() => files.filter(file => file.enabled).length, [files]);

  /** 文件名 → 该文件相关的问题。同一个问题可能涉及多个文件，两边都要能查到。 */
  const issuesByFile = useMemo(() => {
    const map = new Map<string, ModIssue[]>();
    (issues ?? []).forEach(issue => {
      issue.files.forEach(name => {
        map.set(name, [...(map.get(name) ?? []), issue]);
      });
    });
    return map;
  }, [issues]);

  /** 文件名 → 可用更新。 */
  const updateByFile = useMemo(() => {
    const map = new Map<string, Extract<ModUpdateReport['status'], { kind: 'updateAvailable' }>>();
    (updates ?? []).forEach(report => {
      if (report.status.kind === 'updateAvailable') map.set(report.fileName, report.status);
    });
    return map;
  }, [updates]);

  /**
   * 文件名 → CurseForge 来源。
   *
   * 用完整文件名做键（含 `.jar.disabled`）：后端扫描的也是 `list_mods` 的同一份
   * 名字，两边口径一致。不能在这里自行去掉 `.disabled` 后缀去匹配——同一个模组
   * 的启用版与停用版是两个不同的文件，混用会把停用版的信息挂到启用版上。
   */
  const sourceByFile = useMemo(() => {
    const map = new Map<string, IdentifiedMod>();
    (sources ?? []).forEach(item => map.set(item.fileName, item));
    return map;
  }, [sources]);

  /**
   * 识别本地模组的 CurseForge 来源。
   *
   * 这里不弹「会把哈希发出去吗」之类的二次确认：哈希是单向的、只用于反查，
   * 而且真正需要披露的是「检查更新」那一步（它决定要不要下载）。识别只读不写，
   * 多加一次确认只会让人放弃这个功能。
   */
  const runIdentify = async () => {
    if (mutation.current || identifying) return;
    const owner = scope.current;
    const revision = owner.revision;
    const fresh = () => scope.current === owner && owner.revision === revision;
    setIdentifying(true);
    try {
      /*
       * 不再先查 key。
       *
       * 后端在没有 key 时会走 MCIM 镜像，仍然能识别来源。之前的「先查 key 再
       * 决定是否继续」会让功能在有镜像的情况下也停在一个假的前提上：它假设
       * 「没有 key 就识别不了」，而这条现在不成立了。
       */
      const found = await curseforgeIdentifyMods(instance.id);
      if (!fresh()) return;
      setSources(found);
      onNotice(
        found.length === 0
          ? t('mods.identifyNone')
          : t('mods.identifyCount', { count: found.length }),
      );
    } catch (error) {
      if (fresh()) onNotice(errorText(error, t('mods.identifyFailed')));
    } finally {
      if (fresh()) setIdentifying(false);
    }
  };

  const toggle = async (file: ModFile) => {
    if (mutation.current || !files.includes(file)) return;
    const owner = scope.current;
    const operation = {};
    mutation.current = operation;
    setToggling(file.fileName);
    invalidateReports();
    try {
      await setModEnabled(instance.id, file.fileName, !file.enabled);
      if (scope.current !== owner) return;
      // Renaming changes the backend key; never leave an actionable row with the old name.
      await refresh(true);
      onChanged();
    } catch (error) {
      if (scope.current === owner) onNotice(errorText(error, t('mods.toggleFailed')));
    } finally {
      if (mutation.current === operation) {
        mutation.current = null;
        setToggling('');
      }
    }
  };

  const runInspect = async () => {
    if (mutation.current || inspecting) return;
    const owner = scope.current;
    const revision = owner.revision;
    const fresh = () => scope.current === owner && owner.revision === revision;
    setInspecting(true);
    try {
      const found = await inspectMods(instance.id);
      if (!fresh()) return;
      setIssues(found);
      onNotice(found.length === 0 ? t('mods.noIssues') : t('mods.issueCount', { count: found.length }));
    } catch (error) {
      if (fresh()) onNotice(errorText(error, t('mods.inspectFailed')));
    } finally {
      if (fresh()) setInspecting(false);
    }
  };

  const runUpdateCheck = async () => {
    if (mutation.current || checking) return;
    const owner = scope.current;
    const revision = owner.revision;
    const fresh = () => scope.current === owner && owner.revision === revision;
    if (!hashConsent) {
      setHashConsent(true);
      // 第一次点击只做披露，不发起请求。第二次点击才真正查询。
      return;
    }
    setChecking(true);
    try {
      const reports = await checkModUpdates(instance.id);
      if (!fresh()) return;
      setUpdates(reports);
      const available = reports.filter(report => report.status.kind === 'updateAvailable').length;
      onNotice(available === 0 ? t('mods.allUpToDate') : t('mods.updateCount', { count: available }));
    } catch (error) {
      if (fresh()) onNotice(errorText(error, t('mods.checkUpdatesFailed')));
    } finally {
      if (fresh()) setChecking(false);
    }
  };

  const applyUpdate = async (fileName: string) => {
    const entry = updateByFile.get(fileName);
    if (mutation.current || !entry || entry.kind !== 'updateAvailable') return;
    const owner = scope.current;
    const operation = {};
    mutation.current = operation;
    setUpdating(fileName);
    invalidateReports();
    try {
      await updateMod(instance.id, entry.update);
      if (scope.current !== owner) return;
      onNotice(t('mods.updated', { values: { name: entry.update.newFileName } }));
      await refresh(true);
      onChanged();
    } catch (error) {
      if (scope.current === owner) onNotice(errorText(error, t('mods.updateFailed')));
    } finally {
      if (mutation.current === operation) {
        mutation.current = null;
        setUpdating('');
      }
    }
  };

  if (state === 'loading') {
    return <div className={styles.empty}>{t('common.loading')}</div>;
  }

  if (state === 'error') {
    return (
      <div className={styles.empty} role="alert">
        <AlertTriangle size={20} />
        <p>{loadError || t('mods.listReadFailed')}</p>
        <Button size="sm" variant="ghost" icon={<RefreshCw size={13} />} onClick={() => void refresh()}>
          {t('common.retry')}
        </Button>
      </div>
    );
  }

  if (files.length === 0) {
    return (
      <div className={styles.empty}>
        <p>{t('mods.empty')}</p>
        <p className={styles.hint}>{t('mods.emptyHint')}</p>
      </div>
    );
  }

  return (
    <div className={styles.page}>
      <div className={styles.toolbar}>
        <span className={styles.summary}>
          {t('mods.summary', { values: { enabled: enabledCount, total: files.length } })}
        </span>
        <div className={styles.actions}>
          <Button
            size="sm"
            variant="ghost"
            icon={<Fingerprint size={13} />}
            loading={identifying}
            onClick={() => void runIdentify()}
          >
            {t('mods.identify')}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            icon={<ShieldCheck size={13} />}
            loading={inspecting}
            onClick={() => void runInspect()}
          >
            {t('mods.checkIssues')}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            icon={<Download size={13} />}
            loading={checking}
            onClick={() => void runUpdateCheck()}
          >
            {hashConsent ? t('mods.checkUpdatesNow') : t('mods.checkUpdates')}
          </Button>
        </div>
      </div>

      {/*
        隐私披露只在「还没同意」时出现。文案写清发什么、发给谁，而不是含糊地
        说「需要联网」——用户才能做有意义的决定。
        同意后这条会收起：常驻的说明会占掉一行高度，而它要传达的信息（哈希会
        外发）在点过一次之后已经知道了。
      */}
      {!hashConsent && (
        <p className={styles.consent}>
          <ShieldAlert size={13} /> {t('mods.hashNotice')}
        </p>
      )}

      {issues && issues.length > 0 && (
        <div className={styles.issueBox} role="status">
          {issues.map((issue, index) => (
            <div key={`${issue.code}-${index}`} className={styles.issue}>
              <AlertTriangle
                size={14}
                className={issue.severity === 'error' ? styles.severityError : styles.severityWarn}
              />
              <span>{issue.message}</span>
            </div>
          ))}
        </div>
      )}

      <ul className={styles.list}>
        {files.map(file => {
          const fileIssues = issuesByFile.get(file.fileName) ?? [];
          const update = updateByFile.get(file.fileName);
          const source = sourceByFile.get(file.fileName);
          return (
            <li key={file.fileName} className={`${styles.row} ${file.enabled ? '' : styles.rowDisabled}`}>
              <button
                type="button"
                className={styles.toggle}
                onClick={() => void toggle(file)}
                disabled={toggling !== '' || updating !== ''}
                aria-pressed={file.enabled}
                aria-label={file.enabled ? t('mods.disable') : t('mods.enable')}
                title={file.enabled ? t('mods.disable') : t('mods.enable')}
              >
                {file.enabled
                  ? <ToggleRight size={20} className={styles.toggleOn} />
                  : <ToggleLeft size={20} className={styles.toggleOff} />}
              </button>

              <span className={styles.name} title={file.fileName}>
                {source ? (
                  <>
                    <a
                      className={styles.sourceName}
                      href={source.projectUrl}
                      target="_blank"
                      rel="noreferrer noopener"
                      title={t('mods.openProject', { values: { name: source.projectTitle } })}
                    >
                      {source.projectTitle}
                    </a>
                    {/*
                      文件名不隐藏：用户按文件名在文件管理器里找同一个 jar，
                      只显示工程名会让这一步对不上。它作为次要信息跟在后面。
                    */}
                    <span className={styles.fileName}>{file.fileName}</span>
                  </>
                ) : (
                  file.fileName
                )}
              </span>

              <span className={styles.size}>{formatBytes(file.size)}</span>

              <span className={styles.badges}>
                {!file.enabled && <span className={styles.badgeMuted}>{t('mods.disabled')}</span>}
                {source && <span className={styles.badgeSource}>{t('mods.sourceCurseForge')}</span>}
                {source?.latest && (
                  <span
                    className={styles.badgeUpdate}
                    title={t('mods.latestHint', {
                      values: {
                        version: source.latest.displayName || source.latest.fileName,
                      },
                    })}
                  >
                    {t('mods.latestShort')}
                  </span>
                )}
                {fileIssues.length > 0 && (
                  <span className={styles.badgeWarn} title={fileIssues.map(i => i.message).join('\n')}>
                    {t('mods.issueFlag')}
                  </span>
                )}
              </span>

              {update && update.kind === 'updateAvailable' && (
                <Button
                  size="sm"
                  loading={updating === file.fileName}
                  onClick={() => void applyUpdate(file.fileName)}
                >
                  {t('mods.update', { values: { version: update.update.newVersion } })}
                </Button>
              )}
            </li>
          );
        })}
      </ul>
    </div>
  );
}
