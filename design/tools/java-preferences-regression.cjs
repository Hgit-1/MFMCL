const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require('../flowlaunch-ui/node_modules/typescript');

const UI = path.resolve(__dirname, '../flowlaunch-ui');
const clone = value => JSON.parse(JSON.stringify(value));
const auto = () => ({ mode: 'auto', range: '', folder: '', path: '' });
const range = value => ({ ...auto(), mode: 'range', range: value });
const forced = () => ({ ...auto(), mode: 'forced', path: 'C:\\Java\\bin\\java.exe' });
const dictionaries = Object.fromEntries([['zh-CN', 'zhCN'], ['en', 'en'], ['ja', 'ja']].map(([language, exported]) => {
  const { outputText } = ts.transpileModule(fs.readFileSync(path.join(UI, `utils/i18n/messages/${language}.ts`), 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 },
  });
  const module = { exports: {} };
  vm.runInNewContext(`(function(module,exports){${outputText}\n})`)(module, module.exports);
  return [language, module.exports[exported]];
}));
const translate = (language, key, options = {}) => Object.entries(options.values ?? {}).reduce(
  (text, [name, value]) => text.split(`{${name}}`).join(String(value)), dictionaries[language][key] ?? key,
);
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};

function timers() {
  let now = 0, sequence = 0;
  const tasks = new Map();
  return {
    setTimeout(callback, delay) { const id = ++sequence; tasks.set(id, { callback, at: now + delay }); return id; },
    clearTimeout(id) { tasks.delete(id); },
    advance(delay) {
      now += delay;
      for (;;) {
        const next = [...tasks].filter(([, task]) => task.at <= now).sort((a, b) => a[1].at - b[1].at)[0];
        if (!next) return;
        tasks.delete(next[0]);
        next[1].callback();
      }
    },
  };
}

// This small hook host executes the real component, effects and subscriptions without a DOM or Vite server.
function hookHost() {
  let current;
  const sameDeps = (a, b) => a && b && a.length === b.length && a.every((value, index) => Object.is(value, b[index]));
  const react = {
    useState(initial) {
      const host = current, index = host.index++;
      if (!host.slots[index]) host.slots[index] = { value: typeof initial === 'function' ? initial() : initial };
      const slot = host.slots[index];
      return [slot.value, value => {
        const next = typeof value === 'function' ? value(slot.value) : value;
        if (host.mounted && !Object.is(next, slot.value)) { slot.value = next; host.dirty = true; }
      }];
    },
    useRef(initial) {
      const host = current, index = host.index++;
      if (!host.slots[index]) host.slots[index] = { current: initial };
      return host.slots[index];
    },
    useEffect(effect, deps) {
      const host = current, index = host.index++;
      const old = host.slots[index];
      if (!old || !sameDeps(old.deps, deps)) {
        host.effects.push(() => {
          old?.cleanup?.();
          host.slots[index] = { deps, cleanup: effect() };
        });
      }
    },
    useSyncExternalStore(subscribe, snapshot) {
      const [, rerender] = react.useState(0);
      react.useEffect(() => subscribe(() => rerender(value => value + 1)), [subscribe]);
      return snapshot();
    },
  };
  react.useLayoutEffect = react.useEffect;
  let nextId = 0;
  react.useId = () => react.useRef(`fixture-id-${++nextId}`).current;
  return {
    react,
    mount(component, props) {
      const host = {
        slots: [], index: 0, effects: [], dirty: true, mounted: true, props, tree: null,
        render(nextProps) {
          if (nextProps) { this.props = nextProps; this.dirty = true; }
          let turns = 0;
          while (this.mounted && this.dirty) {
            assert.ok(++turns < 50, 'component render/effect loop');
            this.dirty = false; this.index = 0; this.effects = [];
            current = this;
            this.tree = component(this.props);
            current = undefined;
            for (const effect of this.effects) effect();
          }
        },
        unmount() { this.mounted = false; for (const slot of this.slots) slot?.cleanup?.(); },
      };
      host.render();
      return host;
    },
  };
}

function fixture() {
  const clock = timers();
  const hooks = hookHost();
  const preferences = { orderedPaths: [], instanceRules: {}, legacyMigrated: true };
  const store = new Map();
  const attempts = [], calls = [];
  const confirmedCallbacks = [];
  const backend = { holdSave: undefined, holdRead: undefined, failNext: false, failReadNext: false, settings: {}, active: 0, maxActive: 0 };
  const native = {
    isDesktopShell: () => true,
    NativeUnavailableError: Error,
    getSettings: async () => clone(backend.settings),
    prefsToSettings: (prefs, base) => ({ ...base, ...prefs }),
  };
  const invoke = async (command, args) => {
    calls.push(command);
    if (command === 'get_java_preferences') {
      const snapshot = clone(preferences);
      const hold = backend.holdRead;
      backend.holdRead = undefined;
      if (hold) await hold.promise;
      if (backend.failReadNext) { backend.failReadNext = false; throw new Error('fixture read failure'); }
      return snapshot;
    }
    if (command === 'migrate_legacy_java_preferences') {
      if (!preferences.legacyMigrated) {
        const { legacy } = args;
        if (!preferences.orderedPaths.length) preferences.orderedPaths = clone(legacy.orderedPaths);
        for (const [id, rule] of Object.entries(legacy.instanceRules)) {
          if (id === 'rejected') throw new Error('instance does not exist');
          if (!(id in preferences.instanceRules)) preferences.instanceRules[id] = clone(rule);
        }
        preferences.legacyMigrated = true;
      }
      return clone(preferences);
    }
    if (command === 'list_preferred_java_runtimes') return [];
    if (command === 'save_java_preferences') {
      attempts.push(clone(args));
      backend.active++;
      backend.maxActive = Math.max(backend.maxActive, backend.active);
      const hold = backend.holdSave;
      backend.holdSave = undefined;
      try {
        if (hold) await hold.promise;
        if (backend.failNext) { backend.failNext = false; throw new Error('fixture disk failure'); }
        const { update, settings } = args;
        if (update.rule?.mode === 'folder' && update.rule.folder === 'jre') throw new Error('Java paths must be absolute');
        if (update.rule) preferences.instanceRules[update.instanceId] = clone(update.rule);
        if (update.orderedPaths) preferences.orderedPaths = clone(update.orderedPaths);
        if (settings) backend.settings = clone(settings);
        return clone(preferences);
      } finally { backend.active--; }
    }
    if (command === 'select_instance_java') return { requirement: { minMajor: 21, maxMajor: null }, runtime: null, canAutoInstall: true };
    throw new Error(`Unexpected IPC ${command}`);
  };
  const context = vm.createContext({
    console, setTimeout: clock.setTimeout, clearTimeout: clock.clearTimeout,
    window: { setTimeout: clock.setTimeout, clearTimeout: clock.clearTimeout, addEventListener() {}, removeEventListener() {} },
    localStorage: { getItem: key => store.get(key) ?? null, setItem: (key, value) => store.set(key, value), removeItem: key => store.delete(key) },
  });
  function load(relative, dependencies) {
    const filename = path.join(UI, relative);
    const { outputText } = ts.transpileModule(fs.readFileSync(filename, 'utf8'), {
      compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX },
      fileName: filename,
    });
    const module = { exports: {} };
    const requireFixture = name => {
      if (!(name in dependencies)) throw new Error(`Unexpected dependency ${name} in ${relative}`);
      return dependencies[name];
    };
    vm.runInContext(`(function(require,module,exports){${outputText}\n})`, context, { filename })(requireFixture, module, module.exports);
    return module.exports;
  }
  const localized = load('utils/i18n/localized.ts', { './index': { translate } });
  const java = load('utils/java.ts', {
    './native': native, './i18n/localized': localized, './i18n': { translate }, './preview': { PREVIEW_DATA: false },
  });
  const api = load('utils/javaNative.ts', {
    '@tauri-apps/api/core': { invoke }, './native': native, './java': java,
    './launcherPrefs': { prefsFromSettings: settings => settings }, './i18n/localized': localized,
  });
  const jsx = (type, props) => ({ type, props });
  const card = load('components/InstanceJavaCard.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    'lucide-react': { AlertTriangle() {}, CheckCircle2() {}, FolderOpen() {}, Terminal() {} },
    '../utils/java': java, '../utils/javaNative': api, '../utils/native': native,
    '../utils/i18n/localized': localized, '../utils/i18n': { useI18n: () => ({ t: key => key, language: 'en' }) },
    './InstanceJavaCard.module.css': { default: {} },
  }).InstanceJavaCard;
  const defaults = { theme: 'dark', javaPath: '', customAccent: '#41c98b' };
  const settingsDependencies = {
    react: { ...hooks.react, default: hooks.react }, 'react/jsx-runtime': { jsx, jsxs: jsx, Fragment: 'fragment' },
    'lucide-react': new Proxy({}, { get: (_target, name) => name }),
    '../utils/java': java, '../utils/javaNative': api,
    '../utils/native': { ...native, aria2Status: async () => null, errorText: error => String(error) },
    '../utils/launcherPrefs': { DEFAULT_PREFS: defaults, copyPrefs: clone, prefsEqual: (a, b) => JSON.stringify(a) === JSON.stringify(b), savePrefs() {} },
    '../utils/i18n/localized': localized, '../utils/i18n': { useI18n: () => ({ t: key => key, language: 'en' }) },
    '../utils/accent': { applyAccent() {}, manualAccent: prefs => prefs.customAccent || '#53b4f6' }, '../hooks/useDialogFocus': { useDialogFocus: () => ({ isTopDialog: () => true }) },
    '../utils/javaBridge': {}, '../utils/motion': { readMotionLevel: () => 'full' },
    '../utils/background': { readBackgroundSetting: () => ({ accentMode: 'manual' }) },
    './SettingsPanel.module.css': { default: {} },
  };
  for (const name of ['Button', 'JavaSettingsSection', 'MemorySection', 'NumberStepper', 'LanguageSection', 'SkinSection', 'LauncherUpdateCard', 'BackgroundSection', 'CurseForgeKeyCard', 'ModelDownloadCard', 'MotionSection', 'LauncherLogCard', 'AriaSection']) {
    settingsDependencies[`./${name}`] = { [name]: name };
  }
  const SettingsPanel = load('components/SettingsPanel.tsx', settingsDependencies).SettingsPanel;
  const hosts = [];
  function mountSettings(callbacks = {}) {
    const host = hooks.mount(SettingsPanel, {
      theme: 'dark', initialPrefs: defaults, initialTab: 'java', dark: true,
      instance: { id: 'fixture', name: 'Fixture', version: '1.21.4', loader: 'vanilla' },
      onThemeChange() {}, onClose() {}, ...callbacks,
    });
    hosts.push(host);
    return host;
  }
  function mount(setting = auto(), instanceId = 'fixture') {
    const props = {
      instance: { id: instanceId, name: 'Fixture', version: '1.21.4', loader: 'vanilla' },
      runtimes: [], setting, globalPath: '', embedded: true, onManageJava() {},
      onChange(rule) { confirmedCallbacks.push(clone(rule)); },
    };
    const host = hooks.mount(card, props);
    hosts.push(host);
    return host;
  }
  async function settle() {
    for (let i = 0; i < 40; i++) { await Promise.resolve(); for (const host of hosts) host.render(); }
  }
  async function until(predicate, message) {
    for (let i = 0; i < 40; i++) { if (predicate()) return; await settle(); }
    assert.fail(message);
  }
  function nodes(tree, predicate) {
    if (!tree || typeof tree !== 'object') return [];
    if (Array.isArray(tree)) return tree.flatMap(node => nodes(node, predicate));
    return [...(predicate(tree) ? [tree] : []), ...nodes(tree.props?.children, predicate)];
  }
  const field = (host, label) => {
    const node = nodes(host.tree, node => node.props?.['aria-label'] === label)[0];
    assert.ok(node, `missing ${label}`);
    return node;
  };
  async function change(host, label, value) {
    field(host, label).props.onChange({ target: { value } });
    host.render();
    await settle();
  }
  const value = host => field(host, 'java.modeAriaLabel').props.value;
  const settingsSection = host => nodes(host.tree, node => node.type === 'JavaSettingsSection')[0];
  const saveButton = host => nodes(host.tree, node => node.type === 'button' && ['settings.saveButton', 'settings.savedButton'].includes(node.props?.children))[0];
  return { api, java, backend, preferences, store, calls, attempts, confirmedCallbacks, clock, mount, mountSettings, settingsSection, saveButton, settle, until, change, field, value };
}

async function navigationCase() {
  const f = fixture();
  const card = f.mount();
  await f.settle();
  await f.change(card, 'java.modeAriaLabel', 'range');
  await f.change(card, 'java.rangeAriaLabel', '21');
  assert.equal(f.attempts.length, 0, 'the fixture must leave before 350ms');
  card.unmount();
  await f.api.flushPendingJavaPreferences('fixture');
  assert.deepEqual(f.preferences.instanceRules.fixture, range('21'));
  const reopened = f.mount(range('21'));
  await f.settle();
  assert.equal(f.value(reopened), 'range');
  assert.equal(f.field(reopened, 'java.rangeAriaLabel').props.value, '21');
}

async function immediateLaunchCase() {
  const f = fixture();
  f.api.stageJavaRuleDraft('fixture', range('21'));
  const held = deferred();
  f.backend.holdSave = held;
  let launched = false;
  const launch = f.api.flushPendingJavaPreferences('fixture').then(() => { launched = true; });
  const concurrentFlush = f.api.flushPendingJavaPreferences('fixture');
  await f.until(() => f.attempts.length === 1, 'flush must start the debounce write immediately');
  assert.equal(launched, false, 'launch must wait for the backend commit');
  held.resolve();
  await Promise.all([launch, concurrentFlush]);
  f.clock.advance(1000);
  await f.settle();
  assert.equal(launched, true);
  assert.equal(f.attempts.length, 1, 'concurrent flushes must share one write');
  assert.deepEqual(f.preferences.instanceRules.fixture, range('21'));
}

async function failureAndRetryCase() {
  const f = fixture();
  const card = f.mount();
  await f.settle();
  f.backend.failNext = true;
  await f.change(card, 'java.modeAriaLabel', 'forced');
  await f.change(card, 'java.forcedAriaLabel', forced().path);
  await assert.rejects(f.api.flushPendingJavaPreferences('fixture'), /fixture disk failure/);
  await f.settle();
  assert.equal(f.api.getJavaRuleDraft('fixture').dirty, true);
  assert.deepEqual(clone(f.api.getJavaRuleDraft('fixture').confirmed), auto());
  assert.match(f.api.getJavaRuleDraft('fixture').error, /fixture disk failure/);
  await assert.rejects(f.api.flushPendingJavaPreferences('fixture'), /fixture disk failure/);
  assert.equal(f.attempts.length, 1, 'failed current revisions are sticky, not retried silently');
  card.unmount();
  const reopened = f.mount();
  await f.settle();
  assert.equal(f.value(reopened), 'forced', 'failed drafts survive navigation and reloads');
  await f.change(reopened, 'java.modeAriaLabel', 'auto');
  await f.api.flushPendingJavaPreferences('fixture');
  await f.settle();
  assert.equal(f.api.getJavaRuleDraft('fixture').error, undefined);
  assert.equal(f.api.getJavaRuleDraft('fixture').dirty, false);
  assert.deepEqual(f.preferences.instanceRules.fixture, auto());
}

async function externalConfirmationCase() {
  const f = fixture();
  const card = f.mount();
  await f.settle();
  await f.api.saveJavaSettingsDraft({ memoryMb: 4096 }, { instanceId: 'fixture', rule: forced() });
  card.render({ ...card.props, setting: forced() });
  await f.settle();
  assert.equal(f.value(card), 'forced', 'a still-mounted card must follow the settings transaction');
  assert.equal(f.field(card, 'java.forcedAriaLabel').props.value, forced().path);
  assert.equal(f.attempts.length, 1, 'confirmation is not another persistence operation');
  card.render({ ...card.props, setting: range('17') });
  await f.settle();
  assert.equal(f.value(card), 'range', 'same-instance suppliedSetting changes synchronize a clean draft');
  await f.change(card, 'java.rangeAriaLabel', '21');
  card.render({ ...card.props, setting: auto() });
  await f.settle();
  assert.equal(f.field(card, 'java.rangeAriaLabel').props.value, '21', 'an old parent value cannot replace a dirty edit');
  await f.api.flushPendingJavaPreferences('fixture');
}

async function staleResponsesCase() {
  const f = fixture();
  const card = f.mount();
  await f.settle();
  const held = deferred();
  f.backend.holdSave = held;
  await f.change(card, 'java.modeAriaLabel', 'range');
  await f.change(card, 'java.rangeAriaLabel', '17');
  const flush = f.api.flushPendingJavaPreferences('fixture');
  await f.until(() => f.attempts.length === 1, 'old save must be in flight');
  await f.change(card, 'java.rangeAriaLabel', '21');
  const callbacks = f.confirmedCallbacks.length;
  held.resolve();
  await flush;
  await f.settle();
  assert.deepEqual(f.preferences.instanceRules.fixture, range('21'));
  assert.equal(f.field(card, 'java.rangeAriaLabel').props.value, '21');
  assert.ok(f.confirmedCallbacks.slice(callbacks).every(rule => rule.range !== '17'), 'old save responses must not be published as current');

  const readHeld = deferred();
  f.backend.holdRead = readHeld;
  const reads = f.calls.filter(command => command === 'get_java_preferences').length;
  const read = f.api.loadJavaPreferences();
  await f.until(() => f.calls.filter(command => command === 'get_java_preferences').length > reads, 'read must be in flight');
  await f.change(card, 'java.rangeAriaLabel', '25');
  readHeld.resolve();
  await read;
  await f.settle();
  assert.equal(f.field(card, 'java.rangeAriaLabel').props.value, '25', 'old read responses must not reset newer edits');
  assert.equal(f.api.getJavaRuleDraft('fixture').dirty, true);
  await f.api.flushPendingJavaPreferences('fixture');
}

async function settingsBarrierCase() {
  const f = fixture();
  f.api.stageJavaRuleDraft('fixture', range('17'));
  const held = deferred();
  f.backend.holdSave = held;
  const writing = f.api.flushPendingJavaPreferences('fixture');
  await f.until(() => f.attempts.length === 1, 'pending card write must start');
  const settings = f.api.saveJavaSettingsDraft({ memoryMb: 6144 }, { instanceId: 'fixture', rule: forced() });
  const reading = f.api.loadPersistedJavaState();
  await f.settle();
  assert.equal(f.attempts.length, 1, 'settings transaction must wait for pending card writes');
  held.resolve();
  await Promise.all([writing, settings, reading]);
  await f.api.flushPendingJavaPreferences();
  f.clock.advance(1000);
  await f.settle();
  assert.equal(f.backend.maxActive, 1);
  assert.equal(f.attempts.length, 2);
  assert.deepEqual(f.attempts.map(call => call.update.rule.mode), ['range', 'forced']);
  assert.deepEqual(f.preferences.instanceRules.fixture, forced(), 'an old timer cannot overwrite the settings transaction');
  assert.equal(f.backend.settings.memoryMb, 6144);
}

async function modeSwitchCase() {
  const f = fixture();
  const card = f.mount();
  await f.settle();
  await f.change(card, 'java.modeAriaLabel', 'folder');
  await f.change(card, 'java.folderAriaLabel', 'jre');
  await f.change(card, 'java.modeAriaLabel', 'auto');
  await f.api.flushPendingJavaPreferences('fixture');
  assert.deepEqual(f.preferences.instanceRules.fixture, auto());
  const hidden = { ...auto(), range: 'invalid', folder: 'jre', path: 'relative-java' };
  for (const rule of [hidden, { ...hidden, mode: 'range', range: '21' }, { ...hidden, mode: 'folder', folder: 'C:\\Java' }, { ...hidden, ...forced() }]) {
    const expected = clone(f.java.javaRuleForPersistence(rule));
    await f.api.saveJavaSettingsDraft({}, { instanceId: 'fixture', rule });
    assert.deepEqual(f.preferences.instanceRules.fixture, expected);
    assert.deepEqual(clone(f.java.javaSelectionRequest(rule, [], '')), { ...expected, orderedPaths: [], globalPath: '' });
  }
  f.api.stageJavaRuleDraft('fixture', { ...auto(), mode: 'forced' });
  await assert.rejects(f.api.flushPendingJavaPreferences('fixture'), /java.noForced/);
  f.api.stageJavaRuleDraft('fixture', auto());
  await f.api.flushPendingJavaPreferences('fixture');
}

async function settingsPanelCase() {
  const f = fixture();
  const card = f.mount();
  await f.settle();
  const confirmations = [];
  const panel = f.mountSettings({
    onInstanceJavaChange(_id, rule) {
      confirmations.push(clone(rule));
      card.render({ ...card.props, setting: rule });
    },
  });
  await f.settle();
  const section = () => f.settingsSection(panel).props;
  const submit = async () => {
    assert.equal(f.saveButton(panel).props.disabled, false);
    f.saveButton(panel).props.onClick();
    await f.settle();
    await f.until(() => !panel.tree.props['aria-busy'], 'settings transaction must finish');
  };
  section().onInstanceSettingChange({ ...auto(), folder: 'jre', range: 'bad range', path: 'relative' });
  panel.render();
  await submit();
  assert.deepEqual(f.preferences.instanceRules.fixture, auto(), 'the actual panel strips hidden fields before save');
  assert.deepEqual(clone(section().instanceSetting), auto(), 'the confirmed panel draft must match its normalized payload');
  assert.equal(f.saveButton(panel).props.disabled, true);

  section().onInstanceSettingChange(forced());
  panel.render();
  const beforeFailure = confirmations.length;
  f.backend.failNext = true;
  await submit();
  assert.equal(confirmations.length, beforeFailure, 'failed panel transactions must not publish a confirmation');
  assert.deepEqual(f.preferences.instanceRules.fixture, auto());
  assert.equal(f.value(card), 'auto');
  assert.equal(f.saveButton(panel).props.disabled, false, 'a failed panel draft stays dirty for explicit retry');
  await submit();
  await f.settle();
  assert.deepEqual(f.preferences.instanceRules.fixture, forced());
  assert.equal(f.value(card), 'forced', 'the background card must follow a real settings-panel save');
  assert.equal(f.saveButton(panel).props.disabled, true);
}

async function settingsPanelRecoveryCase() {
  for (const folder of ['jre', '']) {
    const f = fixture();
    const card = f.mount();
    await f.settle();
    f.api.stageJavaRuleDraft('fixture', { ...auto(), mode: 'folder', folder });
    await assert.rejects(f.api.flushPendingJavaPreferences('fixture'), folder ? /absolute/ : /java.noFolder/);
    const panel = f.mountSettings();
    await f.settle();
    assert.equal(f.settingsSection(panel).props.busy, false, 'a broken current draft must not lock SettingsPanel loading');
    assert.equal(f.settingsSection(panel).props.instanceSetting.mode, 'folder');
    assert.equal(f.settingsSection(panel).props.instanceSetting.folder, folder);
    f.settingsSection(panel).props.onInstanceSettingChange(auto());
    panel.render();
    assert.equal(f.saveButton(panel).props.disabled, false, 'Auto matching the old backend default is still a recovery edit');

    f.backend.failNext = true;
    f.saveButton(panel).props.onClick();
    await f.settle();
    await f.until(() => !panel.tree.props['aria-busy'], 'failed recovery attempt must finish');
    assert.equal(f.api.getJavaRuleDraft('fixture').dirty, true);
    await assert.rejects(f.api.flushPendingJavaPreferences('fixture'), /fixture disk failure/);
    assert.equal(f.saveButton(panel).props.disabled, false);
    f.saveButton(panel).props.onClick();
    await f.settle();
    await f.until(() => !panel.tree.props['aria-busy'], 'explicit retry must replace the failed shared draft');
    assert.deepEqual(f.preferences.instanceRules.fixture, auto());
    assert.equal(f.api.getJavaRuleDraft('fixture').dirty, false);
    assert.equal(f.api.getJavaRuleDraft('fixture').error, undefined);
    assert.equal(f.value(card), 'auto');
    await f.api.flushPendingJavaPreferences('fixture');
  }
}

async function unrelatedDraftSettingsCase() {
  const f = fixture();
  f.api.stageJavaRuleDraft('invalid-other', { ...auto(), mode: 'range', range: 'bad range' });
  f.api.stageJavaRuleDraft('failed-other', { ...auto(), mode: 'folder', folder: 'jre' });
  await assert.rejects(f.api.flushPendingJavaPreferences('failed-other'), /absolute/);
  const panel = f.mountSettings({ instance: undefined });
  await f.settle();
  assert.equal(f.settingsSection(panel).props.busy, false, 'unrelated failed/invalid drafts cannot block the global settings reader');
  f.settingsSection(panel).props.onRuntimesChange([{ path: forced().path, majorVersion: 21, versionText: 'fixture' }]);
  panel.render();
  assert.equal(f.saveButton(panel).props.disabled, false);
  f.saveButton(panel).props.onClick();
  await f.settle();
  await f.until(() => !panel.tree.props['aria-busy'], 'global settings save must not flush invalid unrelated drafts');
  assert.equal(f.saveButton(panel).props.disabled, true);
  assert.deepEqual(f.preferences.orderedPaths, [forced().path]);
  assert.equal(f.api.getJavaRuleDraft('invalid-other').dirty, true);
  assert.equal(f.api.getJavaRuleDraft('failed-other').dirty, true);
  await assert.rejects(f.api.flushPendingJavaPreferences('invalid-other'), /java.rangeBrackets/);
  await assert.rejects(f.api.flushPendingJavaPreferences('failed-other'), /absolute/);
  f.api.stageJavaRuleDraft('healthy', range('21'));
  await f.api.flushPendingJavaPreferences('healthy');
  assert.deepEqual(f.preferences.instanceRules.healthy, range('21'));
  await f.api.saveJavaSettingsDraft({ memoryMb: 4096 }, { instanceId: 'healthy', rule: forced() });
  await f.api.flushPendingJavaPreferences('healthy');
  assert.deepEqual(f.preferences.instanceRules.healthy, forced());
}

async function replacementOrderingCase() {
  for (const oldInFlight of [true, false]) {
    const f = fixture();
    const held = deferred();
    let oldRead;
    if (!oldInFlight) {
      await f.api.loadJavaPreferences();
      f.backend.holdRead = held;
      const previousReads = f.calls.filter(command => command === 'get_java_preferences').length;
      oldRead = f.api.loadJavaPreferences();
      await f.until(() => f.calls.filter(command => command === 'get_java_preferences').length > previousReads, 'hold the queue before the old write');
    } else {
      f.backend.holdSave = held;
    }
    f.api.stageJavaRuleDraft('fixture', range('17'));
    const oldFlush = f.api.flushPendingJavaPreferences('fixture');
    if (oldInFlight) await f.until(() => f.attempts.length === 1, 'old write must be active');
    const replacement = f.api.saveJavaSettingsDraft({ memoryMb: 4096 }, { instanceId: 'fixture', rule: auto() });
    const launchBarrier = f.api.flushPendingJavaPreferences('fixture');
    held.resolve();
    await Promise.all([oldFlush, replacement, launchBarrier, oldRead]);
    f.clock.advance(1000);
    await f.settle();
    assert.deepEqual(f.preferences.instanceRules.fixture, auto());
    assert.equal(f.api.getJavaRuleDraft('fixture').dirty, false);
    assert.equal(f.attempts.length, oldInFlight ? 2 : 1, 'queued obsolete revisions must skip persistence');
    assert.equal(f.attempts.at(-1).update.rule.mode, 'auto', 'the replacement must be the last backend write');
  }
}

async function localizedValidationCase() {
  const f = fixture();
  for (const [rule, key] of [
    [{ ...auto(), mode: 'range' }, 'java.rangeMissing'],
    [{ ...auto(), mode: 'range', range: 'bad range' }, 'java.rangeBrackets'],
    [{ ...auto(), mode: 'folder' }, 'java.noFolder'],
    [{ ...auto(), mode: 'forced' }, 'java.noForced'],
  ]) {
    f.api.stageJavaRuleDraft('fixture', rule);
    let failure;
    await assert.rejects(f.api.flushPendingJavaPreferences('fixture'), error => { failure = error; return error.text?.key === key; });
    for (const language of ['zh-CN', 'en', 'ja']) {
      assert.equal(f.api.localizeJavaPreferenceError(failure, language), dictionaries[language][key]);
    }
  }
  assert.equal(f.api.localizeJavaPreferenceError('backend detail', 'ja'), 'backend detail');
}

async function scopedFlushCase() {
  const f = fixture();
  f.api.stageJavaRuleDraft('one', { ...auto(), mode: 'range', range: 'bad range' });
  f.api.stageJavaRuleDraft('two', range('21'));
  await f.api.flushPendingJavaPreferences('two');
  assert.deepEqual(f.preferences.instanceRules.two, range('21'));
  await assert.rejects(f.api.flushPendingJavaPreferences(), /java.rangeBrackets/);
  f.api.stageJavaRuleDraft('one', auto());
  await f.api.flushPendingJavaPreferences();
}

async function legacyMigrationRecoveryCase() {
  // Corrupt legacy browser data must not block reads or launches; valid data still imports once.
  const broken = fixture();
  broken.preferences.legacyMigrated = false;
  broken.store.set('flowlaunch-java-runtimes', '{not json');
  broken.store.set('flowlaunch-instance-java', '{"fixture":"C:\\\\Java\\\\bin\\\\java.exe"}');
  const state = await broken.api.loadPersistedJavaState();
  assert.equal(state.preferences.legacyMigrated, true, 'the marker must be written despite unreadable legacy data');
  assert.deepEqual(state.preferences.instanceRules, {}, 'nothing may be fabricated from corrupt legacy data');
  assert.equal(broken.attempts.length, 0);
  broken.api.stageJavaRuleDraft('fixture', range('21'));
  await broken.api.flushPendingJavaPreferences('fixture');
  assert.deepEqual(broken.preferences.instanceRules.fixture, range('21'));
  // A second launch path still works and never retries migration.
  await broken.api.loadJavaPreferences();
  assert.equal(broken.calls.filter(command => command === 'migrate_legacy_java_preferences').length, 1);

  // A backend-rejected rule gets the same treatment: finish without it, never retry forever.
  const rejected = fixture();
  rejected.preferences.legacyMigrated = false;
  rejected.store.set('flowlaunch-instance-java', JSON.stringify({ rejected: forced(), fixture: range('21') }));
  const recovered = await rejected.api.loadJavaPreferences();
  assert.equal(recovered.legacyMigrated, true);
  assert.deepEqual(recovered.instanceRules, {}, 'a rejected import finishes without fabricating rules');
  await rejected.api.saveJavaSettingsDraft({ memoryMb: 4096 }, { instanceId: 'fixture', rule: range('21') });
  assert.deepEqual(rejected.preferences.instanceRules.fixture, range('21'));

  const valid = fixture();
  valid.preferences.legacyMigrated = false;
  valid.store.set('flowlaunch-java-runtimes', JSON.stringify([{ path: forced().path, majorVersion: 21, versionText: 'fixture' }]));
  valid.store.set('flowlaunch-instance-java', JSON.stringify({ fixture: forced() }));
  const imported = await valid.api.loadJavaPreferences();
  assert.deepEqual(imported.orderedPaths, [forced().path]);
  assert.deepEqual(imported.instanceRules.fixture, forced());
  assert.equal(valid.store.has('flowlaunch-java-runtimes'), false, 'legacy keys are cleared after a committed import');
  assert.equal(valid.store.has('flowlaunch-instance-java'), false);
}

async function javaReadFailureCase() {
  // A failed Java read must surface only in the Java section, never lock unrelated settings.
  const javaTab = fixture();
  javaTab.backend.failReadNext = true;
  const javaPanel = javaTab.mountSettings({ initialTab: 'java' });
  await javaTab.settle();
  await javaTab.until(() => javaTab.saveButton(javaPanel) !== undefined, 'settings must finish loading after a Java read failure');
  assert.equal(javaTab.settingsSection(javaPanel).props.busy, true, 'the Java section stays disabled until a read succeeds');

  // The same failure must not overwrite confirmed rules or priority while saving other tabs.
  const f = fixture();
  f.preferences.orderedPaths = [forced().path];
  f.preferences.instanceRules.fixture = forced();
  f.backend.failReadNext = true;
  const panel = f.mountSettings({ initialTab: 'game' });
  await f.settle();
  await f.until(() => f.saveButton(panel) !== undefined, 'settings must finish loading after a Java read failure');
  const width = f.field(panel, 'override.widthAria');
  width.props.onChange({ target: { value: '1440' } });
  panel.render();
  await f.settle();
  const save = f.saveButton(panel);
  assert.equal(save.props.disabled, false, 'unrelated settings remain saveable');
  save.props.onClick();
  await f.settle();
  await f.until(() => !panel.tree.props['aria-busy'], 'the unrelated save must finish');
  assert.deepEqual(f.preferences.instanceRules.fixture, forced(), 'a failed Java read must not overwrite the confirmed rule');
  assert.deepEqual(f.preferences.orderedPaths, [forced().path], 'a failed Java read must not overwrite candidate priority');
  assert.equal(f.backend.settings.windowWidth, 1440, 'the unrelated setting must still be persisted');
}

(async () => {
  for (const [name, test] of [
    ['navigation flush and remount', navigationCase],
    ['immediate launch and concurrent flush', immediateLaunchCase],
    ['sticky failure, navigation and new-draft recovery', failureAndRetryCase],
    ['external confirmation and dirty draft precedence', externalConfirmationCase],
    ['stale save/read responses', staleResponsesCase],
    ['settings transaction barrier and load without deadlock', settingsBarrierCase],
    ['mode-specific payloads and invalid-to-auto recovery', modeSwitchCase],
    ['settings panel normalization, failed save and external update', settingsPanelCase],
    ['failed/incomplete folder recovery through SettingsPanel Auto', settingsPanelRecoveryCase],
    ['unrelated invalid drafts do not block global settings or healthy launch', unrelatedDraftSettingsCase],
    ['explicit replacements supersede queued and in-flight old writes', replacementOrderingCase],
    ['validation errors use current-language existing Java messages', localizedValidationCase],
    ['instance-scoped flush and invalid-draft rejection', scopedFlushCase],
    ['corrupt legacy browser data never blocks reads or launches', legacyMigrationRecoveryCase],
    ['a failed Java read does not lock unrelated settings', javaReadFailureCase],
  ]) {
    let timeout;
    try {
      await Promise.race([test(), new Promise((_resolve, reject) => { timeout = setTimeout(() => reject(new Error(`Deadlock in ${name}`)), 5000); })]);
    } finally { clearTimeout(timeout); }
    console.log(`PASS Java preferences: ${name}`);
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
