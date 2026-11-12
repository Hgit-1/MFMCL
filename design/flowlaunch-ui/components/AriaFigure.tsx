// Aria 立绘。两种用法：主页大图（hero）与侧栏小图（portrait 缩略）。
//
// ## 为什么不用 <img> 直接铺满
//
// 立绘是透明 PNG 式的素材，边缘是人物轮廓。摆放上有三件事要处理：
//
//   1. **裁切与缩放的基准**。`object-fit: contain` 会让立绘按容器高度缩放，容器
//      一变高人物就变小；`cover` 则会切掉头和脚。两种用在立绘上都不对，所以这里
//      用固定的比例盒 + `contain`，让「立绘占多高」由 `--aria-figure-height`
//      决定，不受容器形状影响。
//   2. **深浅主题下的对比**。透明立绘站在深色页面上时，浅色衣服会融进背景。
//      这里给一层极淡的径向光晕垫在人物后面（`--aria-glow`），它不是背景色，
//      只负责把人物从底色上「托」起来。
//   3. **换季后的重载闪烁**。`errored` 状态是为了让缺失素材不显示成一个破图标：
//      季节切换、素材被清掉时，这里直接不渲染而不是画一个占位方块。

import { useState } from 'react';
import type { AriaArtwork, AriaPage, Season, SeasonArt } from '../utils/aria';
import { seasonArt, SEASON_LABEL_KEYS, pageArt } from '../utils/aria';
import { useI18n } from '../utils/i18n';
import styles from './AriaFigure.module.css';

interface AriaFigureProps {
  artwork?: AriaArtwork | null;
  season: Season;
  /**
   * `hero` 是主页的竖幅全身；`sidebar` 是侧栏底部的方形小图；`watermark` 是其余
   * 页面右下角的大幅水印。
   */
  variant: 'hero' | 'sidebar' | 'watermark';
  /** 立绘在页面上的用途，只用于无障碍描述。 */
  label?: 'home' | 'sidebar' | 'watermark';
  /**
   * 该页专属的动作。给了就用 `pages/aria-<page>-<season>.webp`，没给就用通用的
   * 季节立绘。
   *
   * 回退是必需的而不是可有可无：分页动作是按需生成的，某页某季缺一张时，
   * 「显示通用的季节立绘」比「什么都不显示」好得多——后者会让那一页在换季时
   * 突然空出一块，看起来像插件坏了。
   */
  page?: AriaPage | null;
  /**
   * 素材彻底取不到时通知调用方。
   *
   * ## 为什么需要这个回调
   *
   * 素材来自已安装包，文件被移除或损坏时可能加载失败。内容区右侧为立绘
   * 留了一列宽度（`--aria-figure-width`）。如果素材加载失败、组件渲染为空，
   * 那一列就变成一块什么都没有的空白——比没有立绘更像故障。
   *
   * 调用方收到这个信号后收回那一列，界面回到「没有插件」的样子。
   */
  onUnavailable?: () => void;
}

/**
 * 单个立绘。
 *
 * `decoding="async"` 是必要的：立绘是 200–350 KB 的 WebP，同步解码会在换季的
 * 那一帧卡住主线程，表现为打开启动器时界面「停顿一下」。
 *
 * 包路径或版本变化时重挂载内部图像，清掉上一份素材的失败与回退状态。
 */
export function AriaFigure(props: AriaFigureProps) {
  const art = seasonArt(props.artwork, props.season);
  if (!art) return null;
  const identity = JSON.stringify([props.artwork?.assetBase, props.artwork?.version, props.season, props.variant, props.page]);
  return <InstalledAriaFigure key={identity} {...props} art={art} />;
}

function InstalledAriaFigure({ season, variant, label, page, artwork, onUnavailable, art }: AriaFigureProps & { art: SeasonArt }) {
  const { t } = useI18n();
  const [source, setSource] = useState(() =>
    (page ? pageArt(artwork, page, season)?.hero : undefined)
      ?? (variant === 'hero' ? art.hero : variant === 'sidebar' ? art.thumb : art.portrait),
  );
  const [failed, setFailed] = useState(false);

  if (failed) return null;

  const seasonName = t(SEASON_LABEL_KEYS[season]);
  /*
   * 水印对读屏软件完全隐藏。
   *
   * 它每页都在，且与页面内容无关；把它读出来等于在每次切页时插一句
   * 「Aria 的冬季立绘」，而用户想问的是这一页有什么。装饰性图片的正确做法是
   * `alt=""` 加 `aria-hidden`，而不是给它一个描述性的替代文本。
   */
  const decorative = label === 'sidebar' || variant === 'watermark';

  /*
   * 分页素材缺失时退回通用季节立绘，只退一次。
   *
   * 不以「HTTP 能不能取到」预判，因为运行期拿不到文件系统；`onError` 是唯一
   * 可靠的信号。退过一次之后再失败就当作真的没有素材。
   */
  const fallback = variant === 'sidebar' ? art.thumb : art.hero;
  const handleError = () => {
    if (page && source !== fallback) {
      setSource(fallback);
      return;
    }
    setFailed(true);
    onUnavailable?.();
  };

  return (
    <figure className={styles.figure} data-variant={variant} data-page={page ?? undefined}>
      <img
        className={styles.image}
        src={source}
        alt={decorative ? '' : t('aria.figureAlt', { values: { season: seasonName } })}
        aria-hidden={decorative ? 'true' : undefined}
        draggable={false}
        decoding="async"
        onError={handleError}
      />
    </figure>
  );
}
