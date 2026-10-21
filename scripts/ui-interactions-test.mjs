// React DOM regressions in jsdom. No browser, native UI, or project service is opened.
import assert from 'node:assert/strict';
import { after, afterEach, test } from 'node:test';
import { mkdtemp, readFile, writeFile, rm, mkdir } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';
import { JSDOM } from 'jsdom';

const repo = path.resolve(fileURLToPath(new URL('..', import.meta.url)));
const outputParent = path.join(repo, '.test-artifacts');
await mkdir(outputParent, { recursive: true });
const output = await mkdtemp(path.join(outputParent, 'ui-dom-'));
const mockPath = path.join(output, 'tauri-mock.mjs');
await writeFile(mockPath, 'export const invoke = (...args) => globalThis.__UI_TEST_INVOKE__(...args); export const isTauri = () => true;\n');
const compiled = new Map();
async function component(name) {
  async function compile(file) {
    if (compiled.has(file)) return compiled.get(file);
    const target = path.join(output, `${path.basename(file).replace(/\.tsx?$/, '')}.mjs`);
    compiled.set(file, target);
    let source = ts.transpileModule(await readFile(file, 'utf8'), {
      compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX },
      fileName: file,
    }).outputText;
    for (const match of [...source.matchAll(/from\s+(['"])(\.[^'"]+)\1/g)]) {
      const relative = match[2];
      const base = path.resolve(path.dirname(file), relative);
      let dependency;
      for (const candidate of [base, `${base}.tsx`, `${base}.ts`]) {
        try { await readFile(candidate); dependency = candidate; break; } catch { /* Try the next TS extension. */ }
      }
      assert.ok(dependency, `Cannot resolve ${relative} in ${file}`);
      const result = await compile(dependency);
      source = source.replaceAll(`${match[1]}${relative}${match[1]}`, JSON.stringify(pathToFileURL(result).href));
    }
    source = source.replaceAll(/(['"])@tauri-apps\/api\/core\1/g, JSON.stringify(pathToFileURL(mockPath).href));
    await writeFile(target, source);
    return target;
  }
  return import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src', name))).href);
}

const dom = new JSDOM('<!doctype html><html><body></body></html>', { url: 'http://localhost/', pretendToBeVisual: true });
for (const name of ['window', 'document', 'Node', 'HTMLElement', 'HTMLInputElement', 'HTMLTextAreaElement', 'Event', 'MouseEvent', 'KeyboardEvent', 'FocusEvent', 'CustomEvent', 'MutationObserver']) {
  Object.defineProperty(globalThis, name, { value: name === 'window' ? dom.window : dom.window[name], configurable: true });
}
Object.defineProperty(globalThis, 'navigator', { value: dom.window.navigator, configurable: true });
globalThis.getComputedStyle = dom.window.getComputedStyle.bind(dom.window);
globalThis.requestAnimationFrame = dom.window.requestAnimationFrame.bind(dom.window);
globalThis.cancelAnimationFrame = dom.window.cancelAnimationFrame.bind(dom.window);
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
globalThis.__UI_TEST_INVOKE__ = async (command) => { throw new Error(`Unexpected native request: ${command}`); };
dom.window.HTMLElement.prototype.scrollIntoView = function () {};
const { createElement: h, act } = await import('react');
const { createRoot } = await import('react-dom/client');
const { Modal } = await component('Modal.tsx');
const { BackupMaintenancePanel } = await component('BackupMaintenancePanel.tsx');
const { ProjectOverview } = await component('ProjectOverview.tsx');
const { WorkspaceHub } = await component('WorkspaceHub.tsx');
const { WorkspaceStorageSummary } = await component('StoragePanels.tsx');
const { WorkspaceCleanupPanel } = await component('WorkspaceCleanupPanel.tsx');
const { default: App } = await component('App.tsx');
const { SourceAcquisitionPanel } = await component('SourceAcquisitionPanel.tsx');
let root;
let render;
async function setup() {
  document.body.innerHTML = '<button id="trigger">Open details</button><aside id="already-inert" inert></aside><div id="root"></div>';
  document.body.style.overflow = 'auto';
  document.getElementById('trigger').focus();
  root = createRoot(document.getElementById('root'));
  render = async (tree) => { await act(async () => { root.render(tree); }); };
}
async function click(element) { assert.ok(element); await act(async () => { element.click(); }); }
async function key(keyValue, shiftKey = false) {
  const event = new window.KeyboardEvent('keydown', { key: keyValue, shiftKey, bubbles: true, cancelable: true });
  await act(async () => { document.activeElement.dispatchEvent(event); });
  return event;
}
const dialog = () => document.querySelector('[role="dialog"]');
const closeButton = () => dialog().querySelector('button[aria-label^="关闭"]');
const layer = () => dialog().parentElement;
afterEach(async () => {
  if (root) await act(async () => root.unmount());
  root = null;
  globalThis.__UI_TEST_INVOKE__ = async (command) => { throw new Error(`Unexpected native request: ${command}`); };
  delete navigator.clipboard;
});
after(async () => { dom.window.close(); await rm(output, { recursive: true, force: true }); });

test('Source acquisition preserves failed input and blocks duplicate submission and premature close', async () => {
  await setup(); const accepted = deferred(); const calls = []; let closed = 0;
  globalThis.__UI_TEST_INVOKE__ = async (command, args) => {
    if (command === 'source_list') return [];
    assert.equal(command, 'source_acquire'); calls.push(args); return accepted.promise;
  };
  await render(h(SourceAcquisitionPanel, { disabled: false, onTask() {}, onClose() { closed++; } }));
  await inputValue(document.getElementById('source-url'), 'https://github.com/disposable/source.git');
  await click(buttonNamed('获取源码')); await click(buttonNamed('正在提交…')); await key('Escape');
  assert.equal(calls.length, 1); assert.equal(closed, 0);
  await act(async () => accepted.reject(new Error('Source download was rejected')));
  assert.match(dialog().textContent, /Source download was rejected/);
  assert.equal(document.getElementById('source-url').value, 'https://github.com/disposable/source.git');
  assert.equal(buttonNamed('获取源码').disabled, false);
});

test('Source cancel acceptance remains pending until the real task reaches its final state', async () => {
  await setup(); let cancelled = false; const forwarded = [];
  const task = { taskId: 'task_source', type: 'sourceAcquire', projectId: null, status: 'running', final: false, phase: 'downloading' };
  globalThis.__UI_TEST_INVOKE__ = async command => {
    if (command === 'source_list') return [];
    if (command === 'source_acquire') return task;
    if (command === 'source_cancel') { cancelled = true; return task; }
    assert.equal(command, 'task_get');
    return cancelled ? { ...task, status: 'failed', final: true, error: { code: 'SOURCE_ACQUISITION_CANCELLED', message: 'Cancelled; partial files retained' } } : task;
  };
  await render(h(SourceAcquisitionPanel, { disabled: false, onTask(value) { forwarded.push(value); }, onClose() {} }));
  await inputValue(document.getElementById('source-url'), 'https://github.com/disposable/source.git');
  await click(buttonNamed('获取源码')); await click(buttonNamed('取消获取'));
  assert.match(dialog().textContent, /已请求取消，正在等待任务停止/);
  assert.equal(buttonNamed('获取源码').disabled, true);
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 1100)); });
  assert.match(dialog().textContent, /Cancelled; partial files retained/);
  assert.equal(buttonNamed('获取源码').disabled, false);
  assert.equal(forwarded.at(-1).final, true);
});

test('Local source inspection only requests declarations and never registers or starts an app', async () => {
  await setup(); const calls = [];
  globalThis.__UI_TEST_INVOKE__ = async (command, args) => {
    calls.push([command, args]);
    if (command === 'source_list') return [];
    if (command === 'choose_path') { assert.equal(args.kind, 'source'); return '/disposable/local-source'; }
    assert.equal(command, 'source_inspect');
    return { sourcePath: args.sourcePath, inspection: { kind: 'needs-adaptation', executionState: 'not-executed', issues: ['Missing supported declaration'] } };
  };
  await render(h(SourceAcquisitionPanel, { disabled: false, onTask() {}, onClose() {} }));
  await click(buttonNamed('检查本机源码目录'));
  assert.match(dialog().textContent, /需要适配的源码 · 未执行代码/);
  assert.match(dialog().textContent, /Missing supported declaration/);
  assert.deepEqual(calls.map(([command]) => command), ['source_list', 'choose_path', 'source_inspect']);
});

test('Modal names itself, focuses its heading, locks the background, and restores focus/scroll/inert state', async () => {
  await setup();
  await render(h(Modal, { title: '项目设置', description: 'Read actual project information', onClose() {} }, h('input', { 'aria-label': 'Name' })));
  const modal = dialog();
  assert.equal(modal.getAttribute('aria-modal'), 'true');
  assert.equal(document.getElementById(modal.getAttribute('aria-labelledby')).textContent, '项目设置');
  assert.equal(document.getElementById(modal.getAttribute('aria-describedby')).textContent, 'Read actual project information');
  assert.equal(document.activeElement.tagName, 'H2');
  assert.ok(modal.contains(document.activeElement));
  assert.equal(document.body.style.overflow, 'hidden');
  assert.ok(document.getElementById('root').hasAttribute('inert'));
  await render(null);
  assert.equal(document.body.style.overflow, 'auto');
  assert.equal(document.activeElement.id, 'trigger');
  assert.equal(document.getElementById('root').hasAttribute('inert'), false);
  assert.ok(document.getElementById('already-inert').hasAttribute('inert'));
});

test('Modal traps Tab/Shift+Tab and ignores disabled or hidden controls', async () => {
  await setup();
  await render(h(Modal, { title: 'Focus loop', onClose() {} },
    h('button', { disabled: true }, 'Disabled'),
    h('button', { hidden: true }, 'Hidden'),
    h('div', { style: { display: 'none' } }, h('button', null, 'Hidden ancestor')),
    h('input', { id: 'draft' }), h('button', { id: 'last' }, 'Last')));
  document.getElementById('last').focus();
  assert.equal((await key('Tab')).defaultPrevented, true);
  assert.equal(document.activeElement, closeButton());
  await key('Tab', true);
  assert.equal(document.activeElement.id, 'last');
  document.getElementById('trigger').focus();
  assert.ok(dialog().contains(document.activeElement), 'Programmatic focus outside must return inside');
  await key('Tab');
  assert.equal(document.activeElement, closeButton());
});

test('Modal focus loop includes disclosure summaries while excluding controls behind closed disclosures', async () => {
  await setup();
  await render(h(Modal, { title: 'Disclosures', onClose() {} },
    h('details', { id: 'closed' }, h('summary', { id: 'summary' }, 'Runtime'), h('input', { id: 'closed-input' })),
    h('details', { open: true }, h('summary', { id: 'about-summary' }, 'About'), h('button', { id: 'visible-action' }, 'Visible action')),
    h('details', null, h('summary', { id: 'last-summary' }, 'Guide'), h('button', { id: 'hidden-last' }, 'Hidden last action'))));
  document.getElementById('summary').focus();
  assert.equal((await key('Tab')).defaultPrevented, false, 'A summary is part of the focus loop');
  document.getElementById('last-summary').focus();
  assert.equal((await key('Tab')).defaultPrevented, true);
  assert.equal(document.activeElement, closeButton());
  await key('Tab', true);
  assert.equal(document.activeElement.id, 'last-summary', 'Shift+Tab skips controls in closed disclosures');
  document.getElementById('closed').open = true;
  document.getElementById('closed-input').focus();
  assert.equal((await key('Tab')).defaultPrevented, false, 'An expanded disclosure joins the same loop');
});

test('Modal Escape, backdrop, and close button share the draft guard; clicks inside do not close', async () => {
  await setup();
  let approved = false; let closes = 0; let guards = 0;
  await render(h(Modal, { title: 'Draft', onClose() { closes++; }, beforeClose() { guards++; return approved; } }, h('button', { id: 'inside' }, 'Keep editing')));
  await click(document.getElementById('inside'));
  assert.equal(guards, 0);
  await key('Escape'); await click(layer()); await click(closeButton());
  assert.equal(guards, 3); assert.equal(closes, 0);
  approved = true;
  await key('Escape'); await click(layer()); await click(closeButton());
  assert.equal(closes, 3);
});

test('Modal pending gate blocks every close path and uses current props without stealing draft focus', async () => {
  await setup();
  let closes = 0; let guards = 0;
  const view = (pending, label) => h(Modal, { title: label, closeDisabled: pending, beforeClose() { guards++; return true; }, onClose() { closes++; } }, h('input', { id: 'draft', defaultValue: 'unsaved input' }));
  await render(view(true, 'Pending'));
  const input = document.getElementById('draft'); input.focus(); input.value = 'keep this draft';
  await key('Escape'); await click(layer()); await click(closeButton());
  assert.equal(closes, 0); assert.equal(guards, 0);
  await render(view(false, 'Ready'));
  assert.equal(document.activeElement, input); assert.equal(input.value, 'keep this draft');
  await key('Escape');
  assert.equal(closes, 1); assert.equal(guards, 1);
});

const project = { id: 'prj_ui_test', name: 'Disposable DOM fixture', state: 'ready', issues: [] };
const normalStatus = { mode: 'normal', readyForChanges: false, recoveryRequired: false };
function backupProps(extra = {}) {
  return { project, status: normalStatus, embedded: true, active: true, onTask() {}, onChanged: async () => {}, onClose() {}, ...extra };
}
async function inputValue(input, value) {
  assert.ok(input);
  await act(async () => {
    Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set.call(input, value);
    input.dispatchEvent(new window.Event('input', { bubbles: true }));
  });
}
async function submit(form) {
  await act(async () => { form.dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true })); });
}
function deferred() {
  let resolve; let reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

test('Embedded backup tab preserves both drafts while hidden, reports dirty state, and never scrolls the background', async () => {
  await setup();
  let scrolls = 0; const states = [];
  const previousScroll = dom.window.HTMLElement.prototype.scrollIntoView;
  dom.window.HTMLElement.prototype.scrollIntoView = () => { scrolls++; };
  globalThis.__UI_TEST_INVOKE__ = async (command) => { assert.equal(command, 'backup_list'); return []; };
  const view = (active) => h('div', { hidden: !active }, h(BackupMaintenancePanel, backupProps({ active, onDraftStateChange: (state) => states.push(state) })));
  try {
    await render(view(true));
    assert.equal(document.querySelector('[aria-label="关闭备份与维护"]'), null);
    await inputValue(document.getElementById('maintenance-reason'), 'Preserve maintenance reason');
    await inputValue(document.getElementById('backup-note'), 'Preserve backup note');
    assert.deepEqual(states.at(-1), { dirty: true, pending: false });
    await render(view(false));
    assert.equal(document.getElementById('backup-note').disabled, true);
    await render(view(true));
    assert.equal(document.getElementById('maintenance-reason').value, 'Preserve maintenance reason');
    assert.equal(document.getElementById('backup-note').value, 'Preserve backup note');
    assert.equal(scrolls, 0);
    await render(null);
    assert.deepEqual(states.at(-1), { dirty: false, pending: false });
  } finally { dom.window.HTMLElement.prototype.scrollIntoView = previousScroll; }
});

test('Backup submission reports pending, rejects duplicate submits, and clears only the accepted draft', async () => {
  await setup();
  const accepted = deferred(); const states = []; const tasks = []; let submissions = 0;
  globalThis.__UI_TEST_INVOKE__ = async (command, args) => {
    if (command === 'backup_list') return [];
    assert.equal(command, 'backup_create'); assert.equal(args.input.note, 'Snapshot note'); submissions++;
    return accepted.promise;
  };
  await render(h(BackupMaintenancePanel, backupProps({ onDraftStateChange: (state) => states.push(state), onTask: (task) => tasks.push(task) })));
  await inputValue(document.getElementById('maintenance-reason'), 'Unsubmitted maintenance reason');
  await inputValue(document.getElementById('backup-note'), 'Snapshot note');
  const form = document.querySelector('.backup-create-form');
  await submit(form); await submit(form);
  assert.equal(submissions, 1); assert.deepEqual(states.at(-1), { dirty: true, pending: true });
  await act(async () => accepted.resolve({ taskId: 'task_fixture', projectId: project.id, final: false }));
  assert.equal(tasks.length, 1);
  assert.equal(document.getElementById('backup-note').value, '');
  assert.equal(document.getElementById('maintenance-reason').value, 'Unsubmitted maintenance reason');
  assert.deepEqual(states.at(-1), { dirty: true, pending: false });
});

test('Failed backup submission preserves its note and reports the error without replaying the request', async () => {
  await setup(); let submissions = 0;
  globalThis.__UI_TEST_INVOKE__ = async (command) => {
    if (command === 'backup_list') return [];
    assert.equal(command, 'backup_create'); submissions++; throw new Error('Fixture response uncertain');
  };
  await render(h(BackupMaintenancePanel, backupProps()));
  await inputValue(document.getElementById('backup-note'), 'Do not lose this note');
  await submit(document.querySelector('.backup-create-form'));
  assert.equal(submissions, 1);
  assert.equal(document.getElementById('backup-note').value, 'Do not lose this note');
  assert.match(document.querySelector('[role="alert"]').textContent, /Fixture response uncertain/);
  await click([...document.querySelectorAll('button')].find((button) => button.textContent.trim() === '刷新状态'));
  assert.match(document.querySelector('[role="alert"]').textContent, /Fixture response uncertain/, 'A successful list refresh must not erase an unresolved submission error');
  assert.equal(submissions, 1);
});

test('Leaving the backup tab revokes restore/end confirmations while keeping the selected snapshot', async () => {
  await setup();
  const snapshot = { backupId: 'backup_fixture', verified: true, kind: 'manual', createdAt: 1, files: [], snapshotState: 'ready' };
  globalThis.__UI_TEST_INVOKE__ = async (command) => {
    if (command === 'backup_list') return [snapshot];
    assert.equal(command, 'backup_info'); return snapshot;
  };
  const view = (active) => h('div', { hidden: !active }, h(BackupMaintenancePanel, backupProps({ active, status: { mode: 'maintenance', readyForChanges: true, maintenanceId: 'maintenance_fixture' } })));
  await render(view(true));
  await click(document.querySelector('.backup-row'));
  const named = (name) => [...document.querySelectorAll('button')].find((button) => button.textContent.trim() === name);
  await click(named('恢复这份备份'));
  await click(document.querySelector('.restore-acknowledgement input'));
  await click(named('检查并结束维护'));
  assert.ok(document.querySelector('[aria-label="恢复备份确认"]'));
  assert.ok(document.querySelector('[aria-label="结束维护确认"]'));
  await render(view(false)); await render(view(true));
  assert.equal(document.querySelector('[aria-label="恢复备份确认"]'), null);
  assert.equal(document.querySelector('[aria-label="结束维护确认"]'), null);
  assert.match(document.querySelector('.backup-detail-column').textContent, /backup_fixture/);
  await click(named('恢复这份备份'));
  assert.equal(document.querySelector('.restore-acknowledgement input').checked, false);
});

const metadata = { name: project.name, description: '', metadataRevision: 'revision_fixture', iconStatus: { state: 'default' }, metadataIssues: [], manifest: { docs: { readme: 'README.md' } } };
function overviewProps(extra = {}) {
  return { project: { ...project, projectRoot: '/disposable/dom-fixture' }, initialTab: 'overview', status: normalStatus, disabled: false, onTask() {}, onChanged: async () => {}, onHandoff() {}, onClose() {}, ...extra };
}
const buttonNamed = (name) => [...document.querySelectorAll('button')].find((button) => button.textContent.trim() === name);

test('Project overview and Modal preserve metadata/backup drafts across tabs and require explicit discard to close', async () => {
  await setup(); let guard; let closes = 0;
  globalThis.__UI_TEST_INVOKE__ = async (command) => {
    if (command === 'project_metadata') return metadata;
    if (command === 'backup_list') return [];
    throw new Error(`Unexpected native request: ${command}`);
  };
  const close = () => { if (!guard || guard()) closes++; };
  await render(h(Modal, { title: '项目详情', onClose: close }, h(ProjectOverview, overviewProps({ onDismissGuardChange: (value) => { guard = value; }, onClose: close }))));
  const name = document.querySelector('.overview-section form input');
  await inputValue(name, 'Unsaved display name');
  await click([...document.querySelectorAll('[role="tab"]')].find((tab) => tab.textContent === '备份维护'));
  await inputValue(document.getElementById('backup-note'), 'Unsaved snapshot note');
  await click([...document.querySelectorAll('[role="tab"]')].find((tab) => tab.textContent === '概览'));
  assert.equal(name.value, 'Unsaved display name');
  assert.equal(document.getElementById('backup-note').value, 'Unsaved snapshot note');
  assert.ok(document.getElementById('backup-note').closest('[hidden]'));
  await key('Escape');
  assert.equal(closes, 0); assert.ok(document.querySelector('[aria-label="关闭详情确认"]'));
  await click(buttonNamed('继续编辑'));
  assert.equal(document.querySelector('[aria-label="关闭详情确认"]'), null);
  assert.equal(name.value, 'Unsaved display name');
  await click(closeButton());
  await click(buttonNamed('放弃未提交内容并关闭'));
  assert.equal(closes, 1);
});

test('Project icon chooser blocks closing and ignores a late selected file after unmount', async () => {
  await setup(); const chooser = deferred(); let guard; const requests = [];
  globalThis.__UI_TEST_INVOKE__ = async (command) => {
    requests.push(command);
    if (command === 'project_metadata') return metadata;
    if (command === 'choose_path') return chooser.promise;
    throw new Error(`A closed chooser must not write: ${command}`);
  };
  await render(h(ProjectOverview, overviewProps({ onDismissGuardChange: (value) => { guard = value; } })));
  await click(buttonNamed('导入 PNG 图标'));
  let canClose;
  await act(async () => { canClose = guard(); });
  assert.equal(canClose, false);
  assert.ok([...document.querySelectorAll('[role="tab"]')].every((tab) => tab.disabled));
  await render(null);
  await act(async () => chooser.resolve('/disposable/late-image.png'));
  assert.deepEqual(requests, ['project_metadata', 'choose_path']);
  assert.equal(guard, null);
});

test('Overview tabs support keyboard navigation and revoke delete acknowledgement after leaving', async () => {
  await setup();
  globalThis.__UI_TEST_INVOKE__ = async (command) => {
    if (command === 'project_metadata') return metadata;
    if (command === 'backup_list') return [];
    throw new Error(`Unexpected native request: ${command}`);
  };
  await render(h(ProjectOverview, overviewProps({ initialTab: 'manage' })));
  assert.ok(document.querySelector('[aria-label="删除项目确认"]'));
  await click(document.querySelector('.project-delete-confirmation input'));
  const tabs = [...document.querySelectorAll('[role="tab"]')];
  assert.equal(tabs.filter((tab) => tab.tabIndex === 0).length, 1);
  tabs.at(-1).focus(); await key('Home');
  assert.equal(document.activeElement, tabs[0]); assert.equal(tabs[0].getAttribute('aria-selected'), 'true');
  await key('End');
  assert.equal(document.activeElement, tabs.at(-1)); assert.equal(tabs.at(-1).getAttribute('aria-selected'), 'true');
  assert.equal(document.querySelector('[aria-label="删除项目确认"]'), null);
  await key('ArrowLeft');
  assert.equal(document.activeElement, tabs[2]); assert.equal(tabs[2].getAttribute('aria-selected'), 'true');
  await key('ArrowRight');
  assert.equal(document.activeElement, tabs[3]);
  await click(document.querySelector('.project-delete-heading button'));
  assert.equal(document.querySelector('.project-delete-confirmation input').checked, false);
});

function hubProps(extra = {}) {
  return { workspace: { workspaceRoot: '/disposable/workspace', migrationState: 'ready', retainedRoots: [] }, revision: 1, enabled: true, migrating: false, projects: [], onTask() {}, onRefresh: async () => {}, onSelect() {}, ...extra };
}
const workspaceStorage = { platformProgram: { totalBytes: null }, workspace: { totalBytes: null }, otherLocalLocations: [] };
const hubTab = (name) => [...document.querySelectorAll('[role="tab"]')].find((tab) => tab.textContent === name);

test('Workspace opens with read-only summary and quick actions focus the newly selected tab', async () => {
  await setup(); const requests = [];
  globalThis.__UI_TEST_INVOKE__ = async (command) => {
    requests.push(command);
    if (command === 'workspace_storage') return workspaceStorage;
    if (command === 'trash_list') return [];
    throw new Error(`Navigation must not mutate or open a native chooser: ${command}`);
  };
  await render(h(WorkspaceHub, hubProps()));
  assert.deepEqual(requests, ['workspace_storage']);
  assert.equal(document.querySelector('.workspace-migration-form'), null);
  await click(buttonNamed('查看工作区位置'));
  assert.equal(document.activeElement, hubTab('工作区位置'));
  assert.equal(hubTab('工作区位置').getAttribute('aria-selected'), 'true');
  assert.equal(document.querySelector('.workspace-migration-form'), null);
  await click(hubTab('空间总览')); await click(buttonNamed('管理回收站'));
  assert.equal(document.activeElement, hubTab('回收站'));
  assert.equal(hubTab('回收站').getAttribute('aria-selected'), 'true');
  assert.deepEqual([...new Set(requests)], ['workspace_storage', 'trash_list']);
  assert.equal(document.querySelector('.purge-confirmation'), null);
});

test('Storage explains retained sizes within their scopes, aggregates old roots, and preserves unknown measurements', async () => {
  await setup();
  let data = {
    platformProgram: { totalBytes: 100, complete: true },
    workspace: { totalBytes: 2000, complete: true, categories: {
      trash: { totalBytes: 0 }, backups: { totalBytes: null }, operations: { totalBytes: 1500 }, sharedCache: { totalBytes: 500 },
    } },
    otherLocalLocations: [
      { scope: { kind: 'retainedWorkspace' }, totalBytes: 1000, complete: true },
      { scope: { kind: 'retainedWorkspace' }, totalBytes: { $int64: '2000' }, complete: true },
      { scope: { kind: 'applicationData' }, totalBytes: 321, complete: true },
    ],
  };
  globalThis.__UI_TEST_INVOKE__ = async (command) => { assert.equal(command, 'workspace_storage'); return data; };
  await render(h(WorkspaceStorageSummary, { revision: 1, enabled: true }));
  const rows = () => [...document.querySelectorAll('.workspace-retention-list > div')];
  const row = (prefix) => rows().find((element) => element.querySelector('dt').textContent.startsWith(prefix));
  assert.equal(row('回收站'), undefined, 'A known complete zero category should not imply retained content');
  assert.match(row('历史备份').textContent, /尚未统计/);
  assert.equal(row('操作现场').querySelector('strong').title, '1500 字节');
  assert.equal(row('共享缓存').querySelector('strong').title, '500 字节');
  assert.equal(row('迁移保留目录').querySelector('strong').title, '3000 字节');
  assert.equal(row('应用设置').querySelector('strong').title, '321 字节');
  assert.equal(rows().filter((element) => element.querySelector('dt').textContent.startsWith('迁移保留目录')).length, 1);
  assert.match(document.querySelector('.workspace-retention-explanation').textContent, /不再与上方数字相加|不等于可以释放/);
  data = { ...data, workspace: { ...data.workspace, complete: false, stale: true }, otherLocalLocations: [...data.otherLocalLocations, { scope: { kind: 'retainedWorkspace' }, totalBytes: null, complete: false }] };
  await render(h(WorkspaceStorageSummary, { revision: 2, enabled: true }));
  assert.match(row('回收站').textContent, /0 B.*已过期/);
  assert.match(row('迁移保留目录').textContent, /尚未统计.*部分统计/);
});

function cleanupPlan(extra = {}) {
  return { planId: 'cleanup_plan_dom', expiresAt: Date.now() + 60000, workspaceRoot: '/disposable/workspace', confirmationText: '永久清理所选内容', preserved: ['当前项目和全部当前备份'], items: [
    { id: 'retained_fixture', kind: 'retainedWorkspace', label: 'Old disposable snapshot', path: '/disposable/old-workspace', totalBytes: 1234, fileCount: 4, canClean: true, blockedReasons: [] },
    { id: 'original_protected', kind: 'recoveryOriginal', label: 'Protected recovery fixture', path: '/disposable/workspace/.platform/operations/task_fixture/original', totalBytes: null, fileCount: null, canClean: false, blockedReasons: ['Unresolved recovery still references this copy'] },
  ], ...extra };
}
const cleanupPreview = (plan) => ({ taskId: `task_preview_${plan.planId}`, type: 'workspaceCleanupPreview', projectId: null, final: true, status: 'succeeded', phase: 'completed', result: plan });
const cleanupProps = (extra = {}) => ({ workspaceRoot: '/disposable/workspace', revision: 1, enabled: true, active: true, migrating: false, onTask() {}, onRefresh: async () => {}, ...extra });
const cleanupConfirmation = () => document.querySelector('[aria-label="历史清理范围确认"]');
async function acknowledgeCleanup() {
  await click(buttonNamed('核对所选清理范围'));
  await inputValue(cleanupConfirmation().querySelector('input:not([type="checkbox"])'), '永久清理所选内容');
  await click(cleanupConfirmation().querySelector('input[type="checkbox"]'));
}

test('Cleanup controls identify same-name projects and copy or submit only the associated scope', async () => {
  await setup(); const copied = []; const submissions = [];
  const items = ['kept', 'disposable'].map((suffix) => ({ id: `trash_${suffix}`, kind: 'trash', label: '回收站完整项目', name: 'Same display name', projectId: `prj_${suffix}`, path: `/disposable/workspace/.platform/trash/trash_${suffix}`, totalBytes: 42, fileCount: 1, canClean: true, blockedReasons: [] }));
  const plan = cleanupPlan({ items });
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async (text) => { copied.push(text); } } });
  globalThis.__UI_TEST_INVOKE__ = async (command, args) => {
    if (command === 'workspace_cleanup_preview') return cleanupPreview(plan);
    assert.equal(command, 'workspace_cleanup'); submissions.push(args.input);
    return { taskId: 'task_cleanup_named_scope', type: 'workspaceCleanup', projectId: null, final: true, status: 'succeeded', phase: 'completed', result: { completed: true } };
  };
  await render(h(WorkspaceCleanupPanel, cleanupProps())); await click(buttonNamed('扫描可清理范围'));
  const rows = [...document.querySelectorAll('.workspace-cleanup-item')];
  const names = rows.map((row) => row.querySelector('input').getAttribute('aria-label'));
  assert.equal(new Set(names).size, items.length);
  for (const [index, row] of rows.entries()) {
    const checkbox = row.querySelector('input'); const copy = row.querySelector('button');
    for (const control of [checkbox, copy]) {
      const name = control.getAttribute('aria-label');
      assert.ok(name.includes('回收站项目')); assert.ok(name.includes(items[index].name));
      assert.ok(name.includes(items[index].projectId)); assert.ok(name.includes(items[index].path));
    }
    await click(copy);
  }
  assert.deepEqual(copied, items.map((item) => item.path));
  await click(rows[1].querySelector('input')); await acknowledgeCleanup();
  assert.equal(rows[0].querySelector('input').checked, false);
  await click(buttonNamed('确认永久清理所选内容'));
  assert.deepEqual(submissions, [{ planId: plan.planId, itemIds: ['trash_disposable'], confirmation: plan.confirmationText }]);
});

test('Cleanup review reveals its heading, keeps typing focus and restores the review action on cancel', async () => {
  await setup(); const scrolled = []; const scrollIntoView = HTMLElement.prototype.scrollIntoView;
  HTMLElement.prototype.scrollIntoView = function (options) { scrolled.push({ element: this, options }); };
  try {
    globalThis.__UI_TEST_INVOKE__ = async (command) => { assert.equal(command, 'workspace_cleanup_preview'); return cleanupPreview(cleanupPlan()); };
    const props = cleanupProps();
    await render(h(WorkspaceCleanupPanel, props)); await click(buttonNamed('扫描可清理范围'));
    assert.equal(document.activeElement.id, 'trigger'); assert.equal(scrolled.length, 0);
    await click(document.querySelector('.workspace-cleanup-item input'));
    const review = buttonNamed('核对所选清理范围'); review.focus(); await click(review);
    const heading = cleanupConfirmation().querySelector('h3');
    assert.equal(document.activeElement, heading); assert.equal(scrolled.at(-1).element, heading);
    assert.equal(scrolled.at(-1).options.block, 'start');
    const input = cleanupConfirmation().querySelector('input:not([type="checkbox"])'); input.focus();
    await inputValue(input, '永久清理所选内容'); await click(cleanupConfirmation().querySelector('input[type="checkbox"]'));
    await render(h(WorkspaceCleanupPanel, { ...props }));
    assert.equal(document.activeElement, input); assert.equal(scrolled.length, 1);
    buttonNamed('取消，继续保留').focus(); await click(buttonNamed('取消，继续保留'));
    assert.equal(cleanupConfirmation(), null); assert.equal(document.activeElement, review);
    assert.equal(scrolled.at(-1).element, review); assert.equal(scrolled.length, 2);
    await click(review);
    assert.equal(document.activeElement, cleanupConfirmation().querySelector('h3'));
    // Explicitly reviewing again should reveal the scope anew.
    review.focus(); await click(review);
    assert.equal(document.activeElement, cleanupConfirmation().querySelector('h3')); assert.equal(scrolled.length, 4);
  } finally { HTMLElement.prototype.scrollIntoView = scrollIntoView; }
});

test('Cleanup acceptance and its first final transition reveal results without ordinary updates stealing focus', async () => {
  await setup(); const accepted = deferred(); const scrolled = []; const scrollIntoView = HTMLElement.prototype.scrollIntoView;
  HTMLElement.prototype.scrollIntoView = function () { scrolled.push(this); };
  try {
    const oldTask = { taskId: 'task_cleanup_historical', type: 'workspaceCleanup', projectId: null, final: true, status: 'succeeded', phase: 'completed', result: { completed: true } };
    let props = cleanupProps({ cleanupTask: oldTask });
    globalThis.__UI_TEST_INVOKE__ = async (command) => {
      if (command === 'workspace_cleanup_preview') return cleanupPreview(cleanupPlan());
      assert.equal(command, 'workspace_cleanup'); return accepted.promise;
    };
    await render(h(WorkspaceCleanupPanel, props));
    assert.equal(document.activeElement.id, 'trigger'); assert.equal(scrolled.length, 0, 'Historical results must not take focus on entry');
    await click(buttonNamed('扫描可清理范围')); await click(document.querySelector('.workspace-cleanup-item input')); await acknowledgeCleanup();
    await click(buttonNamed('确认永久清理所选内容')); document.getElementById('trigger').focus();
    const running = { ...oldTask, taskId: 'task_cleanup_new', final: false, status: 'queued', phase: 'queued', result: null };
    await act(async () => accepted.resolve(running));
    const resultHeading = document.querySelector('.workspace-cleanup-result h3');
    assert.equal(document.activeElement, resultHeading); assert.equal(scrolled.at(-1), resultHeading);
    const acceptedScrollCount = scrolled.length;
    document.getElementById('trigger').focus();
    props = { ...props, cleanupTask: { ...running, status: 'running', phase: 'cleaning-trash' } };
    await render(h(WorkspaceCleanupPanel, props));
    assert.equal(document.activeElement.id, 'trigger'); assert.equal(scrolled.length, acceptedScrollCount);
    const final = { ...running, final: true, status: 'succeeded', phase: 'completed', result: { completed: true } };
    props = { ...props, cleanupTask: final }; await render(h(WorkspaceCleanupPanel, props));
    assert.equal(document.activeElement, resultHeading); assert.equal(scrolled.length, acceptedScrollCount + 1);
    document.getElementById('trigger').focus(); await render(h(WorkspaceCleanupPanel, { ...props, cleanupTask: { ...final } }));
    assert.equal(document.activeElement.id, 'trigger'); assert.equal(scrolled.length, acceptedScrollCount + 1, 'Duplicate final polls must not take focus again');
  } finally { HTMLElement.prototype.scrollIntoView = scrollIntoView; }
});

test('Cleanup completion in a hidden tab does not move focus then or when returning to the tab', async () => {
  await setup(); const scrolled = []; const scrollIntoView = HTMLElement.prototype.scrollIntoView;
  HTMLElement.prototype.scrollIntoView = function () { scrolled.push(this); };
  try {
    const running = { taskId: 'task_cleanup_hidden', type: 'workspaceCleanup', projectId: null, final: false, status: 'running', phase: 'cleaning-trash', result: null };
    const props = cleanupProps({ active: false, cleanupTask: running });
    await render(h(WorkspaceCleanupPanel, props));
    const final = { ...running, final: true, status: 'succeeded', phase: 'completed', result: { completed: true } };
    await render(h(WorkspaceCleanupPanel, { ...props, cleanupTask: final }));
    assert.equal(document.activeElement.id, 'trigger'); assert.equal(scrolled.length, 0);
    await render(h(WorkspaceCleanupPanel, { ...props, active: true, cleanupTask: final }));
    assert.equal(document.activeElement.id, 'trigger'); assert.equal(scrolled.length, 0);
  } finally { HTMLElement.prototype.scrollIntoView = scrollIntoView; }
});

test('Historical cleanup is read-only until selected IDs and the exact irreversible scope are confirmed', async () => {
  await setup(); const plan = cleanupPlan(); const accepted = deferred(); const requests = []; let refreshes = 0;
  globalThis.__UI_TEST_INVOKE__ = async (command, args) => {
    requests.push({ command, args });
    if (command === 'workspace_cleanup_preview') return cleanupPreview(plan);
    assert.equal(command, 'workspace_cleanup'); return accepted.promise;
  };
  await render(h(WorkspaceCleanupPanel, cleanupProps({ onRefresh: async () => { refreshes++; } })));
  assert.deepEqual(requests, []);
  await click(buttonNamed('扫描可清理范围'));
  const items = [...document.querySelectorAll('.workspace-cleanup-item input[type="checkbox"]')];
  assert.equal(items[0].checked, false); assert.equal(items[1].checked, false); assert.equal(items[1].disabled, true);
  assert.match(document.querySelector('.workspace-cleanup-blocked').textContent, /Unresolved recovery/);
  assert.match(document.querySelector('.workspace-cleanup-plan').textContent, /不等于实际可释放/);
  await click(items[0]); await click(buttonNamed('核对所选清理范围'));
  assert.equal(buttonNamed('确认永久清理所选内容').disabled, true);
  assert.match(cleanupConfirmation().textContent, /disposable\/old-workspace/);
  await inputValue(cleanupConfirmation().querySelector('input:not([type="checkbox"])'), '永久清理所选内容');
  assert.equal(buttonNamed('确认永久清理所选内容').disabled, true);
  await click(cleanupConfirmation().querySelector('input[type="checkbox"]'));
  await click(buttonNamed('确认永久清理所选内容'));
  assert.equal(cleanupConfirmation(), null);
  assert.deepEqual(requests[1].args.input, { planId: plan.planId, itemIds: ['retained_fixture'], confirmation: '永久清理所选内容' });
  assert.equal(buttonNamed('核对所选清理范围').disabled, true);
  await act(async () => accepted.resolve({ taskId: 'task_cleanup_partial', type: 'workspaceCleanup', projectId: null, final: true, status: 'failed', phase: 'failed', result: { completed: false, deleted: [{ path: '/disposable/old-workspace/old-file', logicalBytes: 10 }], failures: [{ path: '/disposable/old-workspace/new-file', reason: 'TARGET_CHANGED fixture' }], remaining: [{ path: '/disposable/old-workspace' }] }, error: { code: 'CLEANUP_INCOMPLETE', message: 'Partial cleanup fixture' } }));
  assert.equal(refreshes, 1);
  assert.match(document.querySelector('.workspace-cleanup-result-details').textContent, /实际已删除|TARGET_CHANGED fixture|不会恢复/);
  assert.equal(requests.filter((request) => request.command === 'workspace_cleanup').length, 1);
});

test('Changing cleanup selection, refreshing or leaving the tab revokes the permanent-cleanup confirmation', async () => {
  await setup(); const plan = cleanupPlan();
  globalThis.__UI_TEST_INVOKE__ = async (command) => { assert.equal(command, 'workspace_cleanup_preview'); return cleanupPreview(plan); };
  let props = cleanupProps();
  await render(h(WorkspaceCleanupPanel, props)); await click(buttonNamed('扫描可清理范围'));
  const selected = () => document.querySelector('.workspace-cleanup-item input[type="checkbox"]');
  await click(selected()); await acknowledgeCleanup();
  assert.equal(buttonNamed('确认永久清理所选内容').disabled, false);
  await click(selected()); assert.equal(cleanupConfirmation(), null);
  await click(selected()); await acknowledgeCleanup();
  props = { ...props, revision: 2 }; await render(h(WorkspaceCleanupPanel, props));
  assert.equal(cleanupConfirmation(), null); assert.equal(selected().checked, true);
  await acknowledgeCleanup();
  props = { ...props, active: false }; await render(h(WorkspaceCleanupPanel, props));
  props = { ...props, active: true }; await render(h(WorkspaceCleanupPanel, props));
  assert.equal(cleanupConfirmation(), null); assert.equal(selected().checked, true);
  await acknowledgeCleanup();
  props = { ...props, migrating: true }; await render(h(WorkspaceCleanupPanel, props));
  assert.equal(cleanupConfirmation(), null); assert.equal(selected().disabled, true);
});

test('Expired cleanup plans and plans for a different workspace cannot submit destructive actions', async () => {
  await setup(); const plan = cleanupPlan({ expiresAt: Date.now() - 1 }); const requests = [];
  globalThis.__UI_TEST_INVOKE__ = async (command) => { requests.push(command); assert.equal(command, 'workspace_cleanup_preview'); return cleanupPreview(plan); };
  const props = cleanupProps();
  await render(h(WorkspaceCleanupPanel, props)); await click(buttonNamed('扫描可清理范围'));
  assert.equal(document.querySelector('.workspace-cleanup-item input').disabled, true);
  assert.match(document.querySelector('.workspace-cleanup-card').textContent, /范围已过期/);
  assert.equal(buttonNamed('核对所选清理范围').disabled, true);
  await render(h(WorkspaceCleanupPanel, { ...props, workspaceRoot: '/disposable/new-workspace' }));
  assert.match(document.querySelector('.workspace-cleanup-card').textContent, /工作区已改变/);
  assert.deepEqual(requests, ['workspace_cleanup_preview']);
});

test('Historical cleanup shows per-item results, uncertain deletion lower bounds and the durable receipt', async () => {
  await setup();
  const task = { taskId: 'task_cleanup_results', type: 'workspaceCleanup', projectId: null, final: true, status: 'failed', phase: 'failed', result: {
    completed: false, historicalBackupsRetained: true, receiptPath: '/disposable/development/cleanup/receipts/task_cleanup_results', items: [
      { id: 'one', kind: 'trash', path: '/disposable/trash/project', status: 'removed', completed: true, removed: { removedLogicalBytes: 1234, removedFileCount: 5, completed: true } },
      { id: 'two', kind: 'recoveryOriginal', path: '/disposable/recovery/original', status: 'failed', completed: false, error: { code: 'TARGET_CHANGED', message: 'Disposable partial fixture', details: { cleanupResult: { removedLogicalBytes: 42, removedFileCount: 1, accountingComplete: false, countsAreLowerBounds: true, remainingPath: '/disposable/recovery/original' } } } },
      { id: 'three', kind: 'retainedWorkspace', path: '/disposable/retained', status: 'skipped', completed: false, reason: 'A preceding item needs attention' },
    ],
  } };
  await render(h(WorkspaceCleanupPanel, cleanupProps({ cleanupTask: task })));
  const results = [...document.querySelectorAll('.workspace-cleanup-item-result')];
  assert.equal(results.length, 3);
  assert.match(results[0].textContent, /已清理.*实际删除.*5 个/);
  assert.match(results[1].textContent, /已确认至少删除.*42 B.*下限.*待核对的残留目录/);
  assert.match(results[2].textContent, /未处理.*没有继续处理/);
  assert.match(document.querySelector('.workspace-cleanup-result-details').textContent, /历史备份已保留/);
  assert.equal(document.querySelector('.workspace-cleanup-receipt code').textContent, task.result.receiptPath);
});

test('A cleaned retired workspace remains an explicit historical record instead of claiming space is still occupied', async () => {
  await setup();
  globalThis.__UI_TEST_INVOKE__ = async (command) => { assert.equal(command, 'workspace_storage'); return workspaceStorage; };
  await render(h(WorkspaceHub, hubProps({ workspace: { workspaceRoot: '/disposable/workspace', migrationState: 'ready', retainedRoots: [{ path: '/disposable/old-workspace', available: false, cleaned: true, cleanup: { completed: true, receiptPath: '/disposable/development/cleanup/receipt' } }] } })));
  await click(buttonNamed('查看工作区位置'));
  const record = document.querySelector('.retained-roots');
  assert.match(record.textContent, /已清理/);
  assert.match(record.textContent, /不表示该目录仍存在或仍占用空间/);
  assert.match(record.textContent, /disposable\/old-workspace/);
});

test('Trash ignores out-of-order details, keeps confirmation bound to its selected ID, and resets it after tab changes', async () => {
  await setup(); const first = deferred(); const second = deferred();
  const entries = ['A', 'B'].map((id) => ({ trashId: `trash_${id}`, projectId: `prj_${id}`, name: 'Same display name', state: 'trashed', restoreAvailable: true, issues: [] }));
  globalThis.__UI_TEST_INVOKE__ = async (command, args) => {
    if (command === 'workspace_storage') return workspaceStorage;
    if (command === 'trash_list') return entries;
    if (command === 'trash_info') return args.trashId === 'trash_A' ? first.promise : second.promise;
    throw new Error(`Unexpected mutation/request: ${command}`);
  };
  await render(h(WorkspaceHub, hubProps()));
  await click(hubTab('回收站'));
  const rows = [...document.querySelectorAll('.trash-row')];
  await click(rows[0]); await click(rows[1]);
  await act(async () => second.resolve(entries[1]));
  await click(buttonNamed('永久删除…'));
  await inputValue(document.querySelector('.purge-confirmation input:not([type="checkbox"])'), 'Same display name');
  await click(document.querySelector('.purge-confirmation input[type="checkbox"]'));
  assert.equal(buttonNamed('确认永久删除完整项目').disabled, false);
  await act(async () => first.resolve(entries[0]));
  assert.match(document.querySelector('.workspace-trash-details').textContent, /trash_B/);
  assert.doesNotMatch(document.querySelector('.workspace-trash-details').textContent, /trash_A/);
  await click(hubTab('工作区位置')); await click(hubTab('回收站'));
  assert.equal(document.querySelector('.purge-confirmation'), null);
});

test('Trash refuses restore for a record explicitly marked unavailable', async () => {
  await setup();
  const entry = { trashId: 'trash_unavailable', projectId: 'prj_unavailable', name: 'Needs review', state: 'needsAttention', restoreAvailable: false, issues: ['Identity mismatch'] };
  globalThis.__UI_TEST_INVOKE__ = async (command) => {
    if (command === 'workspace_storage') return workspaceStorage;
    if (command === 'trash_list') return [entry];
    if (command === 'trash_info') return entry;
    throw new Error(`Unexpected request: ${command}`);
  };
  await render(h(WorkspaceHub, hubProps())); await click(hubTab('回收站')); await click(document.querySelector('.trash-row'));
  assert.equal(buttonNamed('恢复项目').disabled, true);
  assert.match(document.querySelector('.workspace-trash-details').textContent, /Identity mismatch/);
});

test('A failed migration submission still settles after leaving and returning to its tab', async () => {
  await setup(); const result = deferred(); let submissions = 0;
  globalThis.__UI_TEST_INVOKE__ = async (command) => {
    if (command === 'workspace_storage') return workspaceStorage;
    if (command === 'trash_list') return [];
    assert.equal(command, 'workspace_migrate'); submissions++; return result.promise;
  };
  await render(h(WorkspaceHub, hubProps())); await click(hubTab('工作区位置')); await click(buttonNamed('迁移工作区'));
  await inputValue(document.querySelector('.workspace-migration-form input:not([type="checkbox"])'), '/disposable/new-workspace');
  await submit(document.querySelector('.workspace-migration-form form'));
  await click(document.querySelector('.workspace-migration-confirmation input[type="checkbox"]'));
  await click(buttonNamed('确认开始迁移'));
  assert.equal(submissions, 1);
  await click(hubTab('回收站'));
  await act(async () => result.reject(new Error('Migration fixture rejected')));
  await click(hubTab('工作区位置'));
  assert.equal(document.querySelector('.workspace-migration-form input:not([type="checkbox"])').disabled, false);
  assert.match(document.querySelector('.workspace-location-card [role="alert"]').textContent, /Migration fixture rejected/);
  assert.equal(document.querySelector('.workspace-migration-confirmation'), null);
  assert.equal(submissions, 1);
});

const appProject = (id, name) => ({ ...project, id, name, description: '', projectRoot: `/disposable/workspace/projects/${id}`, manifest: metadata.manifest });
function mockApp(state, custom = async (command) => { throw new Error(`Unexpected native request: ${command}`); }) {
  globalThis.__UI_TEST_INVOKE__ = async (command, args) => {
    if (command === 'bootstrap_info') return { displayName: '熊猫应用小抽屉', version: '0.0.0', identifier: 'dev.localprojects.platform.m0', instanceId: state.instanceId ?? 'instance_dom_fixture', workspaceRoot: '/disposable/workspace', dataRoot: '/disposable/development', resources: { sdk: true, template: true, readme: true, cli: true } };
    if (command === 'project_list') return state.projects;
    if (command === 'node_app_list') return state.nodeApps ?? [];
    if (command === 'workspace_info') return { workspaceRoot: '/disposable/workspace', migrationState: 'ready', retainedRoots: [] };
    if (command === 'maintenance_status') return normalStatus;
    if (command === 'project_metadata') return { ...metadata, name: state.projects.find((item) => item.id === args.projectId)?.name ?? 'Removed fixture' };
    if (command === 'backup_list') return [];
    return custom(command, args);
  };
}
async function openProjectMenu(name) {
  const entry = document.querySelector(`[aria-label='进入项目「${name}」']`);
  assert.ok(entry);
  await act(async () => entry.closest('article').dispatchEvent(new window.MouseEvent('contextmenu', { bubbles: true, cancelable: true })));
}
async function openDetails(name) {
  await openProjectMenu(name);
  await click(document.querySelector(`[aria-label='查看项目「${name}」的详情']`));
}

test('Project details offer one native window action and send only the desktop open target', async () => {
  await setup(); const opened = deferred(); const calls = [];
  const state = { projects: [appProject('prj_native_window', 'Native window fixture')] };
  mockApp(state, async (command, args) => {
    assert.equal(command, 'project_open'); calls.push(args); return opened.promise;
  });
  await render(h(App)); await openDetails('Native window fixture');
  const actions = dialog().querySelector('.project-dialog-actions');
  assert.ok(actions);
  assert.equal([...actions.querySelectorAll('button')].filter((button) => button.textContent === '进入项目').length, 1);
  assert.equal([...actions.querySelectorAll('button')].some((button) => /浏览器/.test(button.textContent)), false);
  const enter = actions.querySelector('button');
  await click(enter); await click(enter);
  assert.deepEqual(calls, [{ projectId: 'prj_native_window', target: 'desktop', reload: false }]);
  await act(async () => opened.resolve({ projectId: 'prj_native_window', target: 'desktop', reused: false }));
  assert.match(dialog().textContent, /已在独立窗口打开/);
});

test('App keeps the selected project and its draft after an external list removal, while disabling mutations', async () => {
  await setup();
  const state = { projects: [appProject('prj_A', 'Fixture A')] };
  mockApp(state);
  await render(h(App)); await openDetails('Fixture A');
  await inputValue(document.querySelector('.overview-section form input'), 'Retain this edited display name');
  await click(hubTab('备份维护'));
  state.projects = [];
  await click([...document.querySelectorAll('.backup-panel button')].find((button) => button.textContent.trim() === '刷新状态'));
  assert.ok(dialog(), 'An external disappearance must not discard the detail editor');
  assert.match(dialog().textContent, /项目已不在当前工作区列表中/);
  assert.ok([...document.querySelectorAll('.project-dialog-actions button')].every((button) => button.disabled));
  await click(hubTab('概览'));
  assert.equal(document.querySelector('.overview-section form input').value, 'Retain this edited display name');
  assert.equal(buttonNamed('保存基本信息').disabled, true);
  await key('Escape');
  assert.ok(document.querySelector('[aria-label="关闭详情确认"]'));
  await click(buttonNamed('继续编辑'));
  assert.ok(dialog());
});

test('App rejects a late AI handoff for a closed project after another project is selected', async () => {
  await setup(); const first = deferred(); const copied = [];
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async (text) => { copied.push(text); } } });
  const state = { projects: [appProject('prj_A', 'Fixture A'), appProject('prj_B', 'Fixture B')] };
  mockApp(state, async (command, args) => {
    assert.equal(command, 'project_handoff');
    return args.projectId === 'prj_A' ? first.promise : { text: 'Current handoff for B' };
  });
  await render(h(App)); await openDetails('Fixture A'); await click(buttonNamed('复制 AI 接手说明'));
  await click(closeButton()); await openDetails('Fixture B'); await click(buttonNamed('复制 AI 接手说明'));
  assert.equal(document.querySelector('[aria-label="给 AI 的完整说明"]').value, 'Current handoff for B');
  await act(async () => first.resolve({ text: 'Stale handoff for A' }));
  assert.equal(document.querySelector('[aria-label="给 AI 的完整说明"]').value, 'Current handoff for B');
  assert.deepEqual(copied, ['Current handoff for B']);
  assert.doesNotMatch(dialog().textContent, /Stale handoff for A|已复制「Fixture A」/);
});

test('Creating a project opens its details and offers AI handoff without copying until explicitly requested', async () => {
  await setup(); const prepared = deferred(); const copied = []; let handoffs = 0;
  const created = appProject('prj_new', 'New disposable fixture');
  const state = { projects: [] };
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async (text) => { copied.push(text); } } });
  mockApp(state, async (command, args) => {
    if (command === 'project_create') { assert.equal(args.input.name, created.name); state.projects = [created]; return created; }
    assert.equal(command, 'project_handoff'); assert.equal(args.projectId, created.id); handoffs++; return prepared.promise;
  });
  await render(h(App));
  await click([...document.querySelectorAll('.radio-toolbar button')].find((button) => button.textContent.trim() === '新建项目'));
  await inputValue(document.getElementById('project-name'), created.name);
  await submit(document.querySelector('.create-project-form'));
  assert.ok(dialog()); assert.match(dialog().textContent, /New disposable fixture/);
  assert.equal(document.querySelector('.create-project-form'), null);
  assert.equal(handoffs, 0); assert.deepEqual(copied, []);
  assert.match(document.querySelector('.handoff-card').textContent, /软件介绍、当前项目目录/);
  await click(buttonNamed('复制 AI 接手说明'));
  assert.equal(buttonNamed('正在准备说明…').disabled, true);
  await click(buttonNamed('正在准备说明…'));
  assert.equal(handoffs, 1);
  await act(async () => prepared.resolve({ text: `Platform description and project root: ${created.projectRoot}` }));
  assert.deepEqual(copied, [`Platform description and project root: ${created.projectRoot}`]);
  assert.equal(buttonNamed('复制 AI 接手说明').disabled, false);
  assert.match(dialog().querySelector('[role="status"]').textContent, /已复制/);
});

test('AI handoff remains selectable when the clipboard rejects access', async () => {
  await setup(); let attempts = 0;
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async () => { attempts++; throw new Error('Clipboard fixture denied'); } } });
  mockApp({ projects: [appProject('prj_A', 'Fixture A')] }, async (command) => {
    assert.equal(command, 'project_handoff'); return { text: 'Complete platform introduction and current project paths' };
  });
  await render(h(App)); await openDetails('Fixture A'); await click(buttonNamed('复制 AI 接手说明'));
  const text = document.querySelector('[aria-label="给 AI 的完整说明"]');
  assert.equal(text.value, 'Complete platform introduction and current project paths');
  await act(async () => text.focus());
  assert.equal(text.selectionStart, 0); assert.equal(text.selectionEnd, text.value.length);
  assert.equal(attempts, 1);
  assert.match(dialog().querySelector('[role="status"]').textContent, /可以选中下面的文字并复制/);
  assert.doesNotMatch(dialog().textContent, /已复制「/);
  await click(buttonNamed('收起全文'));
  assert.equal(document.querySelector('[aria-label="给 AI 的完整说明"]'), null);
  assert.ok(buttonNamed('复制 AI 接手说明'));
});

test('Clipboard writes stay ordered across project changes when an older system write is still pending', async () => {
  await setup(); const firstWrite = deferred(); const started = []; const completed = [];
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async (text) => {
    started.push(text);
    if (text === 'Handoff A') await firstWrite.promise;
    completed.push(text);
  } } });
  mockApp({ projects: [appProject('prj_A', 'Fixture A'), appProject('prj_B', 'Fixture B')] }, async (command, args) => {
    assert.equal(command, 'project_handoff'); return { text: args.projectId === 'prj_A' ? 'Handoff A' : 'Handoff B' };
  });
  await render(h(App)); await openDetails('Fixture A'); await click(buttonNamed('复制 AI 接手说明'));
  assert.deepEqual(started, ['Handoff A']);
  await click(closeButton()); await openDetails('Fixture B'); await click(buttonNamed('复制 AI 接手说明'));
  assert.deepEqual(started, ['Handoff A'], 'The new write must wait for an already-started system write');
  assert.equal(buttonNamed('正在准备说明…').disabled, true);
  await act(async () => firstWrite.resolve());
  assert.deepEqual(completed, ['Handoff A', 'Handoff B']);
  assert.equal(document.querySelector('[aria-label="给 AI 的完整说明"]').value, 'Handoff B');
  assert.match(dialog().querySelector('[role="status"]').textContent, /已复制「Fixture B」/);
});

test('A refreshed project removal invalidates an in-flight AI handoff without discarding the detail draft', async () => {
  await setup(); const prepared = deferred(); const copied = [];
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async (text) => { copied.push(text); } } });
  const state = { projects: [appProject('prj_A', 'Fixture A')] };
  mockApp(state, async (command) => { assert.equal(command, 'project_handoff'); return prepared.promise; });
  await render(h(App)); await openDetails('Fixture A');
  await inputValue(document.querySelector('.overview-section form input'), 'Preserve handoff fixture draft');
  await click(buttonNamed('复制 AI 接手说明'));
  await click(hubTab('备份维护')); state.projects = [];
  await click([...document.querySelectorAll('.backup-panel button')].find((button) => button.textContent.trim() === '刷新状态'));
  await act(async () => prepared.resolve({ text: 'Removed project must not be copied' }));
  assert.deepEqual(copied, []); assert.equal(document.querySelector('[aria-label="给 AI 的完整说明"]'), null);
  assert.equal(buttonNamed('复制 AI 接手说明').disabled, true);
  await click(hubTab('概览'));
  assert.equal(document.querySelector('.overview-section form input').value, 'Preserve handoff fixture draft');
});

const codexHandoff = (project, text = 'Fresh project instructions', instanceId = 'instance_dom_fixture') => ({ text, projectRoot: project.projectRoot, instanceId });
const codexButton = () => buttonNamed('复制说明并在 Codex 打开');

test('Codex handoff explicitly fetches fresh instructions, copies them, then opens only the checked project', async () => {
  await setup(); const events = []; let handoffs = 0;
  const project = appProject('prj_codex', 'Codex disposable fixture');
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async text => { events.push(['copy', text]); } } });
  mockApp({ projects: [project] }, async (command, args) => {
    events.push([command, args]);
    if (command === 'project_handoff') return codexHandoff(project, `Fresh handoff ${++handoffs}`);
    assert.equal(command, 'project_open_codex');
    return { projectRoot: project.projectRoot, requested: true };
  });
  await render(h(App)); await openDetails(project.name);
  assert.deepEqual(events, []);
  assert.match(document.querySelector('.handoff-card').textContent, /不会自动提交任务/);
  await click(codexButton());
  assert.deepEqual(events, [
    ['project_handoff', { projectId: project.id }], ['copy', 'Fresh handoff 1'],
    ['project_open_codex', { projectId: project.id, expectedProjectRoot: project.projectRoot, expectedInstanceId: 'instance_dom_fixture' }],
  ]);
  assert.equal(document.querySelector('[aria-label="给 AI 的完整说明"]').value, 'Fresh handoff 1');
  assert.ok(document.querySelector('.handoff-preview').textContent.includes(project.projectRoot));
  assert.match(dialog().querySelector('[role="status"]').textContent, /已请求 Codex.*自行发送/);
  assert.doesNotMatch(dialog().textContent, /AI 已接手|接手已完成|任务已提交/);
  await click(codexButton());
  assert.equal(handoffs, 2);
  assert.equal(events.filter(([name]) => name === 'project_open_codex').length, 2);
  await click(buttonNamed('复制 AI 接手说明'));
  assert.equal(handoffs, 3);
  assert.equal(events.filter(([name]) => name === 'project_open_codex').length, 2, 'Generic copy never opens Codex');
});

test('Codex can open when clipboard permission fails, retaining selectable instructions and honest feedback', async () => {
  await setup(); const project = appProject('prj_codex_clipboard', 'Clipboard fallback'); const opened = [];
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async () => { throw new Error('Clipboard denied'); } } });
  mockApp({ projects: [project] }, async (command, args) => {
    if (command === 'project_handoff') return codexHandoff(project, 'Retain these complete instructions');
    assert.equal(command, 'project_open_codex'); opened.push(args); return { projectRoot: project.projectRoot, requested: true };
  });
  await render(h(App)); await openDetails(project.name); await click(codexButton());
  assert.equal(opened.length, 1);
  const text = document.querySelector('[aria-label="给 AI 的完整说明"]');
  assert.equal(text.value, 'Retain these complete instructions');
  await act(async () => text.focus());
  assert.equal(text.selectionStart, 0); assert.equal(text.selectionEnd, text.value.length);
  assert.match(dialog().querySelector('[role="status"]').textContent, /剪贴板未能写入.*手动复制/);
  assert.doesNotMatch(dialog().querySelector('[role="status"]').textContent, /说明已复制/);
});

test('A Codex opening failure retains the full handoff and path, and retry refreshes the handoff', async () => {
  await setup(); const project = appProject('prj_codex_missing', 'Codex opening failure'); const copied = []; let handoffs = 0; let opens = 0;
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async text => { copied.push(text); } } });
  mockApp({ projects: [project] }, async command => {
    if (command === 'project_handoff') return codexHandoff(project, `Retained handoff ${++handoffs}`);
    assert.equal(command, 'project_open_codex'); opens++;
    throw { code: 'CODEX_NOT_INSTALLED', message: '未找到已安装的 Codex' };
  });
  await render(h(App)); await openDetails(project.name); await click(codexButton());
  assert.equal(opens, 1); assert.equal(handoffs, 1);
  assert.match(dialog().querySelector('[role="alert"]').textContent, /未找到已安装的 Codex/);
  assert.match(dialog().querySelector('[role="status"]').textContent, /说明已复制.*手动打开 Codex/);
  assert.equal(document.querySelector('[aria-label="给 AI 的完整说明"]').value, 'Retained handoff 1');
  assert.ok(document.querySelector('.handoff-preview').textContent.includes(project.projectRoot));
  assert.equal(codexButton().disabled, false);
  await click(codexButton());
  assert.equal(opens, 2); assert.equal(handoffs, 2);
  assert.deepEqual(copied, ['Retained handoff 1', 'Retained handoff 2']);
});

test('Codex handoff and native-open pending phases each reject duplicate and generic submissions', async () => {
  await setup(); const project = appProject('prj_codex_pending', 'Codex pending fixture'); const handoff = deferred(); const opened = deferred(); const calls = [];
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async () => {} } });
  mockApp({ projects: [project] }, async command => {
    calls.push(command);
    if (command === 'project_handoff') return handoff.promise;
    assert.equal(command, 'project_open_codex'); return opened.promise;
  });
  await render(h(App)); await openDetails(project.name); await click(codexButton());
  assert.equal(buttonNamed('正在准备并打开…').disabled, true);
  assert.equal(buttonNamed('复制 AI 接手说明').disabled, true);
  await click(buttonNamed('正在准备并打开…')); await click(buttonNamed('复制 AI 接手说明'));
  assert.deepEqual(calls, ['project_handoff']);
  await act(async () => handoff.resolve(codexHandoff(project)));
  assert.equal(buttonNamed('正在准备并打开…').disabled, true);
  assert.equal(buttonNamed('收起全文').disabled, true);
  await click(buttonNamed('正在准备并打开…'));
  assert.deepEqual(calls, ['project_handoff', 'project_open_codex']);
  await act(async () => opened.resolve({ projectRoot: project.projectRoot, requested: true }));
  assert.equal(codexButton().disabled, false);
});

test('Closing a Codex request before handoff returns prevents copying or opening the old project', async () => {
  await setup(); const project = appProject('prj_codex_old', 'Old Codex fixture'); const next = appProject('prj_codex_new', 'New Codex fixture'); const prepared = deferred(); const copied = []; const opens = [];
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async text => { copied.push(text); } } });
  mockApp({ projects: [project, next] }, async (command, args) => {
    if (command === 'project_handoff') return args.projectId === project.id ? prepared.promise : codexHandoff(next, 'Current handoff');
    assert.equal(command, 'project_open_codex'); opens.push(args); return { projectRoot: project.projectRoot, requested: true };
  });
  await render(h(App)); await openDetails(project.name); await click(codexButton());
  await click(closeButton()); await openDetails(next.name); await click(buttonNamed('复制 AI 接手说明'));
  await act(async () => prepared.resolve(codexHandoff(project, 'Stale handoff')));
  assert.deepEqual(copied, ['Current handoff']); assert.deepEqual(opens, []);
  assert.equal(document.querySelector('[aria-label="给 AI 的完整说明"]').value, 'Current handoff');
  assert.doesNotMatch(dialog().textContent, /Stale handoff|已请求 Codex/);
});

test('A changed instance or project root during a clipboard write invalidates the pending Codex open', async () => {
  for (const change of ['instance', 'root']) {
    await setup(); const project = appProject(`prj_codex_${change}`, `Codex ${change} fixture`); const write = deferred(); const opens = [];
    const state = { projects: [project] };
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async () => write.promise } });
    mockApp(state, async (command, args) => {
      if (command === 'project_handoff') return codexHandoff(project);
      assert.equal(command, 'project_open_codex'); opens.push(args); return { projectRoot: project.projectRoot, requested: true };
    });
    await render(h(App)); await openDetails(project.name); await click(codexButton());
    assert.ok(document.querySelector('[aria-label="给 AI 的完整说明"]'));
    if (change === 'instance') state.instanceId = 'instance_after_migration';
    else state.projects = [{ ...project, projectRoot: '/disposable/migrated/projects/current' }];
    await click(hubTab('备份维护'));
    await click([...document.querySelectorAll('.backup-panel button')].find(button => button.textContent.trim() === '刷新状态'));
    assert.equal(document.querySelector('[aria-label="给 AI 的完整说明"]'), null);
    await act(async () => write.resolve());
    assert.deepEqual(opens, []);
    assert.equal(codexButton().disabled, false);
    await act(async () => root.unmount()); root = null;
  }
});

test('A late Codex launch response cannot replace the new project handoff or report its success there', async () => {
  await setup(); const project = appProject('prj_codex_launch_old', 'Old launch fixture'); const next = appProject('prj_codex_launch_new', 'Current launch fixture'); const opened = deferred();
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async () => {} } });
  mockApp({ projects: [project, next] }, async (command, args) => {
    if (command === 'project_handoff') return codexHandoff(args.projectId === project.id ? project : next, args.projectId === project.id ? 'Old instructions' : 'Current instructions');
    assert.equal(command, 'project_open_codex'); return opened.promise;
  });
  await render(h(App)); await openDetails(project.name); await click(codexButton());
  await click(closeButton()); await openDetails(next.name); await click(buttonNamed('复制 AI 接手说明'));
  await act(async () => opened.resolve({ projectRoot: project.projectRoot, requested: true }));
  assert.equal(document.querySelector('[aria-label="给 AI 的完整说明"]').value, 'Current instructions');
  assert.doesNotMatch(dialog().querySelector('[role="status"]').textContent, /已请求 Codex/);
});

test('A mismatched fresh handoff preserves its text but is never copied or used to open Codex', async () => {
  await setup(); const project = appProject('prj_codex_changed', 'Changed handoff fixture'); const copied = []; const commands = [];
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async text => copied.push(text) } });
  mockApp({ projects: [project] }, async command => {
    commands.push(command); assert.equal(command, 'project_handoff');
    return { text: 'Current service instructions', projectRoot: '/disposable/moved/project', instanceId: 'new_instance' };
  });
  await render(h(App)); await openDetails(project.name); await click(codexButton());
  assert.deepEqual(commands, ['project_handoff']); assert.deepEqual(copied, []);
  assert.equal(document.querySelector('[aria-label="给 AI 的完整说明"]').value, 'Current service instructions');
  assert.match(dialog().querySelector('[role="alert"]').textContent, /项目目录或服务信息已变化/);
  assert.equal(codexButton().disabled, false);
});

test('Automatic storage scans do not hide a failed build or replace the result of its explicit retry', async () => {
  await setup();
  const project = appProject('prj_build_scan', 'Build scan fixture');
  const state = { projects: [project] };
  const build = { taskId: 'task_build_failure', type: 'build', projectId: project.id, status: 'queued', final: false, phase: 'queued' };
  const scan = { taskId: 'task_auto_scan', type: 'storageScan', projectId: project.id, status: 'running', final: false, phase: 'scan' };
  let submissions = 0;
  mockApp(state, async (command, args) => {
    if (command === 'project_build') {
      submissions++;
      return submissions === 1 ? build : { ...build, taskId: 'task_build_retry', status: 'succeeded', final: true, phase: 'completed' };
    }
    assert.equal(command, 'task_get');
    if (args.taskId === build.taskId) {
      project.storage = { activeTask: scan };
      return { ...build, status: 'failed', final: true, error: { code: 'BUILD_FAILED', message: 'Explicit build failure must stay visible' } };
    }
    assert.equal(args.taskId, scan.taskId);
    project.storage = {};
    return { ...scan, status: 'succeeded', final: true, phase: 'completed' };
  });
  await render(h(App)); await openDetails(project.name); await click(buttonNamed('构建项目'));
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 1100)); });
  assert.match(dialog().textContent, /Explicit build failure must stay visible/);
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 1100)); });
  assert.match(dialog().textContent, /Explicit build failure must stay visible/);
  assert.equal(buttonNamed('构建项目').disabled, false);
  await click(buttonNamed('构建项目'));
  assert.equal(submissions, 2);
  assert.doesNotMatch(dialog().textContent, /Explicit build failure must stay visible/);
  assert.match(dialog().querySelector('.project-last-task').textContent, /新网页已发布/);
});

test('App retains a failed delete dialog and closes it only after its accepted task succeeds', async () => {
  await setup(); let submissions = 0;
  const state = { projects: [appProject('prj_A', 'Fixture A')] };
  const task = { taskId: 'task_delete_fixture', type: 'delete', projectId: 'prj_A', status: 'queued', final: false, phase: 'queued' };
  mockApp(state, async (command) => {
    if (command === 'project_delete') {
      submissions++;
      return submissions === 1 ? { ...task, taskId: 'task_failed_delete', status: 'failed', final: true, error: { code: 'FIXTURE_FAILURE', message: 'Delete fixture failed' } } : task;
    }
    assert.equal(command, 'task_get');
    return { ...task, status: 'succeeded', final: true, phase: 'completed' };
  });
  await render(h(App)); await openProjectMenu('Fixture A'); await click(document.querySelector('[aria-label="删除项目「Fixture A」"]'));
  await click(document.querySelector('.project-delete-confirmation input'));
  await click(buttonNamed('确认删除项目'));
  assert.ok(dialog()); assert.match(dialog().textContent, /Delete fixture failed/);
  await click(document.querySelector('.project-delete-heading button'));
  await click(document.querySelector('.project-delete-confirmation input'));
  await click(buttonNamed('确认删除项目'));
  assert.equal(submissions, 2); assert.ok(dialog(), 'Queued acceptance is not final success');
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 1100)); });
  assert.equal(dialog(), null);
  assert.match(document.querySelector('.notice-success').textContent, /项目已移入回收站/);
});

test('App blocks closing a pending create, shows its failure inside the modal, and preserves the name on reopening', async () => {
  await setup(); const creation = deferred(); let submissions = 0;
  mockApp({ projects: [] }, async (command) => { assert.equal(command, 'project_create'); submissions++; return creation.promise; });
  await render(h(App));
  await click([...document.querySelectorAll('.radio-toolbar button')].find((button) => button.textContent.trim() === '新建项目'));
  await inputValue(document.getElementById('project-name'), 'Unsubmitted project draft');
  await submit(document.querySelector('.create-project-form'));
  await key('Escape'); await click(layer());
  assert.ok(dialog()); assert.equal(submissions, 1);
  await act(async () => creation.reject(new Error('Create fixture failed')));
  assert.match(dialog().querySelector('[role="alert"]').textContent, /Create fixture failed/);
  await click(closeButton());
  assert.equal(dialog(), null);
  await click([...document.querySelectorAll('.radio-toolbar button')].find((button) => button.textContent.trim() === '新建项目'));
  assert.equal(document.getElementById('project-name').value, 'Unsubmitted project draft');
  assert.equal(submissions, 1);
});
