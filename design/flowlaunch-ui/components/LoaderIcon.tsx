// 加载器标志图标。
//
// 与 `BlockIcon` 的区别在于三点：
//   1. 按素材决定是否像素化放大。像素画（Fabric 织纹、NeoForge 狐狸头、原版
//      方块）要硬边；插画与矢量（Forge 铁砧、Quilt 拼布）要平滑，套用像素化会
//      把抗锯齿边缘变成锯齿块。
//   2. 保持原始宽高比。各标志的画布比例不同，方形强制拉伸会变形，所以用
//      `object-fit: contain` 居中缩放，不裁切。
//   3. 支持按比例缩小（`scale`）与去色（`muted`）：前者让留白不同的标志视觉
//      等重，后者用于「实例安装失败」这类不可用状态。
//
// 加载失败时回退到调用方提供的兜底节点（通常是方块图标），而不是露出浏览器的
// 破图图标。

import { useState } from 'react';
import type { ReactNode } from 'react';
import type { LoaderIcon } from '../utils/store/loaders';
import { useI18n } from '../utils/i18n';
import styles from './LoaderBadge.module.css';

interface LoaderIconProps extends LoaderIcon {
  /** 图标框边长（px）。图片按 `contain` 缩放并居中，不裁切。 */
  size?: number;
  /** 加载失败时渲染的兜底内容；不传则什么都不渲染。 */
  fallback?: ReactNode;
  /**
   * 去色显示。
   *
   * 用于「这个实例现在不可用」的状态（安装失败）。彩色标志会让人以为实例一切
   * 正常，而灰化的标志和置灰按钮是同一套视觉语言，不需要额外文字就能读出「这里
   * 出问题了」。
   */
  muted?: boolean;
  className?: string;
}

export function LoaderIcon({ src, altKey, pixelated, scale = 1, plate, muted, size = 40, fallback, className }: LoaderIconProps) {
  const { t } = useI18n();
  const [failed, setFailed] = useState(false);
  if (failed) return <>{fallback ?? null}</>;
  // 外框固定，图片按 scale 收缩到内部——这样不同留白的标志视觉重量一致，
  // 而排在同一列时基线不会因为外框尺寸不同而跳动。
  const inner = Math.round(size * scale);
  return (
    <span
      className={`${styles.frame} ${plate ? styles.plate : ''} ${muted ? styles.mutedFrame : ''} ${className ?? ''}`}
      style={{ width: size, height: size }}
    >
      <img
        className={`${styles.icon} ${pixelated ? styles.pixelated : ''} ${muted ? styles.muted : ''}`}
        src={src}
        alt={t(altKey)}
        width={inner}
        height={inner}
        draggable={false}
        style={{ width: inner, height: inner }}
        onError={() => setFailed(true)}
      />
    </span>
  );
}
