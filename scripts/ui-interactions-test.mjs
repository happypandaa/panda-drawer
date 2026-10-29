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
await writeFile(mockPath, 'export const invoke = (command, ...args) => ["source_import_recover", "node_app_delete_tasks"].includes(command) ? Promise.resolve([]) : command === "project_delete_preview" ? globalThis.__UI_DELETE_PREVIEW__(...args) : globalThis.__UI_TEST_INVOKE__(command, ...args); export const isTauri = () => true; export const listen = async (name, callback) => { const listeners = globalThis.__UI_EVENT_LISTENERS__ ??= new Map(); listeners.set(name, callback); return () => listeners.delete(name); };\n');
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
    source = source.replaceAll(/(['"])@tauri-apps\/api\/(?:core|event)\1/g, JSON.stringify(pathToFileURL(mockPath).href));
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
const { WorkspaceHub } = await component('WorkspaceHub.tsx');
const { StorageScopeDetails } = await component('StoragePanels.tsx');
const { WorkspaceLocation } = await component('WorkspaceLocation.tsx');
const { resetHistoryScanForTests } = await component('history-scan-store.ts');
const { WorkspaceCleanupPanel } = await component('WorkspaceCleanupPanel.tsx');
const { default: App } = await component('App.tsx');
const { resetProjectDeletionsForTests, acceptProjectDeletion, observeDeletionRuntime, projectWasDeleted } = await component('project-deletion.ts');
const { ImportPanel } = await component('LifecyclePanels.tsx');
const { ProjectDeleteDialog } = await component('ProjectDeleteDialog.tsx');
const { ProjectTask } = await component('ProjectTask.tsx');
let root;
let render;
let previewState = null;
const previewContexts = new Map();
function previewFixture(value, status = normalStatus, instanceId = 'preview-fixture') {
  return { projectId: value.id, instanceId, projectRoot: value.projectRoot, expectedDocumentEpoch: value.documentEpoch || '22222222-2222-4222-8222-222222222222', mode: status?.mode ?? 'ready', maintenanceId: status?.maintenanceId ?? null, deletePaths: [value.projectRoot, `/disposable/owned-material/${value.id}`], alreadyAbsent: false, cleanupOnly: false };
}
async function setup() {
  previewState = null; previewContexts.clear();
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

test('Project import always requests a new independent identity', async () => {
  await setup(); const calls = []; const tasks = [];
  globalThis.__UI_TEST_INVOKE__ = async (command, args) => {
    assert.equal(command, 'project_import'); calls.push(args);
    const projectId = `prj_disposable_${calls.length}`;
    return { taskId: `task_disposable_import_${calls.length}`, projectId, type: 'import', status: 'succeeded', final: true, phase: 'completed', result: { projectId, targetProjectId: projectId, operationOutcome: 'imported', cleanupPending: false, cleanup: { completed: true } }, error: null };
  };
  const props = { disabled: false, onTask(value) { tasks.push(value); }, onClose() {} };
  await render(h(ImportPanel, props));
  assert.equal(document.querySelector('input[name="import-mode"]'), null);
  assert.match(document.querySelector('.project-help').textContent, /重复导入可以并存/);
  await inputValue(document.querySelector('.path-input input'), '/disposable/shared-project.zip');
  await inputValue(document.querySelector('form > label input'), 'My independent project');
  await click(buttonNamed('校验并导入'));
  assert.deepEqual(calls, [{ input: { archivePath: '/disposable/shared-project.zip', cleanupTemporaryFiles: true, name: 'My independent project' } }]);
  assert.match(dialog().textContent, /导入成功/);
  await act(async () => root.unmount()); root = null;
  await setup(); await render(h(ImportPanel, props));
  await inputValue(document.querySelector('.path-input input'), '/disposable/shared-project.zip');
  assert.equal(document.querySelector('input[value="original"]'), null);
  assert.match(document.querySelector('.project-help').textContent, /每次导入都生成新 ID/);
  await click(buttonNamed('校验并导入'));
  assert.deepEqual(calls[1], { input: { archivePath: '/disposable/shared-project.zip', cleanupTemporaryFiles: true } });
  assert.equal(tasks.length, 2);
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

const project = { id: 'prj_ui_test', projectRoot: '/disposable/workspace/projects/prj_ui_test', name: 'Disposable DOM fixture', documentEpoch: '11111111-1111-4111-8111-111111111111', state: 'ready', issues: [] };
const normalStatus = { mode: 'ready', readyForChanges: false, recoveryRequired: false };
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
const metadata = { name: project.name, description: '', metadataRevision: 'revision_fixture', iconStatus: { state: 'default' }, metadataIssues: [], manifest: { docs: { readme: 'README.md' } } };
function overviewProps(extra = {}) {
  return { project: { ...project, projectRoot: '/disposable/dom-fixture' }, initialTab: 'overview', status: normalStatus, disabled: false, onTask() {}, onChanged: async () => {}, onHandoff() {}, onClose() {}, ...extra };
}
const buttonNamed = (name) => [...document.querySelectorAll('button')].find((button) => button.textContent.trim() === name);

test('Ordinary project details show three pages with version tools collapsed and never submit legacy backup tools', async () => {
  await setup(); const calls = [];
  globalThis.__UI_TEST_INVOKE__ = async command => {
    calls.push(command);
    if (command === 'project_metadata') return metadata;
    if (command === 'project_git_source_restore_status') return clearRecovery;
    if (command === 'storage_info') return emptyProjectStorage;
    if (command === 'storage_files') return emptyStorageFiles;
    throw new Error(`Ordinary details must not invoke legacy operations: ${command}`);
  };
  await render(h(ProjectOverview, overviewProps()));
  assert.deepEqual([...document.querySelectorAll('[role="tab"]')].map(tab => tab.textContent), ['概览', '空间与文件', '更多操作']);
  assert.equal(document.querySelector('.operation-recovery'), null);
  await click(hubTab('空间与文件')); await click(hubTab('更多操作')); await click(hubTab('概览'));
  assert.equal(document.querySelector('.project-git-panel'), null);
  assert.equal(document.querySelector('.backup-panel'), null);
  assert.doesNotMatch(document.querySelector('.project-overview').textContent, /源码版本|创建独立副本|历史与维护|旧版工具|完整快照/);
  assert.equal(calls.some(command => /^(backup_|maintenance_begin|project_git_(initialize|save|branch|source_restore_(apply|recover)))/.test(command)), false);
});

function overviewGitStatus(extra = {}) {
  return { repository: { state: 'project-root', root: '/disposable/dom-fixture', markers: [], head: { oid: 'a'.repeat(40), branch: 'refs/heads/main' }, indexState: 'matched', git: { path: '/disposable/git', version: '2.56.0' } }, scope: { policy: 'project-source-v1', includedRoots: ['app'], includedPaths: ['app/src/App.tsx'], excluded: [], unclassifiedPaths: [], protectedTrackedPaths: [], complete: true }, changes: [{ path: 'app/src/App.tsx', kind: 'modify', staged: false }], snapshotDigest: 'b'.repeat(64), canInitialize: false, canSave: true, canListVersions: true, canCreateBranch: false, branchCreationIssue: null, database: { state: 'available', canSave: true, reason: 'Disposable data' }, issues: [], ...extra };
}
async function openOverviewVersions() {
  const details = [...document.querySelectorAll('details')].find(element => element.querySelector('summary')?.textContent === '源码与数据版本');
  assert.ok(details);
  await act(async () => { details.open = true; details.dispatchEvent(new window.Event('toggle')); });
  return details;
}

test('Web details explicitly mount retained read-only version tools without save drafts', async () => {
  await setup(); const calls = []; const states = [];
  globalThis.__UI_TEST_INVOKE__ = async command => {
    calls.push(command);
    if (command === 'project_metadata') return metadata;
    if (command === 'project_git_source_restore_status' || command === 'project_git_data_save_status' || command === 'project_git_data_restore_status') return clearRecovery;
    if (command === 'project_git_status') return overviewGitStatus();
    if (command === 'storage_info') return emptyProjectStorage;
    if (command === 'storage_files') return emptyStorageFiles;
    throw new Error(`Unexpected version integration request: ${command}`);
  };
  await render(h(ProjectOverview, overviewProps({ onDevelopmentStateChange: value => states.push(value) })));
  assert.equal(calls.includes('project_git_status'), false);
  const details = await openOverviewVersions();
  assert.ok(document.querySelector('.project-git-panel'));
  assert.equal([...document.querySelectorAll('label')].some(element => element.textContent === '版本说明'), false);
  assert.equal(calls.some(command => /project_git_(save|initialize|branch_create|data_save)$/.test(command)), false);
  await act(async () => { details.open = false; details.dispatchEvent(new window.Event('toggle')); });
  assert.equal(details.open, false);
  await click(hubTab('空间与文件')); await click(hubTab('概览'));
  assert.equal(document.querySelector('[aria-label="关闭详情确认"]'), null);
  await render(h('div'));
  assert.deepEqual(states.at(-1), { dirty: false, pending: false });
});

test('A project requiring data recovery opens its recovery entry even when ordinary Git and metadata are unavailable', async () => {
  await setup(); const calls = [];
  globalThis.__UI_TEST_INVOKE__ = async command => {
    calls.push(command);
    if (command === 'project_git_data_restore_status') return { state: 'pending', businessBlocked: true, operations: [], issues: [{ message: '保留的恢复现场需要核对' }] };
    if (command === 'project_git_source_restore_status' || command === 'project_git_data_save_status') return clearRecovery;
    if (command === 'project_git_status') throw new Error('当前活库缺失，普通 Git 检查不可用');
    if (command === 'project_metadata') throw new Error('元信息暂不可读取');
    throw new Error(`Recovery details must not mutate: ${command}`);
  };
  await render(h(ProjectOverview, overviewProps({ initialTab: 'manage', project: { ...project, state: 'error', projectRoot: '/disposable/dom-fixture', buildInfo: { sourceState: 'unknown', dataRecoveryRequired: true, dataRecoveryState: 'pending' } } })));
  const section = document.querySelector('[aria-label="业务数据取回恢复"]');
  assert.ok(section);
  assert.match(section.textContent, /保留的恢复现场需要核对/);
  assert.equal(hubTab('概览').getAttribute('aria-selected'), 'true');
  assert.match(document.querySelector('.project-git-panel').textContent, /当前活库缺失/);
  assert.ok(calls.indexOf('project_git_data_restore_status') < calls.indexOf('project_git_status'));
  const details = section.closest('details');
  await act(async () => { details.open = false; details.dispatchEvent(new window.Event('toggle')); });
  assert.equal(details.open, true);
  assert.equal(calls.some(command => /_(apply|recover|save|initialize)$/.test(command)), false);
});

test('An immediately completed data recovery refreshes the parent project information', async () => {
  await setup(); let restored = false; let changes = 0;
  const operation = { restoreTaskId: 'task_recovery_original', targetCommit: 'a'.repeat(40), planDigest: 'b'.repeat(64), journalDigest: 'c'.repeat(64), phase: 'exchanging', operationOutcome: 'not_committed', dataState: 'before', documentEpoch: null, canContinue: true, canUndo: false, completionPublished: false, originalRetained: true, retainedPath: '/disposable/recovery-evidence', slots: [], issues: [] };
  globalThis.__UI_TEST_INVOKE__ = async command => {
    if (command === 'project_metadata') return metadata;
    if (command === 'project_git_data_restore_status') return { state: restored ? 'clear' : 'pending', businessBlocked: !restored, operations: restored ? [] : [operation], issues: [] };
    if (command === 'project_git_source_restore_status' || command === 'project_git_data_save_status') return clearRecovery;
    if (command === 'project_git_status') return overviewGitStatus();
    if (command === 'project_git_data_restore_recover') {
      restored = true;
      return { taskId: 'task_recovery_complete', projectId: project.id, type: 'gitDataRestoreContinue', status: 'succeeded', final: true, phase: 'complete', result: { operation: 'data-restore-continue', operationOutcome: 'committed', dataState: 'applied', pending: false, businessBlocked: false }, error: null };
    }
    throw new Error(`Unexpected recovery request: ${command}`);
  };
  await render(h(ProjectOverview, overviewProps({ project: { ...project, buildInfo: { sourceState: 'unknown', dataRecoveryRequired: true } }, onChanged: async () => { changes++; } })));
  assert.equal(changes, 0);
  await click(document.querySelector('[aria-label="业务数据取回恢复"] input[type="checkbox"]'));
  await click(buttonNamed('继续并核对数据取回'));
  assert.equal(changes, 1);
  assert.equal(document.querySelector('[aria-label="业务数据取回恢复"]'), null);
  assert.match(document.querySelector('.build-task').textContent, /数据库和附件已取回/);
});

test('Project overview and Modal preserve metadata and storage choices across pages and require explicit discard to close', async () => {
  await setup(); let guard; let closes = 0;
  globalThis.__UI_TEST_INVOKE__ = async command => {
    if (command === 'project_metadata') return metadata;
    if (command === 'project_git_source_restore_status') return clearRecovery;
    if (command === 'storage_info') return emptyProjectStorage;
    if (command === 'storage_files') return emptyStorageFiles;
    throw new Error(`Unexpected native request: ${command}`);
  };
  const close = () => { if (!guard || guard()) closes++; };
  await render(h(Modal, { title: '项目详情', onClose: close }, h(ProjectOverview, overviewProps({ onDismissGuardChange: value => { guard = value; }, onClose: close }))));
  await click(buttonNamed('编辑信息'));
  const name = document.querySelector('.detail-metadata-form input');
  await inputValue(name, 'Unsaved display name');
  await click(hubTab('空间与文件'));
  await click(buttonNamed('查看可清理内容'));
  await click(document.querySelector('.cleanup-options input'));
  await click(buttonNamed('检查清理范围'));
  await click(document.querySelector('.project-storage-cleanup-details .checkbox-label input'));
  await click(hubTab('概览'));
  assert.equal(name.value, 'Unsaved display name');
  assert.equal(document.querySelector('.cleanup-options input').checked, true);
  assert.ok(document.querySelector('.cleanup-options').closest('[hidden]'));
  assert.equal(document.querySelector('.project-storage-cleanup-details .operation-confirmation'), null);
  await click(hubTab('空间与文件'));
  await click(buttonNamed('检查清理范围'));
  assert.equal(document.querySelector('.project-storage-cleanup-details .checkbox-label input').checked, false);
  await click(hubTab('概览'));
  await key('Escape');
  assert.equal(closes, 0); assert.ok(document.querySelector('[aria-label="关闭详情确认"]'));
  await click(buttonNamed('继续编辑'));
  assert.equal(document.querySelector('[aria-label="关闭详情确认"]'), null);
  assert.equal(name.value, 'Unsaved display name');
  await click(closeButton()); await click(buttonNamed('放弃未提交内容并关闭'));
  assert.equal(closes, 1);
});

test('Project icon chooser blocks closing and ignores a late selected file after unmount', async () => {
  await setup(); const chooser = deferred(); let guard; const requests = [];
  globalThis.__UI_TEST_INVOKE__ = async (command) => {
    requests.push(command);
    if (command === 'project_metadata') return metadata;
    if (command === 'project_git_source_restore_status') return clearRecovery;
    if (command === 'choose_path') return chooser.promise;
    throw new Error(`A closed chooser must not write: ${command}`);
  };
  await render(h(ProjectOverview, overviewProps({ onDismissGuardChange: (value) => { guard = value; } })));
  await click(buttonNamed('编辑信息')); await click(buttonNamed('导入 PNG 图标'));
  let canClose;
  await act(async () => { canClose = guard(); });
  assert.equal(canClose, false);
  assert.ok([...document.querySelectorAll('[role="tab"]')].every((tab) => tab.disabled));
  await render(null);
  await act(async () => chooser.resolve('/disposable/late-image.png'));
  assert.deepEqual(requests, ['project_git_source_restore_status', 'project_metadata', 'choose_path']);
  assert.equal(guard, null);
});

test('Overview tabs support keyboard navigation without opening deletion from the More page', async () => {
  await setup();
  globalThis.__UI_TEST_INVOKE__ = async (command) => {
    if (command === 'project_metadata') return metadata;
    if (command === 'project_git_source_restore_status') return clearRecovery;
    if (command === 'storage_info') return emptyProjectStorage;
    if (command === 'storage_files') return emptyStorageFiles;
    throw new Error(`Unexpected native request: ${command}`);
  };
  await render(h(ProjectOverview, overviewProps({ initialTab: 'manage' })));
  assert.equal(document.querySelector('[aria-label="删除项目确认"]'), null);
  const tabs = [...document.querySelectorAll('[role="tab"]')];
  assert.equal(tabs.filter((tab) => tab.tabIndex === 0).length, 1);
  tabs.at(-1).focus(); await key('Home');
  assert.equal(document.activeElement, tabs[0]); assert.equal(tabs[0].getAttribute('aria-selected'), 'true');
  await key('End');
  assert.equal(document.activeElement, tabs.at(-1)); assert.equal(tabs.at(-1).getAttribute('aria-selected'), 'true');
  assert.equal(document.querySelector('[aria-label="删除项目确认"]'), null);
  await key('ArrowLeft');
  assert.equal(document.activeElement, tabs.at(-2)); assert.equal(tabs.at(-2).getAttribute('aria-selected'), 'true');
  await key('ArrowRight');
  assert.equal(document.activeElement, tabs.at(-1));
  assert.equal(document.querySelector('[aria-label="删除项目确认"]'), null);
});

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
  assert.match(dialog().textContent, /恢复材料也会随项目永久删除/);
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

function hubProps(extra = {}) {
  return { workspace: { workspaceRoot: '/disposable/workspace', migrationState: 'ready', retainedRoots: [] }, revision: 1, enabled: true, migrating: false, projects: [], onTask() {}, onRefresh: async () => {}, ...extra };
}
const workspaceStorage = { platformProgram: { totalBytes: null }, workspace: { totalBytes: null }, otherLocalLocations: [] };
const hubTab = (name) => [...document.querySelectorAll('[role="tab"]')].find((tab) => tab.firstChild?.textContent === name);

test('Space opens on a read-only overview, quick actions focus their tab and only the history tab scans', async () => {
  await setup(); resetHistoryScanForTests(); const requests = [];
  globalThis.__UI_TEST_INVOKE__ = async (command) => {
    requests.push(command);
    if (command === 'workspace_storage') return workspaceStorage;
    if (command === 'workspace_cleanup_preview') return cleanupPreview(cleanupPlan());
    throw new Error(`Navigation must not mutate or open a native chooser: ${command}`);
  };
  await render(h(WorkspaceHub, hubProps()));
  assert.deepEqual(requests, ['workspace_storage']);
  assert.deepEqual([...document.querySelectorAll('[role="tab"]')].map((tab) => tab.firstChild.textContent), ['概览', '按项目', '历史资料']);
  assert.match(document.querySelector('.space-summary').textContent, /历史资料.*尚未扫描/);
  await click(buttonNamed('查看项目'));
  assert.equal(document.activeElement, hubTab('按项目'));
  assert.equal(hubTab('按项目').getAttribute('aria-selected'), 'true');
  assert.deepEqual(requests, ['workspace_storage']);
  await click(hubTab('历史资料'));
  assert.deepEqual(requests, ['workspace_storage', 'workspace_cleanup_preview']);
  assert.equal(document.querySelectorAll('.workspace-cleanup-item').length, 2);
  // Returning keeps the result; the overview now reports it.
  await click(hubTab('概览')); await click(hubTab('历史资料'));
  assert.deepEqual(requests, ['workspace_storage', 'workspace_cleanup_preview']);
  await click(hubTab('概览'));
  assert.match(hubTab('历史资料').textContent, /1\.2 KB/);
  assert.doesNotMatch(document.querySelector('.space-summary').textContent, /尚未扫描/);
});

test('Overview groups measured space without double counting and keeps unknown parts visible', async () => {
  await setup(); resetHistoryScanForTests();
  const data = {
    platformProgram: { totalBytes: 100, complete: true },
    workspace: { totalBytes: 3000, complete: true, categories: { projects: { bytes: 2500 }, operations: { bytes: 500 } } },
    nodeApps: [{ scope: { kind: 'nodeApp', appId: 'node_space_fixture' }, totalBytes: 700, complete: true }],
    otherLocalLocations: [{ scope: { kind: 'applicationData' }, totalBytes: 900, complete: true }],
    knownLocalTotal: { totalBytes: 4000, complete: true },
    disks: [{ exampleRoot: '/disposable/workspace', availableBytes: 5000 }],
  };
  globalThis.__UI_TEST_INVOKE__ = async (command) => { assert.equal(command, 'workspace_storage'); return data; };
  const scanned = { ...project, id: 'prj_space_a', name: 'Space fixture A', storage: { currentProject: { status: 'complete', complete: true, totalBytes: 2500, completedAt: 1, categories: { dependencies: { bytes: 300 }, cache: { bytes: 200 } } } } };
  const unscanned = { ...project, id: 'prj_space_b', name: 'Space fixture B', storage: undefined };
  const nodeApps = [{ id: 'node_space_fixture', name: 'Node space fixture', installation: { kind: 'managed' } }];
  await render(h(WorkspaceHub, hubProps({ projects: [scanned, unscanned], nodeApps })));
  const group = (label) => [...document.querySelectorAll('.space-group')].find((element) => element.querySelector('strong').textContent === label);
  assert.match(document.querySelector('.space-totals').textContent, /共占用4\.0 KB/);
  assert.match(group('我的项目').textContent, /2 个网页项目和 1 个 Node 应用/);
  assert.match(group('我的项目').querySelector('.space-number').textContent, /2\.7 KB/);
  assert.match(group('可重新生成').textContent, /500 B.*|另有 1 个项目尚未统计/);
  assert.match(group('可重新生成').textContent, /另有 1 个项目尚未统计/);
  assert.equal(group('历史资料').querySelector('.space-number').textContent, '尚未扫描');
  assert.match(group('软件本身与记录').textContent, /含尚未扫描的历史资料/);
  assert.equal(group('软件本身与记录').querySelector('.space-number').textContent, '800 B');
  assert.match(document.querySelector('.space-totals').textContent, /磁盘剩余5\.0 KB/);
  assert.deepEqual([...document.querySelectorAll('.space-top-row strong:first-child')].map((element) => element.textContent), ['Space fixture A', 'Node space fixture']);
});

test('Projects tab lists both project types, cleans regenerable files after one confirmation and opens deletion', async () => {
  await setup(); const requests = []; const deleted = []; const tasks = [];
  globalThis.__UI_TEST_INVOKE__ = async (command, args) => {
    requests.push({ command, args });
    if (command === 'workspace_storage') return { ...workspaceStorage, nodeApps: [{ scope: { kind: 'nodeApp', appId: 'node_space_fixture' }, totalBytes: 50 }] };
    if (command === 'storage_cleanup') return { taskId: `task_clean_${args.projectId}`, type: 'storageCleanup', projectId: args.projectId, final: false, status: 'queued', phase: 'queued', result: null };
    if (command === 'storage_scan') return { taskId: `task_scan_${args.projectId}`, type: 'storageScan', projectId: args.projectId, final: false, status: 'queued', phase: 'queued', result: null };
    throw new Error(`Unexpected command: ${command}`);
  };
  const scanned = (id, name, deps) => ({ ...project, id, name, storage: { currentProject: { status: 'complete', complete: true, totalBytes: 1000, completedAt: 1, categories: { dependencies: { bytes: deps }, cache: { bytes: 0 } } } } });
  const projects = [scanned('prj_space_a', 'Space A', 400), scanned('prj_space_b', 'Space B', 0), { ...project, id: 'prj_space_c', name: 'Space C' }];
  const nodeApps = [{ id: 'node_space_fixture', name: 'Node space fixture', installation: null }];
  await render(h(WorkspaceHub, hubProps({ projects, nodeApps, tab: 'projects', isProjectBusy: (value) => value.id === 'prj_space_c' && false, onProjectTask: (task) => tasks.push(task.taskId), onProjectDelete: (value) => deleted.push(value.id), onNodeDelete: (value) => deleted.push(value.id) })));
  const rows = () => [...document.querySelectorAll('.space-project-row:not(.is-header)')];
  assert.equal(rows().length, 4);
  assert.match(rows().find((row) => row.textContent.includes('Node space fixture')).textContent, /Node 应用.*项目文件夹归属待核对|项目文件夹归属待核对.*Node 应用/);
  assert.equal(document.querySelector('[aria-label="清理「Space B」的可重新生成文件"]'), null);
  assert.ok(document.querySelector('[aria-label="统计「Space C」的占用"]'));
  await click(document.querySelector('[aria-label="清理「Space A」的可重新生成文件"]'));
  const dialog = document.querySelector('.space-clean-dialog');
  assert.match(dialog.textContent, /保留：源码、数据库、文件、当前可运行的应用/);
  assert.equal(requests.filter((request) => request.command === 'storage_cleanup').length, 0);
  await click([...dialog.querySelectorAll('button')].find((button) => button.textContent.startsWith('清理 ')));
  assert.deepEqual(requests.filter((request) => request.command === 'storage_cleanup').map((request) => request.args), [{ projectId: 'prj_space_a', input: { categories: ['dependencies', 'buildCache', 'logs'] } }]);
  assert.deepEqual(tasks, ['task_clean_prj_space_a']);
  assert.equal(document.querySelector('.space-clean-dialog'), null);
  await click(document.querySelector('[aria-label="删除「Space B」"]'));
  await click(document.querySelector('[aria-label="删除「Node space fixture」"]'));
  assert.deepEqual(deleted, ['prj_space_b', 'node_space_fixture']);
  await click(buttonNamed('统计全部项目'));
  assert.deepEqual(requests.filter((request) => request.command === 'storage_scan').map((request) => request.args.projectId), ['prj_space_a', 'prj_space_b', 'prj_space_c']);
  await click([...document.querySelectorAll('[role="group"][aria-label="类型"] button')].find((button) => button.textContent.startsWith('Node 应用')));
  assert.equal(rows().length, 1);
});

test('Storage explains retained sizes within their scopes, aggregates old roots, and preserves unknown measurements', async () => {
  await setup();
  let data = {
    platformProgram: { totalBytes: 100, complete: true },
    workspace: { totalBytes: 2000, complete: true, categories: {
      operations: { totalBytes: 1500 }, sharedCache: { totalBytes: null },
    } },
    otherLocalLocations: [
      { scope: { kind: 'retainedWorkspace' }, totalBytes: 1000, complete: true },
      { scope: { kind: 'retainedWorkspace' }, totalBytes: { $int64: '2000' }, complete: true },
      { scope: { kind: 'applicationData' }, totalBytes: 321, complete: true },
    ],
  };
  await render(h(StorageScopeDetails, { data }));
  const rows = () => [...document.querySelectorAll('.workspace-retention-list > div')];
  const row = (prefix) => rows().find((element) => element.querySelector('dt').textContent.startsWith(prefix));
  assert.equal(row('历史保留项目'), undefined);
  assert.equal(row('历史备份'), undefined);
  assert.match(row('共享缓存').textContent, /尚未统计/);
  assert.equal(row('操作记录').querySelector('strong').title, '1500 字节');
  assert.equal(row('迁移前的旧工作区').querySelector('strong').title, '3000 字节');
  assert.equal(row('应用设置').querySelector('strong').title, '321 字节');
  assert.equal(rows().filter((element) => element.querySelector('dt').textContent.startsWith('迁移前的旧工作区')).length, 1);
  assert.match(document.querySelector('.workspace-retention-explanation').textContent, /不再与上方数字相加|不等于可以释放/);
  data = { ...data, workspace: { ...data.workspace, complete: false, stale: true }, otherLocalLocations: [...data.otherLocalLocations, { scope: { kind: 'retainedWorkspace' }, totalBytes: null, complete: false }] };
  await render(h(StorageScopeDetails, { data }));
  assert.match(row('操作记录').textContent, /已过期/);
  assert.match(row('迁移前的旧工作区').textContent, /尚未统计.*部分统计/);
});

function cleanupPlan(extra = {}) {
  return { planId: 'cleanup_plan_dom', expiresAt: Date.now() + 60000, workspaceRoot: '/disposable/workspace', confirmationText: '永久清理所选内容', preserved: ['当前项目'], items: [
    { id: 'retained_fixture', kind: 'retainedWorkspace', label: 'Old disposable snapshot', path: '/disposable/old-workspace', totalBytes: 1234, fileCount: 4, canClean: true, blockedReasons: [] },
    { id: 'retained_protected', kind: 'retainedWorkspace', label: 'Protected old workspace fixture', path: '/disposable/older-workspace', totalBytes: null, fileCount: null, canClean: false, blockedReasons: ['Unresolved recovery still references this copy'] },
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

test('Cleanup lists every history category and shows protected reasons without internal codes', async () => {
  await setup();
  const items = [
    { id: 'datarestore_task_a', kind: 'restoreOriginal', label: '数据取回前的原件', name: 'Fixture project', projectId: 'prj_a', path: '/disposable/workspace/.platform/project-state/data-restore/prj_a/task_a', totalBytes: 10, fileCount: 1, canClean: true, blockedReasons: [] },
    { id: 'install_a', kind: 'restoreOriginal', label: '取回前的原件', name: 'Fixture app', path: '/disposable/workspace/.platform/data-restores/restore_pending/original', totalBytes: 20, fileCount: 2, canClean: false, blockedReasons: ['CLEANUP_ITEM_PROTECTED: 这个恢复操作尚未完成，保留'] },
  ];
  globalThis.__UI_TEST_INVOKE__ = async (command) => { assert.equal(command, 'workspace_cleanup_preview'); return cleanupPreview(cleanupPlan({ items })); };
  await render(h(WorkspaceCleanupPanel, cleanupProps())); await click(buttonNamed('扫描可清理范围'));
  const rows = [...document.querySelectorAll('.workspace-cleanup-item')];
  assert.equal(rows.length, 2);
  assert.deepEqual([...document.querySelectorAll('.history-group h3')].map((heading) => heading.textContent), ['取回前的原件']);
  const blocked = rows.find((row) => row.textContent.includes('这个恢复操作尚未完成'));
  assert.match(blocked.textContent, /这个恢复操作尚未完成/);
  assert.doesNotMatch(blocked.textContent, /CLEANUP_ITEM_PROTECTED/);
  assert.equal(blocked.querySelector('input').disabled, true);
  // Selecting a whole group skips protected items.
  await click(buttonNamed('全选本组'));
  assert.equal(document.querySelectorAll('.workspace-cleanup-item input:checked').length, 1);
  assert.match(document.querySelector('.history-selection-bar').textContent, /已选 1 项/);
});

test('Cleanup controls identify same-name projects and copy or submit only the associated scope', async () => {
  await setup(); const copied = []; const submissions = [];
  const items = ['kept', 'disposable'].map((suffix) => ({ id: `retained_${suffix}`, kind: 'retainedWorkspace', label: '迁移前的旧工作区', name: 'Same display name', projectId: `prj_${suffix}`, path: `/disposable/old-workspace-${suffix}`, totalBytes: 42, fileCount: 1, canClean: true, blockedReasons: [] }));
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
      assert.ok(name.includes('迁移前的旧工作区')); assert.ok(name.includes(items[index].name));
      assert.ok(name.includes(items[index].projectId)); assert.ok(name.includes(items[index].path));
    }
    await click(copy);
  }
  assert.deepEqual(copied, items.map((item) => item.path));
  await click(rows[1].querySelector('input')); await acknowledgeCleanup();
  assert.equal(rows[0].querySelector('input').checked, false);
  await click(confirmCleanupButton());
  assert.deepEqual(submissions, [{ planId: plan.planId, itemIds: ['retained_disposable'], confirmation: plan.confirmationText }]);
});

test('Cleanup review opens a named dialog and cancelling returns focus to the review action', async () => {
  await setup();
  globalThis.__UI_TEST_INVOKE__ = async (command) => { assert.equal(command, 'workspace_cleanup_preview'); return cleanupPreview(cleanupPlan()); };
  const props = cleanupProps();
  await render(h(WorkspaceCleanupPanel, props)); await click(buttonNamed('扫描可清理范围'));
  assert.equal(document.activeElement.id, 'trigger');
  await click(document.querySelector('.workspace-cleanup-item input'));
  const review = buttonNamed('清理所选…'); review.focus(); await click(review);
  const heading = cleanupConfirmation().querySelector('h2');
  assert.match(heading.textContent, /永久清理 1 项历史资料/);
  assert.equal(document.activeElement, heading);
  assert.match(cleanupConfirmation().textContent, /Old disposable snapshot.*1\.2 KB/);
  assert.equal(confirmCleanupButton().disabled, true);
  await click(cleanupConfirmation().querySelector('input[type="checkbox"]'));
  assert.equal(confirmCleanupButton().disabled, false);
  await render(h(WorkspaceCleanupPanel, { ...props }));
  assert.equal(cleanupConfirmation().querySelector('input[type="checkbox"]').checked, true, 'Ordinary rerenders keep the acknowledgement');
  await click(buttonNamed('取消，保留'));
  assert.equal(cleanupConfirmation(), null); assert.equal(document.activeElement, review);
  await click(review);
  assert.equal(cleanupConfirmation().querySelector('input[type="checkbox"]').checked, false, 'Reopening requires a fresh acknowledgement');
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
    await click(confirmCleanupButton()); document.getElementById('trigger').focus();
    const running = { ...oldTask, taskId: 'task_cleanup_new', final: false, status: 'queued', phase: 'queued', result: null };
    await act(async () => accepted.resolve(running));
    const resultHeading = document.querySelector('.workspace-cleanup-result h3');
    assert.equal(document.activeElement, resultHeading); assert.equal(scrolled.at(-1), resultHeading);
    const acceptedScrollCount = scrolled.length;
    document.getElementById('trigger').focus();
    props = { ...props, cleanupTask: { ...running, status: 'running', phase: 'cleaning-retainedWorkspace' } };
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
    const running = { taskId: 'task_cleanup_hidden', type: 'workspaceCleanup', projectId: null, final: false, status: 'running', phase: 'cleaning-retainedWorkspace', result: null };
    const props = cleanupProps({ active: false, cleanupTask: running });
    await render(h(WorkspaceCleanupPanel, props));
    const final = { ...running, final: true, status: 'succeeded', phase: 'completed', result: { completed: true } };
    await render(h(WorkspaceCleanupPanel, { ...props, cleanupTask: final }));
    assert.equal(document.activeElement.id, 'trigger'); assert.equal(scrolled.length, 0);
    await render(h(WorkspaceCleanupPanel, { ...props, active: true, cleanupTask: final }));
    assert.equal(document.activeElement.id, 'trigger'); assert.equal(scrolled.length, 0);
  } finally { HTMLElement.prototype.scrollIntoView = scrollIntoView; }
});

test('Historical cleanup is read-only until selected IDs and the irreversible scope are acknowledged', async () => {
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
  assert.equal(buttonNamed('清理所选…').disabled, true);
  await click(items[0]); await click(buttonNamed('清理所选…'));
  assert.equal(confirmCleanupButton().disabled, true);
  assert.match(cleanupConfirmation().textContent, /disposable\/old-workspace/);
  await click(cleanupConfirmation().querySelector('input[type="checkbox"]'));
  await click(confirmCleanupButton());
  assert.equal(cleanupConfirmation(), null);
  assert.deepEqual(requests[1].args.input, { planId: plan.planId, itemIds: ['retained_fixture'], confirmation: '永久清理所选内容' });
  assert.equal(buttonNamed('清理所选…').disabled, true);
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
  assert.equal(confirmCleanupButton().disabled, false);
  props = { ...props, revision: 2 }; await render(h(WorkspaceCleanupPanel, props));
  assert.equal(cleanupConfirmation(), null); assert.equal(selected().checked, true);
  await acknowledgeCleanup();
  props = { ...props, active: false }; await render(h(WorkspaceCleanupPanel, props));
  props = { ...props, active: true }; await render(h(WorkspaceCleanupPanel, props));
  assert.equal(cleanupConfirmation(), null); assert.equal(selected().checked, true);
  await acknowledgeCleanup();
  props = { ...props, migrating: true }; await render(h(WorkspaceCleanupPanel, props));
  assert.equal(cleanupConfirmation(), null); assert.equal(selected().disabled, true);
  props = { ...props, migrating: false }; await render(h(WorkspaceCleanupPanel, props));
  await click(buttonNamed('取消选择'));
  assert.equal(selected().checked, false); assert.equal(buttonNamed('清理所选…').disabled, true);
});

test('Expired cleanup plans and plans for a different workspace cannot submit destructive actions', async () => {
  await setup(); const plan = cleanupPlan({ expiresAt: Date.now() - 1 }); const requests = [];
  globalThis.__UI_TEST_INVOKE__ = async (command) => { requests.push(command); assert.equal(command, 'workspace_cleanup_preview'); return cleanupPreview(plan); };
  const props = cleanupProps();
  await render(h(WorkspaceCleanupPanel, props)); await click(buttonNamed('扫描可清理范围'));
  assert.equal(document.querySelector('.workspace-cleanup-item input').disabled, true);
  assert.match(document.querySelector('.workspace-cleanup-card').textContent, /范围已过期/);
  assert.equal(buttonNamed('清理所选…').disabled, true);
  await render(h(WorkspaceCleanupPanel, { ...props, workspaceRoot: '/disposable/new-workspace' }));
  assert.match(document.querySelector('.workspace-cleanup-card').textContent, /工作区已改变/);
  assert.deepEqual(requests, ['workspace_cleanup_preview']);
});

test('Historical cleanup shows current per-item results and uncertain lower bounds without a receipt', async () => {
  await setup();
  const task = { taskId: 'task_cleanup_results', type: 'workspaceCleanup', projectId: null, final: true, status: 'failed', phase: 'failed', result: {
    completed: false, items: [
      { id: 'one', kind: 'retainedWorkspace', path: '/disposable/old-one', status: 'removed', completed: true, removed: { removedLogicalBytes: 1234, removedFileCount: 5, completed: true } },
      { id: 'two', kind: 'retainedWorkspace', path: '/disposable/old-two', status: 'failed', completed: false, error: { code: 'TARGET_CHANGED', message: 'Disposable partial fixture', details: { cleanupResult: { removedLogicalBytes: 42, removedFileCount: 1, accountingComplete: false, countsAreLowerBounds: true, remainingPath: '/disposable/old-two' } } } },
      { id: 'three', kind: 'retainedWorkspace', path: '/disposable/retained', status: 'skipped', completed: false, reason: 'A preceding item needs attention' },
    ],
  } };
  await render(h(WorkspaceCleanupPanel, cleanupProps({ cleanupTask: task })));
  const results = [...document.querySelectorAll('.workspace-cleanup-item-result')];
  assert.equal(results.length, 3);
  assert.match(results[0].textContent, /已清理.*实际删除.*5 个/);
  assert.match(results[1].textContent, /已确认至少删除.*42 B.*下限.*待核对的残留目录/);
  assert.match(results[2].textContent, /未处理.*没有继续处理/);
  assert.equal(document.querySelector('.workspace-cleanup-receipt'), null);
});

test('An absent retired workspace reports current absence without claiming a recorded deletion', async () => {
  await setup();
  globalThis.__UI_TEST_INVOKE__ = async (command) => { throw new Error(`Reading locations must not call ${command}`); };
  const workspace = { workspaceRoot: '/disposable/workspace', migrationState: 'ready', retainedRoots: [{ path: '/disposable/old-workspace', available: false, absent: true, cleanup: { status: 'absent', completed: false } }] };
  await render(h(WorkspaceLocation, { workspace, dataRoot: '/disposable/development', enabled: true, active: true, migrating: false, onTask() {}, onRefresh: async () => {} }));
  const record = document.querySelector('.retained-roots');
  assert.match(record.textContent, /目录当前不存在/);
  assert.match(record.textContent, /不保留删除历史/);
  assert.match(record.textContent, /disposable\/old-workspace/);
  assert.equal(document.querySelector('.settings-history-notice'), null, 'A cleaned root is not offered for cleanup again');
  // Current absence is informational, without a checkbox or completion claim.
  globalThis.__UI_TEST_INVOKE__ = async (command) => { assert.equal(command, 'workspace_cleanup_preview'); return cleanupPreview(cleanupPlan({ items: [] })); };
  await render(h(WorkspaceCleanupPanel, cleanupProps({ absentRecords: workspace.retainedRoots }))); await click(buttonNamed('扫描可清理范围'));
  const cleaned = document.querySelector('.workspace-cleanup-item.is-absent');
  assert.match(cleaned.textContent, /disposable\/old-workspace.*目录当前不存在/);
  assert.equal(cleaned.querySelector('input'), null);
});

test('A failed migration submission still settles after leaving and returning to its settings section', async () => {
  await setup(); const result = deferred(); let submissions = 0;
  globalThis.__UI_TEST_INVOKE__ = async (command) => {
    assert.equal(command, 'workspace_migrate'); submissions++; return result.promise;
  };
  const props = { workspace: { workspaceRoot: '/disposable/workspace', migrationState: 'ready', retainedRoots: ['/disposable/older'] }, dataRoot: '/disposable/development', enabled: true, active: true, migrating: false, onTask() {}, onRefresh: async () => {}, onOpenHistory() {} };
  await render(h(WorkspaceLocation, props));
  assert.match(document.querySelector('.settings-history-notice').textContent, /1 个迁移前的旧工作区还没清理/);
  await click(buttonNamed('迁移工作区'));
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

const appProject = (id, name) => ({ ...project, id, name, description: '', projectRoot: `/disposable/workspace/projects/${id}`, manifest: metadata.manifest });
function mockApp(state, custom = async (command) => { throw new Error(`Unexpected native request: ${command}`); }) {
  previewState = state;
  globalThis.__UI_TEST_INVOKE__ = async (command, args) => {
    if (command === 'bootstrap_info') return { displayName: '熊猫小抽屉', version: '0.0.0', identifier: 'dev.happypandaa.pandadrawer', instanceId: state.instanceId ?? 'instance_dom_fixture', workspaceRoot: '/disposable/workspace', dataRoot: '/disposable/development', resources: { sdk: true, template: true, readme: true, cli: true } };
    if (command === 'project_list') return state.projects;
    if (command === 'node_app_list') return state.nodeApps ?? [];
    if (command === 'workspace_info') return { workspaceRoot: '/disposable/workspace', migrationState: 'ready', retainedRoots: [] };
    if (command === 'maintenance_status') return normalStatus;
    if (command === 'project_metadata') return { ...metadata, name: state.projects.find((item) => item.id === args.projectId)?.name ?? 'Removed fixture' };
    if (command === 'project_git_source_restore_status') return clearRecovery;
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
  assert.equal([...actions.querySelectorAll('button')].filter((button) => button.textContent === '进入应用').length, 1);
  assert.equal([...actions.querySelectorAll('button')].some((button) => /浏览器/.test(button.textContent)), false);
  await click(buttonNamed('打开文件夹'));
  assert.deepEqual(folders, [{ path: projectRoot }]);
  const enter = buttonNamed('进入应用');
  await click(enter); await click(enter);
  assert.deepEqual(calls, [{ projectId: 'prj_native_window', target: 'desktop', reload: false }]);
  await act(async () => opened.resolve({ projectId: 'prj_native_window', target: 'desktop', reused: false }));
  assert.match(dialog().textContent, /已在独立窗口打开/);
});

test('App keeps the selected project and its draft after an external list removal, while disabling mutations', async () => {
  await setup();
  const state = { projects: [appProject('prj_A', 'Fixture A')] };
  const projectRoot = state.projects[0].projectRoot; const folders = [];
  mockApp(state, async (command, args) => {
    assert.equal(command, 'open_project_folder'); folders.push(args);
  });
  await render(h(App)); await openDetails('Fixture A');
  await click(buttonNamed('编辑信息'));
  await inputValue(document.querySelector('.detail-metadata-form input'), 'Retain this edited display name');
  await click(hubTab('空间与文件'));
  state.projects = [];
  await click(buttonNamed('重新扫描'));
  assert.ok(dialog(), 'An external disappearance must not discard the detail editor');
  assert.match(dialog().textContent, /项目已不在当前工作区列表中/);
  assert.ok([...document.querySelectorAll('.detail-action-row button')].filter(button => button.textContent !== '打开文件夹').every(button => button.disabled));
  assert.equal(buttonNamed('打开文件夹').disabled, false);
  await click(buttonNamed('打开文件夹'));
  assert.deepEqual(folders, [{ path: projectRoot }]);
  await click(hubTab('概览'));
  assert.equal(document.querySelector('.detail-metadata-form input').value, 'Retain this edited display name');
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
  await click([...document.querySelectorAll('.radio-toolbar button')].find((button) => button.textContent.trim() === '新建项目'));
  await inputValue(document.getElementById('project-name'), created.name);
  await submit(document.querySelector('.create-project-form'));
  assert.ok(dialog()); assert.match(dialog().textContent, /New disposable fixture/);
  assert.equal(document.querySelector('.create-project-form'), null);
  assert.equal(handoffs, 0); assert.deepEqual(copied, []);
  assert.equal(buttonNamed('复制提示词').disabled, false);
  assert.equal(buttonNamed('在 Codex 打开'), undefined);
  assert.equal(buttonNamed('构建更新').disabled, false);
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

test('A refreshed project removal invalidates an in-flight AI handoff without discarding the detail draft', async () => {
  await setup(); const prepared = deferred(); const copied = [];
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async (text) => { copied.push(text); } } });
  const state = { projects: [appProject('prj_A', 'Fixture A')] };
  mockApp(state, async (command) => { assert.equal(command, 'project_handoff'); return prepared.promise; });
  await render(h(App)); await openDetails('Fixture A');
  await click(buttonNamed('编辑信息'));
  await inputValue(document.querySelector('.detail-metadata-form input'), 'Preserve handoff fixture draft');
  await click(buttonNamed('复制提示词'));
  await click(hubTab('空间与文件')); state.projects = [];
  await click(buttonNamed('重新扫描'));
  await act(async () => prepared.resolve({ text: 'Removed project must not be copied' }));
  assert.deepEqual(copied, []); assert.equal(document.querySelector('[aria-label="给 AI 的完整说明"]'), null);
  assert.equal(buttonNamed('复制提示词').disabled, true);
  await click(hubTab('概览'));
  assert.equal(document.querySelector('.detail-metadata-form input').value, 'Preserve handoff fixture draft');
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
  assert.doesNotMatch(dialog().textContent, /Codex|完成后构建更新/);
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
    await click(hubTab('空间与文件')); await click(buttonNamed('重新扫描'));
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
  await render(h(App)); await openDetails(project.name); await click(buttonNamed('构建更新'));
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 1100)); });
  assert.match(dialog().textContent, /Explicit build failure must stay visible/);
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 1100)); });
  assert.match(dialog().textContent, /Explicit build failure must stay visible/);
  assert.equal(buttonNamed('构建更新').disabled, false);
  await click(buttonNamed('构建更新'));
  assert.equal(submissions, 2);
  assert.doesNotMatch(dialog().textContent, /Explicit build failure must stay visible/);
  assert.match(dialog().querySelector('.project-last-task').textContent, /新网页已发布/);
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
  assert.doesNotMatch(dialog().textContent, /复制提示词|在 Codex 打开|构建更新/);
  assert.match(dialog().textContent, /永久删除.*无法撤销/);
  assert.match(dialog().textContent, /恢复、任务材料及迁移前的项目副本/);
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
    await click(document.querySelector('.radio-task-button'));
    assert.match(document.querySelector('.task-center-panel').textContent, /暂无任务/);
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
  await click(document.querySelector('[aria-label="刷新项目和运行状态"]'));
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

test('The nested deletion dialog cancels back to the same details and preserves metadata and storage drafts', async () => {
  await setup();
  mockApp({ projects: [appProject('prj_nested_delete', 'Nested deletion fixture')] });
  await render(h(App)); await openDetails('Nested deletion fixture');
  await click(buttonNamed('编辑信息'));
  await inputValue(document.querySelector('.detail-metadata-form input'), 'Keep this unsaved name');
  await click(hubTab('空间与文件')); await click(buttonNamed('查看可清理内容'));
  await click(document.querySelector('.cleanup-options input'));
  await click(hubTab('更多操作'));
  const trigger = buttonNamed('删除应用'); trigger.focus(); await click(trigger);
  const deleteDialog = [...document.querySelectorAll('[role="dialog"]')].at(-1);
  assert.equal(document.querySelectorAll('[role="dialog"]').length, 2);
  assert.ok(deleteDialog.contains(document.activeElement));
  assert.equal(deleteDialog.querySelector('[role="tablist"]'), null);
  await click(document.querySelector('.project-delete-confirmation input'));
  await click(buttonNamed('取消，保留项目'));
  assert.equal(document.querySelectorAll('[role="dialog"]').length, 1);
  assert.equal(document.activeElement, trigger);
  await click(hubTab('概览'));
  assert.equal(document.querySelector('.detail-metadata-form input').value, 'Keep this unsaved name');
  await click(hubTab('空间与文件'));
  assert.equal(document.querySelector('.cleanup-options input').checked, true);
  await click(hubTab('更多操作')); await click(buttonNamed('删除应用'));
  assert.equal(document.querySelector('.project-delete-confirmation input').checked, false);
  await key('Escape');
  assert.equal(document.querySelectorAll('[role="dialog"]').length, 1);
  assert.equal(document.querySelector('[aria-label="关闭详情确认"]'), null);
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


async function managerDeletion(value) {
  const listener = globalThis.__UI_EVENT_LISTENERS__?.get('panda-project-deleted');
  assert.ok(listener, 'Manager must install its native deletion listener before listing projects');
  await act(async () => listener({ payload: value }));
}
const managerRemoved = (id, extra = {}) => ({ version: 1, kind: 'web', id, taskIds: [], sourceIds: [], sourceRoots: [], instanceId: 'instance_dom_fixture', ...extra });
test('A CLI deletion notification rejects a late project list and keeps another project metadata draft', async () => {
  await setup(); const a = appProject('prj_cli_A', 'CLI removed A'); const b = appProject('prj_cli_B', 'Preserved B');
  const state = { projects: [a, b] }; mockApp(state);
  await render(h(App)); await openDetails(b.name); await click(buttonNamed('编辑信息'));
  await inputValue(document.querySelector('.detail-metadata-form input'), 'Keep unrelated B draft');
  const original = globalThis.__UI_TEST_INVOKE__; const stale = deferred(); let delay = true;
  globalThis.__UI_TEST_INVOKE__ = (command, args) => command === 'project_list' && delay ? (delay = false, stale.promise) : original(command, args);
  await click(document.querySelector('[aria-label="刷新项目和运行状态"]'));
  state.projects = [b];
  await managerDeletion(managerRemoved(a.id, { sourceRoots: [a.projectRoot] }));
  await act(async () => stale.resolve([a, b]));
  assert.equal(document.querySelector(`[aria-label="进入项目「${a.name}」"]`), null);
  assert.equal(document.querySelector('.detail-metadata-form input').value, 'Keep unrelated B draft');
  await managerDeletion(managerRemoved(b.id, { instanceId: 'another-runtime' }));
  assert.equal(document.querySelector('.detail-metadata-form input').value, 'Keep unrelated B draft');
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
  await click(document.querySelector('[aria-label="刷新项目和运行状态"]'));
  state.projects = [b]; await managerDeletion(managerRemoved(a.id));
  await act(async () => stale.resolve({ instanceId: 'obsolete-bootstrap-runtime', workspaceRoot: '/obsolete-workspace' }));
  assert.equal(projectWasDeleted(a.id), true);
  assert.equal(document.querySelector(`[aria-label="进入项目「${a.name}」"]`), null);
  assert.ok(document.querySelector(`[aria-label="进入项目「${b.name}」"]`));
});
