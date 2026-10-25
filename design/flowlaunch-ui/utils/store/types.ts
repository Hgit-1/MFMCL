// 内容商店的共享类型。
//
// 三个来源的数据形状差别很大：
//   - Mojang 清单只有版本号、类型和时间，没有作者、下载量；
//   - Modrinth 有 figures（downloads/follows），但没有「地图」这一分类；
//   - CurseForge 有 classId 区分四类，但缺少 follows。
//
// 所以这里定义一个「够用就好」的公共条目：各源只填自己拿得到的字段，缺失
// 就用 undefined，排序和渲染都不假设字段一定存在。

import type { LocalizedText } from '../i18n/localized';
import type { MessageKey } from '../i18n/messages/zh-CN';

export type StoreCategory = 'vanilla' | 'mod' | 'modpack' | 'resourcepack' | 'world';

/** 仅原版分类使用的版本种类。 */
export type VersionKind = 'release' | 'snapshot' | 'old' | 'special';

export type StoreSource = 'Modrinth' | 'CurseForge' | 'Mojang';

/** 愚人节版本说明的词典键。 */
export type FoolNoteKey = Extract<MessageKey, `fool.${string}`>;

export interface StoreEntry {
  /** 源内唯一 ID（Modrinth project_id / CurseForge id / 版本号）。 */
  id: string;
  source: StoreSource;
  category: StoreCategory;
  name: string;
  author?: string;
  summary?: string;
  /** 下载量。原版条目没有这个字段。 */
  downloads?: number;
  /** Modrinth 专有的关注数。 */
  follows?: number;
  /** 最近更新时间（毫秒时间戳）。原版用 releaseTime。 */
  modified?: number;
  /** 首次发布时间（毫秒时间戳），用于「最新」排序。 */
  released?: number;
  iconUrl?: string;
  url?: string;
  /** 原版条目：版本种类、是否最新、愚人节文案。 */
  version?: {
    kind: VersionKind;
    latest?: boolean;
    /** PCL2 GetMcFoolName 的彩蛋说明，这里存词典键以便随语言切换。 */
    foolNote?: FoolNoteKey;
    /** 估算的 Java 主版本，见 utils/store/format.ts。 */
    javaMajor: number;
  };
}

export interface StorePage<T> {
  entries: T[];
  total: number;
}

/**
 * 一次请求的可读错误。
 *
 * 文案以「键 + 参数」保存（见 i18n/localized.ts），由界面层按当前语言渲染；
 * `message` 只是键名，仅供控制台定位，不直接展示给用户。
 */
export class StoreError extends Error {
  readonly source: StoreSource;
  readonly text: LocalizedText;
  /** 需要用户干预才能继续的错误（如缺少 API Key）。 */
  readonly actionable: boolean;

  constructor(source: StoreSource, text: LocalizedText, actionable = false) {
    super(text.key);
    this.name = 'StoreError';
    this.source = source;
    this.text = text;
    this.actionable = actionable;
  }
}

/** 分类标签的词典键，界面层用 t(CATEGORY_KEYS[category]) 渲染。 */
export const CATEGORY_KEYS: Record<StoreCategory, `category.${StoreCategory}`> = {
  vanilla: 'category.vanilla',
  mod: 'category.mod',
  modpack: 'category.modpack',
  resourcepack: 'category.resourcepack',
  world: 'category.world',
};

export const SOURCE_LABELS: Record<StoreSource, string> = {
  Modrinth: 'Modrinth',
  CurseForge: 'CurseForge',
  Mojang: 'Mojang',
};

/** 排序键。原版分类只用 newest / oldest / name，其余键在 UI 里隐藏。 */
export type SortKey = 'popular' | 'downloads' | 'follows' | 'newest' | 'updated' | 'name';
export type SortDirection = 'asc' | 'desc';
