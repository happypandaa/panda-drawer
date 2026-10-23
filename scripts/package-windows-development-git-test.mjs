import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFileSync } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { inspectWindowsDevelopmentGit, prepareWindowsDevelopmentGit, WINDOWS_DEVELOPMENT_GIT_RELEASE as release } from './package-windows-development-git.mjs';
import { validateRelease } from './windows-development-tools.mjs';
import { ownedRoot } from './windows-development-tools-test-support.mjs';

test('MinGit uses the official standard x64 asset including helpers, trust store and all license files', () => {
  validateRelease(release); assert.equal(release.version, '2.56.0.windows.2');
  assert.equal(release.source.sha256, 'da35e72aa21c005a5a0d298cfbae110bc1609a815730ea0dde84b01a1b3cd3be');
  assert.equal(release.archiveRoot, null); assert.equal(release.files.length, 373); assert.equal(release.tree.totalBytes, 96142395);
  assert.equal(release.paths.executable, 'cmd/git.exe'); assert.equal(release.paths.binPath, 'ucrt64/bin');
  for (const name of ['cmd/git.exe', 'ucrt64/bin/git.exe', 'ucrt64/bin/git-remote-https.exe']) {
    const file = release.files.find(file => file.path === name); assert.equal(file.executable, true); assert.equal(file.peMachine, 0x8664);
  }
  for (const name of ['LICENSE.txt', 'ucrt64/doc/git-credential-manager/LICENSE', 'ucrt64/etc/ssl/certs/ca-bundle.crt', 'ucrt64/share/git-core/templates/info/exclude']) assert.ok(release.files.some(file => file.path === name));
  // The upstream native x64 distribution also carries PE32 AnyCPU .NET DLLs.
  // Pin their actual architecture; do not misclassify or omit complete helpers.
  assert.ok(release.files.some(file => file.path.endsWith('.dll') && file.peMachine === 0x14c));
  assert.throws(() => { release.files.pop(); }, TypeError);
});
test('Official private MinGit isolates configuration and saves source objects from a Chinese-space path', { skip: process.platform !== 'win32' || !process.env.WINDOWS_DEVELOPMENT_GIT_TEST_ARCHIVE, timeout: 120_000 }, async () => {
  const parent = await ownedRoot('official-git'); const output = path.join(parent, '开发 Git 工具'); const home = path.join(parent, 'isolated home'); await mkdir(home);
  await writeFile(path.join(home, '.gitconfig'), '[core]\n  hooksPath = ../hostile-hooks\n[credential]\n  helper = !must-never-run\n');
  const checked = await prepareWindowsDevelopmentGit({ outputDir: output, archive: path.resolve(process.env.WINDOWS_DEVELOPMENT_GIT_TEST_ARCHIVE) });
  const env = { SystemRoot: process.env.SystemRoot, PATH: [path.dirname(checked.executable), checked.binPath, path.join(output, 'usr/bin'), path.join(process.env.SystemRoot, 'System32')].join(';'),
    HOME: home, USERPROFILE: home, XDG_CONFIG_HOME: home, GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_SYSTEM: 'NUL', GIT_CONFIG_GLOBAL: 'NUL', GIT_CONFIG_COUNT: '0',
    GIT_CEILING_DIRECTORIES: parent, GIT_DISCOVERY_ACROSS_FILESYSTEM: '0', GIT_EXEC_PATH: checked.execPath, GIT_TEMPLATE_DIR: checked.templatePath, GIT_SSL_CAINFO: checked.certificateBundle,
    GIT_TERMINAL_PROMPT: '0', GIT_AUTHOR_NAME: 'Disposable Fixture', GIT_AUTHOR_EMAIL: 'fixture@example.invalid', GIT_COMMITTER_NAME: 'Disposable Fixture', GIT_COMMITTER_EMAIL: 'fixture@example.invalid' };
  const options = { cwd: home, env, encoding: 'utf8', timeout: 15_000, windowsHide: true, shell: false };
  const run = (args, extra = {}) => execFileSync(checked.executable, args, { ...options, ...extra });
  assert.equal(run(['--version']).trim(), 'git version 2.56.0.windows.2'); assert.equal(run(['config', '--list', '--show-origin']).trim(), '');
  const repository = path.join(parent, '可丢弃 仓库'); run(['-c', 'core.hooksPath=' + path.join(parent, 'no-hooks'), 'init', '-b', 'main', repository]);
  const repo = { cwd: repository }; const source = 'Synthetic source saved by private MinGit.\n';
  const oid = run(['hash-object', '-w', '--stdin', '--no-filters'], { ...repo, input: source }).trim();
  const tree = run(['mktree'], { ...repo, input: `100644 blob ${oid}\tfile.txt\n` }).trim();
  const commit = run(['-c', 'core.hooksPath=' + path.join(parent, 'no-hooks'), 'commit-tree', tree, '-m', 'disposable fixture'], repo).trim();
  run(['update-ref', 'refs/heads/main', commit], repo); assert.equal(run(['cat-file', '-p', 'HEAD:file.txt'], repo), source);
  await inspectWindowsDevelopmentGit(output);
});
