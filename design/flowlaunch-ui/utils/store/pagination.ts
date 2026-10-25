// 结果分页的纯计算。
//
// 两个来源的分页模型不同，所以这里只放「跟数据无关的算术」，不碰 React 和网络：
//
//   原版     清单一次拉全量，本地筛选排序后切片
//   Mod 等   API 按 offset/limit 分页，只返回当前页
//
// 跨源时页数取各源 total 的较大者——用户看到的是合并列表，页数必须覆盖条目更
// 多的那个源，否则另一个源的可翻条目会被截断。

/** 每页条数。Modrinth 与 CurseForge 都接受这个 pageSize。 */
export const PAGE_SIZE = 20;

/**
 * 页数上限。
 *
 * 不设上限的话，Modrinth 的 total 动辄几十万，用户可以一路点到几万页之后，
 * 每次都是一次真实请求。50 页（1000 条）足够日常浏览，也远低于 CurseForge
 * 「index + pageSize ≤ 10000」的限制。
 */
export const MAX_PAGE = 50;

/** 把页码夹进 [0, pages-1]；pages 至少为 1，避免出现 -1 页。 */
export function clampPage(page: number, pages: number): number {
  const last = Math.max(0, pages - 1);
  return Math.max(0, Math.min(last, Math.trunc(page)));
}

/** 由总条数算页数。0 条也算 1 页，这样「第 1 / 1 页」不会变成「第 1 / 0 页」。 */
export function pageCountFor(total: number, pageSize = PAGE_SIZE): number {
  if (!Number.isFinite(total) || total <= 0) return 1;
  return Math.max(1, Math.ceil(total / pageSize));
}

/**
 * 跨源页数：取各源 total 的最大值。
 * 传入空数组（尚未搜索）时返回 1 页。
 */
export function mergedPageCount(totals: (number | undefined)[], pageSize = PAGE_SIZE, maxPage = MAX_PAGE): number {
  const known = totals.filter((value): value is number => typeof value === 'number' && Number.isFinite(value));
  if (known.length === 0) return 1;
  return Math.min(maxPage, pageCountFor(Math.max(...known), pageSize));
}

export interface PageSlice<T> {
  entries: T[];
  /** 本页首条在整体结果中的序号（1 起）。空页时等于起始偏移 + 1。 */
  from: number;
  /** 本页末条序号（1 起）。空页时与 from 相等减一，即 from - 1。 */
  to: number;
  page: number;
  pages: number;
  total: number;
}

/** 本地列表切片。页码会先被夹进有效范围，越界访问不会返回空页。 */
export function slicePage<T>(items: T[], page: number, pageSize = PAGE_SIZE): PageSlice<T> {
  const pages = pageCountFor(items.length, pageSize);
  const safePage = clampPage(page, pages);
  const start = safePage * pageSize;
  const entries = items.slice(start, start + pageSize);
  return {
    entries,
    from: start + 1,
    to: start + entries.length,
    page: safePage,
    pages,
    total: items.length,
  };
}
