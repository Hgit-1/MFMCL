// 「跟随全局」徽标：放在分段按钮组左侧，表示这一项是否覆盖全局设置。
//
// 它不是一个取值，所以不能用分段按钮的形式出现——那会让按钮组里混进一个
// 「继承」档，和左右两档的取值语义打架（用户看到的就是「关 / 跟随全局 / 开」
// 这种既不像取值也不像状态的中间档）。徽标单独成键：亮起=未覆盖（跟随全局），
// 点击=恢复继承。
//
// 全局取值不写在徽标里：那会让徽标在两个宽度之间切换，按钮组跟着左右漂。
// 徽标只显示一个圆点 + 固定文字，宽度恒定；「跟随时实际用哪个值」写在按钮组
// 下方的说明行里（见 SegmentedChoice 的 caption）。

import styles from './SegmentedChoice.module.css';

export interface FollowGlobalBadgeProps {
  /** 当前是否处于跟随全局状态。 */
  active: boolean;
  /** 徽标文字，例如「跟随全局」。 */
  text: string;
  /** 悬停/无障碍说明，应包含全局当前取值。 */
  title: string;
  disabled?: boolean;
  onClick: () => void;
}

export function FollowGlobalBadge({
  active,
  text,
  title,
  disabled = false,
  onClick,
}: FollowGlobalBadgeProps) {
  return (
    <button
      type="button"
      className={`${styles.follow} ${active ? styles.followActive : ''}`}
      aria-pressed={active}
      aria-label={title}
      title={title}
      disabled={disabled}
      onClick={onClick}
    >
      {/* 圆点是状态的载体：文字不随状态变化，宽度才能恒定。 */}
      <span className={styles.followDot} aria-hidden="true" />
      {text}
    </button>
  );
}

export default FollowGlobalBadge;
