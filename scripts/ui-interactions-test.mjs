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
// These unrelated interaction fixtures have no interrupted Git additions.
await writeFile(mockPath, 'export const invoke = (command, ...args) => ["source_import_recover", "node_app_delete_tasks"].includes(command) ? Promise.resolve([]) : command === "project_delete_preview" ? globalThis.__UI_DELETE_PREVIEW__(...args) : globalThis.__UI_TEST_INVOKE__(command, ...args); export const isTauri = () => true; export const getAllWindows = async () => globalThis.__UI_WINDOWS__ ?? []; export const listen = async (name, callback) => { const listeners = globalThis.__UI_EVENT_LISTENERS__ ??= new Map(); listeners.set(name, callback); return () => listeners.delete(name); };\n');
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
    source = source.replaceAll(/(['"])@tauri-apps\/api\/(?:core|event|window)\1/g, JSON.stringify(pathToFileURL(mockPath).href));
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
const { ProjectOverview } = await component('ProjectOverview.tsx');
const { WorkspaceStoragePanel } = await component('WorkspaceStoragePanel.tsx');
const { WorkspaceLocation } = await component('WorkspaceLocation.tsx');
const { FrontendPlatformProvider, frontendPlatformFromBootstrap } = await component('frontend-platform.tsx');
const { default: App } = await component('App.tsx');
const { resetProjectDeletionsForTests, acceptProjectDeletion, observeDeletionRuntime, projectWasDeleted } = await component('project-deletion.ts');
const { ProjectDeleteDialog } = await component('ProjectDeleteDialog.tsx');
const { ProjectTask } = await component('ProjectTask.tsx');
let root;
let render;
let previewState = null;
const previewContexts = new Map();
function previewFixture(value, status = normalStatus, instanceId = 'preview-fixture') {
  return { projectId: value.id, instanceId, projectRoot: value.projectRoot, expectedDocumentEpoch: value.documentEpoch || '22222222-2222-4222-8222-222222222222', mode: status?.mode ?? 'ready', maintenanceId: status?.maintenanceId ?? null, ownership: value.ownership ?? 'platform', sourcePreserved: value.ownership === 'user', deletePaths: [...(value.ownership === 'user' ? [] : [value.projectRoot]), `/disposable/owned-material/${value.id}`], alreadyAbsent: false, cleanupOnly: false };
}
async function setup() {
  previewState = null; previewContexts.clear(); globalThis.__UI_WINDOWS__ = [];
  globalThis.__UI_DELETE_PREVIEW__ = async ({ projectId }) => {
    const context = previewContexts.get(projectId);
    if (context) return previewFixture(context.project, context.status, context.instanceId);
    const current = previewState?.projects.find(value => value.id === projectId);
    if (current) return previewFixture(current, normalStatus, previewState.instanceId ?? 'instance_dom_fixture');
    throw new Error('No reviewed fixture identity');
  };
  document.body.innerHTML = '<button id="trigger">Open details</button><aside id="already-inert" inert></aside><div id="root"></div>';
  document.body.style.overflow = 'auto';
  document.getElementById('trigger').focus();
  root = createRoot(document.getElementById('root'));
  render = async (tree) => { if (tree?.props?.project) previewContexts.set(tree.props.project.id, tree.props); await act(async () => { root.render(tree); }); };
}
async function refreshAutomatically() {
  await act(async () => { window.dispatchEvent(new window.Event('focus')); });
}
async function openMore() {
  const more = document.querySelector('.application-detail-more');
  assert.ok(more);
  await act(async () => { more.open = true; more.dispatchEvent(new window.Event('toggle')); });
  return more;
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
function deletionProgress(stage) {
  const progress = dialog().querySelector('.delete-progress[role="status"]');
  assert.ok(progress, 'Pending deletion needs a visible status');
  assert.equal(progress.closest('[hidden]'), null);
  assert.equal(progress.getAttribute('aria-live'), 'polite');
  assert.equal(progress.getAttribute('aria-atomic'), 'true');
  assert.equal(progress.closest('[aria-busy="true"]'), null, 'The live status remains outside busy review content');
  assert.equal(progress.querySelector('.delete-progress-spinner')?.getAttribute('aria-hidden'), 'true');
  assert.ok(progress.textContent.includes(stage), `Missing deletion stage: ${stage}`);
  assert.equal(document.querySelector('.project-delete-review')?.hidden ?? true, true);
  assert.doesNotMatch(progress.textContent, /目标已变化|重新核对后再删除/);
  return progress;
}
afterEach(async () => {
  if (root) await act(async () => root.unmount());
  root = null;
  resetProjectDeletionsForTests();
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

const project = { runtime: 'web', ownership: 'platform', id: 'prj_ui_test', projectRoot: '/disposable/workspace/projects/prj_ui_test', name: 'Disposable DOM fixture', documentEpoch: '11111111-1111-4111-8111-111111111111', state: 'ready', issues: [] };

test('Web deletion distinguishes retained file data from browser data that is always cleared', async () => {
  for (const ownership of ['user', 'platform']) {
    await setup();
    const value = { ...project, ownership };
    await render(h(ProjectDeleteDialog, { project: value, status: normalStatus, disabled: false, onTask() {}, onClose() {} }));
    const scopes = document.querySelectorAll('.project-delete-scope');
    assert.match(scopes[0].textContent, /浏览器数据：localStorage、IndexedDB、Cookie 等一律清除/);
    assert.equal([...scopes[0].querySelectorAll('code')].some(element => element.textContent === value.projectRoot), ownership === 'platform');
    if (ownership === 'user') assert.match(scopes[1].textContent, /项目文件夹、源码、Git 和 data\/ 下的文件数据保留/);
    else assert.match(scopes[0].textContent, /整个项目文件夹.*data\/ 下的数据库及附件.*一起删除/);
    await act(async () => root.unmount()); root = null;
  }
});
const normalStatus = { mode: 'ready', readyForChanges: false, recoveryRequired: false };

test('Web agent details and deletion reminder use the saved marker when the folder is missing', async () => {
  for (const isAgent of [true, false]) {
    await setup();
    const value = { ...project, isAgent, state: 'missing', manifest: null };
    await render(h(ProjectOverview, { project: value, disabled: false, onTask() {}, onChanged: async () => {}, onClose() {} }));
    await openMore();
    assert.equal(document.querySelector('.agent-app-badge')?.textContent === 'agent 应用', isAgent);
    await render(h(ProjectDeleteDialog, { project: value, status: normalStatus, disabled: false, onTask() {}, onClose() {} }));
    assert.equal(dialog().textContent.includes('如果你在 AI 工具里为它设置过定时任务'), isAgent);
    await act(async () => root.unmount()); root = null;
  }
});

async function inputValue(input, value) {
  assert.ok(input);
  await act(async () => {
    const prototype = input instanceof window.HTMLTextAreaElement ? window.HTMLTextAreaElement.prototype : window.HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(prototype, 'value').set.call(input, value);
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

const clearRecovery = { state: 'clear', operations: [], issues: [] };
const emptyProjectStorage = { currentProject: { status: 'not_scanned', complete: false, stale: false, categories: {}, totalBytes: null }, activeTask: null };
const emptyStorageFiles = { files: [], nextCursor: null, complete: true, issues: [] };
const authorManifest = { manifestVersion: 1, runtime: 'web', app: { name: project.name }, web: { root: 'dist', entry: 'index.html' }, platform: { sdkApiVersion: 1 }, development: { build: { kind: 'npm', script: 'build' } }, documentation: { readme: 'README.md' } };
function overviewProps(extra = {}) {
  return { project: { ...project, projectRoot: '/disposable/dom-fixture' }, status: normalStatus, disabled: false, onTask() {}, onChanged: async () => {}, onHandoff() {}, onClose() {}, ...extra };
}
const buttonNamed = (name) => [...document.querySelectorAll('button')].find((button) => button.textContent.trim() === name);

function overviewGitStatus(extra = {}) {
  return { repository: { state: 'project-root', root: '/disposable/dom-fixture', markers: [], head: { oid: 'a'.repeat(40), branch: 'refs/heads/main' }, indexState: 'matched', git: { path: '/disposable/git', version: '2.56.0' } }, scope: { policy: 'project-source-v1', includedRoots: ['app'], includedPaths: ['app/src/App.tsx'], excluded: [], unclassifiedPaths: [], protectedTrackedPaths: [], complete: true }, changes: [{ path: 'app/src/App.tsx', kind: 'modify', staged: false }], snapshotDigest: 'b'.repeat(64), canInitialize: false, canSave: true, canListVersions: true, canCreateBranch: false, branchCreationIssue: null, database: { state: 'available', canSave: true, reason: 'Disposable data' }, issues: [], ...extra };
}
async function openOverviewVersions() {
  const details = [...document.querySelectorAll('details')].find(element => element.querySelector('summary')?.textContent === '源码与数据版本');
  assert.ok(details);
  await act(async () => { details.open = true; details.dispatchEvent(new window.Event('toggle')); });
  return details;
}

test('Permanent deletion never submits when the read-only identity preview cannot be verified', async () => {
  await setup(); const calls = [];
  globalThis.__UI_DELETE_PREVIEW__ = async () => { throw new Error('Disposable ownership cannot be verified'); };
  globalThis.__UI_TEST_INVOKE__ = async (...args) => { calls.push(args); throw new Error('A missing identity must not submit'); };
  await render(h(ProjectDeleteDialog, { project: { ...project, documentEpoch: null }, status: undefined, onTask() {}, onClose() {}, disabled: false }));
  assert.match(dialog().textContent, /无法核对删除范围.*ownership cannot be verified/);
  assert.equal(document.querySelector('.project-delete-confirmation input').disabled, true);
  await click(buttonNamed('确认永久删除项目'));
  assert.deepEqual(calls, []);
});

test('A damaged project without a business epoch can delete using its private identity preview', async () => {
  await setup(); const calls = []; const expectedEpoch = '22222222-2222-4222-8222-222222222222';
  globalThis.__UI_TEST_INVOKE__ = async (command, args) => { calls.push([command, args]); return { taskId: 'task_damaged_preview', projectId: project.id, type: 'delete', status: 'queued', final: false, phase: 'queued' }; };
  await render(h(ProjectDeleteDialog, { project: { ...project, documentEpoch: null }, status: undefined, onTask() {}, onClose() {}, disabled: false }));
  assert.equal(document.querySelector('.project-delete-confirmation input').disabled, false);
  await click(document.querySelector('.project-delete-confirmation input')); await click(buttonNamed('确认永久删除项目'));
  assert.deepEqual(calls, [['project_delete', { projectId: project.id, input: { expectedDocumentEpoch: expectedEpoch } }]]);
});

test('Deletion preview rejects a mismatched root or service and never sends filesystem paths', async () => {
  for (const changed of [{ projectRoot: '/disposable/foreign-project' }, { instanceId: 'foreign-service' }, { alreadyAbsent: true }]) {
    await setup(); const writes = [];
    globalThis.__UI_DELETE_PREVIEW__ = async args => { assert.deepEqual(args, { projectId: project.id }); return { ...previewFixture(project, normalStatus, 'reviewed-service'), ...changed }; };
    globalThis.__UI_TEST_INVOKE__ = async (...args) => { writes.push(args); throw new Error('A mismatched preview cannot authorize a write'); };
    await render(h(ProjectDeleteDialog, { project, status: normalStatus, instanceId: 'reviewed-service', disabled: false, onTask() {}, onClose() {} }));
    assert.match(dialog().textContent, /删除范围或项目身份不符/);
    assert.equal(buttonNamed('确认永久删除项目').disabled, true);
    assert.deepEqual(writes, []);
    await act(async () => root.unmount()); root = null;
  }
});

test('A late read-only deletion preview cannot authorize the next selected project', async () => {
  await setup(); const old = deferred(); const next = { ...project, id: 'prj_next_preview', projectRoot: '/disposable/next-preview', documentEpoch: '33333333-3333-4333-8333-333333333333' };
  globalThis.__UI_DELETE_PREVIEW__ = async ({ projectId }) => projectId === project.id ? old.promise : previewFixture(next);
  const props = { project, disabled: false, onTask() {}, onClose() {} };
  await render(h(ProjectDeleteDialog, props));
  assert.match(dialog().textContent, /正在核对删除范围/);
  assert.equal(buttonNamed('取消，保留项目').disabled, false);
  await render(h(ProjectDeleteDialog, { ...props, project: next }));
  await act(async () => old.resolve(previewFixture(project)));
  assert.ok(dialog().textContent.includes(next.projectRoot));
  assert.equal(document.querySelector('.project-delete-confirmation input').disabled, false);
  assert.equal(document.querySelector('.project-delete-confirmation input').checked, false);
});

test('A cleanup-only deletion preview permits finalization without inventing an epoch', async () => {
  await setup(); const writes = [];
  globalThis.__UI_DELETE_PREVIEW__ = async () => ({ ...previewFixture(project), projectRoot: null, expectedDocumentEpoch: null, cleanupOnly: true, deletePaths: ['/disposable/remaining-owned-task'] });
  globalThis.__UI_TEST_INVOKE__ = async (command, args) => { writes.push([command, args]); return { taskId: 'task_cleanup_only', projectId: project.id, type: 'delete', status: 'queued', final: false, phase: 'queued' }; };
  await render(h(ProjectDeleteDialog, { project: { ...project, documentEpoch: null }, disabled: false, onTask() {}, onClose() {} }));
  assert.match(dialog().textContent, /只清除已核实归属的剩余材料/);
  await click(document.querySelector('.project-delete-confirmation input')); await click(buttonNamed('确认永久删除项目'));
  assert.deepEqual(writes, [['project_delete', { projectId: project.id, input: {} }]]);
});

test('Deletion acknowledgment is revoked for changed ID, epoch, root or service and submits only the current identity', async () => {
  await setup(); const calls = [];
  let props = { project: { ...project, projectRoot: '/disposable/reviewed-project' }, instanceId: 'old-instance', status: normalStatus, onTask() {}, onClose() {}, disabled: false };
  globalThis.__UI_TEST_INVOKE__ = async (command, args) => {
    calls.push([command, args]);
    return { taskId: 'task_identity_checked', type: 'delete', projectId: args.projectId, status: 'failed', final: true, phase: 'failed', result: { operationOutcome: 'not_deleted' } };
  };
  await render(h(ProjectDeleteDialog, props));
  for (const change of [
    { project: { ...props.project, documentEpoch: '22222222-2222-4222-8222-222222222222' } },
    { project: { ...props.project, documentEpoch: '22222222-2222-4222-8222-222222222222', projectRoot: '/disposable/new-reviewed-project' } },
    { instanceId: 'new-instance' },
    { project: { ...props.project, id: 'prj_new_reviewed_identity', documentEpoch: '33333333-3333-4333-8333-333333333333', projectRoot: '/disposable/other-reviewed-project' } },
  ]) {
    await click(document.querySelector('.project-delete-confirmation input'));
    assert.equal(buttonNamed('确认永久删除项目').disabled, false);
    props = { ...props, ...change }; await render(h(ProjectDeleteDialog, props));
    assert.deepEqual(calls, []);
    assert.equal(document.querySelector('.project-delete-confirmation input').checked, false);
    assert.equal(buttonNamed('确认永久删除项目').disabled, true);
  }
  await click(document.querySelector('.project-delete-confirmation input')); await click(buttonNamed('确认永久删除项目'));
  assert.deepEqual(calls, [['project_delete', { projectId: props.project.id, input: { expectedDocumentEpoch: props.project.documentEpoch } }]]);
});

test('A late deletion acceptance cannot replace a changed project identity or show its failure there', async () => {
  await setup(); const accepting = deferred(); const tasks = []; const calls = [];
  const reviewed = { ...project, projectRoot: '/disposable/old-deletion-target' };
  const props = { project: reviewed, instanceId: 'old-delete-service', status: normalStatus, disabled: false, onTask: task => tasks.push(task), onClose() {} };
  globalThis.__UI_TEST_INVOKE__ = async (command, args) => { calls.push([command, args]); return accepting.promise; };
  await render(h(ProjectDeleteDialog, props));
  await click(document.querySelector('.project-delete-confirmation input')); await click(buttonNamed('确认永久删除项目'));
  const current = { ...reviewed, projectRoot: '/disposable/new-deletion-target', documentEpoch: '44444444-4444-4444-8444-444444444444' };
  await render(h(ProjectDeleteDialog, { ...props, project: current, instanceId: 'new-delete-service' }));
  assert.equal(document.querySelector('.project-delete-confirmation input').checked, false);
  assert.ok(dialog().textContent.includes(current.projectRoot));
  await act(async () => accepting.resolve({ taskId: 'task_stale_delete_acceptance', projectId: reviewed.id, type: 'delete', status: 'failed', final: true, error: { code: 'OLD_DELETE_FAILURE', message: 'Old deletion failure must stay with its original scope' } }));
  assert.deepEqual(calls, [['project_delete', { projectId: reviewed.id, input: { expectedDocumentEpoch: reviewed.documentEpoch } }]]);
  assert.deepEqual(tasks, []);
  assert.doesNotMatch(dialog().textContent, /Old deletion failure/);
  assert.equal(buttonNamed('确认永久删除项目').disabled, true);
});

test('The deletion dialog keeps status gates and submits the currently reviewed maintenance identity', async () => {
  await setup(); const calls = [];
  let props = { project: { ...project, projectRoot: '/disposable/maintenance-deletion-target' }, status: normalStatus, disabled: false, onTask() {}, onClose() {} };
  globalThis.__UI_TEST_INVOKE__ = async (command, args) => {
    calls.push([command, args]);
    return { taskId: 'task_maintenance_delete', projectId: project.id, type: 'delete', status: 'failed', final: true, result: { operationOutcome: 'not_deleted' } };
  };
  for (const gate of [
    { disabled: true, status: normalStatus },
    { status: { ...normalStatus, activeTask: 'task_other_project_operation' } },
  ]) {
    props = { ...props, ...gate }; await render(h(ProjectDeleteDialog, props));
    assert.equal(document.querySelector('.project-delete-confirmation input').disabled, true);
    assert.equal(buttonNamed('确认永久删除项目').disabled, true);
    await click(buttonNamed('确认永久删除项目'));
  }
  assert.deepEqual(calls, []);
  for (const temporaryStatus of [
    { ...normalStatus, activeTask: 'task_temporary_project_operation' },
  ]) {
    props = { ...props, disabled: false, status: normalStatus };
    await render(h(ProjectDeleteDialog, props));
    await click(document.querySelector('.project-delete-confirmation input'));
    assert.equal(buttonNamed('确认永久删除项目').disabled, false);
    props = { ...props, status: temporaryStatus }; await render(h(ProjectDeleteDialog, props));
    assert.equal(document.querySelector('.project-delete-confirmation input').checked, false);
    assert.equal(document.querySelector('.project-delete-confirmation input').disabled, true);
    props = { ...props, status: normalStatus }; await render(h(ProjectDeleteDialog, props));
    assert.equal(document.querySelector('.project-delete-confirmation input').checked, false);
    assert.equal(document.querySelector('.project-delete-confirmation input').disabled, false);
    assert.equal(buttonNamed('确认永久删除项目').disabled, true);
    await click(buttonNamed('确认永久删除项目'));
  }
  assert.deepEqual(calls, []);
  props = { ...props, status: { ...normalStatus, mode: 'maintenance', maintenanceId: 'maintenance_old_confirmation' } };
  await render(h(ProjectDeleteDialog, props)); await click(document.querySelector('.project-delete-confirmation input'));
  props = { ...props, status: { ...props.status, maintenanceId: 'maintenance_current_confirmation' } };
  await render(h(ProjectDeleteDialog, props));
  assert.equal(document.querySelector('.project-delete-confirmation input').checked, false);
  await click(document.querySelector('.project-delete-confirmation input')); await click(buttonNamed('确认永久删除项目'));
  assert.deepEqual(calls, [['project_delete', { projectId: project.id, input: { expectedDocumentEpoch: project.documentEpoch, maintenanceId: 'maintenance_current_confirmation' } }]]);
});

test('Static recovery evidence is included in permanent deletion without requiring recovery first', async () => {
  await setup(); const calls = [];
  globalThis.__UI_TEST_INVOKE__ = async (command, args) => { calls.push([command, args]); return { taskId: 'task_delete_recovery', projectId: project.id, type: 'delete', status: 'queued', final: false, phase: 'queued' }; };
  await render(h(ProjectDeleteDialog, { project, status: { ...normalStatus, recoveryRequired: true }, disabled: false, onTask() {}, onClose() {} }));
  assert.match(dialog().textContent, /当前未完成操作会先停止/);
  assert.equal(document.querySelector('.project-delete-confirmation input').disabled, false);
  await click(document.querySelector('.project-delete-confirmation input')); await click(buttonNamed('确认永久删除项目'));
  assert.deepEqual(calls, [['project_delete', { projectId: project.id, input: { expectedDocumentEpoch: project.documentEpoch } }]]);
});

test('An accepted deletion keeps its closing lock across name and maintenance refreshes until its final task', async () => {
  await setup(); let closes = 0; const calls = []; const tasks = [];
  const accepted = { taskId: 'task_refresh_locked_delete', projectId: project.id, type: 'delete', status: 'queued', final: false, phase: 'queued' };
  let props = { project: { ...project, projectRoot: '/disposable/refresh-locked-deletion' }, status: normalStatus, disabled: false, instanceId: 'same-delete-service', onTask: task => tasks.push(task), onClose() { closes++; } };
  globalThis.__UI_TEST_INVOKE__ = async (command, args) => { calls.push([command, args]); return accepted; };
  await render(h(ProjectDeleteDialog, props));
  await click(document.querySelector('.project-delete-confirmation input')); await click(buttonNamed('确认永久删除项目'));
  for (const change of [
    { project: { ...props.project, name: 'Refreshed deletion display name' } },
    { status: { ...normalStatus, mode: 'maintenance', maintenanceId: 'maintenance_refreshed_one' } },
    { status: { ...normalStatus, mode: 'maintenance', maintenanceId: 'maintenance_refreshed_two' } },
    { status: normalStatus },
  ]) {
    props = { ...props, ...change, task: accepted }; await render(h(ProjectDeleteDialog, props));
    deletionProgress('删除任务已受理，正在等待执行');
    assert.equal(closeButton().disabled, true);
    assert.equal(buttonNamed('取消，保留项目').disabled, true);
    assert.equal(document.querySelector('.project-delete-confirmation input').disabled, true);
    await key('Escape'); await click(layer()); await click(closeButton()); await click(buttonNamed('取消，保留项目'));
    assert.equal(closes, 0); assert.equal(calls.length, 1); assert.deepEqual(tasks, [accepted]);
  }
  await render(h(ProjectDeleteDialog, { ...props, task: { ...accepted, status: 'failed', final: true, phase: 'failed', result: { operationOutcome: 'not_deleted' } } }));
  assert.equal(dialog().querySelector('.delete-progress'), null);
  assert.equal(document.querySelector('.project-delete-review').hidden, false);
  assert.equal(buttonNamed('取消，保留项目').disabled, false);
  assert.equal(document.querySelector('.project-delete-confirmation input').checked, false);
  await click(buttonNamed('取消，保留项目')); assert.equal(closes, 1);
});

test('Deletion loading follows task phases and ends for failed, interrupted and unconfirmed final results', async () => {
  await setup();
  const finals = [
    { status: 'failed', final: true, phase: 'failed', result: { operationOutcome: 'not_deleted' }, error: { code: 'DISPOSABLE_DELETE_FAILURE', message: 'Disposable deletion failed' } },
    { status: 'interrupted', final: true, phase: 'interrupted', result: { operationOutcome: 'unknown' }, error: { code: 'DISPOSABLE_DELETE_INTERRUPTED', message: 'Disposable deletion interrupted' } },
    { status: 'succeeded', final: true, phase: 'completed', result: null },
  ];
  for (const [index, final] of finals.entries()) {
    const accepted = { taskId: `task_loading_phases_${index}`, projectId: project.id, type: 'delete', status: 'queued', final: false, phase: 'queued' };
    const props = { key: index, project: { ...project, projectRoot: '/disposable/loading-phases' }, status: normalStatus, disabled: false, onTask() {}, onClose() {} };
    globalThis.__UI_TEST_INVOKE__ = async command => { assert.equal(command, 'project_delete'); return accepted; };
    await render(h(ProjectDeleteDialog, props));
    await click(document.querySelector('.project-delete-confirmation input')); await click(buttonNamed('确认永久删除项目'));
    deletionProgress('删除任务已受理，正在等待执行');
    for (const [phase, stage] of [
      ['delete-freeze', '正在停止项目写入并关闭数据库'],
      ['delete-prepared', '正在核对项目身份和删除范围'],
      ['delete-intent', '正在永久删除项目文件'],
      ['unrecognized-phase', '正在等待删除任务的最终结果'],
    ]) {
      await render(h(ProjectDeleteDialog, { ...props, disabled: true, task: { ...accepted, status: 'running', phase } }));
      deletionProgress(stage);
      assert.equal(buttonNamed('正在永久删除…').disabled, true);
      assert.equal(document.querySelector('.project-delete-confirmation input').disabled, true);
      assert.equal(closeButton().disabled, true);
    }
    await render(h(ProjectDeleteDialog, { ...props, task: { ...accepted, ...final } }));
    assert.equal(dialog().querySelector('.delete-progress'), null);
    assert.equal(document.querySelector('.project-delete-review').hidden, false);
    assert.ok(dialog().textContent.includes(props.project.projectRoot));
    if (final.error) assert.ok(dialog().textContent.includes(final.error.message));
    else assert.match(dialog().textContent, /永久删除结果尚不能确认/);
    assert.equal(document.querySelector('.project-delete-confirmation input').checked, false);
    assert.equal(buttonNamed('确认永久删除项目').disabled, true);
    assert.equal(buttonNamed('取消，保留项目').disabled, false);
  }
});

test('A rejected deletion submission ends loading and restores its reviewed scope with the error', async () => {
  await setup(); const accepting = deferred(); let submissions = 0;
  globalThis.__UI_TEST_INVOKE__ = async command => { assert.equal(command, 'project_delete'); submissions++; return accepting.promise; };
  await render(h(ProjectDeleteDialog, { project: { ...project, projectRoot: '/disposable/rejected-deletion' }, status: normalStatus, disabled: false, onTask() {}, onClose() {} }));
  await click(document.querySelector('.project-delete-confirmation input')); await click(buttonNamed('确认永久删除项目'));
  deletionProgress('正在提交删除请求');
  await act(async () => accepting.reject(new Error('Disposable submission failed')));
  assert.equal(dialog().querySelector('.delete-progress'), null);
  assert.equal(document.querySelector('.project-delete-review').hidden, false);
  assert.match(dialog().querySelector('[role=alert]').textContent, /Disposable submission failed/);
  assert.match(dialog().textContent, /\/disposable\/rejected-deletion/);
  assert.equal(document.querySelector('.project-delete-confirmation input').checked, false);
  assert.equal(buttonNamed('确认永久删除项目').disabled, true);
  assert.equal(buttonNamed('取消，保留项目').disabled, false);
  assert.equal(submissions, 1);
});

function storagePanelProps(extra = {}) {
  return { workspace: { workspaceRoot: '/disposable/workspace', migrationState: 'ready', retainedRoots: [] }, revision: 1, enabled: true, active: true, migrating: false, cleanupBusy: false, projects: [], nodeApps: [], onTask() {}, onProjectTask() {}, onRefresh: async () => {}, ...extra };
}
const workspaceStorage = { platformProgram: { totalBytes: null }, workspace: { totalBytes: null, projects: [] }, otherLocalLocations: [] };

test('Application storage sums only current application rows and reports unknown measurements', async () => {
  await setup();
  const data = { ...workspaceStorage, knownLocalTotal: { totalBytes: 999999 }, workspace: { totalBytes: 3000, projects: [{ scope: { projectId: 'prj_space_a' }, totalBytes: 2500, complete: true }] }, nodeApps: [{ scope: { appId: 'node_space_fixture' }, totalBytes: 700, complete: true }], disks: [{ exampleRoot: '/disposable/workspace', availableBytes: 5000 }] };
  globalThis.__UI_TEST_INVOKE__ = async command => { assert.equal(command, 'workspace_storage'); return data; };
  await render(h(WorkspaceStoragePanel, storagePanelProps({ projects: [{ ...project, id: 'prj_space_a', name: 'Space A' }, { ...project, id: 'prj_unknown', name: 'Unknown' }], nodeApps: [{ id: 'node_space_fixture', name: 'Node space fixture' }] })));
  assert.match(document.querySelector('.space-totals').textContent, /应用共占用至少 3\.2 KB.*部分统计.*磁盘剩余5\.0 KB/);
  assert.doesNotMatch(document.querySelector('.workspace-storage-panel').textContent, /软件本身与记录|999\.9/);
  assert.deepEqual([...document.querySelectorAll('.storage-application-row .space-project-name strong')].map(item => item.textContent), ['Space A', 'Node space fixture', 'Unknown']);
  assert.match(document.querySelectorAll('.storage-application-row')[2].textContent, /尚未统计/);
});

test('Application occupancy defaults to five rows and expands the size-sorted list', async () => {
  await setup();
  const projects = Array.from({ length: 7 }, (_, index) => ({ ...project, id: `space_${index}`, name: `App ${index}` }));
  const data = { ...workspaceStorage, workspace: { projects: projects.map((value, index) => ({ scope: { projectId: value.id }, totalBytes: 100 * (index + 1), complete: true })) } };
  globalThis.__UI_TEST_INVOKE__ = async command => { assert.equal(command, 'workspace_storage'); return data; };
  await render(h(WorkspaceStoragePanel, storagePanelProps({ projects })));
  assert.equal(document.querySelectorAll('.storage-application-row').length, 5);
  assert.equal(document.querySelector('.storage-application-row strong').textContent, 'App 6');
  await click(buttonNamed('查看全部 7 个'));
  assert.equal(document.querySelectorAll('.storage-application-row').length, 7);
  assert.equal(document.querySelector('input[type="search"]'), null);
});

function cleanupPlan(extra = {}) {
  return { planId: 'cleanup_plan_dom', expiresAt: Date.now() + 60000, workspaceRoot: '/disposable/workspace', confirmationText: '永久清理所选内容', preserved: ['当前项目'], items: [
    { id: 'restore_fixture', kind: 'restoreOriginal', label: 'Disposable restore original', path: '/disposable/restore-original', totalBytes: 1234, fileCount: 4, canClean: true, blockedReasons: [] },
    { id: 'restore_protected', kind: 'restoreOriginal', label: 'Protected restore original fixture', path: '/disposable/older-restore-original', totalBytes: null, fileCount: null, canClean: false, blockedReasons: ['Unresolved recovery still references this copy'] },
  ], ...extra };
}
const cleanupPreview = (plan) => ({ taskId: `task_preview_${plan.planId}`, type: 'workspaceCleanupPreview', projectId: null, final: true, status: 'succeeded', phase: 'completed', result: plan });
const cleanupProps = (extra = {}) => ({ workspaceRoot: '/disposable/workspace', revision: 1, enabled: true, active: true, migrating: false, onTask() {}, onRefresh: async () => {}, ...extra });
const cleanupConfirmation = () => document.querySelector('.history-cleanup-dialog');
const confirmCleanupButton = () => [...cleanupConfirmation().querySelectorAll('button')].find((button) => button.textContent.startsWith('永久清理'));
async function acknowledgeCleanup() {
  await click(buttonNamed('清理所选…'));
  await click(cleanupConfirmation().querySelector('input[type="checkbox"]'));
}

test('A failed migration submission still settles after leaving and returning to its settings section', async () => {
  await setup(); const result = deferred(); let submissions = 0;
  globalThis.__UI_TEST_INVOKE__ = async (command) => {
    assert.equal(command, 'workspace_migrate'); submissions++; return result.promise;
  };
  const props = { workspace: { workspaceRoot: '/disposable/workspace', migrationState: 'ready', retainedRoots: ['/disposable/older'] }, dataRoot: '/disposable/development', enabled: true, active: true, migrating: false, onTask() {}, onRefresh: async () => {}, onOpenHistory() {} };
  await render(h(WorkspaceLocation, props));
  assert.equal(document.querySelector('.settings-history-notice'), null);
  assert.equal(buttonNamed('去清理'), undefined);
  assert.equal(buttonNamed('重试删除'), undefined);
  await click(buttonNamed('更换位置'));
  await inputValue(document.querySelector('.workspace-migration-form input:not([type="checkbox"])'), '/disposable/new-workspace');
  await submit(document.querySelector('.workspace-migration-form form'));
  await click(document.querySelector('.workspace-migration-confirmation input[type="checkbox"]'));
  await click(buttonNamed('确认开始迁移'));
  assert.equal(submissions, 1);
  await render(h(WorkspaceLocation, { ...props, active: false }));
  await act(async () => result.reject(new Error('Migration fixture rejected')));
  await render(h(WorkspaceLocation, props));
  assert.equal(document.querySelector('.workspace-migration-form input:not([type="checkbox"])').disabled, false);
  assert.match(document.querySelector('.workspace-location-card [role="alert"]').textContent, /Migration fixture rejected/);
  assert.equal(document.querySelector('.workspace-migration-confirmation'), null);
  assert.equal(submissions, 1);
});

const migrationCleanupWorkspace = (extra = {}) => ({
  workspaceRoot: '/disposable/new-workspace', migrationState: 'ready', activeTask: null,
  retainedRoots: [{ taskId: 'task_disposable_migration', path: '/disposable/old-workspace', sourceCleanup: {
    phase: 'pending', cleanupPending: true, completed: false, sourcePath: '/disposable/old-workspace',
    error: { code: 'WORKSPACE_SOURCE_CLEANUP_PENDING', message: 'Disposable removal permission failure' },
  } }], ...extra,
});
const migrationCleanupProps = (workspace, extra = {}) => ({ workspace, dataRoot: '/disposable/development', enabled: true, active: true, migrating: false, onTask() {}, onRefresh: async () => {}, ...extra });
const migrationCleanupTask = (extra = {}) => ({ taskId: 'task_disposable_migration_cleanup', projectId: null, type: 'workspaceMigrationCleanup', status: 'running', final: false, phase: 'cleaning-old-workspace', result: null, error: null, ...extra });

test('Windows removes only new migration and preserves current location, refresh and retained-record cleanup', async () => {
  await setup(); const calls = []; const tracked = []; let refreshes = 0;
  const current = String.raw`C:\Disposable current workspace`;
  const workspace = migrationCleanupWorkspace({ workspaceRoot: current });
  globalThis.__UI_TEST_INVOKE__ = async (command, args) => {
    calls.push({ command, args });
    if (command === 'open_project_folder') return null;
    assert.equal(command, 'workspace_migration_cleanup');
    return migrationCleanupTask({ final: true, status: 'succeeded', phase: 'completed', result: { completed: true, cleanupPending: false } });
  };
  const props = migrationCleanupProps(workspace, { onTask: value => tracked.push(value), onRefresh: async () => { refreshes++; } });
  const platform = frontendPlatformFromBootstrap({ build: { target: 'x86_64-pc-windows-msvc' } });
  await render(h(FrontendPlatformProvider, { value: platform }, h(WorkspaceLocation, props)));
  assert.equal(buttonNamed('更换位置'), undefined);
  assert.equal(document.querySelector('.settings-migration'), null);
  assert.match(document.querySelector('.workspace-location-card').textContent, /Disposable current workspace/);
  await click(buttonNamed('在资源管理器中打开'));
  await click(buttonNamed('刷新状态'));
  assert.deepEqual(calls, [{ command: 'open_project_folder', args: { path: current } }]);
  assert.equal(refreshes, 1);
  await click(buttonNamed('重试删除'));
  assert.deepEqual(calls.at(-1), { command: 'workspace_migration_cleanup', args: { migrationId: 'task_disposable_migration' } });
  assert.equal(tracked.at(-1).final, true);
  assert.equal(refreshes, 2);
  assert.equal(calls.some(value => value.command === 'workspace_migrate' || value.command === 'choose_path'), false);
});

test('Windows bootstrap arrival hides an open migration form and discards an earlier path-dialog reply', async () => {
  await setup(); const chosen = deferred(); const calls = [];
  globalThis.__UI_TEST_INVOKE__ = async command => { calls.push(command); assert.equal(command, 'choose_path'); return chosen.promise; };
  const props = migrationCleanupProps(migrationCleanupWorkspace({ retainedRoots: [] }));
  await render(h(FrontendPlatformProvider, { value: frontendPlatformFromBootstrap(null) }, h(WorkspaceLocation, props)));
  await click(buttonNamed('更换位置'));
  await click(buttonNamed('选择…'));
  const platform = frontendPlatformFromBootstrap({ build: { target: 'x86_64-pc-windows-msvc' } });
  await render(h(FrontendPlatformProvider, { value: platform }, h(WorkspaceLocation, props)));
  await act(async () => chosen.resolve(String.raw`D:\Disposable new location`));
  assert.equal(buttonNamed('更换位置'), undefined);
  assert.equal(document.querySelector('.settings-migration'), null);
  assert.equal(buttonNamed('确认开始迁移'), undefined);
  assert.deepEqual(calls, ['choose_path']);
});

test('Migration explains automatic deletion only after the new location reopens', async () => {
  await setup(); const calls = [];
  globalThis.__UI_TEST_INVOKE__ = async command => { calls.push(command); throw Error(command); };
  await render(h(WorkspaceLocation, migrationCleanupProps(migrationCleanupWorkspace({ retainedRoots: [] }))));
  assert.equal(document.querySelector('.settings-migration-heading'), null);
  await click(buttonNamed('更换位置'));
  assert.match(document.querySelector('.settings-migration-heading').textContent, /复制并核验.*新位置成功重开后自动删除原目录.*此前中断会保留原目录/);
  assert.doesNotMatch(document.querySelector('.settings-migration-heading').textContent, /历史资料/);
  await inputValue(document.querySelector('.workspace-migration-form input:not([type="checkbox"])'), '/disposable/next-workspace');
  await submit(document.querySelector('.workspace-migration-form form'));
  assert.match(document.querySelector('.workspace-migration-confirmation').textContent, /网页项目和平台管理的 Node 项目文件夹.*新位置成功重开后.*自动永久删除/s);
  assert.deepEqual(calls, []);
});

test('A source-cleanup retry sends only its migration ID and waits for the durable task', async () => {
  await setup(); const accepted = deferred(); const calls = []; const tracked = []; let refreshes = 0;
  const running = migrationCleanupTask();
  const final = migrationCleanupTask({ status: 'succeeded', final: true, phase: 'completed', result: { completed: true, cleanupPending: false, sourceRetained: false } });
  globalThis.__UI_TEST_INVOKE__ = async (command, args) => {
    calls.push({ command, args });
    if (command === 'workspace_migration_cleanup') return accepted.promise;
    assert.equal(command, 'task_get'); assert.equal(args.taskId, running.taskId); return final;
  };
  let workspace = migrationCleanupWorkspace();
  const props = migrationCleanupProps(workspace, { onTask: task => tracked.push(task), onRefresh: async () => { refreshes++; } });
  await render(h(WorkspaceLocation, props));
  assert.match(document.querySelector('.settings-history-notice').textContent, /当前工作区可继续使用/);
  assert.equal(buttonNamed('重试删除').closest('details'), null, 'Pending cleanup and its retry stay visible outside technical details');
  assert.equal(buttonNamed('更换位置').disabled, true, 'Only another migration waits for cleanup');
  assert.equal(buttonNamed('去清理'), undefined);
  await click(buttonNamed('重试删除'));
  await click(buttonNamed('正在删除原目录…'));
  assert.deepEqual(calls, [{ command: 'workspace_migration_cleanup', args: { migrationId: 'task_disposable_migration' } }]);
  await act(async () => accepted.resolve(running));
  assert.match(document.querySelector('.workspace-source-cleanup-result').textContent, /删除迁移原目录/);
  assert.match(document.querySelector('.workspace-location-card').textContent, /离开此页不会取消.*当前工作区可继续使用/);
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 600)); });
  assert.equal(refreshes, 1);
  assert.equal(calls.filter(call => call.command === 'workspace_migration_cleanup').length, 1);
  assert.equal(tracked.at(-1).final, true);
  workspace = migrationCleanupWorkspace({ migrationCleanupTask: final, retainedRoots: [] });
  await render(h(WorkspaceLocation, { ...props, workspace }));
  assert.equal(buttonNamed('重试删除'), undefined);
  assert.equal(document.querySelector('.settings-history-notice'), null);
  assert.equal(buttonNamed('更换位置').disabled, false);
});

test('A restored cleanup task is polled independently and a stale pending snapshot cannot revive it', async () => {
  await setup(); const tracked = []; let refreshes = 0;
  globalThis.__UI_TEST_INVOKE__ = async command => { throw Error(`No extra request expected: ${command}`); };
  const running = migrationCleanupTask();
  const props = migrationCleanupProps(migrationCleanupWorkspace({ migrationCleanupTask: running }), { onTask: task => tracked.push(task), onRefresh: async () => { refreshes++; } });
  await render(h(WorkspaceLocation, props));
  assert.equal(buttonNamed('正在删除原目录…').disabled, true);
  const final = migrationCleanupTask({ final: true, status: 'succeeded', phase: 'completed', result: { completed: true, cleanupPending: false } });
  await render(h(WorkspaceLocation, { ...props, workspace: migrationCleanupWorkspace({ migrationCleanupTask: final, retainedRoots: [] }) }));
  assert.equal(refreshes, 1);
  await render(h(WorkspaceLocation, props));
  assert.equal(tracked.at(-1).final, true);
  assert.equal(buttonNamed('重试删除').disabled, false);
  assert.doesNotMatch(document.querySelector('.workspace-location-card').textContent, /原目录删除会继续执行/);
});

test('An uncertain source-cleanup acknowledgement refreshes status without repeating deletion', async () => {
  await setup(); const calls = []; let refreshes = 0;
  globalThis.__UI_TEST_INVOKE__ = async (command, args) => { calls.push({ command, args }); throw Error('Disposable lost response'); };
  await render(h(WorkspaceLocation, migrationCleanupProps(migrationCleanupWorkspace(), { onRefresh: async () => { refreshes++; } })));
  await click(buttonNamed('重试删除'));
  assert.equal(refreshes, 1);
  assert.match(document.querySelector('.workspace-location-card [role="alert"]').textContent, /删除结果需要核对.*Disposable lost response.*不会自动重复提交/);
  assert.deepEqual(calls, [{ command: 'workspace_migration_cleanup', args: { migrationId: 'task_disposable_migration' } }]);
});

test('A cleanup acknowledgement still reaches task tracking after leaving settings', async () => {
  await setup(); const accepted = deferred(); const tracked = []; const calls = [];
  globalThis.__UI_TEST_INVOKE__ = async (command, args) => { calls.push({ command, args }); assert.equal(command, 'workspace_migration_cleanup'); return accepted.promise; };
  await render(h(WorkspaceLocation, migrationCleanupProps(migrationCleanupWorkspace(), { onTask: task => tracked.push(task) })));
  await click(buttonNamed('重试删除'));
  await render(h('p', null, 'Settings closed'));
  const running = migrationCleanupTask();
  await act(async () => accepted.resolve(running));
  assert.deepEqual(tracked, [running]);
  assert.equal(calls.length, 1);
  assert.equal(document.querySelector('.workspace-source-cleanup-result'), null);
});

const appProject = (id, name) => ({ ...project, id, name, description: '', projectRoot: `/disposable/workspace/projects/${id}`, manifest: authorManifest });
function mockApp(state, custom = async (command) => { throw new Error(`Unexpected native request: ${command}`); }) {
  previewState = state;
  globalThis.__UI_TEST_INVOKE__ = async (command, args) => {
    if (command === 'bootstrap_info') return { displayName: '熊猫小抽屉', version: '0.0.0', identifier: 'dev.happypandaa.pandadrawer', instanceId: state.instanceId ?? 'instance_dom_fixture', workspaceRoot: '/disposable/workspace', dataRoot: '/disposable/development', resources: { sdk: true, template: true, readme: true, cli: true } };
    if (command === 'project_list') return state.projects;
    if (command === 'node_app_list') return state.nodeApps ?? [];
    if (command === 'workspace_info') return { workspaceRoot: '/disposable/workspace', migrationState: 'ready', retainedRoots: [] };
    if (command === 'maintenance_status') return normalStatus;
    if (command === 'workspace_storage') return workspaceStorage;
    if (command === 'storage_info') return emptyProjectStorage;
    if (command === 'storage_files') return emptyStorageFiles;
    if (command === 'storage_scan') return { taskId: 'task_disposable_scan', projectId: args.projectId, type: 'storageScan', status: 'succeeded', final: true, phase: 'completed', result: {}, error: null };
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
  await setup(); const opened = deferred(); const calls = []; const folders = [];
  const projectRoot = '/disposable/workspace/projects/ke-du-xiang-mu-a7c92e10';
  const state = { projects: [{ ...appProject('prj_native_window', 'Native window fixture'), projectRoot }] };
  mockApp(state, async (command, args) => {
    if (command === 'open_project_folder') { folders.push(args); return; }
    assert.equal(command, 'project_open'); calls.push(args); return opened.promise;
  });
  await render(h(App)); await openDetails('Native window fixture');
  assert.ok(dialog().textContent.includes(projectRoot), 'Show the actual registered directory rather than reconstructing it from the ID');
  const actions = dialog().querySelector('.detail-action-row');
  assert.ok(actions);
  assert.equal([...actions.querySelectorAll('button')].filter((button) => button.textContent === '打开').length, 1);
  assert.equal([...actions.querySelectorAll('button')].some((button) => /浏览器/.test(button.textContent)), false);
  await click(document.querySelector('.application-detail-row .text-button'));
  assert.deepEqual(folders, [{ path: projectRoot }]);
  const enter = document.querySelector('.application-primary-actions .primary-button');
  await click(enter); await click(enter);
  assert.deepEqual(calls, [{ projectId: 'prj_native_window', target: 'desktop', reload: false }]);
  await act(async () => opened.resolve({ projectId: 'prj_native_window', target: 'desktop', reused: false }));
  assert.match(dialog().textContent, /已在独立窗口打开/);
});

test('App preserves selected read-only details after external removal and disables mutations', async () => {
  await setup();
  const state = { projects: [appProject('prj_A', 'Fixture A')] };
  const projectRoot = state.projects[0].projectRoot; const folders = [];
  mockApp(state, async (command, args) => {
    assert.equal(command, 'open_project_folder'); folders.push(args);
  });
  await render(h(App)); await openDetails('Fixture A');
  await openMore();
  state.projects = [];
  await refreshAutomatically();
  assert.ok(dialog(), 'An external disappearance must not discard the detail editor');
  assert.match(dialog().textContent, /项目已不在当前工作区列表中/);
  assert.ok([...document.querySelectorAll('.detail-action-row button')].filter(button => button.textContent !== '打开文件夹').every(button => button.disabled));
  assert.equal(document.querySelector('.application-detail-row .text-button').disabled, false);
  await click(document.querySelector('.application-detail-row .text-button'));
  assert.deepEqual(folders, [{ path: projectRoot }]);
  assert.match(dialog().textContent, /Fixture A/);
  assert.equal(buttonNamed('编辑信息'), undefined);
  await key('Escape');
  assert.equal(dialog(), null);
});

test('App rejects a late AI handoff for a closed project after another project is selected', async () => {
  await setup(); const first = deferred(); const copied = [];
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async (text) => { copied.push(text); } } });
  const state = { projects: [appProject('prj_A', 'Fixture A'), appProject('prj_B', 'Fixture B')] };
  mockApp(state, async (command, args) => {
    assert.equal(command, 'project_handoff');
    return args.projectId === 'prj_A' ? first.promise : { text: 'Current handoff for B' };
  });
  await render(h(App)); await openDetails('Fixture A'); await click(buttonNamed('复制提示词'));
  await click(closeButton()); await openDetails('Fixture B'); await click(buttonNamed('复制提示词'));
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
  await click([...document.querySelectorAll('.application-heading button')].find((button) => button.textContent.trim() === '新建'));
  await inputValue(document.getElementById('project-name'), created.name);
  await submit(document.querySelector('.create-project-form'));
  assert.ok(dialog()); assert.match(dialog().textContent, /New disposable fixture/);
  assert.equal(document.querySelector('.create-project-form'), null);
  assert.equal(handoffs, 0); assert.deepEqual(copied, []);
  assert.equal(buttonNamed('复制提示词').disabled, false);
  assert.equal(buttonNamed('在 Codex 打开'), undefined);
  assert.equal(buttonNamed('准备应用').disabled, false);
  await click(buttonNamed('复制提示词'));
  assert.equal(buttonNamed('正在准备说明…').disabled, true);
  await click(buttonNamed('正在准备说明…'));
  assert.equal(handoffs, 1);
  await act(async () => prepared.resolve({ text: `Platform description and project root: ${created.projectRoot}` }));
  assert.deepEqual(copied, [`Platform description and project root: ${created.projectRoot}`]);
  assert.equal(buttonNamed('复制提示词').disabled, false);
  assert.match(document.querySelector('.detail-actions [role="status"]').textContent, /已复制/);
});

test('AI handoff remains selectable when the clipboard rejects access', async () => {
  await setup(); let attempts = 0;
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async () => { attempts++; throw new Error('Clipboard fixture denied'); } } });
  mockApp({ projects: [appProject('prj_A', 'Fixture A')] }, async (command) => {
    assert.equal(command, 'project_handoff'); return { text: 'Complete platform introduction and current project paths' };
  });
  await render(h(App)); await openDetails('Fixture A'); await click(buttonNamed('复制提示词'));
  const text = document.querySelector('[aria-label="给 AI 的完整说明"]');
  assert.equal(text.value, 'Complete platform introduction and current project paths');
  await act(async () => text.focus());
  assert.equal(text.selectionStart, 0); assert.equal(text.selectionEnd, text.value.length);
  assert.equal(attempts, 1);
  assert.match(dialog().querySelector('[role="status"]').textContent, /可以选中下面的文字并复制/);
  assert.doesNotMatch(dialog().textContent, /已复制「/);
  await click(buttonNamed('收起全文'));
  assert.equal(document.querySelector('[aria-label="给 AI 的完整说明"]'), null);
  assert.ok(buttonNamed('复制提示词'));
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
  await render(h(App)); await openDetails('Fixture A'); await click(buttonNamed('复制提示词'));
  assert.deepEqual(started, ['Handoff A']);
  await click(closeButton()); await openDetails('Fixture B'); await click(buttonNamed('复制提示词'));
  assert.deepEqual(started, ['Handoff A'], 'The new write must wait for an already-started system write');
  assert.equal(buttonNamed('正在准备说明…').disabled, true);
  await act(async () => firstWrite.resolve());
  assert.deepEqual(completed, ['Handoff A', 'Handoff B']);
  assert.equal(document.querySelector('[aria-label="给 AI 的完整说明"]').value, 'Handoff B');
  assert.match(dialog().querySelector('[role="status"]').textContent, /已复制「Fixture B」/);
});

test('A refreshed project removal invalidates an in-flight AI handoff while preserving its selected details', async () => {
  await setup(); const prepared = deferred(); const copied = [];
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async (text) => { copied.push(text); } } });
  const state = { projects: [appProject('prj_A', 'Fixture A')] };
  mockApp(state, async (command) => { assert.equal(command, 'project_handoff'); return prepared.promise; });
  await render(h(App)); await openDetails('Fixture A');
  await openMore();
  await click(buttonNamed('复制提示词'));
  state.projects = [];
  await refreshAutomatically();
  await act(async () => prepared.resolve({ text: 'Removed project must not be copied' }));
  assert.deepEqual(copied, []); assert.equal(document.querySelector('[aria-label="给 AI 的完整说明"]'), null);
  assert.equal(buttonNamed('复制提示词').disabled, true);
  assert.match(dialog().textContent, /Fixture A/);
});

const completeHandoff = (project, text = 'Fresh project instructions', instanceId = 'instance_dom_fixture') => ({ text, projectRoot: project.projectRoot, instanceId });

test('The shared handoff expands fresh text and project paths without copying until requested', async () => {
  await setup(); const events = []; let handoffs = 0;
  const project = appProject('prj_handoff', 'Handoff disposable fixture');
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async text => { events.push(['copy', text]); } } });
  mockApp({ projects: [project] }, async (command, args) => {
    events.push([command, args]); assert.equal(command, 'project_handoff');
    return completeHandoff(project, `Fresh handoff ${++handoffs}`);
  });
  await render(h(App)); await openDetails(project.name);
  assert.deepEqual(events, []);
  assert.equal(buttonNamed('在 Codex 打开'), undefined);
  await click(buttonNamed('查看交接说明'));
  assert.match(document.querySelector('.detail-handoff-help').textContent, /自行发送/);
  assert.deepEqual(events, [['project_handoff', { projectId: project.id }]]);
  assert.equal(document.querySelector('[aria-label="给 AI 的完整说明"]').value, 'Fresh handoff 1');
  assert.ok(document.querySelector('.handoff-preview').textContent.includes(project.projectRoot));
  assert.doesNotMatch(dialog().textContent, /Codex|完成后构建/);
  await click(buttonNamed('复制提示词'));
  assert.deepEqual(events.at(-1), ['copy', 'Fresh handoff 2']);
  assert.match(dialog().querySelector('[role="status"]').textContent, /已复制/);
  assert.equal(handoffs, 2);
});

test('Shared handoff clipboard failure retains selectable instructions and paths', async () => {
  await setup(); const project = appProject('prj_handoff_clipboard', 'Clipboard fallback'); const commands = [];
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async () => { throw new Error('Clipboard denied'); } } });
  mockApp({ projects: [project] }, async command => {
    commands.push(command); assert.equal(command, 'project_handoff');
    return completeHandoff(project, 'Retain these complete instructions');
  });
  await render(h(App)); await openDetails(project.name); await click(buttonNamed('复制提示词'));
  const text = document.querySelector('[aria-label="给 AI 的完整说明"]');
  assert.equal(text.value, 'Retain these complete instructions');
  await act(async () => text.focus());
  assert.equal(text.selectionStart, 0); assert.equal(text.selectionEnd, text.value.length);
  assert.ok(document.querySelector('.handoff-preview').textContent.includes(project.projectRoot));
  assert.match(dialog().querySelector('[role="status"]').textContent, /选中.*复制/);
  assert.doesNotMatch(dialog().querySelector('[role="status"]').textContent, /已复制/);
  assert.deepEqual(commands, ['project_handoff']);
});

test('Shared handoff failures release the action and retry fetches fresh instructions', async () => {
  await setup(); const project = appProject('prj_handoff_retry', 'Handoff retry'); let handoffs = 0;
  mockApp({ projects: [project] }, async command => {
    assert.equal(command, 'project_handoff');
    if (++handoffs === 1) throw { code: 'DISPOSABLE_READ_FAILED', message: '人工交接读取失败' };
    return completeHandoff(project, 'Retried instructions');
  });
  await render(h(App)); await openDetails(project.name); await click(buttonNamed('查看交接说明'));
  assert.match(dialog().querySelector('[role="alert"]').textContent, /人工交接读取失败/);
  assert.equal(buttonNamed('复制提示词').disabled, false);
  await click(buttonNamed('复制提示词'));
  assert.equal(document.querySelector('[aria-label="给 AI 的完整说明"]').value, 'Retried instructions');
  assert.equal(handoffs, 2);
});

test('Shared handoff preparation and clipboard phases reject duplicate submissions', async () => {
  await setup(); const project = appProject('prj_handoff_pending', 'Handoff pending fixture'); const handoff = deferred(); const write = deferred(); const calls = [];
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async () => write.promise } });
  mockApp({ projects: [project] }, async command => { calls.push(command); assert.equal(command, 'project_handoff'); return handoff.promise; });
  await render(h(App)); await openDetails(project.name); await click(buttonNamed('复制提示词'));
  assert.equal(buttonNamed('正在准备说明…').disabled, true);
  await click(buttonNamed('正在准备说明…')); await click(buttonNamed('查看交接说明'));
  assert.deepEqual(calls, ['project_handoff']);
  await act(async () => handoff.resolve(completeHandoff(project)));
  assert.equal(buttonNamed('正在准备说明…').disabled, true);
  assert.equal(buttonNamed('收起全文').disabled, true);
  await click(buttonNamed('正在准备说明…'));
  assert.deepEqual(calls, ['project_handoff']);
  await act(async () => write.resolve());
  assert.equal(buttonNamed('复制提示词').disabled, false);
});

test('Closing a request before handoff returns prevents copying the old project', async () => {
  await setup(); const project = appProject('prj_handoff_old', 'Old handoff fixture'); const next = appProject('prj_handoff_new', 'New handoff fixture'); const prepared = deferred(); const copied = [];
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async text => copied.push(text) } });
  mockApp({ projects: [project, next] }, async (command, args) => {
    assert.equal(command, 'project_handoff'); return args.projectId === project.id ? prepared.promise : completeHandoff(next, 'Current handoff');
  });
  await render(h(App)); await openDetails(project.name); await click(buttonNamed('复制提示词'));
  await click(closeButton()); await openDetails(next.name); await click(buttonNamed('复制提示词'));
  await act(async () => prepared.resolve(completeHandoff(project, 'Stale handoff')));
  assert.deepEqual(copied, ['Current handoff']);
  assert.equal(document.querySelector('[aria-label="给 AI 的完整说明"]').value, 'Current handoff');
  assert.doesNotMatch(dialog().textContent, /Stale handoff/);
});

test('A changed instance or project root invalidates a pending handoff clipboard response', async () => {
  for (const change of ['instance', 'root']) {
    await setup(); const project = appProject(`prj_handoff_${change}`, `Handoff ${change} fixture`); const write = deferred();
    const state = { projects: [project] };
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async () => write.promise } });
    mockApp(state, async command => { assert.equal(command, 'project_handoff'); return completeHandoff(project); });
    await render(h(App)); await openDetails(project.name); await click(buttonNamed('复制提示词'));
    assert.ok(document.querySelector('[aria-label="给 AI 的完整说明"]'));
    if (change === 'instance') state.instanceId = 'instance_after_migration';
    else state.projects = [{ ...project, projectRoot: '/disposable/migrated/projects/current' }];
    await refreshAutomatically();
    assert.equal(document.querySelector('[aria-label="给 AI 的完整说明"]'), null);
    await act(async () => write.resolve());
    assert.equal(buttonNamed('复制提示词').disabled, false);
    assert.doesNotMatch(dialog().textContent, /已复制/);
    await act(async () => root.unmount()); root = null;
  }
});

test('A mismatched fresh handoff preserves its text but is never copied', async () => {
  await setup(); const project = appProject('prj_handoff_changed', 'Changed handoff fixture'); const copied = []; const commands = [];
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async text => copied.push(text) } });
  mockApp({ projects: [project] }, async command => {
    commands.push(command); assert.equal(command, 'project_handoff');
    return { text: 'Current service instructions', projectRoot: '/disposable/moved/project', instanceId: 'new_instance' };
  });
  await render(h(App)); await openDetails(project.name); await click(buttonNamed('复制提示词'));
  assert.deepEqual(commands, ['project_handoff']); assert.deepEqual(copied, []);
  assert.equal(document.querySelector('[aria-label="给 AI 的完整说明"]').value, 'Current service instructions');
  assert.match(dialog().querySelector('[role="alert"]').textContent, /项目目录或服务信息已变化/);
  assert.equal(buttonNamed('复制提示词').disabled, false);
});

test('Automatic storage scans do not hide a failed build or replace the result of its explicit retry', async () => {
  await setup();
  const project = appProject('prj_build_scan', 'Build scan fixture');
  const state = { projects: [project] };
  const build = { taskId: 'task_build_failure', type: 'build', projectId: project.id, status: 'queued', final: false, phase: 'queued', result: null, error: null };
  const scan = { taskId: 'task_auto_scan', type: 'storageScan', projectId: project.id, status: 'running', final: false, phase: 'scan', result: null, error: null };
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
  await render(h(App)); await openDetails(project.name); await click(buttonNamed('准备应用'));
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 1100)); });
  assert.match(dialog().textContent, /Explicit build failure must stay visible/);
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 1100)); });
  assert.match(dialog().textContent, /Explicit build failure must stay visible/);
  assert.equal(buttonNamed('准备应用').disabled, false);
  await click(buttonNamed('准备应用'));
  assert.equal(submissions, 2);
  assert.doesNotMatch(dialog().textContent, /Explicit build failure must stay visible/);
  assert.match(dialog().querySelector('.project-last-task').textContent, /构建已完成，打开应用即可使用当前文件/);
});

test('Delete task results distinguish permanent deletion, partial deletion and no deletion', async () => {
  await setup();
  for (const [operationOutcome, expected, rejected] of [
    ['deleted', /项目文件已永久删除/, /可从.*恢复|完整项目已移入回收站/],
    ['delete_incomplete', /部分项目文件可能已删除/, /项目文件已永久删除/],
    ['not_deleted', /尚未执行删除/, /项目文件已永久删除/],
  ]) {
    await render(h(ProjectTask, { task: { taskId: `task_delete_${operationOutcome}`, type: 'delete', projectId: 'prj_fixture', status: operationOutcome === 'delete_incomplete' ? 'failed' : 'succeeded', final: true, phase: 'completed', error: null, result: { operationOutcome } } }));
    const result = document.querySelector('.build-task').textContent;
    assert.match(result, expected); assert.doesNotMatch(result, rejected);
  }
});

test('App retains a failed delete dialog and closes it only after its accepted task confirms deleted', async () => {
  await setup(); let submissions = 0;
  const state = { projects: [appProject('prj_A', 'Fixture A')] };
  const task = { taskId: 'task_delete_fixture', type: 'delete', projectId: 'prj_A', status: 'queued', final: false, phase: 'queued' };
  mockApp(state, async (command, args) => {
    if (command === 'project_delete') {
      assert.deepEqual(args, { projectId: 'prj_A', input: { expectedDocumentEpoch: project.documentEpoch } });
      submissions++;
      return submissions === 1 ? { ...task, taskId: 'task_failed_delete', status: 'failed', final: true, error: { code: 'FIXTURE_FAILURE', message: 'Delete fixture failed' } } : task;
    }
    assert.equal(command, 'task_get');
    state.projects = [];
    return { ...task, status: 'succeeded', final: true, phase: 'completed', result: { operationOutcome: 'deleted' } };
  });
  await render(h(App)); await openProjectMenu('Fixture A'); await click(document.querySelector('[aria-label="删除项目「Fixture A」"]'));
  assert.equal(dialog().getAttribute('aria-labelledby') && document.getElementById(dialog().getAttribute('aria-labelledby')).textContent, '删除项目');
  assert.equal(dialog().querySelector('[role="tablist"]'), null);
  assert.doesNotMatch(dialog().textContent, /复制提示词|在 Codex 打开|构建/);
  assert.match(dialog().textContent, /永久删除.*无法撤销/);
  assert.match(dialog().textContent, /平台登记、配置、日志、任务和缓存/);
  assert.ok(dialog().textContent.includes(state.projects[0].projectRoot));
  assert.equal(buttonNamed('确认永久删除项目').disabled, true);
  await click(document.querySelector('.project-delete-confirmation input'));
  await click(buttonNamed('确认永久删除项目'));
  assert.ok(dialog()); assert.match(dialog().textContent, /Delete fixture failed/);
  await click(buttonNamed('取消，保留项目'));
  assert.equal(dialog(), null);
  await openProjectMenu('Fixture A'); await click(document.querySelector('[aria-label="删除项目「Fixture A」"]'));
  assert.equal(document.querySelector('.project-delete-confirmation input').checked, false);
  assert.equal(buttonNamed('确认永久删除项目').disabled, true);
  await click(document.querySelector('.project-delete-confirmation input'));
  await click(buttonNamed('确认永久删除项目'));
  assert.equal(submissions, 2); assert.ok(dialog(), 'Queued acceptance is not final success');
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 1100)); });
  assert.equal(dialog(), null);
  assert.match(document.querySelector('.notice-success').textContent, /项目已永久删除/);
});

test('A final project-task deletion result ends loading while its lifecycle poll remains pending', async () => {
  const originalInterval = window.setInterval; const originalClear = window.clearInterval;
  const timers = new Map(); let timerId = 0;
  window.setInterval = (callback, delay) => { const id = ++timerId; timers.set(id, { callback, delay }); return id; };
  window.clearInterval = id => timers.delete(id);
  try {
    for (const status of ['succeeded', 'failed', 'interrupted']) {
      await setup(); const stalled = deferred(); let polls = 0;
      const fixture = appProject(`prj_single_poll_${status}`, `Single deletion poll ${status}`);
      const state = { projects: [fixture] };
      const task = { taskId: `task_single_poll_${status}`, type: 'delete', projectId: fixture.id, status: 'queued', final: false, phase: 'queued', result: null, error: null };
      mockApp(state, async (command, args) => {
        if (command === 'project_delete') return task;
        assert.equal(command, 'task_get'); assert.deepEqual(args, { taskId: task.taskId, projectId: task.projectId, taskType: 'delete' });
        if (++polls > 1) return stalled.promise;
        if (status === 'succeeded') state.projects = [];
        return { ...task, status, final: true, phase: status === 'succeeded' ? 'completed' : status, result: { operationOutcome: status === 'succeeded' ? 'deleted' : status === 'failed' ? 'not_deleted' : 'unknown' }, ...(status === 'succeeded' ? {} : { error: { code: 'DISPOSABLE_SINGLE_POLL', message: `Deletion ${status} from project poll` } }) };
      });
      await render(h(App)); await openProjectMenu(fixture.name); await click(document.querySelector(`[aria-label="删除项目「${fixture.name}」"]`));
      await click(document.querySelector('.project-delete-confirmation input')); await click(buttonNamed('确认永久删除项目'));
      deletionProgress('删除任务已受理，正在等待执行');
      const pollingTimers = [...timers.values()].filter(timer => timer.delay === 1000);
      assert.equal(pollingTimers.length, 2, 'Both project and lifecycle observers are waiting for the accepted task');
      await act(async () => { for (const timer of pollingTimers) timer.callback(); });
      assert.equal(polls, 2, 'The lifecycle observation is still unresolved');
      assert.equal(Boolean(document.querySelector('.delete-progress')), false, 'Either validated final observation must end loading');
      if (status === 'succeeded') {
        assert.equal(dialog(), null);
        assert.equal(document.querySelector(`[aria-label="进入项目「${fixture.name}」"]`), null);
      } else {
        assert.ok(dialog());
        assert.ok(dialog().textContent.includes(`Deletion ${status} from project poll`));
        assert.equal(buttonNamed('取消，保留项目').disabled, false);
      }
      await act(async () => root.unmount()); root = null;
    }
  } finally { window.setInterval = originalInterval; window.clearInterval = originalClear; }
});

test('A missing successful deletion task receipt clears project state and ignores a stale list response', async () => {
  const originalInterval = window.setInterval; const originalClear = window.clearInterval; const timers = new Map(); let timerId = 0;
  window.setInterval = (callback, delay) => { const id = ++timerId; timers.set(id, { callback, delay }); return id; };
  window.clearInterval = id => timers.delete(id);
  try {
    await setup(); const fixture = appProject('prj_absent_delete', 'Disposable absent deletion');
    const task = { taskId: 'task_absent_delete', type: 'delete', projectId: fixture.id, status: 'running', final: false, phase: 'delete-intent', result: null, error: null };
    mockApp({ projects: [fixture] }, async (command, args) => {
      if (command === 'project_delete') return task;
      assert.equal(command, 'task_get'); assert.deepEqual(args, { taskId: task.taskId, projectId: fixture.id, taskType: 'delete' });
      return { ...task, status: 'succeeded', final: true, phase: 'completed', result: { projectId: fixture.id, operationOutcome: 'deleted', alreadyAbsent: true } };
    });
    await render(h(App)); await openProjectMenu(fixture.name); await click(document.querySelector(`[aria-label="删除项目「${fixture.name}」"]`));
    await click(document.querySelector('.project-delete-confirmation input')); await click(buttonNamed('确认永久删除项目'));
    await act(async () => { for (const timer of [...timers.values()].filter(timer => timer.delay === 1000)) timer.callback(); });
    assert.equal(dialog(), null);
    assert.equal(document.querySelector(`[aria-label="进入项目「${fixture.name}」"]`), null);
    assert.equal(document.querySelector('.radio-task-button'), null);
    assert.equal(document.body.textContent.includes(task.taskId), false);
  } finally { window.setInterval = originalInterval; window.clearInterval = originalClear; }
});

test('A final deletion task with an incomplete or missing outcome keeps the deletion dialog open', async () => {
  await setup();
  for (const outcome of ['delete_incomplete', null]) {
    mockApp({ projects: [appProject('prj_delete_review', 'Deletion review fixture')] }, async command => {
      assert.equal(command, 'project_delete');
      return { taskId: `task_review_${outcome}`, type: 'delete', projectId: 'prj_delete_review', status: 'succeeded', final: true, phase: 'completed', error: null, result: outcome ? { operationOutcome: outcome, remainingPath: '/disposable/workspace/projects/prj_delete_review' } : null };
    });
    await render(h(App)); await openProjectMenu('Deletion review fixture'); await click(document.querySelector('[aria-label="删除项目「Deletion review fixture」"]'));
    await click(document.querySelector('.project-delete-confirmation input')); await click(buttonNamed('确认永久删除项目'));
    assert.ok(dialog()); assert.equal(document.querySelector('.notice-success'), null);
    assert.match(dialog().textContent, outcome ? /部分项目文件可能已删除/ : /永久删除结果尚不能确认/);
    assert.equal(document.querySelector('.project-delete-confirmation input').checked, false);
    assert.equal(buttonNamed('确认永久删除项目').disabled, true);
    await render(null);
  }
});

test('Cancelling the standalone launcher deletion dialog never writes and reopening requires fresh acknowledgement', async () => {
  await setup(); const calls = [];
  mockApp({ projects: [appProject('prj_cancel_delete', 'Cancel deletion fixture')] }, async (...args) => {
    calls.push(args); throw new Error('Cancelling must not issue a write');
  });
  await render(h(App));
  const openDelete = async () => { await openProjectMenu('Cancel deletion fixture'); await click(document.querySelector('[aria-label="删除项目「Cancel deletion fixture」"]')); };
  for (const close of [() => click(buttonNamed('取消，保留项目')), () => key('Escape'), () => click(layer()), () => click(closeButton())]) {
    await openDelete();
    assert.equal(document.querySelector('.project-delete-confirmation input').checked, false);
    assert.equal(buttonNamed('确认永久删除项目').disabled, true);
    await click(document.querySelector('.project-delete-confirmation input'));
    await close();
    assert.equal(dialog(), null);
    assert.ok(document.querySelector('[aria-label="进入项目「Cancel deletion fixture」"]'));
  }
  assert.deepEqual(calls, []);
});

test('The deletion dialog blocks every close path and duplicate submission through acceptance and task completion', async () => {
  await setup(); const accepting = deferred(); const completion = deferred(); let submissions = 0;
  const state = { projects: [appProject('prj_pending_delete', 'Pending deletion fixture')] };
  const task = { taskId: 'task_pending_delete_dialog', type: 'delete', projectId: state.projects[0].id, status: 'queued', final: false, phase: 'queued' };
  mockApp(state, async command => {
    if (command === 'project_delete') { submissions++; return accepting.promise; }
    assert.equal(command, 'task_get'); return completion.promise;
  });
  await render(h(App)); await openProjectMenu('Pending deletion fixture'); await click(document.querySelector('[aria-label="删除项目「Pending deletion fixture」"]'));
  await click(document.querySelector('.project-delete-confirmation input')); await click(buttonNamed('确认永久删除项目'));
  deletionProgress('正在提交删除请求');
  const attemptClose = async () => {
    assert.equal(closeButton().disabled, true);
    assert.equal(buttonNamed('取消，保留项目').disabled, true);
    assert.equal(document.querySelector('.project-delete-confirmation input').disabled, true);
    await key('Escape'); await click(layer()); await click(closeButton()); await click(buttonNamed('取消，保留项目'));
    assert.ok(dialog());
    await click(dialog().querySelector('.confirmation-actions .danger-button'));
    assert.equal(submissions, 1);
  };
  await attemptClose();
  await act(async () => accepting.resolve(task));
  deletionProgress('删除任务已受理，正在等待执行');
  await attemptClose();
  assert.ok(document.querySelector('[aria-label="进入项目「Pending deletion fixture」"]'));
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 1100)); });
  state.projects = [];
  await act(async () => completion.resolve({ ...task, status: 'succeeded', final: true, phase: 'completed', result: { operationOutcome: 'deleted' } }));
  assert.equal(dialog(), null);
  assert.equal(document.querySelector('[aria-label="进入项目「Pending deletion fixture」"]'), null);
  assert.equal(submissions, 1);
});

test('A completed deletion for an old same-ID instance neither removes its replacement nor leaves a generic success after cancel', async () => {
  await setup(); const completion = deferred(); let submissions = 0;
  const original = appProject('prj_replaced_delete', 'Replaced deletion fixture');
  const replacement = { ...original, projectRoot: '/disposable/workspace/projects/same-id-new-instance', documentEpoch: '55555555-5555-4555-8555-555555555555' };
  const state = { projects: [original] };
  const task = { taskId: 'task_replaced_delete', projectId: original.id, type: 'delete', status: 'queued', final: false, phase: 'queued' };
  mockApp(state, async command => {
    if (command === 'project_delete') { submissions++; return task; }
    assert.equal(command, 'task_get'); return completion.promise;
  });
  await render(h(App)); await openProjectMenu(original.name); await click(document.querySelector(`[aria-label="删除项目「${original.name}」"]`));
  await click(document.querySelector('.project-delete-confirmation input')); await click(buttonNamed('确认永久删除项目'));
  state.projects = [replacement];
  // Simulate a completed background refresh while the deletion task is pending.
  await refreshAutomatically();
  assert.ok(dialog().textContent.includes(replacement.projectRoot));
  assert.equal(document.querySelector('.project-delete-confirmation input').checked, false);
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 1100)); });
  await act(async () => completion.resolve({ ...task, status: 'succeeded', final: true, phase: 'completed', result: { operationOutcome: 'deleted' } }));
  assert.ok(dialog()); assert.ok(dialog().textContent.includes(replacement.projectRoot));
  assert.ok(document.querySelector(`[aria-label="进入项目「${replacement.name}」"]`));
  await click(buttonNamed('取消，保留项目'));
  assert.equal(dialog(), null);
  assert.equal(document.querySelector('.notice-success'), null);
  assert.ok(document.querySelector(`[aria-label="进入项目「${replacement.name}」"]`));
  assert.equal(submissions, 1);
});

test('The footer deletion dialog cancels back to the same details and preserves the selected application', async () => {
  await setup();
  mockApp({ projects: [appProject('prj_nested_delete', 'Nested deletion fixture')] });
  await render(h(App)); await openDetails('Nested deletion fixture');
  await openMore();
  const trigger = buttonNamed('停止并删除…'); trigger.focus(); await click(trigger);
  const deleteDialog = [...document.querySelectorAll('[role="dialog"]')].at(-1);
  assert.equal(document.querySelectorAll('[role="dialog"]').length, 2);
  assert.ok(deleteDialog.contains(document.activeElement));
  assert.equal(deleteDialog.querySelector('[role="tablist"]'), null);
  await click(document.querySelector('.project-delete-confirmation input'));
  await click(buttonNamed('取消，保留项目'));
  assert.equal(document.querySelectorAll('[role="dialog"]').length, 1);
  assert.equal(document.activeElement, trigger);
  assert.match(dialog().textContent, /Nested deletion fixture/);
  await click(trigger);
  assert.equal(document.querySelector('.project-delete-confirmation input').checked, false);
  await key('Escape');
  assert.equal(document.querySelectorAll('[role="dialog"]').length, 1);
  assert.equal(document.querySelector('[aria-label="关闭详情确认"]'), null);
});

test('App blocks closing a pending create, shows its failure inside the modal, and preserves the name on reopening', async () => {
  await setup(); const creation = deferred(); let submissions = 0;
  mockApp({ projects: [] }, async (command) => { assert.equal(command, 'project_create'); submissions++; return creation.promise; });
  await render(h(App));
  await click([...document.querySelectorAll('.application-heading button')].find((button) => button.textContent.trim() === '新建'));
  await inputValue(document.getElementById('project-name'), 'Unsubmitted project draft');
  await submit(document.querySelector('.create-project-form'));
  await key('Escape'); await click(layer());
  assert.ok(dialog()); assert.equal(submissions, 1);
  await act(async () => creation.reject(new Error('Create fixture failed')));
  assert.match(dialog().querySelector('[role="alert"]').textContent, /Create fixture failed/);
  await click(closeButton());
  assert.equal(dialog(), null);
  await click([...document.querySelectorAll('.application-heading button')].find((button) => button.textContent.trim() === '新建'));
  assert.equal(document.getElementById('project-name').value, 'Unsubmitted project draft');
  assert.equal(submissions, 1);
});


async function managerDeletion(value) {
  const listener = globalThis.__UI_EVENT_LISTENERS__?.get('panda-project-deleted');
  assert.ok(listener, 'Manager must install its native deletion listener before listing projects');
  await act(async () => listener({ payload: value }));
}
const managerRemoved = (id, extra = {}) => ({ version: 1, kind: 'web', id, taskIds: [], sourceIds: [], sourceRoots: [], instanceId: 'instance_dom_fixture', ...extra });
test('A CLI deletion notification rejects a late project list and keeps another project selected', async () => {
  await setup(); const a = appProject('prj_cli_A', 'CLI removed A'); const b = appProject('prj_cli_B', 'Preserved B');
  const state = { projects: [a, b] }; mockApp(state);
  await render(h(App)); await openDetails(b.name); await openMore();
  const original = globalThis.__UI_TEST_INVOKE__; const stale = deferred(); let delay = true;
  globalThis.__UI_TEST_INVOKE__ = (command, args) => command === 'project_list' && delay ? (delay = false, stale.promise) : original(command, args);
  await refreshAutomatically();
  state.projects = [b];
  await managerDeletion(managerRemoved(a.id, { sourceRoots: [a.projectRoot] }));
  await act(async () => stale.resolve([a, b]));
  assert.equal(document.querySelector(`[aria-label="进入项目「${a.name}」"]`), null);
  assert.match(dialog().textContent, /Preserved B/);
  await managerDeletion(managerRemoved(b.id, { instanceId: 'another-runtime' }));
  assert.match(dialog().textContent, /Preserved B/);
});
test('A CLI deletion revokes the unified import and ignores its late status response', async () => {
 await setup();const delayed=deferred();const owner='disposable_node_cli_A';
 const task={taskId:'task_cli_null_source',projectId:null,type:'nodeGitImport',status:'running',final:false,phase:'downloading',result:{ownerAppId:owner},error:null};
 mockApp({projects:[]},async command=>{
   if(command==='node_git_import_start')return task;
   if(command==='node_git_import_status')return delayed.promise;
   if(command==='task_get')return delayed.promise.then(value=>value.task);
   throw Error(command);
 });
 await render(h(App));await click(buttonNamed('从 Git 添加'));await inputValue(document.getElementById('git-import-url'),'https://github.com/disposable/cli-source');await click(buttonNamed('开始添加'));await click(buttonNamed('在后台继续'));
 await click(document.querySelector('.radio-task-button'));
 assert.equal(document.querySelectorAll('.task-center-entry').length,1);
 await managerDeletion(managerRemoved(owner,{kind:'node',taskIds:[task.taskId],sourceIds:[]}));
 await act(async()=>delayed.resolve({task:{...task,final:true,status:'succeeded',phase:'completed'},appId:owner,sourcePath:'/disposable/source',commitState:'committed',cleanupPending:false,remainingPaths:[],app:null}));
 assert.equal(document.querySelectorAll('.task-center-entry').length,0);assert.equal(document.querySelector('.task-center-import'),null);
});

test('An obsolete bootstrap reply cannot change the deletion runtime or erase a confirmed revocation', async () => {
  await setup(); const a = appProject('prj_stale_bootstrap_A', 'Bootstrap removed A'); const b = appProject('prj_stale_bootstrap_B', 'Bootstrap B');
  const state = { projects: [a, b] }; mockApp(state); await render(h(App));
  const original = globalThis.__UI_TEST_INVOKE__; const stale = deferred(); let delay = true;
  globalThis.__UI_TEST_INVOKE__ = (command, args) => command === 'bootstrap_info' && delay ? (delay = false, stale.promise) : original(command, args);
  await refreshAutomatically();
  state.projects = [b]; await managerDeletion(managerRemoved(a.id));
  await act(async () => stale.resolve({ instanceId: 'obsolete-bootstrap-runtime', workspaceRoot: '/obsolete-workspace' }));
  assert.equal(projectWasDeleted(a.id), true);
  assert.equal(document.querySelector(`[aria-label="进入项目「${a.name}」"]`), null);
  assert.ok(document.querySelector(`[aria-label="进入项目「${b.name}」"]`));
});
