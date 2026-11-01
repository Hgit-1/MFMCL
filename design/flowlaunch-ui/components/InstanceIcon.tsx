// 实例图标：优先用用户选的自定义图片，否则用加载器标志。
//
// ## 为什么统一成一个组件
//
// 同一个实例的图标出现在三处：游戏库卡片（48px）、主页大按钮（72px）、实例
// 管理页的预览（56px）。如果每处各自判断「有没有自定义图标」，那么新增一处入口
// 就要再抄一遍回落逻辑，而抄漏一处的症状是「某个界面显示的还是旧图标」——很难
// 被发现。
//
// ## 自定义图标与加载器标志的区别
//
//   - 加载器标志是**素材**：各图标留白不同，要按比例缩小才视觉等重，像素画要
//     硬边放大，深色标志要加底板。
//   - 自定义图标是**用户的一整张图**：应当铺满图标框、裁掉多余部分。用户选的是
//     头像式的图片，而不是一个带留白的标志，套用标志那套规则会得到一圈奇怪的
//     边框和缩小的图。
//
// 所以两条分支的渲染方式不同，而不是简单地换 `src`。
//
// ## 安装失败时
//
// 自定义图标同样去色。视觉语言要和加载器标志那一支保持一致：卡片上任何一处
// 彩色图标都会传递「这个实例正常」的信号。

import { useEffect, useState } from 'react';
import { BlockIcon } from './BlockIcon';
import { LoaderIcon } from './LoaderIcon';
import { VANILLA_BLOCK } from '../utils/store/blocks';
import type { LoaderIcon as LoaderIconSpec } from '../utils/store/loaders';
import { useInstanceIconUrl } from '../hooks/useInstanceIcon';
import type { InstanceIconRecord } from '../utils/instanceIcons';
import styles from './InstanceIcon.module.css';

interface InstanceIconProps {
  instanceId: string;
  /** 加载器对应的标志配置。自定义图标不存在时用它。 */
  loaderIcon: LoaderIconSpec;
  /** 用户为该实例选的自定义图标元数据；没有则为 undefined。 */
  customIcon?: InstanceIconRecord;
  /** 图标框边长（px）。内部图片按容器等比缩放。 */
  size?: number;
  /** 去色。用于「实例当前不可用」（安装失败）。 */
  muted?: boolean;
  /** 自定义图片的替代文本。默认空串：图标是装饰性的，实例名已经在旁边。 */
  alt?: string;
  /**
   * 元数据还在、图片却读不出来（存储被清空）时回调。
   *
   * 上层据此清掉这条失效的元数据。不回调的话，设置页会一直照着元数据说「自定义
   * 图片 · 某文件名」，旁边显示的却是加载器标志。
   */
  onMissing?: () => void;
}

export function InstanceIcon({
  instanceId,
  loaderIcon,
  customIcon,
  size = 48,
  muted,
  alt = '',
  onMissing,
}: InstanceIconProps) {
  const { url, loading, missing } = useInstanceIconUrl(customIcon ? instanceId : undefined, customIcon);
  const [broken, setBroken] = useState(false);

  /*
   * 元数据在、图却读不出来 → 静默回落到加载器标志，并通知上层清理那条失效记录。
   *
   * 用 effect 而不是在渲染期间调用回调：渲染必须是纯的，一次重渲染就会再触发一次
   * 副作用。依赖里只放 `missing`，它只会从 false 变一次 true。
   */
  useEffect(() => {
    if (missing) onMissing?.();
  }, [missing, onMissing]);

  /*
   * 有元数据、地址还没读到、也没坏 → 先占位。
   *
   * 不能在这里回落到加载器标志：那会在每次打开游戏库时都先闪一下方块再变成
   * 自定义图。用一个同样尺寸的空白占位，视觉上是「图还没出来」，而不是
   * 「图标变了两次」。
   */
  if (customIcon) {
    if (loading && !url) {
      return <span className={styles.placeholder} style={{ width: size, height: size }} aria-hidden="true" />;
    }
    if (url && !broken) {
      return (
        <img
          className={`${styles.custom} ${muted ? styles.muted : ''}`}
          src={url}
          alt={alt}
          width={size}
          height={size}
          draggable={false}
          style={{ width: size, height: size }}
          onError={() => setBroken(true)}
        />
      );
    }
    // 元数据在但图读不出来（存储被清空、文件损坏）：回落到加载器标志，
    // 而不是显示一个空白框。同时由上面那个 effect 通知上层清掉失效元数据。
  }

  return (
    <LoaderIcon
      {...loaderIcon}
      size={size}
      muted={muted}
      fallback={<BlockIcon {...VANILLA_BLOCK} size={size} />}
    />
  );
}
