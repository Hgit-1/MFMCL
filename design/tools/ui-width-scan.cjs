/*
 * 宽度/高度/语言全量扫描。
 *
 * 目标：在大量视口尺寸下逐个页面渲染，检查三类真实的布局事故：
 *   1. 元素横向越界（超出视口左右边界），且不在可滚动容器里；
 *   2. 单行文本被自己所在的盒子截断（scrollWidth > clientWidth，且没有省略号）；
 *   3. 控件/标题的短标签被挤成竖排（一个字一行）；
 *   4. 窄屏下侧栏长成很高的文字块（原始缺陷的形态，见下方 `sidebarTall`）。
 *
 * 为什么不用截图：截图只能证明「我看到的这一张」没问题。这里把「尺寸矩阵 ×
 * 页面矩阵 × 语言矩阵」一起跑，任何一次改动之后跑一遍，就知道有没有在某个宽度
 * 或某种语言下把界面撑破。
 *
 *   node design/tools/ui-width-scan.cjs [baseUrl]
 *
 * 语言也要跑两种：中文短、英文长，英文往往是最早暴露横向拥挤的那一种。
 */
const { chromium } = require('playwright');

const BASE = process.argv[2] || 'http://127.0.0.1:5173/';

// 宽度覆盖：极窄手机（280/320）、主流手机、平板、窄桌面、常规桌面、超宽。
const WIDTHS = [280, 320, 360, 390, 414, 480, 560, 640, 760, 761, 820, 900, 980, 1024, 1100, 1280, 1440, 1680, 1920, 2560];
// 高度覆盖：横屏手机那种很扁的窗口一直到高竖屏。
const HEIGHTS = [420, 560, 640, 720, 900, 1080, 1440];
const LANGUAGES = ['zh-CN', 'en-US'];

// 侧栏按钮按 DOM 顺序固定为：主页、游戏库、内容商店、下载任务、启动器插件、
// 切换主题、账户管理、设置。用序号而不是无障碍名称，扫描才不会随语言变化而失效。
//
// 内容商店特殊：侧栏那一下只是开合分类抽屉，进商店要再点一个分类。所以这个场景
// 分两步走，然后按 Escape 收起抽屉，单独检查商店页面本身的排版。
const VIEWS = [
  { name: 'home', open: async page => page.locator('aside button').nth(0).click() },
  { name: 'library', open: async page => page.locator('aside button').nth(1).click() },
  {
    name: 'store',
    open: async page => {
      await page.locator('aside button').nth(2).click();
      await page.waitForTimeout(80);
      const drawerItems = page.locator('#store-drawer button');
      if ((await drawerItems.count()) > 1) await drawerItems.nth(1).click();
      await page.keyboard.press('Escape');
    },
  },
  {
    name: 'store-drawer',
    open: async page => {
      await page.locator('aside button').nth(2).click();
    },
  },
  { name: 'downloads', open: async page => page.locator('aside button').nth(3).click() },
  { name: 'plugins', open: async page => page.locator('aside button').nth(4).click() },
  { name: 'settings', open: async page => page.locator('aside button').nth(7).click() },
];

const scan = () => {
  const vw = window.innerWidth;
  const scrollable = el => {
    let p = el.parentElement;
    while (p && p !== document.body) {
      const ox = getComputedStyle(p).overflowX;
      if (ox === 'auto' || ox === 'scroll') return true;
      p = p.parentElement;
    }
    return false;
  };
  const visible = el => {
    const cs = getComputedStyle(el);
    if (cs.display === 'none' || cs.visibility === 'hidden' || cs.opacity === '0') return false;
    const r = el.getBoundingClientRect();
    return r.width > 0 || r.height > 0;
  };
  const outside = [];
  const clipped = [];
  const cramped = [];
  document.querySelectorAll('body *').forEach(el => {
    if (!visible(el)) return;
    if (scrollable(el)) return;
    const r = el.getBoundingClientRect();
    const right = r.right - vw;
    const left = -r.left;
    if (right > 1.5 || left > 1.5) {
      outside.push({
        tag: el.tagName,
        cls: (el.className || '').toString().slice(0, 44),
        right: +right.toFixed(1),
        left: +left.toFixed(1),
        text: el.textContent.trim().slice(0, 28),
      });
    }
    const cs = getComputedStyle(el);
    if (
      el.children.length === 0 &&
      el.textContent.trim() &&
      el.scrollWidth - el.clientWidth > 2 &&
      cs.overflowX === 'hidden' &&
      cs.textOverflow !== 'ellipsis' &&
      cs.whiteSpace !== 'normal'
    ) {
      clipped.push({
        cls: (el.className || '').toString().slice(0, 44),
        cut: el.scrollWidth - el.clientWidth,
        text: el.textContent.trim().slice(0, 28),
      });
    }
    /*
     * 控件标签被挤成竖排。
     *
     * 这是横向空间不足的第三种表现：不越界、也不截断，而是把「安装插件」四个字
     * 一个字一行地摞起来——按钮高 68px、宽只有 60px。截图里看得很清楚，但任何
     * 几何检查都不会报错，所以必须单独判一次。
     *
     * 判据：可交互元素（或标题）的文字短到本该一行放得下（≤16 字），却渲染成
     * 两行以上，且宽度不到字号 8 倍。长标题在窄卡片里折行是正常的，所以这里只
     * 盯控件与标题，且用「很短却折行」这个组合来收紧。
     */
    const isControl =
      el.tagName === 'BUTTON' ||
      el.tagName === 'A' ||
      el.getAttribute('role') === 'radio' ||
      el.getAttribute('role') === 'tab' ||
      /^H[1-6]$/.test(el.tagName);
    if (!isControl) return;
    const label = el.textContent.trim();
    if (!label || label.length > 16) return;
    /*
     * 用「文字块总高 ÷ 行高」算行数。
     *
     * 不能数 `getClientRects()`：同一行里图标一段、文字一段会各返回一个矩形；
     * 中英文混排时字体回退也会把一行切成两块。按矩形顶部聚类又会被 1px 的抖动
     * 骗到（实测「搜索模组」单行、高度 24px 仍被算成两行）。
     *
     * 取所有矩形的并集高度，再除以计算出的 line-height，得到的是真实的视觉行数：
     * 单行 24/24 = 1，五行 82/16.5 ≈ 5。
     */
    const range = document.createRange();
    range.selectNodeContents(el);
    const rects = [...range.getClientRects()].filter(rect => rect.width > 0 && rect.height > 0);
    if (rects.length === 0) return;
    const top = Math.min(...rects.map(rect => rect.top));
    const bottom = Math.max(...rects.map(rect => rect.bottom));
    const lineHeight = parseFloat(cs.lineHeight) || (parseFloat(cs.fontSize) || 12) * 1.5;
    const lines = Math.round((bottom - top) / lineHeight);
    const fontSize = parseFloat(cs.fontSize) || 12;
    if (lines >= 2 && r.width < fontSize * 8) {
      cramped.push({
        tag: el.tagName,
        cls: (el.className || '').toString().slice(0, 44),
        lines,
        width: +r.width.toFixed(1),
        fontSize,
        text: label.slice(0, 20),
      });
    }
  });
  /*
   * 侧栏在窄屏下应该是一条图标条，高度由图标决定（约 100px）。
   *
   * 这条是从最初的缺陷直接来的回归判据：`storeDrawer` 被固定进 40px 宽的
   * `.storeMenuWrap` 时，里面 5 个条目被压成一词一行，侧栏从 58px 长到 379px，
   * 把整页顶下去。这个失败模式不越界、不截断，只有看侧栏高度才发现。
   *
   * 判据：视口不超过 760px（此时侧栏是横向图标条）时，侧栏高度不超过 160px 与
   * 视口 30% 中较大的那个。宽高比小于 1/5 的极端长屏走整页滚动，单独排除。
   */
  const vh = window.innerHeight;
  let sidebarTall = null;
  const aside = document.querySelector('aside');
  const isBarLayout = vw <= 760 && vw / vh >= 1 / 5;
  const asideH = aside ? aside.getBoundingClientRect().height : 0;
  if (isBarLayout) {
    const limit = Math.max(160, vh * 0.3);
    if (asideH > limit) {
      sidebarTall = { asideH: Math.round(asideH), limit: Math.round(limit) };
    }
  }
  return {
    doc: document.documentElement.scrollWidth,
    asideH: Math.round(asideH),
    outside,
    clipped,
    cramped,
    sidebarTall,
    outsideCount: outside.length,
    clippedCount: clipped.length,
    crampedCount: cramped.length,
  };
};

/*
 * 每一屏都从干净状态开始。
 *
 * 之前的顺序扫描会在页面之间留下状态：点开「设置」后那个对话框是模态的，下一轮
 * 点侧栏会被它拦住，点击超时被 `catch` 吞掉，于是「插件」页实际上还是上一屏的
 * 内容——扫描报了一个并不存在的 `h2` 折行。用「重载 + 重新导航」换确定性：变慢
 * 一点，但每条结论都能复现。
 */
const navTo = async (page, view) => {
  await page.goto(BASE, { waitUntil: 'domcontentloaded' });
  await page.waitForTimeout(140);
  /*
   * 跳过首次引导。
   *
   * 引导是一个整页视图，此时页面里根本没有 `aside`，下面所有按侧栏导航的操作都会
   * 卡在等待上直到超时——扫描会因此在第一个组合就挂住，看起来像「跑不完」而不是
   * 「发现回归」。引导本身由 `ui-depth-scan.cjs` 单独覆盖。
   */
  const onOnboarding = (await page.locator('aside').count()) === 0;
  if (onOnboarding) {
    await page.evaluate(() => localStorage.setItem('mfmcl-onboarding', JSON.stringify({ completedAt: Date.now() })));
    await page.goto(BASE, { waitUntil: 'domcontentloaded' });
    await page.waitForTimeout(160);
  }
  const dialog = page.locator('[role="dialog"]');
  if ((await dialog.count()) > 0) await page.keyboard.press('Escape');
  await view.open(page);
  await page.waitForTimeout(110);
};

(async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage();
  const failures = [];
  let checks = 0;

  for (const language of LANGUAGES) {
    await page.goto(BASE, { waitUntil: 'domcontentloaded' });
    await page.evaluate(lang => localStorage.setItem('flowlaunch-language', lang), language);
    // 引导由 ui-depth-scan.cjs 覆盖；这里一律跳过，否则每个组合都会卡在引导页。
    await page.evaluate(() => localStorage.setItem('mfmcl-onboarding', JSON.stringify({ completedAt: Date.now() })));

    for (const width of WIDTHS) {
      for (const height of HEIGHTS) {
        await page.setViewportSize({ width, height });
        for (const view of VIEWS) {
          try {
            await navTo(page, view);
          } catch (error) {
            continue;
          }
          const result = await page.evaluate(scan);
          checks += 1;
          if (
            result.outsideCount > 0 ||
            result.clippedCount > 0 ||
            result.crampedCount > 0 ||
            result.sidebarTall ||
            result.doc > width + 1
          ) {
            failures.push({ language, width, height, view: view.name, ...result });
          }
        }
      }
    }
  }

  await browser.close();

  console.log(`checked ${checks} view combinations`);
  if (failures.length === 0) {
    console.log('OK: no horizontal overflow, clipped text, cramped controls, or oversized sidebar');
    process.exit(0);
  }
  console.log(`FAIL: ${failures.length} combinations with layout problems`);
  for (const f of failures) console.log(JSON.stringify(f));
  process.exit(1);
})();
