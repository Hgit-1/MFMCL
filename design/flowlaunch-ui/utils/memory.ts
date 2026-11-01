// 内存分配模型（移植自 PCL2）。
//
// 参考：
//   PCL/Plain Craft Launcher 2/Pages/PageSetup/PageSetupLaunch.xaml.vb
//     - RefreshRam：按 TotalPhysicalMemory 设定滑条 MaxValue
//     - GetRam：自动配置的分段预分配 / 自定义的滑条位置换算
//   PCL/Plain Craft Launcher 2/Pages/PageInstance/PageInstanceSetup.xaml.vb
//     - GetRam：同一套公式（PCL 源码里两处互相提醒要同步修改）
//
// 三个要点：
//
// 1. 滑条不是线性的 MB 刻度，而是把 0..MaxValue 的整数位置映射到 GB：
//    小内存段每格 0.1 GB，大内存段每格 2 GB。这样 4 GB 的机器也能精确拖到
//    2.6 GB，64 GB 的机器又不会出现几百个无意义的小格。
// 2. 滑条的最大位置恰好落在「设备总内存」上（见 maxSliderIndexForTotal 的
//    四段公式），所以拖到底就是把整台机器的内存都给游戏——与 PCL 一致。
// 3. 自动配置按「档位 + 可用内存百分比」分配，并且把实例的模组数量算进去
//    （PCL: RamMininum / RamTarget1..3）。模组越多，目标值越高。

import { loc, type Localized } from './i18n/localized';
import { getSystemMemory as nativeGetSystemMemory, isDesktopShell } from './native';

/** 滑条位置与 GB 的换算。index 是 PCL 的 SliderRamCustom.Value。 */

export function sliderIndexToGb(index: number): number {
  if (index <= 12) return index * 0.1 + 0.3;
  if (index <= 25) return (index - 12) * 0.5 + 1.5;
  if (index <= 33) return (index - 25) * 1 + 8;
  return (index - 33) * 2 + 16;
}

/** 反向换算：把一个 GB 值放回最接近的滑条位置。 */
export function gbToSliderIndex(gb: number): number {
  if (gb <= 1.5) return Math.round((gb - 0.3) / 0.1);
  if (gb <= 8) return 12 + Math.round((gb - 1.5) / 0.5);
  if (gb <= 16) return 25 + Math.round(gb - 8);
  return 33 + Math.round((gb - 16) / 2);
}

/**
 * 滑条上限位置：取 PCL 四段公式的结果；如果该位置换算回的 GB 仍低于设备
 * 总内存（例如 7.9 GB 这类不是刻度整数倍的容量），就再往上放一格，保证拖到
 * 最右端一定能够到整机总内存。
 */
export function maxSliderIndexForTotal(totalGb: number): number {
  const base = totalGb <= 1.5
    ? Math.max(Math.floor((totalGb - 0.3) / 0.1), 1)
    : totalGb <= 8
      ? Math.floor((totalGb - 1.5) / 0.5) + 12
      : totalGb <= 16
        ? Math.floor(totalGb - 8) + 25
        : Math.floor((totalGb - 16) / 2) + 33;
  return sliderIndexToGb(base) >= totalGb - 1e-6 ? base : base + 1;
}

/** PCL 的最小值 0.3 GB 低于桌面版后端的 512 MB 下限，这里取 0.5 GB。 */
export const MIN_MEMORY_MB = 512;

/** 512 MB 对应的滑条位置（index 2 → 0.5 GB）。 */
export const MIN_SLIDER_INDEX = 2;

export interface AutoRamInput {
  /** 设备总内存（MB）。 */
  totalMb: number;
  /** 实例里已安装的模组数量；整合包实例通常有几十到几百个。 */
  modCount?: number;
  /** 是否 OptiFine 版本；PCL 对它有单独的目标值。 */
  hasOptiFine?: boolean;
  /**
   * 当前已被系统与其它进程占用的内存（MB）。只有桌面版拿得到。
   *
   * 传了就按「真实剩余」计算可用量，而不是按固定预留比例估算——用户此刻开着
   * 一堆程序时，自动值应当相应变小，否则分配的堆内存会把机器压到换页。
   */
  usedMb?: number;
}

/**
 * 自动配置时分配给游戏的内存（MB）。
 *
 * 可用内存有两条路径：
 *   1. 桌面版给到 usedMb → 用它算真实剩余（总内存 − 已用），但仍保留一个
 *      最低预留，避免系统在游戏加载高峰时不够用；
 *   2. 拿不到（浏览器预览）→ 按「为系统预留 1.5 GB 或 20%（取较大者）」估算。
 *
 * 两种路径之后都走 PCL 的四段预分配。预留比例与后端的
 * recommended_memory_for_total 保持一致，避免同一个界面出现两套口径。
 */
export function autoAllocateMb({ totalMb, modCount = 0, hasOptiFine = false, usedMb }: AutoRamInput): number {
  const totalGb = totalMb / 1024;
  const reserveGb = Math.max(1.5, totalGb * 0.2);
  // 有真实已用量时：剩余 = 总量 − 已用。但仍至少留 1 GB 给系统弹性，
  // 否则「已用 95%」的机器会被算出 0.3 GB 这种不可用的值。
  const measuredFreeGb = usedMb !== undefined
    ? Math.max(1, totalGb - Math.min(usedMb / 1024, totalGb))
    : undefined;
  let available = measuredFreeGb !== undefined
    ? Math.max(0.3, Math.min(measuredFreeGb, totalGb - reserveGb))
    : Math.max(0.3, totalGb - reserveGb);

  // PCL: Modable → 按模组数量抬高档位；OptiFine → 固定档位；其余为原版档位。
  let minimum: number;
  let target1: number;
  let target2: number;
  let target3: number;
  if (modCount > 0) {
    minimum = 0.5 + modCount / 150;
    target1 = 1.5 + modCount / 90;
    target2 = 2.7 + modCount / 50;
    target3 = 4.5 + modCount / 25;
  } else if (hasOptiFine) {
    minimum = 0.5; target1 = 1.5; target2 = 3; target3 = 5;
  } else {
    minimum = 0.5; target1 = 1.5; target2 = 2.5; target3 = 4;
  }

  let give = 0;
  // 阶段一：0 ~ T1，100%
  give += Math.min(available, target1);
  available -= target1;
  if (available >= 0.1) {
    // 阶段二：T1 ~ T2，70%
    const delta = target2 - target1;
    give += Math.min(available * 0.7, delta);
    available -= delta / 0.7;
  }
  if (available >= 0.1) {
    // 阶段三：T2 ~ T3，40%
    const delta = target3 - target2;
    give += Math.min(available * 0.4, delta);
    available -= delta / 0.4;
  }
  if (available >= 0.1) {
    // 阶段四：T3 ~ 2·T3，15%
    give += Math.min(available * 0.15, target3);
  }

  const result = Math.round(Math.max(give, minimum) * 10) / 10;
  // 上限不能只看总内存：有真实已用量时，还要给系统留出它已经占着的那部分，
  // 否则「已用 28 GB / 共 32 GB」时仍会算出 31.5 GB 这种会立刻换页的值。
  const headroomCeiling = usedMb !== undefined
    ? totalGb - usedMb / 1024
    : totalGb - MIN_MEMORY_MB / 1024;
  const ceiling = Math.max(MIN_MEMORY_MB / 1024, Math.min(totalMb / 1024 - MIN_MEMORY_MB / 1024, headroomCeiling));
  return Math.round(Math.min(Math.max(result, MIN_MEMORY_MB / 1024), ceiling) * 1024);
}

// ---------------------------------------------------------------------------
// 设备内存探测
// ---------------------------------------------------------------------------

export const DEVICE_MEMORY_OVERRIDE_KEY = 'flowlaunch-device-memory-mb';

export interface DeviceMemory {
  totalMb: number;
  /**
   * 当前已被系统与其它进程占用的内存（MB）。
   *
   * 只有桌面版能拿到真实值：浏览器没有任何 API 提供整机已用内存。
   * 拿不到时为 undefined，界面据此不画「已使用」段，而不是画一个假的 0。
   */
  usedMb?: number;
  /** 是否来自可信来源（桌面版探测或用户手动指定）。 */
  exact: boolean;
  source: 'desktop' | 'manual' | 'browser' | 'fallback';
  /** 说明文案：存成键 + 参数，切换语言后仍然正确。 */
  note: Localized;
}

export function saveDeviceMemoryOverride(totalMb: number): void {
  try { localStorage.setItem(DEVICE_MEMORY_OVERRIDE_KEY, String(Math.round(totalMb))); } catch { /* storage blocked */ }
}

function readDeviceMemoryOverride(): number | null {
  try {
    const raw = Number(localStorage.getItem(DEVICE_MEMORY_OVERRIDE_KEY));
    return Number.isFinite(raw) && raw >= 1024 ? raw : null;
  } catch {
    return null;
  }
}

/**
 * 最近一次探测到的已用内存（MB）。
 *
 * 保存设置时走的是同步路径 `deviceMemoryMbSync`，但自动分配现在要用到真实已用
 * 量。这里缓存 `detectDeviceMemory` 的结果，让两条路径算出的自动值一致——否则
 * 界面显示 2 GB、实际存下去却是 3 GB。
 */
let cachedUsedMb: number | undefined;

/**
 * 最近一次探测到的**真实**总内存（MB）。
 *
 * 这个缓存解决的是一个会直接误导用户的问题：`navigator.deviceMemory` 在 Chromium
 * 里按规范被截断到 8 GB 上限（隐私考量），所以在 64 GB 的机器上同步路径会报
 * 8 GB。桌面版其实拿得到准确值（后端 `get_system_memory`），但那条路是异步的，
 * 而保存设置的载荷必须同步组装。
 *
 * 于是：探测到准确值就记下来，之后所有同步调用都读它。启动时预热一次
 * （见 `primeDeviceMemory`），同步路径就不必回落到被截断的浏览器值。
 */
let cachedTotalMb: number | undefined;

/**
 * 在应用启动时预探测一次真实内存，供之后的同步路径使用。
 *
 * 为什么必须预热：新手引导、实例设置页、以及每次保存设置的自动配置，走的都是
 * 同步的 `deviceMemoryMbSync`。不预热的话，一台 64 GB 的机器在引导里会被写成
 * 「共 8 GB」，自动分配也跟着按 8 GB 算——用户看到的和自己机器的实际情况矛盾。
 *
 * 失败时静默返回：拿不到真实值时退回浏览器估算，行为与之前一致，不阻塞启动。
 */
export async function primeDeviceMemory(): Promise<void> {
  try {
    await detectDeviceMemory();
  } catch {
    // 探测是增强信息，不是启动的必要条件。
  }
}

/** 同步取「上次探测到的已用量」。没探测过或平台不支持时返回 undefined。 */
export function lastKnownUsedMb(): number | undefined {
  return cachedUsedMb;
}

/**
 * 同步估算设备总内存（MB），给保存流程用。
 *
 * 优先级：手动指定 > 上次探测到的真实值 > navigator.deviceMemory > 8 GB 兜底。
 *
 * 中间那一层是关键：`navigator.deviceMemory` 被截断到 8 GB，只该作为最后手段。
 * 桌面版后端给的是真实容量，缓存下来之后同步路径就能给出正确数字。
 */
export function deviceMemoryMbSync(): number {
  const override = readDeviceMemoryOverride();
  if (override) return override;
  if (cachedTotalMb !== undefined) return cachedTotalMb;
  const reported = (navigator as Navigator & { deviceMemory?: number }).deviceMemory;
  if (typeof reported === 'number' && Number.isFinite(reported) && reported > 0) {
    return Math.round(reported * 1024);
  }
  return 8192;
}

/**
 * 读取设备总内存。
 *
 * 优先级：用户手动指定 > 桌面版 get_system_memory > navigator.deviceMemory
 * > 8 GB 兜底。注意 navigator.deviceMemory 在 Chromium 里按规范被上限截断到
 * 8 GB，因此它只能当“至少这么大”的线索，界面要标成估算并允许修正。
 */
export async function detectDeviceMemory(): Promise<DeviceMemory> {
  const override = readDeviceMemoryOverride();
  if (override) {
    return { totalMb: override, exact: true, source: 'manual', note: loc('memory.detectedManual', { size: `${(override / 1024).toFixed(1)} GB` }) };
  }

  /*
   * 桌面版：读后端 `get_system_memory`。
   *
   * 走 `native.ts` 的封装而不是自己读 `window.__TAURI__`：项目里曾经有三份各自
   * 判断「是否桌面版」的桥接（native / javaBridge / 这里），任意两份得出不同
   * 结论时，界面就会自相矛盾。`getSystemMemory` 在预览模式下返回带估算标记的
   * 占位值，所以这里不需要再判断平台。
   */
  try {
    const info = await nativeGetSystemMemory();
    /*
     * 只在桌面版采信这个值。
     *
     * 浏览器预览下 `getSystemMemory` 返回的是 8 GB 占位，不是探测结果；如果
     * 也当成真实值缓存起来，64 GB 的机器在预览里会一直显示「共 8 GB」，而且
     * 缓存之后连 `navigator.deviceMemory` 都读不到了——看起来像探测坏了。
     */
    if (isDesktopShell() && info && Number.isFinite(info.totalMb) && info.totalMb > 0) {
      // usedMb 可能是 null（后端 JSON 里 Option::None）。这里归一化成
      // undefined，避免下游把 null 当数字用。
      const usedMb = typeof info.usedMb === 'number' && Number.isFinite(info.usedMb) && info.usedMb >= 0
        ? Math.min(info.usedMb, info.totalMb)
        : undefined;
      cachedUsedMb = usedMb;
      /*
       * 缓存真实总内存，供同步路径使用。
       *
       * 桌面版的值来自后端，是这台机器的实际容量。缓存它之后，新手引导、实例
       * 设置页和保存载荷都能拿到正确数字，而不是浏览器那个被截断到 8 GB 的值。
       */
      cachedTotalMb = info.totalMb;
      return {
        totalMb: info.totalMb,
        usedMb,
        exact: true,
        source: 'desktop',
        note: loc('memory.detectedDesktop', { size: `${(info.totalMb / 1024).toFixed(1)} GB` }),
      };
    }
  } catch { /* fall through to the browser estimate */ }

  const reported = (navigator as Navigator & { deviceMemory?: number }).deviceMemory;
  if (typeof reported === 'number' && Number.isFinite(reported) && reported > 0) {
    /*
     * 浏览器估算。注意这个值被规范封顶在 8 GB，只能当「至少这么大」的线索，
     * 所以标成 `exact: false` 并允许用户在内存设置里手动改。
     */
    cachedTotalMb = Math.round(reported * 1024);
    return {
      totalMb: cachedTotalMb,
      exact: false,
      source: 'browser',
      note: loc('memory.detectedBrowser', { size: `${reported} GB` }),
    };
  }

  return { totalMb: 8192, exact: false, source: 'fallback', note: loc('memory.detectedFallback') };
}
