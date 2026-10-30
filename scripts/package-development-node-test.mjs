import assert from 'node:assert/strict';
import test from 'node:test';
import { chmod, link, lstat, mkdir, mkdtemp, readFile, readdir, realpath, rename, rm, symlink, unlink, writeFile } from 'node:fs/promises';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { createHash } from 'node:crypto';
import os from 'node:os';
import path from 'node:path';
import { NODE_RELEASE } from './package-node-runtime.mjs';
import { DEVELOPMENT_NODE_RELEASE, developmentLauncher, developmentTreeDigest, extractDevelopmentNpm, parseDevelopmentArguments, prepareDevelopmentNode, verifyDevelopmentNode as verifyMacDevelopmentNode } from './package-development-node.mjs';
import { verifyDevelopmentNodeWindows } from './package-development-node-windows.mjs';
import { directoryLink, fileSymlinkFixture, posixPermissions } from './package-test-fixtures.mjs';

const execute = promisify(execFile);
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const macArm = process.platform === 'darwin' && process.arch === 'arm64';
const npmRoot = `${NODE_RELEASE.archiveRoot}/lib/node_modules/npm`;
const windowsDependencies = process.env.WINDOWS_NATIVE_PACKAGER_HELPER ? { helper: process.env.WINDOWS_NATIVE_PACKAGER_HELPER } : {};
const verifyDevelopmentNode = process.platform === 'win32'
  ? root => verifyDevelopmentNodeWindows(root, windowsDependencies) : verifyMacDevelopmentNode;
const verificationMetadata = await readFile(new URL(process.platform === 'win32'
  ? './development-node-release.windows-x64.json' : './development-node-release.json', import.meta.url));

async function disposable(t) {
  const root = await mkdtemp(path.join(await realpath(os.tmpdir()), 'disposable-development-node-'));
  await chmod(root, 0o755);
  t.after(() => rm(root, { recursive: true, force: true }));
  return root;
}

function tarEntry(name, bytes = Buffer.alloc(0), type = '0', mode = 0o644) {
  const header = Buffer.alloc(512);
  const split = name.length > 100 ? name.lastIndexOf('/', 154) : -1;
  if (split >= 0) { header.write(name.slice(0, split), 345, 155); name = name.slice(split + 1); }
  assert.ok(Buffer.byteLength(name) <= 100);
  header.write(name, 0, 100, 'utf8');
  header.write(`${mode.toString(8).padStart(7, '0')}\0`, 100, 8);
  header.write('0000000\0', 108, 8); header.write('0000000\0', 116, 8);
  header.write(`${bytes.length.toString(8).padStart(11, '0')}\0`, 124, 12);
  header.write('00000000000\0', 136, 12); header.fill(32, 148, 156);
  header.write(type, 156, 1); header.write('ustar\0', 257, 6); header.write('00', 263, 2);
  const sum = header.reduce((total, byte) => total + byte, 0);
  header.write(`${sum.toString(8).padStart(6, '0')}\0 `, 148, 8);
  return Buffer.concat([header, bytes, Buffer.alloc((512 - bytes.length % 512) % 512)]);
}

function npmTar(extra = [], version = '11.19.1') {
  return Buffer.concat([
    tarEntry(`${NODE_RELEASE.archiveRoot}/`, Buffer.alloc(0), '5'),
    tarEntry(`${npmRoot}/`, Buffer.alloc(0), '5'),
    tarEntry(`${npmRoot}/package.json`, Buffer.from(JSON.stringify({ version }))),
    tarEntry(`${npmRoot}/LICENSE`, Buffer.from('Artificial npm license')),
    tarEntry(`${npmRoot}/bin/npm-cli.js`, Buffer.from('never executed'), '0', 0o755),
    tarEntry(`${npmRoot}/bin/npx-cli.js`, Buffer.from('never executed'), '0', 0o755),
    // The official top-level links are not materialized by this parser.
    tarEntry(`${NODE_RELEASE.archiveRoot}/bin/npm`, Buffer.alloc(0), '2'),
    ...extra, Buffer.alloc(1024),
  ]);
}

test('development CLI accepts only explicit output/cache locations', () => {
  assert.deepEqual(parseDevelopmentArguments(['--output', '/disposable/new', '--cache', '/disposable/cache']), { outputDir: '/disposable/new', cacheDir: '/disposable/cache' });
  for (const args of [[], ['--output'], ['--output', '/a', '--output', '/b'], ['--url', 'https://example.invalid'], ['--metadata', '/override'], ['--version', '26.0.0'], ['--verify', '/a']]) {
    assert.throws(() => parseDevelopmentArguments(args));
  }
});

test('development trust anchor binds the existing official archive and complete payload', () => {
  assert.equal(DEVELOPMENT_NODE_RELEASE.archiveSha256, NODE_RELEASE.source.sha256);
  assert.equal(DEVELOPMENT_NODE_RELEASE.nodeVersion, '24.22.0');
  assert.equal(DEVELOPMENT_NODE_RELEASE.npmVersion, '11.19.1');
  assert.equal(DEVELOPMENT_NODE_RELEASE.tree.fileCount, 1930);
  assert.equal(DEVELOPMENT_NODE_RELEASE.tree.totalBytes, 134867341);
  assert.equal(DEVELOPMENT_NODE_RELEASE.tree.sha256, 'a9f345d82e2d7b1e8355c355e28d5471c8a45f1263343463700d7050cab56b2d');
});

test('npm extraction keeps dependency licenses and executability without top-level links', () => {
  const files = extractDevelopmentNpm(npmTar([
    tarEntry(`${npmRoot}/node_modules/dependency/LICENSE`, Buffer.from('Dependency notice')),
    tarEntry(`${npmRoot}/node_modules/dependency/index.js`, Buffer.from('dependency')),
  ]));
  assert.equal(files.size, 6);
  assert.equal(files.get('lib/node_modules/npm/node_modules/dependency/LICENSE').bytes.toString(), 'Dependency notice');
  assert.equal(files.get('lib/node_modules/npm/bin/npm-cli.js').mode, 0o755);
  assert.equal(files.get('lib/node_modules/npm/LICENSE').mode, 0o644);
  assert.equal(files.has('bin/npm'), false);
});

test('npm extraction rejects traversal, duplicate files, links, wrong versions and malformed tar', () => {
  for (const extra of [
    tarEntry(`${npmRoot}/../escape`, Buffer.from('bad')),
    tarEntry(`${npmRoot}/LICENSE`, Buffer.from('duplicate')),
    tarEntry(`${npmRoot}/linked`, Buffer.alloc(0), '2'),
    tarEntry(`${npmRoot}/hardlink`, Buffer.alloc(0), '1'),
    tarEntry(`${npmRoot}/pipe`, Buffer.alloc(0), '6'),
    tarEntry(npmRoot, Buffer.alloc(0), '2'),
  ]) assert.throws(() => extractDevelopmentNpm(npmTar([extra])), /Unsafe|Duplicate|links|ordinary/);
  assert.throws(() => extractDevelopmentNpm(npmTar([], '99.0.0')), /unexpected npm/);
  const valid = npmTar();
  assert.throws(() => extractDevelopmentNpm(valid.subarray(0, valid.length - 512)), /end marker/);
  const corrupt = Buffer.from(valid); corrupt[0] ^= 1;
  assert.throws(() => extractDevelopmentNpm(corrupt), /checksum/);
});

test('tree digest is ordered by UTF-8 path and binds both names and bytes', () => {
  const values = [['z', Buffer.from('same')], ['a', Buffer.from('other')]];
  assert.deepEqual(developmentTreeDigest(values), developmentTreeDigest([...values].reverse()));
  const expected = digest(Buffer.from(`a\0${values[1][1].length}\0${digest(values[1][1])}\nz\0${values[0][1].length}\0${digest(values[0][1])}\n`));
  assert.equal(developmentTreeDigest(values).sha256, expected);
  assert.notEqual(developmentTreeDigest(values).sha256, developmentTreeDigest([['renamed', values[0][1]], values[1]]).sha256);
  assert.throws(() => developmentTreeDigest([['../outside', Buffer.from('')]]), /Unsafe/);
});

test('verification never executes unverified bytes or trusts self-reported metadata', async t => {
  const root = await disposable(t);
  await mkdir(path.join(root, 'bin'));
  const sentinel = path.join(root, 'unexpected-execution');
  await writeFile(path.join(root, 'bin/node'), `#!/bin/sh\ntouch '${sentinel}'\n`, { mode: 0o755 });
  await writeFile(path.join(root, 'development-node.json'), JSON.stringify({ ...JSON.parse(verificationMetadata), tree: { sha256: 'forged' } }), { mode: 0o644 });
  await assert.rejects(verifyDevelopmentNode(root), /metadata differs/);
  await assert.rejects(lstat(sentinel), { code: 'ENOENT' });
  await writeFile(path.join(root, 'development-node.json'), verificationMetadata);
  await assert.rejects(verifyDevelopmentNode(root), /tree differs/);
  await assert.rejects(lstat(sentinel), { code: 'ENOENT' });
});

test('verification refuses linked parents, linked files, hardlinks and empty unknown directories', async t => {
  const root = await disposable(t);
  const actual = path.join(root, 'actual'); await mkdir(actual);
  await directoryLink(actual, path.join(root, 'alias'));
  await assert.rejects(verifyDevelopmentNode(path.join(root, 'alias')), /ordinary directory|[Rr]eparse/);
  const file = path.join(actual, 'member'); await writeFile(file, 'keep');
  await t.test('file symlink member', async t => {
    const linked = path.join(actual, 'linked');
    if (!await fileSymlinkFixture(t, file, linked)) return;
    try { await assert.rejects(verifyDevelopmentNode(actual), /symbolic link|[Rr]eparse/); }
    finally { await unlink(linked); }
  });
  await link(file, path.join(actual, 'hardlink'));
  await assert.rejects(verifyDevelopmentNode(actual), /without links|ordinary-file read policy/);
  await unlink(path.join(actual, 'hardlink'));
  await mkdir(path.join(actual, 'empty'));
  await assert.rejects(verifyDevelopmentNode(actual), /empty/i);
  assert.equal(await readFile(file, 'utf8'), 'keep');
});

test('verification rejects group-writable, world-writable and special-bit permissions', posixPermissions, async t => {
  const root = await disposable(t);
  const file = path.join(root, 'development-node.json');
  await writeFile(file, await readFile(new URL('./development-node-release.json', import.meta.url)), { mode: 0o644 });
  for (const mode of [0o664, 0o666, 0o4644]) {
    await chmod(file, mode);
    await assert.rejects(verifyDevelopmentNode(root), /permissions differ/);
  }
  await chmod(file, 0o644);
  for (const mode of [0o775, 0o777, 0o1755]) {
    await chmod(root, mode);
    await assert.rejects(verifyDevelopmentNode(root), /directory permissions/);
  }
  await chmod(root, 0o755);
});

test('preparation preserves existing output paths and refuses ambiguous locations', { skip: !macArm }, async t => {
  const root = await disposable(t);
  const existing = path.join(root, 'existing'); await mkdir(existing);
  await writeFile(path.join(existing, 'caller'), 'keep');
  const dangling = path.join(root, 'dangling'); await symlink(path.join(root, 'missing'), dangling);
  for (const outputDir of [existing, dangling]) await assert.rejects(prepareDevelopmentNode({ outputDir }), /already exists/);
  for (const options of [
    { outputDir: 'relative' }, { outputDir: `${root}/../outside` }, { outputDir: `${root}/control\npath` },
    { outputDir: path.join(root, 'new'), cacheDir: path.join(root, 'new/cache') },
  ]) await assert.rejects(prepareDevelopmentNode(options), /absolute path|overlap/);
  await symlink(existing, path.join(root, 'alias'));
  await assert.rejects(prepareDevelopmentNode({ outputDir: path.join(root, 'alias/new') }), /ordinary directory/);
  assert.equal(await readFile(path.join(existing, 'caller'), 'utf8'), 'keep');
  assert.equal((await lstat(dangling)).isSymbolicLink(), true);
});

test('corrupt or linked cache is preserved, output is absent and owned staging is removed', { skip: !macArm }, async t => {
  const root = await disposable(t);
  const cacheDir = path.join(root, 'cache'); await mkdir(cacheDir);
  const cacheFile = path.join(cacheDir, NODE_RELEASE.archiveName);
  await writeFile(cacheFile, 'caller corrupt cache');
  const outputDir = path.join(root, 'new');
  await assert.rejects(prepareDevelopmentNode({ outputDir, cacheDir }), /SHA-256/);
  assert.equal(await readFile(cacheFile, 'utf8'), 'caller corrupt cache');
  await rename(cacheFile, path.join(cacheDir, 'original'));
  await symlink(path.join(cacheDir, 'original'), cacheFile);
  await assert.rejects(prepareDevelopmentNode({ outputDir, cacheDir }), /ordinary file/);
  assert.equal((await lstat(cacheFile)).isSymbolicLink(), true);
  assert.deepEqual(await readdir(root), ['cache']);
});

// Opt in using an existing checksum-verified cache. This never installs tools or
// changes the user's environment, and all created projects are disposable.
test('real official development Node runs offline npm/npx child scripts after relocation', {
  skip: !macArm || !process.env.DEVELOPMENT_NODE_TEST_CACHE,
}, async t => {
  const root = await disposable(t);
  const original = path.join(root, '临时 开发工具');
  const began = performance.now();
  await prepareDevelopmentNode({ outputDir: original, cacheDir: process.env.DEVELOPMENT_NODE_TEST_CACHE });
  const relocated = path.join(root, '移动后 开发工具'); await rename(original, relocated);
  const verifiedAt = performance.now(); const report = await verifyDevelopmentNode(relocated);
  t.diagnostic(`Preparation ${(verifiedAt - began).toFixed(0)} ms; relocated verification ${(performance.now() - verifiedAt).toFixed(0)} ms`);
  const sample = path.join(root, '可丢弃 脚本项目'); await mkdir(sample);
  const home = path.join(root, 'isolated-home'); await mkdir(home);
  const env = { PATH: `${report.binPath}:/usr/bin:/bin`, HOME: home, LC_ALL: 'C',
    npm_config_cache: path.join(root, 'npm-cache'), npm_config_offline: 'true', npm_config_update_notifier: 'false',
    EXPECTED_NODE: report.nodePath, EXPECTED_NPM: report.npmPath };
  const inspect = `const assert=require('node:assert/strict');assert.equal(process.execPath,process.env.EXPECTED_NODE);assert.equal(process.env.npm_node_execpath,process.env.EXPECTED_NODE);assert.equal(process.env.npm_execpath,process.env.EXPECTED_NPM);console.log('same private node and npm');`;
  await writeFile(path.join(sample, 'inspect.cjs'), inspect);
  const pkg = { name: 'disposable-development-node-check', version: '1.0.0', scripts: {
    verify: 'node inspect.cjs && npm run child && npx --offline --no -- node inspect.cjs', child: 'node inspect.cjs',
  } };
  await writeFile(path.join(sample, 'package.json'), JSON.stringify(pkg));
  await writeFile(path.join(sample, 'package-lock.json'), JSON.stringify({ name: pkg.name, version: pkg.version, lockfileVersion: 3, requires: true, packages: { '': { name: pkg.name, version: pkg.version } } }));
  const options = { cwd: sample, env, timeout: 30_000, maxBuffer: 64 * 1024, encoding: 'utf8' };
  await execute(report.nodePath, [report.npmPath, 'ci', '--offline', '--no-audit', '--no-fund', '--ignore-scripts'], options);
  const result = await execute(path.join(report.binPath, 'npm'), ['run', 'verify'], options);
  assert.equal(result.stdout.match(/same private node and npm/g)?.length, 3);
  for (const name of ['npm', 'npx']) assert.equal((await execute(path.join(report.binPath, name), ['--version'], { ...options, env: { PATH: '/usr/bin:/bin', HOME: home } })).stdout.trim(), '11.19.1');
  await verifyDevelopmentNode(relocated);
  const permissionChanges = [
    [report.nodePath, 0o555, 0o755], [report.npmPath, 0o555, 0o755],
    [path.join(relocated, 'LICENSE'), 0o444, 0o644],
    [path.join(relocated, 'development-node.json'), 0o444, 0o644], [relocated, 0o555, 0o755],
  ];
  try {
    for (const [file, mode] of permissionChanges) await chmod(file, mode);
    await verifyDevelopmentNode(relocated);
  } finally { for (const [file, , mode] of permissionChanges.reverse()) await chmod(file, mode); }
  const cli = report.npmPath; const originalCli = await readFile(cli);
  await writeFile(cli, Buffer.concat([originalCli, Buffer.from('\n// unexpected edit\n')]));
  await assert.rejects(verifyDevelopmentNode(relocated), /tree differs/);
  await writeFile(cli, originalCli); await chmod(cli, 0o755);
  const unexpected = path.join(relocated, 'untrusted.js'); await writeFile(unexpected, 'never executed', { mode: 0o644 });
  await assert.rejects(verifyDevelopmentNode(relocated), /tree differs/);
  await unlink(unexpected);
  await verifyDevelopmentNode(relocated);
});
