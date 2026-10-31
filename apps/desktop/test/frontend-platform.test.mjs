// Lexical/presentation regressions only; no filesystem permission or service.
import assert from 'node:assert/strict';
import { after, test } from 'node:test';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';

const repo = fileURLToPath(new URL('../../../', import.meta.url));
const artifacts = path.join(repo, '.test-artifacts');
await mkdir(artifacts, { recursive: true });
const output = await mkdtemp(path.join(artifacts, 'frontend-platform-unit-'));
const target = path.join(output, 'frontend-platform.mjs');
await writeFile(target, ts.transpileModule(await readFile(path.join(repo, 'apps/desktop/src/frontend-platform.tsx'), 'utf8'), {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX },
}).outputText);
const { frontendPlatformFromBootstrap } = await import(pathToFileURL(target).href);
after(async () => {
  assert.equal(path.dirname(output), artifacts);
  await rm(output, { recursive: true, force: true });
});
const mac = frontendPlatformFromBootstrap({ build: { target: 'aarch64-apple-darwin' } });
const windows = frontendPlatformFromBootstrap({ build: { target: 'x86_64-pc-windows-msvc' } });

test('Only Windows removes new workspace migration while neutral and Mac keep their existing presentation', () => {
  assert.equal(windows.migrationAvailable, false);
  assert.equal(mac.migrationAvailable, true);
  assert.equal(frontendPlatformFromBootstrap({ build: { target: 'x86_64-apple-darwin' } }).migrationAvailable, true);
  assert.equal(frontendPlatformFromBootstrap(null).migrationAvailable, true);
});

test('Unknown or malformed bootstrap metadata stays neutral without inferring a host from paths', () => {
  for (const value of [undefined, null, {}, { build: null }, { build: 4 }, { build: { target: 'unsupported' } }, { architecture: 'x86_64', executablePath: 'C:\\disposable\\panda.exe' }]) {
    const unknown = frontendPlatformFromBootstrap(value);
    assert.equal(unknown.openFolderLabel, '在文件管理器中打开');
    assert.doesNotMatch(unknown.exitExplanation, /Dock|Cmd\+Q|macOS/);
    assert.doesNotMatch(unknown.nodePathPlaceholder, /homebrew|Program Files/);
    assert.equal(unknown.isAbsolutePath('/disposable/node'), true);
    assert.equal(unknown.isAbsolutePath(String.raw`C:\disposable\node.exe`), true);
    assert.equal(unknown.isAbsolutePath('relative/node'), false);
  }
});

test('Mac preserves the original exact copy and slash-only lexical policy', () => {
  assert.equal(mac.openFolderLabel, '在访达中打开');
  assert.equal(mac.nodePathPlaceholder, '例如 /opt/homebrew/bin/node');
  assert.equal(mac.gitPathPlaceholder, '例如 /usr/bin/git');
  assert.equal(mac.workspacePathPlaceholder, '/Volumes/磁盘/新工作区');
  assert.equal(mac.workspaceLocationRequirements, '空文件夹、可写入、APFS 格式，在本机或直连磁盘上。');
  assert.equal(mac.exitExplanation, '当前是内部开发版，只使用专用数据目录。macOS 关闭主窗口后软件仍保留在 Dock，可点击 Dock 图标重新打开；从菜单退出或按 Cmd+Q 才会关闭应用窗口并停止受管服务。其他平台关闭主窗口仍会退出。');
  for (const value of ['/', '/opt/homebrew/bin/node', '/disposable/../node', '/a\\b']) assert.equal(mac.isAbsolutePath(value), true);
  for (const value of ['relative', String.raw`C:\tools\node.exe`, String.raw`\\server\share\node.exe`]) assert.equal(mac.isAbsolutePath(value), false);
});

test('Windows accepts native drive, mixed, UNC and extended paths without claiming file access', () => {
  for (const value of [
    String.raw`C:\Program Files\nodejs\node.exe`, 'D:/中文 工具/node.exe', String.raw`D:\tools/../node.exe`,
    String.raw`\\server\share\工具\git.exe`, '//server/share/git.exe', String.raw`\\?\C:\tools\node.exe`,
    String.raw`\\?\UNC\server\share\node.exe`, String.raw`\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}\node.exe`,
  ]) assert.equal(windows.isAbsolutePath(value), true, value);
  assert.equal(windows.openFolderLabel, '在资源管理器中打开');
  assert.doesNotMatch(windows.exitExplanation, /Dock|Cmd\+Q|macOS/);
  assert.match(windows.workspaceLocationRequirements, /NTFS.*本机内置/);
});

test('Windows rejects drive-relative, root-relative and incomplete UNC paths', () => {
  for (const value of ['', 'relative/node.exe', 'C:node.exe', '/node.exe', String.raw`\node.exe`,
    String.raw`\\server`, String.raw`\\?\C:node.exe`, String.raw`\\?\UNC\server`, 'https://example.invalid/node.exe', 'C:\\bad\0node.exe']) {
    assert.equal(windows.isAbsolutePath(value), false, JSON.stringify(value));
  }
});

test('Mac deletion path grouping preserves the original slash presentation', () => {
  assert.equal(mac.parentPath('/data/apps/one/'), '/data/apps');
  assert.equal(mac.pathBaseName('/data/apps/one/'), 'one');
  assert.equal(mac.commonDirectory(['/data/apps/one', '/data/cache/two']), '/data');
  assert.equal(mac.commonDirectory(['/one', '/two']), '/');
  assert.equal(mac.relativeDirectory('/data/cache', '/data'), 'cache/');
  assert.equal(mac.directoryLabel('/'), '/');
});

test('Windows deletion path grouping keeps drive and verbatim namespaces and labels different disks', () => {
  assert.equal(windows.parentPath(String.raw`C:\中文 项目\app`), String.raw`C:\中文 项目`);
  assert.equal(windows.pathBaseName(String.raw`C:\中文 项目\app`), 'app');
  assert.equal(windows.parentPath(String.raw`C:\app`), 'C:\\');
  assert.equal(windows.commonDirectory([String.raw`C:\data\a`, String.raw`C:\data\b`]), String.raw`C:\data`);
  assert.equal(windows.commonDirectory([String.raw`C:\data\a`, String.raw`D:\data\b`]), '');
  assert.equal(windows.directoryLabel(''), '多个磁盘位置');
  assert.equal(windows.directoryLabel('C:\\'), 'C:\\');
  assert.equal(windows.commonDirectory([String.raw`\\?\D:\data\one\a`, String.raw`\\?\D:\data\two\b`]), String.raw`\\?\D:\data`);
  assert.equal(windows.parentPath(String.raw`\\server\share\file`), '\\\\server\\share\\');
  assert.equal(windows.relativeDirectory(String.raw`C:\data\cache`, String.raw`C:\data`), 'cache\\');
});
