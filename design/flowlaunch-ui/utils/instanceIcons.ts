// 实例自定义图标。
//
// ## 为什么单独一个模块
//
// 图标是「图片 + 一条归属记录」，两半的存储位置不同：
//   1. 图片本身是二进制，进不了 localStorage（约 5 MB 且是同步 API，一张
//      512×512 的 PNG 转 data URL 就要几百 KB，会把主线程卡住），存 IndexedDB；
//   2. 「哪个实例用过自定义图标」只是几条元数据，存 localStorage。
//
// 把它和背景图放在同一个 IndexedDB 库里（`mfmcl-media`），是因为两者是同一种
// 东西——用户选的图片。分库会让「清空站点数据」出现两种不同的失败形态。
//
// ## 为什么键就是实例 id
//
// 一个实例只有一个自定义图标，用实例 id 当主键意味着：
//   - 换图标就是 `put` 覆盖，不会像自增键那样留下没人引用的旧图；
//   - 删除实例时按 id 删一次就干净了，不需要先查映射表。
//
// ## 失败时的行为
//
// 所有写入都先落 IndexedDB、成功之后才写元数据。反过来先写元数据的话，一次
// 存储失败就会留下「记录说有图标、实际读不出图」的破图状态——而那时候界面
// 已经切到自定义图标分支了，用户看到的是一个空白方块。

import { deleteBlob, putBlob, readBlob } from './background';
/** 一条自定义图标记录。 */
export interface InstanceIconRecord {
  /** 原始文件名，用于在设置页显示「当前用的是哪张图」。 */
  name: string;
  /** 写入时间（毫秒）。用于在「已恢复默认」这类提示里保持语句具体。 */
  updatedAt: number;
}

export type InstanceIconMap = Record<string, InstanceIconRecord>;

export const INSTANCE_ICONS_KEY = 'mfmcl-instance-icons';

/**
 * 单张图标的大小上限。
 *
 * 图标在界面上最大只显示到 72px，2 MB 已经远超需要的分辨率（足够 1024×1024
 * 的无损 PNG）。再大只会让每次打开游戏库都多读几十 MB 的 Blob。
 */
export const MAX_ICON_BYTES = 2 * 1024 * 1024;

/** 接受的文件类型。与背景图一致，只列浏览器都能解码的格式。 */
export const ICON_ACCEPT = 'image/png,image/jpeg,image/webp,image/avif,image/gif';

/** 被拒的原因。调用方据此选一条本地化文案，而不是自己拼错误字符串。 */
export type IconRejectReason = 'type' | 'size' | 'decode' | 'storage';

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}

/** 读取「哪些实例有自定义图标」。存档损坏时当作全部没有，而不是抛错。 */
export function loadInstanceIcons(): InstanceIconMap {
  let raw: unknown;
  try {
    raw = JSON.parse(localStorage.getItem(INSTANCE_ICONS_KEY) || '{}');
  } catch {
    return {};
  }
  if (!isPlainObject(raw)) return {};
  const result: InstanceIconMap = {};
  for (const [instanceId, value] of Object.entries(raw)) {
    if (!isPlainObject(value)) continue;
    if (typeof value.name !== 'string') continue;
    result[instanceId] = {
      name: value.name.slice(0, 200),
      updatedAt: typeof value.updatedAt === 'number' && Number.isFinite(value.updatedAt)
        ? value.updatedAt
        : 0,
    };
  }
  return result;
}

/** 写入元数据。存储不可用时静默失败——图标是外观，不该阻断任何操作。 */
export function saveInstanceIcons(map: InstanceIconMap): void {
  try {
    localStorage.setItem(INSTANCE_ICONS_KEY, JSON.stringify(map));
  } catch {
    // 隐私模式或配额已满：本次会话内还能看到图，重启后回到默认图标。
  }
}

/**
 * 校验并保存一张自定义图标。
 *
 * 先解码一次再落库，是为了把「选了一张损坏的图」挡在选择的时候，而不是等到
 * 卡片渲染时才失败。`createImageBitmap` 在浏览器里对 GIF/AVIF 的行为不完全
 * 一致，所以解码失败只当作不可用——用户还能选另一张。
 */
export async function importInstanceIcon(
  instanceId: string,
  file: File,
): Promise<{ ok: true; record: InstanceIconRecord } | { ok: false; reason: IconRejectReason }> {
  if (!file.type.startsWith('image/')) return { ok: false, reason: 'type' };
  if (file.size > MAX_ICON_BYTES) return { ok: false, reason: 'size' };
  if (typeof createImageBitmap === 'function') {
    try {
      const bitmap = await createImageBitmap(file);
      bitmap.close();
    } catch {
      return { ok: false, reason: 'decode' };
    }
  }
  const stored = await putBlob('icons', instanceId, file);
  if (!stored) return { ok: false, reason: 'storage' };
  return { ok: true, record: { name: file.name, updatedAt: Date.now() } };
}

/** 读取某个实例的自定义图标 Blob。没有设置过时返回 null。 */
export async function readInstanceIcon(instanceId: string): Promise<Blob | null> {
  return readBlob('icons', instanceId);
}

/**
 * 删除某个实例的自定义图标。
 *
 * 元数据由调用方从 map 里摘掉：这里只负责 IndexedDB 那一半，因为「摘掉记录」
 * 还要和 React 状态同步，属于调用方的职责。
 */
export async function removeInstanceIcon(instanceId: string): Promise<void> {
  await deleteBlob('icons', instanceId);
}
