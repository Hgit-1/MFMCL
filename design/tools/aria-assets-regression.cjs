const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const os = require('node:os');
const path = require('node:path');
const vm = require('node:vm');
const { spawnSync } = require('node:child_process');
const { pathToFileURL } = require('node:url');

const ROOT = path.resolve(__dirname, '../..');
const UI = path.join(ROOT, 'design/flowlaunch-ui');
const POLICY = path.join(UI, 'distribution-policy.mjs');
const args = process.argv.slice(2);

function run(command, argv, options = {}) {
  const result = spawnSync(command, argv, { cwd: ROOT, encoding: 'utf8', ...options });
  if (result.error) throw result.error;
  assert.equal(result.status, 0, `${command} ${argv.join(' ')}\n${result.stderr}`);
  return result.stdout;
}

async function put(root, relative, text = 'synthetic text fixture\n') {
  const file = path.join(root, relative);
  await fs.mkdir(path.dirname(file), { recursive: true });
  await fs.writeFile(file, text, 'utf8');
}

async function files(root, relative = '') {
  const result = [];
  for (const entry of await fs.readdir(path.join(root, relative), { withFileTypes: true })) {
    const name = path.posix.join(relative, entry.name);
    if (entry.isDirectory()) result.push(...await files(root, name));
    else result.push(name);
  }
  return result.sort();
}

async function fixtureTests(policy) {
  const temp = await fs.mkdtemp(path.join(os.tmpdir(), 'mfmcl-distribution-'));
  try {
    const { build } = await import(pathToFileURL(require.resolve('vite', { paths: [UI] })).href);
    const publicDir = path.join(temp, 'public');
    const dist = path.join(temp, 'dist');
    await put(temp, 'index.html', '<!doctype html><html><body><script type="module" src="/main.js"></script></body></html>');
    await put(temp, 'main.js', 'console.log("distribution fixture");');
    const approved = ['favicon.svg', 'textures/blocks/fixture.txt', 'demo-avatars/fixture.txt'];
    for (const file of approved) await put(publicDir, file, `approved fixture ${file}\n`);
    const excluded = [
      'plugins/aria-with-you/sentinel.txt', 'plugins/unknown-plugin/sentinel.txt',
      'aria-with-you.zip', 'unknown-plugin.mflp', 'unknown-plugin.mfplugin',
      'textures/unknown-plugin.zip', 'demo-avatars/aria-with-you.mfplugin',
      'textures/plugins/unknown-plugin/sentinel.txt', 'unreviewed-root/sentinel.txt',
    ];
    for (const file of excluded) await put(publicDir, file, `MUST_NOT_SHIP:${file}\n`);
    const viteBuild = extra => build({
      root: temp, configFile: path.join(UI, 'vite.config.ts'), logLevel: 'silent',
      build: { outDir: dist, ...extra },
    });
    await viteBuild();
    await policy.assertDistributionTree(dist);
    const output = await files(dist);
    assert.ok(output.includes('index.html') && output.some(file => /^assets\/.*\.js$/.test(file)));
    for (const file of approved) assert.equal(await fs.readFile(path.join(dist, file), 'utf8'), `approved fixture ${file}\n`);
    for (const file of output) {
      assert.equal(policy.isForbiddenPayloadPath(file), false, file);
      assert.ok(!(await fs.readFile(path.join(dist, file), 'utf8')).includes('MUST_NOT_SHIP:'), file);
    }
    for (const file of excluded) assert.equal(await fs.readFile(path.join(publicDir, file), 'utf8'), `MUST_NOT_SHIP:${file}\n`);
    console.log('PASS real Vite config copies only approved public paths; Aria/unknown payloads and archives excluded and source fixtures preserved');

    const contamination = [
      'plugins/aria-with-you/sentinel.txt', 'plugins/unknown-plugin/sentinel.txt',
      'assets/aria-with-you.zip', 'textures/unknown-plugin.mflp',
      'assets/unknown-plugin.mfplugin', 'demo-avatars/unknown-plugin.ZIP',
      'assets/unknown-plugin.tar.gz', 'assets/plugins/unknown-plugin/sentinel.txt',
      'unreviewed-root/sentinel.txt',
    ];
    const contaminatedDirs = [];
    for (const [i, file] of contamination.entries()) {
      const dirty = path.join(temp, `contaminated-${i}`);
      await fs.cp(dist, dirty, { recursive: true });
      await put(dirty, file);
      await assert.rejects(policy.assertDistributionTree(dirty), /Distribution policy:/);
      contaminatedDirs.push(dirty);
    }
    await put(dist, 'plugins/unknown-plugin/stale.txt');
    await assert.rejects(viteBuild({ emptyOutDir: false }), /Distribution policy:/);
    await fs.rm(path.join(dist, 'plugins'), { recursive: true });
    await put(temp, 'main.js', 'import "./public/plugins/unknown-plugin/entry.js";');
    await put(publicDir, 'plugins/unknown-plugin/entry.js', 'console.log("MUST_NOT_SHIP:imported-plugin");');
    await assert.rejects(viteBuild(), /plugin payload imports cannot ship/);
    await put(publicDir, 'plugins/unknown-plugin/inline.svg', '<svg xmlns="http://www.w3.org/2000/svg"><text>MUST_NOT_SHIP:inline</text></svg>');
    await put(temp, 'main.js', 'import payload from "./public/plugins/unknown-plugin/inline.svg?inline"; console.log(payload);');
    await assert.rejects(viteBuild(), /plugin payload imports cannot ship/);
    await put(publicDir, 'unreviewed-root/inline.svg', '<svg xmlns="http://www.w3.org/2000/svg"><text>MUST_NOT_SHIP:unknown</text></svg>');
    await put(temp, 'main.js', 'import payload from "./public/unreviewed-root/inline.svg?inline"; console.log(payload);');
    await assert.rejects(viteBuild(), /plugin payload imports cannot ship/);
    await put(temp, 'main.js', 'console.log("distribution fixture");');
    await viteBuild();
    console.log(`PASS ${contamination.length} dirty frontend trees rejected; stale-output, direct-import and inline-import bypasses rejected`);

    const tauri = path.join(temp, 'src-tauri');
    const config = {
      build: { frontendDist: '../not-built' },
      bundle: { resources: { 'resources/aria2/': 'aria2/', 'resources/aria2-manifest.json': 'resources/aria2-manifest.json' } },
    };
    const saveConfig = () => put(tauri, 'tauri.conf.json', JSON.stringify(config));
    await saveConfig();
    await put(tauri, 'resources/aria2/notice.txt', 'approved sidecar text fixture');
    await put(tauri, 'resources/aria2-manifest.json', '{}');
    const watches = [];
    await policy.verifyTauriDistribution(tauri, { onWatch: file => watches.push(file) });
    assert.ok(watches.includes(path.join(tauri, 'resources')) && watches.includes(path.join(temp, 'not-built')));
    await assert.rejects(policy.verifyTauriDistribution(tauri, { requireDist: true }), /frontendDist is missing/);
    const cliEnv = { ...process.env, TAURI_CONFIG: '', CARGO_CFG_TARGET_OS: 'windows' };
    run(process.execPath, [POLICY, '--check-tauri', tauri], { env: cliEnv });
    const missingCli = spawnSync(process.execPath, [POLICY, '--check-tauri', tauri, '--require-dist'], { encoding: 'utf8', env: cliEnv });
    assert.equal(missingCli.status, 1);
    assert.match(missingCli.stderr, /frontendDist is missing/);
    config.build.frontendDist = '../dist';
    await saveConfig();
    await policy.verifyTauriDistribution(tauri, { requireDist: true });
    for (const dirty of contaminatedDirs) {
      await assert.rejects(policy.verifyTauriDistribution(tauri, {
        requireDist: true, override: JSON.stringify({ build: { frontendDist: dirty } }),
      }), /Distribution policy:/);
    }
    for (const file of ['resources/plugins/aria-with-you/sentinel.txt', 'resources/plugins/unknown-plugin/sentinel.txt',
      'resources/unknown-plugin.mflp', 'resources/aria-with-you.zip', 'resources/unreviewed/sentinel.txt', 'plugins/unknown-plugin/sentinel.txt']) {
      await put(tauri, file);
      await assert.rejects(policy.verifyTauriDistribution(tauri, { requireDist: true }), /Distribution policy:/);
      await fs.rm(path.join(tauri, file.split('/').slice(0, file.startsWith('resources/') ? 2 : 1).join('/')), { recursive: true });
    }
    await assert.rejects(policy.verifyTauriDistribution(tauri, {
      requireDist: true, override: JSON.stringify({ bundle: { resources: { 'other/unknown-plugin.zip': 'unknown-plugin.zip' } } }),
    }), /only the reviewed aria2 resource map/);
    await assert.rejects(policy.verifyTauriDistribution(tauri, {
      requireDist: true, override: JSON.stringify({ bundle: { externalBin: ['plugins/unknown-plugin'] } }),
    }), /externalBin/);
    await put(tauri, 'tauri.windows.conf.json', JSON.stringify({ build: { frontendDist: contaminatedDirs[0] } }));
    await assert.rejects(policy.verifyTauriDistribution(tauri, { requireDist: true, platform: 'windows' }), /Distribution policy:/);
    await fs.rm(path.join(tauri, 'tauri.windows.conf.json'));
    await put(tauri, 'tauri.conf.json5', '{}');
    await assert.rejects(policy.verifyTauriDistribution(tauri, { requireDist: true }), /unreviewed configuration format/);
    await fs.rm(path.join(tauri, 'tauri.conf.json5'));
    console.log('PASS Tauri helper/CLI: clean aria2 accepted; missing dist allowed only for dev; contaminated dist/resources, config overrides and unknown config formats rejected');

    const gitFixture = path.join(temp, 'git-fixture');
    await put(gitFixture, '.gitignore', await fs.readFile(path.join(ROOT, '.gitignore'), 'utf8'));
    run('git', ['init', '--quiet', gitFixture]);
    const ignored = [
      'plugins/aria-with-you/sentinel.txt', 'plugins/unknown-plugin/sentinel.txt',
      'plugin-packages/unknown/sentinel.txt', 'third-party-plugins/unknown/sentinel.txt',
      ...excluded.map(file => `design/flowlaunch-ui/public/${file}`),
      'src-tauri/resources/plugins/unknown/sentinel.txt', 'src-tauri/resources/unknown/sentinel.txt',
      'aria-with-you.zip', 'unknown-plugin.mflp', 'nested/unknown-plugin.mfplugin',
      'unknown-plugin.ZIP', 'nested/unknown-plugin.MFPLUGIN', 'nested/unknown-plugin.tar.gz',
    ];
    const ignoreResult = run('git', ['-C', gitFixture, 'check-ignore', '--stdin', '-z'], { input: ignored.join('\0') + '\0' });
    assert.deepEqual(ignoreResult.split('\0').filter(Boolean), ignored);
    for (const file of ['design/flowlaunch-ui/public/favicon.svg', 'design/flowlaunch-ui/public/textures/fixture.txt',
      'design/flowlaunch-ui/public/demo-avatars/fixture.txt', 'src-tauri/resources/aria2-manifest.json', 'src-tauri/resources/aria2/.gitkeep']) {
      const result = spawnSync('git', ['-C', gitFixture, 'check-ignore', '--no-index', file], { encoding: 'utf8' });
      assert.equal(result.status, 1, `Approved source must not be ignored: ${file}`);
    }
    console.log('PASS Git ignore fixtures: generic plugin roots/archives ignored; approved public paths and aria2 exceptions preserved');
  } finally {
    await fs.rm(temp, { recursive: true, force: true });
  }
}

async function checkCatalog() {
  const ts = require(require.resolve('typescript', { paths: [UI] }));
  const source = await fs.readFile(path.join(UI, 'utils/plugins.ts'), 'utf8');
  const compiled = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 } });
  const module = { exports: {} };
  vm.runInNewContext(compiled.outputText, {
    module, exports: module.exports,
    require(id) {
      assert.equal(id, './i18n/localized', 'Catalog regression must not execute unexpected dependencies');
      return { loc: (key, params) => ({ key, params }) };
    },
  }, { filename: 'plugins.ts', timeout: 5000 });
  const catalog = module.exports;
  const allowed = ['download-mirror', 'launch-logger'];
  assert.deepEqual(Array.from(catalog.BUILTIN_PLUGINS, plugin => plugin.id).sort(), allowed);
  for (const plugin of catalog.BUILTIN_PLUGINS) {
    assert.equal(plugin.builtin, true);
    assert.notEqual(plugin.thirdParty, true);
    assert.notEqual(plugin.bundled, true);
  }
  assert.equal((catalog.BUNDLED_PLUGINS ?? []).length, 0, 'Third-party plugins must not have a bundled catalog');
  const fresh = catalog.allPlugins(catalog.EMPTY_PLUGIN_STORE);
  assert.deepEqual(Array.from(fresh, plugin => plugin.id).sort(), allowed, 'Fresh catalog must contain only built-in plugins');
  console.log('PASS catalog metadata: only launch-logger/download-mirror built-ins, no preinstalled or bundled third-party entry');
}

async function checkRepository(policy) {
  const tracked = run('git', ['ls-files', '-z']).split('\0').filter(Boolean);
  const forbidden = tracked.filter(file => {
    if (policy.isForbiddenPayloadPath(file)) return true;
    const prefix = 'design/flowlaunch-ui/public/';
    if (file.startsWith(prefix)) return !policy.PUBLIC_ROOTS.includes(file.slice(prefix.length).split('/')[0]);
    const resources = 'src-tauri/resources/';
    return file.startsWith(resources) && file !== resources + 'aria2-manifest.json' && !file.startsWith(resources + 'aria2/');
  });
  assert.deepEqual(forbidden, [], 'Remove forbidden plugin payloads from the Git index, not from the user\'s disk');
  console.log(`PASS Git index: ${tracked.length} tracked paths contain no plugin payload roots/archives or unapproved public/resource roots`);
  await checkCatalog();
  const dist = await policy.verifyTauriDistribution(path.join(ROOT, 'src-tauri'), { requireDist: true, override: process.env.TAURI_CONFIG, platform: process.env.CARGO_CFG_TARGET_OS });
  console.log(`PASS current frontendDist/resource paths: ${dist} (not an executable or installer inspection)`);
}

async function main() {
  if (args.some(arg => arg === '--stage-source' || arg.startsWith('--stage-source='))) {
    throw new Error('--stage-source was removed: third-party plugins must be installed by the user, never staged into public/ or bundled with releases. Existing local files are left untouched.');
  }
  for (const arg of args) assert.ok(['--fixtures-only', '--production'].includes(arg), `Unknown option ${arg}; use no flags for the complete distribution regression`);
  const policy = await import(pathToFileURL(POLICY).href);
  await fixtureTests(policy);
  if (!args.includes('--fixtures-only')) await checkRepository(policy);
}

main().catch(error => { console.error(error); process.exitCode = 1; });
