import React, { useEffect, useRef, useState } from 'react';
import styles from './NumberStepper.module.css';

export interface NumberStepperProps {
  /** 当前值。受控组件：改动通过 `onChange` 回传，自身不保留真值。 */
  value: number;
  onChange: (value: number) => void;
  min: number;
  max: number;
  /** 每次点 +/- 的步长。直接输入时不受它限制。 */
  step?: number;
  /** 降级/越界时如何归整。默认向上取整（超时、条数这类不能是小数）。 */
  round?: (value: number) => number;
  /** 单位后缀，例如「秒」「次」。 */
  suffix?: string;
  /** 无障碍标签：数字本身说明不了这是什么，必须由调用方给出。 */
  label: string;
}

/**
 * 数字步进器：可以点加减，也可以点数字直接输入。
 *
 * 为什么自己写而不是用 `<input type="number">` 加两个按钮：
 *
 *   1. 原生 number 输入在部分 WebView 里仍显示上下小箭头，与两侧的按钮重复；
 *   2. 输入过程中不能立刻夹逼到区间——用户想输 300，删到「3」时被夹成 10，
 *      接下来输入就变成了「3」+「0」→ 30，永远输不到目标值。所以编辑期间只
 *      过滤非法字符，等失焦或回车时才夹逼；
 *   3. 空字符串必须被允许短暂存在，否则清空后立刻填入最小值，光标位置会跳。
 */
export const NumberStepper: React.FC<NumberStepperProps> = ({
  value,
  onChange,
  min,
  max,
  step = 1,
  round = Math.ceil,
  suffix,
  label,
}) => {
  const [editing, setEditing] = useState(false);
  /** 编辑中的原始文本。null 表示「没在编辑」，显示 `value`。 */
  const [draft, setDraft] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  // 进入编辑时选中全部：用户点数字通常是想整段替换，而不是在后面追加。
  useEffect(() => {
    if (editing) inputRef.current?.select();
  }, [editing]);

  const clamp = (raw: number): number => {
    if (!Number.isFinite(raw)) return value;
    return Math.min(max, Math.max(min, round(raw)));
  };

  /** 把编辑中的文本落定。非法或空内容回退到当前值，不留一个空输入框。 */
  const commit = () => {
    if (draft === null) return;
    const parsed = Number(draft);
    const next = draft.trim() === '' || !Number.isFinite(parsed) ? value : clamp(parsed);
    setDraft(null);
    setEditing(false);
    if (next !== value) onChange(next);
  };

  const nudge = (delta: number) => {
    // 从当前显示值出发，而不是从 draft：点击按钮时编辑已经结束。
    onChange(clamp(value + delta));
  };

  return (
    <div className={styles.stepper}>
      <button
        type="button"
        onClick={() => nudge(-step)}
        disabled={value <= min}
        aria-label={`${label} -${step}`}
      >
        −
      </button>
      {editing ? (
        <input
          ref={inputRef}
          className={styles.input}
          type="text"
          inputMode="numeric"
          value={draft ?? ''}
          aria-label={label}
          onChange={event => {
            /*
             * 只接受数字。负号放行是因为夹逼在提交时做，而输入过程中用户可能先
             * 敲一个负号再改成别的；直接拒绝它会让按键看起来没反应。
             */
            const next = event.target.value;
            if (/^-?\d*$/.test(next)) setDraft(next);
          }}
          onBlur={commit}
          onKeyDown={event => {
            if (event.key === 'Enter') {
              event.preventDefault();
              commit();
            }
            if (event.key === 'Escape') {
              // 放弃这次编辑，恢复原值。与浏览器地址栏的语义一致。
              setDraft(null);
              setEditing(false);
            }
          }}
        />
      ) : (
        <button
          type="button"
          className={styles.value}
          onClick={() => {
            setDraft(String(value));
            setEditing(true);
          }}
          aria-label={`${label}: ${value}. 点击输入`}
          title="点击输入"
        >
          {value}
        </button>
      )}
      <button
        type="button"
        onClick={() => nudge(step)}
        disabled={value >= max}
        aria-label={`${label} +${step}`}
      >
        +
      </button>
      {suffix && <small>{suffix}</small>}
    </div>
  );
};
