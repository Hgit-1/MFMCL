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
import {
  errorText,
  listVersions,
  loaderVersions,
  type LoaderKind,
  type LoaderVersion,
  type VersionListItem,
} from '../utils/native';
import { kindOfVersion, VERSION_KINDS, VERSION_KIND_KEYS } from '../utils/store/format';
import type { VersionKind } from '../utils/store/types';
import styles from './CreateInstanceDialog.module.css';

/**
 * 可选加载器，按「装机代价」排序：先给快的两个。
 *
 * 顺序不是随意的。Fabric / Quilt 发布的是 profile json，十几秒；Forge / NeoForge
 * 要跑 installer 的 Java processor，几分钟。把便宜的先放，默认选择就是低成本的。
 */
const LOADER_KINDS: { id: LoaderKind; label: string; heavy: boolean }[] = [
  { id: 'fabric', label: 'Fabric', heavy: false },
  { id: 'quilt', label: 'Quilt', heavy: false },
  { id: 'forge', label: 'Forge', heavy: true },
  { id: 'neoforge', label: 'NeoForge', heavy: true },
];

interface CreateInstanceDialogProps {
  isOpen: boolean;
  onClose: () => void;
  /**
   * 创建成功。
   *
   * `loader` 形如 `fabric@0.16.10`；为 `null` 表示原版实例。父组件负责把它交给
   * 后端（写进实例的加载器请求标记），装加载器本身发生在随后的安装流程里。
   */
  onCreated: (name: string, version: string, loader: string | null) => Promise<void> | void;
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

  /*
   * 加载器选择。
   *
   * `loaderKind === ''` 就是原版——把它做成「一个都不选」而不是额外的「原版」
   * 卡片，是因为原版是默认结果：用户不需要为「不装加载器」专门点一下。
   */
  const [loaderKind, setLoaderKind] = useState<LoaderKind | ''>('');
  const [loaderState, setLoaderState] = useState<'idle' | 'loading' | 'ready' | 'error'>('idle');
  const [loaderList, setLoaderList] = useState<LoaderVersion[]>([]);
  const [loaderVersion, setLoaderVersion] = useState('');
  const [loaderError, setLoaderError] = useState('');

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
    setLoaderKind('');
    setLoaderList([]);
    setLoaderVersion('');
    setLoaderState('idle');
    setLoaderError('');
    nameTouched.current = false;
    setSubmitting(false);
  }, [isOpen]);

  /*
   * 选定加载器后按「加载器 + 游戏版本」拉一次版本列表。
   *
   * 两个依赖缺一不可：加载器版本与 MC 版本强绑定（Forge 的 1.20.1 与 1.21.1 是
   * 两套完全不同的版本号），换了任何一个旧列表都失效。
   *
   * 取消勾选加载器时不发请求，只清空——原版实例没有加载器版本可谈。
   */
  useEffect(() => {
    if (!isOpen || !loaderKind || !selected) return;
    let cancelled = false;
    setLoaderState('loading');
    setLoaderError('');
    setLoaderList([]);
    setLoaderVersion('');
    void loaderVersions(loaderKind, selected)
      .then(list => {
        if (cancelled) return;
        setLoaderList(list);
        // 默认选第一个稳定版；没有稳定版时退回首项，而不是留空让用户多点一次。
        setLoaderVersion((list.find(item => item.stable) ?? list[0])?.version ?? '');
        setLoaderState('ready');
      })
      .catch(cause => {
        if (cancelled) return;
        setLoaderError(errorText(cause, '无法读取加载器版本。'));
        setLoaderState('error');
      });
    return () => { cancelled = true; };
  }, [isOpen, loaderKind, selected]);

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
    /*
     * 选了加载器但版本还没读出来（或读取失败）时不允许提交。
     *
     * 否则会建出一个「用户以为带加载器、实际是原版」的实例——那比直接报错更糟，
     * 因为界面上一切都显示正常，直到他装模组时才发现游戏里没有加载器。
     */
    if (loaderKind && !loaderVersion) {
      setError(loaderState === 'loading' ? t('create.loaderPending') : t('create.loaderNoVersion'));
      return;
    }
    setSubmitting(true);
    setError('');
    try {
      await onCreated(trimmed, selected, loaderKind ? `${loaderKind}@${loaderVersion}` : null);
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

        {/*
          加载器选择。

          ## 为什么放在这里，而不是等实例建好再去设置页

          之前「新建实例」只问版本，建出来永远是原版。用户想装模组，得先发现实例
          设置里有一张加载器卡片——而他建实例时完全没被提示过这件事。等他看到模组
          列表是空的，才知道还要回去做一件从没提过的事。

          ## 为什么不是「必须先选」

          原版实例是合法且常见的：只玩原版、临时试一个新快照、给整合包当模板。
          所以默认不选，加载器是一个可选的第二问，不是流程上的第二道门。

          ## 为什么选完不在这里装

          装加载器要下载几十到几百 MB，Forge/NeoForge 还要跑几分钟的 Java
          processor。这个对话框只负责把选择记下来（写进实例的加载器请求标记），
          真正的下载发生在随后的安装流程里——那里有进度条、可取消、失败有回滚。
        */}
        <fieldset className={styles.loaderBlock}>
          <legend className={styles.loaderLegend}>{t('create.loader')}</legend>
          <p className={styles.loaderHint}>{t('create.loaderHint')}</p>

          <div className={styles.loaderKinds} role="radiogroup" aria-label={t('create.loader')}>
            {/* 「不安装」本身是一个选项，而不是「什么都不点」：用户需要能明确改回原版。 */}
            <button
              type="button"
              role="radio"
              aria-checked={loaderKind === ''}
              className={`${styles.loaderChip} ${loaderKind === '' ? styles.loaderChipActive : ''}`}
              onClick={() => setLoaderKind('')}
              disabled={!selected}
            >
              <strong>{t('create.loaderNone')}</strong>
              <small>{t('create.loaderNoneHint')}</small>
            </button>
            {LOADER_KINDS.map(item => {
              const active = loaderKind === item.id;
              return (
                <button
                  key={item.id}
                  type="button"
                  role="radio"
                  aria-checked={active}
                  className={`${styles.loaderChip} ${active ? styles.loaderChipActive : ''}`}
                  onClick={() => setLoaderKind(item.id)}
                  disabled={!selected}
                >
                  <strong>{item.label}</strong>
                  <small>
                    {item.heavy ? t('create.loaderHeavy') : t('create.loaderLight')}
                  </small>
                </button>
              );
            })}
          </div>

          {/* 没选游戏版本时加载器无从谈起：版本列表要按 MC 版本查。 */}
          {!selected && <p className={styles.loaderNote}>{t('create.loaderPickVersion')}</p>}

          {selected && loaderKind && (
            <label className={styles.loaderSelect}>
              <span>{t('loader.version')}</span>
              {loaderState === 'loading' && <em className={styles.loaderNote}>{t('loader.detecting')}</em>}
              {loaderState === 'error' && (
                <em className={styles.loaderError} role="alert">{loaderError}</em>
              )}
              {loaderState === 'ready' && loaderList.length === 0 && (
                <em className={styles.loaderNote}>
                  {t('loader.noVersions', { values: { version: selected } })}
                </em>
              )}
              {loaderState === 'ready' && loaderList.length > 0 && (
                <select
                  value={loaderVersion}
                  onChange={event => setLoaderVersion(event.target.value)}
                >
                  {loaderList.map(item => (
                    <option key={item.version} value={item.version}>
                      {item.version}{item.stable ? '' : t('loader.beta')}
                    </option>
                  ))}
                </select>
              )}
            </label>
          )}
        </fieldset>

        {error && state !== 'error' && <p className={styles.error} role="alert">{error}</p>}
      </div>
    </Modal>
  );
}
