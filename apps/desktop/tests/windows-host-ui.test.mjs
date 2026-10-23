// Synthetic DOM checks. These do not launch WebView2 or inspect a real workspace.
import assert from 'node:assert/strict';
import { after, afterEach, describe, test } from 'node:test';
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
const sourceRoot = path.join(repo, 'apps/desktop/src');
const compiled = new Map();
const dependencies = new Map();
async function compile(file) {
  if (compiled.has(file)) return compiled.get(file);
  const target = path.join(output, path.relative(sourceRoot, file).replace(/\.tsx?$/, '.mjs'));
  compiled.set(file, target);
  dependencies.set(file, []);
  let source = ts.transpileModule(await readFile(file, 'utf8'), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX },
    fileName: file,
  }).outputText;
  const parsed = ts.createSourceFile(target, source, ts.ScriptTarget.ES2022, true, ts.ScriptKind.JS);
  const replacements = [];
  for (const statement of parsed.statements) {
    if (!ts.isImportDeclaration(statement) && !ts.isExportDeclaration(statement)) continue;
    const specifier = statement.moduleSpecifier;
    if (!specifier || !ts.isStringLiteral(specifier)) continue;
    let replacement;
    if (specifier.text === '@tauri-apps/api/core') replacement = mock;
    else if (specifier.text.startsWith('.')) {
      const base = path.resolve(path.dirname(file), specifier.text);
      let dependency;
      for (const candidate of [base, `${base}.tsx`, `${base}.ts`, path.join(base, 'index.tsx'), path.join(base, 'index.ts')]) {
        try { await readFile(candidate); dependency = candidate; break; }
        catch (error) { if (!['ENOENT', 'EISDIR'].includes(error.code)) throw error; }
      }
      assert.ok(dependency, `Cannot resolve ${specifier.text} in ${file}`);
      assert.ok(!path.relative(sourceRoot, dependency).startsWith('..'), `UI dependency leaves source tree: ${dependency}`);
      dependencies.get(file).push(dependency);
      replacement = await compile(dependency);
    }
    if (replacement) replacements.push({ start: specifier.getStart(parsed), end: specifier.end, text: JSON.stringify(pathToFileURL(replacement).href) });
  }
  for (const replacement of replacements.reverse()) source = source.slice(0, replacement.start) + replacement.text + source.slice(replacement.end);
  await mkdir(path.dirname(target), { recursive: true });
  await writeFile(target, source);
  return target;
}
const modulePath = await compile(path.join(sourceRoot, 'WindowsHost.tsx'));
const { WINDOWS_MANAGER_CAPABILITIES: capabilities } = await import(pathToFileURL(await compile(path.join(sourceRoot, 'manager-capabilities.ts'))).href);
const allowedCommands = new Set(['project_status', 'project_create', 'project_open', 'project_open_browser', 'host_status',
  'development_environment_doctor', 'development_choose_tool', 'development_environment_configure',
  'source_list', 'source_inspect', 'source_capture', 'source_acquire', 'source_cancel', 'source_choose_directory', 'source_build_prepare', 'source_build_list', 'task_get', 'task_logs',
  'node_app_choose_source', 'node_app_inspect', 'node_app_register', 'node_app_directory_inspect', 'node_app_directory_initialize', 'node_app_directory_register', 'node_app_list', 'node_app_configuration', 'node_app_configuration_save',
  'node_app_start', 'node_app_stop', 'node_app_restart', 'node_app_status', 'node_app_open', 'node_app_open_help']);
const nativeHost = await readFile(path.join(repo, 'apps/desktop/src-tauri/src/windows_host.rs'), 'utf8');
const handler = nativeHost.match(/tauri::generate_handler!\[([\s\S]*?)\]/);
assert.ok(handler, 'Windows Tauri handler registration must be present');
const nativeCommands = new Set(handler[1].split(',').map(value => value.trim().split('::').at(-1)).filter(Boolean));

const dom = new JSDOM('<!doctype html><html><body></body></html>', { url: 'http://tauri.localhost/windows.html', pretendToBeVisual: true });
for (const name of ['window', 'document', 'Node', 'HTMLElement', 'HTMLInputElement', 'HTMLTextAreaElement', 'HTMLSelectElement', 'Event', 'MouseEvent', 'KeyboardEvent', 'FocusEvent', 'CustomEvent', 'MutationObserver']) {
  Object.defineProperty(globalThis, name, { value: name === 'window' ? dom.window : dom.window[name], configurable: true });
}
Object.defineProperty(globalThis, 'navigator', { value: dom.window.navigator, configurable: true });
globalThis.getComputedStyle = dom.window.getComputedStyle.bind(dom.window);
globalThis.requestAnimationFrame = dom.window.requestAnimationFrame.bind(dom.window);
globalThis.cancelAnimationFrame = dom.window.cancelAnimationFrame.bind(dom.window);
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
globalThis.__HOST_IS_TAURI__ = true;
const { createElement, act } = await import('react');
const { createRoot } = await import('react-dom/client');
const { WindowsHost } = await import(pathToFileURL(modulePath).href);
let root;
let commands;
let responder;
let calls;
let nodeResponder;
let restoreSourceClock;
const hostCommands = () => commands.filter(command => !command.startsWith('node_app_'));
const hostCalls = () => calls.filter(call => !call.command.startsWith('node_app_'));

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

async function setup(value = status(), nodeMock) {
  commands = [];
  calls = [];
  responder = async command => command === 'host_status' ? report() : value;
  nodeResponder = nodeMock ?? (async command => { if (command === 'node_app_list') return []; throw new Error('Unexpected Node command: ' + command); });
  globalThis.__HOST_IS_TAURI__ = true;
  globalThis.__HOST_INVOKE__ = async (command, args) => {
    commands.push(command);
    calls.push({ command, args });
    assert.ok(allowedCommands.has(command), 'Windows UI invoked a deferred business command: ' + command);
    assert.ok(nativeCommands.has(command), 'Windows UI invoked an unregistered native command: ' + command);
    return command.startsWith('node_app_') ? nodeResponder(command, args) : responder(command, args);
  };
  document.body.innerHTML = '<div id="root"></div>';
  root = createRoot(document.getElementById('root'));
  await act(async () => { root.render(createElement(WindowsHost)); });
}

async function refresh() {
  const button = document.querySelector('[aria-label="刷新项目和运行状态"]');
  assert.ok(button);
  await act(async () => { button.click(); });
}
function findButton(label, scope = document) { return [...scope.querySelectorAll('button')].find(button => button.textContent.trim() === label); }
async function click(label, scope = document) { const button = findButton(label, scope); assert.ok(button, label); await act(async () => { button.click(); }); }
async function clickElement(element) { assert.ok(element); await act(async () => { element.click(); }); }
const dialog = () => document.querySelector('[role="dialog"]');
const closeButton = () => dialog()?.querySelector('button[aria-label^="关闭"]');
const createButton = () => findButton('新建项目', document.querySelector('.radio-toolbar'));
const cards = () => [...document.querySelectorAll('.project-card')];
const card = (name = '合成样本') => cards().find(element => element.getAttribute('aria-label') === name);
const enterButton = (name = '合成样本') => card(name)?.querySelector('.launcher-open');
const navButton = label => findButton(label, document.querySelector('[aria-label="工作台导航"]'));
async function openActions() {
  const disclosure = document.querySelector('.radio-add-menu');
  if (!disclosure.open) await clickElement(disclosure.querySelector('summary'));
}
async function openSettings() {
  await clickElement(document.querySelector('[aria-label="设置"]'));
  await clickElement(dialog().querySelector('.radio-runtime-details > summary'));
}
async function closeSettings() { await click('完成', dialog()); }
async function beginCreate() { await clickElement(createButton()); }
async function projectMenu(name = '合成样本') {
  const target = enterButton(name).disabled ? card(name) : enterButton(name);
  await act(async () => target.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'ContextMenu', bubbles: true, cancelable: true })));
}
async function details(name = '合成样本') {
  await projectMenu(name);
  await clickElement(document.querySelector('[role="menu"] button[aria-label^="查看项目"]'));
}
async function key(value) {
  await act(async () => { document.activeElement.dispatchEvent(new window.KeyboardEvent('keydown', { key: value, bubbles: true, cancelable: true })); });
}
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
  const form = document.querySelector('.create-project-form');
  assert.ok(form);
  await act(async () => { form.dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true })); });
}

afterEach(async () => {
  if (root) await act(async () => { root.unmount(); });
  root = null;
  restoreSourceClock?.(); restoreSourceClock = null;
});
after(async () => {
  await writeFile(path.join(output, 'scope.txt'), 'Synthetic React DOM tests only. No real native host, WebView2 or workspace inspection was executed.\n');
  dom.window.close();
});

test('Production Windows capabilities and the actual native whitelist admit sources while excluding deferred project lifecycle operations', () => {
  assert.deepEqual([...nativeCommands].sort(), [...allowedCommands].sort());
  assert.equal(capabilities.sourceAcquisition, true);
  assert.equal(capabilities.nodeDirectoryOnboarding, true);
  for (const capability of ['build', 'projectGit', 'nodeManagedInstall', 'nodeDelete', 'deleteProject', 'metadata', 'maintenance', 'storage', 'importProject', 'exportProject', 'copyProject', 'trash', 'workspaceMigration', 'historyCleanup', 'codexHandoff', 'handoff']) assert.equal(capabilities[capability], false, capability);
  for (const command of ['choose_path', 'project_build', 'project_import', 'project_delete', 'node_install_prepare', 'node_install_apply', 'node_app_delete', 'git_initialize', 'git_save', 'project_open_codex']) assert.equal(nativeCommands.has(command), false, command);
});

test('Windows loads the actual shared Radio shell, project cards and create modal', async () => {
  await setup(status('ready', [project()]));
  const reachable = new Set();
  function visit(file) { if (reachable.has(file)) return; reachable.add(file); for (const dependency of dependencies.get(file) ?? []) visit(dependency); }
  visit(path.join(sourceRoot, 'WindowsHost.tsx'));
  for (const name of ['ManagerView.tsx', 'ProjectCard.tsx', 'Modal.tsx']) assert.ok(reachable.has(path.join(sourceRoot, name)), `WindowsHost must load actual ${name}`);
  for (const selector of ['.radio-shell > .radio-header', '.radio-toolbar .project-search', '.project-filter select', '.radio-main .project-grid .project-card', '.radio-footer', '.radio-theme-switch']) assert.ok(document.querySelector(selector), selector);
  assert.equal(document.querySelector('.radio-main > h1').textContent, '我的项目');
  assert.deepEqual([...document.querySelectorAll('.radio-primary-nav button')].map(element => element.textContent), ['我的项目', '空间与工作区']);
  assert.equal(document.querySelector('.radio-brand-caption').textContent, '熊猫应用小抽屉');
  assert.ok(card().querySelector('.launcher-icon'));
  assert.equal(card().querySelector('.launcher-name').textContent, '合成样本');
  assert.equal(card().querySelector('.launcher-more'), null);
  assert.equal(card().getAttribute('aria-keyshortcuts'), 'Shift+F10');
  assert.equal(document.querySelector('.project-filter select').closest('.radio-add-menu'), null);
  assert.equal(document.querySelector('[aria-label="刷新项目和运行状态"]').closest('.radio-add-menu'), null);
  assert.ok(card().querySelector('.launcher-open[aria-label="进入项目「合成样本」"]'));
  await beginCreate();
  assert.equal(dialog().getAttribute('aria-modal'), 'true');
  assert.equal(dialog().querySelector('h2').textContent, '新建项目');
  assert.equal(document.activeElement, dialog().querySelector('h2'));
  assert.ok(document.getElementById('root').hasAttribute('inert'));
  assert.equal(document.querySelector('#project-name').maxLength, 200);
  assert.equal(document.querySelector('#project-description').maxLength, 2000);
  await key('Escape');
  assert.equal(dialog(), null);
  assert.equal(document.getElementById('root').hasAttribute('inert'), false);
  assert.deepEqual(hostCommands(), ['project_status']);
});

test('Windows search and status filters use real events without invoking project or environment commands', async () => {
  const ready = { ...project(), id: 'prj_ready_fixture', name: '课堂费用', description: 'Split Bill sample' };
  const missing = { ...project('missing'), id: 'prj_missing_fixture', name: '另一个项目', description: 'Needs checking' };
  await setup(status('ready', [ready, missing]));
  assert.equal(cards().length, 2);
  await fill('.project-search input', '  SPLIT bill  ');
  assert.deepEqual(cards().map(element => element.getAttribute('aria-label')), [ready.name]);
  await fill('.project-search input', 'PRJ_MISSING_FIXTURE');
  assert.deepEqual(cards().map(element => element.getAttribute('aria-label')), [missing.name]);
  await fill('.project-search input', '');
  const filter = document.querySelector('.project-filter select');
  const select = async value => act(async () => {
    Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set.call(filter, value);
    filter.dispatchEvent(new window.Event('change', { bubbles: true }));
  });
  await select('ready');
  assert.deepEqual(cards().map(element => element.getAttribute('aria-label')), [ready.name]);
  await select('attention');
  assert.deepEqual(cards().map(element => element.getAttribute('aria-label')), [missing.name]);
  assert.equal(enterButton(missing.name).disabled, true);
  await fill('.project-search input', 'not in either project');
  assert.equal(cards().length, 0);
  assert.ok(document.querySelector('.project-search-empty'));
  await click('显示全部项目');
  assert.equal(document.querySelector('.project-search input').value, '');
  assert.equal(filter.value, 'all');
  assert.equal(cards().length, 2);
  assert.deepEqual(hostCommands(), ['project_status']);
});

test('mount reads actual project status and blocks creation for retained unresolved roots', async () => {
  await setup();
  assert.deepEqual(hostCommands(), ['project_status']);
  assert.match(document.body.textContent, /未解决的旧记录/);
  assert.equal(createButton().disabled, true);
  assert.equal(cards().length, 0);
  assert.doesNotMatch(document.querySelector('.empty-projects').textContent, /从第一个项目开始/);
  await openSettings();
  assert.match(document.body.textContent, /现有目录受保护/);
  assert.equal(document.querySelector('code[aria-label="固定开发数据目录"]').textContent, status().dataRoot);
  await click('读取目录诊断');
  assert.deepEqual(hostCommands(), ['project_status', 'host_status']);
  assert.match(document.body.textContent, /迁移目录内条目：3/);
});

test('ready project status never starts an independent root diagnostic', async () => {
  await setup(status('ready', [project()]));
  assert.deepEqual(hostCommands(), ['project_status']);
  assert.ok(card());
  assert.match(document.getElementById(enterButton().getAttribute('aria-describedby')).textContent, /可只读打开/);
  assert.match(enterButton().title, /可只读打开/);
  assert.equal(findButton('读取目录诊断'), undefined);
  assert.equal(enterButton().disabled, false);
  await openSettings();
  assert.equal(findButton('读取目录诊断'), undefined);
  assert.deepEqual(hostCommands(), ['project_status']);
});

test('refresh replaces successful status without creating projects or opening windows', async () => {
  await setup(status('missing'));
  const next = status('ready', [project()]);
  next.dataRoot = 'C:\\synthetic-new-root';
  responder = async () => next;
  await refresh();
  assert.deepEqual(hostCommands(), ['project_status', 'project_status']);
  assert.ok(card());
  await openSettings();
  assert.equal(document.querySelector('code[aria-label="固定开发数据目录"]').textContent, next.dataRoot);
  assert.match(document.body.textContent, /工作区已就绪/);
});

test('a failed refresh preserves old status but blocks creation and opening until a successful refresh', async () => {
  await setup(status('ready', [project()]));
  responder = async () => { throw { code: 'HOST_CHECK_FAILED', message: '合成检查中断' }; };
  await refresh();
  assert.match(document.querySelector('[role="alert"]').textContent, /合成检查中断.*上次成功读取/);
  assert.ok(card());
  assert.equal(createButton().disabled, true);
  assert.equal(enterButton().disabled, true);
  await openSettings();
  assert.equal(document.querySelector('code[aria-label="固定开发数据目录"]').textContent, status().dataRoot);
  await closeSettings();
  responder = async () => status('ready', [project()]);
  await refresh();
  assert.equal(enterButton().disabled, false);
});

test('creation is explicit, keeps metadata limits, and waits for a real acknowledgement', async () => {
  await setup(status('missing'));
  await beginCreate();
  assert.equal(findButton('创建项目', dialog()).disabled, true);
  assert.equal(document.querySelector('#project-name').maxLength, 200);
  assert.equal(document.querySelector('#project-description').maxLength, 2000);
  await fill('#project-name', '  合成样本  ');
  await fill('#project-description', '  可丢弃验证  ');
  let resolveCreation;
  responder = command => command === 'project_create'
    ? new Promise(resolve => { resolveCreation = resolve; })
    : Promise.resolve(status('ready', [project()]));
  await submit();
  await submit();
  assert.deepEqual(hostCommands(), ['project_status', 'project_create']);
  assert.deepEqual(hostCalls()[1].args, { input: { name: '合成样本', description: '可丢弃验证' } });
  assert.equal(findButton('正在创建…', dialog()).disabled, true);
  assert.equal(closeButton().disabled, true);
  await key('Escape');
  await clickElement(dialog().parentElement);
  await clickElement(closeButton());
  assert.ok(dialog(), 'Pending create blocks Escape, backdrop and close button');
  assert.doesNotMatch(document.body.textContent, /已创建「/);
  await act(async () => { resolveCreation(project()); });
  assert.deepEqual(hostCommands(), ['project_status', 'project_create', 'project_status']);
  assert.match(document.body.textContent, /已创建「合成样本」/);
  assert.equal(document.querySelector('.create-project-form'), null);
});

test('cancelling a creation submits nothing and preserves the existing project view and local draft', async () => {
  const original = status('ready', [project()]);
  const before = structuredClone(original);
  await setup(original);
  await beginCreate();
  await fill('#project-name', '尚未提交的新样本');
  await fill('#project-description', '取消不会调用项目写入或删除');
  const cancel = closeButton();
  assert.ok(cancel);
  await act(async () => { cancel.click(); });
  assert.equal(document.querySelector('.create-project-form'), null);
  assert.deepEqual(hostCommands(), ['project_status']);
  assert.equal(cards().length, 1);
  assert.ok(card());
  assert.doesNotMatch(document.body.textContent, /已创建「/);
  assert.deepEqual(original, before);
  await beginCreate();
  assert.equal(document.querySelector('#project-name').value, '尚未提交的新样本');
  assert.equal(document.querySelector('#project-description').value, '取消不会调用项目写入或删除');
  assert.deepEqual(hostCommands(), ['project_status']);
});

test('a successful creation refreshes both projects and opens the newly returned ID without replacing the old one', async () => {
  const original = project();
  const created = { ...project(), id: 'prj_created_fixture', name: '新建合成样本', documentEpoch: 'new-synthetic-epoch' };
  await setup(status('ready', [original]));
  await beginCreate();
  await fill('#project-name', created.name);
  responder = async command => {
    if (command === 'project_create') return created;
    if (command === 'project_status') return status('ready', [original, created]);
    if (command === 'project_open') return { projectId: created.id, reused: false };
    throw new Error('Unexpected command: ' + command);
  };
  await submit();
  assert.deepEqual(hostCommands(), ['project_status', 'project_create', 'project_status']);
  assert.equal(cards().length, 2);
  const createdCard = card(created.name);
  assert.ok(createdCard);
  const open = enterButton(created.name);
  assert.ok(open);
  await act(async () => { open.click(); });
  assert.deepEqual(calls.at(-1), { command: 'project_open', args: { projectId: created.id } });
  assert.match(document.body.textContent, /已打开「新建合成样本」/);
  assert.equal(cards().length, 2);
  assert.ok(card(original.name));
});

test('an ambiguous creation failure preserves the draft and never retries or claims success', async () => {
  await setup(status('missing'));
  await beginCreate();
  await fill('#project-name', '待确认样本');
  responder = async () => { throw { code: 'DISCONNECTED', message: '合成创建响应中断', details: { operationOutcome: 'unknown' } }; };
  await submit();
  assert.deepEqual(hostCommands(), ['project_status', 'project_create']);
  assert.match(dialog().querySelector('[role="alert"]').textContent, /合成创建响应中断/);
  assert.equal(document.querySelector('#project-name').value, '待确认样本');
  assert.equal(findButton('创建项目', dialog()).disabled, true);
  assert.doesNotMatch(document.body.textContent, /已创建「/);
  await click('创建项目', dialog());
  assert.deepEqual(hostCommands(), ['project_status', 'project_create']);
  const committed = { ...project(), id: 'prj_ambiguous_committed', name: '待确认样本' };
  responder = async () => status('ready', [committed]);
  await click('刷新状态', dialog());
  assert.deepEqual(hostCommands(), ['project_status', 'project_create', 'project_status']);
  assert.ok(card(committed.name));
  assert.equal(document.querySelector('#project-name').value, '待确认样本');
  assert.equal(findButton('创建项目', dialog()).disabled, false);
  assert.equal(commands.filter(command => command === 'project_create').length, 1);
  assert.doesNotMatch(document.body.textContent, /已创建「/);
});

test('desktop and browser actions send only the project ID to their separate native commands', async () => {
  await setup(status('ready', [project()]));
  responder = async () => ({ projectId: 'prj_fixture', reused: false });
  await clickElement(enterButton());
  assert.deepEqual(hostCalls()[1], { command: 'project_open', args: { projectId: 'prj_fixture' } });
  assert.match(document.body.textContent, /已打开「合成样本」项目窗口/);
  await details();
  assert.deepEqual(hostCommands(), ['project_status', 'project_open'], 'Opening read-only details makes no native request');
  await click('浏览器打开', dialog());
  assert.deepEqual(hostCalls()[2], { command: 'project_open_browser', args: { projectId: 'prj_fixture' } });
  assert.match(document.body.textContent, /已请求默认浏览器/);
});

test('a pending project opening blocks duplicate and competing actions until its acknowledgement', async () => {
  await setup(status('ready', [project()]));
  await details();
  let resolveOpen;
  responder = command => {
    assert.equal(command, 'project_open');
    return new Promise(resolve => { resolveOpen = resolve; });
  };
  await click('进入项目', dialog());
  assert.deepEqual(hostCommands(), ['project_status', 'project_open']);
  const desktopAction = dialog().querySelector('.project-dialog-actions button');
  assert.equal(desktopAction.disabled, true);
  assert.equal(findButton('浏览器打开', dialog()).disabled, true);
  assert.equal(createButton().disabled, true);
  assert.equal(document.querySelector('[aria-label="刷新项目和运行状态"]').disabled, true);
  assert.doesNotMatch(document.body.textContent, /已打开「/);
  await clickElement(desktopAction);
  await click('浏览器打开', dialog());
  assert.deepEqual(hostCommands(), ['project_status', 'project_open']);
  await act(async () => { resolveOpen({ projectId: 'prj_fixture', reused: false }); });
  assert.match(document.body.textContent, /已打开「合成样本」/);
  assert.equal(findButton('进入项目', dialog()).disabled, false);
  assert.equal(findButton('浏览器打开', dialog()).disabled, false);
  assert.deepEqual(hostCommands(), ['project_status', 'project_open']);
});

test('opening a project blocks a same-tick refresh before React renders the disabled controls', async () => {
  await setup(status('ready', [project()]));
  const enter = enterButton();
  const refreshButton = document.querySelector('[aria-label="刷新项目和运行状态"]');
  assert.equal(enter.disabled, false);
  assert.equal(refreshButton.disabled, false);
  assert.equal(dialog(), null);
  assert.equal(enter.closest('[inert]'), null);
  assert.equal(refreshButton.closest('[inert]'), null);
  let resolveOpen;
  const pending = new Promise(resolve => { resolveOpen = resolve; });
  responder = command => command === 'project_open' ? pending : Promise.resolve(status('ready', [project()]));
  try {
    await act(async () => {
      enter.click();
      assert.equal(refreshButton.disabled, false, 'The second event must run against the pre-render enabled refresh button');
      refreshButton.click();
    });
    assert.deepEqual(hostCommands(), ['project_status', 'project_open'], 'The user refresh handler must see the pending open synchronously');
    assert.equal(refreshButton.disabled, true);
    assert.doesNotMatch(document.body.textContent, /已打开「/);
  } finally {
    await act(async () => { resolveOpen({ projectId: project().id, reused: false }); });
  }
  assert.deepEqual(hostCommands(), ['project_status', 'project_open']);
  assert.match(document.body.textContent, /已打开「合成样本」/);
});

test('a blocked-root diagnostic rejects duplicate diagnostics and refreshes in the same tick', async () => {
  await setup(status('blocked'));
  await openSettings();
  const diagnosticButton = findButton('读取目录诊断');
  const refreshButton = document.querySelector('[aria-label="刷新设置中的项目状态"]');
  assert.equal(diagnosticButton.disabled, false);
  assert.equal(refreshButton.disabled, false);
  assert.ok(dialog());
  assert.equal(diagnosticButton.closest('[inert]'), null);
  assert.equal(refreshButton.closest('[inert]'), null);
  let resolveDiagnostic;
  const pending = new Promise(resolve => { resolveDiagnostic = resolve; });
  responder = command => command === 'host_status' ? pending : Promise.resolve(status('blocked'));
  try {
    await act(async () => {
      diagnosticButton.click();
      assert.equal(diagnosticButton.disabled, false, 'The duplicate event must precede the pending-state render');
      assert.equal(refreshButton.disabled, false, 'The refresh event must precede the pending-state render');
      diagnosticButton.click();
      refreshButton.click();
    });
    assert.deepEqual(hostCommands(), ['project_status', 'host_status'], 'One pending diagnostic must exclude a second diagnostic and a project-status refresh');
    assert.equal(diagnosticButton.disabled, true);
    assert.equal(refreshButton.disabled, true);
  } finally {
    await act(async () => { resolveDiagnostic(report()); });
  }
  assert.deepEqual(hostCommands(), ['project_status', 'host_status']);
  assert.match(document.body.textContent, /合成测试目录检查结果/);
  assert.equal(findButton('读取目录诊断').disabled, false);
});

test('deferred manager entries explain their disabled state and never mount business panels', async () => {
  await setup(status('ready', [project()]));
  await openActions();
  const disabled = [navButton('空间与工作区'), findButton('导入项目')];
  assert.equal(findButton('添加本地应用').disabled, false, 'Ready roots may register internal Node samples');
  for (const element of disabled) {
    assert.ok(element);
    assert.equal(element.disabled, true);
    assert.ok(element.title.trim(), 'A deferred visible entry needs its reason');
    await clickElement(element);
  }
  assert.equal(document.querySelector('.radio-main > h1').textContent, '我的项目');
  await projectMenu();
  const deletion = document.querySelector('[role="menuitem"][aria-label^="删除项目"]');
  assert.equal(deletion.disabled, true);
  assert.ok(deletion.title.trim());
  await clickElement(deletion);
  assert.deepEqual(hostCommands(), ['project_status']);
  await key('Escape');
  await openSettings();
  assert.equal(dialog().querySelector('.modal-header h2').textContent, '设置');
  assert.equal(document.querySelector('code[aria-label="固定开发数据目录"]').textContent, status().dataRoot);
  assert.equal(findButton('读取目录诊断'), undefined, 'A ready root must not be independently inspected');
  for (const selector of ['.build-environment', '.about-platform', '.node-app-detail', '.node-apps-panel']) assert.equal(document.querySelector(selector), null, selector);
  await closeSettings();
  await details();
  assert.equal(dialog().querySelector('h2').textContent, project().name);
  assert.match(dialog().textContent, /只读/);
  assert.match(dialog().textContent, /prj_fixture/);
  assert.match(dialog().textContent, /synthetic-project-fixture/);
  assert.equal(dialog().querySelector('form, input, textarea, [role="tab"]'), null, 'Read-only details must not mount metadata or maintenance editors');
  for (const selector of ['.overview-panel', '.backup-panel', '.workspace-hub', '.workspace-cleanup-panel', '.build-environment', '.handoff-card', '.import-panel']) assert.equal(document.querySelector(selector), null, selector);
  for (const element of dialog().querySelectorAll('button')) {
    const name = element.getAttribute('aria-label') ?? element.textContent;
    if (/修改|编辑|重命名|构建|备份|恢复|导入|导出|复制|回收|迁移|清理|SQL/.test(name)) {
      assert.equal(element.disabled, true, `${name} must not be actionable`);
      assert.ok(element.title.trim(), `${name} needs its unavailable reason`);
      await clickElement(element);
    }
  }
  await key('Escape');
  assert.equal(dialog(), null);
  assert.deepEqual(hostCommands(), ['project_status']);
});

test('Windows shares persistent appearance and themes its body-level settings portal without native commands', async () => {
  const key = 'panda-drawer:appearance:v1';
  window.localStorage.setItem(key, 'dark');
  await setup(status('ready', [project()]));
  assert.equal(document.documentElement.dataset.theme, 'dark');
  assert.equal(document.querySelector('.radio-shell').dataset.theme, 'dark');
  await openSettings();
  assert.equal(document.querySelector('.modal-layer').parentElement, document.body);
  assert.equal(dialog().closest('.radio-shell'), null);
  assert.equal(document.documentElement.dataset.theme, 'dark');
  await closeSettings();
  await clickElement(document.querySelector('[aria-label="切换亮色主题"]'));
  assert.equal(window.localStorage.getItem(key), 'light');
  assert.equal(document.documentElement.dataset.theme, 'light');
  assert.deepEqual(hostCommands(), ['project_status']);
});

test('a failed project opening preserves the last status and requires a successful refresh before retrying', async () => {
  await setup(status('ready', [project()]));
  responder = async () => { throw { code: 'PROJECT_CHANGED', message: '合成项目身份已变化' }; };
  await clickElement(enterButton());
  assert.deepEqual(hostCommands(), ['project_status', 'project_open']);
  assert.match(document.querySelector('[role="alert"]').textContent, /合成项目身份已变化.*请刷新确认后再操作/);
  assert.ok(card());
  assert.doesNotMatch(document.body.textContent, /已打开「/);
  assert.equal(createButton().disabled, true);
  assert.equal(enterButton().disabled, true);
  await details();
  assert.equal(findButton('浏览器打开', dialog()).disabled, true);
  await clickElement(closeButton());
  responder = async () => { throw { code: 'HOST_CHECK_FAILED', message: '合成刷新中断' }; };
  await refresh();
  assert.equal(enterButton().disabled, true);
  responder = async () => status('ready', [project()]);
  await refresh();
  assert.equal(createButton().disabled, false);
  assert.equal(enterButton().disabled, false);
  await details();
  assert.equal(findButton('浏览器打开', dialog()).disabled, false);
});

test('a project with a missing artifact is visible but cannot be opened', async () => {
  await setup(status('ready', [project('missing')]));
  assert.equal(enterButton().disabled, true);
  await details();
  assert.match(dialog().textContent, /合成项目缺失/);
  assert.equal(findButton('进入项目', dialog()).disabled, true);
  assert.equal(findButton('浏览器打开', dialog()).disabled, true);
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
  assert.equal(cards().length, 0);
  assert.doesNotMatch(document.body.textContent, /synthetic-host-fixture|合成目录状态/);
});

// Node integration below uses synthetic IPC replies and artificial credentials.
// It does not start Node, native windows, external services or real data roots.
function nodeSample(extra = {}) {
  return {
    id: 'node_windows_fixture', name: '可丢弃 Node 样本', sourcePath: 'C:\\synthetic-node-source', runtime: 'node-service',
    state: 'stopped', restartRequired: false, runningRevision: null, lastError: null,
    fields: [
      { name: 'API_URL', label: '服务地址', type: 'url', required: true, secret: false, default: 'https://example.invalid/api', description: '合成地址', howToGet: [] },
      { name: 'KEY', label: '样本密钥', type: 'string', required: true, secret: true, endpointField: 'API_URL', helpUrl: 'https://example.invalid/help', description: '人工凭据', howToGet: [] },
      { name: 'LIMIT', label: '数量', type: 'integer', required: false, secret: false, default: 7, description: '合成数量', howToGet: [] },
    ],
    configuration: { revision: 1, values: {}, secretStatus: { KEY: 'set' }, missingRequired: [], canStart: true },
    ...extra,
  };
}
const nodeCalls = () => calls.filter(call => call.command.startsWith('node_app_'));
async function fillNode(label, value) {
  const field = [...dialog().querySelectorAll('label')].find(element => element.textContent.startsWith(label));
  assert.ok(field, label);
  await fill(`[id="${field.htmlFor}"]`, value);
}
async function submitNode() {
  await act(async () => dialog().querySelector('.node-configuration-form').dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true })));
}
async function beginNodeAdd() { await openActions(); await click('添加本地应用'); }
function directoryReview(node, expectedRevision, state = 'ready') {
  return { sourcePath: node.sourcePath, state, expectedRevision, issues: [], reviewPrompt: '完整人工只读检查说明', ...(state === 'ready' ? { inspection: { expectedRevision: 'inner-source-revision', runtime: 'node-service', runtimeLabel: 'Node 服务项目', app: { id: 'sample-manifest', name: node.name, version: '1.0.0' }, configuration: { fields: node.fields, missingRequired: [] } } } : {}) };
}

test('Missing and protected roots never read Node state and explain the initialization requirement', async () => {
  await setup(status('missing'));
  assert.deepEqual(commands, ['project_status']);
  assert.match(document.body.textContent, /先创建一个可丢弃网页项目/);
  await openActions();
  assert.equal(findButton('添加本地应用').disabled, true);
  assert.match(findButton('添加本地应用').title, /初始化固定工作区/);
  responder = async () => status('blocked');
  await refresh();
  assert.deepEqual(nodeCalls(), []);
  await openActions();
  assert.equal(findButton('添加本地应用').disabled, true);
});

test('Ready Windows roots share Node and web counts, search and filters without starting applications', async () => {
  const node = nodeSample();
  await setup(status('ready', [project()]), async command => { assert.equal(command, 'node_app_list'); return [node]; });
  assert.deepEqual(commands, ['project_status', 'node_app_list']);
  assert.equal(cards().length, 2);
  assert.match(document.querySelector('.radio-footer').textContent, /2 APPS/);
  await fill('.project-search input', 'C:\\synthetic-node-source');
  assert.deepEqual(cards().map(element => element.getAttribute('aria-label')), [node.name]);
  assert.match(document.querySelector('.radio-footer').textContent, /1 \/ 2 APPS/);
  await fill('.project-search input', '');
  const filter = document.querySelector('.project-filter select');
  await act(async () => { filter.value = 'ready'; filter.dispatchEvent(new window.Event('change', { bubbles: true })); });
  assert.equal(cards().length, 2);
  assert.equal(enterButton(node.name).disabled, false);
  assert.deepEqual(nodeCalls().map(call => call.command), ['node_app_list']);
});

test('Managed install, project Git, Codex and Node deletion stay gated while source acquisition is explicitly available', async () => {
  // Managed metadata can arrive in an observation without granting a host the
  // corresponding business commands. These replies still do not touch disk.
  const node = nodeSample({ installation: { kind: 'managed', generation: 2, sourceId: 'synthetic-source', sourceDigest: 'synthetic-digest' } });
  await setup(status('ready', [project()]), async command => {
    if (command === 'node_app_list') return [node];
    if (command === 'node_app_configuration') return node;
    throw new Error('Unexpected Node command: ' + command);
  });
  await openActions();
  assert.equal(findButton('获取项目源码').disabled, false);
  await projectMenu(node.name);
  const deletion = document.querySelector('[role="menuitem"][aria-label^="删除项目"]');
  assert.equal(deletion.disabled, true);
  assert.ok(deletion.title.trim());
  await clickElement(deletion);
  await key('Escape');
  await details(node.name);
  assert.equal(findButton('删除项目', dialog()), undefined);
  assert.equal(findButton('选择更新来源或查看候选', dialog()), undefined);
  assert.equal(dialog().querySelector('.node-install-panel, .node-delete-confirmation'), null);
  assert.match(dialog().textContent, /完整外部分发包、迁移、备份与回收流程尚未接入/);
  await click('应用配置', dialog());
  await click('运行状态', dialog());
  await key('Escape');
  await beginNodeAdd();
  assert.ok(findButton('选择本地项目文件夹', dialog()));
  assert.equal(findButton('确认生成起始项目', dialog()), undefined);
  assert.ok(dialog().querySelector('.local-directory-panel'));
  await key('Escape');
  await openSettings();
  for (const selector of ['.radio-git-details', '.git-environment', '.handoff-card', '.source-acquisition-panel', '.node-install-panel']) assert.equal(document.querySelector(selector), null, selector);
  assert.deepEqual(commands, ['project_status', 'node_app_list', 'node_app_configuration']);
});

test('Failed Node reads retain samples and disable Node actions until an explicit successful refresh', async () => {
  const node = nodeSample();
  await setup(status('ready', [project()]), async () => [node]);
  nodeResponder = async () => { throw { message: '合成列表读取失败' }; };
  await refresh();
  assert.ok(card(node.name));
  assert.equal(enterButton(node.name).disabled, true);
  assert.equal(enterButton().disabled, false, 'A Node list error does not invent a web-project failure');
  await openActions();
  assert.equal(findButton('添加本地应用').disabled, true);
  const before = nodeCalls().length;
  await clickElement(enterButton(node.name));
  await projectMenu(node.name);
  assert.equal(document.querySelector('[role="menu"]'), null);
  assert.equal(nodeCalls().length, before);
  nodeResponder = async () => [node];
  await refresh();
  assert.equal(enterButton(node.name).disabled, false);
  responder = async () => { throw { message: '合成工作区状态中断' }; };
  await refresh();
  assert.equal(enterButton(node.name).disabled, true);
  assert.equal(nodeCalls().length, before + 1, 'A failed project status cannot trigger a Node read');
});

test('Missing required Node settings open configuration; saving keeps revision and never starts the application', async () => {
  const missing = nodeSample({ configuration: { revision: 4, values: {}, secretStatus: { KEY: 'missing' }, missingRequired: ['KEY'], canStart: false } });
  const saved = nodeSample({ configuration: { revision: 5, values: { LIMIT: 0 }, secretStatus: { KEY: 'set' }, missingRequired: [], canStart: true } });
  await setup(status('ready', [project()]), async (command) => {
    if (command === 'node_app_list') return [missing];
    if (command === 'node_app_configuration') return missing;
    if (command === 'node_app_configuration_save') return saved;
    throw new Error(command);
  });
  await clickElement(enterButton(missing.name));
  assert.equal(dialog().querySelector('[role="tab"][aria-selected="true"]').textContent, '应用配置');
  assert.match(dialog().textContent, /请先配置：样本密钥/);
  await fillNode('样本密钥', 'artificial-windows-secret');
  await fillNode('数量', '0');
  await submitNode();
  assert.deepEqual(nodeCalls().at(-1), { command: 'node_app_configuration_save', args: { appId: missing.id, input: { revision: 4, values: { LIMIT: 0 }, secrets: { KEY: { action: 'replace', value: 'artificial-windows-secret' } } } } });
  assert.equal(dialog().querySelector('input[type="password"]').value, '');
  assert.equal(nodeCalls().some(call => /node_app_start|node_app_restart/.test(call.command)), false);
});

test('Pending Node entry excludes duplicate starts and same-tick web, refresh and settings actions', async () => {
  const node = nodeSample();
  let settleStart;
  let observed = node;
  await setup(status('ready', [project()]), async command => {
    if (command === 'node_app_list') return [observed];
    if (command === 'node_app_configuration') return observed;
    if (command === 'node_app_start') return new Promise(resolve => { settleStart = resolve; });
    if (command === 'node_app_open') return null;
    throw new Error(command);
  });
  const nodeEntry = enterButton(node.name);
  const webEntry = enterButton();
  const refreshButton = document.querySelector('[aria-label="刷新项目和运行状态"]');
  const settingsButton = document.querySelector('[aria-label="设置"]');
  await act(async () => { nodeEntry.click(); nodeEntry.click(); webEntry.click(); refreshButton.click(); settingsButton.click(); });
  assert.equal(dialog(), null);
  assert.deepEqual(commands, ['project_status', 'node_app_list', 'node_app_configuration', 'node_app_start']);
  assert.equal(nodeEntry.disabled, true);
  observed = { ...node, state: 'running', runningRevision: 1 };
  await act(async () => settleStart(observed));
  await clickElement(enterButton(node.name));
  assert.deepEqual(nodeCalls().slice(-2).map(call => call.command), ['node_app_configuration', 'node_app_open']);
  assert.equal(nodeCalls().filter(call => call.command === 'node_app_start').length, 1);
});

test('Windows local-directory selection previews first, registers once and opens configuration without starting', async () => {
  const node = nodeSample();
  const preview = directoryReview(node, 'synthetic-reviewed-directory-revision');
  await setup(status('ready', [project()]), async command => {
    if (command === 'node_app_list') return [];
    if (command === 'node_app_choose_source') return node.sourcePath;
    if (command === 'node_app_directory_inspect') return preview;
    if (command === 'node_app_directory_register') return node;
    throw new Error(command);
  });
  await beginNodeAdd();
  await click(findButton('选择本地项目文件夹', dialog()) ? '选择本地项目文件夹' : '重新选择文件夹', dialog());
  assert.deepEqual(nodeCalls().slice(-2), [
    { command: 'node_app_choose_source', args: undefined },
    { command: 'node_app_directory_inspect', args: { sourcePath: node.sourcePath } },
  ]);
  assert.match(dialog().textContent, /添加不读取旧的 .env，也不会自动运行应用/);
  assert.equal(nodeCalls().some(call => call.command === 'node_app_directory_register'), false);
  await click('确认添加此项目', dialog());
  assert.deepEqual(nodeCalls().at(-1), { command: 'node_app_directory_register', args: { sourcePath: node.sourcePath, expectedRevision: preview.expectedRevision } });
  assert.equal(dialog().querySelector('[role="tab"][aria-selected="true"]').textContent, '应用配置');
  assert.equal(cards().length, 2);
  assert.equal(nodeCalls().some(call => call.command === 'node_app_start'), false);
});

test('Windows empty directories require a separate acknowledged initialization and reviewed registration', async () => {
  const node = nodeSample();
  await setup(status('ready'), async command => {
    if (command === 'node_app_list') return [];
    if (command === 'node_app_choose_source') return node.sourcePath;
    if (command === 'node_app_directory_inspect') return directoryReview(node, 'synthetic-empty-review', 'empty');
    if (command === 'node_app_directory_initialize') return directoryReview(node, 'synthetic-initialized-review');
    if (command === 'node_app_directory_register') return node;
    throw new Error(command);
  });
  await beginNodeAdd();
  await click('选择本地项目文件夹', dialog());
  assert.match(dialog().textContent, /这是一个空目录/);
  assert.equal(findButton('确认添加此项目', dialog()), undefined);
  const name = dialog().querySelector('.local-directory-empty input');
  assert.equal(name.value, 'synthetic-node-source');
  await fill(`[id="${name.id}"]`, 'Windows disposable starter');
  await click('在空目录生成起始项目…', dialog());
  assert.equal(findButton('确认生成起始项目', dialog()).disabled, true);
  assert.equal(nodeCalls().some(call => call.command === 'node_app_directory_initialize'), false);
  await clickElement(dialog().querySelector('[aria-label="生成起始项目确认"] input[type="checkbox"]'));
  await click('确认生成起始项目', dialog());
  assert.deepEqual(nodeCalls().at(-1), { command: 'node_app_directory_initialize', args: { sourcePath: node.sourcePath, input: { expectedRevision: 'synthetic-empty-review', name: 'Windows disposable starter' } } });
  assert.match(dialog().textContent, /尚未添加或运行/);
  assert.equal(nodeCalls().some(call => call.command === 'node_app_directory_register' || call.command === 'node_app_start'), false);
  await click('确认添加此项目', dialog());
  assert.deepEqual(nodeCalls().at(-1), { command: 'node_app_directory_register', args: { sourcePath: node.sourcePath, expectedRevision: 'synthetic-initialized-review' } });
  assert.equal(dialog().querySelector('[role="tab"][aria-selected="true"]').textContent, '应用配置');
  assert.equal(nodeCalls().some(call => /node_app_start|node_app_restart|node_install|source_acquire/.test(call.command)), false);
});

test('Windows review-needed folders keep complete AI instructions and canonical case aliases without writing', async () => {
  const node = nodeSample(); const review = { ...directoryReview(node, 'synthetic-review-only', 'needs-review'), reviewPrompt: 'Complete artificial instructions\nDo not execute this folder.' };
  const copied = [];
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async value => copied.push(value) } });
  await setup(status('ready'), async command => {
    if (command === 'node_app_list') return [];
    if (command === 'node_app_choose_source') return node.sourcePath.toLowerCase().replaceAll('\\', '/');
    if (command === 'node_app_directory_inspect') return review;
    throw new Error(command);
  });
  await beginNodeAdd(); await click('选择本地项目文件夹', dialog());
  assert.equal(dialog().querySelector('.local-directory-path input').value, node.sourcePath);
  assert.equal(dialog().querySelector('textarea').value, review.reviewPrompt);
  assert.deepEqual(copied, []);
  await click('复制给 AI 检查', dialog()); assert.deepEqual(copied, [review.reviewPrompt]);
  assert.equal(findButton('确认添加此项目', dialog()), undefined);
  assert.equal(findButton('确认生成起始项目', dialog()), undefined);
  assert.deepEqual(nodeCalls().map(call => call.command), ['node_app_list', 'node_app_choose_source', 'node_app_directory_inspect']);
  delete navigator.clipboard;
});

test('Unknown Windows initialization preserves the selected folder and requires reconciliation without repeating the write', async () => {
  const node = nodeSample();
  await setup(status('ready'), async command => {
    if (command === 'node_app_list') return [];
    if (command === 'node_app_choose_source') return node.sourcePath;
    if (command === 'node_app_directory_inspect') return directoryReview(node, 'synthetic-unknown-initialization', 'empty');
    if (command === 'node_app_directory_initialize') throw { message: '合成初始化响应中断，请保留已生成文件', details: { operationOutcome: 'unknown' } };
    throw new Error(command);
  });
  await beginNodeAdd(); await click('选择本地项目文件夹', dialog());
  await click('在空目录生成起始项目…', dialog());
  await clickElement(dialog().querySelector('[aria-label="生成起始项目确认"] input[type="checkbox"]'));
  await click('确认生成起始项目', dialog());
  assert.match(dialog().querySelector('.local-directory-path input').value, /synthetic-node-source/);
  assert.equal(findButton('重新检查', dialog()).disabled, true);
  assert.equal(findButton('确认生成起始项目', dialog()), undefined);
  await click('重新检查', dialog());
  await clickElement(closeButton()); await refresh();
  assert.equal(dialog(), null);
  assert.equal(nodeCalls().filter(call => call.command === 'node_app_directory_initialize').length, 1);
});

for (const invalidRevision of [undefined, '', '   ', null, 42]) {
  test(`Windows cannot register a source preview without a valid revision (${JSON.stringify(invalidRevision)})`, async () => {
    const node = nodeSample();
    await setup(status('ready'), async command => {
      if (command === 'node_app_list') return [];
      if (command === 'node_app_choose_source') return node.sourcePath;
      if (command === 'node_app_directory_inspect') return directoryReview(node, invalidRevision);
      throw new Error('Unexpected Node command: ' + command);
    });
    await beginNodeAdd();
    await click(findButton('选择本地项目文件夹', dialog()) ? '选择本地项目文件夹' : '重新选择文件夹', dialog());
    assert.match(dialog().textContent, /目录检查结果不完整或路径不符/);
    assert.match(dialog().querySelector('.local-directory-path input').value, /synthetic-node-source/);
    assert.equal(findButton('确认添加此项目', dialog()), undefined);
    assert.equal(nodeCalls().some(call => call.command === 'node_app_directory_register'), false);
  });
}

test('Changed Windows source revisions revoke the reviewed preview and require another explicit selection before registration', async () => {
  const node = nodeSample(); let previews = 0; let registrations = 0;
  await setup(status('ready'), async command => {
    if (command === 'node_app_list') return [];
    if (command === 'node_app_choose_source') return node.sourcePath;
    if (command === 'node_app_directory_inspect') return directoryReview(node, `synthetic-review-${++previews}`);
    if (command === 'node_app_directory_register') {
      if (++registrations === 1) throw { code: 'NODE_DIRECTORY_CHANGED', message: '目录身份或声明已变化，请重新检查后登记。', details: { operationOutcome: 'not_started' } };
      return node;
    }
    throw new Error('Unexpected Node command: ' + command);
  });
  await beginNodeAdd();
  await click(findButton('选择本地项目文件夹', dialog()) ? '选择本地项目文件夹' : '重新选择文件夹', dialog());
  await click('确认添加此项目', dialog());
  assert.deepEqual(nodeCalls().at(-1), { command: 'node_app_directory_register', args: { sourcePath: node.sourcePath, expectedRevision: 'synthetic-review-1' } });
  assert.match(dialog().textContent, /目录身份或声明已变化/);
  assert.match(dialog().querySelector('.local-directory-path input').value, /synthetic-node-source/);
  assert.equal(dialog().querySelector('.node-inspection-preview'), null);
  assert.equal(findButton('确认添加此项目', dialog()), undefined);
  assert.equal(registrations, 1);
  await click(findButton('选择本地项目文件夹', dialog()) ? '选择本地项目文件夹' : '重新选择文件夹', dialog());
  assert.equal(registrations, 1, 'A fresh preview cannot replay a rejected registration');
  await click('确认添加此项目', dialog());
  assert.deepEqual(nodeCalls().at(-1), { command: 'node_app_directory_register', args: { sourcePath: node.sourcePath, expectedRevision: 'synthetic-review-2' } });
  assert.equal(registrations, 2);
  assert.equal(dialog().querySelector('[role="tab"][aria-selected="true"]').textContent, '应用配置');
  assert.equal(nodeCalls().some(call => call.command === 'node_app_start'), false);
});

test('An unknown Node start is never replayed; explicit refresh observes a running app before focusing it', async () => {
  let observed = nodeSample();
  await setup(status('ready', [project()]), async command => {
    if (command === 'node_app_list') return [observed];
    if (command === 'node_app_configuration') return observed;
    if (command === 'node_app_start') throw { message: '合成启动响应中断', details: { operationOutcome: 'unknown' } };
    if (command === 'node_app_open') return null;
    throw new Error(command);
  });
  await clickElement(enterButton(observed.name));
  assert.ok(dialog());
  assert.match(dialog().textContent, /状态尚未确认|合成启动响应中断/);
  assert.equal(findButton('进入项目', dialog()).disabled, true);
  await click('进入项目', dialog());
  assert.equal(nodeCalls().filter(call => call.command === 'node_app_start').length, 1);
  await clickElement(closeButton());
  observed = { ...observed, state: 'running', runningRevision: 1 };
  await refresh();
  await clickElement(enterButton(observed.name));
  assert.equal(nodeCalls().at(-1).command, 'node_app_open');
  assert.equal(nodeCalls().filter(call => call.command === 'node_app_start').length, 1);
});

test('Unknown registration preserves its source and does not register again when the actual list is refreshed', async () => {
  const node = nodeSample();
  let registered = false;
  await setup(status('ready', [project()]), async command => {
    if (command === 'node_app_list') return registered ? [node] : [];
    if (command === 'node_app_choose_source') return node.sourcePath;
    if (command === 'node_app_directory_inspect') return directoryReview(node, 'synthetic-unknown-registration-review');
    if (command === 'node_app_directory_register') { registered = true; throw { message: '合成登记响应中断', details: { operationOutcome: 'unknown' } }; }
    throw new Error(command);
  });
  await beginNodeAdd();
  await click('选择本地项目文件夹', dialog());
  await click('确认添加此项目', dialog());
  assert.match(dialog().querySelector('.local-directory-path input').value, /synthetic-node-source/);
  assert.equal(findButton('确认添加此项目', dialog()), undefined);
  assert.equal(findButton('重新检查', dialog()).disabled, true);
  await click('重新检查', dialog());
  assert.equal(nodeCalls().filter(call => call.command === 'node_app_directory_register').length, 1);
  await clickElement(closeButton());
  await refresh();
  assert.ok(card(node.name));
  assert.equal(dialog(), null, 'Restoring readiness cannot replay the consumed add request');
  assert.equal(nodeCalls().filter(call => call.command === 'node_app_directory_register').length, 1);
});

test('Unknown configuration saves retain the secret draft and require list reconciliation without replay', async () => {
  const node = nodeSample();
  await setup(status('ready', [project()]), async command => {
    if (command === 'node_app_list') return [node];
    if (command === 'node_app_configuration') return node;
    if (command === 'node_app_configuration_save') throw { message: '合成保存响应中断', details: { operationOutcome: 'unknown' } };
    throw new Error(command);
  });
  await details(node.name);
  await click('应用配置', dialog());
  await fillNode('样本密钥', 'artificial-secret-draft');
  await submitNode();
  assert.equal(dialog().querySelector('input[type="password"]').value, 'artificial-secret-draft');
  assert.equal(findButton('保存配置', dialog()).disabled, true);
  await submitNode();
  assert.equal(nodeCalls().filter(call => call.command === 'node_app_configuration_save').length, 1);
  await clickElement(closeButton());
  assert.ok(dialog().querySelector('[aria-label="关闭配置确认"]'));
  await click('丢弃草稿并关闭', dialog());
  await refresh();
  assert.equal(dialog(), null);
  assert.equal(nodeCalls().filter(call => call.command === 'node_app_configuration_save').length, 1);
});

test('Node runtime actions share Mac command names, keep app IDs and apply saved running configuration only on explicit restart', async () => {
  let observed = nodeSample({ state: 'running', runningRevision: 1 });
  await setup(status('ready', [project()]), async command => {
    if (command === 'node_app_list') return [observed];
    if (command === 'node_app_configuration') return observed;
    if (command === 'node_app_configuration_save') { observed = { ...observed, restartRequired: true, configuration: { ...observed.configuration, revision: 2, values: { LIMIT: 3 } } }; return observed; }
    if (command === 'node_app_restart') { observed = { ...observed, runningRevision: 2, restartRequired: false }; return observed; }
    if (command === 'node_app_stop') { observed = { ...observed, state: 'stopped', runningRevision: null }; return observed; }
    if (command === 'node_app_status') return observed;
    if (command === 'node_app_open_help') return null;
    throw new Error(command);
  });
  await details(observed.name);
  await click('应用配置', dialog());
  await fillNode('数量', '3');
  await submitNode();
  assert.equal(nodeCalls().some(call => call.command === 'node_app_restart'), false);
  await clickElement(dialog().querySelector('.node-field-guide > summary'));
  await click('打开官方说明', dialog());
  assert.deepEqual(nodeCalls().at(-1), { command: 'node_app_open_help', args: { appId: observed.id, fieldName: 'KEY' } });
  await click('运行状态', dialog());
  await click('重启并应用配置', dialog());
  assert.deepEqual(nodeCalls().at(-1), { command: 'node_app_restart', args: { appId: observed.id } });
  await click('刷新运行状态', dialog());
  assert.deepEqual(nodeCalls().at(-1), { command: 'node_app_status', args: { appId: observed.id } });
  await click('关闭应用', dialog());
  assert.deepEqual(nodeCalls().at(-1), { command: 'node_app_stop', args: { appId: observed.id } });
  assert.match(dialog().textContent, /应用已关闭/);
});

// Development tool tests also use artificial IPC. They neither probe actual
// executables nor choose, configure or initialize a native workspace.
const developmentCalls = () => calls.filter(call => call.command.startsWith('development_'));
function developmentReport(extra = {}) {
  return {
    ready: true,
    node: { path: 'C:\\synthetic-dev-tools\\node\\node.exe', version: '24.22.0', selection: 'managed' },
    npm: { path: 'C:\\synthetic-dev-tools\\node\\node_modules\\npm\\bin\\npm-cli.js', version: '11.13.0' },
    git: { path: 'C:\\synthetic-dev-tools\\git\\cmd\\git.exe', version: '2.54.0', selection: 'managed' },
    issues: [], ...extra,
  };
}
const developmentPanel = () => document.querySelector('[aria-label="Windows 开发工具"]');
function deferredReply() { let resolve; let reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; }

test('Windows development settings remain observational until explicit checks, including missing workspace roots', async () => {
  await setup(status('missing')); await openSettings();
  assert.ok(developmentPanel()); assert.equal(findButton('检查开发工具', developmentPanel()).disabled, true);
  assert.deepEqual(developmentCalls(), []); assert.deepEqual(commands, ['project_status']);
  await closeSettings(); responder = async () => status('ready'); await refresh();
  await openSettings(); assert.deepEqual(developmentCalls(), []);
  responder = async command => { assert.equal(command, 'development_environment_doctor'); return developmentReport(); };
  await click('检查开发工具', developmentPanel());
  assert.match(developmentPanel().textContent, /开发工具已就绪/);
  assert.match(developmentPanel().textContent, /24.22.0/);
  assert.deepEqual(developmentCalls(), [{ command: 'development_environment_doctor', args: undefined }]);
});

test('Malformed Windows tool reports revoke old readiness and require a valid reread', async () => {
  await setup(status('ready')); await openSettings(); let next = developmentReport();
  responder = async command => { assert.equal(command, 'development_environment_doctor'); return next; };
  for (const malformed of [developmentReport({ node: null }), developmentReport({ npm: { path: 'relative/npm.js', version: '11' } }), developmentReport({ issues: ['unstructured error'] }), { ready: true }]) {
    next = developmentReport(); await click('检查开发工具', developmentPanel()); assert.match(developmentPanel().textContent, /开发工具已就绪/);
    next = malformed; await click('检查开发工具', developmentPanel());
    assert.doesNotMatch(developmentPanel().textContent, /开发工具已就绪/);
    assert.match(developmentPanel().textContent, /检查结果|就绪状态/);
    assert.equal(findButton('使用内置Node', developmentPanel()).disabled, true);
    assert.equal(findButton('检查开发工具', developmentPanel()).disabled, false);
  }
  assert.equal(developmentCalls().some(call => call.command === 'development_environment_configure'), false);
});

test('Windows tool picking changes only a validated draft and configuration needs its own explicit confirmation', async () => {
  await setup(status('ready')); await openSettings(); let selected = null;
  responder = async (command, args) => {
    if (command === 'development_choose_tool') return selected;
    if (command === 'development_environment_configure') return developmentReport({ node: { path: args.path, version: '24.22.0', selection: 'configured' } });
    throw new Error(command);
  };
  for (selected of [null, '', 'relative/node.exe', 'C:\\synthetic-tools\\other.exe']) {
    await click('选择开发 Node', developmentPanel());
    assert.equal(findButton('确认使用所选工具', developmentPanel()), undefined);
    assert.equal(developmentCalls().some(call => call.command === 'development_environment_configure'), false);
  }
  selected = 'C:\\synthetic-tools\\node.exe'; await click('选择开发 Node', developmentPanel());
  assert.equal(developmentCalls().at(-1).command, 'development_choose_tool');
  assert.match(developmentPanel().textContent, /synthetic-tools/);
  await click('取消选择', developmentPanel()); assert.equal(findButton('确认使用所选工具', developmentPanel()), undefined);
  assert.equal(developmentCalls().some(call => call.command === 'development_environment_configure'), false);
  await click('选择开发 Node', developmentPanel()); await click('确认使用所选工具', developmentPanel());
  assert.deepEqual(developmentCalls().at(-1), { command: 'development_environment_configure', args: { kind: 'node', path: selected } });
  assert.match(developmentPanel().textContent, /已选择/); assert.equal(findButton('确认使用所选工具', developmentPanel()), undefined);
});

test('Every pending Windows tool operation blocks all settings close paths, duplicate work and same-tick refresh', async () => {
  for (const phase of ['doctor', 'choose', 'configure']) {
    await setup(status('ready')); await openSettings(); const delayed = deferredReply();
    responder = async command => {
      if (command === 'development_choose_tool' && phase === 'configure') return 'C:\\synthetic-tools\\git.exe';
      if (command === `development_${phase === 'doctor' ? 'environment_doctor' : phase === 'choose' ? 'choose_tool' : 'environment_configure'}`) return delayed.promise;
      throw new Error(command);
    };
    if (phase === 'configure') await click('选择Git', developmentPanel());
    const action = findButton(phase === 'doctor' ? '检查开发工具' : phase === 'choose' ? '选择Git' : '确认使用所选工具', developmentPanel());
    const complete = findButton('完成', dialog()); const close = closeButton(); const layer = dialog().parentElement;
    const refresher = document.querySelector('[aria-label="刷新项目和运行状态"]'); const before = developmentCalls().length;
    await act(async () => { action.click(); action.click(); complete.click(); close.click(); layer.click(); document.activeElement.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'Escape', bubbles: true })); refresher.click(); });
    assert.ok(dialog()); assert.equal(complete.disabled, true); assert.equal(close.disabled, true);
    assert.equal(developmentCalls().length, before + 1); assert.equal(commands.filter(command => command === 'project_status').length, 1);
    await act(async () => delayed.resolve(phase === 'choose' ? null : developmentReport()));
    assert.equal(findButton('完成', dialog()).disabled, false); await closeSettings(); assert.equal(dialog(), null);
    await act(async () => root.unmount()); root = null;
  }
});

for (const outcome of ['not_started', 'unknown']) {
  test(`A ${outcome} Windows tool configuration failure preserves the draft and requires explicit rereading before another save`, async () => {
    await setup(status('ready')); await openSettings(); const selected = 'C:\\synthetic-tools\\node.exe'; let attempts = 0;
    responder = async (command, args) => {
      if (command === 'development_choose_tool') return selected;
      if (command === 'development_environment_doctor') return developmentReport();
      if (command === 'development_environment_configure') {
        if (++attempts === 1) throw { code: 'ARTIFICIAL_TOOL_FAILURE', message: '人工配置失败', details: { operationOutcome: outcome } };
        return args.path === null ? developmentReport() : developmentReport({ node: { path: selected, version: '24.22.0', selection: 'configured' } });
      }
      throw new Error(command);
    };
    await click('检查开发工具', developmentPanel()); await click('选择开发 Node', developmentPanel()); await click('确认使用所选工具', developmentPanel());
    assert.match(developmentPanel().textContent, /人工配置失败/); assert.match(developmentPanel().textContent, /先重新检查开发工具/);
    assert.match(developmentPanel().textContent, /synthetic-tools/); assert.doesNotMatch(developmentPanel().textContent, /开发工具已就绪/);
    assert.equal(findButton('确认使用所选工具', developmentPanel()).disabled, true);
    await click('确认使用所选工具', developmentPanel()); await click('使用内置Git', developmentPanel()); assert.equal(attempts, 1);
    if (outcome === 'unknown') {
      await closeSettings(); await openSettings();
      assert.equal(findButton('选择开发 Node', developmentPanel()).disabled, true, 'Reopening settings must not bypass reconciliation');
      assert.equal(findButton('使用内置Node', developmentPanel()).disabled, true);
    }
    await click('检查开发工具', developmentPanel()); assert.equal(attempts, 1, 'Rereading must not replay the failed save');
    if (outcome === 'unknown') await click('选择开发 Node', developmentPanel());
    await click('确认使用所选工具', developmentPanel()); assert.equal(attempts, 2);
    assert.deepEqual(developmentCalls().at(-1), { command: 'development_environment_configure', args: { kind: 'node', path: selected } });
    await click('使用内置Node', developmentPanel());
    assert.deepEqual(developmentCalls().at(-1), { command: 'development_environment_configure', args: { kind: 'node', path: null } });
    assert.doesNotMatch(developmentPanel().textContent, /已选择/);
  });
}

// These synthetic IPC fixtures exercise the complete WindowsHost path using
// the real production capability map without replacing or overriding it.
describe('Windows source workflow integration with the production capability map', () => {
  const sourceId = `src_${'2'.repeat(32)}`; const digest = 'd'.repeat(64);
  const sourcePath = 'C:\\synthetic-external-source';
  const sourceInspection = { kind: 'project-v1-source', name: '可丢弃源码候选', nextStep: 'review-project-source-before-integration', executionState: 'not-executed', issues: [] };
  const source = { sourceId, taskId: 'capture-fixture', name: '可丢弃源码候选', status: 'ready', provider: 'local', url: '', localSourcePath: sourcePath, sourcePath: 'C:\\synthetic-source-copy', evidencePath: 'C:\\synthetic-source-evidence', contentDigest: digest, inspection: sourceInspection };
  const build = extra => ({ taskId: 'candidate-fixture', projectId: null, type: 'sourceBuild', status: 'running', final: false, phase: 'installing-dependencies', result: null, error: null, ...extra });
  const sourceCalls = () => calls.filter(call => call.command.startsWith('source_') || ['task_get', 'task_logs'].includes(call.command));
  async function openSources() { await openActions(); await click('获取项目源码'); assert.equal(dialog()?.querySelector('h2').textContent, '获取项目源码'); }
  function controlSourceClock() {
    const set = dom.window.setInterval; const clear = dom.window.clearInterval; const timers = new Map(); let id = 0;
    dom.window.setInterval = callback => { timers.set(++id, callback); return id; };
    dom.window.clearInterval = id => timers.delete(id);
    restoreSourceClock = () => { dom.window.setInterval = set; dom.window.clearInterval = clear; };
    return async () => { await act(async () => { for (const callback of [...timers.values()]) callback(); }); };
  }
  async function selectSource() {
    const select = document.querySelector('.source-build-panel select'); assert.ok(select);
    await act(async () => { select.value = sourceId; select.dispatchEvent(new Event('change', { bubbles: true })); });
  }

  test('Opening explicitly loads only source history and excludes Mac, installation, publication and competing host actions', async () => {
    await setup(status('ready', [project()]));
    assert.equal(sourceCalls().length, 0);
    responder = async command => { if (['source_list', 'source_build_list'].includes(command)) return []; throw new Error(command); };
    await openActions(); const acquire = findButton('获取项目源码'); const refresher = document.querySelector('[aria-label="刷新项目和运行状态"]');
    await act(async () => { acquire.click(); refresher.click(); enterButton().click(); document.querySelector('[aria-label="设置"]').click(); });
    assert.equal(dialog().querySelector('h2').textContent, '获取项目源码');
    assert.deepEqual(sourceCalls(), [{ command: 'source_build_list', args: undefined }, { command: 'source_list', args: undefined }]);
    assert.equal(findButton('查看安装候选与准备安装'), undefined); assert.equal(document.querySelector('.node-install-panel'), null);
    assert.equal(commands.filter(command => command === 'project_status').length, 1);
    assert.equal(commands.some(command => /choose_path|node_install|project_build|project_import|project_open|development_environment/.test(command)), false);
    await clickElement(closeButton()); assert.equal(dialog(), null);
  });

  for (const rootStatus of ['missing', 'blocked']) {
    test(`A ${rootStatus} root cannot inspect, acquire, capture or build with the production source capability enabled`, async () => {
      await setup(status(rootStatus)); await openActions();
      const acquire = findButton('获取项目源码'); assert.ok(acquire); assert.equal(acquire.disabled, true); assert.ok(acquire.title.trim());
      if (rootStatus === 'missing') assert.match(acquire.title, /先创建一个可丢弃网页项目/);
      await clickElement(acquire); assert.equal(dialog(), null); assert.equal(sourceCalls().length, 0);
      assert.equal(commands.some(command => /project_create|node_app_directory_initialize/.test(command)), false);
    });
  }

  test('A failed root refresh revokes source admission while preserving displayed projects', async () => {
    await setup(status('ready', [project()]));
    responder = async command => { if (command === 'project_status') throw new Error('Artificial stale root'); throw new Error(command); };
    await refresh(); assert.ok(card()); await openActions();
    assert.equal(findButton('获取项目源码').disabled, true); await click('获取项目源码');
    assert.equal(dialog(), null); assert.equal(sourceCalls().length, 0);
  });

  test('Explicit local capture, digest-bound build, polling, cancellation and logs keep durable candidates on reopening without replay', async () => {
    await setup(status('ready')); const tick = controlSourceClock();
    let sources = []; let recorded = []; let current; let cancelRequested = false;
    responder = async (command, args) => {
      if (command === 'source_list') return sources;
      if (command === 'source_build_list') return recorded;
      if (command === 'source_choose_directory') { assert.equal(args, undefined); return sourcePath; }
      if (command === 'source_inspect') return { sourcePath, inspection: sourceInspection };
      if (command === 'source_capture') { sources = [source]; return { ...build(), taskId: 'capture-fixture', type: 'sourceCapture', status: 'succeeded', final: true, phase: 'completed', result: { sourceId, sourcePath: source.sourcePath } }; }
      if (command === 'source_build_prepare') { current = build(); recorded = [current]; return current; }
      if (command === 'source_cancel') { cancelRequested = true; return current; }
      if (command === 'task_get') {
        current = cancelRequested ? build({ status: 'failed', final: true, phase: 'building-candidate', error: { code: 'SOURCE_BUILD_CANCELLED', message: '合成候选已取消', details: { operationOutcome: 'not_committed', evidencePath: 'C:\\synthetic-retained-candidate' } } }) : build({ phase: 'building-candidate' });
        recorded = [current]; return current;
      }
      if (command === 'task_logs') return { taskId: args.taskId, text: '合成候选构建日志', nextCursor: 16, truncated: false, gaps: [], eof: true };
      throw new Error(command);
    };
    await openSources(); await click('检查本机源码目录');
    assert.equal(sourceCalls().some(call => call.command === 'source_capture'), false);
    await click('接入此目录的源码副本'); assert.deepEqual(sourceCalls().find(call => call.command === 'source_capture').args, { input: { sourcePath } });
    await selectSource(); assert.equal(findButton('准备独立构建候选').disabled, true);
    await clickElement(document.querySelector('.source-build-panel input[type="checkbox"]')); await click('准备独立构建候选');
    assert.deepEqual(sourceCalls().find(call => call.command === 'source_build_prepare').args, { input: { sourceId, expectedDigest: digest, acknowledgeScripts: true } });
    await tick(); assert.match(dialog().textContent, /构建独立候选网页/);
    await click('取消候选构建'); assert.deepEqual(sourceCalls().find(call => call.command === 'source_cancel').args, { taskId: 'candidate-fixture' });
    assert.match(dialog().textContent, /正在等待最终状态/); await tick(); assert.match(dialog().textContent, /synthetic-retained-candidate/);
    const buildTask = document.querySelector('[aria-label="准备独立源码候选任务"]'); await click('查看任务日志', buildTask);
    assert.deepEqual(sourceCalls().at(-1), { command: 'task_logs', args: { taskId: 'candidate-fixture', cursor: 0 } }); assert.match(buildTask.textContent, /合成候选构建日志/);
    const mutations = sourceCalls().filter(call => ['source_capture', 'source_build_prepare', 'source_cancel'].includes(call.command));
    await clickElement(closeButton()); await openSources(); assert.match(dialog().textContent, /synthetic-retained-candidate/);
    assert.deepEqual(sourceCalls().filter(call => ['source_capture', 'source_build_prepare', 'source_cancel'].includes(call.command)), mutations);
    assert.equal(sourceCalls().filter(call => call.command === 'task_logs').length, 1);
    assert.equal(commands.some(command => /node_install|project_build|project_import|choose_path/.test(command)), false);
  });

  test('An unknown build acknowledgment recovers from task history after reopening without submitting the source twice', async () => {
    await setup(status('ready')); controlSourceClock(); let accepted = false;
    responder = async command => {
      if (command === 'source_list') return [source];
      if (command === 'source_build_list') return accepted ? [build()] : [];
      if (command === 'source_build_prepare') { accepted = true; throw { code: 'ARTIFICIAL_REPLY_LOST', message: '合成回执丢失', details: { operationOutcome: 'unknown' } }; }
      throw new Error(command);
    };
    await openSources(); await selectSource(); await clickElement(document.querySelector('.source-build-panel input[type="checkbox"]')); await click('准备独立构建候选');
    assert.match(dialog().textContent, /先刷新候选构建任务/); await clickElement(closeButton()); await openSources();
    assert.match(dialog().textContent, /准备独立源码候选 · 正在执行/);
    assert.equal(sourceCalls().filter(call => call.command === 'source_build_prepare').length, 1);
    assert.equal(sourceCalls().some(call => call.command === 'source_capture'), false);
    assert.equal(document.querySelector('.source-build-panel select').disabled, true);
  });
});
