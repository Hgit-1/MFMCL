// 分段选择控件：2–3 档，左侧带一个定宽的「覆盖关系」插槽。
//
// 为什么单独抽出来：实例管理页里既有布尔项，也有值项。两者必须是同一套控件——
// 同一列里出现两种高度、两种圆角、两种字号的选择器，会让整页看起来像拼装的。
//
// 交互取舍：
//   - 用分段按钮而不是真正的多位置滑块。三段滑块在 190px 宽度内每段只有
//     62px，鼠标命中区太窄，触摸更难；而且「跟随全局」的位置语义不如文字直白。
//   - 各段严格等宽：靠左/右的位置本身传达状态，比只改文字颜色更直观。
//   - 选中态用一个滑块承载，切换时靠 transform 位移。分段本身只改文字颜色，
//     于是"选中"是一段看得见去向的移动，而不是底色瞬间跳变。
//
// 布局契约（这一条决定了整页能不能"对整齐"）：
//   左侧插槽是**无条件占位**的定宽列，即使这一行没有徽标也保留空白。按钮组宽度
//   只由档数决定，与徽标文字、当前状态、全局取值都无关。于是同一列里每一个控件
//   的按钮组都落在一个完全重合的矩形里——从上一行扫到下一行，滑块的位置是可以
//   预期的，不会因为「有没有徽标」「徽标是哪个语言」而左右挪动。
//
//   代价是没徽标的行左侧会空出一段。这不是浪费：那一段正是"徽标可能出现"的位置，
//   留着它，徽标出现/消失就只改变内容、不改变几何。

import { useCallback, useEffect, useRef, useState, type CSSProperties, type KeyboardEvent, type ReactNode } from 'react';
import styles from './SegmentedChoice.module.css';

export interface SegmentedChoiceOption<T> {
  value: T;
  text: string;
  /** 该档表示「不覆盖」时用中性选中态，避免看起来像已启用某个功能。 */
  neutral?: boolean;
  title?: string;
}

export interface SegmentedChoiceProps<T> {
  value: T;
  options: SegmentedChoiceOption<T>[];
  onChange: (value: T) => void;
  /** 无障碍标签：说明这是哪一项设置。 */
  label: string;
  /** 按钮组下方的补充说明，例如「全局为关」。为空的档位保留空白占位。 */
  caption?: string;
  /** 按钮组前面的内容（通常是可点击的「跟随全局」徽标）。位置始终预留。 */
  leading?: ReactNode;
  disabled?: boolean;
}

export function SegmentedChoice<T extends string | number | boolean | undefined>({
  value,
  options,
  onChange,
  label,
  caption,
  leading,
  disabled = false,
}: SegmentedChoiceProps<T>) {
  const buttonsRef = useRef<Array<HTMLButtonElement | null>>([]);

  // 值可能来自全局（例如 undefined），找不到对应档位时就让滑块保持隐藏，
  // 不要退化成"高亮第一档"——那会把「未选择」误读成「选中了某一项」。
  const activeIndex = options.findIndex(option => option.value === value);
  const activeOption = activeIndex >= 0 ? options[activeIndex] : undefined;

  /**
   * 滑块位移时的一点点「拉伸回弹」。
   *
   * 位移本身由 transform 过渡承担；这里只在索引真正变化后短暂挂一个 class，
   * 让滑块内部的色块先压窄再弹回（类似 iOS 分段控件的橡皮筋）。用 class 而不是
   * 常驻动画，是因为常驻会让每次重渲染都重放一遍。
   */
  const [gliding, setGliding] = useState(false);
  const previousIndex = useRef(activeIndex);
  useEffect(() => {
    if (previousIndex.current === activeIndex) return;
    previousIndex.current = activeIndex;
    setGliding(true);
    const timer = window.setTimeout(() => setGliding(false), 280);
    return () => window.clearTimeout(timer);
  }, [activeIndex]);

  // 进度相关变量挂在最外层，`.line` 的网格列与 `.group` 的宽度读的是同一组值，
  // 左右两列不会各自算出不同的小数而错开半个像素。
  const layoutStyle = {
    '--seg-count': options.length,
    '--seg-index': Math.max(0, activeIndex),
  } as CSSProperties;

  /**
   * 方向键在档位间移动。
   *
   * radiogroup 的键盘约定是「一组里只有一个 Tab 停靠点，其余用方向键切换」，
   * 不实现的话键盘用户必须 Tab 穿过多档才能到下一个设置项。
   */
  const onKeyDown = useCallback(
    (event: KeyboardEvent<HTMLDivElement>) => {
      if (disabled) return;
      const forward = event.key === 'ArrowRight' || event.key === 'ArrowDown';
      const backward = event.key === 'ArrowLeft' || event.key === 'ArrowUp';
      if (!forward && !backward) return;
      event.preventDefault();
      const step = forward ? 1 : -1;
      const start = activeIndex >= 0 ? activeIndex : forward ? -1 : options.length;
      const next = (start + step + options.length) % options.length;
      onChange(options[next].value);
      buttonsRef.current[next]?.focus();
    },
    [activeIndex, disabled, onChange, options],
  );

  return (
    <div className={styles.wrap} style={layoutStyle}>
      <span className={styles.line}>
        {/* 插槽固定：没有徽标时也渲染一个空盒子，按钮组的 x 才不会跟着变。 */}
        <span className={styles.lead}>{leading}</span>
        <span
          className={styles.group}
          role="radiogroup"
          aria-label={label}
          onKeyDown={onKeyDown}
        >
          <span
            aria-hidden="true"
            className={`${styles.thumb} ${activeIndex >= 0 ? styles.thumbVisible : ''} ${activeOption?.neutral ? styles.thumbNeutral : ''} ${gliding ? styles.thumbGliding : ''}`}
          >
            {/*
              位移与形变必须分在两层元素上。
              位移由外层 .thumb 的 transform 过渡承担，橡皮筋由内层 .thumbFill 的
              scaleX 动画承担。放在同一个元素上时，@keyframes 里的 transform 会整段
              接管该属性，把过渡替换掉——滑块就会"闪现"到目标档而不是滑过去。
            */}
            <span className={styles.thumbFill} />
          </span>
          {options.map((option, index) => {
            const active = option.value === value;
            return (
              <button
                key={String(option.value)}
                ref={element => {
                  buttonsRef.current[index] = element;
                }}
                type="button"
                role="radio"
                aria-checked={active}
                // 只有选中档进入 Tab 序列，组内移动交给方向键。
                tabIndex={active || (activeIndex < 0 && index === 0) ? 0 : -1}
                title={option.title}
                className={`${styles.segment} ${active ? styles.active : ''} ${option.neutral ? styles.neutral : ''}`}
                disabled={disabled}
                onClick={() => onChange(option.value)}
              >
                {option.text}
              </button>
            );
          })}
        </span>
      </span>
      {/*
        说明行始终渲染：内容为空时透明，但仍占住同一块高度。
        这样「跟随全局」的提示出现或消失都不会把整行往上/往下顶。
      */}
      <span className={`${styles.caption} ${caption ? '' : styles.captionEmpty}`} aria-live="polite">
        {caption ?? ''}
      </span>
    </div>
  );
}

export default SegmentedChoice;
