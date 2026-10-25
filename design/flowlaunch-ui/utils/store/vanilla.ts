// Mojang 官方版本清单。
//
// PCL2 的「原版游戏」列表来自 version_manifest_v2.json（ModMinecraft.vb
// 的 McVersionList 加载逻辑），再按类型分到「正式版 / 快照 / 远古 / 彩蛋」
// 四张卡片。这里做同样的事，但把 920 条压平成一个可筛选的列表。
//
// 彩蛋版的识别与文案照搬 PCL2 的 GetMcFoolName（ModMinecraft.vb 899-925 行），
// 不做改写——那些文案是 PCL 作者写的，改动会让两个启动器的同一版本显示不同
// 说法。

import type { StoreEntry, StorePage, StoreSource, VersionKind } from './types';
import type { FoolNoteKey } from './types';
import { StoreError } from './types';
import { vanillaJavaMajorPrecise } from './format';
import { getJson, HttpError } from './http';
import { loc, localizedTextOf } from '../i18n/localized';
import { translate, type Language } from '../i18n';

const MANIFEST_URL = 'https://launchermeta.mojang.com/mc/game/version_manifest_v2.json';
const SOURCE: StoreSource = 'Mojang';

interface ManifestVersion {
  id: string;
  type: 'release' | 'snapshot' | 'old_beta' | 'old_alpha';
  url: string;
  time: string;
  releaseTime: string;
  sha1: string;
  complianceLevel?: number;
}

export interface VersionManifest {
  latest: { release: string; snapshot: string };
  versions: ManifestVersion[];
}

/** PCL2 GetMcFoolName 的逐条移植。入参按 PCL 的做法转小写。
 *  返回词典键而不是文案：这几个彩蛋说明也要跟着界面语言切换。 */
export function foolNote(name: string): FoolNoteKey | null {
  const id = name.toLowerCase();
  if (id.startsWith('2.0')) return 'fool.2013';
  if (id === '15w14a') return 'fool.2015';
  if (id === '1.rv-pre1') return 'fool.2016';
  if (id === '3d shareware v1.34') return 'fool.2019';
  if (id.startsWith('20w14inf') || id === '20w14∞') return 'fool.2020';
  if (id === '22w13oneblockatatime') return 'fool.2022';
  if (id === '23w13a_or_b') return 'fool.2023';
  if (id === '24w14potato') return 'fool.2024';
  if (id === '25w14craftmine') return 'fool.2025';
  if (id === '26w14a') return 'fool.2026';
  return null;
}

/**
 * 版本种类判定。
 *
 * 彩蛋版优先：它们本身是 snapshot 或 release，但 PCL 把愚人节版本单独归类，
 * 因为它们的玩法和兼容性都和正常版本不同。
 */
export function classifyVersion(version: ManifestVersion): VersionKind {
  if (foolNote(version.id)) return 'special';
  if (version.type === 'release') return 'release';
  if (version.type === 'snapshot') return 'snapshot';
  return 'old';
}

function toEntry(version: ManifestVersion, latest: VersionManifest['latest']): StoreEntry {
  const kind = classifyVersion(version);
  const javaMajor = vanillaJavaMajorPrecise(version.id);
  const note = foolNote(version.id);
  return {
    id: version.id,
    source: SOURCE,
    category: 'vanilla',
    name: version.id,
    summary: undefined,
    modified: Date.parse(version.releaseTime) || Date.parse(version.time) || undefined,
    released: Date.parse(version.releaseTime) || undefined,
    url: `https://www.minecraft.net/en-us/article/minecraft-java-edition-${version.id}`,
    version: {
      kind,
      latest: version.id === latest.release || version.id === latest.snapshot,
      foolNote: note ?? undefined,
      javaMajor: javaMajor.major,
    },
  };
}

/** 拉取完整清单。返回的条目按时间从新到旧。 */
export async function fetchManifest(signal?: AbortSignal, fresh = false): Promise<{ manifest: VersionManifest; entries: StoreEntry[] }> {
  let manifest: VersionManifest;
  try {
    manifest = await getJson<VersionManifest>(MANIFEST_URL, { cacheKey: 'mojang-manifest', signal, fresh });
  } catch (error) {
    if (error instanceof DOMException && error.name === 'AbortError') throw error;
    if (error instanceof HttpError) {
      throw new StoreError(SOURCE, loc('store.manifestHttp', { status: error.status }));
    }
    throw new StoreError(SOURCE, localizedTextOf(error) ?? loc('store.manifestFailed'));
  }
  if (!Array.isArray(manifest?.versions)) {
    throw new StoreError(SOURCE, loc('store.manifestMalformed'));
  }
  const entries = manifest.versions.map(version => toEntry(version, manifest.latest));
  entries.sort((a, b) => (b.modified ?? 0) - (a.modified ?? 0));
  return { manifest, entries };
}

export interface VanillaQuery {
  /** 勾选的版本种类；空数组表示什么都不要。 */
  kinds: VersionKind[];
  /** 名称过滤（输入框）。 */
  query?: string;
  /** 当前界面语言，用于让彩蛋说明也能被搜索到。 */
  language?: Language;
  /** 只保留某个正式版兼容的版本？原版列表不使用，预留给筛选扩展。 */
  limit?: number;
}

/** 本地过滤（清单是一次性拉下来的，不需要分页）。 */
export function queryVersions(entries: StoreEntry[], { kinds, query, language = 'zh-CN', limit }: VanillaQuery): StorePage<StoreEntry> {
  const needle = query?.trim().toLowerCase() ?? '';
  const filtered = entries.filter(entry => {
    const kind = entry.version?.kind;
    if (!kind || !kinds.includes(kind)) return false;
    if (!needle) return true;
    const note = entry.version?.foolNote ? translate(language, entry.version.foolNote) : '';
    return `${entry.name} ${note}`.toLowerCase().includes(needle);
  });
  return { entries: limit ? filtered.slice(0, limit) : filtered, total: filtered.length };
}

/** 取前若干正式版，供「游戏版本」下拉使用（去重、只取 release、最多 12 项）。 */
export function releaseOptions(entries: StoreEntry[], count = 12): string[] {
  const seen = new Set<string>();
  const result: string[] = [];
  for (const entry of entries) {
    if (entry.version?.kind !== 'release') continue;
    if (seen.has(entry.name)) continue;
    seen.add(entry.name);
    result.push(entry.name);
    if (result.length >= count) break;
  }
  return result;
}
