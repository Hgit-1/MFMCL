// Modrinth 搜索。
//
// 端点与参数照官方文档 v2/search。两个必须注意的行为：
//
// 1. Modrinth 的 API 使用条款要求请求带可识别的 User-Agent，浏览器无法设置
//    这个头（会被当成 forbidden header 丢弃）。因此这里只做「用户主动搜索」，
//    并做 350ms 防抖；不做轮询、不做后台预取，把请求量压到最低。
// 2. 没有 "world" 这个 project_type。地图类只能走 CurseForge，所以这里对
//    category === 'world' 直接返回说明性错误，而不是发一个必然为空请求。

import type { SortDirection, SortKey, StoreCategory, StoreEntry, StorePage } from './types';
import { StoreError } from './types';
import { getJson, HttpError } from './http';
import { loc, localizedTextOf } from '../i18n/localized';

const SEARCH_URL = 'https://api.modrinth.com/v2/search';

/** StoreCategory → Modrinth project_type。null 表示该分类 Modrinth 不提供。 */
export function projectTypeFor(category: StoreCategory): string | null {
  switch (category) {
    case 'mod': return 'mod';
    case 'modpack': return 'modpack';
    case 'resourcepack': return 'resourcepack';
    case 'vanilla': return null;
    case 'world': return null;
  }
}

/** SortKey → Modrinth index。popular 用 downloads 近似（Modrinth 没有 popularity 指标）。 */
export function indexFor(sort: SortKey): 'relevance' | 'downloads' | 'follows' | 'newest' | 'updated' {
  switch (sort) {
    case 'popular': return 'downloads';
    case 'downloads': return 'downloads';
    case 'follows': return 'follows';
    case 'newest': return 'newest';
    case 'updated': return 'updated';
    case 'name': return 'relevance'; // Modrinth 不支持按名称排序，取默认相关度后本地排序
  }
}

interface ModrinthHit {
  project_id: string;
  project_type: string;
  slug: string;
  title: string;
  description: string;
  author: string;
  downloads: number;
  follows: number;
  date_modified: string;
  date_created?: string;
  categories: string[];
  display_categories?: string[];
  icon_url: string | null;
  versions?: string[];
}

interface ModrinthSearchResponse {
  hits: ModrinthHit[];
  offset: number;
  limit: number;
  total_hits: number;
}

export interface ModrinthQuery {
  category: StoreCategory;
  query: string;
  /** 单个游戏版本过滤，如 1.21.4；空字符串表示全部。 */
  gameVersion?: string;
  sort: SortKey;
  direction: SortDirection;
  limit?: number;
  offset?: number;
  signal?: AbortSignal;
  fresh?: boolean;
}

function facetFor({ category, gameVersion }: Pick<ModrinthQuery, 'category' | 'gameVersion'>): string {
  const groups: string[][] = [];
  const projectType = projectTypeFor(category);
  if (projectType) groups.push([`project_type:${projectType}`]);
  if (gameVersion) groups.push([`versions:${gameVersion}`]);
  return JSON.stringify(groups);
}

export async function searchModrinth(options: ModrinthQuery): Promise<StorePage<StoreEntry>> {
  const { category, query, gameVersion, sort, direction, limit = 20, offset = 0, signal, fresh } = options;
  if (!projectTypeFor(category)) {
    throw new StoreError(
      'Modrinth',
      category === 'world'
        ? loc('store.noWorldModrinth')
        : loc('store.noVanillaModrinth'),
      category === 'world',
    );
  }

  const params = new URLSearchParams({
    query: query.trim(),
    limit: String(Math.min(100, Math.max(1, limit))),
    offset: String(Math.max(0, offset)),
    index: indexFor(sort),
  });
  const facets = facetFor({ category, gameVersion });
  if (facets !== '[]') params.set('facets', facets);

  const cacheKey = `modrinth:${params.toString()}`;
  let body: ModrinthSearchResponse;
  try {
    body = await getJson<ModrinthSearchResponse>(`${SEARCH_URL}?${params}`, { cacheKey, signal, fresh });
  } catch (error) {
    if (error instanceof DOMException && error.name === 'AbortError') throw error;
    if (error instanceof HttpError) {
      // 429 是 Modrinth 的限流，需要给用户明确的等待建议。
      if (error.status === 429) throw new StoreError('Modrinth', loc('store.modrinthRateLimited'));
      throw new StoreError('Modrinth', loc('store.modrinthHttp', { status: error.status }));
    }
    throw new StoreError('Modrinth', localizedTextOf(error) ?? loc('store.modrinthUnavailable'));
  }

  const entries = (body.hits ?? []).map((hit): StoreEntry => ({
    id: hit.project_id,
    source: 'Modrinth',
    category,
    name: hit.title,
    author: hit.author,
    summary: hit.description,
    downloads: hit.downloads,
    follows: hit.follows,
    modified: Date.parse(hit.date_modified) || undefined,
    released: hit.date_created ? Date.parse(hit.date_created) : undefined,
    iconUrl: hit.icon_url ?? undefined,
    url: `https://modrinth.com/${hit.project_type}/${hit.slug}`,
  }));

  // index=relevance 时方向无意义；其余情况让「降序/升序」对本地结果也生效。
  if (direction === 'asc' && indexFor(sort) === 'relevance') entries.reverse();

  return { entries, total: body.total_hits ?? entries.length };
}
