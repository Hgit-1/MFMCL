// 实例级覆盖设置。
//
// 设计目标：管理页列出的每一项都可以「跟随全局」——这是默认值。用户只在确实
// 需要偏离全局时才改成具体值，而不是一进管理页就被迫做一堆选择。
//
// 为什么用三档而不是两档开关：
//   两档开关只有「开 / 关」，无法表达「不设置，用全局的」。如果拿开尔文当默认，
//   用户就分不清「这是全局的值」还是「我在这里设成了这个值」——之后在全局改了
//   设置，实例不会跟着变，用户却以为会。三档把「继承」显式化。
//
// 数据形状：每个字段是 `Override<T>`，undefined 表示继承。
//   布尔字段用 'off' | 'on'，不设就直接 undefined；
//   数值/文本字段用具体值，不设就 undefined。
//
// 与 Java 设置的关系：Java 有自己更复杂的模型（auto / range / folder / forced，
// 见 utils/java.ts），已经单独存储，这里不重复。

import type { GameInstance } from '../types';

export const INSTANCE_OVERRIDES_KEY = 'flowlaunch-instance-overrides';

/** 可被实例覆盖的字段。选择依据：这些值会直接影响这一次游戏运行。 */
export interface InstanceOverrides {
  /**
   * 内存分配方式。自动档不存具体数值——自动值取决于本机内存和模组数，
   * 存下来会在换机器后失真。
   */
  memoryMode?: 'auto' | 'custom';
  /** 最大内存（MB），仅在 memoryMode === 'custom' 时有意义。 */
  memoryMb?: number;
  /** 启动前进行内存优化。 */
  optimizeMemory?: boolean;
  /** 以全屏启动。 */
  fullscreen?: boolean;
  /** 显示游戏日志。 */
  showGameLog?: boolean;
  /** 窗口尺寸。 */
  windowWidth?: number;
  windowHeight?: number;
  /** 额外 JVM 参数。 */
  extraJvmArgs?: string;
  /** 游戏参数。 */
  gameArgs?: string;
  /** 启动前执行的命令。 */
  preLaunchCommand?: string;
}

export type OverrideKey = keyof InstanceOverrides;

/** 一个字段当前的来源，界面据此显示「跟随全局」提示。 */
export type OverrideState = 'global' | 'own';

export const EMPTY_OVERRIDES: InstanceOverrides = {};

/** 布尔三档。string 用 'inherit' 表示继承；这里用 undefined 表达，与数值字段统一。 */
export type BooleanOverride = boolean | undefined;

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}

function positiveNumber(value: unknown, min: number, max: number): number | undefined {
  if (typeof value !== 'number' || !Number.isFinite(value)) return undefined;
  if (value < min || value > max) return undefined;
  return value;
}

function optionalString(value: unknown, maxLength: number): string | undefined {
  if (typeof value !== 'string') return undefined;
  const trimmed = value.trim();
  const text = trimmed || value;
  return text.slice(0, maxLength);
}

/**
 * 归一化单个实例的覆盖值。
 *
 * 存档可能来自旧版本、手改的 localStorage、或被别的代码写过，所以逐字段校验
 * 而不是整体 trust。越界或类型不对的字段直接丢弃（等于继承），这比保留一个
 * 非法值更安全：非法值会让启动命令生成失败。
 */
export function normalizeOverrides(raw: unknown): InstanceOverrides {
  if (!isPlainObject(raw)) return {};
  const result: InstanceOverrides = {};

  const mode = raw.memoryMode;
  if (mode === 'auto' || mode === 'custom') result.memoryMode = mode;

  const memoryMb = positiveNumber(raw.memoryMb, 512, 1_048_576);
  if (memoryMb !== undefined) result.memoryMb = Math.round(memoryMb);

  const optimizeMemory = raw.optimizeMemory;
  if (typeof optimizeMemory === 'boolean') result.optimizeMemory = optimizeMemory;

  const fullscreen = raw.fullscreen;
  if (typeof fullscreen === 'boolean') result.fullscreen = fullscreen;

  const showGameLog = raw.showGameLog;
  if (typeof showGameLog === 'boolean') result.showGameLog = showGameLog;

  const width = positiveNumber(raw.windowWidth, 640, 7680);
  if (width !== undefined) result.windowWidth = Math.round(width);

  const height = positiveNumber(raw.windowHeight, 360, 4320);
  if (height !== undefined) result.windowHeight = Math.round(height);

  const jvm = optionalString(raw.extraJvmArgs, 8192);
  if (jvm !== undefined) result.extraJvmArgs = jvm;

  const args = optionalString(raw.gameArgs, 4096);
  if (args !== undefined) result.gameArgs = args;

  const preLaunch = optionalString(raw.preLaunchCommand, 4096);
  if (preLaunch !== undefined) result.preLaunchCommand = preLaunch;

  return result;
}

export function loadInstanceOverrides(): Record<string, InstanceOverrides> {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(INSTANCE_OVERRIDES_KEY) || '{}');
    if (!isPlainObject(parsed)) return {};
    return Object.fromEntries(
      Object.entries(parsed).map(([id, value]) => [id, normalizeOverrides(value)]),
    );
  } catch {
    return {};
  }
}

export function saveInstanceOverrides(overrides: Record<string, InstanceOverrides>): void {
  try {
    localStorage.setItem(INSTANCE_OVERRIDES_KEY, JSON.stringify(overrides));
  } catch { /* storage full or blocked */ }
}

/**
 * 清除一个字段的覆盖，让它回到「跟随全局」。
 *
 * 单独写这个是因为 `{ ...obj, key: undefined }` 会让 JSON.stringify 丢掉该键
 * （正是我们要的），但直接 delete 更明确，也避免某些序列化实现把 undefined
 * 写成 null。
 */
export function clearOverride(overrides: InstanceOverrides, key: OverrideKey): InstanceOverrides {
  const next = { ...overrides };
  delete next[key];
  return next;
}

/** 该字段当前是否偏离全局。 */
export function overrideState(overrides: InstanceOverrides, key: OverrideKey): OverrideState {
  return overrides[key] === undefined ? 'global' : 'own';
}

/** 实例的某个设置最终生效的值：有覆盖用覆盖，否则用全局。 */
export function resolveOverride<K extends OverrideKey>(
  overrides: InstanceOverrides,
  key: K,
  globalValue: NonNullable<InstanceOverrides[K]>,
): NonNullable<InstanceOverrides[K]> {
  const own = overrides[key];
  return (own === undefined ? globalValue : own) as NonNullable<InstanceOverrides[K]>;
}

/** 有多少项偏离了全局，用于在实例卡片上提示「已自定义 N 项」。 */
export function countOverrides(overrides: InstanceOverrides): number {
  // 窗口尺寸成对生效，两个字段算一项——否则界面会说「已自定义 2 项」，
  // 而用户明明只改了一个尺寸。
  let count = 0;
  let windowCounted = false;
  (Object.keys(overrides) as OverrideKey[]).forEach(key => {
    if (overrides[key] === undefined) return;
    if (key === 'windowWidth' || key === 'windowHeight') {
      if (!windowCounted) { windowCounted = true; count += 1; }
      return;
    }
    // memoryMode 与 memoryMb 是同一项设置（内存分配），只算一项；
    // 自动档下没有 memoryMb，自定义档下它只是 memoryMode 的附带数值。
    if (key === 'memoryMb') return;
    // 文本项被清空等于「没有额外内容」，效果与继承相同，不必计入自定义项数。
    if (typeof overrides[key] === 'string' && !overrides[key]!.trim()) return;
    count += 1;
  });
  return count;
}

/**
 * 给保存流程用：把实例覆盖合并成最终设置（未覆盖的取全局值）。
 *
 * 返回值是「已解析」的完整值，不再是可能为 undefined 的覆盖表——调用方拿到的
 * 就是这次启动要用的数字/布尔，不需要自己再判断继承。
 */
export function effectiveSettings(
  overrides: InstanceOverrides,
  global: Required<InstanceOverrides>,
): Required<InstanceOverrides> {
  const result = { ...global };
  (Object.keys(overrides) as OverrideKey[]).forEach(key => {
    const own = overrides[key];
    if (own !== undefined) (result as Record<string, unknown>)[key] = own;
  });
  return result;
}

/** 预留给后续接入：实例是否声明了自定义。 */
export function hasCustomization(overrides: InstanceOverrides): boolean {
  return countOverrides(overrides) > 0;
}

/** 仅用于测试与调试：实例 id 与覆盖值的简单映射类型别名。 */
export type OverrideMap = Record<string, InstanceOverrides>;

/** 取得某实例的覆盖值。 */
export function overridesFor(map: OverrideMap, instance: Pick<GameInstance, 'id'>): InstanceOverrides {
  return map[instance.id] ?? EMPTY_OVERRIDES;
}
