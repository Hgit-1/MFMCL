// CurseForge API Key 设置卡片。
//
// 从 `CurseForgePanel` 里抽出来，因为它需要被两处复用：
//   1. 内容商店的来源区域——用户勾选 CurseForge 时，如果没 key，就地给出设置入口，
//      而不是先报错再让他去找；
//   2. 资源包/地图分类——这两类只走 CurseForge，没有 key 时整页就是这张卡片。
//
// key 本身始终由后端存进系统凭据库；这个组件只负责收集、上传、清除，且上传后
// 立刻从组件状态里丢弃本地副本。

import { useCallback, useEffect, useRef, useState } from 'react';
import { AlertTriangle, KeyRound, Trash2 } from 'lucide-react';
import { Button } from './Button';
import { useI18n } from '../utils/i18n';
import {
  curseforgeClearKey,
  curseforgeKeyStatus,
  curseforgeSaveKey,
  errorText,
  type CurseForgeKeyStatus,
} from '../utils/native';
import styles from './CurseForgeKeyCard.module.css';

interface CurseForgeKeyCardProps {
  /** 状态变化时通知父组件（例如搜索结果需要重跑）。 */
  onStatusChange?: (status: CurseForgeKeyStatus) => void;
  onNotice?: (message: string) => void;
  /** 紧凑模式：嵌在筛选卡里时用，去掉大标题与说明段落。 */
  compact?: boolean;
}

export function CurseForgeKeyCard({ onStatusChange, onNotice, compact = false }: CurseForgeKeyCardProps) {
  const { t } = useI18n();
  const [status, setStatus] = useState<CurseForgeKeyStatus | null>(null);
  const [draft, setDraft] = useState('');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  /** 内置 key 可用时，用户是否主动展开了「换成自己的 key」表单。 */
  const [overriding, setOverriding] = useState(false);

  /*
   * 回调放进 ref，让 `refresh` 只依赖「取状态的函数」而不依赖父组件传来的闭包。
   *
   * 之前 `refresh` 的依赖里直接放着 `onStatusChange`：父组件若在渲染中内联传一个
   * 箭头函数（`onStatusChange={status => setKeyStatus(status)}`），每次渲染都会产生
   * 新引用 → `refresh` 变成新函数 → 下面的 `useEffect` 再跑一次 → `setStatus`
   * 触发父组件重渲染 → 又传新回调。结果是一个无限渲染循环，点进资源包分类页
   * 直接把界面卡死。
   */
  const onStatusChangeRef = useRef(onStatusChange);
  onStatusChangeRef.current = onStatusChange;

  const refresh = useCallback(async () => {
    try {
      const next = await curseforgeKeyStatus();
      setStatus(next);
      onStatusChangeRef.current?.(next);
    } catch {
      const next: CurseForgeKeyStatus = { configured: false, problem: null, source: null };
      setStatus(next);
      onStatusChangeRef.current?.(next);
    }
  }, []);

  // 只在挂载时查一次：key 的变化走保存/清除后显式调用 refresh。
  useEffect(() => { void refresh(); }, [refresh]);

  const save = async () => {
    const value = draft.trim();
    if (!value) { setError(t('cf.keyEmpty')); return; }
    setSaving(true);
    setError('');
    try {
      await curseforgeSaveKey(value);
      // 上传成功后立刻丢弃本地副本：不在内存里留一份不必要的凭据。
      setDraft('');
      await refresh();
      onNotice?.(t('cf.keySaved'));
    } catch (cause) {
      setError(errorText(cause, t('cf.keyInvalid')));
    } finally {
      setSaving(false);
    }
  };

  const clear = async () => {
    try {
      await curseforgeClearKey();
      await refresh();
      onNotice?.(t('cf.keyCleared'));
    } catch (cause) {
      setError(errorText(cause, t('cf.keyClearFailed')));
    }
  };

  // 已可用：区分「用户自己配的」与「构建期内置的」。
  if (status?.configured) {
    const builtin = status.source === 'builtin';
    /*
     * 内置 key 时默认只显示状态行，但也允许用户换成自己的 key。
     *
     * 不给入口的话，内置 key 一旦配额耗尽或被轮换，用户就只能等新版本——而他
     * 手上可能就有自己的 key。这里做成「点一下展开输入框」，不额外占版面。
     */
    if (builtin && !overriding) {
      return (
        <div className={compact ? styles.configuredCompact : styles.configured}>
          <span className={styles.configuredDot} aria-hidden="true" />
          <span className={styles.configuredText}>{t('cf.keyBuiltin')}</span>
          <button type="button" className={styles.clearButton} onClick={() => setOverriding(true)}>
            {t('cf.keyUseOwn')}
          </button>
        </div>
      );
    }
    if (!builtin) {
      return (
        <div className={compact ? styles.configuredCompact : styles.configured}>
          <span className={styles.configuredDot} aria-hidden="true" />
          <span className={styles.configuredText}>{t('cf.keyConfigured')}</span>
          <button type="button" className={styles.clearButton} onClick={() => void clear()}>
            <Trash2 size={12} />{t('cf.keyClear')}
          </button>
        </div>
      );
    }
    // builtin && overriding：落到下面的输入表单。
  }

  return (
    <div className={compact ? styles.cardCompact : styles.card}>
      <div className={styles.head}>
        <KeyRound size={compact ? 15 : 17} strokeWidth={1.9} />
        <div>
          <strong>{t('cf.keyTitle')}</strong>
          {!compact && <p>{t('cf.keyHint')}</p>}
        </div>
      </div>

      {status?.problem && (
        <div className={styles.warn} role="alert">
          <AlertTriangle size={14} />
          <span>{status.problem}</span>
        </div>
      )}

      <label className={styles.field}>
        <span>{t('cf.keyLabel')}</span>
        <input
          type="password"
          value={draft}
          onChange={event => setDraft(event.target.value)}
          onKeyDown={event => { if (event.key === 'Enter') void save(); }}
          placeholder="$2a$10$…"
          autoComplete="off"
          spellCheck={false}
        />
      </label>

      {error && <div className={styles.error} role="alert">{error}</div>}

      <div className={styles.actions}>
        <Button size={compact ? 'sm' : 'md'} onClick={() => void save()} loading={saving} disabled={!draft.trim()}>
          {t('cf.keySave')}
        </Button>
        <a className={styles.link} href="https://console.curseforge.com/" target="_blank" rel="noreferrer">
          {t('cf.keyLink')}
        </a>
        {/* 从「内置 key」展开来的表单要能收回去，否则用户只能靠刷新页面。 */}
        {status?.configured && overriding && (
          <Button size={compact ? 'sm' : 'md'} variant="ghost" onClick={() => setOverriding(false)}>
            {t('common.cancel')}
          </Button>
        )}
      </div>

      {!compact && <p className={styles.note}>{t('cf.keyStorage')}</p>}
    </div>
  );
}

/** 供父组件在别处显示「是否已配置」时不重复请求。 */
export function useCurseForgeKey(): {
  status: CurseForgeKeyStatus | null;
  setStatus: (status: CurseForgeKeyStatus) => void;
} {
  const [status, setStatus] = useState<CurseForgeKeyStatus | null>(null);
  return { status, setStatus };
}
