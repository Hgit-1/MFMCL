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

import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  AlertTriangle,
  Download,
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
  errorText,
  formatBytes,
  inspectMods,
  listMods,
  setModEnabled,
  updateMod,
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

  const refresh = useCallback(async () => {
    setState('loading');
    setLoadError('');
    try {
      setFiles(await listMods(instance.id));
      setState('ready');
    } catch (error) {
      setLoadError(errorText(error, '无法读取模组列表。'));
      setState('error');
    }
  }, [instance.id]);

  useEffect(() => { void refresh(); }, [refresh]);

  // 切换实例时清空「检查结果」：那些结果属于上一个实例的文件名，沿用会误导。
  useEffect(() => {
    setIssues(null);
    setUpdates(null);
    setHashConsent(false);
  }, [instance.id]);

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

  const toggle = async (file: ModFile) => {
    setToggling(file.fileName);
    try {
      await setModEnabled(instance.id, file.fileName, !file.enabled);
      // 本地先翻转，避免整表刷新造成的闪烁；随后再对齐一次后端。
      setFiles(current => current.map(item =>
        item.fileName === file.fileName ? { ...item, enabled: !item.enabled } : item));
      onChanged();
    } catch (error) {
      onNotice(errorText(error, '无法切换模组状态。'));
    } finally {
      setToggling('');
    }
  };

  const runInspect = async () => {
    setInspecting(true);
    try {
      const found = await inspectMods(instance.id);
      setIssues(found);
      onNotice(found.length === 0 ? t('mods.noIssues') : t('mods.issueCount', { count: found.length }));
    } catch (error) {
      onNotice(errorText(error, '无法检查模组问题。'));
    } finally {
      setInspecting(false);
    }
  };

  const runUpdateCheck = async () => {
    if (!hashConsent) {
      setHashConsent(true);
      // 第一次点击只做披露，不发起请求。第二次点击才真正查询。
      return;
    }
    setChecking(true);
    try {
      const reports = await checkModUpdates(instance.id);
      setUpdates(reports);
      const available = reports.filter(report => report.status.kind === 'updateAvailable').length;
      onNotice(available === 0 ? t('mods.allUpToDate') : t('mods.updateCount', { count: available }));
    } catch (error) {
      onNotice(errorText(error, '无法检查模组更新。'));
    } finally {
      setChecking(false);
    }
  };

  const applyUpdate = async (fileName: string) => {
    const entry = updateByFile.get(fileName);
    if (!entry || entry.kind !== 'updateAvailable') return;
    setUpdating(fileName);
    try {
      const path = await updateMod(instance.id, entry.update);
      onNotice(t('mods.updated', { values: { name: entry.update.newFileName } }));
      void path;
      await refresh();
      // 更新后旧的检查结果已失效，清掉避免把「还有更新」挂在已升级的文件上。
      setUpdates(current => current?.filter(report => report.fileName !== fileName) ?? null);
      onChanged();
    } catch (error) {
      onNotice(errorText(error, '更新模组失败。'));
    } finally {
      setUpdating('');
    }
  };

  if (state === 'loading') {
    return <div className={styles.empty}>{t('common.loading')}</div>;
  }

  if (state === 'error') {
    return (
      <div className={styles.empty} role="alert">
        <AlertTriangle size={20} />
        <p>{loadError}</p>
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
          return (
            <li key={file.fileName} className={`${styles.row} ${file.enabled ? '' : styles.rowDisabled}`}>
              <button
                type="button"
                className={styles.toggle}
                onClick={() => void toggle(file)}
                disabled={toggling === file.fileName}
                aria-pressed={file.enabled}
                aria-label={file.enabled ? t('mods.disable') : t('mods.enable')}
                title={file.enabled ? t('mods.disable') : t('mods.enable')}
              >
                {file.enabled
                  ? <ToggleRight size={20} className={styles.toggleOn} />
                  : <ToggleLeft size={20} className={styles.toggleOff} />}
              </button>

              <span className={styles.name} title={file.fileName}>{file.fileName}</span>

              <span className={styles.size}>{formatBytes(file.size)}</span>

              <span className={styles.badges}>
                {!file.enabled && <span className={styles.badgeMuted}>{t('mods.disabled')}</span>}
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
