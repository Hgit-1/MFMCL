// 商店列表的图标：真实项目 logo，带占位与失败回退。
//
// 项目 logo 来自各自 CDN（cdn.modrinth.com / media.forgecdn.net），无边框、
// 不加底板。加载中显示分类图标占位，失败时退回同一个占位，所以列表里永远
// 不会出现破图图标或空白方块。

import { useEffect, useState } from 'react';
import { Box, Image as ImageIcon, Map, Package, Puzzle } from 'lucide-react';
import type { StoreCategory } from '../utils/store/types';
import styles from './StoreWorkspace.module.css';

const CATEGORY_ICONS: Record<StoreCategory, React.ReactNode> = {
  vanilla: <Box size={20} strokeWidth={1.8} />,
  mod: <Puzzle size={20} strokeWidth={1.8} />,
  modpack: <Package size={20} strokeWidth={1.8} />,
  resourcepack: <ImageIcon size={20} strokeWidth={1.8} />,
  world: <Map size={20} strokeWidth={1.8} />,
};

interface StoreLogoProps {
  src?: string;
  alt: string;
  category: StoreCategory;
}

export function StoreLogo({ src, alt, category }: StoreLogoProps) {
  const [state, setState] = useState<'loading' | 'ok' | 'failed'>(src ? 'loading' : 'failed');

  // 换项目时（key 变化复用 DOM）要重置状态，否则上一个项目的失败会传染。
  useEffect(() => { setState(src ? 'loading' : 'failed'); }, [src]);

  const placeholder = <span className={styles.logoPlaceholder} aria-hidden="true">{CATEGORY_ICONS[category]}</span>;

  if (!src || state === 'failed') return placeholder;

  return (
    <span className={styles.logoWrap}>
      {state === 'loading' && placeholder}
      <img
        className={`${styles.logo} ${state === 'ok' ? styles.logoReady : ''}`}
        src={src}
        alt={alt}
        loading="lazy"
        draggable={false}
        onLoad={() => setState('ok')}
        onError={() => setState('failed')}
      />
    </span>
  );
}
