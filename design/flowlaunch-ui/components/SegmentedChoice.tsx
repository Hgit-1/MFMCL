// 2–3 档分段选择控件。
//
// 为什么单独抽出来：实例管理页里既有「关 / 跟随全局 / 开」这种三档布尔项，
// 也有「跟随全局 / 自定义」这种二档值项。两者必须是同一套控件——同一列里
// 出现两种高度、两种圆角、两种字号的选择器，会让整页看起来像拼装的。
// TriStateSwitch 就是本控件的布尔特化。
//
// 交互取舍：
//   - 用分段按钮而不是真正的多位置滑块。三段滑块在 190px 宽度内每段只有
//     60px，鼠标命中区太窄，触摸更难；而且「跟随全局」的位置语义（中间）
//     不如文字直白。
//   - 各段严格等宽：靠左/中/右的位置本身传达状态，比只改文字颜色更直观。
//   - 选中档不只用底色区分，因为轨道底色与色调底色差值很小。

import type { CSSProperties } from 'react';
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
  /** 控件下方的补充说明，例如「（全局为关）」；仅在有内容时渲染。 */
  caption?: string;
  disabled?: boolean;
}

export function SegmentedChoice<T extends string | number | boolean | undefined>({
  value,
  options,
  onChange,
  label,
  caption,
  disabled = false,
}: SegmentedChoiceProps<T>) {
  return (
    <div className={styles.wrap}>
      <div
        className={styles.group}
        role="radiogroup"
        aria-label={label}
        style={{ '--seg-count': options.length } as CSSProperties}
      >
        {options.map(option => {
          const active = option.value === value;
          return (
            <button
              key={String(option.value)}
              type="button"
              role="radio"
              aria-checked={active}
              title={option.title}
              className={`${styles.segment} ${active ? styles.active : ''} ${option.neutral ? styles.neutral : ''}`}
              disabled={disabled}
              onClick={() => onChange(option.value)}
            >
              {option.text}
            </button>
          );
        })}
      </div>
      {caption && <span className={styles.caption}>{caption}</span>}
    </div>
  );
}

export default SegmentedChoice;
