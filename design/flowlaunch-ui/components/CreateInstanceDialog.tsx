// 新建实例对话框。
//
// 为什么需要它：之前「创建新游戏」只是把用户切到内容商店，而商店的原版条目
// 又提示用户「去游戏库里新建实例」——两条指引互相指向对方，用户实际无法创建
// 实例。这个对话框是那条流程缺的一环。
//
// 版本列表来自后端 `list_versions`（Mojang 官方清单），不在前端硬编码。搜索与
// 类型筛选都在本地做：清单一次拉全量，本地过滤比分页请求快，也不会因为用户连
// 续点筛选而堆积请求。这与内容商店里的原版列表用的是同一份数据源。
//
// 名称默认填成「Minecraft <版本>」，用户可以直接改。名称校验交给后端
// （`CreateInstanceInput::validate`：非空、最多 48 字符），这里只做「不为空」
// 的前置检查，避免无意义的往返。

import { useEffect, useMemo, useRef, useState } from 'react';
import { Search } from 'lucide-react';
import { Button } from './Button';
import { Input } from './Input';
import { Modal } from './Modal';
import { useI18n } from '../utils/i18n';
import type { MessageKey } from '../utils/i18n/messages/zh-CN';
import { errorText, listVersions, type VersionListItem } from '../utils/native';
import { kindOfVersion, VERSION_KINDS, VERSION_KIND_KEYS } from '../utils/store/format';
import type { VersionKind } from '../utils/store/types';
import styles from './CreateInstanceDialog.module.css';

interface CreateInstanceDialogProps {
  isOpen: boolean;
  onClose: () => void;
  /** 创建成功：父组件刷新实例列表并选中新实例。 */
  onCreated: (name: string, version: string) => Promise<void> | void;
}

export function CreateInstanceDialog({ isOpen, onClose, onCreated }: CreateInstanceDialogProps) {
  const { t, language } = useI18n();
  const [name, setName] = useState('');
  const [query, setQuery] = useState('');
  const [kinds, setKinds] = useState<VersionKind[]>(['release']);
  const [versions, setVersions] = useState<VersionListItem[]>([]);
  const [state, setState] = useState<'idle' | 'loading' | 'ready' | 'error'>('idle');
  const [error, setError] = useState('');
  const [selected, setSelected] = useState('');
  const [submitting, setSubmitting] = useState(false);
  /** 用户是否手动改过名称。改过之后就不再自动跟随版本号覆盖。 */
  const nameTouched = useRef(false);

  // 每次打开都重新拉一次清单：Mojang 会发布新版本，缓存超过一次会话就没有意义。
  useEffect(() => {
    if (!isOpen) return;
    let cancelled = false;
    setState('loading');
    setError('');
    void listVersions()
      .then(list => {
        if (cancelled) return;
        setVersions(list);
        setState('ready');
      })
      .catch(cause => {
        if (cancelled) return;
        setError(errorText(cause, '无法读取版本列表。'));
        setState('error');
      });
    return () => { cancelled = true; };
  }, [isOpen]);

  // 关闭时清空，避免下次打开看到上一次的输入残留。
  useEffect(() => {
    if (isOpen) return;
    setName('');
    setQuery('');
    setSelected('');
    setKinds(['release']);
    nameTouched.current = false;
    setSubmitting(false);
  }, [isOpen]);

  const rows = useMemo(() => {
    const needle = query.trim().toLowerCase();
    return versions
      .filter(version => kinds.includes(kindOfVersion(version.id, version.type)))
      .filter(version => !needle || version.id.toLowerCase().includes(needle))
      // 新版本在前：清单本身已按发布时间倒序，这里只做一层保险，避免后端顺序变化。
      .sort((a, b) => Date.parse(b.releaseTime) - Date.parse(a.releaseTime));
  }, [kinds, query, versions]);

  const choose = (version: VersionListItem) => {
    setSelected(version.id);
    if (!nameTouched.current) setName(`Minecraft ${version.id}`);
  };

  const toggleKind = (kind: VersionKind) => {
    setKinds(current => {
      const next = current.includes(kind) ? current.filter(item => item !== kind) : [...current, kind];
      // 至少保留一类：全部取消后列表会空掉，用户会以为加载失败。
      return next.length === 0 ? current : next;
    });
  };

  const submit = async () => {
    const trimmed = name.trim();
    if (!selected) { setError('请先选择一个游戏版本。'); return; }
    if (!trimmed) { setError('请填写实例名称。'); return; }
    setSubmitting(true);
    setError('');
    try {
      await onCreated(trimmed, selected);
      onClose();
    } catch (cause) {
      setError(errorText(cause, '创建实例失败。'));
    } finally {
      setSubmitting(false);
    }
  };

  const dateOf = (value: string) => new Date(value).toLocaleDateString(
    language === 'zh-CN' ? 'zh-CN' : language === 'ja' ? 'ja-JP' : 'en-US',
    { year: 'numeric', month: 'short', day: 'numeric' },
  );

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={t('create.title')}
      size="lg"
      footer={
        <>
          <Button variant="ghost" onClick={onClose} disabled={submitting}>{t('common.cancel')}</Button>
          <Button onClick={() => void submit()} loading={submitting} disabled={!selected}>
            {t('create.submit')}
          </Button>
        </>
      }
    >
      <div className={styles.form}>
        <Input
          label={t('create.name')}
          value={name}
          onChange={event => { nameTouched.current = true; setName(event.target.value); }}
          placeholder={t('create.namePlaceholder')}
          hint={t('create.nameHint')}
          maxLength={48}
        />

        <div className={styles.filters}>
          <Input
            label={t('create.version')}
            value={query}
            onChange={event => setQuery(event.target.value)}
            placeholder={t('store.searchPlaceholderVersion')}
            icon={<Search size={15} />}
          />
          <div className={styles.kinds} role="group" aria-label={t('store.versionType')}>
            {VERSION_KINDS.map(kind => (
              <label key={kind} className={styles.kind}>
                <input
                  type="checkbox"
                  checked={kinds.includes(kind)}
                  onChange={() => toggleKind(kind)}
                />
                <span>{t(VERSION_KIND_KEYS[kind] as MessageKey)}</span>
              </label>
            ))}
          </div>
        </div>

        {state === 'error' && <p className={styles.error} role="alert">{error}</p>}

        <div className={styles.list} role="radiogroup" aria-label={t('create.version')}>
          {state === 'loading' && <p className={styles.hint}>{t('common.loading')}</p>}
          {state === 'ready' && rows.length === 0 && <p className={styles.hint}>{t('create.noVersion')}</p>}
          {rows.map(version => {
            const checked = selected === version.id;
            return (
              <button
                key={version.id}
                type="button"
                role="radio"
                aria-checked={checked}
                className={`${styles.row} ${checked ? styles.rowActive : ''}`}
                onClick={() => choose(version)}
              >
                <span className={styles.rowMain}>
                  <strong>{version.id}</strong>
                  <small>{t(VERSION_KIND_KEYS[kindOfVersion(version.id, version.type)] as MessageKey)} · {dateOf(version.releaseTime)}</small>
                </span>
                {checked && <span className={styles.tick} aria-hidden="true">✓</span>}
              </button>
            );
          })}
        </div>

        {error && state !== 'error' && <p className={styles.error} role="alert">{error}</p>}
      </div>
    </Modal>
  );
}
