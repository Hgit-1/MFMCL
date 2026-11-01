// Java runtime model and selection rules.
//
// The behaviour here is a port of PCL2's Java handling (见
// PCL/Plain Craft Launcher 2/Modules/Minecraft/ModJava.vb and
// PCL/Plain Craft Launcher 2/Pages/PageInstance/PageInstanceSetup.xaml.vb):
//
//   Global settings keep one ordered *list* of detected Java runtimes. At launch
//   the launcher walks that list from top to bottom and uses the first runtime
//   that satisfies the instance's requirement, so priority is expressed by order
//   (PCL: UpdateJavaList + MoveJavaInList).
//
//   Every instance then chooses *how* its Java is found:
//     auto   - derive the required Java range from the game version and loaders,
//              then take the first compatible entry of the global list.
//     range  - same, but the user types the acceptable version range
//              (PCL: "自动选择指定版本的 Java" + VersionArgumentJavaRange).
//     folder - look for a Java shipped inside the instance folder
//              (PCL: "使用版本文件夹中的 Java").
//     forced - use one specific runtime from the list
//              (PCL: "使用指定的 Java" + Configs.JavaForced).
//
// The requirement analysis below follows PCL's GetJavaRequirement, simplified to
// the vanilla and loader rules that the launcher can evaluate without reading
// the version JSON.

import type { GameInstance } from '../types';
import { loc, renderLocalized, type Localized } from './i18n/localized';
import { translate, type Language } from './i18n';
import { PREVIEW_DATA } from './preview';

export interface JavaRuntime {
  path: string;
  majorVersion: number;
  versionText: string;
}

export const JAVA_LIST_KEY = 'flowlaunch-java-runtimes';
export const INSTANCE_JAVA_KEY = 'flowlaunch-instance-java';

/**
 * 开发预览用的示例 Java 运行时。
 *
 * 为什么整个函数体由 `PREVIEW_DATA` 包起来而不是让调用方判断：常量折叠是逐语句
 * 做的，`if (!PREVIEW_DATA) return []` 之后仍然存在一条「会调用本函数」的路径，
 * 打包器因此必须保留函数体（连同这些路径字符串）。把判断放进函数内部，生产
 * 构建里函数体变成 `if (false) { ... }`，内部的对象字面量就没有任何引用者，
 * 才能被真正删除。
 */
export function previewJavaRuntimes(): JavaRuntime[] {
  if (!PREVIEW_DATA) return [];
  return [
    { path: '/usr/lib/jvm/java-21-openjdk/bin/java', majorVersion: 21, versionText: 'openjdk version "21.0.5"' },
    { path: '/usr/lib/jvm/java-17-openjdk/bin/java', majorVersion: 17, versionText: 'openjdk version "17.0.13"' },
    { path: '/usr/lib/jvm/temurin-8/bin/java', majorVersion: 8, versionText: 'openjdk version "1.8.0_432"' },
  ];
}

function isRuntime(value: unknown): value is JavaRuntime {
  if (!value || typeof value !== 'object') return false;
  const runtime = value as Partial<JavaRuntime>;
  return typeof runtime.path === 'string'
    && runtime.path.length > 0
    && typeof runtime.majorVersion === 'number'
    && Number.isFinite(runtime.majorVersion);
}

/** Load the global Java list. Dev preview seeds the sample list; production starts empty. */
export function loadJavaList(): JavaRuntime[] {
  try {
    const raw = localStorage.getItem(JAVA_LIST_KEY);
    if (raw === null) {
      // 生产构建里 PREVIEW_DATA 是编译期 false，这段播种逻辑与示例数据一起被移除，
      // 首次启动得到空列表，由后端 `list_java_runtimes` 填真实结果。
      if (!PREVIEW_DATA) return [];
      const preview = previewJavaRuntimes();
      saveJavaList(preview);
      return preview;
    }
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.filter(isRuntime) : [];
  } catch {
    return PREVIEW_DATA ? previewJavaRuntimes() : [];
  }
}

export function saveJavaList(list: JavaRuntime[]): void {
  try { localStorage.setItem(JAVA_LIST_KEY, JSON.stringify(list)); } catch { /* storage full or blocked */ }
}

export type InstanceJavaMode = 'auto' | 'range' | 'folder' | 'forced';

export interface InstanceJavaSetting {
  mode: InstanceJavaMode;
  /** Version range used by `range` mode, e.g. `[21.0, )`. */
  range: string;
  /** Folder searched by `folder` mode. */
  folder: string;
  /** Runtime path used by `forced` mode. */
  path: string;
}

export const DEFAULT_INSTANCE_JAVA: InstanceJavaSetting = { mode: 'auto', range: '', folder: '', path: '' };

/**
 * Accept both the current record format and the legacy shape where the stored
 * value was just a path string (empty string meant "follow global").
 */
export function normalizeInstanceJavaSetting(raw: unknown): InstanceJavaSetting {
  if (typeof raw === 'string') {
    return { ...DEFAULT_INSTANCE_JAVA, mode: raw ? 'forced' : 'auto', path: raw };
  }
  if (raw && typeof raw === 'object') {
    const value = raw as Partial<InstanceJavaSetting>;
    const mode: InstanceJavaMode = value.mode === 'range' || value.mode === 'folder' || value.mode === 'forced' ? value.mode : 'auto';
    return {
      mode,
      range: typeof value.range === 'string' ? value.range : '',
      folder: typeof value.folder === 'string' ? value.folder : '',
      path: typeof value.path === 'string' ? value.path : '',
    };
  }
  return { ...DEFAULT_INSTANCE_JAVA };
}

export function loadInstanceJavaSettings(): Record<string, InstanceJavaSetting> {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(INSTANCE_JAVA_KEY) || '{}');
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return {};
    return Object.fromEntries(
      Object.entries(parsed as Record<string, unknown>).map(([id, value]) => [id, normalizeInstanceJavaSetting(value)]),
    );
  } catch {
    return {};
  }
}

export function saveInstanceJavaSettings(settings: Record<string, InstanceJavaSetting>): void {
  try { localStorage.setItem(INSTANCE_JAVA_KEY, JSON.stringify(settings)); } catch { /* storage full or blocked */ }
}

// ---------------------------------------------------------------------------
// Versions and ranges
// ---------------------------------------------------------------------------

export interface JavaVersion { major: number; minor: number; patch: number }

export interface JavaRange {
  min: JavaVersion | null;
  minInclusive: boolean;
  max: JavaVersion | null;
  maxInclusive: boolean;
}

function compareVersion(left: JavaVersion, right: JavaVersion): number {
  if (left.major !== right.major) return left.major - right.major;
  if (left.minor !== right.minor) return left.minor - right.minor;
  return left.patch - right.patch;
}

function toVersion(text: string): JavaVersion | null {
  const parts = text.trim().split('.');
  if (parts.length === 0 || parts.length > 3) return null;
  if (parts.some(part => !/^\d+$/.test(part))) return null;
  return { major: Number(parts[0]), minor: Number(parts[1] ?? 0), patch: Number(parts[2] ?? 0) };
}

/**
 * 把用户随手写的版本范围解析成 `JavaRange`。
 *
 * ## 为什么需要它
 *
 * 原来的输入框只接受数学区间语法 `[21.0, )`，并且提示语是
 * 「方括号表示包含、圆括号表示不包含、省略一侧表示无限制」。这是集合论写法，
 * 对绝大多数用户都是陌生的——他们想的只是「Java 17 以上」或「17 到 21 之间」。
 *
 * 所以这里接受几种自然写法，内部统一翻译成区间：
 *
 * | 用户输入 | 含义 |
 * | --- | --- |
 * | `21` | 恰好 21 |
 * | `21+`、`≥21`、`>=21` | 21 及以上 |
 * | `<=17`、`≤17`、`17-` | 17 及以下 |
 * | `17-21`、`17~21`、`17 - 21` | 17 到 21（含两端） |
 * | `[21.0, )` | 仍兼容旧的区间语法 |
 *
 * 解析顺序很重要：先试区间语法（以 `[` 或 `(` 开头），再试自然写法。
 * 反过来的话，`[21.0, )` 会被当成非法输入——它以符号开头，自然写法解析不了。
 *
 * 任何一侧解析失败都返回带具体位置说明的错误，而不是笼统的「格式错误」：
 * 用户需要知道是哪一边写错了。
 */
export function parseFriendlyRange(text: string): JavaRange | { error: Localized } {
  const trimmed = text.trim();
  if (!trimmed) return { error: loc('java.rangeMissing') };

  // 1) 旧语法：以方/圆括号开头，交给原有解析器，保持向后兼容
  //    （已保存的设置里存的都是这种形式，不能因为换了输入方式就读不出来）。
  if (/^[[(]/.test(trimmed)) return parseJavaRange(trimmed);

  /*
   * 2) 自然写法。先归一化各种符号，避免后面出现四套分支：
   *    - 全角/Unicode 比较符 → ASCII
   *    - 波浪号、中文顿号 → 连字符
   *    - 两侧空白交给各分支自己 trim
   */
  const normalized = trimmed
    .replace(/≥/g, '>=')
    .replace(/≤/g, '<=')
    .replace(/[～〜]/g, '~')
    .replace(/—|–/g, '-')
    .replace(/\s+/g, ' ');

  // 2a) `>=21` / `>21` / `<=17` / `<17`
  const comparison = /^(>=|<=|>|<)\s*(\d+(?:\.\d+){0,2})$/.exec(normalized);
  if (comparison) {
    const [, operator, value] = comparison;
    const bound = toVersion(value);
    if (!bound) return { error: loc('java.rangeBadValue', { value }) };
    switch (operator) {
      case '>=':
        return { min: bound, minInclusive: true, max: null, maxInclusive: false };
      case '>':
        return { min: bound, minInclusive: false, max: null, maxInclusive: false };
      case '<=':
        return { min: null, minInclusive: false, max: bound, maxInclusive: true };
      default:
        return { min: null, minInclusive: false, max: bound, maxInclusive: false };
    }
  }

  // 2b) `21+`（及以上）与 `17-`（及以下）
  const suffix = /^(\d+(?:\.\d+){0,2})\s*([+-])$/.exec(normalized);
  if (suffix) {
    const bound = toVersion(suffix[1]);
    if (!bound) return { error: loc('java.rangeBadValue', { value: suffix[1] }) };
    return suffix[2] === '+'
      ? { min: bound, minInclusive: true, max: null, maxInclusive: false }
      : { min: null, minInclusive: false, max: bound, maxInclusive: true };
  }

  // 2c) 区间写法：`17-21`、`17 ~ 21`、`17 到 21`
  const between = /^(\d+(?:\.\d+){0,2})\s*(?:-|~|到|至|to)\s*(\d+(?:\.\d+){0,2})$/.exec(normalized);
  if (between) {
    const min = toVersion(between[1]);
    const max = toVersion(between[2]);
    if (!min) return { error: loc('java.rangeBadLower', { value: between[1] }) };
    if (!max) return { error: loc('java.rangeBadUpper', { value: between[2] }) };
    if (compareVersion(min, max) > 0) return { error: loc('java.rangeInverted') };
    return { min, minInclusive: true, max, maxInclusive: true };
  }

  // 2d) 单个版本号：`21`、`17.0.2`
  const single = /^(\d+(?:\.\d+){0,2})$/.exec(normalized);
  if (single) {
    const exact = toVersion(single[1]);
    if (!exact) return { error: loc('java.rangeBadValue', { value: single[1] }) };
    // 单个版本理解为「在这一大版本内都可接受」：用户写 21 时想要的是 Java 21，
    // 而 21.0.1、21.0.2 都属于 Java 21。若按精确匹配，几乎永远选不中运行时。
    // 因此把上限开到下一个大版本之前，下界从这个大版本的 .0 开始。
    return {
      min: { major: exact.major, minor: 0, patch: 0 },
      minInclusive: true,
      max: { major: exact.major + 1, minor: 0, patch: 0 },
      maxInclusive: false,
    };
  }

  return { error: loc('java.rangeBrackets') };
}

/** Parse a PCL-style range such as `[21.0, )`, `(, 18.0)` or `[8.0.81, 8.0.141]`. */
export function parseJavaRange(text: string): JavaRange | { error: Localized } {
  const trimmed = text.trim();
  if (!trimmed) return { error: loc('java.rangeMissing') };
  if (!/^[[(]/.test(trimmed) || !/[)\]]$/.test(trimmed)) return { error: loc('java.rangeBrackets') };
  const body = trimmed.slice(1, -1);
  const comma = body.indexOf(',');
  if (comma < 0) return { error: loc('java.rangeComma') };
  const left = body.slice(0, comma).trim();
  const right = body.slice(comma + 1).trim();
  const min = left ? toVersion(left) : null;
  const max = right ? toVersion(right) : null;
  if (left && !min) return { error: loc('java.rangeBadLower', { value: left }) };
  if (right && !max) return { error: loc('java.rangeBadUpper', { value: right }) };
  if (min && max && compareVersion(min, max) > 0) return { error: loc('java.rangeInverted') };
  return {
    min,
    minInclusive: trimmed.startsWith('['),
    max,
    maxInclusive: trimmed.endsWith(']'),
  };
}

/**
 * 把区间翻译成一句人能读的话。
 *
 * 为什么需要：`formatJavaRange` 输出的是 `[21.0, )` 这类集合语法，用它当
 * 「解析结果」的反馈，等于让刚逃离这个写法的用户又看到一遍。这里按区间形态
 * 分三类给出自然语言描述，用户能立刻确认「我要的就是这个」。
 *
 * 四种形态各自的说法：
 *   - 双侧都是开区间端点（单个版本号会被翻译成这种）→ 「Java 21」
 *   - 只有下界                                        → 「Java 21 及更高」
 *   - 只有上界                                        → 「Java 17 及更低」
 *   - 双侧                                            → 「Java 17 到 21」
 */
export function describeJavaRange(range: JavaRange, language: Language): string {
  /*
   * 版本号的显示规则：必须保留中间的 0。
   *
   * `8.0.81` 的 minor 是 0，如果按「non-zero 才显示」的写法会输出 `8.81`——
   * 把 patch 顶到了 minor 的位置，读起来完全是另一个版本。所以按层级判断：
   * 有 patch 就三段全写，只有 minor 就写两段，否则只写大版本。
   */
  const show = (version: JavaVersion) => {
    if (version.patch > 0) return `${version.major}.${version.minor}.${version.patch}`;
    if (version.minor > 0) return `${version.major}.${version.minor}`;
    return String(version.major);
  };

  /*
   * 单个大版本会在 parseFriendlyRange 里展开成 `[major.0, major+1.0)`。
   * 这种形态对用户来说就是「Java 21」，直接还原成简短说法，而不是显示
   * 「Java 21.0 到 22.0」——后者既啰嗦又容易让人以为排除了 21 的小版本。
   */
  if (
    range.min && range.max
    && range.minInclusive && !range.maxInclusive
    && range.min.minor === 0 && range.min.patch === 0
    && range.max.minor === 0 && range.max.patch === 0
    && range.max.major === range.min.major + 1
  ) {
    return renderLocalized(language, loc('java.rangeExact', { value: String(range.min.major) }));
  }

  if (range.min && !range.max) {
    return renderLocalized(language, loc('java.rangeAtLeast', { value: show(range.min) }));
  }
  if (!range.min && range.max) {
    return renderLocalized(language, loc('java.rangeAtMost', { value: show(range.max) }));
  }
  if (range.min && range.max) {
    return renderLocalized(language, loc('java.rangeBetween', { from: show(range.min), to: show(range.max) }));
  }
  return renderLocalized(language, loc('java.rangeAny'));
}

export function formatJavaRange(range: JavaRange): string {
  /*
   * 保留 patch：原先只输出 `major.minor`，于是 `[8.0.81, 8.0.141]` 会显示成
   * `[8.0, 8.0]`——一个看着像空区间的东西，用户无法确认自己写的范围。
   */
  const show = (version: JavaVersion) => {
    if (version.patch > 0) return `${version.major}.${version.minor}.${version.patch}`;
    if (version.minor > 0) return `${version.major}.${version.minor}`;
    return String(version.major);
  };
  const lower = range.min ? show(range.min) : '';
  const upper = range.max ? show(range.max) : '';
  return `${range.minInclusive ? '[' : '('}${lower}, ${upper}${range.maxInclusive ? ']' : ')'}`;
}

export function rangeContains(range: JavaRange, version: JavaVersion): boolean {
  if (range.min) {
    const cmp = compareVersion(version, range.min);
    if (cmp < 0 || (cmp === 0 && !range.minInclusive)) return false;
  }
  if (range.max) {
    const cmp = compareVersion(version, range.max);
    if (cmp > 0 || (cmp === 0 && !range.maxInclusive)) return false;
  }
  return true;
}

/** Read a runtime's numeric version from its `java -version` text, falling back to the major version. */
export function runtimeVersion(runtime: JavaRuntime): JavaVersion {
  const candidates = runtime.versionText.match(/\d+\.\d+(?:\.\d+)?/g) ?? [];
  for (const candidate of candidates) {
    const version = toVersion(candidate);
    // Skip build numbers such as "8u432" remnants or a stray "1.8.0_432" segment set.
    if (version && version.major >= 5 && version.major <= 60) return version;
  }
  return { major: runtime.majorVersion, minor: 0, patch: 0 };
}

export function formatRuntime(runtime: JavaRuntime): string {
  return `Java ${runtime.majorVersion}（${runtime.versionText}）· ${runtime.path}`;
}

export function formatRuntimeShort(runtime: JavaRuntime): string {
  return `Java ${runtime.majorVersion} · ${runtime.versionText}`;
}

/** java.exe 所在文件夹的最后一级，用来在列表里显示更短的名称。 */
export function runtimeFolder(runtime: JavaRuntime): string {
  const trimmed = runtime.path.replace(/[\\/]+$/, '');
  return trimmed.slice(Math.max(trimmed.lastIndexOf('/'), trimmed.lastIndexOf('\\')) + 1) || trimmed;
}

// ---------------------------------------------------------------------------
// Requirement analysis (port of PCL2 GetJavaRequirement)
// ---------------------------------------------------------------------------

export interface JavaRequirement {
  minMajor: number;
  /** Exclusive upper bound on the major version, or null when unbounded. */
  maxMajor: number | null;
  /** 判定理由，按应用顺序排列。存成「键 + 参数」而不是成品字符串，
   *  这样切换语言后重新渲染即可，不必重新计算需求。 */
  notes: Localized[];
}

interface GameVersion { minor: number; patch: number; known: boolean }

/** "1.20.4" -> { minor: 20, patch: 4 }. Snapshots fall back to a permissive default. */
function gameVersion(version: string): GameVersion {
  const match = /^1\.(\d+)(?:\.(\d+))?/.exec(version.trim());
  if (!match) return { minor: 0, patch: 0, known: false };
  return { minor: Number(match[1]), patch: Number(match[2] ?? 0), known: true };
}

function compareLoaderVersion(left: string, right: string): number {
  const parse = (text: string) => text.split(/[.\-+_]/).map(part => Number.parseInt(part, 10)).filter(part => Number.isFinite(part));
  const a = parse(left);
  const b = parse(right);
  for (let index = 0; index < Math.max(a.length, b.length); index += 1) {
    const diff = (a[index] ?? 0) - (b[index] ?? 0);
    if (diff !== 0) return diff;
  }
  return 0;
}

function atLeast(version: GameVersion, minor: number, patch = 0): boolean {
  return version.minor > minor || (version.minor === minor && version.patch >= patch);
}

/**
 * Derive the acceptable Java major versions for an instance. Mirrors PCL2's
 * GetJavaRequirement: vanilla version thresholds first, then loader-specific
 * bounds. OptiFine-only rules are omitted because the prototype doesn't track
 * whether OptiFine is installed.
 */
export function javaRequirementFor(instance: Pick<GameInstance, 'version' | 'loader' | 'loaderVersion'>): JavaRequirement {
  const version = gameVersion(instance.version);
  const loader = instance.loader ?? 'vanilla';
  const loaderVersion = instance.loaderVersion ?? '';
  const notes: Localized[] = [];
  let minMajor = 8;
  let maxMajor: number | null = null;

  if (!version.known) {
    // PCL 对无法解析的版本只看发布时间；这里保守地要求 Java 17。
    minMajor = 17;
    notes.push(loc('java.requirementUnknown'));
  } else if (atLeast(version, 20, 5)) {
    minMajor = 21;
    notes.push(loc('java.reqMc205'));
  } else if (atLeast(version, 18)) {
    minMajor = 17;
    notes.push(loc('java.reqMc118'));
  } else if (atLeast(version, 17)) {
    minMajor = 16;
    notes.push(loc('java.reqMc117'));
  } else if (atLeast(version, 12)) {
    minMajor = 8;
    notes.push(loc('java.reqMc112'));
  } else if (version.minor <= 5) {
    minMajor = 8;
    maxMajor = 9;
    notes.push(loc('java.reqMcOld'));
  }

  if (loader === 'fabric' || loader === 'quilt') {
    if (version.known && version.minor >= 18) {
      minMajor = Math.max(minMajor, 17);
      notes.push(loc('java.reqFabric118'));
    } else if (version.known && version.minor >= 15) {
      minMajor = Math.max(minMajor, 8);
      notes.push(loc('java.reqFabric115'));
    }
    if (loaderVersion && compareLoaderVersion(loaderVersion, '0.17.0') < 0) {
      maxMajor = Math.min(maxMajor ?? 25, 25);
      notes.push(loc('java.reqFabricLoader', { version: loaderVersion }));
    }
  }

  if (loader === 'neoforge') {
    const early = (version.known && version.minor === 20 && version.patch === 1)
      || (loaderVersion && compareLoaderVersion(loaderVersion, '20.2.62-beta') < 0);
    if (early) {
      maxMajor = Math.min(maxMajor ?? 22, 22);
      notes.push(loc('java.reqNeoForge'));
    }
  }

  if (loader === 'forge') {
    if (version.known && version.minor >= 6 && version.minor <= 7) {
      minMajor = 7;
      maxMajor = 8;
      notes.push(loc('java.reqForge167'));
    } else if (version.known && version.minor <= 12) {
      maxMajor = Math.min(maxMajor ?? 9, 9);
      notes.push(loc('java.reqForge112'));
    } else if (version.known && version.minor <= 14) {
      maxMajor = Math.min(maxMajor ?? 11, 11);
      notes.push(loc('java.reqForge113'));
    } else if (version.known && version.minor === 15) {
      maxMajor = Math.min(maxMajor ?? 16, 16);
      notes.push(loc('java.reqForge115'));
    } else if (loaderVersion) {
      if (compareLoaderVersion(loaderVersion, '34.0.0') >= 0 && compareLoaderVersion(loaderVersion, '36.2.25') <= 0) {
        maxMajor = Math.min(maxMajor ?? 9, 9);
        notes.push(loc('java.reqForge163', { version: loaderVersion }));
      } else if (compareLoaderVersion(loaderVersion, '36.2.26') >= 0 && compareLoaderVersion(loaderVersion, '37.0.0') < 0) {
        maxMajor = Math.min(maxMajor ?? 24, 24);
        notes.push(loc('java.reqForge36', { version: loaderVersion }));
      } else if (compareLoaderVersion(loaderVersion, '37.0.0') >= 0 && compareLoaderVersion(loaderVersion, '37.0.79') <= 0) {
        maxMajor = Math.min(maxMajor ?? 17, 17);
        notes.push(loc('java.reqForge171', { version: loaderVersion }));
      } else if (compareLoaderVersion(loaderVersion, '45.0.21') >= 0 && compareLoaderVersion(loaderVersion, '45.0.65') <= 0) {
        maxMajor = Math.min(maxMajor ?? 20, 20);
        notes.push(loc('java.reqForge19', { version: loaderVersion }));
      } else if (compareLoaderVersion(loaderVersion, '45.0.66') >= 0 && compareLoaderVersion(loaderVersion, '47.4.8') <= 0) {
        maxMajor = Math.min(maxMajor ?? 22, 22);
        notes.push(loc('java.reqForge21', { version: loaderVersion }));
      }
    }
  }

  return { minMajor, maxMajor, notes };
}

/**
 * Java 版本需求的显示文案，按传入语言渲染。
 */
export function renderRequirement(requirement: JavaRequirement, language: Language = 'zh-CN'): string {
  if (requirement.maxMajor === null) return translate(language, 'java.noRuntimeLabel', { values: { min: requirement.minMajor } });
  if (requirement.maxMajor === requirement.minMajor + 1) return `Java ${requirement.minMajor}`;
  return `Java ${requirement.minMajor} – ${requirement.maxMajor - 1}`;
}

/** 把需求里的理由串成一句，分隔符随语言变化。 */
export function renderRequirementNotes(requirement: JavaRequirement, language: Language): string {
  return requirement.notes
    .map(note => renderLocalized(language, note))
    .filter(Boolean)
    .join(translate(language, 'java.separatorNotes'));
}

export function isJavaCompatible(runtime: JavaRuntime, requirement: JavaRequirement): boolean {
  if (runtime.majorVersion < requirement.minMajor) return false;
  if (requirement.maxMajor !== null && runtime.majorVersion >= requirement.maxMajor) return false;
  return true;
}

// ---------------------------------------------------------------------------
// Resolution (port of PCL2 SelectOrDownloadJava)
// ---------------------------------------------------------------------------

export interface JavaResolution {
  runtime: JavaRuntime | null;
  tone: 'ok' | 'warn' | 'error';
  /** 说明当前会用什么，或为什么用不了。同样延迟到渲染时再取文案。 */
  message: Localized;
}

export interface ResolveJavaOptions {
  setting: InstanceJavaSetting;
  requirement: JavaRequirement;
  runtimes: JavaRuntime[];
  /** Optional global preference; PCL2 expresses this purely through list order. */
  globalPath?: string;
  /** 渲染嵌套片段（版本需求、范围错误）时用的语言。 */
  language?: Language;
}

function firstCompatible(runtimes: JavaRuntime[], requirement: JavaRequirement): JavaRuntime | null {
  return runtimes.find(runtime => isJavaCompatible(runtime, requirement)) ?? null;
}

export function resolveJava({ setting, requirement, runtimes, globalPath = '', language = 'zh-CN' }: ResolveJavaOptions): JavaResolution {
  if (setting.mode === 'forced') {
    if (!setting.path) {
      return { runtime: null, tone: 'warn', message: loc('java.noForced') };
    }
    const runtime = runtimes.find(item => item.path === setting.path) ?? null;
    if (!runtime) {
      return { runtime: null, tone: 'warn', message: loc('java.forcedMissing') };
    }
    if (!isJavaCompatible(runtime, requirement)) {
      return { runtime, tone: 'warn', message: loc('java.forcedIncompatible', { runtime: formatRuntime(runtime), requirement: renderRequirement(requirement, language) }) };
    }
    return { runtime, tone: 'ok', message: loc('java.willUse', { runtime: formatRuntime(runtime) }) };
  }

  if (setting.mode === 'folder') {
    if (!setting.folder.trim()) {
      return { runtime: null, tone: 'warn', message: loc('java.noFolder') };
    }
    const runtime = runtimes.find(item => item.path.startsWith(setting.folder.trim())) ?? null;
    if (!runtime) {
      return { runtime: null, tone: 'warn', message: loc('java.folderEmpty') };
    }
    return { runtime, tone: 'ok', message: loc('java.willUseFolder', { runtime: formatRuntime(runtime) }) };
  }

  if (setting.mode === 'range') {
    const parsed = parseJavaRange(setting.range);
    if ('error' in parsed) {
      return { runtime: null, tone: 'error', message: loc('java.rangeInvalidMessage', { message: renderLocalized(language, parsed.error) }) };
    }
    const runtime = runtimes.find(item => rangeContains(parsed, runtimeVersion(item))) ?? null;
    if (!runtime) {
      return { runtime: null, tone: 'warn', message: loc('java.rangeNoMatch', { range: formatJavaRange(parsed) }) };
    }
    return { runtime, tone: 'ok', message: loc('java.willUse', { runtime: formatRuntime(runtime) }) };
  }

  const preferred = globalPath ? runtimes.find(item => item.path === globalPath) ?? null : null;
  if (preferred && isJavaCompatible(preferred, requirement)) {
    return { runtime: preferred, tone: 'ok', message: loc('java.willUsePreferred', { runtime: formatRuntime(preferred) }) };
  }
  const runtime = firstCompatible(runtimes, requirement);
  if (!runtime) {
    return {
      runtime: null,
      tone: 'warn',
      message: runtimes.length
        ? loc('java.noCompatible', { requirement: renderRequirement(requirement, language) })
        : loc('java.noRuntimeAtAll'),
    };
  }
  return { runtime, tone: 'ok', message: loc('java.willUse', { runtime: formatRuntime(runtime) }) };
}

/**
 * Order freshly discovered runtimes the way PCL2 does in JavaUtils.SortAsync:
 * prefer runtimes living in a well-known runtime folder, then the major version
 * closest to 21 (the version Mojang currently ships).
 */
const CANDIDATE_FOLDER_PATTERN = /[\\/](?:\.minecraft[\\/]runtime|\.hmcl[\\/]java|runtime|jvm|java)[\\/]/i;

export function sortDiscovered(runtimes: JavaRuntime[]): JavaRuntime[] {
  return [...runtimes].sort((left, right) => {
    const leftCandidate = CANDIDATE_FOLDER_PATTERN.test(left.path) ? 0 : 1;
    const rightCandidate = CANDIDATE_FOLDER_PATTERN.test(right.path) ? 0 : 1;
    if (leftCandidate !== rightCandidate) return leftCandidate - rightCandidate;
    return Math.abs(left.majorVersion - 21) - Math.abs(right.majorVersion - 21);
  });
}

/** Move an entry inside the ordered list, mirroring PCL's MoveJavaInList. */
export function moveJava(runtimes: JavaRuntime[], index: number, offset: number): JavaRuntime[] {
  const target = index + offset;
  if (index < 0 || index >= runtimes.length || target < 0 || target >= runtimes.length) return runtimes;
  const next = [...runtimes];
  const [entry] = next.splice(index, 1);
  next.splice(target, 0, entry);
  return next;
}
