// Run: node design/tools/i18n-critical-regression.cjs
// Real handlers and dictionaries, deterministic hooks, mocked native calls; no server or filesystem mutations.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require('../flowlaunch-ui/node_modules/typescript');
const root = path.resolve(__dirname, '../flowlaunch-ui');
const languages = ['zh-CN', 'en', 'ja'];
const instance = { id: 'fixture', name: 'Test game', version: '1.21.1', loader: 'fabric', status: 'ready' };
const raw = '后端诊断原文：fixture / E_ACCESS';
const noop = () => {};
const tick = async () => { for (let i = 0; i < 30; i++) await Promise.resolve(); };
const sourceOf = file => ts.createSourceFile(file, fs.readFileSync(path.join(root, file), 'utf8'), ts.ScriptTarget.Latest, true);

function evaluate(source, globals = {}, requireModule = () => { throw new Error('Unexpected import'); }) {
  const module = { exports: {} };
  const output = ts.transpileModule(source, {
    compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX },
    reportDiagnostics: true,
  });
  assert.equal(output.diagnostics.filter(d => d.category === ts.DiagnosticCategory.Error).length, 0);
  vm.runInNewContext(output.outputText, { module, exports: module.exports, require: requireModule, Error, console, ...globals });
  return module.exports;
}

const dictionaries = Object.fromEntries(languages.map(language => {
  const exports = evaluate(fs.readFileSync(path.join(root, 'utils/i18n/messages', `${language}.ts`), 'utf8'));
  return [language, exports[language === 'zh-CN' ? 'zhCN' : language]];
}));
const { translate: translateMessage } = evaluate(fs.readFileSync(path.join(root, 'utils/i18n/index.ts'), 'utf8'), {}, name => {
  if (name === 'react') return { createContext: () => ({}) };
  const language = path.basename(name);
  return { [language === 'zh-CN' ? 'zhCN' : language]: dictionaries[language] };
});
function translate(language, key, options) {
  assert.ok(Object.hasOwn(dictionaries[language], key), `${language}: untranslated key ${key}`);
  return translateMessage(language, key, options);
}
function declaration(file, name) {
  const source = sourceOf(file);
  let found;
  const visit = node => {
    if ((ts.isVariableDeclaration(node) || ts.isFunctionDeclaration(node)) && node.name?.getText(source) === name) found = node;
    ts.forEachChild(node, visit);
  };
  visit(source);
  assert.ok(found, `${file}: ${name} missing`);
  return ts.isVariableDeclaration(found) ? found.initializer.getText(source) : found.getText(source).replace(/^export\s+/, '');
}
const errorText = evaluate(`module.exports = ${declaration('utils/native.ts', 'errorText')}`);
const appHandler = (name, globals) => evaluate(`module.exports = ${declaration('src/App.tsx', name)}`, globals);
const textOf = node => Array.isArray(node) ? node.map(textOf).join(' ')
  : node && typeof node === 'object' ? textOf(node.props?.children)
    : typeof node === 'string' || typeof node === 'number' ? String(node) : '';

function harness(file, props, language, native = {}) {
  const slots = [];
  let cursor = 0, dirty = true, tree, pending = [];
  let t = (key, options) => translate(language, key, options);
  const same = (a, b) => a && b && a.length === b.length && a.every((value, i) => Object.is(value, b[i]));
  const react = {
    useState(initial) {
      const index = cursor++;
      if (!(index in slots)) slots[index] = typeof initial === 'function' ? initial() : initial;
      return [slots[index], value => {
        const next = typeof value === 'function' ? value(slots[index]) : value;
        if (!Object.is(next, slots[index])) { slots[index] = next; dirty = true; }
      }];
    },
    useRef(initial) {
      const index = cursor++;
      if (!(index in slots)) slots[index] = { current: initial };
      return slots[index];
    },
    useMemo(fn, deps) {
      const index = cursor++;
      if (!slots[index] || !same(slots[index].deps, deps)) slots[index] = { deps, value: fn() };
      return slots[index].value;
    },
    useCallback(fn, deps) { return react.useMemo(() => fn, deps); },
    useEffect(fn, deps) {
      const index = cursor++;
      if (!slots[index] || !same(slots[index].deps, deps)) {
        const previous = slots[index];
        slots[index] = { deps, cleanup: previous?.cleanup };
        pending.push(() => { previous?.cleanup?.(); slots[index].cleanup = fn(); });
      }
    },
  };
  react.useLayoutEffect = react.useEffect;
  const jsx = (type, props) => ({ type, props: props || {} });
  const nativeMock = { errorText, formatBytes: bytes => `${bytes} B`, ...native };
  const exports = evaluate(fs.readFileSync(path.join(root, 'components', file), 'utf8'), {}, name => {
    if (name === 'react') return react;
    if (name === 'react/jsx-runtime') return { jsx, jsxs: jsx, Fragment: 'Fragment' };
    if (name === '../utils/native') return nativeMock;
    if (name === '../utils/i18n') return { useI18n: () => ({ t, language }) };
    if (name === '../utils/store/format') return {
      kindOfVersion: () => 'release', VERSION_KINDS: ['release'], VERSION_KIND_KEYS: { release: 'versionKind.release' },
    };
    return new Proxy({}, { get: (_, key) => key });
  });
  const Component = exports[path.basename(file, '.tsx')];
  const h = {
    async flush() {
      for (let i = 0; i < 30; i++) {
        if (dirty) {
          dirty = false; cursor = 0; pending = [];
          tree = Component(props);
          pending.forEach(effect => effect());
        }
        await Promise.resolve();
      }
      assert.equal(dirty, false, `${file} did not settle`);
    },
    nodes() {
      const result = [];
      const walk = node => {
        if (Array.isArray(node)) node.forEach(walk);
        else if (node && typeof node === 'object') {
          result.push(node); walk(node.props?.children); walk(node.props?.footer);
        }
      };
      walk(tree); return result;
    },
    text: () => textOf(tree),
    find(predicate) {
      const node = h.nodes().find(predicate);
      assert.ok(node, `${file}: node missing. ${h.text()}`);
      return node.props;
    },
    button(key, index = 0, options) {
      const matches = h.nodes().filter(node => node.props?.onClick && textOf(node.props.children) === t(key, options));
      assert.ok(matches[index], `${file}: button ${key}[${index}] missing. ${h.text()}`);
      return matches[index].props;
    },
    input(key, value) { h.find(node => node.props?.label === t(key)).onChange({ target: { value } }); },
    setLanguage(next) { language = next; t = (key, options) => translate(language, key, options); dirty = true; },
    unmount() { slots.forEach(slot => slot?.cleanup?.()); },
  };
  return h;
}

function auditSources() {
  for (const file of ['src/App.tsx', ...['CreateInstanceDialog', 'BackupDiagnostics', 'ModManager'].map(name => `components/${name}.tsx`)]) {
    const source = sourceOf(file);
    const visit = node => {
      if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node) || ts.isTemplateHead(node)
        || ts.isTemplateMiddle(node) || ts.isTemplateTail(node) || ts.isJsxText(node)) {
        assert.ok(!/\p{Script=Han}/u.test(node.text), `${file}:${source.getLineAndCharacterOfPosition(node.getStart(source)).line + 1}: hardcoded UI text ${node.text}`);
      }
      ts.forEachChild(node, visit);
    };
    visit(source);
  }
  const keys = Object.keys(dictionaries['zh-CN']).sort();
  for (const language of languages) {
    assert.deepEqual(Object.keys(dictionaries[language]).sort(), keys, `${language}: dictionary key drift`);
    const source = sourceOf(`utils/i18n/messages/${language}.ts`);
    const seen = new Set();
    const visit = node => {
      if (ts.isPropertyAssignment(node) && ts.isStringLiteral(node.name)) {
        assert.ok(!seen.has(node.name.text), `${language}: duplicate ${node.name.text}`);
        seen.add(node.name.text);
      }
      ts.forEachChild(node, visit);
    };
    visit(source);
    for (const key of keys.filter(key => /^(app\.(offlineConfirm|stopConfirm|install|repair|launchFailed|deleteFailed)|instance\.delete|create\.|mods\.|backup\.|diagnostics\.)/.test(key))) {
      const placeholders = value => [...new Set(value.match(/\{\w+\}/g) || [])].sort();
      assert.deepEqual(placeholders(dictionaries[language][key]), placeholders(dictionaries['zh-CN'][key]), `${language}/${key}: placeholder drift`);
      assert.ok(dictionaries[language][key].trim(), `${language}/${key}: empty translation`);
    }
  }
}

async function createCases(language, cause) {
  const t = (key, options) => translate(language, key, options);
  const failed = async () => { throw cause; };
  let h = harness('CreateInstanceDialog.tsx', { isOpen: true, onClose: noop, onCreated: failed }, language, { listVersions: failed });
  await h.flush();
  assert.ok(h.text().includes(errorText(cause, t('create.versionsReadFailed'))));
  if (cause === null) {
    const next = languages[(languages.indexOf(language) + 1) % languages.length];
    h.setLanguage(next); await h.flush();
    assert.ok(h.text().includes(translate(next, 'create.versionsReadFailed')));
  }
  h.unmount();
  let versionReads = 0, loaderReads = 0;
  h = harness('CreateInstanceDialog.tsx', { isOpen: true, onClose: noop, onCreated: failed }, language, {
    listVersions: async () => { versionReads++; return [{ id: '1.21.1', type: 'release', releaseTime: '2026-01-01' }]; },
    loaderVersions: async () => { loaderReads++; throw cause; },
  });
  await h.flush();
  h.find(node => node.props?.role === 'radio' && textOf(node).includes('1.21.1')).onClick(); await h.flush();
  h.button('create.submit').onClick(); await h.flush();
  assert.ok(h.text().includes(errorText(cause, t('create.failed'))));
  h.find(node => node.props?.role === 'radio' && textOf(node).includes('Fabric')).onClick(); await h.flush();
  assert.ok(h.text().includes(errorText(cause, t('create.loaderVersionsReadFailed'))));
  const next = languages[(languages.indexOf(language) + 1) % languages.length];
  h.setLanguage(next); await h.flush();
  assert.ok(h.text().includes(errorText(cause, translate(next, 'create.loaderVersionsReadFailed'))));
  assert.equal(versionReads, 1, 'Language change must not reload game versions');
  assert.equal(loaderReads, 1, 'Language change must not reset the selected loader');
  h.unmount();
}

async function modCases(language, cause) {
  const t = (key, options) => translate(language, key, options);
  const failed = async () => { throw cause; };
  const props = { instance, onChanged: noop };
  let h = harness('ModManager.tsx', { ...props, onNotice: noop }, language, { listMods: failed });
  await h.flush();
  assert.ok(h.text().includes(errorText(cause, t('mods.listReadFailed'))));
  if (cause === null) {
    const next = languages[(languages.indexOf(language) + 1) % languages.length];
    h.setLanguage(next); await h.flush();
    assert.ok(h.text().includes(translate(next, 'mods.listReadFailed')));
  }
  h.unmount();
  for (const [method, key, action] of [
    ['curseforgeIdentifyMods', 'mods.identifyFailed', 'mods.identify'],
    ['inspectMods', 'mods.inspectFailed', 'mods.checkIssues'],
    ['checkModUpdates', 'mods.checkUpdatesFailed', 'mods.checkUpdatesNow'],
    ['setModEnabled', 'mods.toggleFailed', 'mods.disable'],
    ['setModEnabled', 'mods.toggleFailed', 'mods.enable'],
    ['updateMod', 'mods.updateFailed', 'mods.update'],
  ]) {
    const notices = [];
    const update = { newVersion: 'v2', newFileName: 'new.jar' };
    h = harness('ModManager.tsx', { ...props, onNotice: value => notices.push(value) }, language, {
      listMods: async () => [{ fileName: 'test.jar', size: 10, enabled: action !== 'mods.enable' }],
      checkModUpdates: async () => [{ fileName: 'test.jar', status: { kind: 'updateAvailable', update } }],
      [method]: failed,
    });
    await h.flush();
    if (method === 'checkModUpdates' || method === 'updateMod') {
      h.button('mods.checkUpdates').onClick(); await h.flush();
    }
    if (method === 'updateMod') {
      h.button('mods.checkUpdatesNow').onClick(); await h.flush();
      h.button(action, 0, { values: { version: 'v2' } }).onClick();
    } else if (method === 'setModEnabled') {
      h.find(node => node.props?.['aria-label'] === t(action)).onClick();
    } else h.button(action).onClick();
    await h.flush();
    assert.equal(notices.at(-1), errorText(cause, t(key)), `${language}/${method}`);
    h.unmount();
  }
}

async function backupCases(language, cause) {
  const t = (key, options) => translate(language, key, options);
  const notices = [], filters = [];
  let readable = false, restored = 0;
  const failed = async () => { throw cause; };
  const h = harness('BackupDiagnostics.tsx', { instance, onNotice: value => notices.push(value), onRestored: () => restored++ }, language, {
    createBackup: failed, restoreBackup: failed, getInstanceCrashReport: failed, exportDiagnostics: failed,
    summarizeDiagnostics: failed,
    pickFile: async (title, selectedFilters) => { assert.equal(title, t('backup.pickArchive')); filters.push(selectedFilters); return 'fixture.zip'; },
    previewBackup: async () => { if (!readable) throw cause; return { name: 'Original', minecraft: '1.21.1', files: 1, totalBytes: 10, sha256: 'fixture-sha' }; },
  });
  await h.flush();
  h.button('backup.create').onClick(); await h.flush();
  assert.equal(notices.at(-1), t('backup.needPrivacy'));
  h.find(node => node.type === 'input' && node.props.type === 'checkbox').onChange({ target: { checked: true } });
  h.input('backup.directory', 'fixture'); await h.flush();
  h.button('backup.create').onClick(); await h.flush();
  assert.equal(notices.at(-1), errorText(cause, t('backup.createFailed')));
  h.button('backup.browse', 1).onClick(); await h.flush();
  assert.equal(notices.at(-1), errorText(cause, t('backup.readFailed')));
  assert.equal(filters[0][0].name, t('backup.archiveType'));
  readable = true;
  h.button('backup.browse', 1).onClick(); await h.flush();
  assert.equal(h.find(node => node.props?.label === t('backup.restoreName')).value, t('backup.restoredName', { values: { name: 'Original' } }));
  h.button('backup.restore').onClick(); await h.flush();
  assert.equal(notices.at(-1), errorText(cause, t('backup.restoreFailed')));
  assert.equal(restored, 0);
  for (const [action, fallback] of [['diagnostics.summarize', 'diagnostics.summaryFailed'], ['crash.loadReport', 'diagnostics.readFailed']]) {
    h.button(action).onClick(); await h.flush();
    assert.equal(notices.at(-1), errorText(cause, t(fallback)));
  }
  h.input('diagnostics.directory', 'fixture'); await h.flush();
  h.button('diagnostics.export').onClick(); await h.flush();
  assert.equal(notices.at(-1), errorText(cause, t('diagnostics.exportFailed')));
  h.unmount();
}

async function diagnosticPassthrough(language) {
  const t = (key, options) => translate(language, key, options);
  const h = harness('BackupDiagnostics.tsx', { instance, onNotice: noop, onRestored: noop }, language, {
    summarizeDiagnostics: async () => ({ title: raw, explanation: raw, confidence: 'high', filesFound: 1,
      reasons: [{ title: raw, explanation: raw, confidence: 'high', mods: ['test.jar'], suggestions: [raw], evidence: [raw] }] }),
  });
  await h.flush(); h.button('diagnostics.summarize').onClick(); await h.flush();
  assert.equal(h.nodes().filter(node => node.props?.children === raw).length, 6);
  assert.ok(h.text().includes(t('diagnostics.confidenceHigh')));
  h.unmount();
  const mods = harness('ModManager.tsx', { instance, onNotice: noop, onChanged: noop }, language, {
    listMods: async () => [{ fileName: 'test.jar', size: 10, enabled: true }],
    inspectMods: async () => [{ code: 'fixture', severity: 'error', files: ['test.jar'], message: raw }],
  });
  await mods.flush(); mods.button('mods.checkIssues').onClick(); await mods.flush();
  assert.ok(mods.text().includes(raw)); mods.unmount();
}

async function appCases(language, cause) {
  const t = (key, options) => translate(language, key, options);
  const notices = [], confirmations = [];
  const failed = async () => { throw cause; };
  const base = { t, language, errorText, localizeJavaPreferenceError: error => error, setToast: value => notices.push(value), refreshInstances: async () => {}, refreshDownloadTasks: async () => {} };
  for (const repairing of [false, true]) {
    for (const success of [false, true]) {
      notices.length = 0;
      const queue = appHandler('queueStoreDownload', { ...base, instances: [instance], selectedInstance: instance,
        isDesktopShell: () => true, installInstanceNative: success ? async () => {} : failed });
      queue(instance.name, 'vanilla', instance.id, repairing);
      assert.equal(notices[0], t(repairing ? 'app.repairing' : 'app.installingNamed', { values: { name: instance.name } }));
      await tick();
      assert.equal(notices.at(-1), success ? t(repairing ? 'app.repaired' : 'app.installed', { values: { name: instance.name } })
        : errorText(cause, t(repairing ? 'app.repairFailed' : 'app.installFailed')));
    }
  }
  const confirmAction = async message => { confirmations.push(message); return true; };
  for (const failureStage of ['flush', 'launch']) {
    notices.length = 0;
    const calls = [], progress = [];
    let resolveFlush, rejectFlush;
    const pendingFlush = new Promise((resolve, reject) => { resolveFlush = resolve; rejectFlush = reject; });
    const launch = appHandler('launchInstance', { ...base, currentAccount: { id: 'offline', type: 'offline', username: 'Player' }, confirmAction,
      flushPendingJavaPreferences: id => { calls.push(['flush', id]); return pendingFlush; },
      loadJavaPreferences: async () => { calls.push(['load']); },
      launchInstanceNative: async (...args) => { calls.push(['launch', ...args]); throw cause; },
      setLaunchProgress: value => progress.push(value), setShowAccountModal: noop });
    launch(instance); await tick();
    assert.equal(confirmations.at(-1), t('app.offlineConfirm', { values: { name: 'Player' } }));
    assert.deepEqual(calls, [['flush', instance.id]], `${language}: launch must wait for pending Java preferences`);
    assert.deepEqual(notices, [], `${language}: pending flush must not report a launch failure`);
    if (failureStage === 'flush') rejectFlush(cause);
    else resolveFlush();
    await tick();
    assert.deepEqual(calls, failureStage === 'flush' ? [['flush', instance.id]]
      : [['flush', instance.id], ['load'], ['launch', instance.id, 'offline', true, null]],
    `${language}/${failureStage}: failed flush must block loading and launch; successful flush must precede both`);
    assert.deepEqual(progress, [null], `${language}/${failureStage}: launch failure must clear progress`);
    assert.deepEqual(notices, [errorText(cause, t('app.launchFailed'))], `${language}/${failureStage}: preserve raw errors or use the short fallback`);
  }
  appHandler('stopInstance', { ...base, confirmAction, stopGame: failed })(instance.id); await tick();
  assert.equal(confirmations.at(-1), t('app.stopConfirm'));
  assert.equal(notices.at(-1), errorText(cause, t('app.stopFailed')));
  await appHandler('handleDeleteInstance', { ...base, deleteInstanceNative: failed })(instance, false);
  assert.equal(notices.at(-1), errorText(cause, t('app.deleteFailed')));
  await appHandler('handleDeleteInstance', { ...base, deleteInstanceNative: async () => {}, setPendingDelete: noop,
    selectedInstance: instance, setSelectedInstance: noop, context: 'instance-detail', setContext: noop, clearInstanceIcon: async () => {} })(instance, false);
  assert.equal(notices.at(-1), t('instance.deleted', { values: { name: instance.name } }));
  const source = fs.readFileSync(path.join(root, 'src/App.tsx'), 'utf8');
  for (const key of ['instance.deleteTitle', 'instance.deleteBody', 'instance.deleteFiles', 'instance.deleteFilesHint', 'instance.deleteConfirm', 'common.cancel']) {
    assert.ok(source.includes(`t('${key}'`), `Delete confirmation missing ${key}`);
    assert.notEqual(t(key, { values: { name: instance.name } }), key);
  }
}

(async () => {
  auditSources();
  console.log('PASS F12 no hardcoded Han UI literals, duplicate keys, key or critical placeholder drift');
  for (const language of languages) {
    for (const cause of [null, new Error(raw), raw]) {
      await createCases(language, cause);
      await modCases(language, cause);
      await backupCases(language, cause);
      await appCases(language, cause);
    }
    await diagnosticPassthrough(language);
    console.log(`PASS F12 ${language}: create, loaders, mods, backup/restore, diagnostics, install/repair, launch/stop/delete, Java flush gating; fallback and raw errors`);
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
