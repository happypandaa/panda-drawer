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
});
after(async () => {
  await writeFile(path.join(output, 'scope.txt'), 'Synthetic React DOM tests only. No real native host, WebView2 or workspace inspection was executed.\n');
  dom.window.close();
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
  assert.deepEqual(commands, ['project_status']);
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
  assert.deepEqual(commands, ['project_status']);
});

test('mount reads actual project status and blocks creation for retained unresolved roots', async () => {
  await setup();
  assert.deepEqual(commands, ['project_status']);
  assert.match(document.body.textContent, /未解决的旧记录/);
  assert.equal(createButton().disabled, true);
  assert.equal(cards().length, 0);
  assert.doesNotMatch(document.querySelector('.empty-projects').textContent, /从第一个项目开始/);
  await openSettings();
  assert.match(document.body.textContent, /现有目录受保护/);
  assert.equal(document.querySelector('code[aria-label="固定开发数据目录"]').textContent, status().dataRoot);
  await click('读取目录诊断');
  assert.deepEqual(commands, ['project_status', 'host_status']);
  assert.match(document.body.textContent, /迁移目录内条目：3/);
});

test('ready project status never starts an independent root diagnostic', async () => {
  await setup(status('ready', [project()]));
  assert.deepEqual(commands, ['project_status']);
  assert.ok(card());
  assert.match(document.getElementById(enterButton().getAttribute('aria-describedby')).textContent, /可只读打开/);
  assert.match(enterButton().title, /可只读打开/);
  assert.equal(findButton('读取目录诊断'), undefined);
  assert.equal(enterButton().disabled, false);
  await openSettings();
  assert.equal(findButton('读取目录诊断'), undefined);
  assert.deepEqual(commands, ['project_status']);
});

test('refresh replaces successful status without creating projects or opening windows', async () => {
  await setup(status('missing'));
  const next = status('ready', [project()]);
  next.dataRoot = 'C:\\synthetic-new-root';
  responder = async () => next;
  await refresh();
  assert.deepEqual(commands, ['project_status', 'project_status']);
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
  assert.deepEqual(commands, ['project_status', 'project_create']);
  assert.deepEqual(calls[1].args, { input: { name: '合成样本', description: '可丢弃验证' } });
  assert.equal(findButton('正在创建…', dialog()).disabled, true);
  assert.equal(closeButton().disabled, true);
  await key('Escape');
  await clickElement(dialog().parentElement);
  await clickElement(closeButton());
  assert.ok(dialog(), 'Pending create blocks Escape, backdrop and close button');
  assert.doesNotMatch(document.body.textContent, /已创建「/);
  await act(async () => { resolveCreation(project()); });
  assert.deepEqual(commands, ['project_status', 'project_create', 'project_status']);
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
  assert.deepEqual(commands, ['project_status']);
  assert.equal(cards().length, 1);
  assert.ok(card());
  assert.doesNotMatch(document.body.textContent, /已创建「/);
  assert.deepEqual(original, before);
  await beginCreate();
  assert.equal(document.querySelector('#project-name').value, '尚未提交的新样本');
  assert.equal(document.querySelector('#project-description').value, '取消不会调用项目写入或删除');
  assert.deepEqual(commands, ['project_status']);
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
  assert.deepEqual(commands, ['project_status', 'project_create', 'project_status']);
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
  assert.deepEqual(commands, ['project_status', 'project_create']);
  assert.match(dialog().querySelector('[role="alert"]').textContent, /合成创建响应中断/);
  assert.equal(document.querySelector('#project-name').value, '待确认样本');
  assert.equal(findButton('创建项目', dialog()).disabled, true);
  assert.doesNotMatch(document.body.textContent, /已创建「/);
  await click('创建项目', dialog());
  assert.deepEqual(commands, ['project_status', 'project_create']);
  const committed = { ...project(), id: 'prj_ambiguous_committed', name: '待确认样本' };
  responder = async () => status('ready', [committed]);
  await click('刷新状态', dialog());
  assert.deepEqual(commands, ['project_status', 'project_create', 'project_status']);
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
  assert.deepEqual(calls[1], { command: 'project_open', args: { projectId: 'prj_fixture' } });
  assert.match(document.body.textContent, /已打开「合成样本」项目窗口/);
  await details();
  assert.deepEqual(commands, ['project_status', 'project_open'], 'Opening read-only details makes no native request');
  await click('浏览器打开', dialog());
  assert.deepEqual(calls[2], { command: 'project_open_browser', args: { projectId: 'prj_fixture' } });
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
  assert.deepEqual(commands, ['project_status', 'project_open']);
  const desktopAction = dialog().querySelector('.project-dialog-actions button');
  assert.equal(desktopAction.disabled, true);
  assert.equal(findButton('浏览器打开', dialog()).disabled, true);
  assert.equal(createButton().disabled, true);
  assert.equal(document.querySelector('[aria-label="刷新项目和运行状态"]').disabled, true);
  assert.doesNotMatch(document.body.textContent, /已打开「/);
  await clickElement(desktopAction);
  await click('浏览器打开', dialog());
  assert.deepEqual(commands, ['project_status', 'project_open']);
  await act(async () => { resolveOpen({ projectId: 'prj_fixture', reused: false }); });
  assert.match(document.body.textContent, /已打开「合成样本」/);
  assert.equal(findButton('进入项目', dialog()).disabled, false);
  assert.equal(findButton('浏览器打开', dialog()).disabled, false);
  assert.deepEqual(commands, ['project_status', 'project_open']);
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
    assert.deepEqual(commands, ['project_status', 'project_open'], 'The user refresh handler must see the pending open synchronously');
    assert.equal(refreshButton.disabled, true);
    assert.doesNotMatch(document.body.textContent, /已打开「/);
  } finally {
    await act(async () => { resolveOpen({ projectId: project().id, reused: false }); });
  }
  assert.deepEqual(commands, ['project_status', 'project_open']);
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
    assert.deepEqual(commands, ['project_status', 'host_status'], 'One pending diagnostic must exclude a second diagnostic and a project-status refresh');
    assert.equal(diagnosticButton.disabled, true);
    assert.equal(refreshButton.disabled, true);
  } finally {
    await act(async () => { resolveDiagnostic(report()); });
  }
  assert.deepEqual(commands, ['project_status', 'host_status']);
  assert.match(document.body.textContent, /合成测试目录检查结果/);
  assert.equal(findButton('读取目录诊断').disabled, false);
});

test('deferred manager entries explain their disabled state and never mount business panels', async () => {
  await setup(status('ready', [project()]));
  await openActions();
  const disabled = [navButton('空间与工作区'), findButton('添加本地应用'), findButton('导入项目')];
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
  assert.deepEqual(commands, ['project_status']);
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
  assert.deepEqual(commands, ['project_status']);
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
  assert.deepEqual(commands, ['project_status']);
});

test('a failed project opening preserves the last status and requires a successful refresh before retrying', async () => {
  await setup(status('ready', [project()]));
  responder = async () => { throw { code: 'PROJECT_CHANGED', message: '合成项目身份已变化' }; };
  await clickElement(enterButton());
  assert.deepEqual(commands, ['project_status', 'project_open']);
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
