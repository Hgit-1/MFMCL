// 国际化内核（简体中文 / English / 日本語）。
//
// 为什么不用 i18next 之类的库：这个项目是一条零依赖的原型，只需要「按 key 取
// 字符串 + 插值 + 复数列 + 运行时切换」这几件事，几百行足够，也避免为一个
// 原型引入几十 KB 的运行时。
//
// 设计要点：
//
// 1. 词典按语言拆文件，key 用扁平的点号路径（`settings.game.memory`）。
//    类型由 zh-CN 推导，所以漏翻或写错 key 在编译期就会报错，而不是运行时
//    静默回落到中文。
// 2. 插值用 `{name}`，只在缺失时抛错；没被替换的占位符以原文显示，便于发现。
// 3. 复数用 `|` 分隔（`one|many`），按语言的复数规则选；中/日只有一种形式。
// 4. 语言优先级：用户显式选择 > localStorage > 浏览器语言 > 中文。
//    选择「跟随系统」时不落盘，跟随 navigator.language。
// 5. 日期/数字格式化统一走 Intl，并把 BCP47 标签放在这里，避免各处硬编码
//    `zh-CN`。

import { createContext, useContext } from 'react';
import { en } from './messages/en';
import { ja } from './messages/ja';
import { zhCN, type MessageKey } from './messages/zh-CN';

export type Language = 'zh-CN' | 'en' | 'ja';
/** 用户在设置里可选的值：前三种是具体语言，system 表示跟随浏览器。 */
export type LanguagePreference = Language | 'system';

/**
 * 可选语言列表。只放 id 和该语言的自称（native），显示文案由界面的
 * `language.*` 词典键提供，这样「跟随系统」也能按当前语言显示。
 */
export const LANGUAGES: { id: LanguagePreference; native: string }[] = [
  { id: 'system', native: 'System' },
  { id: 'zh-CN', native: '简体中文' },
  { id: 'en', native: 'English' },
  { id: 'ja', native: '日本語' },
];

/** 词典形状由中文词典推导，三种语言必须 key 完全一致。 */
export type Dictionary = Record<MessageKey, string>;

export const DICTIONARIES: Record<Language, Dictionary> = {
  'zh-CN': zhCN,
  en,
  ja,
};

export const LANGUAGE_STORAGE_KEY = 'flowlaunch-language';

/** 存进 <html lang> 与 Intl 的标签。 */
export const LOCALES: Record<Language, string> = {
  'zh-CN': 'zh-CN',
  en: 'en-US',
  ja: 'ja-JP',
};

/** 把浏览器语言归一到我们支持的三选一。 */
export function normalizeLanguage(tag: string | undefined): Language {
  const value = (tag ?? '').toLowerCase();
  if (value.startsWith('ja')) return 'ja';
  if (value.startsWith('zh')) return 'zh-CN';
  if (value.startsWith('en')) return 'en';
  return 'zh-CN';
}

export function readLanguagePreference(): LanguagePreference {
  try {
    const raw = localStorage.getItem(LANGUAGE_STORAGE_KEY);
    if (raw === 'system' || raw === 'zh-CN' || raw === 'en' || raw === 'ja') return raw;
  } catch { /* storage blocked */ }
  return 'system';
}

export function resolveLanguage(preference: LanguagePreference): Language {
  if (preference !== 'system') return preference;
  const languages = typeof navigator === 'undefined' ? [] : navigator.languages ?? [navigator.language];
  // 按浏览器偏好顺序找第一个能识别的语言。
  for (const tag of languages) {
    if (!tag) continue;
    const lowered = tag.toLowerCase();
    if (lowered.startsWith('ja') || lowered.startsWith('zh') || lowered.startsWith('en')) return normalizeLanguage(tag);
  }
  return 'zh-CN';
}

export interface TranslateOptions {
  /** 插值变量。 */
  values?: Record<string, string | number>;
  /** 复数选择用的数量。 */
  count?: number;
}

/**
 * 取一条文案。
 *
 * `key` 在类型上是 MessageKey，但如果调用方从外部数据（比如 API 返回的错误码）
 * 拼出来，仍可能拿到不存在的 key。这时开发环境打警告并回落到中文，生产环境
 * 直接显示 key，绝不因为缺一条翻译而崩溃。
 */
export function translate(
  language: Language,
  key: MessageKey,
  options: TranslateOptions = {},
): string {
  const dict = DICTIONARIES[language] ?? zhCN;
  const raw = dict[key] ?? zhCN[key];
  if (raw === undefined) {
    if (typeof console !== 'undefined') console.warn(`[i18n] missing message: ${key}`);
    return key;
  }
  let text = raw;
  if (options.count !== undefined) text = pickPlural(text, options.count, language);
  // count 既是复数选择器，也是一个可用占位符：词典里的 `{count} 个版本`
  // 只传 count 就能同时完成选形和插值，不必让每个调用方再写一遍 values。
  if (options.count !== undefined) text = text.split('{count}').join(String(options.count));
  if (options.values) {
    for (const [name, value] of Object.entries(options.values)) {
      text = text.split(`{${name}}`).join(String(value));
    }
  }
  return text;
}

/**
 * 复数列：`one|many`。
 * 中/日只有一种形式，直接取第一个；英文按 1 与其余区分。
 */
function pickPlural(text: string, count: number, language: Language): string {
  if (!text.includes('|')) return text;
  const forms = text.split('|');
  if (language === 'en') return count === 1 ? forms[0] : forms[1] ?? forms[0];
  return forms[0];
}

// ---------------------------------------------------------------------------
// React 绑定
// ---------------------------------------------------------------------------

export interface I18nValue {
  language: Language;
  preference: LanguagePreference;
  /** 带类型的取词函数：t('settings.game.memory')。 */
  t: (key: MessageKey, options?: TranslateOptions) => string;
  setLanguage: (preference: LanguagePreference) => void;
}

export const I18nContext = createContext<I18nValue | null>(null);

export function useI18n(): I18nValue {
  const value = useContext(I18nContext);
  if (!value) throw new Error('useI18n 必须在 <I18nProvider> 内使用');
  return value;
}

/** 简写：只要取词函数时用这个，读起来比 useI18n().t 短。 */
export function useT() {
  return useI18n().t;
}
