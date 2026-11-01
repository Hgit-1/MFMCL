// 设置面板「外观」页里的动效区块。
//
// ## 为什么是四个离散档而不是滑条
//
// 「缓慢 / 标准 / 快速 / 关闭」每一档都能用一句话说清，而连续滑条会让用户停在
// 一个自己也描述不出的位置（1.37 倍），还多一个需要保存和校验的数值。四个档已经
// 覆盖真实需求：想省电的选关闭，觉得晃眼的选缓慢，其余保持标准。
//
// ## 为什么选完立刻生效、不等「保存」
//
// 动效是一种要**看着调**的东西。让用户先在四个选项里猜、点保存、关掉设置、再
// 回来看效果，是这个功能最容易失败的交互。所以这里与背景设置同一条路子：改动
// 即时应用并落盘，不参与设置面板的「保存/取消」流程。
//
// 这不会造成「半保存状态」——它写的是 localStorage 里的一个字符串，不经过后端，
// 也没有会失败的字段校验。

import { Check, Sparkles } from 'lucide-react';
import { useI18n } from '../utils/i18n';
import type { MessageKey } from '../utils/i18n/messages/zh-CN';
import { MOTION_LEVELS, applyMotionLevel, saveMotionLevel, type MotionLevel } from '../utils/motion';
import styles from './MotionSection.module.css';

interface MotionSectionProps {
  level: MotionLevel;
  onChange: (level: MotionLevel) => void;
}

/** 每档的名称与一句说明。说明写的是「这一档会带来什么」而不是「它是什么」。 */
const LABELS: Record<MotionLevel, { name: MessageKey; hint: MessageKey }> = {
  slow: { name: 'motion.slow', hint: 'motion.slowHint' },
  normal: { name: 'motion.normal', hint: 'motion.normalHint' },
  fast: { name: 'motion.fast', hint: 'motion.fastHint' },
  off: { name: 'motion.off', hint: 'motion.offHint' },
};

export function MotionSection({ level, onChange }: MotionSectionProps) {
  const { t } = useI18n();

  const pick = (next: MotionLevel) => {
    // 先应用再上报：应用是纯粹写变量，不会失败；上报让父组件记进偏好。
    applyMotionLevel(next);
    saveMotionLevel(next);
    onChange(next);
  };

  return (
    <section className={styles.group}>
      <h3>{t('motion.title')}</h3>
      <div className={styles.row}>
        <div className={styles.copy}>
          <span className={styles.icon} aria-hidden="true"><Sparkles size={16} /></span>
          <div>
            <strong>{t('motion.speed')}</strong>
            <span>{t('motion.description')}</span>
          </div>
        </div>
        <div className={styles.options} role="radiogroup" aria-label={t('motion.speed')}>
          {MOTION_LEVELS.map(id => {
            const active = level === id;
            return (
              <button
                key={id}
                type="button"
                role="radio"
                aria-checked={active}
                className={active ? styles.active : ''}
                onClick={() => pick(id)}
                title={t(LABELS[id].hint)}
              >
                <span>{t(LABELS[id].name)}</span>
                {active && <Check size={13} aria-hidden="true" />}
              </button>
            );
          })}
        </div>
      </div>
      {/*
        当前档的说明放在下面而不是塞进按钮 title：title 只在悬停时出现，
        而「关闭动效会怎样」是选择前就该读到的一句话。

        前面加一个「当前：」前缀。不加的话，这行灰字会被读成一条警告或禁用说明
        （「完全关闭动画与过渡」很像在说「此功能已停用」），而它实际是在解释
        用户刚选的那一档。
      */}
      <p className={styles.current}>
        {t('motion.current', { values: { level: t(LABELS[level].name) } })}
        {t(LABELS[level].hint)}
      </p>
    </section>
  );
}
