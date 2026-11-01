// 实例自定义图标的预览地址。
//
// 和 `useMediaUrl` 是同一种东西（读 IndexedDB 里的 Blob → 对象地址），但依赖是
// 「实例 id + 这张图的更新时间」而不是一个固定的 store/id。用更新时间当依赖，
// 是因为换图时实例 id 不变、IndexedDB 里的键也不变（同一个实例只存一张），
// 只靠 id 的话组件不会重新读库，界面会一直显示上一张。
//
// 释放规则与 useMediaUrl 一致：地址被替换或组件卸载时 revoke。实例卡片在游戏库
// 里可能有十几个，共用一份地址会让任意一张卸载时把别的卡片的图弄坏。

import { useEffect, useState } from 'react';
import { objectUrl, readBlob } from '../utils/background';
import type { InstanceIconRecord } from '../utils/instanceIcons';

/**
 * 读一个实例的自定义图标，返回可直接放进 `<img src>` 的地址。
 *
 * @param instanceId 实例 id；传 undefined 表示没有实例（返回空地址）。
 * @param record 元数据。只有存在记录时才会去读库——没有记录说明用户没设过图标，
 *   此时直接用加载器标志，不必为一个必然为空的读取等一次异步。
 */
export function useInstanceIconUrl(
  instanceId: string | undefined,
  record: InstanceIconRecord | undefined,
): { url: string; loading: boolean; missing: boolean } {
  const [state, setState] = useState<{ url: string; loading: boolean; missing: boolean }>({
    url: '',
    loading: false,
    missing: false,
  });
  // 更新时间是「同一实例换了图」的唯一信号，必须进依赖。
  const token = instanceId && record ? `${instanceId}@${record.updatedAt}` : '';

  useEffect(() => {
    if (!instanceId || !record) {
      setState({ url: '', loading: false, missing: false });
      return;
    }
    let cancelled = false;
    let created = '';
    setState(current => ({ url: current.url, loading: true, missing: false }));

    void readBlob('icons', instanceId).then(blob => {
      if (cancelled) return;
      if (!blob) {
        /*
         * 元数据还在但二进制没了（例如清了站点数据）。
         *
         * 这里必须把 `missing` 报给上层，而不是安静地回落到加载器标志：
         * 设置页会照着元数据说「自定义图片 · 某个文件名」，而旁边显示的是草方块——
         * 界面在自相矛盾。上层据此清掉这条元数据，状态就重新一致了。
         */
        setState({ url: '', loading: false, missing: true });
        return;
      }
      created = objectUrl(blob);
      setState({ url: created, loading: false, missing: false });
    });

    return () => {
      cancelled = true;
      if (created) URL.revokeObjectURL(created);
    };
    // `record` 只通过 token 参与依赖：它每次渲染都是新对象，直接依赖会每帧读库。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [token]);

  return state;
}
