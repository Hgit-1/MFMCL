const assert = require('node:assert/strict');
const { chromium } = require('playwright');
const BASE = process.argv[2] || 'http://127.0.0.1:5173';
const RECORD = {
  id: 'aria-with-you', version: '1.0.0-test', directory: 'aria-with-you',
  path: 'C:/synthetic-plugin-fixture/aria-with-you',
  assetBase: 'http://asset.localhost/synthetic-install-fixture/aria-with-you',
  manifest: {
    name: 'Aria with You', description: 'Synthetic install regression fixture', author: 'Regression tests',
    glyph: 'palette', accent: '#e58fb2', hooks: ['theme', 'sidebar', 'background'],
    permissions: ['theme', 'uiSlot', 'storage'], enabledByDefault: false,
  },
};
const SVG = '<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="#e76b8a"/></svg>';
const HTML = '<!doctype html><html><head><script type="module">import R from "/@react-refresh";R.injectIntoGlobalHook(window);window.$RefreshReg$=()=>{};window.$RefreshSig$=()=>type=>type;window.__vite_plugin_react_preamble_installed__=true;</script></head><body><div id="root"></div></body></html>';

async function fixture(browser, options = {}) {
  const context = await browser.newContext({ viewport: { width: 1280, height: 900 }, reducedMotion: 'reduce', serviceWorkers: 'block' });
  const page = await context.newPage();
  const errors = [], assets = [], images = [], remote = [], publicAssets = [];
  const policy = { failAssets: false };
  page.on('pageerror', error => errors.push(error.message));
  page.on('request', request => { if (request.resourceType() === 'image') images.push(request.url()); });
  page.on('dialog', dialog => dialog.accept());
  await context.route('**/*', route => {
    const url = new URL(route.request().url());
    if (/\/(?:public\/)?plugins\//.test(url.pathname)) {
      publicAssets.push(url.href);
      return route.abort();
    }
    if (url.hostname === 'asset.localhost') {
      assets.push(url.href);
      return route.fulfill({ status: policy.failAssets ? 404 : 200, contentType: 'image/svg+xml', body: policy.failAssets ? '' : SVG });
    }
    if (url.origin !== new URL(BASE).origin) {
      remote.push(url.href);
      return route.abort();
    }
    // Unexpected image requests must never read any artwork from the workspace.
    if (route.request().resourceType() === 'image') return route.fulfill({ contentType: 'image/svg+xml', body: SVG });
    return route.continue();
  });
  await page.route('**/__mfmcl-plugin-install-test__', route => route.fulfill({ contentType: 'text/html', body: HTML }));
  await page.route(/\/utils\/eula\.ts(?:\?t=\d+)?$/, route => route.fulfill({ contentType: 'text/javascript', body: 'export const hasAcceptedEula=()=>true;export const acceptEula=()=>{};' }));
  await page.route(/\/utils\/onboarding\.ts(?:\?t=\d+)?$/, route => route.fulfill({ contentType: 'text/javascript', body: 'export const isOnboardingPending=()=>false;export const markOnboardingComplete=()=>{};export const resetOnboarding=()=>{};' }));
  await page.route(/\/utils\/native\.ts(?:\?t=\d+)?$/, route => route.fulfill({ contentType: 'text/javascript', body: `
    export * from '/utils/native.ts?plugin-test-base';
    export const listInstances = async () => [];
    export const listAccounts = async () => [];
    export const listMods = async () => [];
    export const listDownloadTasks = async () => [];
    export const previewFileProgress = () => [];
    export const clearFinishedDownloadTasks = async () => 0;
  ` }));
  await page.route(/\/utils\/pluginInstall\.ts(?:\?t=\d+)?$/, route => route.fulfill({ contentType: 'text/javascript', body: `
    export * from '/utils/pluginInstall.ts?plugin-test-base';
    export const listInstalledPlugins = async () => {
      const state = window.pluginFixture;
      state.scans++;
      try {
        if (state.holdScan) await new Promise(resolve => state.releaseScans.push(resolve));
        if (state.scanFailure) { state.failedScans++; throw new Error('Synthetic backend scan failure'); }
        return structuredClone(state.records);
      } finally { state.settledScans++; }
    };
    export const reloadPlugins = listInstalledPlugins;
    export const removeInstalledPlugin = async id => {
      window.pluginFixture.removed.push(id);
      window.pluginFixture.records = window.pluginFixture.records.filter(record => record.id !== id);
    };
  ` }));
  await page.goto(`${BASE}/__mfmcl-plugin-install-test__`);
  await page.waitForFunction(() => window.__vite_plugin_react_preamble_installed__);
  await page.evaluate(async ({ options, record }) => {
    const plugins = await import('/utils/plugins.ts');
    const aria = await import('/utils/aria.ts');
    const prefs = await import('/utils/launcherPrefs.ts');
    const native = await import('/utils/native.ts?plugin-test-base');
    window.pluginFixture = {
      records: options.installed ? [record] : [], scanFailure: Boolean(options.scanFailure), holdScan: Boolean(options.holdScan),
      scans: 0, settledScans: 0, failedScans: 0, releaseScans: [], removed: [], unavailable: 0,
    };
    if (options.stale) {
      const cached = plugins.pluginFromInstalled(record);
      plugins.savePluginStore({
        ...plugins.EMPTY_PLUGIN_STORE, installed: [cached], enabled: { [record.id]: true },
        custom: [{ ...cached, builtin: true, bundled: true, thirdParty: true }],
      });
    } else if (options.installed) {
      plugins.savePluginStore({ ...plugins.EMPTY_PLUGIN_STORE, enabled: { [record.id]: false } });
    }
    prefs.savePrefs({ ...prefs.DEFAULT_PREFS, theme: 'light', customAccent: '#53b4f6' });
    await native.saveSettings({ ...native.fallbackSettings(), themeMode: 'light', customAccent: '#53b4f6', clearFinishedTasks: false });
    aria.saveAriaSetting({ ...aria.DEFAULT_ARIA, enabled: true, mode: 'fixed', fixedSeason: 'spring' });
    const { default: R } = await import('/node_modules/.vite/deps/react.js');
    const { default: client } = await import('/node_modules/.vite/deps/react-dom_client.js');
    const { I18nProvider } = await import('/utils/i18n/I18nProvider.tsx');
    const state = window.pluginFixture;
    state.root = client.createRoot(document.querySelector('#root'));
    state.render = element => state.root.render(R.createElement(I18nProvider, { initialPreference: 'en' }, element));
    state.mountApp = async () => {
      const { default: App } = await import('/src/App.tsx');
      state.render(R.createElement(R.StrictMode, null, R.createElement(App)));
    };
  }, { options, record: RECORD });
  return { context, page, errors, assets, images, remote, publicAssets, policy };
}

async function settle(page) {
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
}
function clean(fixture) {
  assert.deepEqual(fixture.errors, [], 'no uncaught frontend errors');
  assert.deepEqual(fixture.remote, [], 'no remote requests');
  assert.deepEqual(fixture.publicAssets, [], 'no public plugin artwork requests');
}

async function modelCases(browser) {
  const f = await fixture(browser);
  try {
    const checks = await f.page.evaluate(async record => {
      const p = await import('/utils/plugins.ts');
      const a = await import('/utils/aria.ts');
      const checks = [];
      const check = (label, actual, expected) => checks.push({ label, actual, expected });
      const empty = () => ({ custom: [], installed: [], enabled: {}, options: {}, acknowledgedUpdates: {} });
      const ids = store => p.allPlugins(store).map(plugin => plugin.id);
      const fresh = p.loadPluginStore();
      check('fresh store has exactly the two builtins', ids(fresh), ['launch-logger', 'download-mirror']);
      check('fresh installed count', p.installedPluginCount(fresh), 2);
      check('fresh enabled count', p.enabledPluginCount(fresh), 2);
      check('fresh entries are builtin and not thirdparty', p.allPlugins(fresh).every(plugin => plugin.builtin && !plugin.thirdParty), true);
      check('no bundled thirdparty catalogue', p.BUNDLED_PLUGINS, []);
      check('no fresh Aria descriptor', a.installedAriaArtwork(fresh), null);
      check('persisted preference alone cannot activate', a.activeAriaArtwork({ ...fresh, enabled: { [record.id]: true } }), null);
      check('Aria enabled preference really is set', a.readAriaSetting().enabled, true);

      const installed = p.pluginFromInstalled(record);
      const manifestOnly = p.pluginFromInstalled({ ...record, assetBase: undefined, manifest: { ...record.manifest, assetBase: record.assetBase } });
      check('manifest cannot grant a missing backend asset base', a.installedAriaArtwork({ ...empty(), installed: [manifestOnly] }), null);
      const conflictingManifest = p.pluginFromInstalled({ ...record, manifest: { ...record.manifest, assetBase: 'https://example.test/forged' } });
      check('backend asset base overrides manifest URL', conflictingManifest.assetBase, record.assetBase);
      const custom = p.normalizePlugin({ ...record.manifest, id: record.id, version: record.version, assetBase: record.assetBase, builtin: true, bundled: true, thirdParty: true });
      check('custom manifest loses asset access', custom.assetBase ?? null, null);
      check('custom manifest cannot claim builtin', custom.builtin, false);
      check('custom manifest cannot claim bundled', Boolean(custom.bundled), false);
      check('custom with forged installed fields cannot activate', a.activeAriaArtwork({ ...empty(), custom: [installed], enabled: { [record.id]: true } }), null);
      p.savePluginStore({ ...empty(), installed: [installed], custom: [custom], enabled: { [record.id]: true } });
      const loaded = p.loadPluginStore();
      check('cached install is not restored', loaded.installed, []);
      check('enabled preference is preserved without granting availability', loaded.enabled[record.id], true);
      check('custom plus cached install cannot provide artwork', a.installedAriaArtwork(loaded), null);
      check('custom plus cached install cannot activate', a.activeAriaArtwork(loaded), null);

      for (const assetBase of [record.assetBase, 'asset://localhost/synthetic-install-fixture/aria-with-you', 'https://asset.localhost/synthetic-install-fixture/aria-with-you']) {
        const plugin = p.pluginFromInstalled({ ...record, assetBase });
        const descriptor = { assetBase, version: record.version };
        const store = { ...empty(), installed: [plugin] };
        check(`installed descriptor ${assetBase}`, a.installedAriaArtwork(store), descriptor);
        check(`default disabled ${assetBase}`, a.activeAriaArtwork(store), null);
        store.enabled[record.id] = true;
        check(`explicitly enabled ${assetBase}`, a.activeAriaArtwork(store), descriptor);
        store.enabled[record.id] = false;
        check(`explicitly disabled ${assetBase}`, a.activeAriaArtwork(store), null);
        for (const season of a.SEASONS) {
          check(`season paths ${assetBase} ${season}`, a.seasonArt(descriptor, season), {
            hero: `${assetBase}/assets/aria-${season}-hero.webp`, portrait: `${assetBase}/assets/aria-${season}-portrait.webp`, thumb: `${assetBase}/assets/aria-${season}-portrait-thumb.webp`,
          });
          for (const page of a.ARIA_PAGES) {
            check(`page paths ${assetBase} ${page} ${season}`, a.pageArt(descriptor, page, season), {
              hero: `${assetBase}/assets/pages/aria-${page}-${season}.webp`, thumb: `${assetBase}/assets/pages/aria-${page}-${season}-thumb.webp`,
            });
          }
        }
      }
      const onByDefault = p.pluginFromInstalled({ ...record, manifest: { ...record.manifest, enabledByDefault: true } });
      check('installed enabled-by-default package activates', Boolean(a.activeAriaArtwork({ ...empty(), installed: [onByDefault] })), true);
      check('explicit disable overrides package default', a.activeAriaArtwork({ ...empty(), installed: [onByDefault], enabled: { [record.id]: false } }), null);
      for (const invalid of ['', '/plugins/aria-with-you', '/public/plugins/aria-with-you', `${location.origin}/plugins/aria-with-you`,
        'https://example.test/aria-with-you', 'http://localhost/aria-with-you', '//asset.localhost/aria-with-you',
        'file:///C:/synthetic/aria-with-you', 'data:image/svg+xml,synthetic', 'javascript:void(0)',
        'https://asset.localhost.example.test/aria-with-you', 'asset://example.test/aria-with-you',
        'http://user:password@asset.localhost/aria-with-you', 'http://asset.localhost:1234/aria-with-you',
        `${record.assetBase}?remote=https://example.test`, `${record.assetBase}#fragment`]) {
        const descriptor = { assetBase: invalid, version: record.version };
        const store = { ...empty(), installed: [p.pluginFromInstalled({ ...record, assetBase: invalid })], enabled: { [record.id]: true } };
        check(`reject installed base ${invalid}`, a.installedAriaArtwork(store), null);
        check(`reject active base ${invalid}`, a.activeAriaArtwork(store), null);
        check(`reject season base ${invalid}`, a.seasonArt(descriptor, 'spring'), null);
        check(`reject page base ${invalid}`, a.pageArt(descriptor, 'library', 'spring'), null);
      }
      for (const flag of [{ builtin: true }, { thirdParty: false }, { incompatible: true }, { planned: true }]) {
        check(`reject unavailable identity ${JSON.stringify(flag)}`, a.activeAriaArtwork({ ...empty(), installed: [{ ...installed, ...flag }], enabled: { [record.id]: true } }), null);
      }
      check('uninstall removes availability despite enabled preference', a.installedAriaArtwork({ ...empty(), enabled: { [record.id]: true } }), null);
      check('uninstall removes activation despite enabled preference', a.activeAriaArtwork({ ...empty(), enabled: { [record.id]: true } }), null);
      check('no descriptor means no season URL', a.seasonArt(null, 'spring'), null);
      check('no descriptor means no page URL', a.pageArt(null, 'library', 'spring'), null);
      return checks;
    }, RECORD);
    for (const { label, actual, expected } of checks) assert.deepEqual(actual, expected, label);
    assert.deepEqual(f.images, []);
    clean(f);
    console.log(`PASS model: ${checks.length} assertions covering two builtins, cache/custom isolation, enabled install, uninstall, local asset protocols, rejected remote/public URLs`);
  } finally { await f.context.close(); }
}

async function componentCases(browser) {
  const f = await fixture(browser);
  const { page } = f;
  try {
    await page.evaluate(async () => {
      const { default: R } = await import('/node_modules/.vite/deps/react.js');
      const { AriaFigure } = await import('/components/AriaFigure.tsx');
      const { AriaSection } = await import('/components/AriaSection.tsx');
      const a = await import('/utils/aria.ts');
      window.pluginFixture.renderComponents = (artwork, enabled) => window.pluginFixture.render(R.createElement(R.Fragment, null,
        R.createElement('div', { id: 'figure' }, R.createElement(AriaFigure, { artwork: enabled ? artwork : null, season: 'spring', variant: 'watermark', page: 'library', onUnavailable: () => window.pluginFixture.unavailable++ })),
        R.createElement('div', { id: 'section' }, R.createElement(AriaSection, { artwork, setting: { ...a.DEFAULT_ARIA, mode: 'fixed' }, pluginEnabled: enabled, dark: false, onChange: () => {} })),
      ));
      window.pluginFixture.renderComponents(undefined, true);
    });
    const master = page.getByRole('checkbox', { name: /^Enable the seasonal theme/ });
    await master.waitFor();
    assert.equal(await master.isDisabled(), true);
    assert.equal(await page.locator('img').count(), 0);
    assert.deepEqual(f.images, [], 'absent descriptor cannot request images');
    const descriptor = { assetBase: RECORD.assetBase, version: RECORD.version };
    await page.evaluate(descriptor => window.pluginFixture.renderComponents(descriptor, false), descriptor);
    await settle(page);
    assert.equal(await master.isDisabled(), false);
    assert.equal(await master.isChecked(), false);
    assert.equal(await page.locator('img').count(), 0);
    assert.deepEqual(f.images, [], 'disabled package cannot request previews or figures');

    async function render(artwork) {
      await page.evaluate(artwork => window.pluginFixture.renderComponents(artwork, true), artwork);
      await settle(page);
      await page.locator('#section').scrollIntoViewIfNeeded();
      await page.evaluate(() => document.querySelectorAll('img[loading="lazy"]').forEach(img => { img.loading = 'eager'; }));
    }
    async function failed(count) {
      await page.waitForFunction(count => window.pluginFixture.unavailable === count && !document.querySelector('img'), count);
    }
    async function recovered(artwork) {
      await page.waitForFunction(() => {
        const imgs = [...document.querySelectorAll('img')];
        return imgs.length === 5 && imgs.every(img => img.complete && img.naturalWidth === 8);
      });
      assert.equal(await page.locator('#figure img').getAttribute('src'), `${artwork.assetBase}/assets/pages/aria-library-spring.webp`, 'update resets fallback source as well as failure');
      assert.equal(await page.locator('#section img').count(), 4, 'all failed seasonal previews retry');
    }
    f.policy.failAssets = true;
    await render(descriptor);
    await failed(1);
    assert.equal(f.assets.filter(url => url.endsWith('/pages/aria-library-spring.webp')).length, 1);
    assert.equal(f.assets.filter(url => url.endsWith('/aria-spring-hero.webp')).length, 1, 'page fallback is attempted once');
    f.policy.failAssets = false;
    const updated = { ...descriptor, version: '2.0.0-test' };
    await render(updated);
    await recovered(updated);
    f.policy.failAssets = true;
    // A distinct failing path avoids Chromium's decoded-image cache from the successful revision.
    const failedRevision = { ...updated, assetBase: `${RECORD.assetBase}-failure` };
    await render(failedRevision);
    await failed(2);
    f.policy.failAssets = false;
    const moved = { ...failedRevision, assetBase: `${RECORD.assetBase}-moved` };
    await render(moved);
    await recovered(moved);
    const requestsBeforeRemoval = f.images.length;
    await page.evaluate(() => window.pluginFixture.renderComponents(null, true));
    await settle(page);
    assert.equal(await page.locator('img').count(), 0);
    assert.equal(f.images.length, requestsBeforeRemoval, 'removal cannot fall back to public artwork');
    clean(f);
    console.log('PASS components: no-package/disabled image gating; one local fallback; package-version and asset-base changes reset figure and four preview failures');
  } finally { await f.context.close(); }
}

async function mountApp(page) {
  await page.evaluate(() => window.pluginFixture.mountApp());
  await page.getByRole('button', { name: 'Settings', exact: true }).waitFor();
}
async function openAppearance(page) {
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('button', { name: 'Appearance', exact: true }).click();
  return page.getByRole('checkbox', { name: /^Enable the seasonal theme/ });
}
async function closeSettings(page) {
  await page.getByRole('button', { name: 'Close settings', exact: true }).click();
  await page.getByRole('dialog').waitFor({ state: 'hidden' });
}
async function noActivation(f) {
  await settle(f.page);
  assert.equal(await f.page.evaluate(() => document.documentElement.dataset.ariaSeason ?? null), null);
  assert.equal(await f.page.locator('figure[data-variant]').count(), 0);
  assert.deepEqual(f.images, [], 'no image requests without an installed, enabled package');
  assert.deepEqual(f.assets, []);
  clean(f);
}

async function startupCases(browser) {
  for (const options of [{}, { stale: true }, { stale: true, scanFailure: true }]) {
    const f = await fixture(browser, { ...options, holdScan: true });
    const { page } = f;
    try {
      await mountApp(page);
      await page.waitForFunction(() => window.pluginFixture.scans > 0);
      await noActivation(f);
      let master = await openAppearance(page);
      assert.equal(await master.count(), 0, 'pending backend scan cannot expose seasonal controls from cache');
      await noActivation(f);
      await page.evaluate(() => {
        window.pluginFixture.holdScan = false;
        window.pluginFixture.releaseScans.forEach(resolve => resolve());
      });
      await page.waitForFunction(() => window.pluginFixture.settledScans === window.pluginFixture.scans);
      await noActivation(f);
      assert.equal(await master.count(), 0, 'empty/failed backend scan keeps seasonal controls unavailable');
      assert.deepEqual(await page.evaluate(() => JSON.parse(localStorage.getItem('flowlaunch-plugins')).installed), []);
      if (options.scanFailure) assert.ok(await page.evaluate(() => window.pluginFixture.failedScans > 0));
      await closeSettings(page);
      await page.getByRole('button', { name: 'Launcher plugins', exact: true }).click();
      await page.locator('article').first().waitFor();
      if (!options.stale) assert.equal(await page.locator('article').count(), 2, 'fresh plugin page contains only two builtins');
      master = await openAppearance(page);
      assert.equal(await master.count(), 0);
      await noActivation(f);
      console.log(`PASS startup: ${options.scanFailure ? 'backend scan rejection with forged cache' : options.stale ? 'empty scan with forged cache/custom and enabled preferences' : 'fresh store, exactly two builtin cards'} stays fail-closed before and after scan`);
    } finally { await f.context.close(); }
  }
}

async function uninstallCase(browser) {
  const f = await fixture(browser, { installed: true });
  const { page } = f;
  try {
    await mountApp(page);
    await page.waitForFunction(() => JSON.parse(localStorage.getItem('flowlaunch-plugins') || '{}').installed?.length === 1);
    await noActivation(f);
    let master = await openAppearance(page);
    assert.equal(await master.isDisabled(), false);
    assert.equal(await master.isChecked(), false);
    await noActivation(f);
    await closeSettings(page);
    await page.getByRole('button', { name: 'Launcher plugins', exact: true }).click();
    const card = page.locator('article').filter({ hasText: 'Aria with You' });
    await card.getByRole('switch').click();
    await page.waitForFunction(() => document.documentElement.dataset.ariaSeason === 'spring');
    await page.waitForFunction(() => document.documentElement.style.getPropertyValue('--color-primary-500') === '#e58fb2');
    await page.waitForFunction(() => [...document.querySelectorAll('figure[data-variant] img')].some(img => img.complete && img.naturalWidth === 8));
    master = await openAppearance(page);
    assert.equal(await master.isChecked(), true);
    await closeSettings(page);
    await card.getByRole('button', { name: 'Remove Aria with You', exact: true }).click();
    await card.waitFor({ state: 'hidden' });
    await page.waitForFunction(() => !document.documentElement.dataset.ariaSeason);
    const requestsAfterRemoval = f.images.length;
    master = await openAppearance(page);
    assert.equal(await master.count(), 0, 'uninstalled package has no seasonal settings section');
    await settle(page);
    assert.equal(await page.locator('figure[data-variant], img[src*="asset.localhost"]').count(), 0);
    assert.equal(f.images.length, requestsAfterRemoval, 'uninstall leaves no new image requests');
    assert.deepEqual(await page.evaluate(() => window.pluginFixture.removed), ['aria-with-you']);
    assert.deepEqual(await page.evaluate(() => window.pluginFixture.records), []);
    assert.deepEqual(await page.evaluate(() => JSON.parse(localStorage.getItem('flowlaunch-plugins')).installed), []);
    clean(f);
    console.log('PASS app: backend-installed package is available but inactive until enabled; plugin toggle activates seasonal palette; confirmed uninstall removes availability and all artwork');
  } finally { await f.context.close(); }
}

(async () => {
  const browser = await chromium.launch(process.env.MFMCL_BROWSER_CHANNEL ? { channel: process.env.MFMCL_BROWSER_CHANNEL } : {});
  try {
    await modelCases(browser);
    await componentCases(browser);
    await startupCases(browser);
    await uninstallCase(browser);
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
