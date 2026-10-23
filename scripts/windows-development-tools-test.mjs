import assert from 'node:assert/strict';
import { test } from 'node:test';
import { link, lstat, mkdir, readFile, readdir, rmdir, symlink, unlink, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { downloadRedirect, extractDevelopmentZip, inspectDevelopmentTree, peIdentity, sha256, validateRelease, verifyPayload } from './windows-development-tools.mjs';
import { prepareWindowsDevelopmentNode, parseWindowsDevelopmentNodeArguments } from './package-windows-development-node.mjs';
import { prepareWindowsDevelopmentGit, parseWindowsDevelopmentGitArguments } from './package-windows-development-git.mjs';
import { dependencies, fixture, ownedRoot, pe, zip } from './windows-development-tools-test-support.mjs';

test('Both CLIs reject mutable trust, shell, URL, target and duplicate overrides', () => {
  for (const parse of [parseWindowsDevelopmentNodeArguments, parseWindowsDevelopmentGitArguments]) {
    assert.deepEqual(parse(['--output', 'new', '--cache', 'cache', '--archive', 'official.zip']), { outputDir: 'new', cacheDir: 'cache', archive: 'official.zip' });
    for (const args of [[], ['--output'], ['--output', 'a', '--output', 'b'], ['--output', 'a', '--url', 'https://example.invalid'], ['--metadata', 'custom'], ['--shell', 'cmd'], ['--version', 'latest'], ['--target', 'arm64']]) assert.throws(() => parse(args));
  }
});
test('ZIP names cannot traverse, alias, collide, create links or reinterpret file parents', () => {
  const safe = { name: 'safe.txt', bytes: Buffer.from('safe') };
  for (const entry of [
    { name: '../escape' }, { name: '/outside' }, { name: 'dir\\file' }, { name: 'CON.txt' }, { name: 'AUX' }, { name: 'LPT9.ini' },
    { name: 'ads:stream' }, { name: 'trailing.' }, { name: 'trailing ' }, { name: 'SAFE.TXT' }, { name: 'safe.txt/child' },
    { name: 'link', attributes: ((0xa000 << 16) | 0x20) >>> 0 }, { name: 'junction', attributes: 0x420 }, { name: 'encrypted', flags: 1 },
  ]) assert.throws(() => extractDevelopmentZip(zip([safe, entry])));
  assert.throws(() => extractDevelopmentZip(zip([{ name: 'dir/file' }, { name: 'dir' }])));
  assert.throws(() => extractDevelopmentZip(zip([{ name: 'e\u0301' }, { name: 'é', flags: 0x800 }])));
});
test('ZIP rejects malformed local records, CRC, overlapping data, unsupported streams and missing root', () => {
  const value = zip([{ name: 'data.txt', bytes: Buffer.from('bounded') }]);
  for (const mutate of [
    bytes => bytes.writeUInt16LE(8, 6), bytes => bytes.writeUInt32LE(0, 14), bytes => bytes.writeUInt32LE(0xffffffff, 18),
    bytes => { bytes[30 + 'data.txt'.length] ^= 1; }, bytes => bytes.writeUInt16LE(1, bytes.length - 18),
  ]) { const changed = Buffer.from(value); mutate(changed); assert.throws(() => extractDevelopmentZip(changed)); }
  assert.throws(() => extractDevelopmentZip(value.subarray(0, -1)));
  assert.throws(() => extractDevelopmentZip(Buffer.concat([value, Buffer.from('unclaimed')])));
  assert.throws(() => extractDevelopmentZip(value, { archiveRoot: 'expected' }));
});
test('File hashes cannot replace PE architecture and executable flags', () => {
  assert.equal(peIdentity(pe()).machine, 0x8664); assert.equal(peIdentity(pe({ x86: true, dll: true })).dll, true);
  const arm = pe(); arm.writeUInt16LE(0xaa64, 68); assert.throws(() => peIdentity(arm), /architecture/);
  const noExec = pe(); noExec.writeUInt16LE(0, 86); assert.throws(() => peIdentity(noExec), /flags/);
  const value = fixture('node'); const changed = structuredClone(value.release); changed.files.find(file => file.path === 'node.exe').peMachine = 0x14c;
  assert.throws(() => validateRelease(changed), /native Windows x64/);
  const badBytes = new Map(value.files); badBytes.set('node.exe', Buffer.from('not PE')); assert.throws(() => verifyPayload(badBytes, value.release));
});
test('Only the fixed GitHub release-asset HTTPS redirect class is allowed', () => {
  assert.equal(new URL(downloadRedirect('https://github.com/git-for-windows/git/releases/download/fixed/file.zip', 'https://release-assets.githubusercontent.com/asset.zip?sig=opaque')).hostname, 'release-assets.githubusercontent.com');
  for (const target of ['https://example.invalid/archive.zip', 'http://release-assets.githubusercontent.com/a', 'https://user@release-assets.githubusercontent.com/a', 'https://release-assets.githubusercontent.com:444/a']) assert.throws(() => downloadRedirect('https://github.com/fixed', target));
  assert.throws(() => downloadRedirect('https://nodejs.org/dist/fixed.zip', 'https://release-assets.githubusercontent.com/a'));
});

for (const [kind, prepare] of [['node', prepareWindowsDevelopmentNode], ['git', prepareWindowsDevelopmentGit]]) {
  test(`${kind}: packaging is new-only, uses the private tree and retains named developer evidence`, async () => {
    const value = fixture(kind); const parent = await ownedRoot(kind); const output = path.join(parent, '开发 工具'); let probes = 0;
    const result = await prepare({ outputDir: output }, dependencies(value, { probe: async checked => { probes++; assert.equal(checked.root.startsWith(parent), true); assert.equal(await readFile(checked.executable).then(sha256), value.release.files.find(file => file.path === value.release.paths.executable).sha256); } }));
    assert.equal(probes, 1); assert.equal(result.outputDir, output); assert.equal(result.tree.sha256, value.release.tree.sha256);
    assert.ok((await lstat(result.evidenceDir)).isDirectory()); await inspectDevelopmentTree(output, value);
    await assert.rejects(prepare({ outputDir: output }, dependencies(value)), /already exists/); await inspectDevelopmentTree(output, value);
  });
  test(`${kind}: explicit archives work offline, corrupt or linked cache is never repaired`, async () => {
    const value = fixture(kind); const parent = await ownedRoot(kind); const archive = path.join(parent, value.release.archiveName); const cache = path.join(parent, 'cache'); await writeFile(archive, value.archive);
    const deps = dependencies(value, { download: async () => assert.fail('Offline preparation must not download') });
    await prepare({ outputDir: path.join(parent, 'first'), archive, cacheDir: cache }, deps);
    await prepare({ outputDir: path.join(parent, 'second'), cacheDir: cache }, deps);
    const cached = path.join(cache, value.release.archiveName); await writeFile(cached, 'original invalid cache');
    await assert.rejects(prepare({ outputDir: path.join(parent, 'third'), cacheDir: cache }, deps), /SHA-256/); assert.equal(await readFile(cached, 'utf8'), 'original invalid cache');
    const linked = path.join(parent, 'linked.zip'); await link(archive, linked);
    await assert.rejects(prepare({ outputDir: path.join(parent, 'linked-out'), archive: linked }, deps), /without links/);
  });
  test(`${kind}: host, options, overlap and linked ancestors fail without touching external contents`, async () => {
    const value = fixture(kind); const parent = await ownedRoot(kind); const external = path.join(parent, 'external'); await mkdir(external); const junction = path.join(parent, 'junction'); await symlink(external, junction, process.platform === 'win32' ? 'junction' : 'dir');
    await assert.rejects(prepare({ outputDir: path.join(parent, 'wrong') }, dependencies(value, { host: { platform: 'darwin', arch: 'arm64' } })), /Windows x64/);
    await assert.rejects(prepare({ outputDir: 'relative' }, dependencies(value)), /absolute/);
    await assert.rejects(prepare({ outputDir: path.join(junction, 'new') }, dependencies(value)), /link or non-directory/);
    await assert.rejects(prepare({ outputDir: path.join(parent, 'out'), cacheDir: parent }, dependencies(value)), /overlap/);
    await assert.rejects(prepare({ outputDir: path.join(parent, 'out'), allowFallback: true }, dependencies(value)), /Unknown/); assert.deepEqual(await readdir(external), []);
  });
  test(`${kind}: changed execution tree and competing output cannot be published or overwritten`, async () => {
    const value = fixture(kind); const parent = await ownedRoot(kind); const output = path.join(parent, 'changed');
    await assert.rejects(prepare({ outputDir: output }, dependencies(value, { probe: async checked => { await writeFile(checked.executable, Buffer.alloc(256)); } })), /PE|fixed manifest/); await assert.rejects(lstat(output), { code: 'ENOENT' });
    const raced = path.join(parent, 'raced');
    await assert.rejects(prepare({ outputDir: raced }, dependencies(value, { probe: async () => { await mkdir(raced); await writeFile(path.join(raced, 'preserve.txt'), 'unrelated competing output'); } })), { code: 'EEXIST' }); assert.equal(await readFile(path.join(raced, 'preserve.txt'), 'utf8'), 'unrelated competing output');
    assert.ok((await readdir(parent)).filter(name => name.startsWith(`.development-${kind}-stage-`)).length >= 2);
  });
  test(`${kind}: installed metadata, complete notices, empty extra dirs and hardlinks remain checked`, async () => {
    const value = fixture(kind); const parent = await ownedRoot(kind); const output = path.join(parent, 'out'); await prepare({ outputDir: output }, dependencies(value));
    const notice = path.join(output, value.release.licenses[0]); const bytes = await readFile(notice); await writeFile(notice, 'shortened'); await assert.rejects(inspectDevelopmentTree(output, value), /fixed manifest/); await writeFile(notice, bytes);
    const unknown = path.join(output, 'unknown-empty'); await mkdir(unknown); await assert.rejects(inspectDevelopmentTree(output, value), /Unexpected development directory/); await rmdir(unknown);
    const metadata = path.join(output, value.metadataFile); await writeFile(metadata, '{}'); await assert.rejects(inspectDevelopmentTree(output, value), /metadata differs/); await writeFile(metadata, value.metadataBytes);
    const executable = path.join(output, value.release.paths.executable); await link(executable, path.join(parent, 'external-hardlink.exe')); await assert.rejects(inspectDevelopmentTree(output, value), /without links/);
    await unlink(path.join(parent, 'external-hardlink.exe'));
  });
}
