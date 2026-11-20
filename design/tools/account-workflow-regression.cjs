const assert = require('node:assert/strict');
const { chromium } = require('playwright');
const base = process.argv[2] || 'http://127.0.0.1:5173';
const html = '<!doctype html><html><head><script type="module">import R from "/@react-refresh";R.injectIntoGlobalHook(window);window.$RefreshReg$=()=>{};window.$RefreshSig$=()=>type=>type;window.__vite_plugin_react_preamble_installed__=true;</script></head><body><div id="root"></div></body></html>';

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    const page = await browser.newPage({ viewport: { width: 1280, height: 900 }, reducedMotion: 'reduce' });
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.route('**/*', route => new URL(route.request().url()).origin === new URL(base).origin ? route.continue() : route.abort());
    await page.route('**/__account_test__', route => route.fulfill({ contentType: 'text/html', body: html }));
    await page.route(/\/utils\/confirm\.ts(?:\?t=\d+)?$/, route => route.fulfill({ contentType: 'text/javascript', body: 'export const confirmAction = async () => true;' }));
    await page.route(/\/utils\/native\.ts(?:\?t=\d+)?$/, route => route.fulfill({ contentType: 'text/javascript', body: `
      export * from '/utils/native.ts?account-test-base';
      export const selectAccount = async id => { window.test.selected = id; };
      export const removeAccount = async id => { window.test.removed = id; };
      export const cancelSkinLogin = async id => { window.test.cancelled.push(id); };
      export const skinLogin = async (server, username, _password, confirmedServer) => {
        window.test.auth.push({ server, username, confirmedServer });
        const result = { pendingId: String(window.test.auth.length), profiles: [{ id: 'one', name: 'Role One' }, { id: 'two', name: 'Role Two' }] };
        if (window.test.delayLogin) await new Promise(resolve => { window.test.releaseLogin = resolve; });
        return result;
      };
      export const skinSelectProfile = async () => { throw new Error('Session expired'); };
    ` }));
    await page.goto(`${base}/__account_test__`);
    await page.waitForFunction(() => window.__vite_plugin_react_preamble_installed__);
    await page.evaluate(async () => {
      const { default: React } = await import('/node_modules/.vite/deps/react.js');
      const { default: dom } = await import('/node_modules/.vite/deps/react-dom_client.js');
      const { I18nProvider } = await import('/utils/i18n/I18nProvider.tsx');
      const { AccountModal } = await import('/components/AccountModal.tsx');
      const root = dom.createRoot(document.querySelector('#root'));
      const first = { id: 'first', username: 'FirstPlayer', uuid: 'first', type: 'offline', hasMinecraft: false };
      const second = { ...first, id: 'second', username: 'SecondPlayer', uuid: 'second' };
      window.test = { selected: first.id, auth: [], cancelled: [], notices: [] };
      const render = (isOpen, current = window.test.selected === second.id ? second : first) => root.render(React.createElement(I18nProvider, { initialPreference: 'en' }, React.createElement(AccountModal, {
        isOpen, currentAccount: current, recentAccounts: [current === first ? second : first],
        onClose: () => render(false), onLogin: account => { window.test.current = account?.id; }, onNotice: text => window.test.notices.push(text),
      })));
      window.test.open = () => render(true);
      window.test.open();
    });
    const dialog = page.getByRole('dialog');
    const use = dialog.getByRole('button', { name: 'Switch to this account · SecondPlayer' });
    await use.waitFor();
    let focused = false;
    for (let count = 0; count < 20 && !focused; count++) {
      await page.keyboard.press('Tab');
      focused = await use.evaluate(el => el === document.activeElement);
    }
    assert.equal(focused, true, 'saved-account action is in the keyboard tab order without hovering');
    await page.keyboard.press('Enter');
    await dialog.waitFor({ state: 'hidden' });
    assert.deepEqual(await page.evaluate(() => [window.test.selected, window.test.current]), ['second', 'second']);
    await page.evaluate(() => window.test.open());
    const edit = dialog.getByRole('button', { name: 'Edit · FirstPlayer' });
    await edit.focus();
    await page.keyboard.press('Enter');
    await dialog.getByRole('button', { name: 'Delete', exact: true }).focus();
    await page.keyboard.press('Enter');
    await page.waitForFunction(() => window.test.removed === 'first');

    const enterSkin = async () => {
      await dialog.getByRole('button', { name: 'Add account', exact: true }).click();
      await dialog.getByRole('button', { name: /^Third-party/ }).last().click();
    };
    await enterSkin();
    const server = dialog.getByRole('textbox', { name: 'Auth server URL' });
    const username = dialog.getByRole('textbox', { name: 'Username' });
    const password = dialog.locator('input[type=password]');
    const trust = dialog.getByRole('checkbox');
    const connect = dialog.getByRole('button', { name: /^Connect to auth server/ });
    await server.fill('https://old.example/api/yggdrasil');
    await username.fill('sample');
    await password.fill('synthetic');
    await trust.check();
    await server.fill('https://New.example:443/api/yggdrasil');
    assert.equal(await trust.isChecked(), false);
    assert.equal(await connect.isDisabled(), true);
    assert.equal(await page.evaluate(() => window.test.auth.length), 0);
    await trust.check();
    await connect.click();
    await dialog.getByRole('button', { name: 'Role One' }).click();
    await server.waitFor();
    assert.equal(await trust.isChecked(), false);
    assert.equal(await password.inputValue(), '');
    assert.deepEqual(await page.evaluate(() => window.test.auth), [{ server: 'https://new.example/api/yggdrasil/', username: 'sample', confirmedServer: 'https://new.example/api/yggdrasil/' }]);
    await page.evaluate(() => { window.test.delayLogin = true; });
    await password.fill('synthetic');
    await trust.check();
    await connect.click();
    await page.waitForFunction(() => Boolean(window.test.releaseLogin));
    await dialog.getByRole('button', { name: 'Back', exact: true }).click();
    await enterSkin();
    await page.evaluate(() => window.test.releaseLogin());
    await page.waitForFunction(() => window.test.cancelled.includes('2'));
    assert.equal(await dialog.getByRole('button', { name: 'Role One' }).count(), 0);
    assert.equal(await trust.isChecked(), false);
    await server.waitFor();
    assert.deepEqual(errors, []);
    console.log('PASS account workflows: keyboard selection and deletion; URL change clears consent; expired and late role responses cannot trap a new sign-in');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
