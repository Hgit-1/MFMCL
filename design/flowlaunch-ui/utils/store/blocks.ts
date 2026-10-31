// 内容商店的图标素材（Minecraft 原版方块贴图）。
//
// 为什么用原版方块而不是通用图标：PCL2 的版本列表就是用游戏内方块图标区分
// 版本类型（正式版草方块、快照命令方块、远古圆石、彩蛋金块），启动器玩家一眼
// 就能认出来。
//
// 贴图来源与授权见 public/textures/blocks/SOURCES.md。所有 PNG 都是 16×16
// 的原始贴图（命令方块已从 16×64 的动画条带裁出首帧），渲染时必须用
// image-rendering: pixelated 放大，且**不加边框、圆角、底板或阴影**。
//
// 本文件只负责内容商店的版本类型图标。游戏库的实例图标按「加载器」区分，用的是
// 各加载器的官方标志，见 `loaders.ts`——两者素材不同，不共用映射。

import type { VersionKind } from './types';
import type { MessageKey } from '../i18n/messages/zh-CN';

const BASE = '/textures/blocks';

export interface BlockTexture {
  src: string;
  /** 无障碍文本的词典键：说明这是哪一类版本，而不是「一张图片」。 */
  altKey: MessageKey;
}

/** 版本种类 → 方块贴图。彩蛋版用金块，与正式版的草方块、快照版的命令方块区分开。 */
export const VERSION_BLOCKS: Record<VersionKind, BlockTexture> = {
  release: { src: `${BASE}/grass_block_side.png`, altKey: 'block.grassAlt' },
  snapshot: { src: `${BASE}/command_block.png`, altKey: 'block.commandAlt' },
  old: { src: `${BASE}/cobblestone.png`, altKey: 'block.cobbleAlt' },
  special: { src: `${BASE}/gold_block.png`, altKey: 'block.goldSpecialAlt' },
};

/** 「原版游戏」分类的图标：草方块。 */
export const VANILLA_BLOCK: BlockTexture = { src: `${BASE}/grass_block_side.png`, altKey: 'block.grassVanillaAlt' };

/** 某个分类对应的方块贴图（只有原版有）。 */
export function blockForVersion(kind: VersionKind | undefined): BlockTexture | null {
  return kind ? VERSION_BLOCKS[kind] : null;
}
