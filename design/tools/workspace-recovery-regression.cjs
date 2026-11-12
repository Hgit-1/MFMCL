// Run: node design/tools/workspace-recovery-regression.cjs
// Executes the real component handlers with deterministic hooks and deferred native calls; no disk/network writes.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require('../flowlaunch-ui/node_modules/typescript');

const root = path.resolve(__dirname, '../flowlaunch-ui');
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
const t = (key, options) => `${key}${options?.values?.detail ? `: ${options.values.detail}` : ''}`;
const instance = (id) => ({ id, name: id, version: '1.21.1', loader: 'fabric', status: 'ready' });
const project = (id) => ({ projectId: id, title: id, description: '', downloads: 100, follows: 2 });
const version = (id) => ({
  versionId: id, versionNumber: id, displayName: id, fileName: `${id}.jar`, size: 10,
  url: `https://example.test/${id}.jar`, sha1: 'test', dateCreated: '', gameVersions: ['1.21.1'], loaders: ['fabric'],
});
const packRecord = (id) => ({
  id, name: `Imported ${id}`, gameVersion: '1.21.1', loader: 'vanilla', state: 'not_installed',
  gameDir: `C:/instances/${id}/game`,
});
const missingPackFile = (id) => ({
  projectId: 100 + id, fileId: 200 + id, fileName: `restricted-${id}.jar`,
  required: true, reason: `manual-download-${id}`, canSelectLocal: true,
});
const packResolution = (sourcePath, unresolvedFiles = []) => ({
  sourcePath, unresolvedFiles,
  preview: {
    name: 'Restricted fixture', version: '1.0', minecraft: '1.21.1', loader: 'fabric@0.16',
    requiredFiles: 2, optionalFiles: 0, totalBytes: 20, overrideFiles: 1,
  },
});

function harness(file, props, native = {}) {
  const slots = [];
  let cursor = 0, dirty = true, tree, pending = [];
  const dependenciesEqual = (a, b) => a && b && a.length === b.length && a.every((value, i) => Object.is(value, b[i]));
  const react = {
    useState(initial) {
      const index = cursor++;
      if (!(index in slots)) slots[index] = typeof initial === 'function' ? initial() : initial;
      return [slots[index], (value) => {
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
      if (!slots[index] || !dependenciesEqual(slots[index].deps, deps)) slots[index] = { deps, value: fn() };
      return slots[index].value;
    },
    useCallback(fn, deps) { return react.useMemo(() => fn, deps); },
    useEffect(fn, deps) {
      const index = cursor++;
      if (!slots[index] || !dependenciesEqual(slots[index].deps, deps)) {
        const previous = slots[index];
        slots[index] = { deps, cleanup: previous?.cleanup };
        pending.push(() => { previous?.cleanup?.(); slots[index].cleanup = fn(); });
      }
    },
  };
  react.useLayoutEffect = react.useEffect;
  const nativeMock = new Proxy({
    listVersions: async () => [], curseforgeKeyStatus: async () => ({ configured: true }),
    searchMods: async () => [], searchModsUnscoped: async () => [], curseforgeSearch: async () => [],
    isDesktopShell: () => true, formatBytes: (bytes) => `${bytes} B`,
    errorText: (cause, fallback = 'failed') => cause?.message || (typeof cause === 'string' ? cause : fallback),
    ...native,
  }, { get: (target, key) => target[key] ?? (async () => []) });
  const jsx = (type, props) => ({ type, props: props || {} });
  const module = { exports: {} };
  const source = ts.transpileModule(fs.readFileSync(path.join(root, 'components', file), 'utf8'), {
    compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX },
  }).outputText;
  vm.runInNewContext(source, {
    module, exports: module.exports,
    require(name) {
      if (name === 'react') return react;
      if (name === 'react/jsx-runtime') return { jsx, jsxs: jsx, Fragment: 'Fragment' };
      if (name === '../utils/native') return nativeMock;
      if (name === '../utils/adapters') {
        const adapter = { exports: {} };
        const compiled = ts.transpileModule(fs.readFileSync(path.join(root, 'utils/adapters.ts'), 'utf8'), {
          compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.CommonJS },
        }).outputText;
        vm.runInNewContext(compiled, { module: adapter, exports: adapter.exports }, { filename: 'adapters.ts' });
        return adapter.exports;
      }
      if (name === '../utils/i18n') return { useI18n: () => ({ t, language: 'en' }) };
      if (name === '../utils/motion') return { prefersReducedMotion: () => true };
      if (name === '../utils/launcherPrefs') return { readStoredPrefs: () => ({ downloadDirectory: 'D:/downloads' }) };
      if (name === '../utils/store/format') return {
        fitInstanceLabel: (value) => value, kindOfVersion: () => 'release', VERSION_KINDS: ['release'], VERSION_KIND_KEYS: { release: 'release' },
      };
      if (name === '../utils/store/blocks') return { blockForVersion: () => 'grass', VANILLA_BLOCK: 'grass' };
      if (name === './VersionPicker') return { ALL_VERSIONS: '__all__', VersionPicker: 'VersionPicker' };
      if (name.endsWith('.module.css')) return new Proxy({}, { get: (_, key) => key });
      return new Proxy({}, { get: (_, key) => key });
    },
    localStorage: { getItem: () => null, setItem: () => {} },
    requestAnimationFrame: () => 1, cancelAnimationFrame: () => {},
    console,
  }, { filename: file });
  const Component = module.exports[path.basename(file, '.tsx')];
  const render = () => {
    if (!dirty) return;
    dirty = false; cursor = 0; pending = [];
    tree = Component(props);
    const effects = pending; pending = [];
    effects.forEach(effect => effect());
  };
  return {
    async flush() {
      for (let i = 0; i < 30; i++) { render(); await Promise.resolve(); }
      assert.equal(dirty, false, `${file} did not settle`);
    },
    setProps(next) { props = { ...props, ...next }; dirty = true; },
    nodes() {
      const found = [];
      const walk = (node) => {
        if (Array.isArray(node)) node.forEach(walk);
        else if (node && typeof node === 'object') { found.push(node); walk(node.props?.children); }
      };
      walk(tree); return found;
    },
    text() {
      const walk = (node) => Array.isArray(node) ? node.map(walk).join(' ')
        : node && typeof node === 'object' ? walk(node.props?.children)
          : typeof node === 'string' || typeof node === 'number' ? String(node) : '';
      return walk(tree);
    },
    buttons(label) {
      return this.nodes().filter(node => node.props?.onClick && node.props.children === label).map(node => node.props);
    },
    button(label, index = 0) {
      const matches = this.buttons(label);
      assert.ok(matches[index], `Button ${label}[${index}] missing. ${this.text()}`);
      return matches[index];
    },
    unmount() { slots.forEach(slot => slot?.cleanup?.()); },
  };
}

async function testStore(provider, pack) {
  const a = deferred(), b = deferred(), calls = [];
  const request = (id) => id === 'projectA' ? a.promise : b.promise;
  const native = {
    searchMods: async () => provider === 'mr' ? [project('projectA'), project('projectB')] : [],
    searchModsUnscoped: async () => provider === 'mr' ? [project('projectA'), project('projectB')] : [],
    curseforgeSearch: async () => provider === 'cf' ? [project('projectA'), project('projectB')] : [],
    listModVersions: (_, id) => request(id), listProjectVersions: request, curseforgeFiles: request,
    installModVersion: async (id, file) => { calls.push([id, file.versionId]); return 'installed'; },
    curseforgeInstall: async (id, file) => { calls.push([id, file.versionId]); return 'installed'; },
    downloadAndImportModpack: async (file, id) => { calls.push([id, file.versionId]); return packResolution('C:/cache/fixture.zip'); },
    downloadAndImportModrinthPack: async (file, id) => { calls.push([id, file.versionId]); return packRecord('new'); },
  };
  const h = harness('StoreWorkspace.tsx', {
    category: pack ? 'modpack' : 'mod', instances: [instance('one'), instance('two')], selectedInstance: instance('one'),
    onNotice() {}, onTasksChanged() {}, onCreated() {}, onSelectInstance() {},
  }, native);
  await h.flush();
  h.button('cf.chooseVersion', 0).onClick(); await h.flush();
  h.button('cf.chooseVersion', 0).onClick(); await h.flush();
  b.resolve([version('versionB')]); await h.flush();
  a.resolve([version('versionA')]); await h.flush();
  assert.ok(h.text().includes('versionB'));
  assert.ok(!h.text().includes('versionA'), `${provider}/${pack}: A overwrote B`);
  const install = h.button(pack ? 'store.downloadAndImport' : 'store.install');
  await install.onClick(); await h.flush();
  assert.deepEqual(calls, [['one', 'versionB']]);
  if (pack) {
    h.setProps({ instances: [{ ...instance('one'), version: '1.20.1' }, instance('two')] });
  } else {
    const select = h.nodes().find(node => node.type === 'select' && node.props.value === 'one');
    assert.ok(select, 'target instance selector missing');
    select.props.onChange({ target: { value: 'two' } });
  }
  await h.flush();
  await install.onClick(); await h.flush();
  assert.deepEqual(calls, [['one', 'versionB']], 'old handler installed against a switched target');
  assert.ok(!h.text().includes('versionB'), 'old detail survived target change');
  h.unmount();
}

function assertPackCancels(h, disabled, count = 2) {
  const buttons = h.buttons('common.cancel');
  assert.equal(buttons.length, count, 'pack cancel entry points changed');
  for (const [index, button] of buttons.entries()) {
    assert.equal(Boolean(button.disabled), disabled, `pack cancel ${index} has wrong busy state`);
    assert.match(String(button.onClick), /\bcancelPack\s*\(/, `pack cancel ${index} bypasses the shared cleanup handler`);
  }
  return buttons;
}

async function testPackSupplements(online) {
  const source = online ? 'C:/cache/online-original.zip' : 'C:/packs/local-original.zip';
  const selectedFile = 'C:/Downloads/renamed-first.bin', directory = 'C:/Downloads/remaining';
  const missing = [missingPackFile(1), missingPackFile(2)];
  const initial = deferred(), fileScan = deferred(), folderScan = deferred(), reread = deferred();
  const scans = online ? [fileScan, folderScan, reread] : [initial, fileScan, folderScan, reread];
  const attempts = [deferred(), deferred()];
  const calls = { picks: [], previews: [], imports: [], downloads: [], discards: [], created: [], changes: [], notices: [], directCreates: [] };
  let fileSelections = 0;
  const h = harness('StoreWorkspace.tsx', {
    category: 'modpack', instances: [instance('one')], selectedInstance: instance('one'),
    onNotice: value => calls.notices.push(value), onTasksChanged: () => calls.changes.push('changed'),
    onCreated: record => calls.created.push(record), onSelectInstance() {},
  }, {
    curseforgeSearch: async () => online ? [project('pack-project')] : [],
    curseforgeFiles: async () => [{ ...version('pack-file'), fileName: 'online-original.zip' }],
    pickFile: async (title, filters) => { calls.picks.push(['pack', title, filters]); return source; },
    pickPackSupplementFiles: async title => {
      calls.picks.push(['files', title]);
      return fileSelections++ === 0 ? null : [selectedFile, selectedFile];
    },
    pickDirectory: async title => { calls.picks.push(['directory', title]); return directory; },
    previewModpack: (packPath, localPaths = []) => {
      calls.previews.push([packPath, Array.from(localPaths)]);
      const scan = scans.shift();
      assert.ok(scan, 'unexpected extra pack preview');
      return scan.promise;
    },
    importModpack: (packPath, localPaths = []) => {
      const attempt = attempts[calls.imports.length];
      calls.imports.push([packPath, Array.from(localPaths)]);
      assert.ok(attempt, 'unexpected extra pack import');
      return attempt.promise;
    },
    downloadAndImportModpack: (file, id) => {
      calls.downloads.push([file.versionId, id]);
      return initial.promise;
    },
    discardModpackSource: async packPath => { calls.discards.push(packPath); },
    createInstance: async () => { calls.directCreates.push('create'); throw new Error('unexpected-direct-pack-creation'); },
  });
  const assertNoImport = () => {
    assert.deepEqual(calls.imports, [], 'pack imported before all required files and confirmation');
    assert.deepEqual(calls.created, [], 'precheck called onCreated');
    assert.deepEqual(calls.directCreates, [], 'precheck created a partial instance');
  };
  const probeBusyCancels = async (count = 2) => {
    // Invoke disabled handlers as well: queued events must not bypass packBusy or clear the source.
    for (const button of assertPackCancels(h, true, count)) button.onClick();
    await h.flush();
    assert.deepEqual(calls.discards, [], 'busy cancel discarded a source still in use');
    if (count) assert.ok(h.text().includes(source), 'busy cancel hid the selected pack');
  };
  await h.flush();
  if (online) {
    h.button('cf.chooseVersion').onClick(); await h.flush();
    const download = h.button('store.downloadAndImport');
    download.onClick(); download.onClick(); await h.flush();
    assert.deepEqual(calls.downloads, [['pack-file', 'one']], 'double click downloaded the pack twice');
    assert.equal(h.button('store.packImportLocal').disabled, true);
    await probeBusyCancels(0);
  } else {
    h.button('store.packImportLocal').onClick(); await h.flush();
    assert.equal(calls.picks[0][1], 'store.packPickTitle');
    assert.deepEqual(Array.from(calls.picks[0][2][0].extensions), ['zip', 'mrpack']);
    assert.deepEqual(calls.previews, [[source, []]]);
    await probeBusyCancels(1);
  }
  assertNoImport();
  initial.resolve(packResolution(source, missing)); await h.flush();
  assert.ok(h.text().includes(source));
  assert.ok(h.text().includes(missing[0].fileName));
  assert.ok(h.text().includes(missing[1].fileName));
  assert.deepEqual(h.nodes().filter(node => node.type === 'a').map(node => node.props.href), missing.map(file =>
    `https://www.curseforge.com/projects/${file.projectId}/files/${file.fileId}`));
  assert.equal(h.button('store.packConfirmImport').disabled, true);
  assertPackCancels(h, false);
  h.button('store.packConfirmImport').onClick(); await h.flush();
  assertNoImport();

  const previewCount = calls.previews.length;
  h.button('store.packSelectFiles').onClick(); await h.flush();
  assert.equal(calls.previews.length, previewCount, 'cancelled supplement picker started validation');
  assert.ok(h.text().includes(source));
  assertPackCancels(h, false);
  h.button('store.packSelectFiles').onClick(); await h.flush();
  await probeBusyCancels();
  assert.deepEqual(calls.previews.at(-1), [source, [selectedFile]], 'file selections were not deduplicated');
  fileScan.resolve(packResolution(source, [missing[1]])); await h.flush();
  assert.ok(!h.text().includes(missing[0].fileName));
  assert.ok(h.text().includes(missing[1].fileName));
  assert.equal(h.button('store.packConfirmImport').disabled, true);
  h.button('store.packConfirmImport').onClick(); await h.flush();
  assertNoImport();

  h.button('store.packSelectFolder').onClick(); await h.flush();
  await probeBusyCancels();
  assert.deepEqual(calls.previews.at(-1), [source, [selectedFile, directory]], 'directory selection lost earlier files');
  folderScan.resolve(packResolution(source)); await h.flush();
  assert.equal(h.button('store.packConfirmImport').disabled, false);
  assert.equal(h.buttons('store.packSelectFiles').length, 0);
  assertPackCancels(h, false);
  assertNoImport();

  const confirm = h.button('store.packConfirmImport');
  confirm.onClick(); confirm.onClick(); await h.flush();
  assert.deepEqual(calls.imports, [[source, [selectedFile, directory]]], 'double confirmation duplicated import');
  await probeBusyCancels();
  attempts[0].reject(new Error('pack-import-fixture-failure')); await h.flush();
  assert.ok(h.text().includes('pack-import-fixture-failure'));
  assert.deepEqual(calls.previews.at(-1), [source, [selectedFile, directory]], 'failure reread lost source or supplement paths');
  assert.equal(calls.imports.length, 1, 'failure retried import automatically');
  assert.deepEqual(calls.created, []);
  await probeBusyCancels();
  reread.resolve(packResolution(source)); await h.flush();
  assert.ok(h.text().includes(source));
  assert.equal(h.button('store.packConfirmImport').disabled, false);
  assertPackCancels(h, false);
  assert.equal(calls.imports.length, 1);

  const retry = h.button('store.packConfirmImport');
  retry.onClick(); retry.onClick(); await h.flush();
  assert.deepEqual(calls.imports, [[source, [selectedFile, directory]], [source, [selectedFile, directory]]]);
  await probeBusyCancels();
  attempts[1].resolve(packRecord('one-complete-pack')); await h.flush();
  assert.deepEqual(calls.created.map(record => record.id), ['one-complete-pack'], 'retry did not complete exactly once');
  assert.equal(calls.created[0].version, '1.21.1', 'success did not use the real record adapter');
  assert.equal(calls.notices.filter(notice => notice === 'store.packImported').length, 1);
  assert.equal(calls.changes.length, online ? 2 : 1);
  assert.equal(calls.imports.length, 2);
  assert.deepEqual(calls.directCreates, []);
  assert.equal(calls.downloads.length, online ? 1 : 0, 'continuation redownloaded the original pack');
  assert.deepEqual(calls.previews, [
    ...(online ? [] : [[source, []]]),
    [source, [selectedFile]], [source, [selectedFile, directory]], [source, [selectedFile, directory]],
  ]);
  assert.ok(h.text().includes('store.packSelectFirst'));
  assert.ok(!h.text().includes(source));
  assert.equal(h.buttons('store.packConfirmImport').length, 0);
  assertPackCancels(h, false, 0);
  h.unmount(); await h.flush();
  assert.deepEqual(calls.discards, [], 'completed import was discarded a second time on unmount');
}

async function testPackCancel(index) {
  const source = 'C:/packs/cancel-original.zip';
  const cleanup = deferred();
  const calls = { previews: [], imports: [], discards: [], created: [], directCreates: [] };
  let picks = 0;
  const h = harness('StoreWorkspace.tsx', {
    category: 'modpack', instances: [], onNotice() {}, onTasksChanged() {}, onSelectInstance() {},
    onCreated: record => calls.created.push(record),
  }, {
    pickFile: async () => picks++ === 0 ? null : source,
    previewModpack: async packPath => { calls.previews.push(packPath); return packResolution(source, [missingPackFile(1)]); },
    importModpack: async packPath => { calls.imports.push(packPath); return packRecord('unexpected'); },
    createInstance: async () => { calls.directCreates.push('create'); return packRecord('unexpected'); },
    discardModpackSource: packPath => { calls.discards.push(packPath); return cleanup.promise; },
  });
  await h.flush();
  h.button('store.packImportLocal').onClick(); await h.flush();
  assert.deepEqual(calls.previews, [], 'cancelled pack picker read a source');
  assertPackCancels(h, false, 0);
  h.button('store.packImportLocal').onClick(); await h.flush();
  const cancels = assertPackCancels(h, false);
  cancels[index].onClick(); cancels[1 - index].onClick(); await h.flush();
  assert.deepEqual(calls.discards, [source], 'cancel did not use shared, single-source cleanup');
  assert.ok(h.text().includes(source), 'UI cleared before source cleanup completed');
  cleanup.resolve(); await h.flush();
  assert.ok(h.text().includes('store.packSelectFirst'));
  assert.ok(!h.text().includes(source));
  assertPackCancels(h, false, 0);
  assert.deepEqual(calls.imports, []);
  assert.deepEqual(calls.created, []);
  assert.deepEqual(calls.directCreates, []);
  h.unmount(); await h.flush();
  assert.deepEqual(calls.discards, [source], 'cancelled source leaked into unmount cleanup');
}

async function testStoreInvalidation() {
  const pending = deferred();
  const h = harness('StoreWorkspace.tsx', {
    category: 'mod', instances: [instance('one')], selectedInstance: instance('one'),
    onNotice() {}, onTasksChanged() {}, onCreated() {}, onSelectInstance() {},
  }, { searchMods: async () => [project('projectA')], listModVersions: () => pending.promise });
  await h.flush();
  h.button('cf.chooseVersion').onClick(); await h.flush();
  h.setProps({ category: 'resourcepack' }); await h.flush();
  pending.reject(new Error('obsolete-failure')); await h.flush();
  assert.ok(!h.text().includes('obsolete-failure'));
  h.unmount();
}

async function testMods() {
  let files = [{ fileName: 'sample.jar', enabled: true, size: 10 }], listing = 0;
  const calls = [], stale = deferred(), staleIssues = deferred(), staleUpdates = deferred();
  const h = harness('ModManager.tsx', { instance: instance('one'), onNotice() {}, onChanged() {} }, {
    listMods: async () => { listing++; return files.map(item => ({ ...item })); },
    curseforgeIdentifyMods: () => stale.promise,
    inspectMods: () => staleIssues.promise, checkModUpdates: () => staleUpdates.promise,
    setModEnabled: async (id, fileName, enabled) => {
      calls.push([id, fileName, enabled]);
      assert.equal(fileName, files[0].fileName);
      files = [{ ...files[0], enabled, fileName: enabled ? 'sample.jar' : 'sample.jar.disabled' }];
    },
  });
  await h.flush();
  h.button('mods.identify').onClick(); await h.flush();
  h.button('mods.checkIssues').onClick();
  h.button('mods.checkUpdates').onClick(); await h.flush();
  h.button('mods.checkUpdatesNow').onClick(); await h.flush();
  await h.nodes().find(node => node.props['aria-label'] === 'mods.disable').props.onClick(); await h.flush();
  stale.resolve([{ fileName: 'sample.jar', projectTitle: 'obsolete-source', projectUrl: '#', latest: {} }]);
  staleIssues.resolve([{ files: ['sample.jar'], message: 'obsolete-issue', severity: 'error', code: 'test' }]);
  staleUpdates.resolve([{ fileName: 'sample.jar', status: { kind: 'updateAvailable', update: { newVersion: 'obsolete-update' } } }]);
  await h.flush();
  assert.ok(h.text().includes('sample.jar.disabled'));
  assert.ok(!h.text().includes('obsolete-source'));
  assert.ok(!h.text().includes('obsolete-issue'));
  assert.ok(!h.text().includes('mods.update '));
  await h.nodes().find(node => node.props['aria-label'] === 'mods.enable').props.onClick(); await h.flush();
  assert.deepEqual(calls, [['one', 'sample.jar', false], ['one', 'sample.jar.disabled', true]]);
  assert.equal(listing, 3);
  assert.ok(!h.text().includes('sample.jar.disabled'));
  h.unmount();
}

async function testModReadFailure() {
  let reads = 0, fileName = 'sample.jar';
  const h = harness('ModManager.tsx', { instance: instance('one'), onNotice() {}, onChanged() {} }, {
    listMods: async () => {
      if (++reads === 2) throw new Error('read-failure');
      return [{ fileName, enabled: fileName === 'sample.jar', size: 10 }];
    },
    setModEnabled: async () => { fileName = 'sample.jar.disabled'; },
  });
  await h.flush();
  await h.nodes().find(node => node.props['aria-label'] === 'mods.disable').props.onClick(); await h.flush();
  assert.ok(h.text().includes('read-failure'));
  assert.ok(!h.nodes().some(node => node.props['aria-label'] === 'mods.disable'), 'old filename remained actionable');
  await h.button('common.retry').onClick(); await h.flush();
  assert.ok(h.text().includes('sample.jar.disabled'));
  assert.ok(h.nodes().some(node => node.props['aria-label'] === 'mods.enable'));
  h.unmount();
}

async function testInstall(failureStage) {
  const calls = [], notices = [], changed = [], done = [];
  let attempt = 0;
  const h = harness('InstallPlan.tsx', {
    target: { version: '1.21.1' }, onClose() {}, onChanged: id => changed.push(id), onDone: id => done.push(id), onNotice: value => notices.push(value),
  }, {
    loaderVersions: async () => [{ version: '0.16', stable: true }],
    createInstance: async (name, version, loader) => {
      calls.push(`create:${loader ?? 'none'}`);
      if (failureStage === 'creating' && attempt++ === 0) throw new Error('create-failure');
      return { id: 'same-instance' };
    },
    installInstance: async id => { calls.push(`game:${id}`); if (failureStage === 'installing' && attempt++ === 0) throw new Error('game-failure'); },
    installLoader: async id => { calls.push(`loader:${id}`); if (failureStage === 'loader' && attempt++ === 0) throw new Error('loader-failure'); },
  });
  await h.flush();
  h.nodes().find(node => node.props.role === 'radio' && node.props.children[1].props.children === 'Fabric').props.onClick();
  await h.flush();
  await h.button('install.start').onClick(); await h.flush();
  assert.equal(done.length, 0, 'failure called success/close callback');
  assert.ok(h.text().includes(failureStage === 'creating' ? 'create-failure' : 'install.failedVanilla'));
  assert.equal(changed.length, failureStage === 'creating' ? 0 : 1);
  await h.button('install.start').onClick(); await h.flush();
  assert.deepEqual(done, ['same-instance']);
  /*
   * 加载器必须随创建请求一起提交：创建时就写成 `fabric@0.16`，由 install_instance
   * 在官方文件之后消费。之前它被拆成第三次调用，漏跑一次实例就永远停在 vanilla。
   */
  assert.deepEqual(
    calls.filter(call => call.startsWith('create:')),
    failureStage === 'creating' ? ['create:fabric@0.16', 'create:fabric@0.16'] : ['create:fabric@0.16'],
  );
  assert.equal(calls.filter(call => call.startsWith('game:')).length, failureStage === 'installing' ? 2 : 1);
  assert.equal(calls.filter(call => call.startsWith('loader:')).length, 0, 'loader install must not be a separate UI step');
  h.unmount();
}

async function testStoreContextInvalidation(kind) {
  const pending = deferred();
  const h = harness('StoreWorkspace.tsx', {
    category: 'mod', instances: [instance('one')], selectedInstance: instance('one'),
    onNotice() {}, onTasksChanged() {}, onCreated() {}, onSelectInstance() {},
  }, { searchMods: async () => [project('projectA')], listModVersions: () => pending.promise });
  await h.flush();
  h.button('cf.chooseVersion').onClick(); await h.flush();
  if (kind === 'source') {
    const checkbox = h.nodes().find(node => node.type === 'input' && node.props.type === 'checkbox' && node.props.checked);
    checkbox.props.onChange();
  } else if (kind === 'filter') {
    h.nodes().find(node => node.type === 'VersionPicker').props.onChange('1.20.1');
  } else {
    h.button('cf.collapse').onClick();
  }
  await h.flush();
  pending.resolve([version('obsolete-version')]); await h.flush();
  assert.ok(!h.text().includes('obsolete-version'), `${kind} allowed obsolete detail`);
  h.unmount();
}

async function testLoaderRace() {
  const fabric = deferred(), quilt = deferred();
  const h = harness('InstallPlan.tsx', {
    target: { version: '1.21.1' }, onClose() {}, onChanged() {}, onDone() {}, onNotice() {},
  }, { loaderVersions: kind => kind === 'fabric' ? fabric.promise : quilt.promise });
  const choose = label => h.nodes().find(node => node.props.role === 'radio' && node.props.children[1].props.children === label).props.onClick();
  await h.flush(); choose('Fabric'); await h.flush(); choose('Quilt'); await h.flush();
  quilt.resolve([{ version: 'quilt-current', stable: true }]); await h.flush();
  fabric.resolve([{ version: 'fabric-obsolete', stable: true }]); await h.flush();
  assert.equal(h.nodes().find(node => node.type === 'select').props.value, 'quilt-current');
  assert.ok(h.nodes().find(node => node.type === 'input').props.value.includes('quilt-current'));
  assert.ok(!h.text().includes('fabric-obsolete'));
  h.unmount();
}

async function testRefreshFailure() {
  const calls = [], done = [];
  const h = harness('InstallPlan.tsx', {
    target: { version: '1.21.1' }, onClose() {}, onChanged: async () => { throw new Error('refresh-failure'); },
    onDone: id => done.push(id), onNotice() {},
  }, { createInstance: async (name, version, loader) => { calls.push(`create:${loader ?? 'none'}`); return { id: 'same' }; }, installInstance: async () => calls.push('game') });
  await h.flush(); await h.button('install.start').onClick(); await h.flush();
  assert.deepEqual(calls, ['create:none', 'game']);
  assert.deepEqual(done, ['same'], 'refresh failure incorrectly changed successful installation outcome');
  assert.ok(!h.text().includes('install.failed'));
  h.unmount();
}

async function testTargetSwitch() {
  const pending = deferred(), done = [], calls = [];
  let createCount = 0;
  const h = harness('InstallPlan.tsx', {
    target: { version: '1.21.1' }, onClose() {}, onChanged() {}, onNotice() {}, onDone: id => done.push(id),
  }, {
    createInstance: async () => { createCount++; return { id: 'old-instance' }; },
    installInstance: () => pending.promise,
    installLoader: async () => calls.push('loader'),
  });
  await h.flush();
  const first = h.button('install.start').onClick();
  await h.button('install.start').onClick();
  await h.flush();
  assert.equal(createCount, 1, 'double click duplicated instance');
  h.setProps({ target: { version: '1.20.1' } }); await h.flush();
  pending.resolve(); await first; await h.flush();
  assert.deepEqual(done, []);
  assert.deepEqual(calls, []);
  assert.equal(h.nodes().find(node => node.type === 'input').props.value, '1.20.1');
  h.unmount();
}

async function testInstanceLoaderOwnership() {
  const fabric = deferred(), cleanroom = deferred(), changedInstance = deferred(), install = deferred();
  const calls = [];
  let cleanroomReads = 0;
  const h = harness('LoaderInstallCard.tsx', {
    instance: { ...instance('one'), loader: 'vanilla' }, onNotice() {}, onInstalled() {},
  }, {
    fabricApiStatus: async () => null,
    loaderVersions: kind => kind === 'fabric' ? fabric.promise : (++cleanroomReads === 1 ? cleanroom.promise : changedInstance.promise),
    installLoader: (...args) => { calls.push(args); return install.promise; },
  });
  await h.flush();
  for (const label of ['Legacy Fabric', 'LiteLoader', 'Cleanroom', 'OptiFine']) assert.ok(h.button(label));
  h.button('Cleanroom').onClick(); await h.flush();
  cleanroom.resolve([{ version: 'clean-current', stable: true }]); await h.flush();
  fabric.resolve([{ version: 'fabric-obsolete', stable: true }]); await h.flush();
  assert.equal(h.nodes().find(node => node.type === 'select').props.value, 'clean-current');
  assert.ok(!h.text().includes('fabric-obsolete'));
  const oldInstall = h.button('loader.install').onClick;
  oldInstall(); oldInstall(); await h.flush();
  assert.deepEqual(calls, [['one', 'cleanroom', 'clean-current']]);
  install.resolve(); await h.flush();
  h.setProps({ instance: { ...instance('two'), loader: 'vanilla' } }); await h.flush();
  changedInstance.resolve([{ version: 'new-current', stable: true }]); await h.flush();
  oldInstall(); await h.flush();
  assert.equal(calls.length, 1, 'a stale row handler must not install into its former instance');
  h.button('loader.install').onClick(); await h.flush();
  assert.deepEqual(calls[1], ['two', 'cleanroom', 'new-current']);
  h.unmount();
}

(async () => {
  await testInstanceLoaderOwnership(); console.log('PASS loader adapters: eight choices, reversed responses, stale targets and double-click ownership');
  for (const pack of [false, true]) for (const provider of ['mr', 'cf']) {
    await testStore(provider, pack);
    console.log(`PASS F05 ${provider}/${pack ? 'modpack' : 'mod'} reverse responses and target binding`);
  }
  for (const online of [false, true]) {
    await testPackSupplements(online);
    console.log(`PASS F06 ${online ? 'online cached source' : 'local pack'} required supplements, confirmation, busy cancels and single-success retry`);
  }
  for (const index of [0, 1]) {
    await testPackCancel(index);
    console.log(`PASS F06 ${index === 0 ? 'top' : 'bottom'} pack cancel shares cleanup without creating an instance`);
  }
  await testStoreInvalidation(); console.log('PASS F05 category change discards old errors');
  for (const kind of ['source', 'filter', 'collapse']) {
    await testStoreContextInvalidation(kind); console.log(`PASS F05 ${kind} invalidates pending detail`);
  }
  await testMods(); console.log('PASS F07 reversible filenames, rereads, stale provenance/issue/update invalidation');
  await testModReadFailure(); console.log('PASS F07 rename succeeds/read fails, retry uses disk filename');
  for (const stage of ['creating', 'installing']) {
    await testInstall(stage); console.log(`PASS F08 ${stage} failure/retry phase and instance reuse`);
  }
  await testLoaderRace(); console.log('PASS F08 loader versions and default name reject obsolete response');
  await testRefreshFailure(); console.log('PASS F08 refresh callback failure does not become install failure');
  await testTargetSwitch(); console.log('PASS F08 old result cannot close/reset new panel');
})().catch(error => { console.error(error); process.exitCode = 1; });
