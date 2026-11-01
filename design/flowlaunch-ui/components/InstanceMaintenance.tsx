// 实例维护：补全文件（主）与检查官方更新（次）。
//
// ## 为什么把「补全文件」放在主位
//
// 参考 PCL2 的实例管理页：它的对应功能叫「补全文件」——校验版本依赖是否完整，
// 重新下载缺失与校验失败的文件。这是用户真正会遇到的问题（下载中断、磁盘损坏、
// 手滑删了某个库），而且**有明确的处理动作**。
//
// 之前的界面只有一个「检查客户端更新」，它比对的是本地版本配置与 Mojang 当前
// 清单的**元数据**。问题有三：
//   1. Mojang 几乎从不重发同一版本的元数据，所以它绝大多数时候返回「已是最新」，
//      用户点了几次没有反馈，就会认为这个按钮没用；
//   2. 检查出来也没有后续动作——知道「Mojang 改了文件」之后仍然要重新安装；
//   3. 它完全不检查用户磁盘上的文件是否还在。
//
// 所以现在的主操作是「补全文件」：逐个校验 SHA-1，缺的坏的重下，好的跳过。
// 「检查官方更新」保留为次要信息，只在真有差异时才提示。

import { useState } from 'react';
import { RefreshCw, ShieldCheck } from 'lucide-react';
import { Button } from './Button';
import { useI18n } from '../utils/i18n';
import {
  checkClientUpdate,
  errorText,
  installInstance,
  type ClientUpdateStatus,
} from '../utils/native';
import type { GameInstance } from '../types';
import styles from './InstanceMaintenance.module.css';

interface InstanceMaintenanceProps {
  instance: GameInstance;
  onNotice: (message: string) => void;
  /** 补全完成后实例状态可能变化（例如从 failed 回到 ready）。 */
  onChanged?: () => void;
}

export function InstanceMaintenance({ instance, onNotice, onChanged }: InstanceMaintenanceProps) {
  const { t } = useI18n();
  const [checking, setChecking] = useState(false);
  const [repairing, setRepairing] = useState(false);
  const [update, setUpdate] = useState<ClientUpdateStatus | null>(null);

  /**
   * 只有安装完成（ready）或安装失败（error）的实例可以补全。
   *
   * failed 也要允许：那正是最需要「校验并补全」的状态——上次装到一半失败，
   * 磁盘上留着一堆半成品，补全能把它们补齐。
   */
  const canRepair = instance.status === 'ready' || instance.status === 'error';

  const runRepair = async () => {
    setRepairing(true);
    try {
      await installInstance(instance.id);
      onNotice(t('maintain.repairDone'));
      // 补全可能把 failed 的实例救回 ready，让外层重读一次。
      onChanged?.();
      // 文件被重新校验过，之前的更新结论可能已经过期。
      setUpdate(null);
    } catch (error) {
      onNotice(errorText(error, '补全文件失败。'));
      onChanged?.();
    } finally {
      setRepairing(false);
    }
  };

  const runCheck = async () => {
    setChecking(true);
    try {
      setUpdate(await checkClientUpdate(instance.id));
    } catch (error) {
      setUpdate(null);
      onNotice(errorText(error, '无法检查客户端更新。'));
    } finally {
      setChecking(false);
    }
  };

  return (
    <div className={styles.page}>
      {/* --- 补全文件（主操作） --- */}
      <div className={styles.row}>
        <div className={styles.rowMain}>
          <strong>{t('maintain.repairFiles')}</strong>
          <span>{t('maintain.repairFilesHint')}</span>
        </div>
        <Button
          size="sm"
          icon={<ShieldCheck size={13} />}
          loading={repairing}
          disabled={!canRepair || repairing}
          onClick={() => void runRepair()}
        >
          {t('maintain.repair')}
        </Button>
      </div>

      {/* --- 检查官方更新（次要） --- */}
      <div className={styles.row}>
        <div className={styles.rowMain}>
          <strong>{t('maintain.clientUpdate')}</strong>
          <span>{t('maintain.clientUpdateHint')}</span>
          {/*
            结果分三种如实呈现：已是最新 / 有差异（分项计数）/ 尚未检查。
            不把「有差异」写成「可更新」——后端只比较元数据，真正更新仍要走补全，
            给一个「立即更新」按钮会让人以为点一下就能打补丁。
          */}
          {update && (
            <span className={update.upToDate ? styles.ok : styles.warn}>
              {update.upToDate
                ? t('maintain.upToDate')
                : t('maintain.updateSummary', {
                    values: {
                      added: update.added,
                      removed: update.removed,
                      changed: update.changed,
                    },
                  })}
            </span>
          )}
        </div>
        <Button
          size="sm"
          variant="ghost"
          icon={<RefreshCw size={13} />}
          loading={checking}
          disabled={instance.status !== 'ready' || checking}
          onClick={() => void runCheck()}
        >
          {t('maintain.check')}
        </Button>
      </div>
    </div>
  );
}
