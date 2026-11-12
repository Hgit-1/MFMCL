import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

export function releaseVersion(tag) {
  assert.equal(tag, tag.trim(), 'Release tags cannot contain whitespace');
  assert.match(tag, /^v\d+\.\d+\.\d+$/, 'Use an existing vMAJOR.MINOR.PATCH tag');
  return tag.slice(1);
}

export function assetNames(tag) {
  const version = releaseVersion(tag);
  return {
    windows: [
      `MFMCL_${version}_windows_x64-setup.exe`,
      `MFMCL_${version}_windows_x64.msi`,
      `MFMCL_${version}_windows_x64.exe`,
      `mfmcl-cli_${version}_windows_x64.exe`,
    ],
    linux: [
      `MFMCL_${version}_linux_amd64.deb`,
      `MFMCL_${version}_linux_x86_64.rpm`,
      `MFMCL_${version}_linux_amd64.AppImage`,
      `mfmcl-cli_${version}_linux_x64`,
    ],
    macos: [
      `MFMCL_${version}_macos_universal.dmg`,
      `mfmcl-cli_${version}_macos_universal`,
    ],
  };
}

export async function checkVersions(tag, root = ROOT) {
  const version = releaseVersion(tag);
  const cargo = await fs.readFile(path.join(root, 'Cargo.toml'), 'utf8');
  const workspace = cargo.match(/\[workspace\.package\]([\s\S]*?)(?:\n\[|$)/)?.[1];
  assert.equal(workspace?.match(/^version\s*=\s*"([^"]+)"/m)?.[1], version, 'Cargo workspace version mismatch');
  for (const file of ['package.json', 'src-tauri/tauri.conf.json']) {
    const manifest = JSON.parse(await fs.readFile(path.join(root, file), 'utf8'));
    assert.equal(manifest.version, version, `${file} version mismatch`);
  }
  const auth = await fs.readFile(path.join(root, 'crates/launcher-auth/Cargo.toml'), 'utf8');
  assert.equal(auth.match(/^version\s*=\s*"([^"]+)"/m)?.[1], version, 'Authentication crate version mismatch');
  const lock = (await fs.readFile(path.join(root, 'Cargo.lock'), 'utf8')).replaceAll('\r\n', '\n');
  for (const name of ['mc-metadata', 'download-engine', 'launcher-core', 'launcher-auth', 'launcher-runtime', 'mfmcl-cli', 'crash-ai', 'mfmcl-desktop']) {
    const record = lock.split('[[package]]').find(block => block.includes(`\nname = "${name}"\n`));
    assert.equal(record?.match(/^version = "([^"]+)"/m)?.[1], version, `${name} lockfile mismatch`);
  }
}

async function requiredFile(file) {
  const stat = await fs.lstat(file);
  assert.ok(stat.isFile() && !stat.isSymbolicLink() && stat.size > 0, `Missing, empty or non-regular artifact: ${file}`);
  return file;
}

async function singleArtifact(directory, suffix) {
  const matches = (await fs.readdir(directory)).filter(name => name.endsWith(suffix));
  assert.equal(matches.length, 1, `Expected exactly one ${suffix} in ${directory}`);
  return requiredFile(path.join(directory, matches[0]));
}

export async function collect(platform, tag, output, root = ROOT) {
  const names = assetNames(tag)[platform];
  assert.ok(names, 'Unknown release platform');
  const target = path.join(root, 'target');
  const release = path.join(target, 'release');
  let sources;
  if (platform === 'windows') {
    sources = [
      await singleArtifact(path.join(release, 'bundle/nsis'), '.exe'),
      await singleArtifact(path.join(release, 'bundle/msi'), '.msi'),
      path.join(release, 'mfmcl.exe'),
      path.join(release, 'mfmcl-cli.exe'),
    ];
  } else if (platform === 'linux') {
    sources = [
      await singleArtifact(path.join(release, 'bundle/deb'), '.deb'),
      await singleArtifact(path.join(release, 'bundle/rpm'), '.rpm'),
      await singleArtifact(path.join(release, 'bundle/appimage'), '.AppImage'),
      path.join(release, 'mfmcl-cli'),
    ];
  } else {
    sources = [
      await singleArtifact(path.join(target, 'universal-apple-darwin/release/bundle/dmg'), '.dmg'),
      path.join(target, 'mfmcl-cli-universal'),
    ];
  }
  await Promise.all(sources.map(requiredFile));
  await fs.mkdir(output, { recursive: true });
  assert.equal((await fs.readdir(output)).length, 0, 'Artifact output must be empty');
  for (const [index, source] of sources.entries()) {
    await fs.copyFile(source, path.join(output, names[index]));
  }
  return names;
}

export async function checksums(tag, directory) {
  const expected = Object.values(assetNames(tag)).flat().sort();
  const actual = (await fs.readdir(directory)).sort();
  assert.deepEqual(actual, expected, 'Incomplete or unexpected release asset set');
  const lines = [];
  for (const name of expected) {
    const file = await requiredFile(path.join(directory, name));
    const digest = createHash('sha256').update(await fs.readFile(file)).digest('hex');
    lines.push(`${digest}  ${name}`);
  }
  const manifest = `${lines.join('\n')}\n`;
  await fs.writeFile(path.join(directory, 'SHA256SUMS.txt'), manifest);
  return manifest;
}

async function main(args) {
  const [command, ...rest] = args;
  if (command === 'check' && rest.length === 1) {
    await checkVersions(rest[0]);
    console.log(`Release version declarations agree: ${rest[0]}`);
  } else if (command === 'collect' && rest.length === 3) {
    const names = await collect(rest[0], rest[1], path.resolve(rest[2]));
    console.log(`Collected ${names.length} required ${rest[0]} artifacts`);
  } else if (command === 'checksums' && rest.length === 2) {
    process.stdout.write(await checksums(rest[0], path.resolve(rest[1])));
  } else {
    throw new Error('Usage: release.mjs check TAG | collect PLATFORM TAG OUTPUT | checksums TAG DIRECTORY');
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2)).catch(error => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
