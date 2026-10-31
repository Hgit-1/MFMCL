// 自由背景：一张图片、一组轮播图片，或者一段循环播放的视频。
//
// ## 为什么单独一个模块
//
// 「背景」涉及三件互不相同的事，混在组件里会互相牵连：
//
//   1. **来源与校验**（这里）：文件类型、大小、张数、视频能不能播；
//   2. **持久化**：图片与视频都是二进制，进不了后端那个只接受 PNG/JPEG/WebP
//      data URL 的 `custom_background` 字段，也放不进 localStorage；
//   3. **呈现与配色**：图层怎么叠、遮罩多厚、强调色从哪来。
//
// 这个模块负责 1 和 2，`BackgroundLayer` 负责 3。
//
// ## 存储位置：IndexedDB，不放 localStorage
//
// localStorage 只有约 5 MB，而且存的都是字符串——一张 1080p 的 JPEG 转成
// data URL 就要 1-2 MB，一段几秒的视频更是几十 MB，两者都会把主线程卡住
// （同步 API）。IndexedDB 存的直接是 `Blob`，没有编码开销，配额也大得多。
//
// 后端的 `custom_background` 字段仍然保留给「单张图片」的兼容路径，但完整
// 的背景设置（图片集、视频、遮罩、配色策略）只存在本地。这是有意的：它是
// 这台机器上的外观偏好，不该跟着账号或实例走。
//
// ## 为什么不做「把视频抽帧当强调色」
//
// 视频每一帧的颜色都在变，跟随它会得到一个不断闪烁的界面。视频只取第一帧
// 分析一次，之后固定——用户在选完视频的那一刻就看到了最终的配色，不会觉得
// 界面在自己乱动。

import { analyzeImageData } from './accent';

/** 一张图片或一段视频的来源。 */
export type BackgroundMediaKind = 'image' | 'video';

/** 背景图层的来源类型。 */
export type BackgroundMode = 'none' | 'single' | 'slideshow' | 'video';

/** 一张图片在图片集里的独立设置。轮播间隔按全体共用，不逐张配置。 */
export interface BackgroundImage {
  /** IndexedDB 主键，同时也是这一项的稳定标识。 */
  id: string;
  /** 文件名，用于在设置面板里认出是哪一张。 */
  name: string;
  /** 推荐的强调色（来自这张图），跟随背景配色时使用。 */
  accent: string | null;
  /** 图片的平均明度，0-1。用于推荐深浅主题与遮罩浓度。 */
  lightness: number;
}

/** 一整套背景设置。整份存在 localStorage 里（只有元数据，没有二进制）。 */
export interface BackgroundSetting {
  mode: BackgroundMode;
  /** `single` / `slideshow` 用的图片列表；空表示还没选过。 */
  images: BackgroundImage[];
  /** 轮播间隔（秒）。1 秒以下会让人晕，60 秒以上等于静态图。 */
  intervalSec: number;
  /**
   * 视频的元数据；`mode === 'video'` 时才有意义。
   *
   * 与图片一样带 `accent` / `lightness`：它们是从第一帧算出来的，算一次就固定。
   * 不把它们放进元数据的话，每次启动都要重新解码一遍视频才能知道该用什么颜色，
   * 而「跟随背景配色」的用户会在首帧拿到默认蓝再到位的闪烁。
   */
  video: { id: string; name: string; accent: string | null; lightness: number } | null;
  /** 遮罩浓度，0-100。作用是压住背景、保证前景文字可读。 */
  veil: number;
  /** 是否把背景当作模糊底图（PCL2 的「毛玻璃」）。 */
  blur: boolean;
  /**
   * 配色策略：
   *   `manual`  —— 用设置里选的强调色（默认）；
   *   `auto`    —— 从当前背景里取色，跟随图片/视频变化。
   */
  accentMode: 'manual' | 'auto';
  /** 背景铺满还是留白（`contain` 用于竖图，不裁掉内容）。 */
  fit: 'cover' | 'contain';
}

export const DEFAULT_BACKGROUND: BackgroundSetting = {
  mode: 'none',
  images: [],
  intervalSec: 12,
  video: null,
  veil: 55,
  blur: false,
  accentMode: 'manual',
  fit: 'cover',
};

export const BACKGROUND_STORAGE_KEY = 'mfmcl-background';

/** IndexedDB 的库名与两个 object store。 */
const DB_NAME = 'mfmcl-media';
const DB_VERSION = 1;
const STORE_IMAGES = 'images';
const STORE_VIDEO = 'video';

/**
 * 上限。
 *
 * 图片单张 20 MB：再大就不是壁纸而是原图了，解码一次会明显卡顿。视频 200 MB：
 * 桌面端放一段十几秒的循环足够，同时避免用户误选一部电影把配额吃满。
 */
export const MAX_IMAGE_BYTES = 20 * 1024 * 1024;
export const MAX_VIDEO_BYTES = 200 * 1024 * 1024;

/** 图片集张数上限。再多就不是「几张喜欢的图」，而是一个没人会看完的幻灯片。 */
export const MAX_SLIDESHOW_IMAGES = 12;

/** 接受的文件类型。只列浏览器能解码的，避免选完才发现播不出来。 */
export const IMAGE_ACCEPT = 'image/png,image/jpeg,image/webp,image/avif,image/gif';
export const VIDEO_ACCEPT = 'video/mp4,video/webm';

/** 分析强调色时把图片缩到多宽。采样点少一个数量级，颜色不会被细节带偏。 */
const ANALYSIS_WIDTH = 96;

// ---------------------------------------------------------------------------
// 元数据（localStorage）
// ---------------------------------------------------------------------------

/**
 * 读取背景设置。
 *
 * 逐字段兜底而不是直接信任存档：字段会随版本增减，一个缺失的 `veil` 若变成
 * `undefined`，内联样式里会算出 `NaN%`，整个遮罩消失、前景直接压在照片上。
 */
export function readBackgroundSetting(): BackgroundSetting {
  let raw: unknown;
  try {
    raw = JSON.parse(localStorage.getItem(BACKGROUND_STORAGE_KEY) || 'null');
  } catch {
    return { ...DEFAULT_BACKGROUND };
  }
  if (!raw || typeof raw !== 'object') return { ...DEFAULT_BACKGROUND };
  const value = raw as Partial<BackgroundSetting>;
  const images = Array.isArray(value.images)
    ? value.images
        .filter((item): item is BackgroundImage => Boolean(item && typeof item === 'object' && typeof item.id === 'string'))
        .slice(0, MAX_SLIDESHOW_IMAGES)
        .map(item => ({
          id: item.id,
          name: typeof item.name === 'string' ? item.name : '',
          accent: typeof item.accent === 'string' ? item.accent : null,
          lightness: typeof item.lightness === 'number' ? item.lightness : 0.5,
        }))
    : [];
  const video = value.video && typeof value.video.id === 'string'
    ? {
        id: value.video.id,
        name: typeof value.video.name === 'string' ? value.video.name : '',
        accent: typeof value.video.accent === 'string' ? value.video.accent : null,
        lightness: typeof value.video.lightness === 'number' ? value.video.lightness : 0.5,
      }
    : null;

  return {
    mode: value.mode === 'single' || value.mode === 'slideshow' || value.mode === 'video' ? value.mode : 'none',
    images,
    intervalSec: clamp(Number(value.intervalSec) || DEFAULT_BACKGROUND.intervalSec, 3, 120),
    video,
    veil: clamp(Number.isFinite(value.veil) ? Number(value.veil) : DEFAULT_BACKGROUND.veil, 0, 100),
    blur: value.blur === true,
    accentMode: value.accentMode === 'auto' ? 'auto' : 'manual',
    fit: value.fit === 'contain' ? 'contain' : 'cover',
  };
}

/** 写入背景设置。存储不可用时静默失败——背景是外观，不该阻断任何操作。 */
export function saveBackgroundSetting(setting: BackgroundSetting): void {
  try {
    localStorage.setItem(BACKGROUND_STORAGE_KEY, JSON.stringify(setting));
  } catch {
    // 存储被禁用（隐私模式）时只在本次会话生效。
  }
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

// ---------------------------------------------------------------------------
// 媒体（IndexedDB）
// ---------------------------------------------------------------------------

let dbPromise: Promise<IDBDatabase | null> | null = null;

/**
 * 打开数据库。
 *
 * 打不开时返回 `null` 而不是抛错：IndexedDB 在隐私模式、配额被拒、或文件系统
 * 只读的环境里都会失败，而背景功能在这些情况下最坏的结果是「这次选的图看不了」，
 * 不应该让整个设置面板报错。
 */
function openDb(): Promise<IDBDatabase | null> {
  if (dbPromise) return dbPromise;
  dbPromise = new Promise(resolve => {
    if (typeof indexedDB === 'undefined') {
      resolve(null);
      return;
    }
    let request: IDBOpenDBRequest;
    try {
      request = indexedDB.open(DB_NAME, DB_VERSION);
    } catch {
      resolve(null);
      return;
    }
    request.onupgradeneeded = () => {
      const db = request.result;
      if (!db.objectStoreNames.contains(STORE_IMAGES)) db.createObjectStore(STORE_IMAGES);
      if (!db.objectStoreNames.contains(STORE_VIDEO)) db.createObjectStore(STORE_VIDEO);
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => resolve(null);
    request.onblocked = () => resolve(null);
  });
  return dbPromise;
}

function runTransaction<T>(
  store: string,
  mode: IDBTransactionMode,
  work: (objectStore: IDBObjectStore) => IDBRequest<T>,
): Promise<T | null> {
  return openDb().then(db => {
    if (!db) return null;
    return new Promise<T | null>(resolve => {
      let request: IDBRequest<T>;
      try {
        request = work(db.transaction(store, mode).objectStore(store));
      } catch {
        resolve(null);
        return;
      }
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => resolve(null);
    });
  });
}

/** 存一个 Blob，返回它的键。 */
async function putBlob(store: string, id: string, blob: Blob): Promise<boolean> {
  const result = await runTransaction<IDBValidKey>(store, 'readwrite', objectStore => objectStore.put(blob, id));
  return result !== null;
}

/** 取出一个 Blob。键不存在或被删除时返回 null。 */
export async function readBlob(store: 'images' | 'video', id: string): Promise<Blob | null> {
  const result = await runTransaction<Blob | undefined>(store, 'readonly', objectStore => objectStore.get(id));
  return result instanceof Blob ? result : null;
}

async function deleteBlob(store: string, id: string): Promise<void> {
  await runTransaction(store, 'readwrite', objectStore => objectStore.delete(id) as IDBRequest<undefined>);
}

/** 生成一个不会与已有的键冲突的 id。 */
function newId(): string {
  return `bg-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
}

// ---------------------------------------------------------------------------
// 选图 / 选视频
// ---------------------------------------------------------------------------

export interface AcceptedMedia<T extends BackgroundImage = BackgroundImage> {
  /** 校验通过、已经落库的条目。 */
  accepted: T[];
  /** 被拒的文件与原因（键 + 参数，便于本地化）。 */
  rejected: { name: string; reason: 'type' | 'size' | 'decode' | 'count' }[];
}

/**
 * 把用户选的图片存下来并分析出颜色。
 *
 * 只在浏览器里给 `File`；桌面版通过 `<input type="file">` 也能拿到同一份 `File`，
 * 所以不需要走后端的文件对话框——那条路径要读绝对路径、再过 asset 协议，
 * 对一个只用于显示的东西太重了。
 */
export async function importImages(
  files: File[],
  remainingSlots: number,
): Promise<AcceptedMedia> {
  const accepted: BackgroundImage[] = [];
  const rejected: AcceptedMedia['rejected'] = [];

  for (const file of files) {
    if (accepted.length >= remainingSlots) {
      rejected.push({ name: file.name, reason: 'count' });
      continue;
    }
    // 只认 MIME 里的 image/*。扩展名可以改，MIME 至少是浏览器自己读出来的。
    if (!file.type.startsWith('image/')) {
      rejected.push({ name: file.name, reason: 'type' });
      continue;
    }
    if (file.size > MAX_IMAGE_BYTES) {
      rejected.push({ name: file.name, reason: 'size' });
      continue;
    }
    const analysis = await analyzeBlob(file).catch(() => null);
    if (!analysis) {
      rejected.push({ name: file.name, reason: 'decode' });
      continue;
    }
    const id = newId();
    const stored = await putBlob(STORE_IMAGES, id, file);
    if (!stored) {
      rejected.push({ name: file.name, reason: 'decode' });
      continue;
    }
    accepted.push({ id, name: file.name, accent: analysis.accent, lightness: analysis.lightness });
  }

  return { accepted, rejected };
}

/** 存一段视频。只校验类型与大小，能不能播由 `<video>` 自己决定。 */
export async function importVideo(
  file: File,
): Promise<{ video: { id: string; name: string; accent: string | null; lightness: number } | null; reason?: 'type' | 'size' | 'decode' }> {  if (!file.type.startsWith('video/')) return { video: null, reason: 'type' };
  if (file.size > MAX_VIDEO_BYTES) return { video: null, reason: 'size' };
  /*
   * 强调色取第一帧。
   *
   * 不逐帧跟随：界面颜色随视频闪烁既刺眼又廉价。取一帧之后固定下来，用户
   * 在选完视频的瞬间就看到了最终配色。
   *
   * 抽帧失败不算致命：视频可能是不支持的编码，`<video>` 那边同样会失败并给出
   * 提示；这里返回没有强调色的条目即可，不要让「取不到颜色」挡住「能播放」。
   */
  const analysis = await analyzeVideoPoster(file).catch(() => null);
  const id = newId();
  const stored = await putBlob(STORE_VIDEO, id, file);
  if (!stored) return { video: null, reason: 'decode' };
  return { video: { id, name: file.name, accent: analysis?.accent ?? null, lightness: analysis?.lightness ?? 0.5 } };
}

/** 删除一张图片（元数据由调用方从设置里摘掉）。 */
export async function removeImage(id: string): Promise<void> {
  await deleteBlob(STORE_IMAGES, id);
}

/** 删除视频。 */
export async function removeVideo(id: string): Promise<void> {
  await deleteBlob(STORE_VIDEO, id);
}

// ---------------------------------------------------------------------------
// 解码与分析
// ---------------------------------------------------------------------------

/**
 * 把任意图片 Blob 解码后缩到 `ANALYSIS_WIDTH`，返回逐像素的颜色统计。
 *
 * 走 `createImageBitmap` 而不是 `<img>`：前者在缩略图场景下不需要挂到 DOM、
 * 也不会触发一次布局。失败时（不支持的格式）返回 null，由调用方决定怎么报告。
 */
export async function analyzeBlob(blob: Blob): Promise<{ accent: string | null; lightness: number } | null> {
  if (typeof createImageBitmap !== 'function') return null;
  let bitmap: ImageBitmap | null = null;
  try {
    bitmap = await createImageBitmap(blob);
    const scale = Math.min(1, ANALYSIS_WIDTH / Math.max(1, bitmap.width));
    const width = Math.max(1, Math.round(bitmap.width * scale));
    const height = Math.max(1, Math.round(bitmap.height * scale));
    const canvas = document.createElement('canvas');
    canvas.width = width;
    canvas.height = height;
    const context = canvas.getContext('2d', { willReadFrequently: true });
    if (!context) return null;
    context.drawImage(bitmap, 0, 0, width, height);
    return analyzeImageData(context.getImageData(0, 0, width, height).data);
  } catch {
    return null;
  } finally {
    bitmap?.close();
  }
}

/**
 * 从视频里抽一帧做分析。
 *
 * 需要一个临时的 `<video>`：`createImageBitmap` 不接受视频元素，必须经画布
 * 中转。监听 `loadeddata`（有了一帧可画的数据）而不是 `canplay`：后者可能在
 * 缓冲完成前就触发，此时画布上是空的。
 *
 * `muted` + `playsInline` 是自动播放的前提；某些浏览器即使只读取也会拒绝在
 * 没有静音的视频上 seek，所以两者都设上。超时 8 秒后放弃——一个损坏的文件
 * 会让这个事件永远不来。
 */
export async function analyzeVideoPoster(blob: Blob): Promise<{ accent: string | null; lightness: number } | null> {
  const url = URL.createObjectURL(blob);
  const video = document.createElement('video');
  video.muted = true;
  video.playsInline = true;
  video.preload = 'auto';
  video.src = url;

  try {
    await new Promise<void>((resolve, reject) => {
      const timer = window.setTimeout(() => reject(new Error('timeout')), 8000);
      const done = () => { window.clearTimeout(timer); resolve(); };
      const fail = () => { window.clearTimeout(timer); reject(new Error('decode')); };
      video.addEventListener('loadeddata', done, { once: true });
      video.addEventListener('error', fail, { once: true });
    });

    const width = Math.max(1, Math.min(ANALYSIS_WIDTH, video.videoWidth || ANALYSIS_WIDTH));
    const height = Math.max(1, Math.round((video.videoHeight || ANALYSIS_WIDTH) * (width / (video.videoWidth || width))));
    const canvas = document.createElement('canvas');
    canvas.width = width;
    canvas.height = height;
    const context = canvas.getContext('2d', { willReadFrequently: true });
    if (!context) return null;
    context.drawImage(video, 0, 0, width, height);
    return analyzeImageData(context.getImageData(0, 0, width, height).data);
  } catch {
    return null;
  } finally {
    video.removeAttribute('src');
    video.load();
    URL.revokeObjectURL(url);
  }
}

/**
 * 把 Blob 变成一个可以直接放进 `src` 的地址。
 *
 * 调用方负责在卸载时 `URL.revokeObjectURL`——一个还没被释放的地址会让整段视频
 * 留在内存里，切换几次背景就能吃掉几百 MB。`useObjectUrl` 封装了这件事。
 */
export function objectUrl(blob: Blob): string {
  return URL.createObjectURL(blob);
}

/**
 * 按遮罩浓度与主题给出遮罩色。
 *
 * 浅色主题用白遮罩、深色主题用黑遮罩，而不是统一用黑：白色遮罩压亮背景，
 * 深色文字在上面更清楚；统一用黑会让浅色主题的界面整体变灰。
 */
export function veilColor(veil: number, dark: boolean): string {
  const alpha = clamp(veil, 0, 100) / 100;
  return dark ? `rgba(10, 14, 20, ${alpha})` : `rgba(249, 250, 251, ${alpha})`;
}
