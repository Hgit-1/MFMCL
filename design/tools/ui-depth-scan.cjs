/*
 * 深度扫描：主页面之外，把实例设置页、各个对话框、内容商店抽屉也一起跑。
 *
 * `ui-width-scan.cjs` 只点侧栏的六个大导航项，覆盖不到这些「点进去才有」的界面。
 * 这些恰恰是最容易在窄屏上出事的地方——对话框有固定最小宽度（Modal 的 size 档位），
 * 实例设置页有双栏布局，商店抽屉在窄屏下会变成浮层。
 *
 *   node design/tools/ui-depth-scan.cjs [baseUrl]
 *
 * 判据与 ui-width-scan.cjs 相同：横向越界 + 单行文字被截断 + 短标签被挤成竖排
 * + 窄屏侧栏异常变高 + 文档宽度超出视口。重复一份 `scan` 而不是抽取共享模块，
 * 是为了让两个脚本都能单独 `node` 运行、不依赖相对路径解析。
 */
const { chromium } = require('playwright');

const BASE = process.argv[2] || 'http://127.0.0.1:5173/';

const WIDTHS = [280, 320, 360, 390, 414, 480, 560, 640, 760, 820, 900, 1024, 1280, 1440, 1920];
const HEIGHTS = [420, 640, 900];
const LANGUAGES = ['zh-CN', 'en-US'];

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
    // 控件标签被挤成竖排（详见 ui-width-scan.cjs 里的同一条判据）。
    const isControl =
      el.tagName === 'BUTTON' ||
      el.tagName === 'A' ||
      el.getAttribute('role') === 'radio' ||
      el.getAttribute('role') === 'tab' ||
      /^H[1-6]$/.test(el.tagName);
    if (!isControl) return;
    const label = el.textContent.trim();
    if (!label || label.length > 16) return;
    // 用「文字块总高 ÷ 行高」算行数，理由见 ui-width-scan.cjs。
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
        text: label.slice(0, 20),
      });
    }
  });
  const vh = window.innerHeight;
  let sidebarTall = null;
  const aside = document.querySelector('aside');
  const isBarLayout = vw <= 760 && vw / vh >= 1 / 5;
  const asideH = aside ? aside.getBoundingClientRect().height : 0;
  if (isBarLayout) {
    const limit = Math.max(160, vh * 0.3);
    if (asideH > limit) sidebarTall = { asideH: Math.round(asideH), limit: Math.round(limit) };
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

// 每个场景：怎么进入、怎么退出。进入后必须点中真正的按钮，因此一律用第一个匹配项。
const SCENARIOS = [
  {
    name: 'instance-settings',
    enter: async page => {
      await page.locator('aside button').nth(1).click();
      await page.waitForTimeout(120);
      const manage = page.locator('main button').filter({ hasText: /管理|Manage/ }).first();
      if ((await manage.count()) === 0) return false;
      await manage.click();
      await page.waitForTimeout(200);
      return true;
    },
    leave: async page => {
      await page.keyboard.press('Escape');
      await page.waitForTimeout(150);
    },
  },
  {
    /*
     * 首次引导。
     *
     * 它是一个整页视图，出现与否取决于 localStorage 里的完成标记。扫两件事的
     * 组合：欢迎页（内容最多）和外观页（两列预设 + 配色预览——窄屏最容易挤爆
     * 的那一屏）。`leave` 里补写完成标记，否则后续场景全都会被引导挡住。
     */
    name: 'onboarding-welcome',
    enter: async page => {
      await page.evaluate(() => localStorage.removeItem('mfmcl-onboarding'));
      await page.reload({ waitUntil: 'domcontentloaded' });
      await page.waitForTimeout(620);
      const heading = (await page.locator('h1').first().textContent().catch(() => '')) ?? '';
      if (!/欢迎|Welcome|ようこそ/.test(heading)) {
        /*
         * 没进到引导就把标记写回去。
         * 本场景的 enter 会先删标记；如果这里直接返回 false，标记就永久丢了，
         * 后续每个场景都会被引导页挡住，扫描会报一堆假故障。
         */
        await page.evaluate(() => localStorage.setItem('mfmcl-onboarding', JSON.stringify({ completedAt: Date.now() })));
        return false;
      }
      return true;
    },
    leave: async page => {
      // 记下已完成，让后续场景能进入主界面。
      await page.evaluate(() => localStorage.setItem('mfmcl-onboarding', JSON.stringify({ completedAt: Date.now() })));
    },
  },
  {
    name: 'onboarding-appearance',
    enter: async page => {
      await page.evaluate(() => localStorage.removeItem('mfmcl-onboarding'));
      await page.reload({ waitUntil: 'domcontentloaded' });
      await page.waitForTimeout(620);
      // 欢迎 → 语言 → 外观
      let entered = true;
      for (let step = 0; step < 2; step += 1) {
        const next = page.locator('button').filter({ hasText: /下一步|Next|次へ/ }).first();
        if ((await next.count()) === 0) { entered = false; break; }
        await next.click();
        await page.waitForTimeout(160);
      }
      if (!entered) {
        // 同 onboarding-welcome：失败时把标记写回去，避免污染后续场景。
        await page.evaluate(() => localStorage.setItem('mfmcl-onboarding', JSON.stringify({ completedAt: Date.now() })));
        return false;
      }
      await page.waitForTimeout(240);
      return true;
    },
    leave: async page => {
      await page.evaluate(() => localStorage.setItem('mfmcl-onboarding', JSON.stringify({ completedAt: Date.now() })));
    },
  },
  {
    /*
     * 实例图标区块。
     *
     * 它有一行三段（预览、文件名、按钮）和一个可能很长的文件名。这是最容易在
     * 窄屏上挤爆的一段：文件名是用户自选、长度不可控，而它旁边还并排放着两个
     * 按钮。故意用一个超长文件名进入这一场景，否则「文件名会不会溢出」这条
     * 检查永远不会被触发。
     */
    name: 'instance-icon-section',
    enter: async page => {
      await page.evaluate(() => {
        localStorage.setItem('mfmcl-instance-icons', JSON.stringify({
          'preview-vanilla': {
            name: 'a-very-long-file-name-that-should-wrap-instead-of-overflowing-512x512.png',
            updatedAt: Date.now(),
          },
        }));
      });
      await page.locator('aside button').nth(1).click();
      await page.waitForTimeout(120);
      const manage = page.locator('main button').filter({ hasText: /管理|Manage/ }).first();
      if ((await manage.count()) === 0) return false;
      await manage.click();
      await page.waitForTimeout(280);
      return true;
    },
    leave: async page => {
      await page.evaluate(() => localStorage.removeItem('mfmcl-instance-icons'));
      await page.keyboard.press('Escape');
      await page.waitForTimeout(150);
    },
  },
  {
    name: 'create-instance-dialog',
    enter: async page => {
      await page.locator('aside button').nth(1).click();
      await page.waitForTimeout(120);
      const create = page.locator('main button').filter({ hasText: /新建|创建|Create|New/ }).first();
      if ((await create.count()) === 0) return false;
      await create.click();
      await page.waitForTimeout(220);
      return true;
    },
    leave: async page => {
      await page.keyboard.press('Escape');
      await page.waitForTimeout(150);
    },
  },
  {
    name: 'account-dialog',
    enter: async page => {
      const account = page.locator('aside button').filter({ hasText: /账户|Account|登录|Sign/ }).first();
      if ((await account.count()) === 0) return false;
      await account.click();
      await page.waitForTimeout(220);
      return true;
    },
    leave: async page => {
      await page.keyboard.press('Escape');
      await page.waitForTimeout(150);
    },
  },
  {
    name: 'settings-panel',
    enter: async page => {
      await page.locator('aside button').nth(7).click();
      await page.waitForTimeout(260);
      return true;
    },
    leave: async page => {
      const close = page.locator('[role="dialog"] button').filter({ hasText: /关闭|Close|取消|Cancel/ }).first();
      if ((await close.count()) > 0) await close.click();
      await page.keyboard.press('Escape');
      await page.waitForTimeout(200);
    },
  },
  {
    /*
     * 设置里的「下载」页。
     *
     * 这一页装着「本地 AI 模型」卡片：一行三段（图标、文案、状态徽标）、一组事实
     * 行、一段说明、两个按钮。窄屏下最容易出问题的正是这种「徽标贴在标题右侧」
     * 的结构。默认打开的标签页是「启动器」，所以必须单独点进来才能覆盖到它。
     */
    name: 'settings-download-tab',
    enter: async page => {
      await page.locator('aside button').nth(7).click();
      await page.waitForTimeout(300);
      const tab = page.locator('[role="dialog"] button').filter({ hasText: /^下载$|^Download$/ }).first();
      if ((await tab.count()) === 0) return false;
      await tab.click();
      await page.waitForTimeout(340);
      return true;
    },
    leave: async page => {
      const close = page.locator('[role="dialog"] button').filter({ hasText: /关闭|Close|取消|Cancel/ }).first();
      if ((await close.count()) > 0) await close.click();
      await page.keyboard.press('Escape');
      await page.waitForTimeout(200);
    },
  },
  {
    name: 'store-page',
    enter: async page => {
      await page.locator('aside button').nth(2).click();
      await page.waitForTimeout(120);
      const items = page.locator('#store-drawer button');
      if ((await items.count()) > 1) await items.nth(1).click();
      await page.waitForTimeout(180);
      await page.keyboard.press('Escape');
      await page.waitForTimeout(180);
      return true;
    },
    leave: async () => {},
  },
  {
    name: 'store-drawer-open',
    enter: async page => {
      await page.locator('aside button').nth(2).click();
      await page.waitForTimeout(200);
      return true;
    },
    leave: async page => {
      await page.keyboard.press('Escape');
      await page.waitForTimeout(150);
    },
  },
  {
    name: 'plugin-workspace',
    enter: async page => {
      await page.locator('aside button').nth(4).click();
      await page.waitForTimeout(240);
      return true;
    },
    leave: async () => {},
  },
];

(async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage();
  const failures = [];
  const skipped = [];
  let checks = 0;

  for (const language of LANGUAGES) {
    await page.goto(BASE, { waitUntil: 'domcontentloaded' });
    await page.evaluate(lang => localStorage.setItem('flowlaunch-language', lang), language);
    await page.goto(BASE, { waitUntil: 'networkidle' });
    await page.waitForTimeout(400);

    for (const width of WIDTHS) {
      for (const height of HEIGHTS) {
        await page.setViewportSize({ width, height });
        for (const scenario of SCENARIOS) {
          await page.goto(BASE, { waitUntil: 'domcontentloaded' });
          await page.waitForTimeout(260);
          let entered = false;
          try {
            entered = await scenario.enter(page);
          } catch (error) {
            entered = false;
          }
          if (!entered) {
            skipped.push(`${scenario.name} @${width}x${height} ${language}`);
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
            failures.push({ language, width, height, scenario: scenario.name, ...result });
          }
          try {
            await scenario.leave(page);
          } catch (error) {
            /* 退出失败不影响下一轮，因为每轮都会重新加载页面。 */
          }
        }
      }
    }
  }

  await browser.close();

  console.log(`checked ${checks} scenario combinations, skipped ${skipped.length}`);
  // 打印被跳过的组合：静默跳过等于少测一屏，而「通过」会让人以为它测过了。
  for (const entry of skipped) console.log(`  skipped: ${entry}`);
  if (failures.length === 0) {
    console.log('OK: no horizontal overflow, clipped text, cramped controls, or oversized sidebar in deep screens');
    process.exit(0);
  }
  console.log(`FAIL: ${failures.length} combinations with layout problems`);
  for (const f of failures) console.log(JSON.stringify(f));
  process.exit(1);
})();
