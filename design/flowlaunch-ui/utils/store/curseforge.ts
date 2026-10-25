// CurseForge 搜索。
//
// 官方 API（api.curseforge.com/v1/mods/search）必须带 x-api-key。启动器自己
// 申请到的 key 不能随客户端分发（等同于公开凭据），所以这里请用户自己在
// CurseForge for Studios 控制台申请并粘贴进来，只留在本机 localStorage。
//
// 与 PCL2 的差异：PCL 把这些第三方 API 的 key 编译进客户端（见
// ModComp/ResourceProject 里的 ApiKey），这里的做法更保守，代价是首次使用
// 需要用户自己申请一个免费 key。
//
// classId 对照（官方 Categories 文档）：
//   6    = Mods
//   4471 = Modpacks
//   12   = Resource Packs
//   17   = Worlds

import type { SortDirection, SortKey, StoreCategory, StoreEntry, StorePage } from './types';
import { StoreError } from './types';
import { getJson, HttpError } from './http';
import { loc, localizedTextOf } from '../i18n/localized';

const SEARCH_URL = 'https://api.curseforge.com/v1/mods/search';
const GAME_ID = 432; // Minecraft
export const API_KEY_STORAGE = 'flowlaunch-curseforge-key';

export const CLASS_IDS: Record<Exclude<StoreCategory, 'vanilla'>, number> = {
  mod: 6,
  modpack: 4471,
  resourcepack: 12,
  world: 17,
};

/**
 * SortKey → sortField。
 * 官方枚举：1=Featured 2=Popularity 3=LastUpdated 4=Name 5=Author
 * 6=TotalDownloads 7=Category 8=GameVersion 9=EarlyAccess 10=FeaturedReleased
 * 11=ReleasedDate 12=Rating。
 */
export function sortFieldFor(sort: SortKey): number {
  switch (sort) {
    case 'popular': return 2;
    case 'downloads': return 6;
    // CurseForge 没有「关注数」，用 Featured（编辑推荐）作为最接近的替代，
    // 同时 UI 里会把 follows 这个键在该源下隐藏。
    case 'follows': return 1;
    case 'newest': return 11;
    case 'updated': return 3;
    case 'name': return 4;
  }
}

interface CurseForgeMod {
  id: number;
  name: string;
  slug?: string;
  summary?: string;
  downloadCount?: number;
  dateModified?: string;
  dateCreated?: string;
  dateReleased?: string;
  logo?: { thumbnailUrl?: string; url?: string };
  links?: { websiteUrl?: string };
  authors?: { name?: string }[];
  categories?: { name?: string }[];
}

interface CurseForgeResponse {
  data: CurseForgeMod[];
  pagination?: { index: number; pageSize: number; resultCount: number; totalCount: number };
}

export function readCurseForgeKey(): string {
  try { return (localStorage.getItem(API_KEY_STORAGE) ?? '').trim(); } catch { return ''; }
}

export function saveCurseForgeKey(key: string): void {
  try {
    const trimmed = key.trim();
    if (trimmed) localStorage.setItem(API_KEY_STORAGE, trimmed);
    else localStorage.removeItem(API_KEY_STORAGE);
  } catch { /* storage blocked */ }
}

export interface CurseForgeQuery {
  category: StoreCategory;
  query: string;
  gameVersion?: string;
  sort: SortKey;
  direction: SortDirection;
  limit?: number;
  offset?: number;
  signal?: AbortSignal;
  fresh?: boolean;
}

export async function searchCurseForge(options: CurseForgeQuery): Promise<StorePage<StoreEntry>> {
  const { category, query, gameVersion, sort, direction, limit = 20, offset = 0, signal, fresh } = options;
  if (category === 'vanilla') {
    throw new StoreError('CurseForge', loc('store.noVanillaCurseforge'));
  }
  const key = readCurseForgeKey();
  if (!key) {
    throw new StoreError('CurseForge', loc('store.curseforgeNeedsKey'), true);
  }

  const params = new URLSearchParams({
    gameId: String(GAME_ID),
    classId: String(CLASS_IDS[category]),
    index: String(Math.max(0, offset)),
    // 官方上限 50；index + pageSize 不能超过 10000。
    pageSize: String(Math.min(50, Math.max(1, limit))),
    sortField: String(sortFieldFor(sort)),
    sortOrder: direction === 'asc' ? 'asc' : 'desc',
  });
  if (query.trim()) params.set('searchFilter', query.trim());
  if (gameVersion) params.set('gameVersion', gameVersion);

  // 缓存键不含 key 本身，避免把凭据写进 storage。
  const cacheKey = `curseforge:${params.toString()}`;
  let body: CurseForgeResponse;
  try {
    body = await getJson<CurseForgeResponse>(`${SEARCH_URL}?${params}`, {
      cacheKey,
      signal,
      fresh,
      headers: { 'x-api-key': key, Accept: 'application/json' },
    });
  } catch (error) {
    if (error instanceof DOMException && error.name === 'AbortError') throw error;
    if (error instanceof HttpError) {
      if (error.status === 403 || error.status === 401) {
        throw new StoreError('CurseForge', loc('store.curseforgeBadKey'), true);
      }
      if (error.status === 429) throw new StoreError('CurseForge', loc('store.curseforgeRateLimited'));
      throw new StoreError('CurseForge', loc('store.curseforgeHttp', { status: error.status }));
    }
    throw new StoreError('CurseForge', localizedTextOf(error) ?? loc('store.curseforgeUnavailable'));
  }

  const entries = (body.data ?? []).map((mod): StoreEntry => ({
    id: String(mod.id),
    source: 'CurseForge',
    category,
    name: mod.name,
    author: mod.authors?.[0]?.name,
    summary: mod.summary,
    downloads: mod.downloadCount,
    // CurseForge 不返回关注数；留 undefined，排序与 UI 都会跳过。
    follows: undefined,
    modified: mod.dateModified ? Date.parse(mod.dateModified) : undefined,
    released: mod.dateReleased ? Date.parse(mod.dateReleased) : mod.dateCreated ? Date.parse(mod.dateCreated) : undefined,
    iconUrl: mod.logo?.thumbnailUrl ?? mod.logo?.url,
    url: mod.links?.websiteUrl ?? (mod.slug ? `https://www.curseforge.com/minecraft/${categoryPath(category)}/${mod.slug}` : undefined),
  }));

  return { entries, total: body.pagination?.totalCount ?? entries.length };
}

/** CurseForge 网站路径片段。 */
function categoryPath(category: StoreCategory): string {
  switch (category) {
    case 'mod': return 'mc-mods';
    case 'modpack': return 'modpacks';
    case 'resourcepack': return 'texture-packs';
    case 'world': return 'worlds';
    default: return 'mc-mods';
  }
}
