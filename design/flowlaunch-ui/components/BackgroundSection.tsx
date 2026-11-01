// 设置面板里的「背景」区块。
//
// ## 为什么放在「外观」页而不是单独一页
//
// 背景和主题、强调色是同一类东西：它们一起决定界面长什么样，而用户调整它们时
// 往往是连着调的（选定一张图 → 觉得颜色不搭 → 换成跟随背景取色）。分成两页会
// 让这个来回变成两次页面跳转。
//
// ## 为什么用文件输入而不是后端文件对话框
//
// 桌面版能通过 `<input type="file">` 拿到与浏览器一致的 `File` 对象，而走后端
// 需要：读绝对路径 → 过 asset 协议 → 再喂给 `<img>`，多两个失败点，对一件纯
// 显示的事没有收益。`accept` 与 MIME 校验已经能筛掉大部分误选。
//
// ## 关于「跟随背景配色」
//
// 强调色取自图片，而不是「把整个界面调成照片的颜色」。只有一组 CSS 变量由它
// 驱动（`utils/accent.ts`），文字与背景色的对比度仍由原有规则保证——否则一张
// 深色照片会把界面变成一片深色，用户再也读不清按钮上的字。

import { useRef, useState } from 'react';
import { Image as ImageIcon, Layers, Loader2, MonitorPlay, RotateCcw, Trash2, Upload, Video } from 'lucide-react';
import { useI18n } from '../utils/i18n';
import type { MessageKey } from '../utils/i18n/messages/zh-CN';
import {
  DEFAULT_BACKGROUND,
  IMAGE_ACCEPT,
  MAX_IMAGE_BYTES,
  MAX_SLIDESHOW_IMAGES,
  MAX_VIDEO_BYTES,
  VIDEO_ACCEPT,
  importImages,
  importVideo,
  removeImage,
  removeVideo,
  type BackgroundMode,
  type BackgroundSetting,
} from '../utils/background';
import { formatBytes } from '../utils/native';
import { useMediaUrl, useMediaUrls } from '../hooks/useMedia';
import styles from './BackgroundSection.module.css';

interface BackgroundSectionProps {
  setting: BackgroundSetting;
  onChange: (setting: BackgroundSetting) => void;
  /** 提示条：选择的图片被拒、视频无法解码等。 */
  onNotice: (message: string) => void;
  /** 当前主题，决定预览区的底色。 */
  dark: boolean;
}

/** 模式按钮。图标与文案分开，文案走词典。 */
const MODES: { id: BackgroundMode; labelKey: MessageKey; hintKey: MessageKey; icon: React.ReactNode }[] = [
  { id: 'none', labelKey: 'background.modeNone', hintKey: 'background.modeNoneHint', icon: <MonitorPlay size={15} /> },
  { id: 'single', labelKey: 'background.modeSingle', hintKey: 'background.modeSingleHint', icon: <ImageIcon size={15} /> },
  { id: 'slideshow', labelKey: 'background.modeSlideshow', hintKey: 'background.modeSlideshowHint', icon: <Layers size={15} /> },
  { id: 'video', labelKey: 'background.modeVideo', hintKey: 'background.modeVideoHint', icon: <Video size={15} /> },
];

/** 拒绝原因 → 文案键。 */
const REJECT_KEYS: Record<string, MessageKey> = {
  type: 'background.rejectedType',
  size: 'background.rejectedSize',
  decode: 'background.rejectedDecode',
  count: 'background.rejectedCount',
};

export function BackgroundSection({ setting, onChange, onNotice, dark }: BackgroundSectionProps) {
  const { t } = useI18n();
  const [busy, setBusy] = useState(false);
  const imageInputRef = useRef<HTMLInputElement>(null);
  const videoInputRef = useRef<HTMLInputElement>(null);

  /*
   * 缩略图。
   *
   * 和背景层共用同一套地址管理，但读的是同一批 Blob，会各自创建一份对象地址——
   * 这是有意的：两处的生命周期不同（背景层随轮播换地址，缩略图列表固定），
   * 共用一份地址会让任意一方卸载时把另一方的图弄坏。代价是同一张图在内存里
   * 有两份 Blob 引用，对几十张缩略图的规模可以接受。
   */
  const thumbUrls = useMediaUrls(setting.images.map(image => image.id));
  const videoPreview = useMediaUrl(setting.mode === 'video' ? 'video' : null, setting.video?.id ?? null);

  const patch = (next: Partial<BackgroundSetting>) => onChange({ ...setting, ...next });

  /** 选图片：追加到已有的列表，超出的部分如实报告被拒。 */
  const pickImages = async (files: FileList | null) => {
    if (!files || files.length === 0) return;
    const list = Array.from(files);
    setBusy(true);
    try {
      const remaining = Math.max(0, MAX_SLIDESHOW_IMAGES - setting.images.length);
      const result = await importImages(list, remaining);
      if (result.accepted.length > 0) {
        const images = [...setting.images, ...result.accepted];
        /*
         * 首次加图时自动切到「单张」。
         *
         * 用户点「添加图片」的意图就是「用这张当背景」，让他选完还要再点一次
         * 模式按钮是多余的一步；而如果他已经选了轮播，就保持轮播不动——追加
         * 第二张图时自动退化成单张是错的。
         */
        const mode: BackgroundMode = setting.mode === 'slideshow' || setting.mode === 'video'
          ? setting.mode
          : images.length > 1 ? 'slideshow' : 'single';
        patch({ images, mode });
      }
      reportRejected(result.rejected);
    } finally {
      setBusy(false);
      if (imageInputRef.current) imageInputRef.current.value = '';
    }
  };

  const pickVideo = async (file: File | undefined) => {
    if (!file) return;
    setBusy(true);
    try {
      const result = await importVideo(file);
      if (!result.video) {
        onNotice(reasonText(result.reason ?? 'decode'));
        return;
      }
      // 换视频时把旧的删掉，否则每换一次就留下一份几十 MB 的垃圾。
      if (setting.video && setting.video.id !== result.video.id) void removeVideo(setting.video.id);
      patch({ video: result.video, mode: 'video' });
    } finally {
      setBusy(false);
      if (videoInputRef.current) videoInputRef.current.value = '';
    }
  };

  const reportRejected = (rejected: { name: string; reason: string }[]) => {
    if (rejected.length === 0) return;
    const first = rejected[0];
    // 只报第一条：一次拖入 20 个文件时，逐条提示会刷满屏幕，而原因通常相同。
    const suffix = rejected.length > 1 ? `（另有 ${rejected.length - 1} 个）` : '';
    onNotice(`${reasonText(first.reason)}：${first.name}${suffix}`);
  };

  const reasonText = (reason: string) =>
    t(REJECT_KEYS[reason] ?? 'background.rejectedDecode', {
      values: {
        size: formatBytes(MAX_IMAGE_BYTES),
        videoSize: formatBytes(MAX_VIDEO_BYTES),
        count: MAX_SLIDESHOW_IMAGES,
      },
    });

  /** 删除一张图。删空之后回到「无背景」，而不是留一个空轮播。 */
  const deleteImage = async (id: string) => {
    const images = setting.images.filter(image => image.id !== id);
    patch({
      images,
      mode: images.length === 0 && (setting.mode === 'single' || setting.mode === 'slideshow')
        ? 'none'
        : setting.mode,
    });
    await removeImage(id);
  };

  /**
   * 调整图片顺序。
   *
   * 轮播的播放顺序就是列表顺序，所以调序等于调播放顺序。上移一位即可——拖拽
   * 排序在 12 张小卡片的规模上收益有限，却要多写一套指针事件。
   */
  const moveImage = (index: number, delta: number) => {
    const target = index + delta;
    if (target < 0 || target >= setting.images.length) return;
    const images = [...setting.images];
    const [item] = images.splice(index, 1);
    images.splice(target, 0, item);
    patch({ images });
  };

  const clearAll = async () => {
    const ids = setting.images.map(image => image.id);
    if (setting.video) void removeVideo(setting.video.id);
    patch({ ...DEFAULT_BACKGROUND, veil: setting.veil, blur: setting.blur, accentMode: setting.accentMode, fit: setting.fit });
    await Promise.all(ids.map(id => removeImage(id)));
  };

  const hasMedia = setting.images.length > 0 || Boolean(setting.video);

  return (
    <section className={styles.group}>
      <h3>{t('background.title')}</h3>
      <p className={styles.lead}>{t('background.description')}</p>

      {/* --- 模式 --- */}
      <div className={styles.modes} role="radiogroup" aria-label={t('background.title')}>
        {MODES.map(mode => {
          const active = setting.mode === mode.id;
          // 没有素材时不允许选需要素材的模式：选了也看不到东西，是空按钮。
          const needsMedia = mode.id !== 'none';
          const lacking = needsMedia && !hasMedia;
          return (
            <button
              key={mode.id}
              type="button"
              role="radio"
              aria-checked={active}
              disabled={lacking && !active}
              title={t(mode.hintKey)}
              className={`${styles.mode} ${active ? styles.modeActive : ''}`}
              onClick={() => patch({ mode: mode.id })}
            >
              {mode.icon}
              <span>{t(mode.labelKey)}</span>
            </button>
          );
        })}
      </div>

      {/* --- 素材 --- */}
      <div className={styles.mediaBlock}>
        <div className={styles.mediaHead}>
          <div className={styles.rowCopy}>
            <strong>{t('background.images')}</strong>
            <span>{t('background.imagesHint', { values: { max: MAX_SLIDESHOW_IMAGES, size: formatBytes(MAX_IMAGE_BYTES) } })}</span>
          </div>
          <div className={styles.mediaActions}>
            {hasMedia && (
              <button type="button" className={styles.linkDanger} onClick={() => void clearAll()} disabled={busy}>
                <RotateCcw size={13} />{t('background.clearAll')}
              </button>
            )}
            <button type="button" className={styles.button} onClick={() => imageInputRef.current?.click()} disabled={busy || setting.images.length >= MAX_SLIDESHOW_IMAGES}>
              {busy ? <Loader2 size={13} className={styles.spin} /> : <Upload size={13} />}
              {t('background.addImages')}
            </button>
          </div>
        </div>

        <input
          ref={imageInputRef}
          type="file"
          accept={IMAGE_ACCEPT}
          multiple
          className={styles.hiddenInput}
          onChange={event => void pickImages(event.target.files)}
        />

        {setting.images.length === 0 ? (
          <button type="button" className={styles.dropzone} onClick={() => imageInputRef.current?.click()} disabled={busy}>
            <ImageIcon size={22} />
            <span>{t('background.dropHint')}</span>
          </button>
        ) : (
          <ul className={styles.thumbs}>
            {setting.images.map((image, index) => (
              <li key={image.id} className={styles.thumb}>
                <div className={styles.thumbImage}>
                  {thumbUrls[index] ? <img src={thumbUrls[index]} alt="" draggable={false} /> : <Loader2 size={16} className={styles.spin} />}
                  <span className={styles.thumbIndex}>{index + 1}</span>
                </div>
                <div className={styles.thumbFoot}>
                  <span className={styles.thumbName} title={image.name}>{image.name || t('background.unnamed')}</span>
                  <div className={styles.thumbActions}>
                    <button type="button" onClick={() => moveImage(index, -1)} disabled={index === 0} aria-label={t('background.moveUp')} title={t('background.moveUp')}>↑</button>
                    <button type="button" onClick={() => moveImage(index, 1)} disabled={index === setting.images.length - 1} aria-label={t('background.moveDown')} title={t('background.moveDown')}>↓</button>
                    <button type="button" onClick={() => void deleteImage(image.id)} aria-label={t('background.remove')} title={t('background.remove')}><Trash2 size={12} /></button>
                  </div>
                </div>
              </li>
            ))}
          </ul>
        )}
      </div>

      {/* --- 视频 --- */}
      <div className={styles.mediaBlock}>
        <div className={styles.mediaHead}>
          <div className={styles.rowCopy}>
            <strong>{t('background.video')}</strong>
            <span>{t('background.videoHint', { values: { size: formatBytes(MAX_VIDEO_BYTES) } })}</span>
          </div>
          <div className={styles.mediaActions}>
            {setting.video && (
              <button
                type="button"
                className={styles.linkDanger}
                onClick={() => {
                  const id = setting.video?.id;
                  patch({ video: null, mode: setting.mode === 'video' ? 'none' : setting.mode });
                  if (id) void removeVideo(id);
                }}
                disabled={busy}
              >
                <Trash2 size={13} />{t('background.remove')}
              </button>
            )}
            <button type="button" className={styles.button} onClick={() => videoInputRef.current?.click()} disabled={busy}>
              {busy ? <Loader2 size={13} className={styles.spin} /> : <Video size={13} />}
              {setting.video ? t('background.changeVideo') : t('background.pickVideo')}
            </button>
          </div>
        </div>
        <input
          ref={videoInputRef}
          type="file"
          accept={VIDEO_ACCEPT}
          className={styles.hiddenInput}
          onChange={event => void pickVideo(event.target.files?.[0])}
        />
        {setting.video && (
          <div className={styles.videoRow}>
            <div className={styles.videoPreview}>
              {videoPreview.url ? <video src={videoPreview.url} muted loop playsInline autoPlay /> : <Loader2 size={16} className={styles.spin} />}
            </div>
            <span className={styles.thumbName}>{setting.video.name}</span>
          </div>
        )}
      </div>

      {/* --- 呈现 --- */}
      <div className={styles.field}>
        <div className={styles.rowCopy}>
          <strong>{t('background.veil')}</strong>
          <span>{t('background.veilHint')}</span>
        </div>
        <div className={styles.slider}>
          <input
            type="range"
            min={0}
            max={100}
            step={1}
            value={setting.veil}
            onChange={event => patch({ veil: Number(event.target.value) })}
            aria-label={t('background.veil')}
          />
          <output>{setting.veil}%</output>
        </div>
      </div>

      <div className={styles.field}>
        <div className={styles.rowCopy}>
          <strong>{t('background.fit')}</strong>
          <span>{t('background.fitHint')}</span>
        </div>
        <div className={styles.segmented}>
          <button type="button" className={setting.fit === 'cover' ? styles.segmentActive : ''} onClick={() => patch({ fit: 'cover' })}>{t('background.fitCover')}</button>
          <button type="button" className={setting.fit === 'contain' ? styles.segmentActive : ''} onClick={() => patch({ fit: 'contain' })}>{t('background.fitContain')}</button>
        </div>
      </div>

      {setting.mode === 'slideshow' && (
        <div className={styles.field}>
          <div className={styles.rowCopy}>
            <strong>{t('background.interval')}</strong>
            <span>{t('background.intervalHint')}</span>
          </div>
          <div className={styles.slider}>
            <input
              type="range"
              min={3}
              max={120}
              step={1}
              value={setting.intervalSec}
              onChange={event => patch({ intervalSec: Number(event.target.value) })}
              aria-label={t('background.interval')}
            />
            <output>{setting.intervalSec}s</output>
          </div>
        </div>
      )}

      <div className={styles.row}>
        <div className={styles.rowCopy}>
          <strong>{t('background.blur')}</strong>
          <span>{t('background.blurHint')}</span>
        </div>
        <button
          type="button"
          role="switch"
          aria-checked={setting.blur}
          aria-label={t('background.blur')}
          className={`${styles.switch} ${setting.blur ? styles.switchOn : ''}`}
          onClick={() => patch({ blur: !setting.blur })}
        >
          <i />
        </button>
      </div>

      <div className={styles.row}>
        <div className={styles.rowCopy}>
          <strong>{t('background.autoAccent')}</strong>
          <span>{t('background.autoAccentHint')}</span>
        </div>
        <button
          type="button"
          role="switch"
          aria-checked={setting.accentMode === 'auto'}
          aria-label={t('background.autoAccent')}
          className={`${styles.switch} ${setting.accentMode === 'auto' ? styles.switchOn : ''}`}
          onClick={() => patch({ accentMode: setting.accentMode === 'auto' ? 'manual' : 'auto' })}
        >
          <i />
        </button>
      </div>

      {/*
        当前从背景里取到的颜色。用户看不到「后台算出了什么」会怀疑开关没生效——
        一张灰调照片算出来的颜色本来就很淡，不显示色块的话，界面上几乎看不出变化。
      */}
      {setting.accentMode === 'auto' && (
        <p className={styles.accentNote}>
          <span
            className={styles.accentSwatch}
            style={{ background: currentAutoAccent(setting) ?? 'transparent' }}
            aria-hidden="true"
          />
          {currentAutoAccent(setting)
            ? t('background.accentDetected', { values: { color: (currentAutoAccent(setting) ?? '').toUpperCase() } })
            : t('background.accentPending')}
        </p>
      )}

      {/* 预览：让用户在不退出设置的情况下看到遮罩浓度是否合适。 */}
      {hasMedia && (
        <div className={styles.preview} data-dark={dark ? 'true' : undefined}>
          <div className={styles.previewMedia}>
            {setting.mode === 'video' && videoPreview.url
              ? <video src={videoPreview.url} muted loop playsInline autoPlay />
              : thumbUrls[0] ? <img src={thumbUrls[0]} alt="" /> : null}
          </div>
          <div
            className={styles.previewVeil}
            style={{ background: dark ? `rgba(10,14,20,${setting.veil / 100})` : `rgba(249,250,251,${setting.veil / 100})` }}
          />
          <span className={styles.previewLabel}>{t('background.previewLabel')}</span>
        </div>
      )}
    </section>
  );
}

/**
 * 当前应该展示的自动强调色。
 *
 * 单张 / 视频用唯一的那个颜色；轮播用第一张——设置面板里不跟着轮播跳动，
 * 否则用户盯着色块看的时候它自己在闪。
 *
 * 视频的 `accent` 是选视频时从第一帧算出来并存进元数据的，所以这里不需要再
 * 等播放：如果它是 null，那就是真的没算出来（编码不支持），提示就应当如实说。
 */
function currentAutoAccent(setting: BackgroundSetting): string | null {
  if (setting.mode === 'video') return setting.video?.accent ?? null;
  return setting.images[0]?.accent ?? null;
}

/**
 * 供外层读取「采用哪个强调色」。
 *
 * 单独导出而不是写在组件里：App 需要在不打开设置面板的情况下也拿到同一个值。
 *
 * 索引只对轮播有意义：`currentIndex` 是轮播正在显示的那一张。单张与视频忽略它，
 * 因为这两种模式只有一份素材。
 */
export function autoAccentOf(setting: BackgroundSetting, currentIndex = 0): string | null {
  if (setting.accentMode !== 'auto' || setting.mode === 'none') return null;
  if (setting.mode === 'video') return setting.video?.accent ?? null;
  if (setting.mode === 'single') return setting.images[0]?.accent ?? null;
  if (setting.images.length === 0) return null;
  return setting.images[currentIndex % setting.images.length]?.accent ?? null;
}
