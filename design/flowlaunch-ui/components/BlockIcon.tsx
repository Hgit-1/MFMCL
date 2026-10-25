// 方块贴图图标。
//
// 无边框：不带卡片、圆角遮罩、彩色底板或投影，就是一块像素贴图。加载失败时
// 整个组件返回 null，让调用方可以用文字兜底，而不是露出浏览器的破图图标。
//
// alt 文本由词典键渲染，因此切换语言时读屏文本也会跟着变。

import { useState } from 'react';
import type { BlockTexture } from '../utils/store/blocks';
import { useI18n } from '../utils/i18n';
import styles from './StoreWorkspace.module.css';

interface BlockIconProps extends BlockTexture {
  /** 显示边长（px）。贴图按整数倍放大，避免像素被拉伸成非整数倍。 */
  size?: number;
  className?: string;
}

export function BlockIcon({ src, altKey, size = 32, className }: BlockIconProps) {
  const { t } = useI18n();
  const [failed, setFailed] = useState(false);
  if (failed) return null;
  return (
    <img
      className={`${styles.blockIcon} ${className ?? ''}`}
      src={src}
      alt={t(altKey)}
      width={size}
      height={size}
      draggable={false}
      style={{ width: size, height: size }}
      onError={() => setFailed(true)}
    />
  );
}
