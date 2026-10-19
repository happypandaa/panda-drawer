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
for (const name of ['window', 'document', 'Node', 'HTMLElement', 'HTMLInputElement', 'HTMLTextAreaElement', 'Event', 'MouseEvent', 'MutationObserver']) {
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
let calls;

function status(rootStatus = 'blocked', projects = []) {
  return {
    stage: 'createReadOnly', rootStatus, projects,
    dataRoot: 'C:\\synthetic-host-fixture\\dev.localprojects.platform.m0',
    message: rootStatus === 'blocked' ? '合成保留现场：未解决的旧记录' : '合成目录状态',
  };
}
function project(state = 'ready') {
  return {
    id: 'prj_fixture', name: '合成样本', description: '可丢弃验证',
    projectRoot: 'C:\\synthetic-project-fixture', documentEpoch: 'synthetic-epoch',
    state, issues: state === 'ready' ? [] : ['合成项目缺失'],
    manifest: null, buildInfo: { origin: 'bundledTemplate', sourceState: 'matched' },
  };
}
function report() {
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
      status: 'blocked',
      checks: [{ code: 'SYNTHETIC_CHECK', status: 'blocked', message: '合成测试目录检查结果' }],
      evidence: [{ kind: 'migrationEntries', count: 3 }],
    },
  };
}

async function setup(value = status()) {
  commands = [];
  calls = [];
  responder = async command => command === 'host_status' ? report() : value;
  globalThis.__HOST_IS_TAURI__ = true;
  globalThis.__HOST_INVOKE__ = async (command, args) => {
    commands.push(command);
    calls.push({ command, args });
    assert.ok(['project_status', 'project_create', 'project_open', 'project_open_browser', 'host_status'].includes(command), 'Windows UI invoked a deferred business command: ' + command);
    return responder(command, args);
  };
  document.body.innerHTML = '<div id="root"></div>';
  root = createRoot(document.getElementById('root'));
  await act(async () => { root.render(createElement(WindowsHost)); });
}

async function refresh() {
  const button = findButton('刷新状态');
  assert.ok(button);
  await act(async () => { button.click(); });
}
function findButton(label) { return [...document.querySelectorAll('button')].find(button => button.textContent === label); }
async function click(label) { const button = findButton(label); assert.ok(button, label); await act(async () => { button.click(); }); }
async function fill(selector, value) {
  const input = document.querySelector(selector);
  assert.ok(input);
  const prototype = input instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  await act(async () => {
    Object.getOwnPropertyDescriptor(prototype, 'value').set.call(input, value);
    input.dispatchEvent(new window.Event('input', { bubbles: true }));
  });
}
async function submit() {
  const form = document.querySelector('form');
  assert.ok(form);
  await act(async () => { form.dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true })); });
}

afterEach(async () => {
  if (root) await act(async () => { root.unmount(); });
  root = null;
});
after(async () => {
  await writeFile(path.join(output, 'scope.txt'), 'Synthetic React DOM tests only. No real native host, WebView2 or workspace inspection was executed.\n');
  dom.window.close();
});

test('mount reads actual project status and blocks creation for retained unresolved roots', async () => {
  await setup();
  assert.deepEqual(commands, ['project_status']);
  assert.match(document.body.textContent, /现有目录受保护/);
  assert.match(document.body.textContent, /未解决的旧记录/);
  assert.equal(document.querySelector('.host-path').textContent, status().dataRoot);
  assert.equal(findButton('新建验证样本').disabled, true);
  assert.doesNotMatch(document.querySelector('.host-projects').textContent, /尚无项目/);
  await click('读取目录诊断');
  assert.deepEqual(commands, ['project_status', 'host_status']);
  assert.match(document.querySelector('.host-evidence').textContent, /迁移目录内条目：3/);
});

test('ready project status never starts an independent root diagnostic', async () => {
  await setup(status('ready', [project()]));
  assert.deepEqual(commands, ['project_status']);
  assert.match(document.querySelector('.host-projects').textContent, /合成样本/);
  assert.match(document.querySelector('.host-projects').textContent, /可只读打开/);
  assert.equal(findButton('读取目录诊断'), undefined);
  assert.equal(findButton('桌面打开').disabled, false);
  assert.equal(document.querySelectorAll('.host-capability:not(.available)').length, 1);
});

test('refresh replaces successful status without creating projects or opening windows', async () => {
  await setup(status('missing'));
  const next = status('ready', [project()]);
  next.dataRoot = 'C:\\synthetic-new-root';
  responder = async () => next;
  await refresh();
  assert.deepEqual(commands, ['project_status', 'project_status']);
  assert.equal(document.querySelector('.host-path').textContent, next.dataRoot);
  assert.match(document.body.textContent, /工作区已就绪/);
  assert.match(document.querySelector('.host-projects').textContent, /合成样本/);
});

test('a failed refresh preserves old status but blocks creation and opening until a successful refresh', async () => {
  await setup(status('ready', [project()]));
  responder = async () => { throw { code: 'HOST_CHECK_FAILED', message: '合成检查中断' }; };
  await refresh();
  assert.match(document.querySelector('[role="alert"]').textContent, /合成检查中断.*上次成功读取/);
  assert.equal(document.querySelector('.host-path').textContent, status().dataRoot);
  assert.match(document.querySelector('.host-projects').textContent, /合成样本/);
  assert.equal(findButton('新建验证样本').disabled, true);
  assert.equal(findButton('桌面打开').disabled, true);
  responder = async () => status('ready', [project()]);
  await refresh();
  assert.equal(findButton('桌面打开').disabled, false);
});

test('creation is explicit, keeps metadata limits, and waits for a real acknowledgement', async () => {
  await setup(status('missing'));
  await click('新建验证样本');
  assert.equal(findButton('创建样本').disabled, true);
  assert.equal(document.querySelector('input').maxLength, 200);
  assert.equal(document.querySelector('textarea').maxLength, 2000);
  await fill('input', '  合成样本  ');
  await fill('textarea', '  可丢弃验证  ');
  let resolveCreation;
  responder = command => command === 'project_create'
    ? new Promise(resolve => { resolveCreation = resolve; })
    : Promise.resolve(status('ready', [project()]));
  await submit();
  await submit();
  assert.deepEqual(commands, ['project_status', 'project_create']);
  assert.deepEqual(calls[1].args, { input: { name: '合成样本', description: '可丢弃验证' } });
  assert.equal(findButton('正在创建…').disabled, true);
  assert.doesNotMatch(document.body.textContent, /已创建「/);
  await act(async () => { resolveCreation(project()); });
  assert.deepEqual(commands, ['project_status', 'project_create', 'project_status']);
  assert.match(document.body.textContent, /已创建「合成样本」/);
  assert.equal(document.querySelector('form'), null);
});

test('cancelling a creation submits nothing and preserves the existing project view and local draft', async () => {
  const original = status('ready', [project()]);
  const before = structuredClone(original);
  await setup(original);
  await click('新建验证样本');
  await fill('input', '尚未提交的新样本');
  await fill('textarea', '取消不会调用项目写入或删除');
  const cancel = document.querySelector('button[aria-label="关闭新建表单"]');
  assert.ok(cancel);
  await act(async () => { cancel.click(); });
  assert.equal(document.querySelector('form'), null);
  assert.deepEqual(commands, ['project_status']);
  assert.equal(document.querySelector('.host-path').textContent, before.dataRoot);
  assert.equal(document.querySelectorAll('.host-project-list li').length, 1);
  assert.match(document.querySelector('.host-projects').textContent, /合成样本/);
  assert.doesNotMatch(document.body.textContent, /已创建「/);
  assert.deepEqual(original, before);
  await click('新建验证样本');
  assert.equal(document.querySelector('input').value, '尚未提交的新样本');
  assert.equal(document.querySelector('textarea').value, '取消不会调用项目写入或删除');
  assert.deepEqual(commands, ['project_status']);
});

test('a successful creation refreshes both projects and opens the newly returned ID without replacing the old one', async () => {
  const original = project();
  const created = { ...project(), id: 'prj_created_fixture', name: '新建合成样本', documentEpoch: 'new-synthetic-epoch' };
  await setup(status('ready', [original]));
  await click('新建验证样本');
  await fill('input', created.name);
  responder = async command => {
    if (command === 'project_create') return created;
    if (command === 'project_status') return status('ready', [original, created]);
    if (command === 'project_open') return { projectId: created.id, reused: false };
    throw new Error('Unexpected command: ' + command);
  };
  await submit();
  assert.deepEqual(commands, ['project_status', 'project_create', 'project_status']);
  assert.equal(document.querySelectorAll('.host-project-list li').length, 2);
  const card = [...document.querySelectorAll('.host-project-list li')].find(item => item.querySelector('.host-project-id').textContent === created.id);
  assert.ok(card);
  const open = [...card.querySelectorAll('button')].find(button => button.textContent === '桌面打开');
  assert.ok(open);
  await act(async () => { open.click(); });
  assert.deepEqual(calls.at(-1), { command: 'project_open', args: { projectId: created.id } });
  assert.match(document.querySelector('[role="status"].host-notice').textContent, /已打开「新建合成样本」/);
  assert.equal(document.querySelectorAll('.host-project-list li').length, 2);
  assert.ok([...document.querySelectorAll('.host-project-id')].some(item => item.textContent === original.id));
});

test('an ambiguous creation failure preserves the draft and never retries or claims success', async () => {
  await setup(status('missing'));
  await click('新建验证样本');
  await fill('input', '待确认样本');
  responder = async () => { throw { code: 'DISCONNECTED', message: '合成创建响应中断', details: { operationOutcome: 'unknown' } }; };
  await submit();
  assert.deepEqual(commands, ['project_status', 'project_create']);
  assert.match(document.querySelector('[role="alert"]').textContent, /合成创建响应中断/);
  assert.equal(document.querySelector('input').value, '待确认样本');
  assert.equal(findButton('创建样本').disabled, true);
  assert.doesNotMatch(document.body.textContent, /已创建「/);
  await click('创建样本');
  assert.deepEqual(commands, ['project_status', 'project_create']);
  const committed = { ...project(), id: 'prj_ambiguous_committed', name: '待确认样本' };
  responder = async () => status('ready', [committed]);
  await refresh();
  assert.deepEqual(commands, ['project_status', 'project_create', 'project_status']);
  assert.ok([...document.querySelectorAll('.host-project-id')].some(item => item.textContent === committed.id));
  assert.equal(document.querySelector('input').value, '待确认样本');
  assert.equal(findButton('创建样本').disabled, false);
  assert.equal(commands.filter(command => command === 'project_create').length, 1);
  assert.doesNotMatch(document.body.textContent, /已创建「/);
});

test('desktop and browser actions send only the project ID to their separate native commands', async () => {
  await setup(status('ready', [project()]));
  responder = async () => ({ projectId: 'prj_fixture', reused: false });
  await click('桌面打开');
  assert.deepEqual(calls[1], { command: 'project_open', args: { projectId: 'prj_fixture' } });
  assert.match(document.body.textContent, /已打开「合成样本」项目窗口/);
  await click('浏览器打开');
  assert.deepEqual(calls[2], { command: 'project_open_browser', args: { projectId: 'prj_fixture' } });
  assert.match(document.body.textContent, /已请求默认浏览器/);
});

test('a pending project opening blocks duplicate and competing actions until its acknowledgement', async () => {
  await setup(status('ready', [project()]));
  let resolveOpen;
  responder = command => {
    assert.equal(command, 'project_open');
    return new Promise(resolve => { resolveOpen = resolve; });
  };
  await click('桌面打开');
  assert.deepEqual(commands, ['project_status', 'project_open']);
  assert.equal(findButton('正在打开…').disabled, true);
  assert.equal(findButton('浏览器打开').disabled, true);
  assert.equal(findButton('新建验证样本').disabled, true);
  assert.equal(findButton('刷新状态').disabled, true);
  assert.doesNotMatch(document.body.textContent, /已打开「/);
  await click('正在打开…');
  await click('浏览器打开');
  await click('新建验证样本');
  await refresh();
  assert.deepEqual(commands, ['project_status', 'project_open']);
  await act(async () => { resolveOpen({ projectId: 'prj_fixture', reused: false }); });
  assert.match(document.body.textContent, /已打开「合成样本」/);
  assert.equal(findButton('桌面打开').disabled, false);
  assert.equal(findButton('浏览器打开').disabled, false);
  assert.deepEqual(commands, ['project_status', 'project_open']);
});

test('project modification, deletion and deferred lifecycle features have no actionable manager entry', async () => {
  await setup(status('ready', [project()]));
  const deferred = /修改|编辑|重命名|删除|构建|备份|恢复|导入|导出|复制|回收|迁移|清理|SQL/;
  const actionNames = [...document.querySelectorAll('button, a, [role="menuitem"]')]
    .map(element => element.getAttribute('aria-label') ?? element.textContent);
  for (const name of actionNames) assert.doesNotMatch(name, deferred);
  assert.match(document.querySelector('.host-capabilities').textContent, /业务写入与管理.*未开放/);
  assert.deepEqual(commands, ['project_status']);
});

test('a failed project opening preserves the last status and requires a successful refresh before retrying', async () => {
  await setup(status('ready', [project()]));
  responder = async () => { throw { code: 'PROJECT_CHANGED', message: '合成项目身份已变化' }; };
  await click('桌面打开');
  assert.deepEqual(commands, ['project_status', 'project_open']);
  assert.match(document.querySelector('[role="alert"]').textContent, /合成项目身份已变化.*请刷新确认后再操作/);
  assert.match(document.querySelector('.host-projects').textContent, /合成样本/);
  assert.doesNotMatch(document.body.textContent, /已打开「/);
  assert.equal(findButton('新建验证样本').disabled, true);
  assert.equal(findButton('桌面打开').disabled, true);
  assert.equal(findButton('浏览器打开').disabled, true);
  responder = async () => { throw { code: 'HOST_CHECK_FAILED', message: '合成刷新中断' }; };
  await refresh();
  assert.equal(findButton('桌面打开').disabled, true);
  assert.equal(findButton('浏览器打开').disabled, true);
  responder = async () => status('ready', [project()]);
  await refresh();
  assert.equal(findButton('新建验证样本').disabled, false);
  assert.equal(findButton('桌面打开').disabled, false);
  assert.equal(findButton('浏览器打开').disabled, false);
});

test('a project with a missing artifact is visible but cannot be opened', async () => {
  await setup(status('ready', [project('missing')]));
  assert.match(document.querySelector('.host-projects').textContent, /合成项目缺失/);
  assert.equal(findButton('桌面打开').disabled, true);
  assert.equal(findButton('浏览器打开').disabled, true);
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
  assert.match(document.querySelector('.host-path').textContent, /尚未取得项目状态/);
});
