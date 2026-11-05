// Microsoft 应用（公共客户端）ID 的配置区。
//
// ## 为什么登录页里要放这个
//
// 应用 ID 与发布者绑定，不能借用别人的，而构建时注入意味着「想登录就得重新编译」。
// 绝大多数人拿到的是别人构建好的安装包，他们需要一条不必编译的路。
//
// ## 只在需要时出现
//
// 构建时已注入 ID 的发行版不该看到这一块——那是开发者的事，对用户只是噪音。
// 所以先查状态：`configured && source === 'builtin'` 时整块不渲染。
//
// ## 输入什么
//
// 一个 36 位 GUID。形状校验放在后端（保存时），前端只做即时提示：明显的格式错误
// 不该等到点了保存才说。真正的有效性只能靠一次真实登录验证——这里不假装能判断。

import { useCallback, useEffect, useState } from 'react';
import { AlertTriangle, CheckCircle2, KeyRound } from 'lucide-react';
import { Button } from './Button';
import { useI18n } from '../utils/i18n';
import {
  errorText,
  microsoftClearClientId,
  microsoftClientIdStatus,
  microsoftSaveClientId,
  type MicrosoftClientIdStatus,
} from '../utils/native';
import styles from './MicrosoftClientIdCard.module.css';

interface MicrosoftClientIdCardProps {
  /** 保存成功后回调：登录按钮要据此变为可用。 */
  onChanged?: (status: MicrosoftClientIdStatus) => void;
}

/** 36 位 GUID 形状。与后端 `looks_like_client_id` 同一规则。 */
export function looksLikeClientId(value: string): boolean {
  return /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/.test(value.trim());
}

export function MicrosoftClientIdCard({ onChanged }: MicrosoftClientIdCardProps) {
  const { t } = useI18n();
  const [status, setStatus] = useState<MicrosoftClientIdStatus | null>(null);
  const [draft, setDraft] = useState('');
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(() => {
    void microsoftClientIdStatus()
      .then(next => {
        setStatus(next);
        onChanged?.(next);
      })
      .catch(() => setStatus(null));
  }, [onChanged]);

  useEffect(() => { refresh(); }, [refresh]);

  // 构建期已注入：这一块对用户没有意义，不渲染。
  if (!status || (status.configured && status.source === 'builtin')) return null;

  const save = async () => {
    setBusy(true);
    try {
      const next = await microsoftSaveClientId(draft.trim());
      setStatus(next);
      onChanged?.(next);
      setDraft('');
    } catch (error) {
      setStatus(current => current && { ...current, problem: errorText(error, t('msId.saveFailed')) });
    } finally {
      setBusy(false);
    }
  };

  const clear = async () => {
    setBusy(true);
    try {
      const next = await microsoftClearClientId();
      setStatus(next);
      onChanged?.(next);
    } catch (error) {
      setStatus(current => current && { ...current, problem: errorText(error, t('msId.saveFailed')) });
    } finally {
      setBusy(false);
    }
  };

  const valid = looksLikeClientId(draft);

  return (
    <div className={status.configured ? styles.done : styles.card}>
      <div className={styles.head}>
        <KeyRound size={14} />
        <strong>{t('msId.title')}</strong>
        {status.configured && (
          <span className={styles.badge}>
            <CheckCircle2 size={12} />
            {t('msId.configured')}
          </span>
        )}
      </div>

      <p className={styles.hint}>
        {status.configured ? t('msId.configuredHint') : t('msId.needId')}
      </p>

      {/*
        把两个关键入口做成可点的链接。
        纯文本里写「portal.azure.com」用户得手动复制、再猜协议；直接给链接能省掉
        最容易出错的一步。`aka.ms/mce-reviewappid` 是审核表单——注意不是
        `aka.ms/mcapi`，那个短链指向微软内部 SharePoint，外部访问返回 401。
      */}
      {!status.configured && (
        <p className={styles.links}>
          <a href="https://portal.azure.com" target="_blank" rel="noreferrer noopener">
            {t('msId.openPortal')}
          </a>
          <span aria-hidden="true">·</span>
          <a href="https://aka.ms/mce-reviewappid" target="_blank" rel="noreferrer noopener">
            {t('msId.openReview')}
          </a>
        </p>
      )}

      {status.problem && (
        <p className={styles.error} role="alert">
          <AlertTriangle size={12} />
          {status.problem}
        </p>
      )}

      {status.configured ? (
        <div className={styles.actions}>
          <Button size="sm" variant="ghost" loading={busy} onClick={() => void clear()}>
            {t('msId.replace')}
          </Button>
        </div>
      ) : (
        <>
          <input
            className={styles.input}
            value={draft}
            onChange={event => setDraft(event.target.value)}
            placeholder="00000000-0000-0000-0000-000000000000"
            spellCheck={false}
            aria-label={t('msId.title')}
          />
          {draft.trim() !== '' && !valid && (
            <p className={styles.hint}>{t('msId.shape')}</p>
          )}
          <div className={styles.actions}>
            <Button size="sm" loading={busy} disabled={!valid} onClick={() => void save()}>
              {t('msId.save')}
            </Button>
          </div>
        </>
      )}
    </div>
  );
}
