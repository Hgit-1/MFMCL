// 实例级布尔项：左侧「跟随全局」徽标 + 「关 / 开」两档按钮。
//
// 为什么不做三档：把「跟随全局」当成第三个取值塞在 关 和 开 之间，会让按钮组
// 出现一个位置固定、语义却不同的选项——用户在按钮里找的是「这项现在是开还是关」，
// 而「跟随全局」回答的是「这项要不要覆盖全局」。两种问题混在同一排按钮里，
// 中间档既不像取值也不像状态，还挤掉了左右两档的命中宽度。
//
// 所以拆成两件事：
//   - 徽标（按钮左侧，位置恒定）表示覆盖关系，亮起=跟随全局，点击即恢复继承；
//   - 两档按钮表示取值，选中项始终显示当前实际生效的值，点任一项都会写入明确的
//     false / true，从而自动脱钩。
//
// 全局取值写在按钮组下方的说明行里，而不是塞进徽标：徽标只回答「是否覆盖」，
// 说明行只回答「跟随时实际用哪个值」。徽标文字因此永不变化，宽度恒定。

import { useI18n } from '../utils/i18n';
import { FollowGlobalBadge } from './FollowGlobalBadge';
import { SegmentedChoice, type SegmentedChoiceOption } from './SegmentedChoice';

export interface TriStateSwitchProps {
  /** undefined = 跟随全局。 */
  value: boolean | undefined;
  /** 全局当前值：跟随全局时按钮显示它，说明行里也会带上它。 */
  globalValue: boolean;
  onChange: (next: boolean | undefined) => void;
  /** 无障碍标签：说明这是哪一项设置。 */
  label: string;
  disabled?: boolean;
}

export function TriStateSwitch({ value, globalValue, onChange, label, disabled = false }: TriStateSwitchProps) {
  const { t } = useI18n();
  const inheriting = value === undefined;
  const effective = inheriting ? globalValue : value;
  const globalState = globalValue ? t('override.on') : t('override.off');

  const options: SegmentedChoiceOption<boolean>[] = [
    { value: false, text: t('override.off') },
    { value: true, text: t('override.on') },
  ];

  return (
    <SegmentedChoice
      value={effective}
      options={options}
      onChange={onChange}
      label={t('override.valueAria', { values: { label } })}
      disabled={disabled}
      caption={inheriting ? t('override.globalIs', { values: { state: globalState } }) : undefined}
      leading={
        <FollowGlobalBadge
          active={inheriting}
          text={t('override.inherit')}
          title={t('override.followTitle', { values: { state: globalState } })}
          disabled={disabled}
          onClick={() => onChange(undefined)}
        />
      }
    />
  );
}

export default TriStateSwitch;
