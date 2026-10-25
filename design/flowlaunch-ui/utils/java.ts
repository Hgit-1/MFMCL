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

export interface JavaRuntime {
  path: string;
  majorVersion: number;
  versionText: string;
}

export const JAVA_LIST_KEY = 'flowlaunch-java-runtimes';
export const INSTANCE_JAVA_KEY = 'flowlaunch-instance-java';

/**
 * PCL2 在列表为空时会自动触发一次搜索。网页预览没有文件系统，因此用这一组
 * 模拟搜索结果，保证实例的 Java 下拉框一开始就有可选项。
 */
export const PREVIEW_JAVA_RUNTIMES: JavaRuntime[] = [
  { path: '/usr/lib/jvm/java-21-openjdk/bin/java', majorVersion: 21, versionText: 'openjdk version "21.0.5"' },
  { path: '/usr/lib/jvm/java-17-openjdk/bin/java', majorVersion: 17, versionText: 'openjdk version "17.0.13"' },
  { path: '/usr/lib/jvm/temurin-8/bin/java', majorVersion: 8, versionText: 'openjdk version "1.8.0_432"' },
];

function isRuntime(value: unknown): value is JavaRuntime {
  if (!value || typeof value !== 'object') return false;
  const runtime = value as Partial<JavaRuntime>;
  return typeof runtime.path === 'string'
    && runtime.path.length > 0
    && typeof runtime.majorVersion === 'number'
    && Number.isFinite(runtime.majorVersion);
}

/** Load the global Java list. First run seeds the preview list, like PCL's auto search. */
export function loadJavaList(): JavaRuntime[] {
  try {
    const raw = localStorage.getItem(JAVA_LIST_KEY);
    if (raw === null) {
      saveJavaList(PREVIEW_JAVA_RUNTIMES);
      return PREVIEW_JAVA_RUNTIMES.map(runtime => ({ ...runtime }));
    }
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.filter(isRuntime) : [];
  } catch {
    return PREVIEW_JAVA_RUNTIMES.map(runtime => ({ ...runtime }));
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

export function formatJavaRange(range: JavaRange): string {
  const lower = range.min ? `${range.min.major}.${range.min.minor}` : '';
  const upper = range.max ? `${range.max.major}.${range.max.minor}` : '';
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
