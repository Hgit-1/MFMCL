// 格式化与 Java 需求估算。
//
// 中文单位沿用 PCL2 的习惯：万 / 亿，而不是 K/M。PCL 的下载量显示在
// ModComp 等地方用的是「1.2 万」这种写法，这里保持一致，避免出现
// 「2.8e7 次下载」这类不可读的数字。

import type { VersionKind } from './types';
import { loc, type Localized } from '../i18n/localized';
import type { Language } from '../i18n';
import { translate } from '../i18n';

/**
 * 大数字转可读文案。
 *
 * 中文用「万 / 亿」，英文和日文用 K / M / B——把「1234.6 万」直接塞进英文界面
 * 是不可读的。locale 决定分组符号与单位。
 */
export function formatCount(value: number | undefined, language: Language = 'zh-CN'): string {
  if (value === undefined || !Number.isFinite(value) || value < 0) return '—';
  const locale = language === 'zh-CN' ? 'zh-CN' : language === 'ja' ? 'ja-JP' : 'en-US';
  if (value < 10_000) return value.toLocaleString(locale);
  const trim = (n: number) => n.toFixed(1).replace(/\.0$/, '');
  if (language === 'zh-CN') {
    if (value < 100_000_000) return translate(language, 'count.wan', { values: { value: trim(value / 10_000) } });
    return translate(language, 'count.yi', { values: { value: trim(value / 100_000_000) } });
  }
  if (language === 'ja') {
    if (value < 100_000_000) return `${trim(value / 10_000)}万`;
    return `${trim(value / 100_000_000)}億`;
  }
  if (value < 1_000_000) return `${trim(value / 1_000)}K`;
  if (value < 1_000_000_000) return `${trim(value / 1_000_000)}M`;
  return `${trim(value / 1_000_000_000)}B`;
}

/** 基于真实时间戳的相对时间；返回片段以便按语言渲染与复数化。 */
export function formatRelative(timestamp: number | undefined, now = Date.now()): Localized {
  if (!timestamp || !Number.isFinite(timestamp)) return loc('time.unknown');
  const diff = now - timestamp;
  if (diff < 0) return formatDate(timestamp);
  const minute = 60_000;
  const hour = 60 * minute;
  const day = 24 * hour;
  if (diff < minute) return loc('time.justNow');
  if (diff < hour) return loc('time.minutesAgo', { count: Math.floor(diff / minute) });
  if (diff < day) return loc('time.hoursAgo', { count: Math.floor(diff / hour) });
  if (diff < 30 * day) return loc('time.daysAgo', { count: Math.floor(diff / day) });
  if (diff < 365 * day) return loc('time.monthsAgo', { count: Math.floor(diff / (30 * day)) });
  return formatDate(timestamp);
}

export function formatDate(timestamp: number | undefined, language: Language = 'zh-CN'): string {
  if (!timestamp || !Number.isFinite(timestamp)) return '—';
  const locale = language === 'zh-CN' ? 'zh-CN' : language === 'ja' ? 'ja-JP' : 'en-US';
  return new Date(timestamp).toLocaleDateString(locale, { year: 'numeric', month: '2-digit', day: '2-digit' });
}

export const VERSION_KIND_KEYS: Record<VersionKind, `versionKind.${VersionKind}`> = {
  release: 'versionKind.release',
  snapshot: 'versionKind.snapshot',
  old: 'versionKind.old',
  special: 'versionKind.special',
};

/**
 * 界面支持的四类版本，按筛选器的展示顺序。
 *
 * 放在这里而不是某个组件内部：内容商店的原版列表和新建实例对话框都要用同一套
 * 筛选器，两边各写一份迟早会出现「商店能筛彩蛋版、创建对话框不能」这类偏差。
 */
export const VERSION_KINDS: VersionKind[] = ['release', 'snapshot', 'old', 'special'];

/**
 * Mojang 清单的版本类型 → 界面四分类。
 *
 * 关键难点：**Mojang 的清单里没有「彩蛋版」这个类型**。它只有 release / snapshot /
 * old_beta / old_alpha 四种，愚人节版本全部标记为 `snapshot`（实测确认：15w14a、
 * 24w14potato、25w14craftmine 等 8 个已知愚人节版本的 type 都是 snapshot）。
 *
 * 所以「彩蛋版」必须按版本号识别，不能靠 type 字段。之前的实现把未知 type 归到
 * special，而真实数据里没有未知 type，导致这个筛选永远为空——界面提供了一个
 * 筛不出任何内容的选项。
 *
 * `old_beta` 与 `old_alpha` 都归入「远古版」：用户要区分的是「这是不是上古版本」，
 * 而不是 beta 还是 alpha。
 */
export function kindOfVersion(id: string, type: string): VersionKind {
  if (APRIL_FOOLS_VERSIONS.has(id)) return 'special';
  switch (type) {
    case 'release': return 'release';
    case 'snapshot': return 'snapshot';
    case 'old_beta':
    case 'old_alpha': return 'old';
    // 未来若 Mojang 新增类型，落到 special 而不是被静默丢弃。
    default: return 'special';
  }
}

/**
 * 愚人节（彩蛋）版本号。
 *
 * 来源：Mojang 清单里 type 全是 snapshot，只能硬编码识别。列表参照 PCL2 的
 * `GetMcFoolName` 覆盖范围，并补上清单里实际存在的条目（已逐个核对存在于
 * version_manifest_v2.json）。名字不是 `NNwNNa` 形态的（如 `3D Shareware v1.34`）
 * 都是愚人节版本，这也是它们最容易辨认的特征。
 */
const APRIL_FOOLS_VERSIONS = new Set([
  '2.0',
  '15w14a',
  '1.RV-Pre1',
  '3D Shareware v1.34',
  '20w14infinite',
  '22w13oneblockatatime',
  '23w13a_or_b',
  '24w14potato',
  '25w14craftmine',
]);

/**
 * 原版版本需要的 Java 主版本（估算）。
 *
 * 阈值照 PCL2 的 GetJavaRequirement：1.20.5+ → 21，1.18+ → 17，1.17+ → 16，
 * 其余 8。快照版没有官方公布的数字，这里按年份粗判：
 *   26.x / 25.x / 24w+ / 23w+ / 22w+ / 21w+ / 20w45a+ → 21（当前开发线）
 *   20w 早期及更早的快照按 16 / 8 处理。
 * 明确只做估算——拉每个版本 JSON 才能拿到准确值，而 920 个版本显然不能逐个
 * 请求，所以界面上这一列标注为「估算」。
 */
export function vanillaJavaMajor(versionId: string): number {
  const id = versionId.toLowerCase();

  // 新式日期版本号（26.3、26.4-snapshot-5）——2025 年后 Mojang 改用这种格式。
  const calendar = id.match(/^(\d{2})\.(\d+)/);
  if (calendar && !/^1\./.test(id)) return 21;

  // 快照：YYwWWx。
  const snapshot = id.match(/^(\d{2})w(\d{2})/);
  if (snapshot) {
    const year = Number(snapshot[1]);
    const week = Number(snapshot[2]);
    if (year >= 20) {
      // 20w45a 是 1.17 的第一个快照（Java 16）；21w 起回到 17、再往后 21。
      if (year === 20) return week >= 45 ? 16 : 8;
      if (year === 21) return 17;
      return 21;
    }
    return 8;
  }

  // 正式版：只取主.次。
  const release = id.match(/^(\d+)\.(\d+)/);
  if (!release) return 8;
  const major = Number(release[1]);
  const minor = Number(release[2]);
  if (major !== 1) return 8;
  // 1.20 跨了 Java 17 → 21 的界线（1.20.5 起），精确值交给
  // vanillaJavaMajorPrecise；这个粗粒度版本按多数 1.20.x 用的 17 返回。
  if (minor > 20) return 21;
  if (minor === 20) return 17;
  if (minor >= 18) return 17;
  if (minor === 17) return 16;
  return 8;
}

/** 1.20 系列细分为 1.20.5+ → 21。上面为了可读性先按整数段返回，这里再修正。 */
export function vanillaJavaMajorPrecise(versionId: string): { major: number; estimated: boolean } {
  const id = versionId.toLowerCase();
  const match = id.match(/^1\.(\d+)(?:\.(\d+))?$/);
  if (match) {
    const minor = Number(match[1]);
    const patch = Number(match[2] ?? 0);
    if (minor > 20) return { major: 21, estimated: false };
    if (minor === 20) return { major: patch >= 5 ? 21 : 17, estimated: false };
    if (minor >= 18) return { major: 17, estimated: false };
    if (minor === 17) return { major: 16, estimated: false };
    return { major: 8, estimated: false };
  }
  // 快照与彩蛋版没有官方值，一律标记为估算。
  return { major: vanillaJavaMajor(versionId), estimated: true };
}
