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
const { default: App } = await component('App.tsx');
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
    if (command === 'bootstrap_info') return { displayName: 'Local Projects Dev', version: '0.0.0', identifier: 'dev.localprojects.platform.m0', instanceId: state.instanceId ?? 'instance_dom_fixture', workspaceRoot: '/disposable/workspace', dataRoot: '/disposable/development', resources: { sdk: true, template: true, readme: true, cli: true } };
    if (command === 'project_list') return state.projects;
    if (command === 'workspace_info') return { workspaceRoot: '/disposable/workspace', migrationState: 'ready', retainedRoots: [] };
    if (command === 'maintenance_status') return normalStatus;
    if (command === 'project_metadata') return { ...metadata, name: state.projects.find((item) => item.id === args.projectId)?.name ?? 'Removed fixture' };
    if (command === 'backup_list') return [];
    return custom(command, args);
  };
}
const openDetails = (name) => click(document.querySelector(`[aria-label='查看项目「${name}」的详情']`));

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
  await render(h(App)); await openDetails('Fixture A'); await click(buttonNamed('复制给 AI'));
  await click(closeButton()); await openDetails('Fixture B'); await click(buttonNamed('复制给 AI'));
  assert.equal(document.querySelector('[aria-label="给 AI 的完整说明"]').value, 'Current handoff for B');
  await act(async () => first.resolve({ text: 'Stale handoff for A' }));
  assert.equal(document.querySelector('[aria-label="给 AI 的完整说明"]').value, 'Current handoff for B');
  assert.deepEqual(copied, ['Current handoff for B']);
  assert.doesNotMatch(dialog().textContent, /Stale handoff for A|已复制「Fixture A」/);
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
  await render(h(App)); await click(document.querySelector('[aria-label="删除项目「Fixture A」"]'));
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
  await click([...document.querySelectorAll('.heading-actions button')].find((button) => button.textContent.trim() === '新建项目'));
  await inputValue(document.getElementById('project-name'), 'Unsubmitted project draft');
  await submit(document.querySelector('.create-project-form'));
  await key('Escape'); await click(layer());
  assert.ok(dialog()); assert.equal(submissions, 1);
  await act(async () => creation.reject(new Error('Create fixture failed')));
  assert.match(dialog().querySelector('[role="alert"]').textContent, /Create fixture failed/);
  await click(closeButton());
  assert.equal(dialog(), null);
  await click([...document.querySelectorAll('.heading-actions button')].find((button) => button.textContent.trim() === '新建项目'));
  assert.equal(document.getElementById('project-name').value, 'Unsubmitted project draft');
  assert.equal(submissions, 1);
});
