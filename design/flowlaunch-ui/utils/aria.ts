// Aria with You：按季节切换的立绘与配色。
//
// ## 为什么单独一个模块
//
// 这个插件要同时做三件事，而三件事的宿主不同：
//
//   1. **判断现在是哪个季节**（这里）—— 纯函数，可测；
//   2. **换配色**（这里）—— 必须走 `utils/accent.ts` 的既有规则，否则一个
//      明亮的樱粉强调色会把按钮上的字变得读不清；
//   3. **画立绘**（`components/AriaFigure.tsx`）—— 纯展示。
//
// 混在组件里会导致「季节判断」没法单测、配色规则被抄成第二份。第二份是最危险的：
// `accent.ts` 里的对比度修正改了一次，这里抄的那份不会跟着改，于是四个季节里有
// 一个的用户看到不可读的按钮。
//
// ## 配色为什么只给「强调色」，不覆盖整个界面
//
// docs/FUTURE.md 把这条写成了实现前必须过的门槛：插件不得破坏四个预设与深浅
// 主题下的文字对比度。直接写 `--color-bg-primary` 之类的变量能立刻换掉界面底色，
// 但也立刻就能把某个深色主题变成浅底浅字。
//
// 所以这里只做两件安全的事：
//   - 把季节色交给 `applyAccent`，对比度完全由既有规则保证；
//   - 额外给一层可选的低透明度季节染色（`--aria-tint-*`），它只是叠在背景之上
//     的一点色，不参与任何文字与其底色的对比计算。
//
// ## 季节怎么判断
//
// 用**气象季节**（3/6/9/12 月 1 日切换），不是天文季节（春分/夏至/秋分/冬至）。
// 理由是可解释：用户说「现在是春天吗」，心里的答案是月份，不是太阳黄经。
// 春分（3/20 前后）还会让「3 月 1 日」这种日期落在一个反直觉的季节里。
//
// 南半球在同一个日期上是相反的季节，所以有一个显式的半球开关，而不是把月份
// 硬编码成北半球——这个启动器没有地区数据，猜错一次就是整个上半年都错。

import { applyAccent, DEFAULT_ACCENT } from './accent';

/** 四季。取值同时是素材文件名的一部分（`aria-spring-hero.webp`）。 */
export type Season = 'spring' | 'summer' | 'autumn' | 'winter';

export const SEASONS: Season[] = ['spring', 'summer', 'autumn', 'winter'];

/**
 * 所选季节的生效方式。
 *
 * `auto` 是默认：这是插件存在的理由，按真实日期换季。`fixed` 给「我就想看雪」
 * 的人一个出口——每到季节切换时插件会突然换掉他正在看的立绘，而对某些用户来说
 * 那不是惊喜而是打扰。`off` 保留在数据里而不是删掉设置项，切换回来自动模式时
 * 之前的固定季节还在。
 */
export type AriaSeasonMode = 'auto' | 'fixed';

export interface AriaSetting {
  /** 是否启用（与插件开关同步，见 `App.tsx` 的说明）。 */
  enabled: boolean;
  mode: AriaSeasonMode;
  /** `mode === 'fixed'` 时使用。 */
  fixedSeason: Season;
  /** 南半球：同一日期上季节相反。 */
  southernHemisphere: boolean;
  /** 侧栏底部的小立绘。关掉它只为省下那块空间。 */
  showSidebar: boolean;
  /** 主页大立绘。 */
  showHero: boolean;
  /**
   * 其余页面（游戏库、内容商店、下载任务、插件、实例详情）的大幅水印立绘。
   *
   * 与 `showHero` 分开是因为两者的风险完全不同：主页那张立绘占的是一块本来
   * 就留白的区域，而这一张铺在内容下面。用户在面包屑、卡片与表格上读字，
   * 一旦觉得碍眼就必须能单独关掉它，而不是把主页那张也一起关掉。
   */
  showWatermark: boolean;
  /** 低透明度季节染色。 */
  tintSurfaces: boolean;
}

export const ARIA_STORAGE_KEY = 'mfmcl-aria';

export const DEFAULT_ARIA: AriaSetting = {
  enabled: false,
  mode: 'auto',
  fixedSeason: 'spring',
  southernHemisphere: false,
  showSidebar: true,
  showHero: true,
  showWatermark: true,
  tintSurfaces: false,
};

/**
 * 判断某一天属于哪个季节。
 *
 * 边界取每月 1 日：3–5 春、6–8 夏、9–11 秋、12–2 冬。
 *
 * `month` 从 1 开始，与 `Date.getMonth()` 的 0 基不同——调用方必须显式转换，
 * 这个转换写在 `seasonAt` 里只有一处，避免每个调用点各错一次。测试直接传月份，
 * 不需要构造 `Date`。
 */
export function seasonForMonth(month: number, southernHemisphere: boolean): Season {
  // 先按北半球定，再整体挪半年。让「12–2 是冬」这个直觉在代码里只出现一次。
  const northern: Season = month >= 3 && month <= 5
    ? 'spring'
    : month >= 6 && month <= 8
      ? 'summer'
      : month >= 9 && month <= 11
        ? 'autumn'
        : 'winter';
  if (!southernHemisphere) return northern;
  const opposite: Record<Season, Season> = {
    spring: 'autumn',
    summer: 'winter',
    autumn: 'spring',
    winter: 'summer',
  };
  return opposite[northern];
}

/** 按一个 `Date` 判断季节。 */
export function seasonAt(date: Date, southernHemisphere: boolean): Season {
  return seasonForMonth(date.getMonth() + 1, southernHemisphere);
}

/** 实际生效的季节：固定优先，否则看今天的日期。 */
export function resolveSeason(setting: AriaSetting, now: Date = new Date()): Season {
  return setting.mode === 'fixed'
    ? setting.fixedSeason
    : seasonAt(now, setting.southernHemisphere);
}

// ---------------------------------------------------------------------------
// 素材
// ---------------------------------------------------------------------------

/**
 * 素材路径。
 *
 * 写成常量而不是 `fetch('manifest.json')`：八张图的文件名是构建产物的一部分，
 * 运行期再取一次清单只是多一个可能失败的请求。清单仍然随包发布（`manifest.json`
 * 记录了生成用的模型、提示词与哈希），供人核对，不供运行时查询。
 *
 * 尺寸：`hero` 是 1024×1536 竖幅全身，`portrait` 是 1024×1024 半身；`thumb` 是
 * 384 px 的长边缩略图，供侧栏与卡片使用——把 280 KB 的原图塞进 96 CSS px 的框里，
 * 解码成本是白付的。
 */
export interface SeasonArt {
  hero: string;
  portrait: string;
  thumb: string;
}

const ART_BASE = '/plugins/aria-with-you';

export const SEASON_ART: Record<Season, SeasonArt> = {
  spring: {
    hero: `${ART_BASE}/aria-spring-hero.webp`,
    portrait: `${ART_BASE}/aria-spring-portrait.webp`,
    thumb: `${ART_BASE}/aria-spring-portrait-thumb.webp`,
  },
  summer: {
    hero: `${ART_BASE}/aria-summer-hero.webp`,
    portrait: `${ART_BASE}/aria-summer-portrait.webp`,
    thumb: `${ART_BASE}/aria-summer-portrait-thumb.webp`,
  },
  autumn: {
    hero: `${ART_BASE}/aria-autumn-hero.webp`,
    portrait: `${ART_BASE}/aria-autumn-portrait.webp`,
    thumb: `${ART_BASE}/aria-autumn-portrait-thumb.webp`,
  },
  winter: {
    hero: `${ART_BASE}/aria-winter-hero.webp`,
    portrait: `${ART_BASE}/aria-winter-portrait.webp`,
    thumb: `${ART_BASE}/aria-winter-portrait-thumb.webp`,
  },
};

// ---------------------------------------------------------------------------
// 分页动作
// ---------------------------------------------------------------------------

/**
 * 有独立动作的页面。
 *
 * ## 为什么动作属于「页面」而天气属于「季节」
 *
 * 要求是「一个页面一个动作」且「动作随日期切换」，两者相乘是 6 页 × 4 季 = 24
 * 张图。24 张里模型的漂移（发夹、发色）会明显到破坏一致性——这一点在 8 张的
 * 规模上已经验证过。
 *
 * 所以拆成两个正交的维度：**手势由页面决定，它在四季里保持同一个轮廓**；天气、
 * 穿搭、光线由季节决定。于是每页一张动作模板 × 四季各一份天气，用户看到的是
 * 「同样的姿势，不同的季节」，既满足「每页不同」也满足「随日期变化」。
 *
 * `dashboard` 不在此列：它已经有 `hero` 那张竖幅全身（见 `SEASON_ART`），再叠
 * 一张分页图会变成两个人。
 */
export type AriaPage = 'library' | 'store' | 'downloads' | 'plugins' | 'instance-detail';

export const ARIA_PAGES: AriaPage[] = ['library', 'store', 'downloads', 'plugins', 'instance-detail'];

/** 页面动作素材的路径。 */
export function pageArt(page: AriaPage, season: Season): { hero: string; thumb: string } {
  const base = `${ART_BASE}/pages/aria-${page}-${season}`;
  return { hero: `${base}.webp`, thumb: `${base}-thumb.webp` };
}

/**
 * 把 App 的 `context` 映射成动作页面。
 *
 * ## 名称不一致是真实存在的坑
 *
 * 导航里的 `context` 与页面的中文名不是一一对应的：主页是 `dashboard`，而
 * 「游戏库」用的是 `home`。动作键按页面语义命名（`library`），所以这里必须显式
 * 写映射；如果直接把 `context` 当键用，`library` 永远匹配不上，那一页就会静默地
 * 没有立绘。
 *
 * 未知的 context 返回 `null`（不画水印），而不是退回某个默认页：将来新增页面时
 * 「没有动作」是正确且安全的默认，而退回 `library` 会让新页面悄悄显示一张与它
 * 内容无关的立绘。
 */
export function pageForContext(context: string): AriaPage | null {
  switch (context) {
    case 'home':
      return 'library';
    case 'store':
      return 'store';
    case 'downloads':
      return 'downloads';
    case 'plugins':
      return 'plugins';
    case 'instance-detail':
      return 'instance-detail';
    default:
      // `dashboard` 走的是 `SEASON_ART` 那张竖幅全身，不叠分页水印。
      return null;
  }
}

// ---------------------------------------------------------------------------
// 配色
// ---------------------------------------------------------------------------

export interface SeasonPalette {
  /** 强调色。交给 `applyAccent`，由它负责对比度。 */
  accent: string;
  /**
   * 染色用的两色渐变。透明度由 CSS 决定（很低），这里只给颜色。
   *
   * 两色而不是一色：单一色块会让整页像蒙了一层单色滤片，两个相邻色相看起来
   * 像「季节的光」，这也是真实光照的样子（晨光偏粉、正午偏青）。
   */
  tint: [string, string];
}

/**
 * 季节配色。
 *
 * 取值刻意**不收进很窄的色相区间**：`accent.ts` 会按主题把强调色提亮或压暗以
 * 达到 4.5:1，起点过暗（例如深秋的赭石）会被推得很远，最后看起来已经不是那个
 * 季节了。四个值都落在 L≈0.6–0.75，是那些推导规则修改幅度最小的一段。
 */
export const SEASON_PALETTE: Record<Season, SeasonPalette> = {
  spring: { accent: '#e58fb2', tint: ['#fbe4ee', '#e9f6ee'] },
  summer: { accent: '#2aa6d6', tint: ['#dff2fb', '#eaf7e6'] },
  autumn: { accent: '#d97742', tint: ['#fbe9dd', '#f6e7d6'] },
  winter: { accent: '#6f9fd8', tint: ['#e6eefa', '#f2f5fa'] },
};

/**
 * 深色主题下的染色。
 *
 * 浅色那组用在高亮背景上是对的，铺到深色界面会变成一片发白的雾——那正是
 * 「配色功能坏了」的观感。深色版本压到同色相的低明度，保持季节可辨认。
 */
export const SEASON_TINT_DARK: Record<Season, [string, string]> = {
  spring: ['#2a1a24', '#1a2420'],
  summer: ['#12242e', '#172418'],
  autumn: ['#2a1e15', '#241c12'],
  winter: ['#161f2e', '#1a1e26'],
};

// ---------------------------------------------------------------------------
// 应用与撤销
// ---------------------------------------------------------------------------

/**
 * 把季节写进界面。
 *
 * 只碰强调色（经 `applyAccent`）与三个插件自有的装饰变量。**不碰** `--color-bg-*`
 * 与 `--color-text-*`：它们是全站对比度计算的基准，插件改一个值，同一个主题下
 * 其它组件的可读性就跟着变，而插件没有能力重新验证那些组合。
 *
 * 返回撤销函数：关掉插件时调用，界面必须回到用户原来选的强调色，而不是停在
 * 最后一个季节的颜色上。
 */
export function applySeasonTheme(
  season: Season,
  dark: boolean,
  tint: boolean,
  target: HTMLElement = document.documentElement,
): () => void {
  const previousAccent = target.style.getPropertyValue('--color-primary-500');
  applyAccent(SEASON_PALETTE[season].accent, target);

  const colors: [string, string] = dark
    ? SEASON_TINT_DARK[season]
    : SEASON_PALETTE[season].tint;
  target.style.setProperty('--aria-tint-a', colors[0]);
  target.style.setProperty('--aria-tint-b', colors[1]);
  target.style.setProperty('--aria-tint-strength', tint ? '1' : '0');
  target.dataset.ariaSeason = season;

  return () => {
    delete target.dataset.ariaSeason;
    target.style.removeProperty('--aria-tint-a');
    target.style.removeProperty('--aria-tint-b');
    target.style.removeProperty('--aria-tint-strength');
    // 强调色交给调用方恢复：只有它知道用户原本选的是哪个（可能是手选的，也可能
    // 来自背景取色）。在这里恢复成默认蓝会悄悄改掉用户的选择。
    if (previousAccent) target.style.setProperty('--color-primary-500', previousAccent);
    else target.style.removeProperty('--color-primary-500');
  };
}

/** 撤销时用的兜底色，供调用方在拿不到原值时使用。 */
export const ARIA_FALLBACK_ACCENT = DEFAULT_ACCENT;

// ---------------------------------------------------------------------------
// 持久化
// ---------------------------------------------------------------------------

function isSeason(value: unknown): value is Season {
  return typeof value === 'string' && (SEASONS as string[]).includes(value);
}

/**
 * 读取设置。
 *
 * 逐字段兜底：这个对象会随版本增减字段，而一个缺失的布尔值若变成 `undefined`，
 * 界面上的开关会显示为「关」但实际按默认启用执行——两者不一致时用户无法判断
 * 到底哪个是真的。
 */
export function readAriaSetting(): AriaSetting {
  let raw: unknown;
  try {
    raw = JSON.parse(localStorage.getItem(ARIA_STORAGE_KEY) || 'null');
  } catch {
    return { ...DEFAULT_ARIA };
  }
  if (!raw || typeof raw !== 'object') return { ...DEFAULT_ARIA };
  const value = raw as Partial<AriaSetting>;
  return {
    enabled: value.enabled === true,
    mode: value.mode === 'fixed' ? 'fixed' : 'auto',
    fixedSeason: isSeason(value.fixedSeason) ? value.fixedSeason : DEFAULT_ARIA.fixedSeason,
    southernHemisphere: value.southernHemisphere === true,
    showSidebar: value.showSidebar !== false,
    showHero: value.showHero !== false,
    showWatermark: value.showWatermark !== false,
    tintSurfaces: value.tintSurfaces === true,
  };
}

/** 写入设置。存储不可用时静默失败——外观偏好不该阻断任何操作。 */
export function saveAriaSetting(setting: AriaSetting): void {
  try {
    localStorage.setItem(ARIA_STORAGE_KEY, JSON.stringify(setting));
  } catch {
    // 隐私模式下只在本次会话生效。
  }
}

// ---------------------------------------------------------------------------
// 文案
// ---------------------------------------------------------------------------

/** 季节名对应的词典键。集中一处，日历、设置页、立绘标注共用。 */
export const SEASON_LABEL_KEYS = {
  spring: 'aria.seasonSpring',
  summer: 'aria.seasonSummer',
  autumn: 'aria.seasonAutumn',
  winter: 'aria.seasonWinter',
} as const;

/**
 * 下一个换季日期，以及换季后会变成哪个季节。
 *
 * 只在自动模式下有意义：固定季节没有「下次」。界面用它解释插件会自己动，
 * 而不是等用户某天打开启动器发现立绘换了、以为是 bug。
 */
export function nextSeasonChange(
  now: Date,
  southernHemisphere: boolean,
): { season: Season; date: Date } {
  const year = now.getFullYear();
  // 换季日：北半球的 3/1、6/1、9/1、12/1。南半球沿用同一组日期（季节相反）。
  const boundaries = [
    new Date(year, 2, 1),
    new Date(year, 5, 1),
    new Date(year, 8, 1),
    new Date(year, 11, 1),
    new Date(year + 1, 2, 1),
  ];
  const date = boundaries.find(candidate => candidate.getTime() > now.getTime()) ?? boundaries[0];
  // 换季日当天的季节就是「换完之后」的季节——按定义，边界日属于新季节。
  return { season: seasonAt(date, southernHemisphere), date };
}
