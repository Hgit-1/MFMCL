// 备份与诊断页。
//
// 为什么把两件事放一页：它们的使用场景是同一个——「游戏出了问题，我要把现场
// 留下来」。备份是留下可恢复的存档，诊断是留下可分析的信息。分成两个侧栏入口
// 会让用户在两页之间来回找，而它们其实是同一次救助行动的两步。
//
// 三条必须由用户明确同意的边界：
//   1. 备份可能含存档、服务器地址、模组私有配置——必须勾选隐私确认才能创建；
//   2. 恢复会新建一个独立实例，不覆盖现有实例——必须确认才执行；
//   3. 恢复前必须先用 `preview_backup` 拿到归档的 SHA-256，作为完整性凭据。
//      「先预览再恢复」不是多余的步骤，它保证恢复的正是用户看过的那份文件。

import { useCallback, useEffect, useState } from 'react';
import {
  AlertTriangle,
  Archive,
  FileSearch,
  FolderOpen,
  RefreshCw,
  ShieldCheck,
} from 'lucide-react';
import { Button } from './Button';
import { Input } from './Input';
import { useI18n } from '../utils/i18n';
import {
  createBackup,
  errorText,
  exportDiagnostics,
  formatBytes,
  pickDirectory,
  pickFile,
  previewBackup,
  restoreBackup,
  summarizeDiagnostics,
  type BackupPreview,
  type DiagnosticSummary,
} from '../utils/native';
import type { GameInstance } from '../types';
import styles from './BackupDiagnostics.module.css';

interface BackupDiagnosticsProps {
  instance: GameInstance;
  onNotice: (message: string) => void;
  /** 恢复备份会产生新实例，父组件需要刷新列表并跳转过去。 */
  onRestored: () => void;
}

export function BackupDiagnostics({ instance, onNotice, onRestored }: BackupDiagnosticsProps) {
  const { t } = useI18n();

  // --- 备份 -----------------------------------------------------------------
  /** 备份输出目录。桌面版用系统选择器，预览模式允许手动填写以便看布局。 */
  const [backupDir, setBackupDir] = useState('');
  /** 隐私确认。默认不勾：备份里确实可能有不适合外发的内容。 */
  const [privacyConfirmed, setPrivacyConfirmed] = useState(false);
  const [creating, setCreating] = useState(false);

  // --- 恢复 -----------------------------------------------------------------
  const [archivePath, setArchivePath] = useState('');
  const [preview, setPreview] = useState<BackupPreview | null>(null);
  const [previewing, setPreviewing] = useState(false);
  const [restoreName, setRestoreName] = useState('');
  const [restoring, setRestoring] = useState(false);

  // --- 诊断 -----------------------------------------------------------------
  const [summary, setSummary] = useState<DiagnosticSummary | null>(null);
  const [summarizing, setSummarizing] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [diagnosticDir, setDiagnosticDir] = useState('');

  // 切换实例时清空所有结果：备份名、诊断摘要都属于上一个实例，留着会误导。
  useEffect(() => {
    setArchivePath('');
    setPreview(null);
    setRestoreName('');
    setSummary(null);
  }, [instance.id]);

  const chooseBackupDir = async () => {
    const picked = await pickDirectory(t('backup.pickDir'));
    if (picked) setBackupDir(picked);
  };

  const runCreateBackup = async () => {
    if (!privacyConfirmed) { onNotice(t('backup.needPrivacy')); return; }
    if (!backupDir.trim()) { onNotice(t('backup.needDir')); return; }
    setCreating(true);
    try {
      const path = await createBackup(instance.id, backupDir.trim(), true);
      onNotice(t('backup.created', { values: { path } }));
    } catch (error) {
      onNotice(errorText(error, '创建备份失败。'));
    } finally {
      setCreating(false);
    }
  };

  const chooseArchive = async () => {
    const picked = await pickFile(t('backup.pickArchive'), [{ name: 'MFMCL backup', extensions: ['zip'] }]);
    if (!picked) return;
    setArchivePath(picked);
    setPreview(null);
    setPreviewing(true);
    try {
      const info = await previewBackup(picked);
      setPreview(info);
      // 默认名带上原实例与时间，避免恢复出两个同名实例分不清。
      setRestoreName(`${info.name}（恢复）`);
    } catch (error) {
      onNotice(errorText(error, '无法读取这个备份文件。'));
    } finally {
      setPreviewing(false);
    }
  };

  const runRestore = async () => {
    if (!preview) { onNotice(t('backup.needPreview')); return; }
    if (!restoreName.trim()) { onNotice(t('backup.needName')); return; }
    setRestoring(true);
    try {
      await restoreBackup(archivePath, preview.sha256, restoreName.trim(), true);
      onNotice(t('backup.restored', { values: { name: restoreName.trim() } }));
      setPreview(null);
      setArchivePath('');
      onRestored();
    } catch (error) {
      onNotice(errorText(error, '恢复备份失败。'));
    } finally {
      setRestoring(false);
    }
  };

  const runSummarize = async () => {
    setSummarizing(true);
    try {
      setSummary(await summarizeDiagnostics(instance.id));
    } catch (error) {
      onNotice(errorText(error, '无法生成诊断摘要。'));
    } finally {
      setSummarizing(false);
    }
  };

  const chooseDiagnosticDir = async () => {
    const picked = await pickDirectory(t('diagnostics.pickDir'));
    if (picked) setDiagnosticDir(picked);
  };

  const runExportDiagnostics = async () => {
    if (!diagnosticDir.trim()) { onNotice(t('diagnostics.needDir')); return; }
    setExporting(true);
    try {
      const path = await exportDiagnostics(instance.id, diagnosticDir.trim());
      onNotice(t('diagnostics.exported', { values: { path } }));
    } catch (error) {
      onNotice(errorText(error, '导出诊断包失败。'));
    } finally {
      setExporting(false);
    }
  };

  const reload = useCallback(() => {
    setSummary(null);
  }, []);
  useEffect(() => { reload(); }, [reload, instance.id]);

  return (
    <div className={styles.page}>
      {/* ---------------- 备份 ---------------- */}
      <section className={styles.card}>
        <header className={styles.head}>
          <span className={styles.icon}><Archive size={16} strokeWidth={1.8} /></span>
          <div>
            <h2>{t('backup.createTitle')}</h2>
            <p>{t('backup.createHint')}</p>
          </div>
        </header>

        <div className={styles.fieldRow}>
          <Input
            label={t('backup.directory')}
            value={backupDir}
            onChange={event => setBackupDir(event.target.value)}
            placeholder={t('backup.directoryPlaceholder')}
          />
          <Button variant="ghost" icon={<FolderOpen size={14} />} onClick={() => void chooseBackupDir()}>
            {t('backup.browse')}
          </Button>
        </div>

        <label className={styles.check}>
          <input
            type="checkbox"
            checked={privacyConfirmed}
            onChange={event => setPrivacyConfirmed(event.target.checked)}
          />
          <span>{t('backup.privacy')}</span>
        </label>

        <div className={styles.actions}>
          <Button
            loading={creating}
            disabled={!privacyConfirmed || !backupDir.trim()}
            onClick={() => void runCreateBackup()}
          >
            {t('backup.create')}
          </Button>
        </div>
      </section>

      {/* ---------------- 恢复 ---------------- */}
      <section className={styles.card}>
        <header className={styles.head}>
          <span className={styles.icon}><ShieldCheck size={16} strokeWidth={1.8} /></span>
          <div>
            <h2>{t('backup.restoreTitle')}</h2>
            <p>{t('backup.restoreHint')}</p>
          </div>
        </header>

        <div className={styles.fieldRow}>
          <Input
            label={t('backup.archive')}
            value={archivePath}
            onChange={event => { setArchivePath(event.target.value); setPreview(null); }}
            placeholder={t('backup.archivePlaceholder')}
          />
          <Button variant="ghost" icon={<FolderOpen size={14} />} loading={previewing} onClick={() => void chooseArchive()}>
            {t('backup.browse')}
          </Button>
        </div>

        {preview && (
          <div className={styles.previewBox}>
            <dl className={styles.facts}>
              <div><dt>{t('backup.factName')}</dt><dd>{preview.name}</dd></div>
              <div><dt>{t('backup.factVersion')}</dt><dd>{preview.minecraft}</dd></div>
              <div><dt>{t('backup.factFiles')}</dt><dd>{preview.files}</dd></div>
              <div><dt>{t('backup.factSize')}</dt><dd>{formatBytes(preview.totalBytes)}</dd></div>
              <div className={styles.factWide}>
                <dt>{t('backup.factSha')}</dt>
                <dd className={styles.mono} title={preview.sha256}>{preview.sha256.slice(0, 32)}…</dd>
              </div>
            </dl>
            <Input
              label={t('backup.restoreName')}
              value={restoreName}
              onChange={event => setRestoreName(event.target.value)}
              hint={t('backup.restoreNameHint')}
              maxLength={48}
            />
            <div className={styles.actions}>
              <Button loading={restoring} disabled={!restoreName.trim()} onClick={() => void runRestore()}>
                {t('backup.restore')}
              </Button>
            </div>
          </div>
        )}
      </section>

      {/* ---------------- 诊断 ---------------- */}
      <section className={styles.card}>
        <header className={styles.head}>
          <span className={styles.icon}><FileSearch size={16} strokeWidth={1.8} /></span>
          <div>
            <h2>{t('diagnostics.title')}</h2>
            <p>{t('diagnostics.hint')}</p>
          </div>
        </header>

        <div className={styles.actions}>
          <Button variant="ghost" icon={<RefreshCw size={13} />} loading={summarizing} onClick={() => void runSummarize()}>
            {t('diagnostics.summarize')}
          </Button>
        </div>

        {summary && (
          <div className={styles.summary}>
            <h3>{summary.title}</h3>
            <p className={styles.explanation}>{summary.explanation}</p>
            {/*
              置信度必须显示：诊断是基于日志的推断，不是确定结论。不标出这一点，
              用户会把「可能是模组冲突」当成诊断结果去照做。
            */}
            <p className={styles.confidence}>
              <AlertTriangle size={13} /> {t('diagnostics.confidence', { values: { level: summary.confidence } })}
              {' · '}
              {t('diagnostics.filesFound', { count: summary.filesFound })}
            </p>

            {/*
              全部命中的原因。
              崩溃常常同时满足多个特征（内存不足会连带触发 Mixin 失败），只显示
              最高置信度那一条，用户会去改表象而不是根因。这里按置信度列出全部，
              每条带「涉及模组」与「怎么修」。
            */}
            {summary.reasons.length > 0 && (
              <ol className={styles.reasonList}>
                {summary.reasons.map((reason, index) => (
                  <li key={`${reason.title}-${index}`} className={styles.reason}>
                    <div className={styles.reasonHead}>
                      <strong>{reason.title}</strong>
                      <span className={`${styles.reasonTag} ${reason.confidence === 'high' ? styles.reasonHigh : styles.reasonMedium}`}>
                        {t(reason.confidence === 'high' ? 'diagnostics.confidenceHigh' : 'diagnostics.confidenceMedium')}
                      </span>
                    </div>
                    <p className={styles.reasonExplanation}>{reason.explanation}</p>

                    {reason.mods.length > 0 && (
                      <p className={styles.reasonMods}>
                        {t('diagnostics.modsInvolved')}
                        {reason.mods.map(mod => (
                          <code key={mod} className={styles.modName}>{mod}</code>
                        ))}
                      </p>
                    )}

                    {reason.suggestions.length > 0 && (
                      // 建议用编号列表：用户会按顺序尝试，编号让「我试到第几步」可追踪。
                      <ol className={styles.suggestions}>
                        {reason.suggestions.map((suggestion, suggestionIndex) => (
                          <li key={suggestionIndex}>{suggestion}</li>
                        ))}
                      </ol>
                    )}

                    {reason.evidence.length > 0 && (
                      <ul className={styles.reasonEvidence}>
                        {reason.evidence.map((line, lineIndex) => (
                          <li key={lineIndex} className={styles.mono}>{line}</li>
                        ))}
                      </ul>
                    )}
                  </li>
                ))}
              </ol>
            )}
          </div>
        )}

        <div className={styles.fieldRow}>
          <Input
            label={t('diagnostics.directory')}
            value={diagnosticDir}
            onChange={event => setDiagnosticDir(event.target.value)}
            placeholder={t('backup.directoryPlaceholder')}
          />
          <Button variant="ghost" icon={<FolderOpen size={14} />} onClick={() => void chooseDiagnosticDir()}>
            {t('backup.browse')}
          </Button>
        </div>

        <div className={styles.actions}>
          <Button
            loading={exporting}
            disabled={!diagnosticDir.trim()}
            onClick={() => void runExportDiagnostics()}
          >
            {t('diagnostics.export')}
          </Button>
        </div>
      </section>
    </div>
  );
}
