import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { assetNames, checkVersions, checksums, collect, releaseVersion } from './release.mjs';

async function fixture(run) {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), 'mfmcl-release-test-'));
  try { await run(root); }
  finally { await fs.rm(root, { recursive: true, force: true }); }
}

async function put(root, name, bytes = 'synthetic release fixture') {
  const file = path.join(root, name);
  await fs.mkdir(path.dirname(file), { recursive: true });
  await fs.writeFile(file, bytes);
}

test('tag validation rejects paths and shell syntax', () => {
  assert.equal(releaseVersion('v0.1.1'), '0.1.1');
  for (const tag of ['0.1.1', 'v0.1.1/../../x', 'v0.1.1;exit', 'main', 'v0.1.1\n']) {
    assert.throws(() => releaseVersion(tag));
  }
  assert.equal(Object.values(assetNames('v0.1.1')).flat().length, 10);
});

test('source versions agree with the requested tag', async () => {
  await checkVersions('v0.1.1');
  await assert.rejects(checkVersions('v9.9.9'), /version mismatch/);
});

test('each platform collector requires every bundle and CLI', async () => fixture(async root => {
  const files = {
    windows: ['release/bundle/nsis/a.exe', 'release/bundle/msi/a.msi', 'release/mfmcl.exe', 'release/mfmcl-cli.exe'],
    linux: ['release/bundle/deb/a.deb', 'release/bundle/rpm/a.rpm', 'release/bundle/appimage/a.AppImage', 'release/mfmcl-cli'],
    macos: ['universal-apple-darwin/release/bundle/dmg/a.dmg', 'mfmcl-cli-universal'],
  };
  for (const [platform, entries] of Object.entries(files)) {
    const output = path.join(root, platform);
    await assert.rejects(collect(platform, 'v0.1.1', output, root));
    for (const entry of entries) await put(root, `target/${entry}`);
    const collected = await collect(platform, 'v0.1.1', output, root);
    assert.deepEqual((await fs.readdir(output)).sort(), collected.sort());
    await assert.rejects(collect(platform, 'v0.1.1', output, root), /must be empty/);
  }
}));

test('duplicate or empty build artifacts fail closed', async () => fixture(async root => {
  for (const file of ['a.exe', 'b.exe']) await put(root, `target/release/bundle/nsis/${file}`);
  await assert.rejects(collect('windows', 'v0.1.1', path.join(root, 'out'), root), /exactly one/);
  await fs.rm(path.join(root, 'target/release/bundle/nsis/b.exe'));
  await put(root, 'target/release/bundle/msi/a.msi', '');
  await assert.rejects(collect('windows', 'v0.1.1', path.join(root, 'out'), root), /empty/);
}));

test('checksum manifest covers exactly the complete asset set', async () => fixture(async root => {
  const names = Object.values(assetNames('v0.1.1')).flat();
  for (const name of names.slice(1)) await put(root, name);
  await assert.rejects(checksums('v0.1.1', root), /Incomplete/);
  await put(root, names[0]);
  await put(root, 'unknown.zip');
  await assert.rejects(checksums('v0.1.1', root), /unexpected/);
  await fs.rm(path.join(root, 'unknown.zip'));
  const manifest = await checksums('v0.1.1', root);
  assert.equal(manifest.trim().split('\n').length, 10);
  const digest = createHash('sha256').update('synthetic release fixture').digest('hex');
  for (const name of names) assert.ok(manifest.includes(`${digest}  ${name}\n`));
  assert.equal(await fs.readFile(path.join(root, 'SHA256SUMS.txt'), 'utf8'), manifest);
}));
