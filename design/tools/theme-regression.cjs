const assert = require('node:assert/strict');
const { chromium } = require('playwright');
const BASE = process.argv[2] || 'http://127.0.0.1:5173';
const ARIA_RECORD = {
  id: 'aria-with-you', version: '1.0.0-test', directory: 'aria-with-you',
  path: 'C:/synthetic-plugin-fixture/aria-with-you',
  assetBase: 'http://asset.localhost/synthetic-theme-fixture/aria-with-you',
  manifest: {
    name: 'Aria with You', description: 'Synthetic seasonal regression fixture', author: 'Regression tests',
    glyph: 'palette', accent: '#e58fb2', hooks: ['theme', 'sidebar', 'background'],
    permissions: ['theme', 'uiSlot', 'storage'], enabledByDefault: false,
  },
};
const SYNTHETIC_SVG = '<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="#e76b8a"/></svg>';

async function fixture(browser, theme, delayInitial = false) {
  const context = await browser.newContext({ viewport: { width: 1280, height: 900 }, reducedMotion: 'reduce' });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.route('http://asset.localhost/**', route => route.fulfill({ contentType: 'image/svg+xml', body: SYNTHETIC_SVG }));
  await page.route(/\/plugins\//, route => {
    errors.push(`Unexpected public plugin asset request: ${route.request().url()}`);
    return route.abort();
  });
  await page.route(/\/utils\/pluginInstall\.ts(?:\?t=\d+)?$/, route => route.fulfill({ contentType: 'text/javascript', body: `
    export * from '/utils/pluginInstall.ts?theme-base';
    export const listInstalledPlugins = async () => [${JSON.stringify(ARIA_RECORD)}];
  ` }));
  await page.route('**/__mfmcl-theme-test__', route => route.fulfill({
    contentType: 'text/html',
    body: '<!doctype html><html><head><script type="module">import R from "/@react-refresh";R.injectIntoGlobalHook(window);window.$RefreshReg$=()=>{};window.$RefreshSig$=()=>type=>type;window.__vite_plugin_react_preamble_installed__=true;</script></head><body><div id="root"></div></body></html>',
  }));
  await page.route(/\/utils\/eula\.ts(?:\?t=\d+)?$/, route => route.fulfill({ contentType: 'text/javascript', body: 'export const hasAcceptedEula=()=>true;export const acceptEula=()=>{};' }));
  await page.route(/\/utils\/onboarding\.ts(?:\?t=\d+)?$/, route => route.fulfill({ contentType: 'text/javascript', body: 'export const isOnboardingPending=()=>false;export const markOnboardingComplete=()=>{};export const resetOnboarding=()=>{};' }));
  await page.route(/\/utils\/native\.ts(?:\?t=\d+)?$/, route => route.fulfill({ contentType: 'text/javascript', body: `
    export * from '/utils/native.ts?theme-base';
    export const listInstances = async () => {
      if (window.themeFixture.delayInitial) await new Promise(resolve => window.themeFixture.initialReads.push(resolve));
      return [];
    };
    export const listAccounts = async () => [];
    export const listMods = async () => [];
    export const listDownloadTasks = async () => [];
    export const previewFileProgress = () => [];
    export const clearFinishedDownloadTasks = async () => 0;
  ` }));
  await page.goto(`${BASE}/__mfmcl-theme-test__`);
  await page.waitForFunction(() => window.__vite_plugin_react_preamble_installed__);
  await page.evaluate(async ({ theme, delayInitial }) => {
    window.themeFixture = { delayInitial, initialReads: [] };
    const prefs = await import('/utils/launcherPrefs.ts');
    const native = await import('/utils/native.ts?theme-base');
    const plugins = await import('/utils/plugins.ts');
    const aria = await import('/utils/aria.ts');
    localStorage.setItem('theme', theme === 'light' ? 'dark' : 'light');
    prefs.savePrefs({ ...prefs.DEFAULT_PREFS, theme, customAccent: '#41c98b' });
    await native.saveSettings({ ...native.fallbackSettings(), themeMode: theme, customAccent: '#41c98b', clearFinishedTasks: false });
    const store = plugins.loadPluginStore();
    plugins.savePluginStore({ ...store, enabled: { ...store.enabled, 'aria-with-you': false } });
    aria.saveAriaSetting({ ...aria.DEFAULT_ARIA, enabled: false, mode: 'fixed', fixedSeason: 'spring', showHero: false, showSidebar: false, showWatermark: false });
    const { default: R } = await import('/node_modules/.vite/deps/react.js');
    const { default: client } = await import('/node_modules/.vite/deps/react-dom_client.js');
    const { I18nProvider } = await import('/utils/i18n/I18nProvider.tsx');
    const { default: App } = await import('/src/App.tsx');
    window.themeFixture.root = client.createRoot(document.querySelector('#root'));
    window.themeFixture.mount = () => window.themeFixture.root.render(R.createElement(R.StrictMode, null, R.createElement(I18nProvider, { initialPreference: 'en' }, R.createElement(App))));
    window.themeFixture.mount();
  }, { theme, delayInitial });
  await page.getByRole('button', { name: 'Settings', exact: true }).waitFor();
  if (!delayInitial) await page.waitForFunction(theme => document.documentElement.dataset.theme === theme, theme);
  return { context, page, errors };
}

async function palette(page, accent) {
  await page.waitForFunction(accent => document.documentElement.style.getPropertyValue('--color-primary-500') === accent, accent);
  await page.waitForFunction(() => {
    const nav = document.querySelector('aside nav button[class*="_active_"]');
    const hex = getComputedStyle(document.documentElement).getPropertyValue('--color-primary-tonal').trim();
    const rgb = `rgb(${[1, 3, 5].map(i => parseInt(hex.slice(i, i + 2), 16)).join(', ')})`;
    return nav && getComputedStyle(nav).backgroundColor === rgb;
  });
  const result = await page.evaluate(async accent => {
    const { applyAccent, contrastRatio } = await import('/utils/accent.ts');
    const { default: buttonStyles } = await import('/components/Button.module.css');
    const { default: modStyles } = await import('/components/ModManager.module.css');
    const primary = document.createElement('button');
    primary.className = buttonStyles.primary;
    const modToggle = document.createElement('button');
    modToggle.className = modStyles.toggleOn;
    document.body.append(primary, modToggle);
    const primaryBackground = getComputedStyle(primary).backgroundColor;
    const modColour = getComputedStyle(modToggle).color;
    primary.remove();
    modToggle.remove();
    const expected = document.createElement('div');
    applyAccent(accent, expected);
    const root = document.documentElement;
    const style = getComputedStyle(root);
    const mismatches = [...expected.style].filter(key => root.style.getPropertyValue(key) !== expected.style.getPropertyValue(key));
    const nav = document.querySelector('aside nav button[class*="_active_"]');
    const tonal = style.getPropertyValue('--color-primary-tonal').trim();
    const tonalText = style.getPropertyValue('--color-primary-tonal-text').trim();
    const rgb = hex => `rgb(${[1, 3, 5].map(i => parseInt(hex.slice(i, i + 2), 16)).join(', ')})`;
    return {
      mismatches, nav: nav && getComputedStyle(nav).backgroundColor, expectedNav: rgb(tonal),
      primaryBackground, expectedPrimary: rgb(style.getPropertyValue('--color-primary-solid').trim()),
      modColour, expectedMod: rgb(accent),
      contrast: contrastRatio(tonal, tonalText),
      solidContrast: contrastRatio(style.getPropertyValue('--color-primary-solid').trim(), style.getPropertyValue('--color-on-primary').trim()),
    };
  }, accent);
  assert.deepEqual(result.mismatches, [], `derived tokens for ${accent}`);
  assert.equal(result.nav, result.expectedNav);
  assert.equal(result.primaryBackground, result.expectedPrimary);
  assert.equal(result.modColour, result.expectedMod);
  assert.ok(result.contrast >= 4.5 && result.solidContrast >= 4.5);
}

async function openAppearance(page) {
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('button', { name: 'Appearance', exact: true }).click();
  return page.getByRole('dialog');
}
async function closeSettings(page, escape = false) {
  if (escape) await page.keyboard.press('Escape');
  else await page.getByRole('button', { name: 'Close settings', exact: true }).click();
  await page.getByRole('dialog').waitFor({ state: 'hidden' });
}
async function save(page) {
  await page.getByRole('button', { name: 'Save settings', exact: true }).click();
  await page.getByRole('button', { name: 'Saved', exact: true }).waitFor();
}
async function imageFile(page) {
  return page.evaluate(async () => {
    const canvas = document.createElement('canvas');
    canvas.width = canvas.height = 8;
    const ctx = canvas.getContext('2d');
    ctx.fillStyle = '#e76b8a';
    ctx.fillRect(0, 0, 8, 8);
    return [...new Uint8Array(await (await new Promise(resolve => canvas.toBlob(resolve))).arrayBuffer())];
  });
}
async function chooseImage(page, bytes) {
  await page.locator('input[type="file"][accept^="image/"]').setInputFiles({ name: 'theme-fixture.png', mimeType: 'image/png', buffer: Buffer.from(bytes) });
}
async function delayImageAnalysis(page) {
  await page.evaluate(() => {
    const originalPut = IDBObjectStore.prototype.put;
    window.themeFixture.pendingBlobKey = null;
    IDBObjectStore.prototype.put = function (...args) {
      if (this.name === 'images') {
        window.themeFixture.pendingBlobKey = args[1];
        IDBObjectStore.prototype.put = originalPut;
      }
      return originalPut.apply(this, args);
    };
    const original = window.createImageBitmap;
    window.createImageBitmap = async (...args) => {
      window.createImageBitmap = original;
      await new Promise(resolve => { window.themeFixture.releaseImage = resolve; });
      return original(...args);
    };
  });
}
async function releaseImage(page) {
  await page.evaluate(() => window.themeFixture.releaseImage());
}

async function appCases(browser, theme) {
  const { context, page, errors } = await fixture(browser, theme);
  try {
    assert.equal(await page.evaluate(async () => {
      const { manualAccent, pickAccent } = await import('/utils/accent.ts');
      return manualAccent({ customAccent: '', themePreset: 'moss' }) === '#41c98b'
        && pickAccent('#41c98b', 'invalid') === '#41c98b';
    }), true);
    await palette(page, '#41c98b');
    await openAppearance(page);
    await page.getByRole('button', { name: '#8b7cff', exact: true }).click();
    await palette(page, '#8b7cff');
    await page.getByRole('combobox', { name: 'Theme', exact: true }).selectOption(theme === 'light' ? 'dark' : 'light');
    await palette(page, '#8b7cff');
    await closeSettings(page, true);
    await palette(page, '#41c98b');
    assert.equal(await page.evaluate(() => document.documentElement.dataset.theme), theme);
    await openAppearance(page);
    await page.getByRole('button', { name: '#8b7cff', exact: true }).click();
    await save(page);
    await closeSettings(page);
    await palette(page, '#8b7cff');
    await openAppearance(page);
    await page.getByRole('checkbox', { name: /^Enable the seasonal theme/ }).check();
    for (const [season, accent] of [['summer', '#2aa6d6'], ['autumn', '#d97742'], ['winter', '#6f9fd8'], ['spring', '#e58fb2']]) {
      await page.getByRole('button', { name: season, exact: true }).click();
      await palette(page, accent);
    }
    await page.getByRole('button', { name: '#f19b52', exact: true }).click();
    await palette(page, '#e58fb2');
    await page.getByRole('radio', { name: '日本語', exact: true }).check();
    await palette(page, '#e58fb2');
    await page.getByRole('radio', { name: 'English', exact: true }).check();
    await closeSettings(page);
    await palette(page, '#e58fb2');
    await openAppearance(page);
    await page.getByRole('combobox', { name: 'Theme', exact: true }).selectOption(theme === 'light' ? 'dark' : 'light');
    await palette(page, '#e58fb2');
    await closeSettings(page, true);
    await palette(page, '#e58fb2');
    await page.getByRole('button', { name: 'Launcher plugins', exact: true }).click();
    const card = page.locator('article').filter({ hasText: 'Aria with You' });
    const toggle = card.getByRole('switch');
    await toggle.click();
    await palette(page, '#8b7cff');
    await toggle.click();
    await palette(page, '#e58fb2');
    await toggle.click();
    await palette(page, '#8b7cff');
    await page.getByRole('button', { name: 'Home', exact: true }).click();
    const bytes = await imageFile(page);
    await openAppearance(page);
    await page.getByRole('switch', { name: 'Match background colours', exact: true }).click();
    await chooseImage(page, bytes);
    await page.waitForFunction(() => JSON.parse(localStorage.getItem('mfmcl-background') || '{}').images?.length === 1);
    const auto = await page.evaluate(() => JSON.parse(localStorage.getItem('mfmcl-background')).images[0].accent);
    await palette(page, auto);
    await page.getByRole('button', { name: '#f19b52', exact: true }).click();
    await palette(page, auto);
    await save(page);
    await closeSettings(page, true);
    await palette(page, auto);
    await page.getByRole('button', { name: 'Library', exact: true }).click();
    await palette(page, auto);
    await page.getByRole('button', { name: 'Home', exact: true }).click();
    await palette(page, auto);
    await page.reload();
    await page.waitForFunction(() => window.__vite_plugin_react_preamble_installed__);
    await page.evaluate(async () => {
      window.themeFixture = { delayInitial: false, initialReads: [] };
      const { default: R } = await import('/node_modules/.vite/deps/react.js');
      const { default: client } = await import('/node_modules/.vite/deps/react-dom_client.js');
      const { I18nProvider } = await import('/utils/i18n/I18nProvider.tsx');
      const { default: App } = await import('/src/App.tsx');
      client.createRoot(document.querySelector('#root')).render(R.createElement(R.StrictMode, null, R.createElement(I18nProvider, { initialPreference: 'en' }, R.createElement(App))));
    });
    await page.getByRole('button', { name: 'Settings', exact: true }).waitFor();
    await palette(page, auto);
    await openAppearance(page);
    await page.getByRole('checkbox', { name: /^Enable the seasonal theme/ }).check();
    await palette(page, '#e58fb2');
    await closeSettings(page);
    await palette(page, '#e58fb2');
    await openAppearance(page);
    await page.getByRole('checkbox', { name: /^Enable the seasonal theme/ }).uncheck();
    await palette(page, auto);
    await page.getByRole('switch', { name: 'Match background colours', exact: true }).click();
    await palette(page, '#f19b52');
    await page.getByRole('button', { name: 'Advanced', exact: true }).click();
    await page.getByRole('button', { name: 'Restore defaults for this page', exact: true }).click();
    await palette(page, '#53b4f6');
    await save(page);
    await closeSettings(page);
    await palette(page, '#53b4f6');
    assert.deepEqual(errors, []);
    console.log(`PASS ${theme}: manual preview/save/cancel, seasonal priority, plugin toggle, language/theme switches, auto background, return/refresh, reset, derived-token contrast`);
  } finally { await context.close(); }
}

async function asyncCases(browser) {
  const { context, page, errors } = await fixture(browser, 'dark', true);
  try {
    await page.waitForFunction(() => window.themeFixture.initialReads.length > 0);
    await openAppearance(page);
    await page.getByRole('button', { name: '#8b7cff', exact: true }).click();
    await save(page);
    await closeSettings(page);
    await page.evaluate(() => { window.themeFixture.delayInitial = false; window.themeFixture.initialReads.forEach(resolve => resolve()); });
    await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    await palette(page, '#8b7cff');
    const bytes = await imageFile(page);
    await openAppearance(page);
    await page.getByRole('switch', { name: 'Match background colours', exact: true }).click();
    await delayImageAnalysis(page);
    await chooseImage(page, bytes);
    await page.waitForFunction(() => typeof window.themeFixture.releaseImage === 'function');
    await page.getByRole('switch', { name: 'Match background colours', exact: true }).click();
    await page.getByRole('button', { name: '#f19b52', exact: true }).click();
    await page.getByRole('switch', { name: 'Blur background', exact: true }).click();
    await releaseImage(page);
    await page.waitForFunction(() => JSON.parse(localStorage.getItem('mfmcl-background') || '{}').images?.length === 1);
    assert.equal(await page.evaluate(() => JSON.parse(localStorage.getItem('mfmcl-background')).accentMode), 'manual');
    assert.equal(await page.evaluate(() => JSON.parse(localStorage.getItem('mfmcl-background')).blur), true);
    await palette(page, '#f19b52');
    await save(page);
    await delayImageAnalysis(page);
    await chooseImage(page, bytes);
    await page.getByRole('radio', { name: 'None', exact: true }).click();
    await releaseImage(page);
    await page.getByRole('button', { name: 'Add images', exact: true }).waitFor({ state: 'visible' });
    await page.waitForFunction(() => !document.querySelector('button[class*="_button_"]:disabled'));
    assert.equal(await page.evaluate(() => JSON.parse(localStorage.getItem('mfmcl-background')).mode), 'none');
    await palette(page, '#f19b52');
    const before = await page.evaluate(() => localStorage.getItem('mfmcl-background'));
    await delayImageAnalysis(page);
    await chooseImage(page, bytes);
    await closeSettings(page, true);
    await releaseImage(page);
    await page.waitForFunction(async () => {
      const id = window.themeFixture.pendingBlobKey;
      return id && !(await (await import('/utils/background.ts')).readBlob('images', id));
    });
    assert.equal(await page.evaluate(() => localStorage.getItem('mfmcl-background')), before);
    await palette(page, '#f19b52');
    assert.deepEqual(errors, []);
    console.log('PASS async: delayed initial snapshot cannot overwrite save; late analysis preserves manual/blur; mode switch/unmount invalidate import');
  } finally { await context.close(); }
}

(async () => {
  const browser = await chromium.launch(process.env.MFMCL_BROWSER_CHANNEL ? { channel: process.env.MFMCL_BROWSER_CHANNEL } : {});
  try {
    for (const theme of ['light', 'dark']) await appCases(browser, theme);
    await asyncCases(browser);
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
