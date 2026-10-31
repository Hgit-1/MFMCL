// 从 IndexedDB 里读一个 Blob 并转成可以放进 `src` 的对象地址。
//
// 这个 hook 存在的唯一理由是**释放**：`URL.createObjectURL` 创建的是进程级的
// 引用，不 revoke 就一直占着内存，而浏览器不会在元素被移除时自动回收。一段
// 30 MB 的循环视频切换三次背景就留下 90 MB 悬空引用，几次之后启动器会明显变卡。
//
// 释放时机是「地址被替换或组件卸载」，不是在元素 unmount 时——同一个地址会被
// 多个元素共用（当前帧与正在预载的下一帧），由持有者统一负责才不会出现
// 「另一处还在用，地址已经失效」的破图。

import { useEffect, useState } from 'react';
import { objectUrl, readBlob } from '../utils/background';

export interface LoadedMedia {
  url: string;
  /** 读取仍在进行。用于让调用方区分「还没有」和「没有」。 */
  loading: boolean;
}

/**
 * 读取一个媒体 Blob 并返回对象地址。
 *
 * `store` 与 `id` 都为 null 时返回空地址（没有背景）。异步读取期间保留上一份
 * 地址，避免切换图片时闪一下黑屏。
 */
export function useMediaUrl(store: 'images' | 'video' | null, id: string | null): LoadedMedia {
  const [state, setState] = useState<LoadedMedia>({ url: '', loading: Boolean(store && id) });

  useEffect(() => {
    if (!store || !id) {
      setState({ url: '', loading: false });
      return;
    }
    let cancelled = false;
    let created = '';
    setState(current => ({ url: current.url, loading: true }));

    void readBlob(store, id).then((blob: Blob | null) => {
      if (cancelled) return;
      if (!blob) {
        // 元数据还在但二进制没了（用户清了站点数据），当作没有背景处理。
        setState({ url: '', loading: false });
        return;
      }
      created = objectUrl(blob);
      setState({ url: created, loading: false });
    });

    return () => {
      cancelled = true;
      if (created) URL.revokeObjectURL(created);
    };
  }, [store, id]);

  return state;
}

/** 一次读多张图片，供轮播预载使用。与 `useMediaUrl` 同规则：换一批就释放上一批。 */
export function useMediaUrls(ids: string[]): string[] {
  const [urls, setUrls] = useState<string[]>([]);
  // 依赖数组里放拼接后的键而不是数组本身：调用方每次渲染都会新建一个数组，
  // 用引用做依赖会每帧重新读库。
  const key = ids.join('|');

  useEffect(() => {
    const list = key ? key.split('|') : [];
    if (list.length === 0) {
      setUrls([]);
      return;
    }
    let cancelled = false;
    const created: string[] = [];

    void Promise.all(list.map(id => readBlob('images', id))).then(blobs => {
      if (cancelled) return;
      const next: string[] = [];
      for (const blob of blobs) {
        if (!blob) {
          next.push('');
          continue;
        }
        const url = objectUrl(blob);
        created.push(url);
        next.push(url);
      }
      setUrls(next);
    });

    return () => {
      cancelled = true;
      created.forEach(url => URL.revokeObjectURL(url));
    };
  }, [key]);

  return urls;
}
