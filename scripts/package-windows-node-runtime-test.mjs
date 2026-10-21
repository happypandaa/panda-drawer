import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createHash } from 'node:crypto';
import { link, lstat, mkdir, mkdtemp, readFile, readdir, symlink, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { deflateRawSync } from 'node:zlib';
import { root } from './process.mjs';
import { extractWindowsRuntimeZip, packageWindowsNodeRuntime, parseWindowsRuntimeArguments, verifyWindowsRuntimeBytes, WINDOWS_NODE_RELEASE } from './package-windows-node-runtime.mjs';

const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const host = { platform: 'win32', arch: 'x64' };
const rootName = WINDOWS_NODE_RELEASE.archiveRoot;
const binary = Buffer.alloc(256);
binary.writeUInt16LE(0x5a4d, 0); binary.writeUInt32LE(64, 0x3c);
binary.writeUInt32LE(0x4550, 64); binary.writeUInt16LE(0x8664, 68);
binary.writeUInt16LE(2, 86); binary.writeUInt16LE(0x20b, 88);
const license = Buffer.from('Synthetic upstream notices; this is not an executable.\n');
function crc32(bytes) {
  let value = 0xffffffff;
  for (const byte of bytes) { value ^= byte; for (let bit = 0; bit < 8; bit++) value = (value >>> 1) ^ ((value & 1) ? 0xedb88320 : 0); }
  return (value ^ 0xffffffff) >>> 0;
}
function zip(entries) {
  const locals = []; const central = []; let offset = 0;
  for (const entry of entries) {
    const name = Buffer.from(entry.name); const bytes = entry.bytes ?? Buffer.alloc(0);
    const method = entry.method ?? 0; const packed = method === 8 ? deflateRawSync(bytes) : bytes;
    const local = Buffer.alloc(30); local.writeUInt32LE(0x04034b50); local.writeUInt16LE(20, 4);
    local.writeUInt16LE(entry.flags ?? 0, 6); local.writeUInt16LE(method, 8); local.writeUInt32LE(crc32(bytes), 14);
    local.writeUInt32LE(packed.length, 18); local.writeUInt32LE(bytes.length, 22); local.writeUInt16LE(name.length, 26);
    const record = Buffer.alloc(46); record.writeUInt32LE(0x02014b50); record.writeUInt16LE(63, 4); record.writeUInt16LE(20, 6);
    local.copy(record, 8, 6, 26); record.writeUInt16LE(name.length, 28);
    record.writeUInt32LE(entry.attributes ?? (entry.name.endsWith('/') ? 0x10 : 0x20), 38); record.writeUInt32LE(offset, 42);
    locals.push(local, name, packed); central.push(record, name); offset += local.length + name.length + packed.length;
  }
  const directory = Buffer.concat(central); const end = Buffer.alloc(22); end.writeUInt32LE(0x06054b50);
  end.writeUInt16LE(entries.length, 8); end.writeUInt16LE(entries.length, 10); end.writeUInt32LE(directory.length, 12); end.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, directory, end]);
}
function fixture(extra = [], overrides = {}) {
  const entries = [
    { name: rootName + '/' },
    { name: rootName + '/node.exe', bytes: binary, method: 8 },
    { name: rootName + '/LICENSE', bytes: license },
    { name: rootName + '/node_modules/npm/unneeded.js', bytes: Buffer.from('never copied') },
    ...extra,
  ];
  const archive = zip(entries); const release = structuredClone(WINDOWS_NODE_RELEASE);
  release.source.sha256 = digest(archive); release.binary.sha256 = digest(binary); release.binary.size = binary.length; release.license.sha256 = digest(license);
  return { archive, release, ...overrides };
}
async function ownedRoot() {
  const parent = path.join(root, '.test-artifacts'); await mkdir(parent, { recursive: true });
  return mkdtemp(path.join(parent, 'windows-node-package-fixture-'));
}
function dependencies(value, extra = {}) {
  return {
    host, release: value.release,
    download: async ({ url, destination }) => { assert.equal(url, WINDOWS_NODE_RELEASE.source.url); await writeFile(destination, value.archive, { flag: 'wx' }); },
    probe: async absolute => { assert.ok(path.isAbsolute(absolute)); assert.equal(path.basename(absolute), 'node.exe'); assert.deepEqual(await readFile(absolute), binary); return { version: '24.22.0', ...host }; },
    ...extra,
  };
}

test('Windows release pins the official ZIP, PE executable and complete license independently', () => {
  assert.equal(WINDOWS_NODE_RELEASE.source.sha256, 'f9683a19877e12a9b167305c8db71040d812876359de21f0d2e97c3f321f80c3');
  assert.equal(WINDOWS_NODE_RELEASE.binary.sha256, 'ba93f9a70bd86fce3ebbb39ec758501458453467aa92a34a0a3602d1e3fc2b9c');
  assert.equal(WINDOWS_NODE_RELEASE.license.sha256, 'ed34dd8e3f0a78dbaf00d0444ce8e285b015b765379c2e17880455f70370f8e9');
  assert.equal(WINDOWS_NODE_RELEASE.binary.path, 'node.exe');
});
test('CLI requires a new output and rejects URL, metadata, version, duplicate and unknown overrides', () => {
  assert.deepEqual(parseWindowsRuntimeArguments(['--output', 'new', '--archive', 'official.zip', '--cache', 'cache']), { output: 'new', archive: 'official.zip', cache: 'cache' });
  for (const args of [[], ['--output'], ['--output', 'a', '--output', 'b'], ['--url', 'https://example.invalid'], ['--metadata', 'custom.json'], ['--version', '24.22.0'], ['--probe', 'node']]) assert.throws(() => parseWindowsRuntimeArguments(args));
});
test('Strict ZIP parsing selects only node.exe and LICENSE, including deflate members', () => {
  const value = fixture(); const extracted = extractWindowsRuntimeZip(value.archive, value.release);
  assert.deepEqual(extracted, { binary, license }); verifyWindowsRuntimeBytes(extracted, value.release);
});
test('ZIP traversal, aliases, duplicate case names, linked members and file-directory collisions are refused', () => {
  for (const entry of [
    { name: rootName + '/../escape' }, { name: '/outside/node.exe' }, { name: rootName + '/a\\b' },
    { name: rootName + '/CON.txt' }, { name: rootName + '/alias. ' }, { name: rootName + '/name:stream' },
    { name: rootName + '/NODE.EXE' }, { name: rootName + '/link', attributes: ((0xa000 << 16) | 0x20) >>> 0 },
    { name: rootName + '/node.exe/escape' }, { name: rootName + '/secret', flags: 1 },
  ]) assert.throws(() => extractWindowsRuntimeZip(fixture([entry]).archive));
});
test('ZIP central/local disagreements, missing members, truncated data and trailing bytes are refused', () => {
  const value = fixture(); const changed = Buffer.from(value.archive);
  changed.writeUInt16LE(8, 6); assert.throws(() => extractWindowsRuntimeZip(changed));
  assert.throws(() => extractWindowsRuntimeZip(value.archive.subarray(0, -1)));
  assert.throws(() => extractWindowsRuntimeZip(Buffer.concat([value.archive, Buffer.from('tail')])));
  assert.throws(() => extractWindowsRuntimeZip(zip([{ name: rootName + '/LICENSE', bytes: license }])));
  const badCrc = Buffer.from(value.archive); const dataStart = 30 + Buffer.byteLength(rootName + '/') + 30 + Buffer.byteLength(rootName + '/node.exe');
  badCrc[dataStart + 2] ^= 1; assert.throws(() => extractWindowsRuntimeZip(badCrc));
});
test('Hashes cannot substitute for a native x64 PE signature or complete upstream license', () => {
  const value = fixture(); const arm = Buffer.from(binary); arm.writeUInt16LE(0xaa64, 68);
  const release = structuredClone(value.release); release.binary.sha256 = digest(arm);
  assert.throws(() => verifyWindowsRuntimeBytes({ binary: arm, license }, release), /x64 PE/);
  assert.throws(() => verifyWindowsRuntimeBytes({ binary, license: Buffer.from('shortened notice') }, value.release), /fixed release/);
});
test('Packaging probes an absolute private executable and publishes exactly three new files without npm', async () => {
  const parent = await ownedRoot(); const output = path.join(parent, 'new runtime'); const value = fixture();
  const result = await packageWindowsNodeRuntime({ output }, dependencies(value));
  assert.deepEqual((await readdir(output)).sort(), ['LICENSE', 'node.exe', 'runtime.json']);
  assert.deepEqual(await readFile(path.join(output, 'node.exe')), binary);
  assert.deepEqual(await readFile(path.join(output, 'LICENSE')), license);
  assert.equal(result.runtime.platform, 'win32'); assert.equal(result.runtime.binary.sha256, value.release.binary.sha256);
  assert.equal((await readdir(parent)).some(name => name.startsWith('.windows-node-runtime-stage-')), false);
  await assert.rejects(packageWindowsNodeRuntime({ output }, dependencies(value)), /already exists/);
  assert.deepEqual(await readFile(path.join(output, 'node.exe')), binary);
});
test('Explicit local archives and pinned cache avoid downloads while corrupt caches are never replaced', async () => {
  const parent = await ownedRoot(); const value = fixture(); const source = path.join(parent, 'official.zip'); const cache = path.join(parent, 'cache');
  await writeFile(source, value.archive);
  const deps = dependencies(value, { download: async () => assert.fail('Offline fixture must not download') });
  await packageWindowsNodeRuntime({ output: path.join(parent, 'first'), archive: source, cache }, deps);
  await packageWindowsNodeRuntime({ output: path.join(parent, 'second'), cache }, deps);
  const cached = path.join(cache, WINDOWS_NODE_RELEASE.archiveName); await writeFile(cached, 'corrupt original cache');
  await assert.rejects(packageWindowsNodeRuntime({ output: path.join(parent, 'third'), cache }, deps), /SHA-256/);
  assert.equal(await readFile(cached, 'utf8'), 'corrupt original cache');
});
test('Linked input files, junction ancestors, relative paths and existing outputs are preserved and refused', async () => {
  const parent = await ownedRoot(); const value = fixture(); const source = path.join(parent, 'source.zip');
  await writeFile(source, value.archive); const linked = path.join(parent, 'linked.zip'); await link(source, linked);
  await assert.rejects(packageWindowsNodeRuntime({ output: path.join(parent, 'hardlink-out'), archive: linked }, dependencies(value)), /without links/);
  await assert.rejects(packageWindowsNodeRuntime({ output: 'relative' }, dependencies(value)), /absolute/);
  const external = path.join(parent, 'external'); await mkdir(external);
  const junction = path.join(parent, 'junction'); await symlink(external, junction, process.platform === 'win32' ? 'junction' : 'dir');
  await assert.rejects(packageWindowsNodeRuntime({ output: path.join(junction, 'new') }, dependencies(value)), /link or non-directory/);
  assert.deepEqual(await readdir(external), []);
  assert.ok((await lstat(linked)).isFile());
});
test('Wrong host, wrong probe identity and post-probe tampering cannot publish a runtime', async () => {
  const parent = await ownedRoot(); const value = fixture();
  await assert.rejects(packageWindowsNodeRuntime({ output: path.join(parent, 'host') }, dependencies(value, { host: { platform: 'darwin', arch: 'arm64' } })), /Windows x64/);
  const wrong = path.join(parent, 'wrong');
  await assert.rejects(packageWindowsNodeRuntime({ output: wrong }, dependencies(value, { probe: async () => ({ version: '24.22.0', platform: 'win32', arch: 'arm64' }) })), /incompatible identity/);
  await assert.rejects(lstat(wrong), { code: 'ENOENT' });
  const tampered = path.join(parent, 'tampered');
  await assert.rejects(packageWindowsNodeRuntime({ output: tampered }, dependencies(value, { probe: async file => { await writeFile(file, Buffer.alloc(binary.length)); return { version: '24.22.0', ...host }; } })), /fixed release/);
  await assert.rejects(lstat(tampered), { code: 'ENOENT' });
});
