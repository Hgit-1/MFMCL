// 版本选择器：可以输入任意版本号，也可以从完整清单里挑。
//
// ## 为什么不用 `<select>`
//
// 之前这里是一个内置 16 个大版本的下拉。它的问题不只是「不全」：Mojang 的版本
// 超过 900 个（含快照、远古版、愚人节版），任何硬编码的子集都会随新版本发布
// 而变得不准确；而用户真正想找的可能是 1.20.2、23w31a 或 1.7.2 这种不在
// 常用清单里的版本。
//
// 换成输入框 + 下拉列表之后有三件事同时成立：
//   1. 可以输入任意版本号——即使清单还没加载完、或清单里没有这个版本
//      （第三方服务端的版本别名、自定义版本），也照样能筛；
//   2. 可以浏览全量清单，按发布时间倒序，带日期；
//   3. 「跟随实例」作为空值保留，语义与之前一致。
//
// ## 为什么下拉渲染到 portal
//
// 它挂在筛选卡里，而筛选卡在一个 `overflow-y: auto` 的面板内。绝对定位的下拉
// 会被面板裁掉——尤其当筛选卡靠近面板底部时，列表只剩几十像素可见。portal +
// `position: fixed` 不受任何祖先的 overflow 影响，是这类控件的标准解法。
//
// 代价是位置需要在滚动/窗口尺寸变化时重算，见 `useLayoutEffect`。

import { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Check, ChevronDown, Search } from 'lucide-react';
import { useI18n } from '../utils/i18n';
import { formatDate, kindOfVersion, VERSION_KIND_KEYS } from '../utils/store/format';
import type { VersionListItem } from '../utils/native';
import styles from './VersionPicker.module.css';

interface VersionPickerProps {
  /** 已提交的版本号；空串表示「跟随实例」。 */
  value: string;
  /** 选中或输入完成时调用。空串表示回到跟随实例。 */
  onChange: (value: string) => void;
  /** 「跟随实例」这一项显示的文字（含实际版本号）。 */
  followLabel: string;
  /** 完整清单，按发布时间倒序。父组件负责加载与缓存。 */
  versions: VersionListItem[];
  state: 'idle' | 'loading' | 'ready' | 'error';
  /** 第一次展开时触发加载，避免在没人看版本时也拉一份清单。 */
  onRequestVersions: () => void;
  /** 输入框的无障碍标签。 */
  label: string;
  placeholder?: string;
  id?: string;
}

/** 下拉最多同时展示多少项。900 个版本一次性铺开会让滚动变得没有意义。 */
const MAX_VISIBLE = 300;

export function VersionPicker({
  value,
  onChange,
  followLabel,
  versions,
  state,
  onRequestVersions,
  label,
  placeholder,
  id,
}: VersionPickerProps) {
  const { t, language } = useI18n();
  /** 输入框里的文字。与已提交值分开：用户打字的过程中不该立刻触发重搜。 */
  const [text, setText] = useState(value);
  const [open, setOpen] = useState(false);
  /** 键盘高亮项的下标；-1 表示没有高亮（此时回车提交输入框里的文字）。 */
  const [active, setActive] = useState(-1);
  const [rect, setRect] = useState<{ top: number; left: number; width: number } | null>(null);

  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const generatedId = useId();
  const listId = `${generatedId}-list`;

  // 外部值变化（例如「重置条件」）时同步输入框。用值本身做依赖即可：
  // 用户打字不会改变 `value`，所以不会把正在输入的内容冲掉。
  useEffect(() => { setText(value); }, [value]);

  /*
   * 按发布时间倒序。
   *
   * 排序放在这里而不是信任数据源的顺序：`list_versions` 与浏览器直连 Mojang
   * 走的是同一条清单，但顺序不属于接口契约。倒序是用户唯一能接受的顺序——
   * 想找的几乎总是较新的版本，从上往下读比从 2009 年翻起快得多。
   */
  const sorted = useMemo(
    () => [...versions].sort((a, b) => Date.parse(b.releaseTime) - Date.parse(a.releaseTime)),
    [versions],
  );

  /** 过滤后的列表：输入框里的内容同时是搜索词。空输入时展示全部（按时间倒序）。 */
  const matches = useMemo(() => {
    const needle = text.trim().toLowerCase();
    const filtered = needle
      ? sorted.filter(item => item.id.toLowerCase().includes(needle))
      : sorted;
    return filtered.slice(0, MAX_VISIBLE);
  }, [sorted, text]);

  /** 是否可以把输入框里的文字原样当作版本号提交（清单里没有也算）。 */
  const typedVersion = text.trim();
  const typedIsNew = typedVersion !== '' && !sorted.some(item => item.id === typedVersion);

  const openList = useCallback(() => {
    setOpen(true);
    setActive(-1);
    onRequestVersions();
  }, [onRequestVersions]);

  /*
   * 计算下拉的位置。
   *
   * 用 `getBoundingClientRect` + `position: fixed`：与 portal 的坐标系一致，
   * 不受祖先滚动影响。滚动时重算而不是关闭——用户在长页面里滚动时列表跟着
   * 输入框走，比突然消失更符合预期。
   */
  const reposition = useCallback(() => {
    const node = inputRef.current;
    if (!node) return;
    const box = node.getBoundingClientRect();
    setRect({ top: box.bottom + 4, left: box.left, width: box.width });
  }, []);

  useLayoutEffect(() => {
    if (!open) return;
    reposition();
    window.addEventListener('scroll', reposition, true);
    window.addEventListener('resize', reposition);
    return () => {
      window.removeEventListener('scroll', reposition, true);
      window.removeEventListener('resize', reposition);
    };
  }, [open, reposition]);

  /* 点到组件之外就收起。用 pointerdown 而不是 click：click 会在选中项之后冒泡上来。 */
  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node;
      if (inputRef.current?.contains(target)) return;
      if (listRef.current?.contains(target)) return;
      setOpen(false);
      /*
       * 收起时把输入框里的内容提交掉。
       *
       * 这里必须直接读 DOM 的当前值，不能用闭包里的 `text`：这个 effect 只依赖
       * `open`，而用户在浮层打开期间打字不会改变 `open`，闭包里的 `text` 会一直
       * 停在打开那一刻。之前用闭包值导致「输入 1.20.5 后点别处」既不提交也不报错，
       * 输入框显示 1.20.5 而实际筛选还是跟随实例——界面在说谎。
       */
      const typed = inputRef.current?.value.trim() ?? '';
      if (typed !== value) onChange(typed);
    };
    document.addEventListener('pointerdown', onPointerDown);
    return () => document.removeEventListener('pointerdown', onPointerDown);
  }, [open, value, onChange]);

  /** 把高亮项滚进可见范围。键盘上下移动时列表要跟着走。 */
  useEffect(() => {
    if (!open || active < 0) return;
    const node = listRef.current?.querySelector<HTMLElement>(`[data-index="${active}"]`);
    node?.scrollIntoView({ block: 'nearest' });
  }, [active, open]);

  const commit = (next: string) => {
    setText(next);
    setOpen(false);
    setActive(-1);
    if (next !== value) onChange(next);
  };

  /*
   * 收起时提交输入框里的文字。
   *
   * 用户可能输入了一个清单里没有的版本（或只打了前几位就想直接搜）。不提交的话，
   * 输入框显示「1.20」而实际筛选仍是「跟随实例」——界面在说谎。清单里有的版本
   * 在收起时保持原文即可，不需要纠正成规范写法。
   */
  const commitTyped = () => {
    const next = text.trim();
    if (next !== value) onChange(next);
  };

  const onKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
    if (event.key === 'ArrowDown') {
      event.preventDefault();
      if (!open) { openList(); return; }
      setActive(current => Math.min(matches.length - 1, current + 1));
      return;
    }
    if (event.key === 'ArrowUp') {
      event.preventDefault();
      if (!open) return;
      setActive(current => Math.max(-1, current - 1));
      return;
    }
    if (event.key === 'Enter') {
      event.preventDefault();
      if (open && active >= 0 && matches[active]) commit(matches[active].id);
      else commitTyped();
      setOpen(false);
      return;
    }
    if (event.key === 'Escape') {
      if (!open) return;
      event.preventDefault();
      setOpen(false);
      setActive(-1);
      // 放弃未提交的输入，回到已生效的值。
      setText(value);
    }
  };

  const listHeight = rect ? Math.min(320, Math.max(160, window.innerHeight - rect.top - 24)) : 320;

  return (
    <div className={styles.wrap}>
      <div className={`${styles.control} ${open ? styles.controlOpen : ''}`}>
        <Search size={14} className={styles.searchIcon} aria-hidden="true" />
        <input
          ref={inputRef}
          id={id}
          type="text"
          role="combobox"
          aria-label={label}
          aria-expanded={open}
          aria-controls={open ? listId : undefined}
          aria-autocomplete="list"
          aria-activedescendant={open && active >= 0 ? `${listId}-${active}` : undefined}
          className={styles.input}
          value={text}
          placeholder={placeholder ?? followLabel}
          autoComplete="off"
          spellCheck={false}
          onChange={event => {
            setText(event.target.value);
            if (!open) openList();
            setActive(-1);
          }}
          onFocus={() => {
            if (!open) openList();
          }}
          onKeyDown={onKeyDown}
          onBlur={() => {
            /*
             * 失焦不立刻提交。
             *
             * 点下拉项会先触发 blur，如果这里直接提交，就会把用户刚点的选项
             * 覆盖成输入框里残留的旧文字。真正的收尾交给 pointerdown（点到外面）
             * 与回车；这里只处理「用 Tab 键离开且浮层已经收起」这一种情况，
             * 并从 DOM 读当前值，避免用渲染闭包里过期的文字。
             */
            if (open) return;
            const typed = inputRef.current?.value.trim() ?? '';
            if (typed !== value) onChange(typed);
          }}
        />
        <button
          type="button"
          className={styles.caret}
          tabIndex={-1}
          aria-label={label}
          onMouseDown={event => event.preventDefault()}
          onClick={() => {
            if (open) setOpen(false);
            else { openList(); inputRef.current?.focus(); }
          }}
        >
          <ChevronDown size={15} className={open ? styles.caretOpen : undefined} />
        </button>
      </div>

      {/* 当前生效的版本说明。输入框为空时这是唯一的「实际在用什么版本」的线索。 */}
      {value === '' && <span className={styles.followHint}>{followLabel}</span>}

      {open && rect && createPortal(
        <div
          className={styles.popover}
          style={{ top: rect.top, left: rect.left, width: Math.max(rect.width, 240), maxHeight: listHeight }}
        >
          {/* 「跟随实例」永远排在最前：它是默认值，也是清空筛选的入口。 */}
          <button
            type="button"
            className={`${styles.followOption} ${value === '' ? styles.optionActive : ''}`}
            onMouseDown={event => event.preventDefault()}
            onClick={() => commit('')}
          >
            <span>{followLabel}</span>
            {value === '' && <Check size={14} aria-hidden="true" />}
          </button>

          {state === 'loading' && <p className={styles.status}>{t('common.loading')}</p>}
          {state === 'error' && <p className={styles.statusError}>{t('store.versionListFailed')}</p>}

          {state === 'ready' && (
            <ul className={styles.list} role="listbox" id={listId} ref={listRef} aria-label={label}>
              {/* 输入的不是清单里的版本时，把它作为第一项，让用户能直接确认输入的版本。 */}
              {typedIsNew && (
                <li role="none">
                  <button
                    type="button"
                    role="option"
                    aria-selected={value === typedVersion}
                    className={`${styles.option} ${styles.optionTyped}`}
                    onMouseDown={event => event.preventDefault()}
                    onClick={() => commit(typedVersion)}
                  >
                    <span className={styles.optionId}>{typedVersion}</span>
                    <span className={styles.optionMeta}>{t('store.versionUseTyped')}</span>
                  </button>
                </li>
              )}

              {matches.length === 0 && !typedIsNew && (
                <li className={styles.empty}>{t('store.versionNoMatch')}</li>
              )}

              {matches.map((version, index) => {
                const kind = kindOfVersion(version.id, version.type);
                const selected = version.id === value;
                return (
                  <li key={version.id} role="none">
                    <button
                      type="button"
                      role="option"
                      id={`${listId}-${index}`}
                      data-index={index}
                      aria-selected={selected}
                      className={`${styles.option} ${selected ? styles.optionActive : ''} ${active === index ? styles.optionCursor : ''}`}
                      onMouseDown={event => event.preventDefault()}
                      onMouseEnter={() => setActive(index)}
                      onClick={() => commit(version.id)}
                    >
                      <span className={styles.optionId}>{version.id}</span>
                      <span className={styles.optionMeta}>
                        {t(VERSION_KIND_KEYS[kind])}
                        <span className={styles.dot} aria-hidden="true">·</span>
                        {formatDate(Date.parse(version.releaseTime), language)}
                      </span>
                      {selected && <Check size={13} aria-hidden="true" />}
                    </button>
                  </li>
                );
              })}
            </ul>
          )}

          {/* 版本太多时说明为什么只列出一部分，而不是让用户以为清单不完整。 */}
          {state === 'ready' && sorted.length > MAX_VISIBLE && matches.length === MAX_VISIBLE && (
            <p className={styles.status}>{t('store.versionTruncated', { values: { shown: MAX_VISIBLE, total: sorted.length } })}</p>
          )}
        </div>,
        document.body,
      )}
    </div>
  );
}
