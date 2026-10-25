// 可本地化的文本片段的统一表示。
//
// 工具层（Java 需求分析、设备内存探测、商店数据源）需要产出用户可见的文案，
// 但它们不该依赖 React，也不该在生成时就把语言定死——否则切换语言后这些
// 缓存过的字符串会停留在旧语言。
//
// 所以工具层返回「键 + 参数」，由界面层用当前语言渲染。只有在拿不到键的
// 极端情况下（例如底层异常原文）才用 raw 直接透传。

import type { MessageKey } from './messages/zh-CN';
import { translate, type Language } from './index';

export interface LocalizedText {
  key: MessageKey;
  values?: Record<string, string | number>;
  /** 无法用键表达的原文（异常信息等）。优先级低于 key。 */
  raw?: string;
}

export type Localized = LocalizedText | string;

export function loc(key: MessageKey, values?: Record<string, string | number>): LocalizedText {
  return values ? { key, values } : { key };
}

/** 包装未知异常：优先保留原始信息，用给定键作为前缀说明。 */
export function locFromError(prefix: MessageKey, error: unknown, values?: Record<string, string | number>): LocalizedText {
  const detail = error instanceof Error ? error.message : String(error);
  return { key: prefix, values: { ...values, error: detail } };
}

/**
 * 带本地化文案的异常。
 *
 * 数据源（HTTP、Modrinth、CurseForge、Mojang 清单）在抛错时还不知道用户选的
 * 是哪种语言，所以它们抛的是「键 + 参数」，由界面层用当前语言渲染。这样切换
 * 语言后重新搜索，错误提示也是新语言，而不是留在抛错那一刻的语言。
 */
export class LocalizedError extends Error {
  readonly text: LocalizedText;

  constructor(text: LocalizedText) {
    super(text.key);
    this.name = 'LocalizedError';
    this.text = text;
  }
}

/** 判断任意异常是否携带本地化文案。 */
export function hasLocalizedText(error: unknown): error is { text: LocalizedText } {
  if (!error || typeof error !== 'object') return false;
  const text = (error as { text?: unknown }).text;
  return Boolean(text && typeof text === 'object' && 'key' in (text as Record<string, unknown>));
}

/** 取异常里的本地化文案；拿不到时返回 null，由调用方决定兜底键。 */
export function localizedTextOf(error: unknown): LocalizedText | null {
  return hasLocalizedText(error) ? error.text : null;
}

export function renderLocalized(language: Language, text: Localized | undefined): string {
  if (text === undefined) return '';
  if (typeof text === 'string') return text;
  if (text.key) {
    const merged = text.raw ? { ...text.values, raw: text.raw } : text.values;
    // 工具层习惯用 loc(key, { count }) 传数量。这里同时把它当作复数选择器，
    // 否则英文的 `one|many` 会原样带着分隔符显示出来（"1 hour ago|1 hours ago"）。
    const count = typeof merged?.count === 'number' ? merged.count : undefined;
    return translate(language, text.key, { values: merged, count });
  }
  return text.raw ?? '';
}

/** 拼接一组片段，用语言对应的分隔符。 */
export function joinLocalized(language: Language, parts: Localized[], separatorKey: MessageKey): string {
  return parts
    .map(part => renderLocalized(language, part))
    .filter(Boolean)
    .join(renderLocalized(language, loc(separatorKey)));
}
