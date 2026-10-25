// 跨源排序与合并。
//
// 难点：两个源给的排序键不可比——Modrinth 的 relevance 和 CurseForge 的
// Popularity 是各自算法算出来的分数，没有共同标尺；follows 只有 Modrinth 有；
// 名称的大小写与语言也不同。
//
// 所以分两种模式：
//   comparable  - 选中的键两边都有真实值（downloads / newest / updated / name），
//                 各自请求后按同一键本地排序合并，结果顺序是可信的。
//   interleave  - 选中的键只有一边有（follows），或两边口径不同（popular），
//                 各自按自己的默认序取回，再交替插入。UI 上标注「混合排序」，
//                 避免让用户以为这是严格的全局排名。

import type { SortDirection, SortKey, StoreEntry } from './types';
import { LOCALES, type Language } from '../i18n';

/** 该排序键是否是两边都能真实比较的。 */
export function isComparable(sort: SortKey): boolean {
  return sort === 'downloads' || sort === 'newest' || sort === 'updated' || sort === 'name';
}

/**
 * 当前排序是否需要标注「混合排序」。
 *
 * 只有一个来源时不混合：它的原生排序（哪怕叫 popularity）就是唯一权威，
 * 标成混合反而误导。只有同时选了多个来源、而该键无法跨源比较时才标。
 */
export function isMixed(sort: SortKey, sources: string[]): boolean {
  if (sources.length <= 1) return false;
  // follows 只存在于 Modrinth：一旦同时选了 CurseForge，该键就没有共同标尺。
  if (!isComparable(sort)) return true;
  if (sort === 'follows' && sources.includes('CurseForge')) return true;
  return false;
}

function valueFor(entry: StoreEntry, sort: SortKey, language: Language): number | string | undefined {
  switch (sort) {
    case 'downloads':
    case 'popular':
      return entry.downloads;
    case 'follows':
      return entry.follows;
    case 'newest':
      return entry.released ?? entry.modified;
    case 'updated':
      return entry.modified;
    case 'name':
      return entry.name.toLocaleLowerCase(LOCALES[language]);
  }
}

/** 按单一键排序。缺值的条目永远排在后面，不参与方向翻转。 */
export function sortEntries(
  entries: StoreEntry[],
  sort: SortKey,
  direction: SortDirection,
  language: Language = 'zh-CN',
): StoreEntry[] {
  const sign = direction === 'asc' ? 1 : -1;
  const locale = LOCALES[language];
  return [...entries].sort((a, b) => {
    const left = valueFor(a, sort, language);
    const right = valueFor(b, sort, language);
    if (left === undefined && right === undefined) return a.name.localeCompare(b.name, locale);
    if (left === undefined) return 1;
    if (right === undefined) return -1;
    if (typeof left === 'string' && typeof right === 'string') {
      return sign * left.localeCompare(right, locale);
    }
    const diff = Number(left) - Number(right);
    if (diff !== 0) return sign * diff;
    return a.name.localeCompare(b.name, locale);
  });
}

/**
 * 交替合并两个已排序的列表，让两个源都有曝光机会。
 * 这是「混合排序」的呈现方式：不假装有一个共同的分数。
 */
export function interleave(first: StoreEntry[], second: StoreEntry[]): StoreEntry[] {
  const result: StoreEntry[] = [];
  const max = Math.max(first.length, second.length);
  for (let i = 0; i < max; i += 1) {
    if (first[i]) result.push(first[i]);
    if (second[i]) result.push(second[i]);
  }
  return result;
}

/**
 * 合并多源结果。
 * @param perSource 各源自己的结果（已按该源的默认序返回）
 * @param sort 用户选择的排序键
 * @param direction 方向
 */
export function mergeResults(
  perSource: { source: string; entries: StoreEntry[] }[],
  sort: SortKey,
  direction: SortDirection,
  language: Language = 'zh-CN',
): StoreEntry[] {
  const lists = perSource.filter(group => group.entries.length > 0).sort((a, b) => a.source.localeCompare(b.source));
  if (lists.length === 0) return [];

  // 不可比的键（popular / follows）：每个源自己的顺序就是权威，不能在本地
  // 用 downloads 之类的替代值重排，否则会把「热度」偷偷变成「下载量」。
  if (!isComparable(sort) || sort === 'popular' || sort === 'follows') {
    if (lists.length === 1) {
      const native = lists[0].entries;
      // 单源时升/降序仍然尊重用户的点击，只是翻转原生顺序。
      return direction === 'asc' ? [...native].reverse() : native;
    }
    // 多源：各自保持源内顺序，交替合并，让两边都有曝光。
    let merged = lists[0].entries;
    for (let i = 1; i < lists.length; i += 1) merged = interleave(merged, lists[i].entries);
    return merged;
  }

  return sortEntries(lists.flatMap(group => group.entries), sort, direction, language);
}
