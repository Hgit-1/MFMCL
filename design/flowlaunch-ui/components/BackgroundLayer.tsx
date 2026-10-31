// 背景图层：铺在侧栏与内容区之下的一层图片或视频。
//
// ## 为什么是一个独立图层，而不是给 body 设 background-image
//
// 背景需要三样东西叠加：媒体本身、一层遮罩（压住它，保证前景文字可读）、
// 以及可选的模糊。三者各自有不同的过滤与过渡，用一个 `background-image` 表达
// 不了遮罩浓度，也没法对视频生效。所以这里是两层绝对定位的元素：媒体与遮罩。
//
// ## 为什么不监听鼠标做视差
//
// 视差需要每次 pointermove 都改 transform，对一段正在解码播放的视频来说，
// 这个合成层会被迫每帧重新栅格化，风扇会响。背景是「静下来看的东西」，不加
// 这个成本。

import { useEffect, useRef, useState } from 'react';
import type { BackgroundSetting } from '../utils/background';
import { veilColor } from '../utils/background';
import { useMediaUrl, useMediaUrls } from '../hooks/useMedia';
import styles from './BackgroundLayer.module.css';

interface BackgroundLayerProps {
  setting: BackgroundSetting;
  /** 当前解析出的主题，决定遮罩用白还是黑。 */
  dark: boolean;
  /**
   * 轮播切到第几张（0 基）。由上层控制而不是自己计时：上层要同时把「当前这张
   * 图的强调色」交给强调色系统，两处各自计时一定会错位。
   */
  slideIndex: number;
}

export function BackgroundLayer({ setting, dark, slideIndex }: BackgroundLayerProps) {
  const active = setting.mode !== 'none';
  const showsImages = setting.mode === 'single' || setting.mode === 'slideshow';
  const imageUrls = useMediaUrls(showsImages ? setting.images.map(image => image.id) : []);
  const video = useMediaUrl(
    setting.mode === 'video' && setting.video ? 'video' : null,
    setting.mode === 'video' ? setting.video?.id ?? null : null,
  );

  /*
   * 当前显示的图片。
   *
   * `single` 用第一张，`slideshow` 按 `slideIndex` 取模。取模而不是让上层夹紧：
   * 用户删掉几张图之后索引可能超出范围，越界会让背景整块消失，取模则总能落回
   * 一张存在的图。
   */
  const currentImage = setting.mode === 'single'
    ? imageUrls[0] ?? ''
    : setting.mode === 'slideshow' && imageUrls.length > 0
      ? imageUrls[slideIndex % imageUrls.length] ?? ''
      : '';

  /*
   * 图片之间的交叉淡入。
   *
   * 两层交替：新图放进后层，等它 `onLoad` 之后再翻到前面。直接对同一个 `src`
   * 做透明度过渡是不行的——换图的瞬间旧图已经被浏览器丢掉，结果是黑一下再
   * 出现，比不淡入更难看。
   */
  const [front, setFront] = useState('');
  const [back, setBack] = useState('');
  useEffect(() => {
    if (!currentImage) {
      setFront('');
      setBack('');
      return;
    }
    setFront(previousFront => {
      if (previousFront === currentImage) return previousFront;
      // 新图放进后层。没有前层（首次显示）时直接升为前层，省掉一次空过渡。
      if (!previousFront) return currentImage;
      setBack(currentImage);
      return previousFront;
    });
  }, [currentImage]);

  /** 后层加载完成：翻到前面，旧图淡出。 */
  const promoteBack = () => {
    setFront(previousFront => (back || previousFront));
    setBack('');
  };

  /*
   * 视频的播放控制。
   *
   * `autoPlay` 在部分 WebView 里会被静音策略拦下，所以额外调一次 `play()` 并
   * 吞掉 rejection——被拦下的结果是「停在第一帧」，那仍然是一张静图，不算故障。
   */
  const videoRef = useRef<HTMLVideoElement>(null);
  const prefersReducedMotion = usePrefersReducedMotion();
  useEffect(() => {
    const node = videoRef.current;
    if (!node || !video.url) return;
    node.muted = true;
    if (prefersReducedMotion) node.pause();
    else void node.play().catch(() => undefined);
  }, [prefersReducedMotion, video.url]);

  if (!active) return null;

  return (
    <div
      className={styles.layer}
      aria-hidden="true"
      data-blur={setting.blur ? 'true' : undefined}
      data-fit={setting.fit}
    >
      {(setting.mode === 'single' || setting.mode === 'slideshow') && front && (
        <img className={styles.image} src={front} alt="" draggable={false} />
      )}
      {(setting.mode === 'single' || setting.mode === 'slideshow') && back && (
        <img className={`${styles.image} ${styles.imageBack}`} src={back} alt="" draggable={false} onLoad={promoteBack} />
      )}

      {/* 视频：静音、内联、循环。不显示控件——它不是内容，是装饰。 */}
      {setting.mode === 'video' && video.url && (
        <video
          ref={videoRef}
          className={styles.video}
          src={video.url}
          autoPlay
          loop
          muted
          playsInline
          disablePictureInPicture
        />
      )}

      {/*
        遮罩。`veilColor` 按主题给出白/黑两色，浓度由用户调；为 0 时整层不上色，
        但元素仍然存在——去掉它会改变子元素的层叠上下文，模糊层会露到前景上。
      */}
      <div className={styles.veil} style={{ background: veilColor(setting.veil, dark) }} />
    </div>
  );
}

/**
 * 读取系统的「减少动态效果」偏好。
 *
 * 单独成一个 hook 是因为设置面板里也要用它（关掉轮播的说明文字），并且
 * `matchMedia` 在测试环境里可能不存在，需要兜底。
 */
export function usePrefersReducedMotion(): boolean {
  const [reduced, setReduced] = useState(() => {
    if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return false;
    return window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  });

  useEffect(() => {
    if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return;
    const query = window.matchMedia('(prefers-reduced-motion: reduce)');
    const handler = (event: MediaQueryListEvent) => setReduced(event.matches);
    query.addEventListener('change', handler);
    return () => query.removeEventListener('change', handler);
  }, []);

  return reduced;
}
