import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFileSync } from 'node:child_process';
import { lstat, mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { inspectWindowsDevelopmentNode, prepareWindowsDevelopmentNode, WINDOWS_DEVELOPMENT_NODE_RELEASE as release } from './package-windows-development-node.mjs';
import { extractDevelopmentZip, selectNodePayload, validateRelease } from './windows-development-tools.mjs';
import { fixture, ownedRoot } from './windows-development-tools-test-support.mjs';

test('Development Node pins a separate security-patch ZIP, full npm dependency tree and complete licenses', () => {
  validateRelease(release); assert.equal(release.version, '24.22.1'); assert.equal(release.npmVersion, '11.19.1');
  assert.equal(release.source.sha256, 'd2046d28c2c3dcd211ac5145c0bf30e2047ef9dfb306693c8c6e2eccd8b95fa5');
  assert.equal(release.files.length, 1934); assert.equal(release.tree.totalBytes, 106618216); assert.equal(release.paths.executable, 'node.exe');
  assert.equal(release.files.find(file => file.path === 'node.exe').sha256, 'd702daa815eabbff763c7d3808e235cf031cf2ca70211b56847bc72f51e240e5');
  for (const name of ['LICENSE', 'node_modules/npm/LICENSE', 'node_modules/npm/package.json', 'node_modules/npm/bin/npm-cli.js', 'node_modules/npm/bin/npx-cli.js', 'npm.cmd', 'npx.cmd']) assert.ok(release.files.some(file => file.path === name));
  assert.ok(release.files.filter(file => file.path.startsWith('node_modules/npm/node_modules/')).length > 1000);
  for (const file of release.files) assert.equal(file.path.includes('corepack') || file.path === 'install_tools.bat' || file.path === 'nodevars.bat', false);
  assert.throws(() => { release.source.sha256 = '0'.repeat(64); }, TypeError);
});
test('Only the pinned Node executable, launchers, notices and full npm subtree leave the ZIP', () => {
  const value = fixture('node'); const files = extractDevelopmentZip(value.archive, { archiveRoot: value.release.archiveRoot, select: selectNodePayload });
  assert.deepEqual([...files.keys()].sort(), [...value.files.keys()].sort()); assert.equal(files.has('install_tools.bat'), false);
  assert.deepEqual(files.get('node_modules/npm/node_modules/nested/LICENSE'), value.files.get('node_modules/npm/node_modules/nested/LICENSE'));
});
test('Official Windows development Node/npm works offline from a Chinese-space private path without lifecycle scripts', { skip: process.platform !== 'win32' || !process.env.WINDOWS_DEVELOPMENT_NODE_TEST_ARCHIVE, timeout: 120_000 }, async () => {
  const parent = await ownedRoot('official-node'); const output = path.join(parent, '开发 Node 工具'); const project = path.join(parent, '可丢弃 项目'); await mkdir(project);
  const checked = await prepareWindowsDevelopmentNode({ outputDir: output, archive: path.resolve(process.env.WINDOWS_DEVELOPMENT_NODE_TEST_ARCHIVE) });
  const packageJson = { name: 'disposable-development-node-fixture', version: '1.0.0', scripts: { preinstall: 'node -e "require(\'fs\').writeFileSync(\'must-not-run\',\'bad\')"' } };
  await writeFile(path.join(project, 'package.json'), JSON.stringify(packageJson));
  await writeFile(path.join(project, 'package-lock.json'), JSON.stringify({ name: packageJson.name, version: packageJson.version, lockfileVersion: 3, requires: true, packages: { '': { name: packageJson.name, version: packageJson.version, hasInstallScript: true } } }));
  const userConfig = path.join(parent, 'user.npmrc'); const globalConfig = path.join(parent, 'global.npmrc'); await writeFile(userConfig, ''); await writeFile(globalConfig, '');
  const env = { SystemRoot: process.env.SystemRoot, PATH: `${output};${path.join(process.env.SystemRoot, 'System32')}`,
    npm_config_userconfig: userConfig, npm_config_globalconfig: globalConfig, npm_config_cache: path.join(parent, 'cache'), npm_config_update_notifier: 'false' };
  const options = { cwd: project, env, encoding: 'utf8', timeout: 30_000, windowsHide: true, shell: false };
  assert.equal(execFileSync(checked.executable, ['--version'], options).trim(), 'v24.22.1');
  assert.equal(execFileSync(checked.executable, [checked.npmCli, '--version'], options).trim(), '11.19.1');
  execFileSync(checked.executable, [checked.npmCli, 'ci', '--offline', '--ignore-scripts', '--audit=false', '--fund=false'], options);
  await assert.rejects(lstat(path.join(project, 'must-not-run')), { code: 'ENOENT' });
  assert.equal(JSON.parse(await readFile(path.join(project, 'package-lock.json'), 'utf8')).lockfileVersion, 3);
  await inspectWindowsDevelopmentNode(output);
});
