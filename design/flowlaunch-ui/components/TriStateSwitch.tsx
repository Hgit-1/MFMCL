// 三档布尔开关：关 ←→ 跟随全局 ←→ 开。
//
// 中间档是「继承」。这是管理页里每个布尔项的默认状态，也是用户最常停留的状态，
// 所以它必须和另外两档一样好点、一样清楚，而不是塞进某个下拉框里。
//
// 布局与选中态由 SegmentedChoice 负责（同一个页面里二档值项也用那个控件），
// 这里只负责三档布尔语义与词典文案。

import { useI18n } from '../utils/i18n';
import { SegmentedChoice, type SegmentedChoiceOption } from './SegmentedChoice';

export interface TriStateSwitchProps {
  /** undefined = 跟随全局。 */
  value: boolean | undefined;
  /** 全局当前值，用于在「跟随全局」档里显示「（全局为关）」。 */
  globalValue: boolean;
  onChange: (next: boolean | undefined) => void;
  /** 无障碍标签：说明这是哪一项设置。 */
  label: string;
  disabled?: boolean;
}

export function TriStateSwitch({ value, globalValue, onChange, label, disabled = false }: TriStateSwitchProps) {
  const { t } = useI18n();
  const options: SegmentedChoiceOption<boolean | undefined>[] = [
    { value: false, text: t('override.off') },
    { value: undefined, text: t('override.inherit'), neutral: true },
    { value: true, text: t('override.on') },
  ];

  return (
    <SegmentedChoice
      value={value}
      options={options}
      onChange={onChange}
      label={label}
      disabled={disabled}
      /*
        继承档时把全局值写出来。单独放控件右侧而不是塞进按钮里：按钮文字要保持
        短，而「跟随全局（开）」这种拼接在窄屏会换行，破坏三段等宽。
      */
      caption={value === undefined
        ? t('override.globalIs', { values: { state: globalValue ? t('override.on') : t('override.off') } })
        : undefined}
    />
  );
}

export default TriStateSwitch;
