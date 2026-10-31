// 带继承的取值控件：按钮组表示取值，按钮组前面的徽标表示「是否覆盖全局」。
//
// 为什么不做三档：把「跟随全局」当成第三个取值塞在按钮组中间，会让按钮组
// 出现一个位置固定、语义却不同的选项——用户在按钮里找的是「这项现在是哪个值」，
// 而「跟随全局」回答的是「这项要不要覆盖全局」。两种问题混在同一排按钮里，
// 中间档既不像取值也不像状态，还挤掉了左右两档的命中宽度。更糟的是三段等宽时
// 「跟随全局」四个字最宽，会把左右两档压到只剩「关」「开」，视觉上像被截断。
//
// 所以拆成两件事：
//   - 徽标（按钮左侧）表示覆盖关系，亮起=跟随全局，点击即恢复继承；
//   - 按钮组表示取值，选中项始终显示当前实际生效的值，点任一项都会写入明确的值，
//     从而自动脱钩。

import type { ReactNode } from 'react';
import { useI18n } from '../utils/i18n';
import { FollowGlobalBadge } from './FollowGlobalBadge';
import { SegmentedChoice, type SegmentedChoiceOption } from './SegmentedChoice';

export interface OverrideChoiceProps<T> {
  /** true = 跟随全局（没有实例级取值）。 */
  inheriting: boolean;
  /** 按钮上显示的值：跟随时应传入全局生效值，这样用户看到的就是真正会用的值。 */
  value: T;
  /** 可选值（不含「继承」这一档）。 */
  options: SegmentedChoiceOption<T>[];
  /** 选择具体取值：实现方应同时写入该值，从而脱离继承。 */
  onChange: (value: T) => void;
  /** 点击徽标：恢复跟随全局。 */
  onInherit: () => void;
  /** 无障碍标签：说明这是哪一项设置。 */
  label: string;
  /** 全局取值的可读文本，用于徽标提示与说明行。 */
  globalState: string;
  /** 说明行模板，{state} 会替换为全局取值；省略时使用默认文案。 */
  caption?: ReactNode;
  disabled?: boolean;
}

export function OverrideChoice<T extends string | number | boolean>({
  inheriting,
  value,
  options,
  onChange,
  onInherit,
  label,
  globalState,
  caption,
  disabled = false,
}: OverrideChoiceProps<T>) {
  const { t } = useI18n();

  return (
    <SegmentedChoice
      value={value}
      options={options}
      onChange={onChange}
      label={label}
      disabled={disabled}
      // 说明行只说「跟随时会用哪个值」；脱离继承后它没有内容，但位置仍然保留。
      caption={
        inheriting
          ? (typeof caption === 'string' ? caption : t('override.globalIs', { values: { state: globalState } }))
          : undefined
      }
      leading={
        <FollowGlobalBadge
          active={inheriting}
          text={t('override.inherit')}
          title={t('override.followTitle', { values: { state: globalState } })}
          disabled={disabled}
          onClick={onInherit}
        />
      }
    />
  );
}

export default OverrideChoice;
