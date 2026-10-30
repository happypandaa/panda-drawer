import assert from 'node:assert/strict';
import test from 'node:test';
import { chmod, link, lstat, mkdir, mkdtemp, readFile, readdir, realpath, rename, rm, symlink, truncate, unlink, writeFile } from 'node:fs/promises';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { createHash } from 'node:crypto';
import os from 'node:os';
import path from 'node:path';
import { DEVELOPMENT_GIT_RELEASE, gitTreeDigest, parseDevelopmentGitArguments, prepareDevelopmentGit, verifyDevelopmentGit as verifyMacDevelopmentGit } from './package-development-git.mjs';
import { verifyDevelopmentGitWindows } from './package-development-git-windows.mjs';
import { directoryLink, fileSymlinkFixture, posixPermissions } from './package-test-fixtures.mjs';

const execute = promisify(execFile);
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const macArm = process.platform === 'darwin' && process.arch === 'arm64';
const metadataName = 'development-git.json';
const releaseBytes = await readFile(new URL('./development-git-release.json', import.meta.url));
const windowsDependencies = process.env.WINDOWS_NATIVE_PACKAGER_HELPER ? { helper: process.env.WINDOWS_NATIVE_PACKAGER_HELPER } : {};
const verifyDevelopmentGit = process.platform === 'win32'
  ? root => verifyDevelopmentGitWindows(root, windowsDependencies) : verifyMacDevelopmentGit;
const verificationMetadata = process.platform === 'win32'
  ? await readFile(new URL('./development-git-release.windows-x64.json', import.meta.url)) : releaseBytes;

async function disposable(t) {
  const root = await mkdtemp(path.join(await realpath(os.tmpdir()), 'disposable-development-git-'));
  await chmod(root, 0o755);
  t.after(() => rm(root, { recursive: true, force: true }));
  return root;
}

async function fakePayload(root) {
  const payload = path.join(root, 'payload');
  await mkdir(payload, { mode: 0o755 });
  await writeFile(path.join(payload, metadataName), verificationMetadata, { mode: 0o644 });
  return payload;
}

async function assertMissing(file) {
  await assert.rejects(lstat(file), { code: 'ENOENT' });
}

test('Git preparation CLI accepts only explicit output/cache arguments', () => {
  assert.deepEqual(
    parseDevelopmentGitArguments(['--output', '/disposable/new', '--cache', '/disposable/cache']),
    { outputDir: '/disposable/new', cacheDir: '/disposable/cache' },
  );
  for (const args of [
    [], ['--output'], ['--cache', '/cache'], ['--output', '/a', '--output', '/b'],
    ['--output', '/a', '--cache', '/b', '--cache', '/c'],
    ['--url', 'https://example.invalid'], ['--metadata', '/override'],
    ['--version', '99.0.0'], ['--verify', '/a'],
  ]) assert.throws(() => parseDevelopmentGitArguments(args));
});

test('Git trust anchor comes from the fixed release metadata', () => {
  const source = JSON.parse(releaseBytes);
  assert.deepEqual(DEVELOPMENT_GIT_RELEASE, source);
  assert.equal(source.platform, 'darwin');
  assert.equal(source.arch, 'arm64');
  assert.equal(source.gitVersion, '2.56.0');
  assert.equal(source.tree.rule, 'sorted-utf8-path-nul-size-nul-sha256-lf-v1');
  assert.match(source.tree.sha256, /^[a-f0-9]{64}$/);
  assert.ok(Number.isSafeInteger(source.tree.fileCount) && source.tree.fileCount > 1);
  assert.ok(Number.isSafeInteger(source.tree.totalBytes) && source.tree.totalBytes > 0);
  assert.ok(source.executables.includes('bin/git'));
});

test('Git tree digest sorts UTF-8 paths and binds file names, lengths and contents', () => {
  const entries = [
    ['中 文', Buffer.from('unicode')],
    ['z', Buffer.from('same')],
    ['é', Buffer.from('accent')],
    ['a', Buffer.from('other')],
  ];
  const sorted = [...entries].sort(([a], [b]) => Buffer.compare(Buffer.from(a), Buffer.from(b)));
  const expected = sorted.map(([name, bytes]) => name + '\0' + bytes.length + '\0' + digest(bytes) + '\n').join('');
  const result = gitTreeDigest(entries);
  assert.deepEqual(result, gitTreeDigest([...entries].reverse()));
  assert.equal(result.sha256, digest(Buffer.from(expected)));
  assert.equal(result.fileCount, entries.length);
  assert.equal(result.totalBytes, entries.reduce((sum, [, bytes]) => sum + bytes.length, 0));
  assert.notEqual(result.sha256, gitTreeDigest([['renamed', entries[0][1]], ...entries.slice(1)]).sha256);
  assert.notEqual(result.sha256, gitTreeDigest([[entries[0][0], Buffer.from('changed')], ...entries.slice(1)]).sha256);
  for (const name of ['../outside', '/absolute', 'a/../b', 'a//b', 'a\\b', 'control\npath']) {
    assert.throws(() => gitTreeDigest([[name, Buffer.alloc(0)]]));
  }
});

test('Git verification rejects self-reported metadata and never executes unverified Git', async t => {
  const root = await disposable(t);
  const payload = await fakePayload(root);
  await mkdir(path.join(payload, 'bin'), { mode: 0o755 });
  const sentinel = path.join(root, 'unexpected-execution');
  const fakeGit = '#!/bin/sh\n/usr/bin/touch "' + sentinel + '"\n';
  await writeFile(path.join(payload, 'bin/git'), fakeGit, { mode: 0o755 });
  await writeFile(path.join(payload, metadataName), JSON.stringify({
    ...JSON.parse(verificationMetadata), tree: { sha256: 'self-reported' },
  }));
  await assert.rejects(verifyDevelopmentGit(payload), /metadata differs/);
  await assertMissing(sentinel);
  // A semantically identical JSON document must not replace the fixed bytes.
  await writeFile(path.join(payload, metadataName), Buffer.concat([verificationMetadata, Buffer.from('\n')]));
  await assert.rejects(verifyDevelopmentGit(payload), /metadata differs/);
  await assertMissing(sentinel);
  await writeFile(path.join(payload, metadataName), verificationMetadata);
  await assert.rejects(verifyDevelopmentGit(payload), /tree differs/);
  await assertMissing(sentinel);
});

test('Git verification refuses linked ancestors, linked members and hardlinks', async t => {
  const root = await disposable(t);
  const payload = await fakePayload(root);
  const alias = path.join(root, 'alias');
  await directoryLink(payload, alias);
  await assert.rejects(verifyDevelopmentGit(alias), /ordinary directory|[Rr]eparse/);
  await mkdir(path.join(payload, 'nested'), { mode: 0o755 });
  await writeFile(path.join(payload, 'nested', 'member'), 'keep', { mode: 0o644 });
  await assert.rejects(verifyDevelopmentGit(path.join(alias, 'nested')), /ordinary directory|[Rr]eparse/);
  const outside = path.join(root, 'caller-owned');
  await writeFile(outside, 'caller bytes', { mode: 0o644 });
  const linked = path.join(payload, 'linked');
  await t.test('file symlink member', async t => {
    if (!await fileSymlinkFixture(t, outside, linked)) return;
    try { await assert.rejects(verifyDevelopmentGit(payload), /symbolic link|[Rr]eparse/); }
    finally { await unlink(linked); }
  });
  await link(outside, linked);
  await assert.rejects(verifyDevelopmentGit(payload), /without links|ordinary-file read policy/);
  await unlink(linked);
  const metadata = path.join(payload, metadataName);
  await unlink(metadata);
  await t.test('file symlink metadata', async t => {
    if (!await fileSymlinkFixture(t, outside, metadata)) return;
    await assert.rejects(verifyDevelopmentGit(payload), /symbolic link|[Rr]eparse/);
  });
  assert.equal(await readFile(outside, 'utf8'), 'caller bytes');
  assert.equal((await lstat(outside)).nlink, 1);
});

test('Git verification rejects writable or special-bit payload permissions', posixPermissions, async t => {
  const root = await disposable(t);
  const payload = await fakePayload(root);
  const metadata = path.join(payload, metadataName);
  try {
    for (const mode of [0o664, 0o666, 0o4644]) {
      await chmod(metadata, mode);
      await assert.rejects(verifyDevelopmentGit(payload), /permission|mode/i);
    }
  } finally {
    await chmod(metadata, 0o644);
  }
  try {
    for (const mode of [0o775, 0o777, 0o1755]) {
      await chmod(payload, mode);
      await assert.rejects(verifyDevelopmentGit(payload), /permission|mode/i);
    }
  } finally {
    await chmod(payload, 0o755);
  }
});

test('Git verification rejects empty directories and oversized members without executing anything', async t => {
  const root = await disposable(t);
  const payload = await fakePayload(root);
  const empty = path.join(payload, 'empty');
  await mkdir(empty, { mode: 0o755 });
  await assert.rejects(verifyDevelopmentGit(payload), /empty/i);
  await rm(empty, { recursive: true });
  const oversized = path.join(payload, 'oversized');
  await writeFile(oversized, '', { mode: 0o644 });
  // A sparse file tests the size guard without allocating its apparent size.
  await truncate(oversized, 512 * 1024 * 1024 + 1);
  await assert.rejects(verifyDevelopmentGit(payload), /bounded|size|budget|limit|large|bound/i);
});

test('Git preparation preserves existing output files, directories and dangling links', { skip: !macArm }, async t => {
  const root = await disposable(t);
  const existing = path.join(root, 'existing');
  await mkdir(existing);
  await writeFile(path.join(existing, 'caller'), 'keep');
  const file = path.join(root, 'caller-file');
  await writeFile(file, 'original');
  const dangling = path.join(root, 'dangling');
  await symlink(path.join(root, 'missing'), dangling);
  for (const outputDir of [existing, file, dangling]) {
    await assert.rejects(prepareDevelopmentGit({ outputDir }), /exist/i);
  }
  assert.equal(await readFile(path.join(existing, 'caller'), 'utf8'), 'keep');
  assert.equal(await readFile(file, 'utf8'), 'original');
  assert.equal((await lstat(dangling)).isSymbolicLink(), true);
  assert.deepEqual((await readdir(root)).sort(), ['caller-file', 'dangling', 'existing']);
});

test('Git preparation rejects traversal, linked parents and overlapping cache/output locations', { skip: !macArm }, async t => {
  const root = await disposable(t);
  const existing = path.join(root, 'existing');
  await mkdir(existing);
  await symlink(existing, path.join(root, 'alias'));
  for (const options of [
    { outputDir: 'relative' },
    { outputDir: root + '/../outside' },
    { outputDir: root + '/control\npath' },
    { outputDir: path.join(root, 'alias/new') },
    { outputDir: path.join(root, 'new'), cacheDir: path.join(root, 'new') },
    { outputDir: path.join(root, 'new'), cacheDir: path.join(root, 'new/cache') },
    { outputDir: path.join(existing, 'new'), cacheDir: existing },
  ]) await assert.rejects(prepareDevelopmentGit(options));
  assert.deepEqual(await readdir(existing), []);
  assert.deepEqual((await readdir(root)).sort(), ['alias', 'existing']);
});

test('Git preparation preserves corrupt or linked caches and removes only owned staging', { skip: !macArm }, async t => {
  const root = await disposable(t);
  const cacheDir = path.join(root, 'cache');
  await mkdir(cacheDir);
  const cacheFile = path.join(cacheDir, DEVELOPMENT_GIT_RELEASE.source.archiveName);
  const originalBytes = 'caller corrupt cache';
  await writeFile(cacheFile, originalBytes, { mode: 0o644 });
  const outputDir = path.join(root, 'new');
  await assert.rejects(prepareDevelopmentGit({ outputDir, cacheDir }));
  assert.equal(await readFile(cacheFile, 'utf8'), originalBytes);
  await assertMissing(outputDir);
  const original = path.join(cacheDir, 'original');
  await rename(cacheFile, original);
  await symlink(original, cacheFile);
  await assert.rejects(prepareDevelopmentGit({ outputDir, cacheDir }));
  assert.equal((await lstat(cacheFile)).isSymbolicLink(), true);
  assert.equal(await readFile(original, 'utf8'), originalBytes);
  await assertMissing(outputDir);
  assert.deepEqual(await readdir(root), ['cache']);
});

// Opt in with an existing verified source cache. All output and repositories
// belong to this test's temporary directory; no user config or credentials run.
test('real private Git relocates and performs local repository operations with isolated configuration', {
  skip: !macArm || !process.env.DEVELOPMENT_GIT_TEST_CACHE,
  timeout: 15 * 60_000,
}, async t => {
  const root = await disposable(t);
  const original = path.join(root, '临时 Git工具');
  const began = performance.now();
  await prepareDevelopmentGit({ outputDir: original, cacheDir: process.env.DEVELOPMENT_GIT_TEST_CACHE });
  const relocated = path.join(root, '移动后 Git工具');
  await rename(original, relocated);
  const verificationBegan = performance.now();
  const report = await verifyDevelopmentGit(relocated);
  t.diagnostic('Preparation ' + (verificationBegan - began).toFixed(0) + ' ms; relocated verification ' + (performance.now() - verificationBegan).toFixed(0) + ' ms');
  assert.equal(report.root, relocated);
  assert.equal(report.gitPath, path.join(relocated, 'bin/git'));
  assert.deepEqual(report.tree, {
    fileCount: DEVELOPMENT_GIT_RELEASE.tree.fileCount,
    totalBytes: DEVELOPMENT_GIT_RELEASE.tree.totalBytes,
    sha256: DEVELOPMENT_GIT_RELEASE.tree.sha256,
  });
  const home = path.join(root, 'isolated-home');
  const hooks = path.join(root, 'empty-hooks');
  await mkdir(home);
  await mkdir(hooks);
  const env = {
    PATH: path.dirname(report.gitPath) + ':/usr/bin:/bin:/usr/sbin:/sbin',
    HOME: home, XDG_CONFIG_HOME: path.join(home, 'config'), LC_ALL: 'C',
    GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_SYSTEM: '/dev/null', GIT_CONFIG_GLOBAL: '/dev/null',
    GIT_TERMINAL_PROMPT: '0', GIT_ASKPASS: '/usr/bin/false',
  };
  const options = { cwd: root, env, timeout: 30_000, maxBuffer: 64 * 1024, encoding: 'utf8' };
  const git = (args, cwd = root) => execute(report.gitPath, ['-c', 'core.hooksPath=' + hooks, ...args], { ...options, cwd });
  assert.equal((await git(['--version'])).stdout.trim(), 'git version ' + report.gitVersion);
  assert.equal((await git(['--exec-path'])).stdout.trim(), report.execPath);
  assert.equal(await realpath(report.execPath), report.execPath);
  assert.ok(report.execPath.startsWith(relocated + path.sep));
  const sample = path.join(root, '可丢弃 源仓库');
  await git(['init', '--quiet', '--initial-branch=main', sample]);
  await writeFile(path.join(sample, 'tracked.txt'), 'disposable relocation check\n');
  await git(['add', '--', 'tracked.txt'], sample);
  await git(['-c', 'user.name=Disposable Git Test', '-c', 'user.email=disposable@example.invalid',
    '-c', 'commit.gpgsign=false', 'commit', '--quiet', '-m', 'Disposable verification'], sample);
  assert.equal((await git(['status', '--porcelain=v1', '-z'], sample)).stdout, '');
  assert.equal((await git(['branch', '--show-current'], sample)).stdout.trim(), 'main');
  await git(['switch', '--quiet', '-c', 'disposable-branch'], sample);
  assert.equal((await git(['branch', '--show-current'], sample)).stdout.trim(), 'disposable-branch');
  await git(['switch', '--quiet', 'main'], sample);
  const sourceHead = (await git(['rev-parse', 'HEAD'], sample)).stdout.trim();
  const clone = path.join(root, '可丢弃 克隆仓库');
  await git(['clone', '--quiet', '--no-local', sample, clone]);
  assert.equal((await git(['rev-parse', 'HEAD'], clone)).stdout.trim(), sourceHead);
  assert.equal((await git(['status', '--porcelain=v1', '-z'], clone)).stdout, '');
  assert.equal(await readFile(path.join(clone, 'tracked.txt'), 'utf8'), 'disposable relocation check\n');
  await verifyDevelopmentGit(relocated);

  const permissions = [
    [report.gitPath, 0o555, 0o755],
    [path.join(relocated, metadataName), 0o444, 0o644],
    [path.dirname(report.gitPath), 0o555, 0o755],
    [relocated, 0o555, 0o755],
  ];
  try {
    for (const [file, mode] of permissions) await chmod(file, mode);
    await verifyDevelopmentGit(relocated);
  } finally {
    for (const [file, , mode] of [...permissions].reverse()) await chmod(file, mode);
  }
  const binary = await readFile(report.gitPath);
  await writeFile(report.gitPath, Buffer.concat([binary, Buffer.from('unexpected mutation')]));
  await assert.rejects(verifyDevelopmentGit(relocated));
  await writeFile(report.gitPath, binary);
  await chmod(report.gitPath, 0o755);
  const missing = path.join(root, 'temporarily-removed-git');
  await rename(report.gitPath, missing);
  await assert.rejects(verifyDevelopmentGit(relocated));
  await rename(missing, report.gitPath);
  const unknown = path.join(relocated, 'unexpected.txt');
  await writeFile(unknown, 'unexpected', { mode: 0o644 });
  await assert.rejects(verifyDevelopmentGit(relocated));
  await unlink(unknown);
  const empty = path.join(relocated, 'unexpected-empty');
  await mkdir(empty, { mode: 0o755 });
  await assert.rejects(verifyDevelopmentGit(relocated));
  await rm(empty, { recursive: true });
  await verifyDevelopmentGit(relocated);
});
