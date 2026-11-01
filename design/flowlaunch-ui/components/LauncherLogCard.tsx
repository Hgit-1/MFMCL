// 启动器日志开关 + 提交 issue 的引导。
//
// ## 这个控件在做什么
//
// 它取代了原来那个「匿名数据收集」开关。那一个没有任何实现，拨动它不会改变任何
// 行为——却带着一句写死的隐私承诺（「仅收集崩溃和性能统计」）。界面上有一句无法
// 兑现的承诺，比缺一个功能糟得多。
//
// 现在它对应一件真的会发生的事：把启动器自身的事件写进本机的一个文件，由用户
// 自己决定要不要把那个文件贴进 issue。**没有任何数据离开这台机器**，这也是它与
// 「遥测」根本不同的地方——不需要服务端，不需要隐私政策，不需要同意流程。
//
// ## 为什么开启后要展示一段引导
//
// 用户开启日志的目的通常是「遇到问题了，想找人看」。他此刻多半不知道下一步该
// 做什么：文件在哪、要发到哪、发之前该不该检查内容。所以开关打开后立刻展开一段
// 带步骤的引导，把这三个问题按顺序回答。
//
// 这段引导是**静态的**（不播放动画），但用分步高亮和编号让它读起来像一条路径，
// 而不是一段说明文字。理由在下面注释里写：给「去 GitHub 提 issue」做帧动画
// 既没有信息增量，也会让一个本来只需要读三行的动作变得难以扫读。

import { useCallback, useEffect, useState } from 'react';
import {
  AlertTriangle,
  ArrowUpRight,
  FolderOpen,
  Github,
  ScrollText,
  Trash2,
} from 'lucide-react';
import { Button } from './Button';
import { useI18n } from '../utils/i18n';
import {
  clearLauncherLog,
  errorText,
  formatBytes,
  isDesktopShell,
  launcherLogInfo,
  openLauncherLogFolder,
  type LauncherLogInfo,
} from '../utils/native';
import styles from './LauncherLogCard.module.css';

interface LauncherLogCardProps {
  /** 开关当前值（来自设置表单）。 */
  enabled: boolean;
  onChange: (enabled: boolean) => void;
  onNotice?: (message: string) => void;
}

/** 提交 issue 的仓库地址。写成常量，界面与文案引用同一处。 */
const ISSUE_URL = 'https://github.com/Hgit-1/MFMCL/issues/new';

export function LauncherLogCard({ enabled, onChange, onNotice }: LauncherLogCardProps) {
  const { t } = useI18n();
  const [info, setInfo] = useState<LauncherLogInfo | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setInfo(await launcherLogInfo());
    } catch {
      // 读不到状态时不影响开关本身；下面的路径信息会留空。
      setInfo(null);
    }
  }, []);

  useEffect(() => { void refresh(); }, [refresh]);

  const openFolder = async () => {
    setBusy(true);
    try {
      await openLauncherLogFolder();
    } catch (error) {
      onNotice?.(errorText(error, t('launcherLog.openFailed')));
    } finally {
      setBusy(false);
    }
  };

  const clear = async () => {
    setBusy(true);
    try {
      await clearLauncherLog();
      await refresh();
      onNotice?.(t('launcherLog.cleared'));
    } catch (error) {
      onNotice?.(errorText(error, t('launcherLog.clearFailed')));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className={styles.card}>
      <div className={styles.head}>
        <span className={styles.icon}><ScrollText size={16} strokeWidth={1.8} /></span>
        <div className={styles.headCopy}>
          <strong>{t('launcherLog.title')}</strong>
          <p>{t('launcherLog.hint')}</p>
        </div>
        <button
          type="button"
          role="switch"
          aria-checked={enabled}
          aria-label={t('launcherLog.title')}
          className={`${styles.switch} ${enabled ? styles.switchOn : ''}`}
          onClick={() => onChange(!enabled)}
        >
          <i />
        </button>
      </div>

      {/*
        关闭状态什么都不展开。
        「日志关着」不需要更多解释，留一堆灰掉的按钮只会让面板变长。
      */}
      {enabled && (
        <>
          <dl className={styles.facts}>
            <div>
              <dt>{t('launcherLog.location')}</dt>
              <dd className={styles.path} title={info?.path}>{info?.path ?? t('launcherLog.previewPath')}</dd>
            </div>
            <div>
              <dt>{t('launcherLog.size')}</dt>
              <dd>{formatBytes(info?.bytes ?? 0)}</dd>
            </div>
          </dl>

          {/*
            三步引导。
            用有序列表而不是一段说明文字：用户此刻要执行三个动作，编号让「我做到
            第几步」可追踪。每一步只讲一件事，且都指向一个能立刻点的按钮或链接。
          */}
          <ol className={styles.steps}>
            <li>
              <span className={styles.stepNum}>1</span>
              <div className={styles.stepBody}>
                <strong>{t('launcherLog.step1')}</strong>
                <p>{t('launcherLog.step1Hint')}</p>
              </div>
            </li>
            <li>
              <span className={styles.stepNum}>2</span>
              <div className={styles.stepBody}>
                <strong>{t('launcherLog.step2')}</strong>
                <p>{t('launcherLog.step2Hint')}</p>
                {/*
                  警告放在第二步而不是末尾：用户在这一步准备把文件发出去，这是唯一
                  需要先读一遍内容的时刻。放在最后会在他已经点开 issue 之后才看到。
                */}
                <p className={styles.warn}>
                  <AlertTriangle size={12} />
                  {t('launcherLog.step2Warn')}
                </p>
              </div>
            </li>
            <li>
              <span className={styles.stepNum}>3</span>
              <div className={styles.stepBody}>
                <strong>{t('launcherLog.step3')}</strong>
                <p>{t('launcherLog.step3Hint')}</p>
                <a className={styles.issueLink} href={ISSUE_URL} target="_blank" rel="noreferrer noopener">
                  <Github size={13} />
                  {t('launcherLog.openIssue')}
                  <ArrowUpRight size={12} />
                </a>
              </div>
            </li>
          </ol>

          <div className={styles.actions}>
            <Button variant="ghost" icon={<FolderOpen size={13} />} loading={busy} onClick={() => void openFolder()}>
              {t('launcherLog.openFolder')}
            </Button>
            <Button variant="ghost" icon={<Trash2 size={13} />} loading={busy} onClick={() => void clear()}>
              {t('launcherLog.clear')}
            </Button>
          </div>

          {!isDesktopShell() && <p className={styles.note}>{t('launcherLog.previewOnly')}</p>}
        </>
      )}
    </div>
  );
}
