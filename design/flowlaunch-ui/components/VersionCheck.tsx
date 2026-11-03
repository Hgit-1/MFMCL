// 实例版本核对。
//
// ## 为什么需要这块
//
// 实例的 `game_version` 是创建时写进数据库的一个字符串。它可能过时：用户在文件
// 管理器里换了 jar、把外部整合包拷进来、或旧版本的记录本身就写错了。之后所有
// 依赖它的操作（选 Java、筛模组、挑加载器构建）都会跟着错。
//
// 这里把磁盘上的实际内容读出来与记录对比，不一致时让用户确认修正——不静默改写。
// 静默修正会让「我明明设成了 X」变成一个无法解释的状态，而用户是唯一能判断
// 「到底哪个才是对的」的人。
//
// ## 只在能确定时才提示
//
// `confidence: exact` 才认为值得提示（来自版本 JSON 的 `inheritsFrom` 或 `id`）。
// `inferred` 来自目录名之类的弱线索，误判概率高，仅在界面上作为参考显示，不主动
// 建议覆盖记录。

import { useCallback, useEffect, useState } from 'react';
import { AlertTriangle, CheckCircle2, RefreshCw } from 'lucide-react';
import { Button } from './Button';
import { useI18n } from '../utils/i18n';
import { applyDetectedVersion, detectInstanceVersion, errorText, type VersionDetection } from '../utils/native';
import type { GameInstance } from '../types';
import styles from './VersionCheck.module.css';

interface VersionCheckProps {
  instance: GameInstance;
  onNotice: (message: string) => void;
  /** 修正成功后刷新实例列表（版本号变了）。 */
  onFixed: () => void;
}

export function VersionCheck({ instance, onNotice, onFixed }: VersionCheckProps) {
  const { t } = useI18n();
  const [detected, setDetected] = useState<VersionDetection | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(() => {
    void detectInstanceVersion(instance.id)
      .then(setDetected)
      .catch(() => setDetected(null));
  }, [instance.id]);

  // 换实例时重查；同一实例的版本号变了也重查（修正之后要显示新状态）。
  useEffect(() => {
    refresh();
  }, [refresh, instance.version, instance.loader]);

  const fix = async () => {
    setBusy(true);
    try {
      const result = await applyDetectedVersion(instance.id);
      setDetected(result);
      onNotice(t('versionCheck.fixed', { values: { version: result.version ?? '' } }));
      onFixed();
    } catch (error) {
      onNotice(errorText(error, t('versionCheck.fixFailed')));
    } finally {
      setBusy(false);
    }
  };

  /*
   * 判定要显示哪种状态：
   *   - 无法识别 → 不显示（没有可信结论时保持沉默，避免制造焦虑）
   *   - 与记录一致 → 一行确认，说明「记录与文件相符」
   *   - 不一致且可信 → 警告 + 修正按钮
   */
  if (!detected?.version) return null;

  const mismatch = detected.version !== detected.recorded;
  const confident = detected.confidence === 'exact';

  if (!mismatch) {
    return (
      <div className={styles.ok}>
        <CheckCircle2 size={14} />
        <span>
          {t('versionCheck.matches', { values: { version: detected.version } })}
          {detected.loader ? ` · ${detected.loader}${detected.loaderVersion ? ` ${detected.loaderVersion}` : ''}` : ''}
        </span>
      </div>
    );
  }

  return (
    <div className={confident ? styles.mismatch : styles.hintBlock} role={confident ? 'alert' : undefined}>
      <div className={styles.head}>
        {confident ? <AlertTriangle size={14} /> : <RefreshCw size={14} />}
        <strong>
          {confident
            ? t('versionCheck.mismatchTitle')
            : t('versionCheck.weakTitle')}
        </strong>
      </div>
      <p className={styles.body}>
        {t('versionCheck.mismatchBody', {
          values: { recorded: detected.recorded, found: detected.version, source: detected.source },
        })}
      </p>
      {confident && (
        <div className={styles.actions}>
          <Button size="sm" loading={busy} onClick={() => void fix()}>
            {t('versionCheck.useFound', { values: { version: detected.version } })}
          </Button>
        </div>
      )}
    </div>
  );
}
