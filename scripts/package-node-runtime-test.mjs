import assert from 'node:assert/strict';
import test from 'node:test';
import { createHash } from 'node:crypto';
import { chmod, link, lstat, mkdir, mkdtemp, readFile, readdir, realpath, rm, symlink, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { gzipSync } from 'node:zlib';
import { extractRuntimeTar, NODE_RELEASE, packageNodeRuntime, parseRuntimeArguments, verifyRuntimeBytes } from './package-node-runtime.mjs';
import { repairGeneratedNodeRuntimePermissions } from './prepare-desktop.mjs';

const digest = (bytes) => createHash('sha256').update(bytes).digest('hex');
const host = { platform: 'darwin', arch: 'arm64' };
const rootName = NODE_RELEASE.archiveRoot;
const binary = Buffer.alloc(32, 1);
binary.writeUInt32LE(0xfeedfacf, 0);
binary.writeUInt32LE(0x0100000c, 4);
const license = Buffer.from('Artificial fixture license; never a real executable.\n');

function tarEntry(name, bytes = Buffer.alloc(0), type = '0') {
  const header = Buffer.alloc(512);
  header.write(name, 0, 100, 'utf8');
  header.write('0000755\0', 100, 8);
  header.write('0000000\0', 108, 8);
  header.write('0000000\0', 116, 8);
  header.write(`${bytes.length.toString(8).padStart(11, '0')}\0`, 124, 12);
  header.write('00000000000\0', 136, 12);
  header.fill(32, 148, 156);
  header.write(type, 156, 1);
  header.write('ustar\0', 257, 6);
  header.write('00', 263, 2);
  const checksum = header.reduce((sum, byte) => sum + byte, 0);
  header.write(`${checksum.toString(8).padStart(6, '0')}\0 `, 148, 8);
  return Buffer.concat([header, bytes, Buffer.alloc((512 - bytes.length % 512) % 512)]);
}

function tarArchive(entries) { return Buffer.concat([...entries, Buffer.alloc(1024)]); }
function fixture(extra = []) {
  const tar = tarArchive([
    tarEntry(`${rootName}/`, Buffer.alloc(0), '5'),
    tarEntry(`${rootName}/bin/node`, binary),
    tarEntry(`${rootName}/LICENSE`, license),
    tarEntry(`${rootName}/lib/node_modules/npm/unneeded.js`, Buffer.from('not copied')),
    ...extra,
  ]);
  const archive = gzipSync(tar);
  const release = structuredClone(NODE_RELEASE);
  release.source.sha256 = digest(archive);
  release.binary.sha256 = digest(binary);
  release.binary.size = binary.length;
  release.license.sha256 = digest(license);
  return { tar, archive, release };
}

async function ownedRoot(t) {
  const root = await mkdtemp(path.join(await realpath(os.tmpdir()), 'postgresui-node-runtime-fixture-'));
  t.after(async () => {
    async function makeWritable(target) {
      const info = await lstat(target);
      if (!info.isDirectory() || info.isSymbolicLink()) return;
      await chmod(target, 0o700);
      for (const name of await readdir(target)) await makeWritable(path.join(target, name));
    }
    await makeWritable(root);
    await rm(root, { recursive: true });
  });
  return root;
}

function dependencies(value, overrides = {}) {
  return {
    host, release: value.release,
    download: async ({ url, destination }) => {
      assert.equal(url, NODE_RELEASE.source.url);
      await writeFile(destination, value.archive, { flag: 'wx' });
    },
    probe: async (absoluteBinary) => {
      // Fixture bytes are only read, never executed. The real probe is covered
      // separately using the pinned, checksum-verified official archive.
      assert.ok(path.isAbsolute(absoluteBinary));
      assert.deepEqual(await readFile(absoluteBinary), binary);
      return { version: '24.22.0', ...host };
    },
    ...overrides,
  };
}

test('CLI requires a new explicit output and rejects all source overrides', () => {
  assert.deepEqual(parseRuntimeArguments(['--output', '/new', '--cache', '/cache']), { outputDir: '/new', cacheDir: '/cache' });
  for (const args of [[], ['--output'], ['--output', '/a', '--output', '/b'], ['--url', 'https://example.invalid'], ['--metadata', '/local.json'], ['--version', '24.22.0']]) {
    assert.throws(() => parseRuntimeArguments(args));
  }
});

test('fixed release metadata has reviewed archive and binary hashes', () => {
  assert.equal(NODE_RELEASE.source.sha256, '6a9724d9b50a5f22b4816cded441b5a8d9ddae546191a5e5b7efa5a5e2ee26d5');
  assert.equal(NODE_RELEASE.binary.sha256, 'b468fb9c67427c9d3aec07ee1105e63020923a631a9a2a00beb71a7ba3768bfd');
  assert.equal(NODE_RELEASE.license.sha256, '5888dbb9a1d2b18f2c3e6c5f6af1b39de658372b402a0577b002777f14c62ace');
});

test('unsupported hosts fail before download or output creation', async (t) => {
  const root = await ownedRoot(t);
  for (const unsupported of [{ platform: 'win32', arch: 'x64' }, { platform: 'linux', arch: 'arm64' }, { platform: 'darwin', arch: 'x64' }]) {
    await assert.rejects(packageNodeRuntime({ output: path.join(root, 'new') }, dependencies(fixture(), {
      host: unsupported, download: () => assert.fail('unsupported host downloaded'), probe: () => assert.fail('unsupported host executed'),
    })), /macOS arm64 only/);
  }
  assert.deepEqual(await readdir(root), []);
});

test('verified fixture publishes only binary, complete LICENSE and exact metadata with packaging-compatible permissions', async (t) => {
  const root = await ownedRoot(t);
  const value = fixture();
  const output = path.join(root, 'private runtime 中文');
  const result = await packageNodeRuntime({ output, cache: path.join(root, 'cache') }, dependencies(value));
  assert.equal(result.outputDir, output);
  assert.deepEqual((await readdir(output)).sort(), ['LICENSE', 'bin', 'runtime.json']);
  assert.deepEqual(await readdir(path.join(output, 'bin')), ['node']);
  assert.deepEqual(await readFile(path.join(output, 'bin/node')), binary);
  assert.deepEqual(await readFile(path.join(output, 'LICENSE')), license);
  const metadata = JSON.parse(await readFile(path.join(output, 'runtime.json'), 'utf8'));
  assert.deepEqual(Object.keys(metadata).sort(), ['arch', 'binary', 'license', 'platform', 'schemaVersion', 'source', 'version']);
  assert.deepEqual(metadata, result.runtime);
  assert.equal((await lstat(path.join(output, 'bin/node'))).mode & 0o777, 0o755);
  assert.equal((await lstat(path.join(output, 'LICENSE'))).mode & 0o777, 0o644);
  assert.equal((await lstat(path.join(output, 'runtime.json'))).mode & 0o777, 0o644);
  assert.equal((await lstat(output)).mode & 0o777, 0o755);
  assert.equal((await lstat(path.join(output, 'bin'))).mode & 0o777, 0o755);
  assert.equal((await lstat(path.join(output, 'bin/node'))).mode & 0o111, 0o111);
  assert.equal((await lstat(path.join(root, 'cache', NODE_RELEASE.archiveName))).nlink, 1);
  assert.equal((await readdir(root)).filter((name) => name.startsWith('.node-runtime-')).length, 0);
});

test('validated cached bytes are reused without download', async (t) => {
  const root = await ownedRoot(t);
  const value = fixture();
  const cache = path.join(root, 'cache');
  await mkdir(cache);
  await writeFile(path.join(cache, NODE_RELEASE.archiveName), value.archive);
  await packageNodeRuntime({ output: path.join(root, 'new'), cache }, dependencies(value, { download: () => assert.fail('cache should be used') }));
});

test('existing caller output, including empty directories or dangling links, is never replaced', async (t) => {
  const root = await ownedRoot(t);
  const value = fixture();
  const present = path.join(root, 'present');
  const empty = path.join(root, 'empty');
  const dangling = path.join(root, 'dangling');
  await mkdir(present); await mkdir(empty);
  await writeFile(path.join(present, 'keep'), 'caller data');
  await symlink(path.join(root, 'missing'), dangling);
  for (const output of [present, empty, dangling]) await assert.rejects(packageNodeRuntime({ output }, dependencies(value)), /already exists/);
  assert.equal(await readFile(path.join(present, 'keep'), 'utf8'), 'caller data');
  assert.deepEqual(await readdir(empty), []);
  assert.ok((await lstat(dangling)).isSymbolicLink());
});

test('parent symlinks, parent traversal and cache overlap are rejected without touching caller files', async (t) => {
  const root = await ownedRoot(t);
  const real = path.join(root, 'real'); await mkdir(real);
  const alias = path.join(root, 'alias'); await symlink(real, alias);
  const value = fixture();
  for (const options of [
    { output: path.join(alias, 'new') },
    { output: `${root}/../outside` },
    { output: path.join(root, 'new'), cache: root },
    { output: path.join(root, 'new'), cache: path.join(root, 'new', 'cache') },
    { output: path.join(root, 'new'), cache: alias },
  ]) await assert.rejects(packageNodeRuntime(options, dependencies(value)));
  assert.deepEqual(await readdir(real), []);
});

test('corrupt, linked or non-file cache entries are preserved and never executed', async (t) => {
  const root = await ownedRoot(t);
  const value = fixture();
  const cache = path.join(root, 'cache'); await mkdir(cache);
  const cacheFile = path.join(cache, NODE_RELEASE.archiveName);
  await writeFile(cacheFile, 'corrupt caller cache');
  const run = () => packageNodeRuntime({ output: path.join(root, 'new'), cache }, dependencies(value, { probe: () => assert.fail('must not execute corrupt cache') }));
  await assert.rejects(run(), /SHA-256/);
  assert.equal(await readFile(cacheFile, 'utf8'), 'corrupt caller cache');
  await rm(cacheFile);
  const source = path.join(root, 'original'); await writeFile(source, value.archive);
  await symlink(source, cacheFile); await assert.rejects(run(), /ordinary file/);
  assert.ok((await lstat(cacheFile)).isSymbolicLink());
  await rm(cacheFile); await link(source, cacheFile); await assert.rejects(run(), /ordinary file/);
  assert.equal((await lstat(cacheFile)).nlink, 2);
  await rm(cacheFile); await mkdir(cacheFile); await assert.rejects(run(), /ordinary file/);
});

test('download digest failures clean only owned staging and do not execute fixture bytes', async (t) => {
  const root = await ownedRoot(t);
  await writeFile(path.join(root, 'keep'), 'untouched');
  await assert.rejects(packageNodeRuntime({ output: path.join(root, 'new') }, dependencies(fixture(), {
    download: async ({ destination }) => writeFile(destination, 'wrong archive', { flag: 'wx' }),
    probe: () => assert.fail('checksum failure must happen before probe'),
  })), /SHA-256/);
  assert.deepEqual(await readdir(root), ['keep']);
});

test('fixed binary and LICENSE digests reject altered contents before probe', async (t) => {
  const root = await ownedRoot(t);
  for (const member of ['binary', 'license']) {
    const value = fixture(); value.release[member].sha256 = 'a'.repeat(64);
    await assert.rejects(packageNodeRuntime({ output: path.join(root, member) }, dependencies(value, { probe: () => assert.fail('unverified file executed') })), /differs from the fixed release/);
  }
  assert.deepEqual(await readdir(root), []);
});

test('identity mismatch does not commit output or remove unrelated directories', async (t) => {
  const root = await ownedRoot(t);
  const keep = path.join(root, 'keep'); await mkdir(keep);
  await assert.rejects(packageNodeRuntime({ output: path.join(root, 'new') }, dependencies(fixture(), {
    probe: async () => ({ version: '24.22.0', platform: 'darwin', arch: 'x64' }),
  })), /incompatible identity/);
  assert.deepEqual(await readdir(root), ['keep']);
});

test('a concurrent output created before commit is not replaced or deleted', async (t) => {
  const root = await ownedRoot(t);
  const output = path.join(root, 'new');
  await assert.rejects(packageNodeRuntime({ output }, dependencies(fixture(), {
    probe: async () => { await mkdir(output); await writeFile(path.join(output, 'caller'), 'keep'); return { version: '24.22.0', ...host }; },
  })), /EEXIST/);
  assert.equal(await readFile(path.join(output, 'caller'), 'utf8'), 'keep');
});

test('tar parser refuses traversal, duplicate required files, required links and truncation', () => {
  for (const extra of [
    tarEntry(`${rootName}/../escape`, Buffer.from('bad')),
    tarEntry(`${rootName}/bin/node`, binary),
    tarEntry(`${rootName}/LICENSE`, Buffer.alloc(0), '2'),
  ]) assert.throws(() => extractRuntimeTar(fixture([extra]).tar), /Unsafe|duplicate|linked/);
  const value = fixture();
  assert.throws(() => extractRuntimeTar(value.tar.subarray(0, value.tar.length - 512)), /end marker/);
  const corrupted = Buffer.from(value.tar); corrupted[0] ^= 1;
  assert.throws(() => extractRuntimeTar(corrupted), /checksum/);
});

test('binary architecture is checked independently of a supplied matching digest', () => {
  const altered = Buffer.from(binary); altered.writeUInt32LE(0x01000007, 4);
  const value = fixture(); value.release.binary.sha256 = digest(altered);
  assert.throws(() => verifyRuntimeBytes({ binary: altered, license }, value.release), /native arm64 Mach-O/);
});

test('desktop rebuild repairs only known stale generated runtime modes without changing contents', async (t) => {
  const root = await ownedRoot(t);
  const copy = path.join(root, 'node-runtime');
  await mkdir(copy); await mkdir(path.join(copy, 'bin'));
  await writeFile(path.join(copy, 'bin/node'), binary, { mode: 0o555 });
  await writeFile(path.join(copy, 'LICENSE'), license, { mode: 0o444 });
  await writeFile(path.join(copy, 'runtime.json'), '{}', { mode: 0o444 });
  await chmod(path.join(copy, 'bin'), 0o555); await chmod(copy, 0o555);
  await repairGeneratedNodeRuntimePermissions(copy);
  assert.equal((await lstat(path.join(copy, 'bin/node'))).mode & 0o777, 0o755);
  assert.equal((await lstat(path.join(copy, 'LICENSE'))).mode & 0o777, 0o644);
  assert.equal((await lstat(path.join(copy, 'runtime.json'))).mode & 0o777, 0o644);
  assert.deepEqual(await readFile(path.join(copy, 'bin/node')), binary);
  assert.deepEqual(await readFile(path.join(copy, 'LICENSE')), license);
  const linkCopy = path.join(root, 'linked-runtime'); await symlink(copy, linkCopy);
  await assert.rejects(repairGeneratedNodeRuntimePermissions(linkCopy), /ordinary canonical/);
  await writeFile(path.join(copy, 'unrecognized'), 'caller data');
  await assert.rejects(repairGeneratedNodeRuntimePermissions(copy), /unexpected entries/);
  assert.equal(await readFile(path.join(copy, 'unrecognized'), 'utf8'), 'caller data');
});
