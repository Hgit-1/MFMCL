// 启动器自身更新。
//
// 与游戏内容更新是两件事：这里更新的是启动器本身。
//
// 三条来自后端的硬约束，界面必须如实呈现而不是绕开：
//   1. 未配置更新清单的构建**无法**检查更新，后端会直接报错。界面要把这句话
//      原样传达，而不是显示一个永远转圈的按钮。
//   2. 下载下来的安装包会校验 SHA-256，校验失败就不发布到目标目录。
//   3. 启动器**不会**替换自己正在运行的二进制——没有更新签名之前，自替换是
//      不安全的。所以这里只把安装包准备好，由用户手动运行。

import { useState } from 'react';
import { ArrowDownToLine, RefreshCw } from 'lucide-react';
import { Button } from './Button';
import { Input } from './Input';
import { useI18n } from '../utils/i18n';
import {
  checkLauncherUpdate,
  downloadLauncherUpdate,
  errorText,
  formatBytes,
  pickDirectory,
  type LauncherUpdateStatus,
} from '../utils/native';
import styles from './LauncherUpdateCard.module.css';

export function LauncherUpdateCard() {
  const { t } = useI18n();
  const [status, setStatus] = useState<LauncherUpdateStatus | null>(null);
  /** 检查失败的原因。未配置清单是常态而非异常，要能显示出来。 */
  const [error, setError] = useState('');
  const [checking, setChecking] = useState(false);
  const [directory, setDirectory] = useState('');
  const [downloading, setDownloading] = useState(false);
  /** 最近一次成功下载的安装包路径。与 `error` 分开，避免把成功当成错误展示。 */
  const [downloaded, setDownloaded] = useState('');

  const check = async () => {
    setChecking(true);
    setError('');
    try {
      setStatus(await checkLauncherUpdate());
    } catch (cause) {
      setStatus(null);
      setError(errorText(cause, '无法检查更新。'));
    } finally {
      setChecking(false);
    }
  };

  const browse = async () => {
    const picked = await pickDirectory(t('update.pickDir'));
    if (picked) setDirectory(picked);
  };

  const download = async () => {
    if (!status?.url || !status.sha256) return;
    if (!directory.trim()) { setError(t('update.needDir')); return; }
    setDownloading(true);
    setError('');
    try {
      const path = await downloadLauncherUpdate(status.url, status.sha256, directory.trim());
      setDownloaded(path);
      // 下载完成后清掉可下载地址，避免用户重复下载同一个包。
      setStatus({ ...status, url: null });
    } catch (cause) {
      setError(errorText(cause, '下载更新失败。'));
    } finally {
      setDownloading(false);
    }
  };

  return (
    <div className={styles.card}>
      <div className={styles.head}>
        <div>
          <strong>{t('update.title')}</strong>
          <p>{t('update.hint')}</p>
        </div>
        <Button size="sm" variant="ghost" icon={<RefreshCw size={13} />} loading={checking} onClick={() => void check()}>
          {t('update.check')}
        </Button>
      </div>

      {error && <p className={styles.error} role="alert">{error}</p>}

      {status && (
        <div className={styles.status}>
          <dl className={styles.facts}>
            <div><dt>{t('update.current')}</dt><dd>{status.current}</dd></div>
            <div><dt>{t('update.latest')}</dt><dd>{status.latest ?? '—'}</dd></div>
          </dl>

          {status.available && status.url ? (
            <>
              {status.notes && <p className={styles.notes}>{status.notes}</p>}
              <div className={styles.row}>
                <Input
                  label={t('update.directory')}
                  value={directory}
                  onChange={event => setDirectory(event.target.value)}
                  placeholder={t('backup.directoryPlaceholder')}
                />
                <Button variant="ghost" onClick={() => void browse()}>{t('backup.browse')}</Button>
              </div>
              <div className={styles.actions}>
                <Button
                  icon={<ArrowDownToLine size={14} />}
                  loading={downloading}
                  disabled={!directory.trim()}
                  onClick={() => void download()}
                >
                  {t('update.download', { values: { size: status.size ? formatBytes(status.size) : '—' } })}
                </Button>
              </div>
              {/*
                必须写清「不会自动替换」：用户点了下载却看到启动器没变，会以为
                下载失败。这不是缺陷，是刻意的安全设计。
              */}
              <p className={styles.note}>{t('update.manualNote')}</p>
            </>
          ) : (
            <p className={styles.upToDate}>{t('update.upToDate')}</p>
          )}

          {downloaded && <p className={styles.done}>{t('update.downloaded', { values: { path: downloaded } })}</p>}
        </div>
      )}
    </div>
  );
}
