// 动效偏好：开关 + 速度档。
//
// ## 为什么做成「一个变量控制全局」
//
// 各组件里散落着几十处 `transition: ... var(--duration-fast) var(--ease)`。
// 如果动效开关要逐个组件去写条件，那件事永远做不完整——新增一个组件就会漏掉，
// 而「大部分地方有动画、个别地方没有」比完全没有动画更难看。
//
// 所以这里只做一件事：把 `--motion-scale` 写成全局 CSS 变量，并让各组件的时间
// 都乘以它。关掉动效时它是 0，所有 transition/animation 一次性归零；「缓慢」
// 时它是 1.6，全局一起变慢。新增组件只要用 token 就自动跟随。
//
// ## 为什么不用 `prefers-reduced-motion` 就够了
//
// 那条媒体查询是**操作系统级**的偏好，本模块会读它作为默认值。但用户还需要在
// 应用内单独关掉（例如想省电、或觉得动画碍眼），而不改系统设置。两者是叠加关系：
// 系统要求减少动效 → 应用内默认就关；用户还能再手动改。
//
// ## 三个档位而不是滑条
//
// 「缓慢 / 标准 / 快速 / 关闭」四个离散档足够覆盖真实需求，且每个档都能被一句
// 话说清。连续滑条会让用户停在一个自己也无法描述的位置（比如 1.37 倍），
// 却要为此多一个需要保存和校验的数值。

const MOTION_KEY = 'mfmcl-motion';

/** 动效档位。`off` 是彻底关闭，其余是速度倍数。 */
export type MotionLevel = 'slow' | 'normal' | 'fast' | 'off';

/** 每档对应的时长倍数。`off` 用 0 而不是极小值：0 会让 transitionend 立即触发。 */
const SCALE: Record<MotionLevel, number> = {
  slow: 1.6,
  normal: 1,
  fast: 0.6,
  off: 0,
};

/** 档位顺序，供界面按固定顺序渲染按钮。 */
export const MOTION_LEVELS: MotionLevel[] = ['slow', 'normal', 'fast', 'off'];

function isLevel(value: unknown): value is MotionLevel {
  return value === 'slow' || value === 'normal' || value === 'fast' || value === 'off';
}

/**
 * 系统是否要求减少动效。
 *
 * 没有 `matchMedia` 时返回 false（服务端渲染、测试环境），避免把「读不到」当成
 * 「要求减少」——那会让默认体验莫名变成无动画。
 */
export function prefersReducedMotion(): boolean {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return false;
  return window.matchMedia('(prefers-reduced-motion: reduce)').matches;
}

/**
 * 读取动效偏好。
 *
 * 未设置过时跟随系统：系统要求减少动效就给 `off`，否则 `normal`。这样「从来没
 * 进过设置」的用户也能得到符合系统偏好的行为，而不需要在应用里再选一次。
 */
export function readMotionLevel(): MotionLevel {
  try {
    const stored = localStorage.getItem(MOTION_KEY);
    if (isLevel(stored)) return stored;
  } catch {
    // 存储被禁用时按系统偏好走。
  }
  return prefersReducedMotion() ? 'off' : 'normal';
}

/** 写入偏好。`system` 的语义通过「删掉记录」表达，而不是存一个特殊值。 */
export function saveMotionLevel(level: MotionLevel): void {
  try {
    localStorage.setItem(MOTION_KEY, level);
  } catch {
    // 仅本次会话生效。
  }
}

/**
 * 把档位写到根元素上。
 *
 * 同时写两样东西：
 *   - `--motion-scale`：各组件的时间都乘它（见 globals.css）。
 *   - `data-motion`：给需要「完全换一套呈现」的地方用。例如加载指示器在关闭动效
 *     时应当换成静态文本，而不是把动画时长压到 0——那样它看起来像卡住了。
 *
 * `off` 时还额外设一个 0.01ms 的兜底：有些库的动画不读我们的变量，那条全局
 * 规则能覆盖到它们（与 globals.css 里原有的 reduced-motion 规则同一条思路）。
 */
export function applyMotionLevel(level: MotionLevel): void {
  if (typeof document === 'undefined') return;
  const root = document.documentElement;
  root.style.setProperty('--motion-scale', String(SCALE[level]));
  root.setAttribute('data-motion', level);
  root.classList.toggle('motion-off', level === 'off');
}
