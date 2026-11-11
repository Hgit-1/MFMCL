import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const PUBLIC_ROOTS = Object.freeze(['favicon.svg', 'textures', 'demo-avatars']);
const OUTPUT_ROOTS = new Set(['index.html', 'assets', ...PUBLIC_ROOTS]);
const PAYLOAD_DIRS = new Set(['plugins', 'plugin-packages', 'third-party-plugins']);
const ARCHIVE = /\.(?:mflp|mfplugin|zip|7z|rar|tar|tgz|gz|bz2|xz)$/i;
const APPROVED_RESOURCES = Object.freeze({
  'resources/aria2-manifest.json': 'resources/aria2-manifest.json',
  'resources/aria2/': 'aria2/',
});
const PLATFORM = { win32: 'windows', darwin: 'macos', linux: 'linux' }[process.platform];
const fail = message => { throw new Error(`Distribution policy: ${message}`); };

export function isForbiddenPayloadPath(value) {
  const parts = value.replaceAll('\\', '/').toLowerCase().split('/');
  return parts.some(part => PAYLOAD_DIRS.has(part) || ARCHIVE.test(part));
}

async function statIfPresent(file) {
  try { return await fs.lstat(file); }
  catch (error) { if (error.code === 'ENOENT') return null; throw error; }
}

async function walk(root, visit, relative = '') {
  const file = path.join(root, relative);
  const stat = await fs.lstat(file);
  if (stat.isSymbolicLink() || (!stat.isDirectory() && !stat.isFile())) {
    fail(`links and special files cannot ship: ${file}`);
  }
  if (await visit(relative, stat) === false) return;
  if (stat.isDirectory()) {
    for (const entry of (await fs.readdir(file)).sort()) {
      await walk(root, visit, path.join(relative, entry));
    }
  }
}

export async function approvedPublicFiles(publicDir) {
  const files = [];
  for (const root of PUBLIC_ROOTS) {
    const source = path.join(publicDir, root);
    const stat = await statIfPresent(source);
    if (!stat) continue;
    if (root === 'favicon.svg' ? !stat.isFile() : !stat.isDirectory()) {
      fail(`unexpected public root type: ${source}`);
    }
    await walk(source, async (relative, entry) => {
      const fileName = path.posix.join(root, relative.replaceAll('\\', '/'));
      if (isForbiddenPayloadPath(fileName)) return false;
      if (entry.isFile()) files.push({ fileName, source: await fs.readFile(path.join(source, relative)) });
    });
  }
  return files;
}

function assertOutputPath(fileName) {
  const normalized = fileName.replaceAll('\\', '/');
  const parts = normalized.split('/');
  if (parts.some(part => !part || part === '.' || part === '..') ||
      !OUTPUT_ROOTS.has(parts[0]) || isForbiddenPayloadPath(normalized) ||
      (['index.html', 'favicon.svg'].includes(parts[0]) && parts.length !== 1)) {
    fail(`unapproved frontend output: ${fileName}; rebuild the frontend without plugin payloads`);
  }
}

export async function assertDistributionTree(dist) {
  if (!(await statIfPresent(dist))?.isDirectory()) fail(`frontendDist is missing: ${dist}; run the frontend build first`);
  await walk(dist, (relative, stat) => {
    if (!relative) return;
    assertOutputPath(relative);
    const root = relative.replaceAll('\\', '/').split('/')[0];
    if (relative === root && (['index.html', 'favicon.svg'].includes(root) ? !stat.isFile() : !stat.isDirectory())) {
      fail(`unexpected frontend output type: ${relative}`);
    }
  });
  if (!(await statIfPresent(path.join(dist, 'index.html')))?.isFile()) fail(`frontendDist has no index.html: ${dist}`);
}

export function distributionPolicy() {
  let config;
  return {
    name: 'mfmcl-distribution-policy',
    apply: 'build',
    enforce: 'post',
    configResolved(resolved) {
      config = resolved;
      if (config.build.copyPublicDir !== false) fail('Vite copyPublicDir must be false');
    },
    transform(_code, id) {
      const file = id.split('?')[0];
      const publicPath = config.publicDir && path.relative(config.publicDir, file).replaceAll('\\', '/');
      const unapprovedPublic = publicPath && !publicPath.startsWith('../') && !path.isAbsolute(publicPath) &&
        !PUBLIC_ROOTS.includes(publicPath.split('/')[0]);
      if (isForbiddenPayloadPath(file) || unapprovedPublic) fail(`plugin payload imports cannot ship: ${id}`);
    },
    async generateBundle(_options, bundle) {
      for (const [name, output] of Object.entries(bundle)) {
        assertOutputPath(name);
        for (const original of output.originalFileNames ?? []) {
          if (isForbiddenPayloadPath(original)) fail(`plugin payload imports cannot ship: ${original}`);
        }
      }
      if (config.publicDir) {
        for (const asset of await approvedPublicFiles(config.publicDir)) this.emitFile({ type: 'asset', ...asset });
      }
    },
    async closeBundle() {
      if (config.build.write) await assertDistributionTree(path.resolve(config.root, config.build.outDir));
    },
  };
}

function mergeJson(target, patch) {
  if (!patch || typeof patch !== 'object' || Array.isArray(patch)) return patch;
  const result = target && typeof target === 'object' && !Array.isArray(target) ? { ...target } : {};
  for (const [key, value] of Object.entries(patch)) {
    if (value === null) delete result[key];
    else result[key] = mergeJson(result[key], value);
  }
  return result;
}

export async function verifyTauriDistribution(tauriDir, { requireDist = false, override, platform = PLATFORM, onWatch = () => {} } = {}) {
  tauriDir = path.resolve(tauriDir);
  const configFile = path.join(tauriDir, 'tauri.conf.json');
  onWatch(configFile);
  let config = JSON.parse(await fs.readFile(configFile, 'utf8'));
  // Match Tauri's JSON platform merge, then its TAURI_CONFIG merge patch.
  for (const name of ['tauri.conf.json5', 'Tauri.toml', ...['windows', 'linux', 'macos', 'android', 'ios'].flatMap(os => [`tauri.${os}.conf.json5`, `Tauri.${os}.toml`])]) {
    onWatch(path.join(tauriDir, name));
    if (await statIfPresent(path.join(tauriDir, name))) fail(`unreviewed configuration format: ${name}`);
  }
  if (platform) {
    const platformFile = path.join(tauriDir, `tauri.${platform}.conf.json`);
    onWatch(platformFile);
    if (await statIfPresent(platformFile)) config = mergeJson(config, JSON.parse(await fs.readFile(platformFile, 'utf8')));
  }
  if (override) config = mergeJson(config, JSON.parse(override));
  const resources = config?.bundle?.resources ?? {};
  if (typeof resources !== 'object' || Array.isArray(resources) ||
      Object.entries(resources).some(([source, dest]) => APPROVED_RESOURCES[source] !== dest)) {
    fail('only the reviewed aria2 resource map is allowed; third-party plugin resources cannot ship');
  }
  if (config?.bundle?.externalBin?.length) fail('unreviewed externalBin resources cannot ship');
  const resourcesDir = path.join(tauriDir, 'resources');
  onWatch(resourcesDir);
  if (await statIfPresent(resourcesDir)) {
    await walk(resourcesDir, (relative, stat) => {
      if (!relative) return;
      const parts = relative.replaceAll('\\', '/').split('/');
      if (parts[0] === 'aria2') {
        if (parts.length === 1 && !stat.isDirectory()) fail('aria2 resources must be a directory');
      } else if (relative !== 'aria2-manifest.json' || !stat.isFile()) {
        fail(`unapproved Tauri resource: ${relative}`);
      }
    });
  }
  onWatch(tauriDir);
  for (const entry of await fs.readdir(tauriDir)) {
    if (isForbiddenPayloadPath(entry)) fail(`plugin payloads cannot be staged in src-tauri: ${entry}`);
  }
  const frontend = config?.build?.frontendDist;
  if (typeof frontend !== 'string' || !frontend || /^\w+:\/\//.test(frontend)) {
    fail('frontendDist must be one reviewed local output directory');
  }
  const dist = path.resolve(tauriDir, frontend);
  onWatch(dist);
  if (requireDist) await assertDistributionTree(dist);
  return dist;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  if (args[0] !== '--check-tauri' || !args[1] || args.slice(2).some(arg => arg !== '--require-dist')) {
    console.error('Usage: node distribution-policy.mjs --check-tauri <src-tauri> [--require-dist]');
    process.exitCode = 1;
  } else {
    verifyTauriDistribution(args[1], {
      requireDist: args.includes('--require-dist'),
      override: process.env.TAURI_CONFIG,
      platform: process.env.CARGO_CFG_TARGET_OS,
      onWatch: file => console.log(`cargo:rerun-if-changed=${file}`),
    }).catch(error => { console.error(error.message); process.exitCode = 1; });
  }
}
