// Synthetic DOM checks. These do not launch WebView2 or inspect a real workspace.
import assert from 'node:assert/strict';
import { after, afterEach, test } from 'node:test';
import { mkdtemp, mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';
import { JSDOM } from 'jsdom';

const repo = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)));
const evidenceParent = path.join(repo, '.test-artifacts');
await mkdir(evidenceParent, { recursive: true });
const output = await mkdtemp(path.join(evidenceParent, 'windows-host-ui-'));
const mock = path.join(output, 'tauri-mock.mjs');
await writeFile(mock, 'export const invoke = (...args) => globalThis.__HOST_INVOKE__(...args); export const isTauri = () => globalThis.__HOST_IS_TAURI__;\n');
const componentPath = path.join(repo, 'apps/desktop/src/WindowsHost.tsx');
let compiled = ts.transpileModule(await readFile(componentPath, 'utf8'), {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX },
  fileName: componentPath,
}).outputText;
compiled = compiled.replaceAll(/(['"])@tauri-apps\/api\/core\1/g, JSON.stringify(pathToFileURL(mock).href));
const modulePath = path.join(output, 'WindowsHost.mjs');
await writeFile(modulePath, compiled);

const dom = new JSDOM('<!doctype html><html><body></body></html>', { url: 'http://tauri.localhost/windows.html', pretendToBeVisual: true });
for (const name of ['window', 'document', 'Node', 'HTMLElement', 'Event', 'MouseEvent', 'MutationObserver']) {
  Object.defineProperty(globalThis, name, { value: name === 'window' ? dom.window : dom.window[name], configurable: true });
}
Object.defineProperty(globalThis, 'navigator', { value: dom.window.navigator, configurable: true });
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
globalThis.__HOST_IS_TAURI__ = true;
const { createElement, act } = await import('react');
const { createRoot } = await import('react-dom/client');
const { WindowsHost } = await import(pathToFileURL(modulePath).href);
let root;
let commands;
let responder;

function report(status = 'blocked') {
  return {
    schemaVersion: 1,
    identifier: 'dev.localprojects.platform.m0',
    environment: 'development',
    platform: 'windows',
    architecture: 'x86_64',
    stage: 'readOnly',
    runtimeAvailable: false,
    probe: {
      dataRoot: 'C:\\synthetic-host-fixture\\dev.localprojects.platform.m0',
      status,
      checks: [{ code: 'SYNTHETIC_CHECK', status: status === 'blocked' ? 'blocked' : 'passed', message: '合成测试目录检查结果' }],
      evidence: [{ kind: 'migrationEntries', count: 3 }],
    },
  };
}

async function setup(value = report()) {
  commands = [];
  responder = async () => value;
  globalThis.__HOST_IS_TAURI__ = true;
  globalThis.__HOST_INVOKE__ = async (command) => {
    commands.push(command);
    assert.equal(command, 'host_status', 'Windows UI must never invoke deferred business commands');
    return responder();
  };
  document.body.innerHTML = '<div id="root"></div>';
  root = createRoot(document.getElementById('root'));
  await act(async () => { root.render(createElement(WindowsHost)); });
}

async function refresh() {
  const button = document.querySelector('button');
  assert.ok(button);
  assert.equal(button.textContent, '刷新检查');
  await act(async () => { button.click(); });
}

afterEach(async () => {
  if (root) await act(async () => { root.unmount(); });
  root = null;
});
after(async () => {
  await writeFile(path.join(output, 'scope.txt'), 'Synthetic React DOM tests only. No real native host, WebView2 or workspace inspection was executed.\n');
  dom.window.close();
});

test('mount reads only host_status and shows actual blocked checks and retained record counts', async () => {
  await setup();
  assert.deepEqual(commands, ['host_status']);
  assert.match(document.body.textContent, /检查受阻/);
  assert.match(document.body.textContent, /合成测试目录检查结果/);
  assert.equal(document.querySelector('.host-path').textContent, report().probe.dataRoot);
  assert.match(document.querySelector('.host-evidence').textContent, /迁移目录内条目3/);
  assert.match(document.body.textContent, /项目运行时未启动/);
  assert.equal(document.querySelectorAll('button').length, 1);
});

test('refresh replaces successful probe data without opening business operations', async () => {
  await setup();
  const next = report('missing');
  next.probe.dataRoot = 'C:\\synthetic-new-root';
  next.probe.evidence = [];
  responder = async () => next;
  await refresh();
  assert.deepEqual(commands, ['host_status', 'host_status']);
  assert.equal(document.querySelector('.host-path').textContent, next.probe.dataRoot);
  assert.match(document.body.textContent, /未发现数据根/);
  assert.match(document.body.textContent, /本次未列出现场记录/);
  assert.match(document.body.textContent, /项目运行时未启动/);
  assert.equal(document.querySelectorAll('.host-capability:not(.available)').length, 2);
});

test('a failed refresh preserves the prior probe and marks it as an older result', async () => {
  await setup();
  responder = async () => { throw { code: 'HOST_CHECK_FAILED', message: '合成检查中断' }; };
  await refresh();
  assert.match(document.querySelector('[role="alert"]').textContent, /合成检查中断.*上次成功检查结果/);
  assert.equal(document.querySelector('.host-path').textContent, report().probe.dataRoot);
  assert.match(document.querySelector('.host-evidence').textContent, /迁移目录内条目3/);
  assert.deepEqual(commands, ['host_status', 'host_status']);
});

test('a browser preview cannot invent a local probe or invoke native commands', async () => {
  await setup();
  await act(async () => { root.unmount(); });
  root = null;
  commands = [];
  globalThis.__HOST_IS_TAURI__ = false;
  root = createRoot(document.getElementById('root'));
  await act(async () => { root.render(createElement(WindowsHost)); });
  assert.deepEqual(commands, []);
  assert.match(document.querySelector('[role="alert"]').textContent, /从 Windows 桌面应用打开/);
  assert.match(document.querySelector('.host-path').textContent, /尚未取得检查结果/);
});
