// 加载器图标素材。
//
// 与 `blocks.ts` 的分工：那边是 Minecraft 原版方块贴图（16×16 像素画，必须
// `image-rendering: pixelated` 放大）；这里是各加载器的**官方标志**，多数是
// 平滑插画或矢量，放大时不能套用像素化渲染，否则边缘会变成锯齿块。
//
// 为什么用官方标志而不是「用方块做联想」：加载器是具体项目，用户认的就是它
// 自己的标志。织布机、铁砧那套方块隐喻能自圆其说，但对不熟悉隐喻的人不成立，
// 而 Fabric 的织纹钻石、NeoForge 的狐狸头是社区里通用的识别符号。
//
// 授权与来源见 `public/textures/loaders/SOURCES.md`：这些是各项目的商标/标志，
// 仅用于指示「这个实例装的是哪个加载器」，不表示背书关系，也不随成品再分发
// 承诺任何许可。

import type { MessageKey } from '../i18n/messages/zh-CN';

/** 界面认得的五个加载器；与 `types.ts` 的 `GameInstance['loader']` 保持一致。 */
export type GameInstanceLoader = 'vanilla' | 'fabric' | 'quilt' | 'forge' | 'neoforge';

const LOADER_BASE = '/textures/loaders';
const BLOCK_BASE = '/textures/blocks';

export interface LoaderIcon {
  src: string;
  /** 无障碍文本的词典键：说明这是哪个加载器的标志。 */
  altKey: MessageKey;
  /**
   * 是否需要像素化放大。
   *
   * 只有小尺寸像素画需要 `pixelated` 保持硬边；插画与矢量用默认的平滑缩放。
   */
  pixelated?: boolean;
  /**
   * 相对显示尺寸。
   *
   * 各标志的留白比例不同：Fabric 的织纹几乎填满画布，Forge 的铁砧和 NeoForge
   * 的狐狸头自带边距。若都按同一个边长渲染，填满的那个看起来会大一圈。这里按
   * 实测的内容占比补偿，让六个图标排在一起时视觉重量一致：
   *
   *   scale = 最小的内容占比 / 本图的内容占比
   *
   * 只有明显更满的 Fabric 需要缩放；其余差异在 7% 以内，肉眼不可辨。
   */
  scale?: number;

  /**
   * 深色主题下是否需要浅色底板。
   *
   * Forge 的官方标志是深藏青（#1E2D41）的铁砧与锤子，直接放在深色卡片的
   * `bg-secondary`（#111827）上对比度不到 1.5:1，几乎看不见。给它一块浅色底板
   * 才能在深色主题里保住轮廓。
   *
   * 其余三个标志本身是高饱和或浅色（Fabric 米色、Quilt 四色、NeoForge 橙色），
   * 在两种主题下都够清楚，不需要底板——统一加底板反而会让它们像贴纸。
   */
  plate?: boolean;
}

/**
 * 加载器 → 官方标志。
 *
 * 原版没有「加载器标志」这个概念——它就是游戏本身，所以沿用内容商店里表示
 * 原版游戏的草方块，保持两处视觉一致。
 *
 * `pixelated` 按素材本身决定，不是按「是不是方块」：
 *   - 原版草方块、Fabric 织纹、NeoForge 狐狸头都是小尺寸像素画 → 硬边放大
 *   - Forge 铁砧是 128×128 平滑插画、Quilt 是矢量 → 默认平滑缩放
 */
export const LOADER_ICONS: Record<GameInstanceLoader, LoaderIcon> = {
  vanilla: { src: `${BLOCK_BASE}/grass_block_side.png`, altKey: 'loader.vanilla', pixelated: true },
  // Fabric 的内容填满整个画布，缩到 0.90 才和其余图标等重。
  fabric: { src: `${LOADER_BASE}/fabric.png`, altKey: 'loader.fabric', pixelated: true, scale: 0.9 },
  quilt: { src: `${LOADER_BASE}/quilt.svg`, altKey: 'loader.quilt' },
  // Forge 的深色铁砧在深色主题里需要浅色底板才看得见。
  forge: { src: `${LOADER_BASE}/forge.png`, altKey: 'loader.forge', plate: true },
  neoforge: { src: `${LOADER_BASE}/neoforge.png`, altKey: 'loader.neoforge', pixelated: true },
};

/**
 * 未识别加载器的兜底：石头方块。
 *
 * 后端 `loader` 是自由字符串，理论上可能有界面还不认识的取值。给一个中性图标
 * 比借用某个已知加载器的标志更诚实——后者会让人以为装的是那个加载器。
 */
export const UNKNOWN_LOADER_ICON: LoaderIcon = {
  src: `${BLOCK_BASE}/stone.png`,
  altKey: 'loader.unknown',
  pixelated: true,
};

/** 实例的加载器 → 图标。未知加载器给石头，而不是冒充已知标志。 */
export function iconForLoader(loader: GameInstanceLoader | string | undefined): LoaderIcon {
  if (loader && loader in LOADER_ICONS) return LOADER_ICONS[loader as GameInstanceLoader];
  return UNKNOWN_LOADER_ICON;
}
