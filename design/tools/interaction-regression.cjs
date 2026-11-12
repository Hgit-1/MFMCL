const assert = require('node:assert/strict');
const { chromium } = require('playwright');
const BASE = process.argv[2] || 'http://127.0.0.1:5173';

async function prepare(page, theme) {
  await page.route('**/__mfmcl-component-test__', route => route.fulfill({
    contentType: 'text/html',
    body: '<!doctype html><html><head><script type="module">import R from "/@react-refresh"; R.injectIntoGlobalHook(window);window.$RefreshReg$=()=>{};window.$RefreshSig$=()=>type=>type;window.__vite_plugin_react_preamble_installed__=true;</script></head><body><button id="opener">Open test</button><div id="root"></div></body></html>',
  }));
  await page.goto(`${BASE}/__mfmcl-component-test__`);
  await page.waitForFunction(() => window.__vite_plugin_react_preamble_installed__);
  await page.evaluate(async theme => {
    await import('/styles/globals.css');
    document.documentElement.dataset.theme = theme;
    const { default: React } = await import('/node_modules/.vite/deps/react.js');
    const { default: client } = await import('/node_modules/.vite/deps/react-dom_client.js');
    const { I18nProvider } = await import('/utils/i18n/I18nProvider.tsx');
    const prefs = await import('/utils/launcherPrefs.ts');
    window.fixture = { React, root: client.createRoot(document.querySelector('#root')), I18nProvider, prefs };
  }, theme);
}

async function settingsCase(page) {
  await page.locator('#opener').focus();
  await page.evaluate(async () => {
    const { React: R, root, I18nProvider, prefs } = window.fixture;
    const { SettingsPanel } = await import('/components/SettingsPanel.tsx');
    window.fixture.initial = { ...prefs.DEFAULT_PREFS, customAccent: '#53b4f6', windowWidth: 1280, windowHeight: 720 };
    window.fixture.mode = 'fail';
    window.fixture.saved = [];
    window.fixture.javaChanged = [];
    window.fixture.openSettings = () => root.render(R.createElement(I18nProvider, { initialPreference: 'zh-CN' }, R.createElement(SettingsPanel, {
      theme: 'dark', initialTab: 'game', initialPrefs: window.fixture.initial,
      initialJavaRuntimes: [], onThemeChange() {},
      onClose: () => root.render(null),
      onJavaRuntimesChange: value => window.fixture.javaChanged.push(value),
      onSaveSettings: async value => {
        if (window.fixture.mode === 'fail') throw new Error('fixture rejected');
        window.fixture.saved.push(value);
      },
    })));
    window.fixture.openSettings();
  });
  const dialog = page.getByRole('dialog');
  await dialog.waitFor();
  const width = dialog.getByRole('spinbutton').first();
  await width.fill('100');
  await dialog.getByRole('button', { name: '保存设置', exact: true }).click();
  await dialog.getByRole('alert').filter({ hasText: 'fixture rejected' }).waitFor();
  assert.equal(await page.evaluate(() => localStorage.getItem('flowlaunch-settings')), null);
  assert.equal(await page.evaluate(() => window.fixture.javaChanged.length), 0);
  await page.keyboard.press('Escape');
  await dialog.waitFor({ state: 'hidden' });
  assert.equal(await page.evaluate(() => document.activeElement.id), 'opener');
  await page.evaluate(() => { window.fixture.mode = 'success'; window.fixture.openSettings(); });
  await dialog.waitFor();
  assert.equal(await width.inputValue(), '1280');
  await width.fill('1440');
  await dialog.getByRole('button', { name: '保存设置', exact: true }).click();
  await page.waitForFunction(() => JSON.parse(localStorage.getItem('flowlaunch-settings') || '{}').windowWidth === 1440);
  assert.equal(await page.evaluate(() => window.fixture.saved.length), 1);
  assert.equal(await dialog.getByRole('button', { name: '已保存', exact: true }).isDisabled(), true);
  await width.fill('1450');
  await width.fill('1440');
  assert.equal(await dialog.getByRole('button', { name: '已保存', exact: true }).isDisabled(), true);
  for (let i = 0; i < 35; i++) {
    await page.keyboard.press('Tab');
    assert.ok(await page.evaluate(() => document.querySelector('[role="dialog"]').contains(document.activeElement)));
  }
  assert.equal(await page.locator('#opener').evaluate(el => el.inert), true);
  await page.keyboard.press('Escape');
  await dialog.waitFor({ state: 'hidden' });
  assert.equal(await page.locator('#opener').evaluate(el => el.inert), false);
  assert.equal(await page.evaluate(() => document.activeElement.id), 'opener');
}

async function nestedModalCase(page) {
  await page.locator('#opener').focus();
  await page.evaluate(async () => {
    const { React: R, root, I18nProvider } = window.fixture;
    const { Modal } = await import('/components/Modal.tsx');
    const { Input } = await import('/components/Input.tsx');
    function Test() {
      const [open, setOpen] = R.useState(true);
      const [nested, setNested] = R.useState(false);
      return R.createElement(I18nProvider, { initialPreference: 'en' },
        R.createElement(Modal, { isOpen: open, title: 'Outer', onClose: () => setOpen(false) },
          R.createElement(Input, { label: 'Profile name', error: 'Required', hint: 'Test hint' }),
          R.createElement('button', { id: 'nested-open', onClick: () => setNested(true) }, 'Nested'),
          R.createElement(Modal, { isOpen: nested, title: 'Inner', onClose: () => setNested(false) }, R.createElement('button', null, 'Inner action'))));
    }
    root.render(R.createElement(R.StrictMode, null, R.createElement(Test)));
  });
  const outer = page.getByRole('dialog', { name: 'Outer', exact: true });
  await outer.waitFor();
  const input = outer.getByRole('textbox', { name: 'Profile name' });
  assert.equal(await input.getAttribute('aria-invalid'), 'true');
  const description = await input.getAttribute('aria-describedby');
  assert.ok(description);
  await page.getByText('Profile name', { exact: true }).click();
  assert.equal(await input.evaluate(el => el === document.activeElement), true);
  await page.getByRole('button', { name: 'Nested', exact: true }).click();
  const inner = page.getByRole('dialog', { name: 'Inner', exact: true });
  await inner.waitFor();
  await page.keyboard.press('Escape');
  await inner.waitFor({ state: 'hidden' });
  assert.equal(await outer.isVisible(), true);
  assert.equal(await page.evaluate(() => document.activeElement.id), 'nested-open');
  await page.keyboard.press('Escape');
  await outer.waitFor({ state: 'hidden' });
  assert.equal(await page.evaluate(() => document.activeElement.id), 'opener');
}

async function appCases(browser) {
  const context = await browser.newContext({ viewport: { width: 1280, height: 820 } });
  const page = await context.newPage();
  await page.route(/\/utils\/eula\.ts(?:\?t=\d+)?$/, route => route.fulfill({ contentType: 'text/javascript', body: 'export const hasAcceptedEula = () => true; export const acceptEula = () => {};' }));
  await page.route(/\/utils\/onboarding\.ts(?:\?t=\d+)?$/, route => route.fulfill({ contentType: 'text/javascript', body: 'export const isOnboardingPending = () => false; export const markOnboardingComplete = () => {}; export const resetOnboarding = () => {};' }));
  await page.route(/\/utils\/native\.ts(?:\?t=\d+)?$/, route => route.fulfill({ contentType: 'text/javascript', body: `
    export * from '/utils/native.ts?fixture-base';
    const initial = { id:'fixture-uninstalled', name:'New game', gameVersion:'1.21.1', loader:'vanilla', instanceDir:'/fixture/instance', gameDir:'/fixture/instance/game', isolated:true, state:'not_installed', createdAt:'2026-01-01' };
    window.appFixture = { records:[initial], clears:0, deleted:0 };
    export const listInstances = async () => window.appFixture.records;
    export const listMods = async () => [];
    export const listAccounts = async () => [];
    export const listJavaRuntimes = async () => [];
    export const getSettings = async () => ({ ...(await import('/utils/native.ts?fixture-base')).fallbackSettings(), clearFinishedTasks:false });
    export const listDownloadTasks = async () => [
      ...(window.appFixture.clears ? [] : [{ id:'done', instanceId:initial.id, label:'Completed fixture', state:'completed', completedBytes:10, totalBytes:10, createdAt:'', updatedAt:'2026-01-02' }]),
      { id:'paused', instanceId:initial.id, label:'Paused fixture', state:'paused', completedBytes:5, totalBytes:10, createdAt:'', updatedAt:'2026-01-01' },
      { id:'running', instanceId:initial.id, label:'Running fixture', state:'running', completedBytes:1, totalBytes:10, createdAt:'', updatedAt:'' },
    ];
    export const clearFinishedDownloadTasks = async () => { window.appFixture.clears++; return 1; };
    export const previewFileProgress = () => [];
    export const deleteInstance = async () => { window.appFixture.records=[]; window.appFixture.deleted++; };
  ` }));
  await prepare(page, 'light');
  await page.evaluate(async () => {
    const { React: R, root, I18nProvider } = window.fixture;
    const target = await import('/src/App.tsx');
    const App = target.default;
    root.render(R.createElement(I18nProvider, { initialPreference:'en' }, R.createElement(App)));
  });
  await page.waitForFunction(() => window.appFixture?.records.length === 1);
  await page.getByRole('heading', { name:'New game', exact:true }).waitFor();
  await page.getByRole('button', { name:'Library', exact:true }).click();
  await page.getByRole('button', { name:'Install', exact:true }).waitFor();
  assert.equal(await page.getByRole('button', { name:'Play', exact:true }).count(), 0);
  await page.getByRole('button', { name:'Home', exact:true }).click();
  await page.getByRole('button', { name:'Install', exact:true }).waitFor();
  console.log('PASS F02 uninstalled records use install actions in library and dashboard');
  await page.getByRole('button', { name:/^Downloads/ }).click();
  await page.getByText('Paused fixture', { exact:true }).waitFor();
  /*
   * 进行中的任务必须排在已结束之前，即使后端按 updated_at 把刚结束的排在前面。
   * fixture 故意让 Completed 的时间戳最新、Running 最旧。
   */
  assert.deepEqual(await page.evaluate(() => {
    const labels = ['Running fixture', 'Paused fixture', 'Completed fixture'];
    return [...document.querySelectorAll('h3')]
      .map(node => node.textContent?.trim() ?? '')
      .filter(text => labels.includes(text));
  }), ['Running fixture', 'Paused fixture', 'Completed fixture']);
  console.log('PASS download list puts running first, then paused, then finished');
  await page.getByRole('button', { name:/Clear finished/ }).click();
  await page.getByText('Completed fixture', { exact:true }).waitFor({ state:'hidden' });
  assert.equal(await page.getByText('Paused fixture', { exact:true }).isVisible(), true);
  assert.equal(await page.getByText('Running fixture', { exact:true }).isVisible(), true);
  console.log('PASS F09 clearing finished tasks retains paused task and resume access');
  await page.getByRole('button', { name:'Library', exact:true }).click();
  await page.getByRole('button', { name:'Manage', exact:true }).click();
  const deleteButton = page.getByRole('button', { name:/Delete instance/ }).first();
  await deleteButton.click();
  const confirmation = page.getByRole('dialog');
  await confirmation.waitFor();
  await confirmation.getByRole('button', { name:/Delete/ }).click();
  await confirmation.waitFor({ state:'hidden' });
  await page.getByRole('heading', { name:/No games|No instances|library/i }).first().waitFor();
  assert.equal(await page.evaluate(() => window.appFixture.deleted), 1);
  console.log('PASS F10 deleting current final instance returns to library empty state');
  await context.close();
}

(async () => {
  const browser = await chromium.launch(process.env.MFMCL_BROWSER_CHANNEL ? { channel: process.env.MFMCL_BROWSER_CHANNEL } : {});
  try {
    for (const theme of ['light', 'dark']) {
      const context = await browser.newContext({ viewport: { width: 1280, height: 820 } });
      const page = await context.newPage();
      const errors = [];
      page.on('pageerror', error => errors.push(error.message));
      await prepare(page, theme);
      await settingsCase(page);
      console.log(`PASS F04/F11 settings save failure, confirmed draft, dirty state, focus ${theme}`);
      await nestedModalCase(page);
      console.log(`PASS F11 nested Escape, focus restoration, label/error association ${theme}`);
      assert.deepEqual(errors, []);
      await context.close();
    }
    await appCases(browser);
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
