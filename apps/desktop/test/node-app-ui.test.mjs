// Management form regressions with artificial credentials only. These tests do
// not open a native app, system credential store, browser, or external service.
import assert from 'node:assert/strict';
import { after, afterEach, test } from 'node:test';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';
import { JSDOM } from 'jsdom';

const repo = fileURLToPath(new URL('../../../', import.meta.url));
await mkdir(path.join(repo, '.test-artifacts'), { recursive: true });
const output = await mkdtemp(path.join(repo, '.test-artifacts/node-form-dom-'));
const mockPath = path.join(output, 'tauri-mock.mjs');
// These unrelated interaction fixtures have no interrupted Git additions.
await writeFile(mockPath, 'export const invoke = (command, ...args) => command === "source_import_recover" ? Promise.resolve([]) : globalThis.__NODE_FORM_INVOKE__(command, ...args); export const isTauri = () => true; export const getAllWindows = async () => globalThis.__UI_WINDOWS__ ?? []; export const listen = async (name, callback) => { const listeners = globalThis.__NODE_EVENT_LISTENERS__ ??= new Map(); listeners.set(name, callback); return () => listeners.delete(name); };\n');
const compiled = new Map();
async function component(name) {
  async function compile(file) {
    if (compiled.has(file)) return compiled.get(file);
    const target = path.join(output, `${path.basename(file).replace(/\.tsx?$/, '')}.mjs`);
    compiled.set(file, target);
    let source = ts.transpileModule(await readFile(file, 'utf8'), { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX }, fileName: file }).outputText;
    for (const match of [...source.matchAll(/from\s+(['"])(\.[^'"]+)\1/g)]) {
      const base = path.resolve(path.dirname(file), match[2]);
      let dependency;
      for (const candidate of [base, `${base}.tsx`, `${base}.ts`]) {
        try { await readFile(candidate); dependency = candidate; break; } catch { /* Try the next extension. */ }
      }
      assert.ok(dependency);
      source = source.replaceAll(`${match[1]}${match[2]}${match[1]}`, JSON.stringify(pathToFileURL(await compile(dependency)).href));
    }
    source = source.replaceAll(/(['"])@tauri-apps\/api\/(?:core|event|window)\1/g, JSON.stringify(pathToFileURL(mockPath).href));
    await writeFile(target, source);
    return target;
  }
  return import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src', name))).href);
}

const dom = new JSDOM('<!doctype html><html><body></body></html>', { url: 'http://localhost/', pretendToBeVisual: true });
for (const name of ['window', 'document', 'Node', 'HTMLElement', 'HTMLInputElement', 'HTMLSelectElement', 'Event', 'MouseEvent', 'KeyboardEvent', 'FocusEvent', 'CustomEvent', 'MutationObserver']) Object.defineProperty(globalThis, name, { value: name === 'window' ? dom.window : dom.window[name], configurable: true });
Object.defineProperty(globalThis, 'navigator', { value: dom.window.navigator, configurable: true });
globalThis.getComputedStyle = dom.window.getComputedStyle.bind(dom.window);
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
globalThis.__NODE_FORM_INVOKE__ = async (command) => { throw new Error(`Unexpected native request: ${command}`); };
const { createElement: h, act } = await import('react');
const { createRoot } = await import('react-dom/client');
const { NodeAppsPanel, nodeConfigurationInput } = await component('NodeAppsPanel.tsx');
const { LocalDirectoryPanel } = await component('LocalDirectoryPanel.tsx');
const { default: App } = await component('App.tsx');
const { FrontendPlatformProvider, frontendPlatformFromBootstrap } = await component('frontend-platform.tsx');
const { acceptProjectDeletion, observeDeletionRuntime, resetProjectDeletionsForTests } = await component('project-deletion.ts');
const { readRecentProjectFolders, saveRecentProjectFolder, resetRecentProjectFoldersForTests } = await component('local-directory-review.ts');
let root;
afterEach(async () => { if (root) await act(async () => root.unmount()); root = null; resetProjectDeletionsForTests(); resetRecentProjectFoldersForTests(); dom.window.localStorage.clear(); delete navigator.clipboard; globalThis.__NODE_FORM_INVOKE__ = async (command) => { throw new Error(`Unexpected native request: ${command}`); }; });
after(async () => { dom.window.close(); await rm(output, { recursive: true, force: true }); });

function field(name, type, extra = {}) { return { name, type, label: `设置 ${name}`, required: false, secret: false, description: `人工说明 ${name}`, howToGet: [`人工步骤 ${name}`], ...extra }; }
const fields = [
  field('AI_URL', 'url', { required: true, default: 'https://example.invalid/api' }),
  field('AI_KEY', 'string', { required: true, secret: true, endpointField: 'AI_URL', helpUrl: 'https://example.invalid/help' }),
  field('MODEL', 'string', { default: 'test-model' }),
  field('LIMIT', 'integer', { default: 7 }),
  field('ENABLED', 'boolean', { default: true }),
  field('MODE', 'select', { default: 'one', options: [{ label: '第一个', value: 'one' }, { label: '第二个', value: 'two' }] }),
];
function app(extra = {}) { return { id: 'node_fixture', name: 'Disposable Node fixture', sourcePath: '/not-a-real-source', runtime: 'node', state: 'stopped', restartRequired: false, runningRevision: null, lastError: null, configuration: { revision: 1, values: {}, secretStatus: { AI_KEY: 'set' }, missingRequired: [], canStart: true }, fields, ...extra }; }
async function setup(mock, props = {}, Component = NodeAppsPanel) {
  document.body.innerHTML = '<div id="root"></div>';
  root = createRoot(document.getElementById('root'));
  const { deletionTasksMock, workspaceStorageMock, frontendPlatform, ...componentProps } = props;
  const previews = new Map();
  globalThis.__NODE_FORM_INVOKE__ = async (command, args) => {
    if (command === 'workspace_storage') return workspaceStorageMock ? workspaceStorageMock(args) : { workspace: {}, nodeApps: [], otherLocalLocations: [] };
    if (command === 'node_app_delete_tasks') return deletionTasksMock ? deletionTasksMock(args) : [];
    const value = await mock(command, args);
    if (command === 'node_app_delete_preview') previews.set(args.appId, value);
    if (command === 'node_app_delete_submit' && value && !value.taskId && Object.hasOwn(value, 'operationOutcome')) {
      const preview = previews.get(args.appId);
      return deletionTask({ id: value.appId, name: preview?.name ?? 'Disposable Node fixture' }, { targetRevision: preview?.expectedRevision, ownership: value.ownership, phase: value.operationOutcome === 'deleted' ? 'deleted' : 'failed', remaining: value.operationOutcome === 'deleted' ? [] : deletionTask().remaining, canRetry: value.operationOutcome !== 'deleted', canAbandon: value.operationOutcome !== 'deleted', result: value.operationOutcome === 'deleted' ? value : null });
    }
    return value;
  };
  const rendered = h(Component, { enabled: true, onDraftStateChange() {}, ...(Component === NodeAppsPanel ? { request: { app: app(), intent: 'details', token: 1 } } : {}), ...componentProps });
  await act(async () => root.render(frontendPlatform ? h(FrontendPlatformProvider, { value: frontendPlatform }, rendered) : rendered));
  if (Component === NodeAppsPanel && props.request !== null && !['delete', 'delete-task'].includes(props.request?.intent) && document.querySelector('[role=dialog]') && byText('修改')) await click(byText('修改'));
}
function byText(text, selector = 'button') { return [...document.querySelectorAll(selector)].find((element) => element.textContent === text); }
function input(name) { const label = [...document.querySelectorAll('.node-field-label')].find((element) => element.textContent === `设置 ${name}`); assert.ok(label, name); return document.getElementById(label.htmlFor); }
function byLabel(label) { return [...document.querySelectorAll('[aria-label]')].find((element) => element.getAttribute('aria-label') === label); }
async function change(element, value) {
  assert.ok(element);
  await act(async () => {
    const prototype = element.tagName === 'SELECT' ? window.HTMLSelectElement.prototype : window.HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(prototype, 'value').set.call(element, value);
    element.dispatchEvent(new window.Event(element.tagName === 'SELECT' ? 'change' : 'input', { bubbles: true }));
  });
}
async function click(element) { assert.ok(element); await act(async () => element.click()); }
async function openDetails(name) {
  const entry = document.querySelector(`[aria-label='进入项目「${name}」']`);
  assert.ok(entry);
  await act(async () => entry.closest('article').dispatchEvent(new window.MouseEvent('contextmenu', { bubbles: true, cancelable: true })));
  await click(document.querySelector(`[aria-label='查看项目「${name}」的详情']`));
}
async function saveAndContinueEditing() { await act(async () => document.querySelector('.node-configuration-form').dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true }))); if (!document.querySelector('.node-configuration-form') && byText('修改')) await click(byText('修改')); }
async function showDetails() { if (byText('返回详情')) await click(byText('返回详情')); }
function deferred() { let resolve; let reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { resolve, reject, promise }; }
function baseMock(value, action) { return async (command, args) => { if (command === 'node_app_list') return [value]; if (command === 'node_app_configuration') return value; return action(command, args); }; }

function deletionPreview(value, ownership = 'managed', expectedRevision = 'fresh-disposable-revision') {
  return { appId: value.id, name: value.name, ownership, expectedRevision, platformRoot: `/disposable/platform/${value.id}`, sourcePath: '/disposable/original-source', deletePaths: [`/disposable/platform/${value.id}`, `/disposable/tasks/${value.id}`, ...(ownership === 'managed' ? ['/disposable/original-source'] : [])], preservedPaths: ownership === 'external' ? ['/disposable/original-source'] : [] };
}


function deletionTask(value = app(), extra = {}) {
  const preview = deletionPreview(value);
  return { taskId: `disposable-task-${value.id}`, appId: value.id, name: value.name, targetRevision: preview.expectedRevision, journalRevision: 1, phase: 'waiting', active: !['failed', 'deleted', 'abandoned'].includes(extra.phase ?? 'waiting'), ownership: preview.ownership, sourcePath: preview.sourcePath, waitingFor: [], renamedAny: false, canAbandon: true, canRetry: false, abandonRequested: false, targets: preview.deletePaths.map(path => ({ path, stagedPath: `${path}-staged`, state: 'planned' })), remaining: preview.deletePaths.map(path => ({ path, stagedPath: `${path}-staged`, state: 'planned' })), preservedPaths: preview.preservedPaths, error: null, result: null, ...extra };
}

function deleteDialog() {
  const confirmation = document.querySelector('.node-delete-confirmation');
  assert.ok(confirmation, 'Deletion needs its own confirmation');
  const dialog = confirmation.closest('[role=dialog]');
  assert.ok(dialog, 'Deletion confirmation must be inside a dialog');
  assert.equal(dialog.querySelector('[role=tablist]'), null);
  assert.equal(dialog.querySelector('.node-configuration-form'), null);
  assert.equal(dialog.querySelector('.node-runtime-actions'), null);
  assert.equal(dialog.querySelector('.node-install-panel'), null);
  return dialog;
}

function deletionProgress(title) {
  const progress = deleteDialog().querySelector('.delete-progress[role="status"]');
  assert.ok(progress, 'Pending deletion needs a visible status');
  assert.equal(progress.closest('[hidden]'), null);
  assert.equal(progress.getAttribute('aria-live'), 'polite');
  assert.equal(progress.getAttribute('aria-atomic'), 'true');
  assert.equal(progress.closest('[aria-busy="true"]'), null, 'The live status remains outside busy review content');
  assert.equal(progress.querySelector('.delete-progress-spinner')?.getAttribute('aria-hidden'), 'true');
  assert.ok(progress.textContent.includes(title), `Missing deletion stage: ${title}`);
  return progress;
}

async function withDeletionStatusTimers(run) {
  const originalInterval = window.setInterval; const originalClear = window.clearInterval; const timers = new Map(); let timerId = 1_000_000;
  window.setInterval = (callback, delay, ...args) => {
    if (delay !== 1000) return originalInterval.call(window, callback, delay, ...args);
    const id = ++timerId; timers.set(id, () => callback(...args)); return id;
  };
  window.clearInterval = id => { if (!timers.delete(id)) originalClear.call(window, id); };
  try { await run({ tick: () => act(async () => { for (const callback of [...timers.values()]) callback(); }), count: () => timers.size }); }
  finally {
    if (root) await act(async () => root.unmount()); root = null;
    window.setInterval = originalInterval; window.clearInterval = originalClear;
  }
}

test('Node deletion reads ownership-specific scope, requires acknowledgment and sends no filesystem paths', async () => {
  for (const ownership of ['managed', 'external']) {
    const value = app(); const preview = deletionPreview(value, ownership); const finished = deferred(); const calls = []; const deleted = [];
    await setup(async (command, args) => {
      calls.push([command, args]);
      if (command === 'node_app_delete_preview') return preview;
      if (command === 'node_app_delete_submit') return finished.promise;
      throw new Error(command);
    }, { instanceId: 'same', request: { app: value, intent: 'delete', token: 1 }, onAppDeleted: id => deleted.push(id) });
    assert.equal(document.querySelectorAll('[role=dialog]').length, 1);
    assert.equal(document.querySelector('.node-detail-modal'), null, 'Launcher deletion must not open project details');
    const dialog = deleteDialog();
    const confirmation = document.querySelector('[aria-label="永久删除 Node 项目确认"]');
    assert.match(confirmation.textContent, /不会为本次删除保存可恢复副本/);
    assert.match(confirmation.textContent, /会关闭应用窗口，未保存的内容将丢失/);
    assert.doesNotMatch(confirmation.querySelector('.checkbox-label').textContent, /我已关闭项目窗口/);
    assert.match(confirmation.textContent, ownership === 'managed' ? /将永久删除本项目的全部平台管理内容/ : /项目代码、Git 和 data\/ 文件数据保留/);
    assert.match(confirmation.textContent, /浏览器数据/);
    const listed = [...confirmation.querySelectorAll('[data-path]')].map(element => element.getAttribute('data-path'));
    for (const path of [...preview.deletePaths, ...preview.preservedPaths]) assert.ok(listed.includes(path), `missing ${path}`);
    assert.match(confirmation.querySelector('.node-delete-base').textContent, /\/disposable\//);
    assert.equal(confirmation.querySelector('.node-delete-kept').textContent.includes(preview.sourcePath), ownership === 'external');
    assert.equal(byText('确认永久删除项目').disabled, true);
    assert.deepEqual(calls, [['node_app_delete_preview', { appId: value.id }]]);
    await click(confirmation.querySelector('input[type=checkbox]'));
    await click(byText('确认永久删除项目'));
    deletionProgress('正在永久删除…');
    assert.equal(confirmation.querySelector('.node-delete-body')?.hidden ?? true, true, 'The review is hidden while deletion runs');
    assert.equal(confirmation.querySelector('input[type=checkbox]').disabled, true);
    assert.equal(byText('正在永久删除…').disabled, true);
    assert.equal(dialog.querySelector('.modal-close').disabled, true);
    assert.equal(byText('取消，保留项目').disabled, true);
    await act(async () => document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true })));
    await click(dialog.closest('.modal-layer'));
    assert.equal(deleteDialog(), dialog, 'Escape and the backdrop cannot close a pending deletion');
    await click(byText('正在永久删除…'));
    assert.deepEqual(calls.at(-1), ['node_app_delete_submit', { appId: value.id, input: { expectedRevision: preview.expectedRevision, force: false } }]);
    assert.equal(calls.filter(([command]) => command === 'node_app_delete_submit').length, 1);
    await act(async () => finished.resolve({ appId: value.id, operationOutcome: 'deleted', ownership, sourcePreserved: ownership === 'external', evidenceRetained: false }));
    assert.deepEqual(deleted, [value.id]); assert.equal(document.querySelector('[role=dialog]'), null);
    await act(async () => root.unmount()); root = null;
  }
});

test('Pending Node deletion shows journal stages, permits a safe abandon request and keeps its submission gate', async () => {
  await withDeletionStatusTimers(async ({ tick, count }) => {
    const value = app({ state: 'running' }); const calls = [];
    let status = deletionTask(value, { waitingFor: ['Git 操作', '源码发布', 'Git 操作'] });
    await setup(async (command, args) => {
      calls.push([command, args]);
      if (command === 'node_app_delete_preview') return deletionPreview(value);
      if (command === 'node_app_delete_submit' || command === 'node_app_delete_status') return status;
      throw new Error(command);
    }, { instanceId: 'same', request: { app: value, intent: 'delete', token: 1 } });
    await click(document.querySelector('.node-delete-confirmation input')); await click(byText('确认永久删除项目'));
    assert.equal(count(), 1);
    assert.equal(deletionProgress('正在永久删除…').querySelector('.delete-progress-stage').textContent, '等待Git 操作、源码发布完成');
    await tick();
    assert.deepEqual(calls.at(-1), ['node_app_delete_status', { taskId: status.taskId, appId: value.id }]);
    assert.equal(byText('放弃删除').disabled, false);
    assert.equal(byText('关闭').disabled, true);
    assert.equal(deleteDialog().querySelector('.modal-close').disabled, true);
    assert.equal(document.querySelector('.node-delete-confirmation input'), null);
    assert.equal(calls.filter(([command]) => command === 'node_app_delete_submit').length, 1);
    status = { ...status, journalRevision: 2, phase: 'stopping', waitingFor: [] }; await tick();
    assert.equal(deletionProgress('正在永久删除…').querySelector('.delete-progress-stage').textContent, '正在关闭窗口并停止应用…');
    status = { ...status, journalRevision: 3, phase: 'staging', canAbandon: false }; await tick();
    assert.equal(deletionProgress('正在永久删除…').querySelector('.delete-progress-stage').textContent, '正在准备删除文件…');
    status = { ...status, journalRevision: 4, phase: 'removing', renamedAny: true }; await tick();
    assert.equal(deletionProgress('正在永久删除…').querySelector('.delete-progress-stage').textContent, '正在删除文件…');
    assert.equal(byText('放弃删除'), undefined);
    status = { ...status, journalRevision: 5, phase: 'deleted', remaining: [], result: { appId: value.id, operationOutcome: 'deleted', ownership: 'managed', sourcePreserved: false, evidenceRetained: false } }; await tick();
    assert.equal(document.querySelector('[role=dialog]'), null); assert.equal(count(), 0);
  });
});

test('A removed successful journal is accepted through an absent retry receipt without reviving the task', async () => {
  await withDeletionStatusTimers(async ({ tick }) => {
    const value = app(); const deleted = [];
    const task = deletionTask(value, { journalRevision: 7, phase: 'removing', renamedAny: true, canAbandon: false });
    const absent = { ...task, journalRevision: 1, targetRevision: 'absent', ownership: 'absent', sourcePath: '', phase: 'deleted', active: false, renamedAny: false, targets: [], remaining: [], preservedPaths: [], canRetry: false,
      result: { appId: value.id, operationOutcome: 'deleted', ownership: 'absent', alreadyAbsent: true, sourcePreserved: false, evidenceRetained: false } };
    await setup(async command => {
      assert.equal(command, 'node_app_delete_status'); return absent;
    }, { instanceId: 'same', request: { app: value, intent: 'delete-task', task, token: 1 }, onAppDeleted: id => deleted.push(id) });
    await tick();
    assert.deepEqual(deleted, [value.id]);
    assert.equal(document.querySelector('[role=dialog]'), null);
  });
});

test('Journal observations coalesce and a late failed snapshot cannot undo an accepted retry', async () => {
  await withDeletionStatusTimers(async ({ tick }) => {
    const value = app(); const observation = deferred(); let reads = 0; const writes = [];
    const failed = deletionTask(value, { phase: 'failed', journalRevision: 2, canRetry: true, error: { code: 'DISPOSABLE_STOP_FAILED', message: 'Disposable application could not stop' } });
    const retrying = { ...failed, phase: 'waiting', journalRevision: 3, canRetry: false, error: null, waitingFor: ['安全收尾'] };
    await setup(async (command, args) => {
      if (command === 'node_app_delete_status') { reads++; return observation.promise; }
      if (command === 'node_app_delete_retry') { writes.push(args); return retrying; }
      throw new Error(command);
    }, { instanceId: 'same', request: { app: value, intent: 'delete-task', task: failed, token: 1 } });
    const feedback = deleteDialog().querySelector('.node-delete-footer .node-delete-error');
    assert.match(feedback.textContent, /Disposable application could not stop/); assert.equal(document.activeElement, feedback);
    await tick(); await tick(); assert.equal(reads, 1, 'One unresolved observation cannot start overlapping reads');
    await click(byText('重试删除'));
    assert.deepEqual(writes, [{ taskId: failed.taskId, appId: value.id, observedRevision: 2 }]);
    await act(async () => observation.resolve(failed));
    assert.equal(deleteDialog().querySelector('.node-delete-error'), null);
    assert.equal(deletionProgress('正在永久删除…').querySelector('.delete-progress-stage').textContent, '等待安全收尾完成');
    assert.equal(byText('关闭').disabled, true);
  });
});

test('A late journal snapshot cannot contaminate a new app confirmation after the old deletion completes', async () => {
  await withDeletionStatusTimers(async ({ tick }) => {
    const first = app(); const next = app({ id: 'another-disposable-node', name: 'Another disposable Node' }); const observation = deferred();
    const failed = deletionTask(first, { phase: 'failed', canRetry: true });
    const props = { enabled: true, instanceId: 'same', onDraftStateChange() {}, request: { app: first, intent: 'delete-task', task: failed, token: 1 } };
    await setup(async (command, args) => {
      if (command === 'node_app_delete_preview') return deletionPreview(next);
      if (command === 'node_app_delete_retry') return { ...failed, journalRevision: 2, phase: 'deleted', remaining: [], canRetry: false, canAbandon: false, result: { appId: first.id, operationOutcome: 'deleted', ownership: 'managed', sourcePreserved: false, evidenceRetained: false } };
      if (command === 'node_app_delete_status') { assert.equal(args.appId, first.id); return observation.promise; }
      throw new Error(command);
    }, props);
    await tick(); await click(byText('重试删除'));
    await act(async () => root.render(h(NodeAppsPanel, { ...props, request: { app: next, intent: 'delete', token: 2 } })));
    assert.match(deleteDialog().textContent, /Another disposable Node/);
    await act(async () => observation.resolve({ ...failed, journalRevision: 3, phase: 'removing', renamedAny: true, canAbandon: false }));
    assert.equal(deleteDialog().querySelector('.delete-progress'), null);
    assert.doesNotMatch(deleteDialog().textContent, /正在删除文件/);
    assert.equal(document.querySelector('.node-delete-confirmation input').checked, false);
    assert.equal(byText('确认永久删除项目').disabled, true);
  });
});

test('A pending journal abandon holds every close path until the backend confirms abandoned', async () => {
  await withDeletionStatusTimers(async () => {
    const value = app(); const abandoned = deferred(); const writes = []; const finished = []; const states = []; const deleted = [];
    const task = deletionTask(value, { waitingFor: ['Git 操作'] });
    await setup(async (command, args) => {
      if (command === 'node_app_delete_abandon') { writes.push(args); return abandoned.promise; }
      throw new Error(command);
    }, { instanceId: 'same', request: { app: value, intent: 'delete-task', task, token: 1 }, onDeletionAbandoned: snapshot => finished.push(snapshot), onAppDeleted: id => deleted.push(id), onDraftStateChange: state => states.push(state) });
    await click(byText('放弃删除'));
    assert.deepEqual(writes, [{ taskId: task.taskId, appId: task.appId, observedRevision: 1 }]);
    assert.equal(states.at(-1).pending, true);
    assert.match(deletionProgress('正在放弃删除…').textContent, /正在放弃删除，等待安全收尾/);
    assert.equal(byText('正在放弃删除…').disabled, true);
    assert.equal(byText('关闭').disabled, true); assert.equal(deleteDialog().querySelector('.modal-close').disabled, true);
    await act(async () => document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true })));
    await click(deleteDialog().closest('.modal-layer'));
    assert.ok(document.querySelector('[role=dialog]')); assert.equal(writes.length, 1); assert.deepEqual(finished, []);
    const receipt = { ...task, journalRevision: 2, phase: 'abandoned', canAbandon: false, canRetry: false };
    await act(async () => abandoned.resolve(receipt));
    assert.deepEqual(finished, [receipt]); assert.deepEqual(deleted, []);
    assert.equal(document.querySelector('[role=dialog]'), null); assert.equal(states.at(-1).pending, false);
  });
});

test('A failed journal offers retry and conditional abandon beside its actual remaining paths', async () => {
  await withDeletionStatusTimers(async ({ tick }) => {
    const value = app(); const task = deletionTask(value, { phase: 'failed', journalRevision: 7, renamedAny: true, canAbandon: false, canRetry: true,
      error: { code: 'DISPOSABLE_REMOVE_FAILED', message: 'Disposable staged directory remains' }, remaining: [{ path: '/disposable/original/data', stagedPath: '/disposable/pending/data', state: 'removing', reason: 'Disposable file is unavailable' }] });
    await setup(async command => {
      assert.equal(command, 'node_app_delete_status');
      return { ...task, journalRevision: 8, renamedAny: false, canAbandon: true };
    }, { instanceId: 'same', request: { app: value, intent: 'delete-task', task, token: 1 } });
    const footer = deleteDialog().querySelector('.node-delete-footer');
    assert.match(footer.querySelector('[role=alert]').textContent, /Disposable staged directory remains.*DISPOSABLE_REMOVE_FAILED/);
    assert.match(deleteDialog().querySelector('[aria-label="删除任务实际范围"]').textContent, /\/disposable\/original\/data.*\/disposable\/pending\/data.*Disposable file is unavailable/);
    assert.equal(byText('重试删除').disabled, false); assert.equal(byText('放弃删除'), undefined);
    assert.match(footer.textContent, /已有内容进入待清理位置，不能再放弃/);
    await tick();
    assert.equal(byText('放弃删除'), undefined, 'A later response cannot reverse the durable renamedAny boundary');
    assert.match(deleteDialog().textContent, /删除任务状态无法核实/);
    assert.equal(byText('重试删除').disabled, false);
  });
});

test('An accepted nonterminal abandon stays busy across a failed phase until a durable abandoned receipt arrives', async () => {
  await withDeletionStatusTimers(async ({ tick }) => {
    const value = app(); const finished = []; const states = []; let writes = 0;
    const original = deletionTask(value, { waitingFor: ['已有源码操作'] });
    const requested = { ...original, journalRevision: 2, phase: 'failed', abandonRequested: true, canAbandon: false, canRetry: false, error: { code: 'DISPOSABLE_STOP_UNCONFIRMED', message: 'Stop status remains available after abandoning deletion' } };
    let status = requested;
    await setup(async command => {
      if (command === 'node_app_delete_abandon') { writes++; return requested; }
      if (command === 'node_app_delete_status') return status;
      throw new Error(command);
    }, { instanceId: 'same', request: { app: value, intent: 'delete-task', task: original, token: 1 }, onDeletionAbandoned: task => finished.push(task), onDraftStateChange: state => states.push(state) });
    await click(byText('放弃删除'));
    assert.equal(writes, 1); assert.deepEqual(finished, []); assert.equal(states.at(-1).pending, true);
    assert.match(deletionProgress('正在放弃删除…').textContent, /正在放弃删除，等待安全收尾/);
    assert.equal(byText('放弃删除'), undefined); assert.equal(byText('关闭').disabled, true); assert.equal(byText('重试放弃删除').disabled, true);
    await tick(); assert.deepEqual(finished, []); assert.equal(states.at(-1).pending, true);
    status = { ...requested, journalRevision: 3, phase: 'abandoned' }; await tick();
    assert.deepEqual(finished, [status]); assert.equal(states.at(-1).pending, false); assert.equal(document.querySelector('[role=dialog]'), null);
  });
});

test('A deleted journal claim with remaining targets never removes the application', async () => {
  const value = app(); const deleted = [];
  const invalid = deletionTask(value, { phase: 'deleted', canAbandon: false, result: { appId: value.id, operationOutcome: 'deleted', ownership: 'managed', sourcePreserved: false, evidenceRetained: false } });
  await setup(async command => {
    if (command === 'node_app_delete_preview') return deletionPreview(value);
    assert.equal(command, 'node_app_delete_submit'); return invalid;
  }, { instanceId: 'same', request: { app: value, intent: 'delete', token: 1 }, onAppDeleted: id => deleted.push(id) });
  await click(document.querySelector('.node-delete-confirmation input')); await click(byText('确认永久删除项目'));
  assert.deepEqual(deleted, []); assert.match(deleteDialog().textContent, /删除结果尚不能确认/);
  assert.equal(byText('确认永久删除项目').disabled, true);
});

test('A failed abandon finalization exposes its error and retries only that same cleanup after the worker exits', async () => {
  await withDeletionStatusTimers(async ({ tick }) => {
    const value = app(); const receipts = []; const writes = [];
    const failed = deletionTask(value, { phase: 'failed', active: false, journalRevision: 4, abandonRequested: true, canAbandon: false, canRetry: true, error: { code: 'DISPOSABLE_ABANDON_SYNC_FAILED', message: 'Could not persist final abandon receipt' } });
    const waiting = { ...failed, phase: 'waiting', active: true, journalRevision: 5, canRetry: false, error: null };
    let status = waiting;
    await setup(async (command, args) => {
      if (command === 'node_app_delete_retry') { writes.push([command, args]); return waiting; }
      if (command === 'node_app_delete_status') return status;
      throw new Error(command);
    }, { instanceId: 'same', request: { app: value, intent: 'delete-task', task: failed, token: 1 }, onDeletionAbandoned: task => receipts.push(task) });
    assert.equal(deleteDialog().querySelector('.delete-progress'), null);
    const feedback = deleteDialog().querySelector('.node-delete-footer [role=alert]');
    assert.equal(feedback.querySelector('strong').textContent, '放弃删除未完成');
    assert.match(feedback.textContent, /Could not persist final abandon receipt.*DISPOSABLE_ABANDON_SYNC_FAILED/);
    assert.match(feedback.textContent, /重试只会继续放弃删除的安全收尾，不会开始删除或启动应用/);
    assert.equal(byText('重试放弃删除').disabled, false); assert.equal(byText('放弃删除'), undefined); assert.equal(byText('关闭').disabled, false);
    await click(byText('重试放弃删除'));
    assert.deepEqual(writes, [['node_app_delete_retry', { taskId: failed.taskId, appId: failed.appId, observedRevision: 4 }]]);
    assert.match(deletionProgress('正在放弃删除…').textContent, /正在放弃删除，等待安全收尾/);
    assert.equal(byText('关闭').disabled, true); assert.deepEqual(receipts, []);
    status = { ...waiting, phase: 'abandoned', active: false, journalRevision: 6 }; await tick();
    assert.deepEqual(receipts, [status]); assert.equal(document.querySelector('[role=dialog]'), null);
  });
});

test('An inactive quarantined abandon failure exposes its error and permits closing only the task view', async () => {
  await withDeletionStatusTimers(async () => {
    const value = app(); const reads = []; const receipts = []; const snapshots = [];
    const task = deletionTask(value, { phase: 'failed', active: false, journalRevision: 8, abandonRequested: true, canAbandon: false, canRetry: false,
      error: { code: 'DISPOSABLE_JOURNAL_QUARANTINED', message: 'Journal ownership needs inspection before cleanup can resume' } });
    await setup(async (command, args) => {
      reads.push([command, args]); assert.equal(command, 'node_app_delete_status'); return task;
    }, { instanceId: 'same', request: { app: value, intent: 'delete-task', task, token: 1 }, onDeletionTaskChanged: snapshot => snapshots.push(snapshot), onDeletionAbandoned: receipt => receipts.push(receipt) });
    assert.equal(deleteDialog().querySelector('.delete-progress'), null, 'An exited worker must not hide a quarantined failure behind an endless abandon spinner');
    const feedback = deleteDialog().querySelector('.node-delete-footer [role=alert]');
    assert.match(feedback.textContent, /Journal ownership needs inspection.*DISPOSABLE_JOURNAL_QUARANTINED/);
    assert.equal(document.activeElement, feedback);
    assert.equal(byText('重试放弃删除').disabled, true); assert.equal(byText('放弃删除'), undefined); assert.equal(byText('关闭').disabled, false);
    assert.equal(deleteDialog().querySelector('.modal-close').disabled, false);
    await click(byText('重新读取任务'));
    assert.deepEqual(reads, [['node_app_delete_status', { taskId: task.taskId, appId: task.appId }]]);
    assert.equal(deleteDialog().querySelector('.delete-progress'), null); assert.deepEqual(receipts, []);
    assert.equal(snapshots.at(-1).phase, 'failed'); assert.equal(snapshots.at(-1).abandonRequested, true);
    await click(byText('关闭'));
    assert.equal(document.querySelector('[role=dialog]'), null); assert.deepEqual(receipts, []);
    assert.equal(reads.some(([command]) => command !== 'node_app_delete_status'), false, 'Closing a task view must not abandon or release its backend deletion gate');
  });
});

test('A retry response for a replacement task cannot rebind a failed journal to another deletion', async () => {
  await withDeletionStatusTimers(async () => {
    const value = app(); const task = deletionTask(value, { phase: 'failed', canRetry: true, error: { code: 'DISPOSABLE_STOP_FAILED', message: 'Original task still needs review' } });
    await setup(async command => {
      assert.equal(command, 'node_app_delete_retry');
      return { ...task, taskId: 'a-replacement-task', phase: 'waiting', journalRevision: 2, canRetry: false };
    }, { instanceId: 'same', request: { app: value, intent: 'delete-task', task, token: 1 } });
    await click(byText('重试删除'));
    assert.match(deleteDialog().textContent, /删除任务身份已变化/);
    assert.doesNotMatch(deleteDialog().textContent, /a-replacement-task/);
    assert.equal(deleteDialog().querySelector('.delete-progress'), null); assert.equal(byText('重试删除').disabled, false);
  });
});

test('A standalone Node deletion preview keeps cancellation available without offering an unreviewed delete action', async () => {
  const value = app(); const pending = deferred(); const calls = [];
  await setup(async (command, args) => {
    calls.push([command, args]); assert.equal(command, 'node_app_delete_preview'); return pending.promise;
  }, { request: { app: value, intent: 'delete', token: 1 } });
  const dialog = deleteDialog();
  deletionProgress('正在核对本项目的删除范围…');
  assert.equal(document.querySelector('.node-delete-body').getAttribute('aria-busy'), 'true');
  assert.equal(document.querySelector('.node-delete-body').hidden, true);
  assert.equal(dialog.querySelector('.modal-close').disabled, false);
  assert.equal(document.querySelector('.node-delete-footer').hidden, false);
  assert.equal(byText('取消，保留项目').disabled, false);
  assert.equal(byText('取消，保留项目').closest('[hidden]'), null);
  assert.ok(document.querySelector('.node-delete-confirmation input').closest('[hidden]'));
  assert.ok(!byText('重新核对删除范围') || byText('重新核对删除范围').closest('[hidden]'));
  assert.ok(byText('确认永久删除项目').closest('[hidden]'));
  assert.equal(byText('确认永久删除项目').disabled, true);
  assert.deepEqual(calls, [['node_app_delete_preview', { appId: value.id }]]);
  await act(async () => pending.resolve(deletionPreview(value)));
  assert.equal(dialog.querySelector('.delete-progress'), null);
  assert.equal(document.querySelector('.node-delete-body').getAttribute('aria-busy'), 'false');
  assert.equal(document.querySelector('.node-delete-body').hidden, false);
  assert.equal(dialog.querySelector('.modal-close').disabled, false);
  assert.equal(document.querySelector('.node-delete-confirmation input').checked, false);
  assert.equal(byText('确认永久删除项目').disabled, true);
});

test('Every preview close action discards late results and reopening waits for a fresh unacknowledged scope', async () => {
  for (const action of ['cancel', 'close', 'escape', 'backdrop']) {
    const value = app(); const oldPreview = deferred(); const freshPreview = deferred(); const calls = []; const states = []; const changes = []; const deleted = [];
    const props = { enabled: true, instanceId: 'same', onDraftStateChange: state => states.push(state), onAppChanged: detail => changes.push(detail), onAppDeleted: id => deleted.push(id), request: { app: value, intent: 'delete', token: 1 } };
    await setup(async (command, args) => {
      calls.push([command, args]); assert.equal(command, 'node_app_delete_preview');
      return calls.length === 1 ? oldPreview.promise : freshPreview.promise;
    }, props);
    deletionProgress('正在核对本项目的删除范围…');
    assert.equal(states.at(-1).pending, true);
    if (action === 'cancel') await click(byText('取消，保留项目'));
    if (action === 'close') await click(deleteDialog().querySelector('.modal-close'));
    if (action === 'escape') await act(async () => document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true })));
    if (action === 'backdrop') await click(deleteDialog().closest('.modal-layer'));
    assert.equal(Boolean(document.querySelector('[role=dialog]')), false, `${action} dismisses a read-only preview`);
    assert.equal(states.at(-1).pending, false, 'Cancelling releases the local waiting gate immediately');
    assert.equal(calls.length, 1);
    await act(async () => root.render(h(NodeAppsPanel, { ...props, request: { ...props.request, token: 2 } })));
    assert.equal(calls.length, 2, 'Reopening reads the scope again');
    deletionProgress('正在核对本项目的删除范围…');
    if (action === 'close' || action === 'backdrop') await act(async () => oldPreview.reject(new Error('Cancelled preview error must not appear')));
    else await act(async () => oldPreview.resolve({ ...deletionPreview(value, 'external', 'cancelled-old-revision'), name: 'Cancelled old preview' }));
    deletionProgress('正在核对本项目的删除范围…');
    assert.equal(states.at(-1).pending, true, 'A late cancelled result cannot release the new request');
    assert.equal(deleteDialog().querySelectorAll('[role=alert]').length, 0);
    assert.doesNotMatch(deleteDialog().textContent, /Cancelled old preview|Cancelled preview error/);
    await act(async () => freshPreview.resolve({ ...deletionPreview(value, 'managed', 'fresh-reopened-revision'), name: 'Fresh reopened preview' }));
    assert.equal(deleteDialog().querySelector('.delete-progress'), null);
    assert.match(deleteDialog().querySelector('.node-delete-body').textContent, /Fresh reopened preview/);
    assert.equal(document.querySelector('.node-delete-confirmation input').checked, false);
    assert.equal(byText('确认永久删除项目').disabled, true);
    assert.equal(states.at(-1).pending, false);
    assert.deepEqual(calls.map(([command]) => command), ['node_app_delete_preview', 'node_app_delete_preview']);
    assert.deepEqual(changes, []); assert.deepEqual(deleted, []);
    await act(async () => root.unmount()); root = null;
  }
});

test('A failed Node deletion preview ends loading and keeps the error available for a fresh check', async () => {
  const value = app(); const pending = deferred(); let previews = 0;
  await setup(async command => {
    assert.equal(command, 'node_app_delete_preview');
    return ++previews === 1 ? pending.promise : deletionPreview(value);
  }, { request: { app: value, intent: 'delete', token: 1 } });
  deletionProgress('正在核对本项目的删除范围…');
  await act(async () => pending.reject(new Error('Disposable preview failed')));
  assert.equal(deleteDialog().querySelector('.delete-progress'), null);
  assert.match(deleteDialog().querySelector('[role=alert]').textContent, /Disposable preview failed/);
  assert.equal(deleteDialog().querySelector('.modal-close').disabled, false);
  assert.equal(byText('取消，保留项目').disabled, false);
  assert.equal(byText('确认永久删除项目').disabled, true);
  await click(byText('重新核对删除范围'));
  assert.equal(previews, 2);
  assert.equal(document.querySelector('.node-delete-confirmation input').checked, false);
  assert.equal(byText('确认永久删除项目').disabled, true);
});

test('An unconfirmed stop offers re-checking after stopping or deleting directly, never a dead end', async () => {
  const value = app(); const calls = []; let previews = 0;
  const blocker = { code: 'NODE_APP_RUN_RECOVERY_REQUIRED', message: '应用仍有尚未核实停止的运行现场。' };
  await setup(async (command, args) => {
    calls.push([command, args]);
    if (command === 'node_app_delete_preview') return { ...deletionPreview(value, 'external', `uncertain-review-${++previews}`), blocker };
    if (command === 'node_app_delete_submit') return { taskId: 'task_' + '1'.repeat(32), appId: value.id, name: value.name, ownership: 'external', sourcePath: '/disposable/original-source', phase: 'waiting', active: true, journalRevision: 1, targetRevision: args.input.expectedRevision, canAbandon: true, canRetry: false, renamedAny: false, abandonRequested: false, waitingFor: [], targets: [], remaining: [], preservedPaths: ['/disposable/original-source'], error: null };
    return { taskId: 'task_' + '1'.repeat(32), appId: value.id, name: value.name, ownership: 'external', sourcePath: '/disposable/original-source', phase: 'waiting', active: true, journalRevision: 1, targetRevision: `uncertain-review-${previews}`, canAbandon: true, canRetry: false, renamedAny: false, abandonRequested: false, waitingFor: [], targets: [], remaining: [], preservedPaths: ['/disposable/original-source'], error: null };
  }, { request: { app: value, intent: 'delete', token: 1 } });
  const footer = deleteDialog().querySelector('.node-delete-footer');
  const notice = footer.querySelector('.node-delete-error[role=alert]');
  assert.ok(notice);
  assert.equal(notice.querySelector('strong').textContent, '应用可能还在运行');
  assert.ok(notice.textContent.includes(blocker.message));
  await click(byText('已停止，重新检查'));
  assert.equal(previews, 2, 'Re-checking reads a fresh preview and never deletes');
  const acknowledgment = footer.querySelector('input');
  assert.equal(acknowledgment.disabled, false);
  assert.equal(byText('直接删除').disabled, true);
  await click(acknowledgment);
  assert.equal(byText('直接删除').disabled, false);
  await click(byText('直接删除'));
  assert.deepEqual(calls.find(([command]) => command === 'node_app_delete_submit'), ['node_app_delete_submit', { appId: value.id, input: { expectedRevision: 'uncertain-review-2', force: true } }]);
});

test('Malformed deletion blockers cannot be treated as permission to delete', async () => {
  for (const blocker of [false, 'unreadable recovery status', {}, { code: 'NODE_APP_RUN_RECOVERY_REQUIRED', message: 42 }, { code: '', message: 'Missing recovery identity' }]) {
    const value = app(); const calls = [];
    await setup(async command => {
      calls.push(command); assert.equal(command, 'node_app_delete_preview');
      return { ...deletionPreview(value), blocker };
    }, { request: { app: value, intent: 'delete', token: 1 } });
    const feedback = deleteDialog().querySelector('.node-delete-footer .node-delete-error[role=alert]');
    assert.ok(feedback); assert.equal(feedback.closest('[hidden]'), null);
    assert.equal(feedback.querySelector('strong').textContent, '无法核对删除范围');
    assert.match(feedback.textContent, /删除范围不完整或项目身份不符/);
    assert.equal(document.querySelector('.node-delete-confirmation input').disabled, true);
    assert.equal(byText('确认永久删除项目').disabled, true);
    await click(byText('确认永久删除项目'));
    assert.deepEqual(calls, ['node_app_delete_preview']);
    await act(async () => root.unmount()); root = null;
  }
});

test('Failed Node deletion exposes focused feedback and retry actions even after scrolling to the bottom of its scope', async () => {
  const value = app({ state: 'running' }); const failed = deferred(); let previews = 0; const writes = []; const deleted = [];
  await setup(baseMock(value, async (command, args) => {
    if (command === 'node_app_delete_preview') return deletionPreview(value, 'external', `revision-${++previews}`);
    if (command === 'node_app_delete_submit') { writes.push(args); return failed.promise; }
    throw new Error(command);
  }), { request: { app: value, intent: 'delete', token: 1 }, onAppDeleted: id => deleted.push(id) });
  const body = document.querySelector('.node-delete-body');
  body.scrollTop = 700;
  await click(document.querySelector('.node-delete-confirmation input'));
  await click(byText('确认永久删除项目'));
  deletionProgress('正在永久删除…');
  assert.equal(document.querySelector('.node-delete-body')?.hidden ?? true, true);
  await act(async () => failed.reject({ code: 'NODE_APP_ERROR', message: '应用尚未完成安全关闭，未删除文件；请保留运行现场。' }));
  assert.equal(deleteDialog().querySelector('.delete-progress'), null);
  assert.equal(body.hidden, false);
  const footer = deleteDialog().querySelector('.node-delete-footer');
  const feedback = footer.querySelector('.node-delete-error[role=alert]');
  assert.ok(feedback, 'Failure feedback stays with the pinned actions');
  assert.equal(body.contains(feedback), false, 'A scrolled scope cannot hide the failure message');
  assert.equal(feedback.closest('[hidden]'), null);
  assert.equal(feedback.getAttribute('tabindex'), '-1');
  assert.equal(document.activeElement === feedback, true, 'The completed failure moves focus out of the deleted progress view');
  assert.equal(feedback.querySelector('strong').textContent, '删除未完成');
  assert.match(feedback.textContent, /应用尚未完成安全关闭，未删除文件；请保留运行现场。.*NODE_APP_ERROR/);
  assert.ok(byText('确认永久删除项目').closest('[hidden]'));
  assert.equal(byText('确认永久删除项目').disabled, true);
  assert.equal(footer.querySelector('input').checked, false);
  assert.ok(footer.querySelector('input').closest('[hidden]'));
  assert.ok(body.querySelector('[data-path="/disposable/original-source"]'));
  const recheck = [...footer.querySelectorAll('button')].find(button => button.textContent === '重新核对删除范围');
  assert.ok(recheck, 'Failure offers an explicit recheck next to cancellation');
  assert.equal(recheck.closest('[hidden]'), null);
  assert.equal(recheck.disabled, false);
  assert.equal(byText('取消，保留项目').disabled, false);
  assert.equal([...deleteDialog().querySelectorAll('button')].filter(button => button.textContent === '重新核对删除范围').length, 1);
  await click(recheck);
  assert.equal(deleteDialog().querySelector('.node-delete-error'), null);
  assert.equal(document.querySelector('.node-delete-confirmation input').checked, false);
  assert.equal(document.querySelector('.node-delete-confirmation input').closest('[hidden]'), null);
  assert.equal(byText('确认永久删除项目').closest('[hidden]'), null);
  assert.equal(byText('确认永久删除项目').disabled, true);
  await click(document.querySelector('.node-delete-confirmation input')); await click(byText('确认永久删除项目'));
  assert.deepEqual(writes.map(value => value.input.expectedRevision), ['revision-1', 'revision-2']);
  assert.deepEqual(deleted, []);
});

test('Cancelling standalone Node deletion leaves no detail dialog and reopening reads a fresh unacknowledged scope', async () => {
  const value = app(); let previews = 0;
  const props = { enabled: true, onDraftStateChange() {}, request: { app: value, intent: 'delete', token: 1 } };
  await setup(baseMock(value, async command => {
    assert.equal(command, 'node_app_delete_preview'); return deletionPreview(value, 'managed', `revision-${++previews}`);
  }), props);
  await click(document.querySelector('.node-delete-confirmation input'));
  await click(byText('取消，保留项目'));
  assert.equal(document.querySelector('[role=dialog]'), null);
  await act(async () => root.render(h(NodeAppsPanel, { ...props, request: { app: value, intent: 'delete', token: 2 } })));
  assert.equal(previews, 2);
  assert.equal(document.querySelector('.node-delete-confirmation input').checked, false);
  assert.equal(byText('确认永久删除项目').disabled, true);
  assert.equal(document.querySelector('.node-detail-modal'), null);
});

test('Cancelling deletion from Node details preserves the selected tab and form while drafts still prevent deletion', async () => {
  const value = app(); let previews = 0; let configurationReads = 0;
  await setup(async command => {
    if (command === 'node_app_configuration') { configurationReads++; return value; }
    assert.equal(command, 'node_app_delete_preview'); previews++; return deletionPreview(value);
  });
  const form = document.querySelector('.node-configuration-form');
  assert.equal((document.querySelector('.node-configuration-form') ? '应用配置' : '详情'), '应用配置');
  const trigger = byText('停止并删除…'); trigger.focus();
  await click(trigger);
  assert.equal(document.querySelectorAll('[role=dialog]').length, 2);
  deleteDialog();
  await click(byText('取消，保留项目'));
  assert.equal(document.querySelectorAll('[role=dialog]').length, 1);
  assert.equal((document.querySelector('.node-configuration-form') ? '应用配置' : '详情'), '应用配置');
  assert.equal(document.querySelector('.node-configuration-form'), form, 'Cancelling deletion must not recreate the configuration form');
  assert.equal(document.querySelector('.node-detail-modal').contains(document.activeElement), true, 'Cancelling the nested deletion keeps focus in the original details');
  assert.equal(configurationReads, 1);
  await change(input('AI_KEY'), 'artificial-private-draft');
  assert.equal(byText('停止并删除…').disabled, true);
  assert.equal(previews, 1);
  await change(input('AI_KEY'), ''); await click(byText('停止并删除…'));
  assert.equal(previews, 2); assert.equal(document.querySelector('.node-delete-confirmation input').checked, false);
});

test('A previous service instance cannot approve Node deletion with a late preview', async () => {
  const value = app(); const delayed = deferred(); let previews = 0; const deleted = [];
  const props = { enabled: true, instanceId: 'old', onDraftStateChange() {}, onAppDeleted: id => deleted.push(id), request: { app: value, intent: 'delete', token: 1 } };
  await setup(baseMock(value, async command => {
    if (command === 'node_app_delete_preview') { previews++; return previews === 1 ? delayed.promise : deletionPreview(value, 'managed', 'new-revision'); }
    throw new Error(command);
  }), props);
  await act(async () => root.render(h(NodeAppsPanel, { ...props, instanceId: 'new' })));
  await act(async () => delayed.resolve(deletionPreview(value, 'external', 'old-revision')));
  assert.equal(document.querySelector('.node-delete-confirmation'), null); assert.deepEqual(deleted, []);
  assert.equal(document.querySelector('[role=dialog]'), null, 'A stale standalone preview must not reveal project details');
  await act(async () => root.render(h(NodeAppsPanel, { ...props, instanceId: 'new', request: { ...props.request, token: 2 } })));
  assert.match(document.querySelector('.node-delete-confirmation').textContent, /全部平台管理内容/);
  assert.equal(document.querySelector('.node-delete-confirmation input').checked, false);
});

test('Instance changes release a pending Node deletion locally and ignore its late result without replay', async () => {
  const value = app(); const delayed = deferred(); const deleted = []; let writes = 0; let previews = 0;
  const props = { enabled: true, instanceId: 'old', onDraftStateChange() {}, onAppDeleted: id => deleted.push(id), request: { app: value, intent: 'delete', token: 1 } };
  await setup(baseMock(value, async command => {
    if (command === 'node_app_delete_preview') return deletionPreview(value, 'managed', `revision-${++previews}`);
    if (command === 'node_app_delete_submit') { writes++; return delayed.promise; }
    throw new Error(command);
  }), props);
  await click(document.querySelector('.node-delete-confirmation input')); await click(byText('确认永久删除项目'));
  assert.equal(writes, 1);
  await act(async () => root.render(h(NodeAppsPanel, { ...props, instanceId: 'new' })));
  assert.equal(document.querySelector('.node-delete-confirmation'), null);
  assert.equal(document.querySelector('[role=dialog]'), null);
  await act(async () => delayed.resolve({ appId: value.id, operationOutcome: 'deleted', ownership: 'managed', sourcePreserved: false, evidenceRetained: false }));
  assert.deepEqual(deleted, []); assert.equal(writes, 1); assert.equal(document.querySelector('[role=dialog]'), null);
  await act(async () => root.render(h(NodeAppsPanel, { ...props, instanceId: 'new', request: { ...props.request, token: 2 } })));
  assert.equal(previews, 2); assert.equal(document.querySelector('.node-delete-confirmation input').checked, false);
});

test('Workspace gates revoke an unfinished Node preview without leaving its local operation locked', async () => {
  const value = app(); const delayed = deferred(); let previews = 0;
  const props = { enabled: true, instanceId: 'same', onDraftStateChange() {}, request: { app: value, intent: 'delete', token: 1 } };
  await setup(baseMock(value, async command => {
    assert.equal(command, 'node_app_delete_preview');
    return ++previews === 1 ? delayed.promise : deletionPreview(value);
  }), props);
  await act(async () => root.render(h(NodeAppsPanel, { ...props, enabled: false })));
  await act(async () => delayed.resolve(deletionPreview(value)));
  await act(async () => root.render(h(NodeAppsPanel, props)));
  assert.equal(document.querySelector('[role=dialog]'), null);
  await act(async () => root.render(h(NodeAppsPanel, { ...props, request: { ...props.request, token: 2 } })));
  assert.equal(previews, 2); assert.equal(document.querySelector('.node-delete-confirmation input').checked, false);
});

test('A late status observation cannot resurrect a deleted Node app or its cleared detail', async () => {
  const value = app(); const delayed = deferred(); const changes = []; const deleted = [];
  const originalInterval = window.setInterval; const originalClear = window.clearInterval; const timers = new Map(); let timerId = 0;
  window.setInterval = callback => { const id = ++timerId; timers.set(id, callback); return id; };
  window.clearInterval = id => timers.delete(id);
  try {
    await setup(baseMock(value, async command => {
      if (command === 'node_app_status') return delayed.promise;
      if (command === 'node_app_delete_preview') return deletionPreview(value);
      if (command === 'node_app_delete_submit') return { appId: value.id, operationOutcome: 'deleted', ownership: 'managed', sourcePreserved: false, evidenceRetained: false };
      throw new Error(command);
    }), { instanceId: 'same', request: { app: value, intent: 'details', token: 1 }, onAppChanged: value => changes.push(value), onAppDeleted: id => deleted.push(id) });
    await showDetails();
    await act(async () => { for (const callback of timers.values()) callback(); });
    await click(byText('停止并删除…')); await click(document.querySelector('.node-delete-confirmation input')); await click(byText('确认永久删除项目'));
    const observations = changes.length;
    await act(async () => delayed.resolve({ ...value, state: 'running' }));
    assert.deepEqual(deleted, [value.id]); assert.equal(changes.length, observations); assert.equal(document.querySelector('[role=dialog]'), null);
  } finally { window.setInterval = originalInterval; window.clearInterval = originalClear; }
});

test('A mismatched Node deletion preview or unknown deletion result never removes the app', async () => {
  const value = app(); const deleted = []; let previews = 0;
  await setup(baseMock(value, async command => {
    if (command === 'node_app_delete_preview') return { ...deletionPreview(value), appId: ++previews === 1 ? 'another-app' : value.id };
    if (command === 'node_app_delete_submit') return { appId: value.id, operationOutcome: 'delete_incomplete' };
    throw new Error(command);
  }), { request: { app: value, intent: 'delete', token: 1 }, onAppDeleted: id => deleted.push(id) });
  assert.match(document.querySelector('.node-delete-confirmation').textContent, /项目身份不符/);
  assert.equal(byText('确认永久删除项目').disabled, true);
  await click(byText('重新核对删除范围')); await click(document.querySelector('.node-delete-confirmation input')); await click(byText('确认永久删除项目'));
  assert.match(document.querySelector('.node-delete-confirmation').textContent, /删除结果尚不能确认/);
  assert.deepEqual(deleted, []); assert.ok(document.querySelector('[role=dialog]'));
});

test('Standalone Node deletion never publishes incomplete or mismatched success claims', async () => {
  const value = app();
  const confirmed = { appId: value.id, operationOutcome: 'deleted', ownership: 'managed', sourcePreserved: false, evidenceRetained: false };
  const invalid = [null, { ...confirmed, appId: 'unrelated-app' }, { ...confirmed, operationOutcome: 'delete_incomplete' }, { ...confirmed, ownership: 'external' }, { ...confirmed, sourcePreserved: true }, { ...confirmed, evidenceRetained: true }];
  for (const result of invalid) {
    const deleted = []; let writes = 0;
    await setup(async command => {
      if (command === 'node_app_delete_preview') return deletionPreview(value);
      assert.equal(command, 'node_app_delete_submit'); writes++; return result;
    }, { request: { app: value, intent: 'delete', token: 1 }, onAppDeleted: id => deleted.push(id) });
    await click(document.querySelector('.node-delete-confirmation input')); await click(byText('确认永久删除项目'));
    assert.deepEqual(deleted, []);
    assert.equal(writes, 1);
    assert.match(deleteDialog().textContent, /删除结果尚不能确认|删除未完成/);
    if (byText('确认永久删除项目')) { assert.equal(byText('确认永久删除项目').disabled, true); assert.equal(document.querySelector('.node-delete-confirmation input').checked, false); }
    else assert.ok(byText('重试删除'));
    await act(async () => root.unmount()); root = null;
  }
});

test('Git direct overview displays the one runtime folder without an apply workflow', async () => {
 const calls=[];const value=app({sourcePath:'/disposable/author-source',gitSource:{url:'https://github.com/disposable/source.git',branch:null},ownership:'managed'});
 await setup(baseMock(value,async(command,args)=>{calls.push([command,args]);assert.equal(command,'open_project_folder');}),{request:{app:value,intent:'details',token:1}});
 await showDetails();assert.match(document.querySelector('.application-details').textContent,/项目文件夹.*author-source/s);assert.equal(byText('应用修改'),undefined);assert.equal(document.querySelector('.node-install-panel'),null);assert.deepEqual(calls,[]);
 await click([...document.querySelectorAll('.application-detail-row')].find(row => row.textContent.startsWith('项目文件夹')).querySelector('button'));assert.deepEqual(calls,[['open_project_folder',{path:'/disposable/author-source'}]]);
});

test('Node details share the compact actions and information rows without retired development entrances', async () => {
  const calls = [];
  await setup(baseMock(app(), async (command, args) => { calls.push([command, args]); if (command === 'open_project_folder') return; throw Error(command); }));
  await showDetails();
  assert.equal(document.querySelector('[role=tablist]'), null);
  assert.deepEqual([...document.querySelectorAll('.application-detail-row dt')].map(value => value.textContent), ['应用配置', '项目文件夹', '占用空间', '运行日志']);
  assert.deepEqual([...document.querySelectorAll('.application-primary-actions button')].map(value => value.textContent), ['打开']);
  assert.match(document.querySelector('.detail-action-hint').textContent, /想改功能？交给 AI/);
  assert.doesNotMatch(document.querySelector('.application-details').textContent, /构建并更新|源码版本|Codex|安装副本说明|文件与版本范围/);
  await click([...document.querySelectorAll('.application-detail-row')].find(row => row.textContent.startsWith('项目文件夹')).querySelector('button'));
  assert.deepEqual(calls, [['open_project_folder', { path: '/not-a-real-source' }]]);
});

test('Empty fields hide the configuration row and form including configure requests', async () => {
  const value = app({ fields: [] });
  await setup(baseMock(value, async command => { throw Error(command); }), { request: { app: value, intent: 'configure', token: 1 } });
  assert.equal(byText('修改'), undefined);
  assert.equal(document.querySelector('.node-configuration-form'), null);
  assert.equal(document.querySelector('[role=tablist]'), null);
  assert.deepEqual([...document.querySelectorAll('.application-detail-row dt')].map(value => value.textContent), ['项目文件夹', '占用空间', '运行日志']);
});

test('Viewing and copying the Node handoff use the existing command and keep manual clipboard fallback', async () => {
  const calls = []; const copied = [];
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async text => { copied.push(text); throw Error('clipboard unavailable'); } } });
  const value = app();
  await setup(baseMock(value, async (command, args) => { calls.push([command, args]); assert.equal(command, 'node_app_handoff'); return { projectRoot: '/disposable/source', instanceId: 'instance_one', text: '人工交接全文' }; }), { instanceId: 'instance_one' });
  await showDetails(); await click(byText('查看交接说明'));
  assert.deepEqual(copied, []); assert.equal(document.querySelector('.handoff-preview textarea').value, '人工交接全文');
  assert.match(document.querySelector('.handoff-preview').textContent, /disposable\/source/);
  await showDetails(); await click(byText('复制提示词'));
  assert.deepEqual(copied, ['人工交接全文']); assert.match(document.body.textContent, /请手动复制下方完整说明/);
  assert.equal(calls.length, 2); assert.equal(byText('打开').disabled, false);
  assert.doesNotMatch(document.querySelector('.detail-actions').textContent, /Codex/);
});

test('Runtime observations coalesce throughout the visible details and stop when closed', async () => {
  const originalInterval = window.setInterval; const originalClear = window.clearInterval; const timers = new Map(); let timerId = 0;
  window.setInterval = callback => { const id = ++timerId; timers.set(id, callback); return id; };
  window.clearInterval = id => timers.delete(id);
  const delayed = deferred(); let reads = 0;
  try {
    await setup(baseMock(app(), async command => { assert.equal(command, 'node_app_status'); reads++; return delayed.promise; }));
    assert.equal(timers.size, 1); assert.equal(byText('刷新运行状态'), undefined);
    await act(async () => { for (const callback of timers.values()) { callback(); callback(); } });
    assert.equal(reads, 1);
    await act(async () => delayed.resolve(app({ state: 'running' })));
    assert.match(document.querySelector('.detail-status-badge').textContent, /运行中/);
    await click(document.querySelector('[aria-label^="关闭Disposable"]')); assert.equal(timers.size, 0);
  } finally { window.setInterval = originalInterval; window.clearInterval = originalClear; }
});

test('Configuration reads show only secret presence, declarations and defaults without starting an app', async () => {
  const calls = [];
  await setup(baseMock(app(), async (command, args) => { calls.push([command, args]); }));
  assert.equal(input('AI_KEY').type, 'password');
  assert.equal(input('AI_KEY').value, '');
  assert.equal(input('AI_KEY').placeholder, '已设置，留空保留');
  assert.doesNotMatch(document.querySelector('.node-configuration-form').textContent, /必填|选填|人工说明|人工步骤/);
  await click(byLabel('配置与密钥说明'));
  assert.match(document.querySelector('.node-field-help-modal').textContent, /每个应用的密钥分别保存在本机私有配置文件中，仅当前用户可读写/);
  await click(byLabel('关闭配置与密钥说明'));
  assert.doesNotMatch(document.body.textContent, /系统凭据库|钥匙串|加密保存/);
  assert.equal(input('MODEL').value, '');
  assert.equal(input('MODEL').placeholder, 'test-model');
  await click(byLabel('设置 MODEL说明'));
  assert.match(document.querySelector('.node-field-help-modal').textContent, /留空使用默认值：test-model/);
  await click(byLabel('关闭设置 MODEL说明'));
  assert.equal(input('ENABLED').tagName, 'SELECT');
  assert.equal(input('LIMIT').type, 'number');
  assert.deepEqual(calls, []);
  await click(byLabel('设置 AI_KEY说明'));
  await click(byText('打开官方说明'));
  assert.deepEqual(calls, [['node_app_open_help', { appId: 'node_fixture', fieldName: 'AI_KEY' }]]);
});

test('Every declared field has one labelled control and help is offered only when available', async () => {
  const value = app({ fields: [...fields, field('PLAIN', 'string', { description: '', howToGet: [] })] });
  await setup(baseMock(value, async (command) => { throw new Error(command); }), { request: { app: value, intent: 'details', token: 1 } });
  const rows = [...document.querySelectorAll('.node-config-row')];
  assert.equal(rows.length, value.fields.length);
  for (const declaration of value.fields) {
    const control = input(declaration.name);
    const row = control.closest('.node-config-row');
    assert.equal(row.querySelectorAll('.node-field-label').length, 1);
    assert.equal(row.querySelectorAll('input, select').length, 1);
    assert.equal(row.querySelector('.node-field-label').htmlFor, control.id);
    assert.equal(control.getAttribute('aria-required'), String(declaration.required));
  }
  assert.equal(byLabel('设置 PLAIN说明'), undefined);
  assert.equal(byLabel('设置 AI_KEY说明').getAttribute('aria-haspopup'), 'dialog');
  assert.equal(byLabel('移除此应用的密钥').textContent, '');
  assert.doesNotMatch(document.querySelector('.node-configuration-form').textContent, /必填|选填|人工说明|人工步骤/);
});

test('Field explanations open on request and closing them preserves both ordinary and secret drafts', async () => {
  const calls = [];
  await setup(baseMock(app(), async (command) => { calls.push(command); throw new Error(command); }));
  await change(input('MODEL'), 'unsaved model for explanation');
  await change(input('AI_KEY'), 'artificial-help-draft-key');
  assert.doesNotMatch(document.body.textContent, /人工说明 AI_KEY|人工步骤 AI_KEY/);
  await click(byLabel('设置 AI_KEY说明'));
  const explanation = document.querySelector('.node-field-help-modal');
  assert.match(explanation.textContent, /人工说明 AI_KEY/);
  assert.match(explanation.textContent, /人工步骤 AI_KEY/);
  assert.doesNotMatch(explanation.textContent, /artificial-help-draft-key/);
  assert.equal(explanation.getAttribute('role'), 'dialog');
  await click(byLabel('关闭设置 AI_KEY说明'));
  assert.equal(document.querySelector('.node-field-help-modal'), null);
  assert.equal(input('MODEL').value, 'unsaved model for explanation');
  assert.equal(input('AI_KEY').value, 'artificial-help-draft-key');
  assert.deepEqual(calls, []);
});

test('Escape closes only the explanation and returns keyboard focus to its information button', async () => {
  await setup(baseMock(app(), async (command) => { throw new Error(command); }));
  await change(input('MODEL'), 'keep this draft after Escape');
  const trigger = byLabel('设置 MODEL说明');
  await act(async () => trigger.focus());
  await click(trigger);
  assert.equal(document.querySelectorAll('[role=dialog]').length, 2);
  assert.ok(document.querySelector('.node-field-help-modal').contains(document.activeElement));
  await act(async () => document.activeElement.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true })));
  assert.equal(document.querySelectorAll('[role=dialog]').length, 1);
  assert.equal(document.querySelector('.node-field-help-modal'), null);
  assert.equal(document.querySelector('[aria-label="关闭配置确认"]'), null);
  assert.equal(document.activeElement, trigger);
  assert.equal(input('MODEL').value, 'keep this draft after Escape');
});

test('A changed service instance closes its explanation and resets touched errors while preserving the configuration draft', async () => {
  const value = app();
  const props = { enabled: true, instanceId: 'old', onDraftStateChange() {}, request: { app: value, intent: 'details', token: 1 } };
  await setup(baseMock(value, async command => { throw new Error(command); }), props);
  await change(input('AI_URL'), 'http://artificial.invalid');
  await act(async () => { input('AI_URL').focus(); input('AI_URL').blur(); });
  assert.equal(input('AI_URL').getAttribute('aria-invalid'), 'true');
  await change(input('AI_KEY'), 'artificial-instance-draft');
  await click(byLabel('设置 AI_KEY说明'));
  assert.ok(document.querySelector('.node-field-help-modal'));
  await act(async () => root.render(h(NodeAppsPanel, { ...props, instanceId: 'new' })));
  assert.equal(document.querySelector('.node-field-help-modal'), null);
  assert.equal(document.querySelectorAll('[role=dialog]').length, 1);
  assert.equal(input('AI_URL').value, 'http://artificial.invalid');
  assert.equal(input('AI_URL').getAttribute('aria-invalid'), null);
  assert.equal(input('AI_KEY').value, 'artificial-instance-draft');
  assert.equal(document.activeElement.closest('[hidden], [inert]'), null);
});

test('Selecting another app never carries its explanation or invalid required fields into the next configuration', async () => {
  const first = app({ configuration: { revision: 1, values: {}, secretStatus: { AI_KEY: 'missing' }, missingRequired: ['AI_KEY'], canStart: false } });
  const second = app({ id: 'node_second', name: 'Second artificial fixture' });
  const props = { enabled: true, instanceId: 'same', onDraftStateChange() {}, request: { app: first, intent: 'details', token: 1 } };
  await setup(async (command, args) => {
    if (command === 'node_app_configuration') return args.appId === first.id ? first : second;
    throw new Error(command);
  }, props);
  assert.equal(input('AI_KEY').getAttribute('aria-invalid'), 'true');
  await click(byLabel('设置 AI_KEY说明'));
  assert.ok(document.querySelector('.node-field-help-modal'));
  await act(async () => root.render(h(NodeAppsPanel, { ...props, request: { app: second, intent: 'details', token: 2 } })));
  assert.equal(document.querySelector('.node-field-help-modal'), null);
  assert.equal(document.querySelectorAll('[role=dialog]').length, 1);
  assert.equal(document.querySelector('[role=dialog] h2').textContent, second.name);
  await click(byText('修改'));
  assert.equal(input('AI_KEY').getAttribute('aria-invalid'), null);
  assert.equal(input('AI_KEY').value, '');
  assert.equal(input('MODEL').value, '');
});

test('An entry failure switches away from configuration and unmounts its explanation without losing the draft or parent close guard', async () => {
  const entered = deferred(); const calls = [];
  const value = app({ state: 'running', runningRevision: 1 });
  await setup(baseMock(value, async (command, args) => {
    calls.push([command, args]);
    assert.equal(command, 'node_app_open');
    return entered.promise;
  }), { request: { app: value, intent: 'details', token: 1 } });
  await change(input('MODEL'), 'draft retained after entry failure');
  assert.equal(byText('打开').disabled, false, 'Returning to an already running app does not need to apply this draft');
  await click(byText('打开'));
  assert.deepEqual(calls, [['node_app_open', { appId: 'node_fixture' }]]);
  assert.equal(input('MODEL').disabled, true);
  await click(byLabel('设置 MODEL说明'));
  const explanation = document.querySelector('.node-field-help-modal');
  assert.ok(explanation);
  assert.equal(explanation.closest('[hidden]'), null);
  assert.equal(document.querySelectorAll('[role=dialog]').length, 2);
  assert.ok(explanation.contains(document.activeElement));

  await act(async () => entered.reject(new Error('Artificial entry focus failure')));
  assert.equal((document.querySelector('.node-configuration-form') ? '应用配置' : '详情'), '详情');
  assert.equal(document.querySelector('.node-field-help-modal'), null, 'A hidden configuration panel cannot retain a child modal on the stack');
  assert.equal(document.querySelectorAll('[role=dialog]').length, 1);
  assert.match(document.querySelector('[role=alert]').textContent, /Artificial entry focus failure/);
  assert.equal(document.activeElement.closest('[hidden], [inert]'), null);
  await click(byText('修改'));
  assert.equal(document.querySelector('.node-field-help-modal'), null, 'Returning to configuration does not reopen the previous explanation');
  assert.equal(input('MODEL').value, 'draft retained after entry failure');
  assert.equal(input('MODEL').disabled, false);
  await act(async () => document.activeElement.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true })));
  assert.ok(byLabel('关闭配置确认'));
  assert.equal(document.querySelectorAll('[role=dialog]').length, 1);
  assert.equal(document.querySelector('.node-field-help-modal'), null);
  assert.equal(input('MODEL').value, 'draft retained after entry failure');
  assert.deepEqual(calls, [['node_app_open', { appId: 'node_fixture' }]]);
});

test('Missing required fields use invalid controls while an incomplete configuration remains saveable', async () => {
  const calls = [];
  const configuration = { revision: 1, values: {}, secretStatus: { AI_KEY: 'missing' }, missingRequired: ['AI_KEY', 'REQUIRED'], canStart: false };
  const value = app({ fields: [...fields, field('REQUIRED', 'string', { required: true })], configuration });
  await setup(baseMock(value, async (command, args) => {
    calls.push([command, args]);
    assert.equal(command, 'node_app_configuration_save');
    return { ...value, configuration: { ...configuration, revision: 2, values: { MODEL: 'partial draft' } } };
  }), { request: { app: value, intent: 'details', token: 1 } });
  assert.equal(input('AI_KEY').getAttribute('aria-invalid'), 'true');
  assert.equal(input('REQUIRED').getAttribute('aria-invalid'), 'true');
  assert.equal(input('AI_URL').getAttribute('aria-invalid'), null);
  assert.equal(input('ENABLED').getAttribute('aria-invalid'), null);
  assert.equal(input('AI_KEY').checkValidity(), true, 'Missing values do not use a blocking native constraint');
  await change(input('MODEL'), 'partial draft');
  await saveAndContinueEditing();
  assert.deepEqual(calls, [['node_app_configuration_save', { appId: value.id, input: { revision: 1, values: { MODEL: 'partial draft' }, secrets: { AI_KEY: { action: 'keep' } } } }]]);
  assert.equal(input('AI_KEY').getAttribute('aria-invalid'), 'true');
  assert.equal(input('REQUIRED').getAttribute('aria-invalid'), 'true');
  assert.doesNotMatch(document.querySelector('.node-configuration-form').textContent, /必填|选填/);
});

test('A malformed URL is highlighted on blur, blocks native save and focuses its field until corrected', async () => {
  const calls = [];
  const configuration = { revision: 1, values: {}, secretStatus: { AI_KEY: 'missing' }, missingRequired: ['AI_KEY'], canStart: false };
  const value = app({ configuration });
  await setup(baseMock(value, async (command, args) => {
    calls.push([command, args]);
    return { ...value, configuration: { ...configuration, revision: 2, values: { AI_URL: args.input.values.AI_URL } } };
  }), { request: { app: value, intent: 'details', token: 1 } });
  await change(input('AI_URL'), 'https://user:password@example.invalid/api');
  assert.equal(input('AI_URL').getAttribute('aria-invalid'), null, 'Untouched draft errors are quiet until interaction');
  await act(async () => { input('AI_URL').focus(); input('AI_URL').blur(); });
  assert.equal(input('AI_URL').getAttribute('aria-invalid'), 'true');
  assert.match(document.getElementById(input('AI_URL').getAttribute('aria-describedby')).textContent, /HTTPS 地址/);
  await saveAndContinueEditing();
  assert.deepEqual(calls, []);
  assert.equal(document.activeElement, input('AI_URL'));
  let invalidUrlValid;
  await act(async () => { invalidUrlValid = input('AI_URL').checkValidity(); });
  assert.equal(invalidUrlValid, false);
  await change(input('AI_URL'), 'https://corrected.example.invalid/api');
  assert.equal(input('AI_URL').getAttribute('aria-invalid'), null);
  assert.equal(input('AI_URL').checkValidity(), true);
  await saveAndContinueEditing();
  assert.equal(calls.length, 1);
  assert.equal(calls[0][0], 'node_app_configuration_save');
  assert.deepEqual(calls[0][1].input.values, { AI_URL: 'https://corrected.example.invalid/api' });
});

test('Unsafe or fractional integers are rejected before save and correcting to zero clears the field error', async () => {
  const calls = [];
  await setup(baseMock(app(), async (command, args) => {
    calls.push([command, args]);
    return app({ configuration: { revision: 2, values: { LIMIT: 0 }, secretStatus: { AI_KEY: 'set' }, missingRequired: [], canStart: true } });
  }));
  for (const invalid of ['9007199254740993', '1.5']) {
    await change(input('LIMIT'), invalid);
    await saveAndContinueEditing();
    assert.deepEqual(calls, []);
    assert.equal(document.activeElement, input('LIMIT'));
    assert.equal(input('LIMIT').getAttribute('aria-invalid'), 'true');
    assert.match(document.getElementById(input('LIMIT').getAttribute('aria-describedby')).textContent, /整数/);
  }
  await change(input('LIMIT'), '0');
  assert.equal(input('LIMIT').getAttribute('aria-invalid'), null);
  assert.equal(input('LIMIT').checkValidity(), true);
  await saveAndContinueEditing();
  assert.equal(calls.length, 1);
  assert.deepEqual(calls[0][1].input.values, { LIMIT: 0 });
});

// jsdom sanitizes unfinished numeric text immediately and cannot reproduce a
// browser's native badInput flag. Override that one observation while preserving
// its real ValidityState, including customError set by the form.
for (const clearEvent of ['input', 'submit']) {
  test(`An unfinished numeric entry can be cleared with unchanged empty value and saved using its default (${clearEvent})`, async () => {
    const calls = [];
    await setup(baseMock(app(), async (command, args) => {
      calls.push([command, args]);
      assert.equal(command, 'node_app_configuration_save');
      return app({ configuration: { revision: 2, values: { MODEL: 'draft for native numeric validation' }, secretStatus: { AI_KEY: 'set' }, missingRequired: [], canStart: true } });
    }));
    await change(input('MODEL'), 'draft for native numeric validation');
    const number = input('LIMIT');
    const nativeValidity = number.validity;
    let badInput = true;
    Object.defineProperty(number, 'validity', {
      configurable: true,
      get: () => new Proxy(nativeValidity, {
        get: (target, key) => key === 'badInput' ? badInput : Reflect.get(target, key, target),
      }),
    });
    assert.equal(number.value, '');
    await act(async () => number.dispatchEvent(new window.Event('input', { bubbles: true })));
    await saveAndContinueEditing();
    assert.deepEqual(calls, []);
    assert.equal(number.getAttribute('aria-invalid'), 'true');
    assert.equal(document.activeElement, number);
    assert.equal(number.validity.customError, true);
    const invalid = new window.Event('invalid', { cancelable: true });
    await act(async () => number.dispatchEvent(invalid));
    assert.equal(invalid.defaultPrevented, true, 'The field uses one accessible inline error without an extra native bubble');

    badInput = false;
    // The DOM value stays empty when an unfinished sign/exponent is erased, so
    // React onChange may be skipped. Neither recovery path may trust that cache.
    assert.equal(number.value, '');
    if (clearEvent === 'input') {
      await act(async () => number.dispatchEvent(new window.Event('input', { bubbles: true })));
      assert.equal(number.getAttribute('aria-invalid'), null);
      assert.equal(number.validity.customError, false);
    }
    await saveAndContinueEditing();
    assert.equal(calls.length, 1);
    assert.deepEqual(calls[0][1].input.values, { MODEL: 'draft for native numeric validation' });
    assert.equal(Object.hasOwn(calls[0][1].input.values, 'LIMIT'), false, 'Clearing uses the declared default instead of saving an unfinished number');
    assert.equal(input('LIMIT').getAttribute('aria-invalid'), null);
    assert.equal(input('LIMIT').validity.customError, false);
  });
}

test('Read-only explanations remain available and close normally while an official help request is pending', async () => {
  const opened = deferred(); const calls = [];
  await setup(baseMock(app(), async (command, args) => {
    calls.push([command, args]);
    assert.equal(command, 'node_app_open_help');
    return opened.promise;
  }));
  await change(input('MODEL'), 'draft retained while reading');
  await click(byLabel('设置 AI_KEY说明'));
  await click(byText('打开官方说明'));
  assert.equal(input('MODEL').disabled, true);
  assert.equal(byLabel('关闭Disposable Node fixture').disabled, true);
  assert.equal(byText('打开官方说明').disabled, true, 'A help read cannot start a competing native operation');
  assert.equal(byLabel('关闭设置 AI_KEY说明').disabled, false);
  await click(byLabel('关闭设置 AI_KEY说明'));
  assert.equal(document.querySelector('.node-field-help-modal'), null);
  await click(byLabel('配置与密钥说明'));
  assert.equal(byLabel('关闭配置与密钥说明').disabled, false);
  await act(async () => document.activeElement.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true })));
  assert.equal(document.querySelectorAll('[role=dialog]').length, 1);
  assert.equal(byLabel('关闭Disposable Node fixture').disabled, true);
  assert.equal(input('MODEL').value, 'draft retained while reading');
  assert.deepEqual(calls, [['node_app_open_help', { appId: 'node_fixture', fieldName: 'AI_KEY' }]]);
  await act(async () => opened.resolve());
  assert.equal(input('MODEL').disabled, false);
  assert.equal(input('MODEL').value, 'draft retained while reading');
});

test('Machine values and credentials disable capitalization, autocorrection and spellcheck', async () => {
  await setup(baseMock(app(), async (command) => { throw new Error(command); }));
  for (const name of ['AI_URL', 'AI_KEY', 'MODEL', 'LIMIT']) {
    const element = input(name);
    assert.equal(element.getAttribute('autocapitalize'), 'none', name);
    assert.equal(element.getAttribute('autocorrect'), 'off', name);
    assert.equal(element.getAttribute('spellcheck'), 'false', name);
    assert.equal(element.getAttribute('autocomplete'), 'off', name);
  }
});

test('Save keeps false and zero, omits empty ordinary values, clears accepted secret input and does not start', async () => {
  const calls = []; const states = [];
  const saved = app({ configuration: { revision: 2, values: { LIMIT: 0, ENABLED: false, MODE: 'two' }, secretStatus: { AI_KEY: 'set' }, missingRequired: [], canStart: true } });
  await setup(baseMock(app(), async (command, args) => { calls.push([command, args]); assert.equal(command, 'node_app_configuration_save'); return saved; }), { onDraftStateChange: (value) => states.push(value) });
  await change(input('LIMIT'), '0'); await change(input('ENABLED'), 'false'); await change(input('MODE'), 'two');
  await change(input('AI_KEY'), 'artificial-secret-only');
  await saveAndContinueEditing();
  assert.deepEqual(calls, [['node_app_configuration_save', { appId: 'node_fixture', input: { revision: 1, values: { LIMIT: 0, ENABLED: false, MODE: 'two' }, secrets: { AI_KEY: { action: 'replace', value: 'artificial-secret-only' } } } }]]);
  assert.equal(input('AI_KEY').value, ''); assert.equal(document.body.textContent.includes('artificial-secret-only'), false);
  assert.deepEqual(states.at(-1), { dirty: false, pending: false });
});

test('Removing a saved key requires an explicit action; a blank password keeps it', async () => {
  const calls = [];
  await setup(baseMock(app(), async (command, args) => {
    calls.push([command, args]);
    assert.equal(command, 'node_app_configuration_save');
    const removed = args.input.secrets.AI_KEY.action === 'unbind';
    return app({ configuration: { revision: calls.length + 1, values: { MODEL: 'manual-model' }, secretStatus: { AI_KEY: removed ? 'missing' : 'set' }, missingRequired: removed ? ['AI_KEY'] : [], canStart: !removed } });
  }));
  await change(input('MODEL'), 'manual-model'); await saveAndContinueEditing();
  assert.deepEqual(calls[0][1].input.secrets, { AI_KEY: { action: 'keep' } });
  assert.equal(input('AI_KEY').getAttribute('aria-invalid'), null, 'A stored required key does not need to be typed again');
  await click(byLabel('移除此应用的密钥'));
  assert.equal(input('AI_KEY').disabled, true);
  await saveAndContinueEditing();
  assert.deepEqual(calls[1][1].input.secrets, { AI_KEY: { action: 'unbind' } });
  assert.equal(input('AI_KEY').getAttribute('aria-invalid'), 'true');
  assert.equal(input('AI_KEY').disabled, false, 'A required key can be supplied again after removal is saved');
  assert.equal(input('AI_KEY').value, '');
});

test('A changed endpoint needs an explicit rebind, and editing the target again clears that confirmation', async () => {
  const calls = [];
  await setup(baseMock(app(), async (command, args) => { calls.push([command, args]); throw { code: 'ENDPOINT_CHANGED', message: '请确认新的服务地址' }; }));
  await change(input('AI_URL'), 'https://new.example.invalid/api');
  const checkbox = () => document.querySelector('.node-endpoint-confirmation input[type="checkbox"]');
  assert.ok(checkbox()); assert.equal(checkbox().checked, false);
  await saveAndContinueEditing();
  assert.equal(calls.length, 0, 'An unconfirmed credential target is rejected before any native save');
  assert.equal(input('AI_KEY').getAttribute('aria-invalid'), 'true');
  assert.equal(document.activeElement, input('AI_KEY'));
  await click(checkbox());
  assert.equal(input('AI_KEY').getAttribute('aria-invalid'), null);
  await saveAndContinueEditing();
  assert.deepEqual(calls[0][1].input.secrets, { AI_KEY: { action: 'rebind' } });
  await change(input('AI_URL'), 'https://third.example.invalid/api');
  assert.equal(checkbox().checked, false);
  assert.equal(input('AI_URL').value, 'https://third.example.invalid/api');
});

test('Failed saves preserve the draft and reject duplicate pending submissions', async () => {
  const accepted = deferred(); let saves = 0;
  await setup(baseMock(app(), async (command) => { assert.equal(command, 'node_app_configuration_save'); saves++; return accepted.promise; }));
  await change(input('AI_KEY'), 'fixture-draft');
  await saveAndContinueEditing(); await saveAndContinueEditing(); assert.equal(saves, 1);
  await act(async () => accepted.reject({ code: 'CONFIGURATION_REVISION_CHANGED', message: '配置已被其他窗口修改，请重新核对' }));
  assert.equal(input('AI_KEY').value, 'fixture-draft');
  assert.match(document.querySelector('[role="alert"]').textContent, /其他窗口/);
  assert.equal(byText('打开').disabled, true);
});

test('Opening an app makes one native start request, then returning delegates only its app ID', async () => {
  const accepted = deferred(); const calls = [];
  await setup(baseMock(app(), async (command, args) => { if (command === 'node_app_status') return app({ state: 'starting' }); calls.push([command, args]); if (command === 'node_app_start') return accepted.promise; if (command === 'node_app_open') return; throw new Error(command); }));
  assert.equal(byText('打开应用页面'), undefined);
  assert.equal(byText('启动应用'), undefined);
  await click(byText('打开')); await click(byText('打开'));
  assert.deepEqual(calls, [['node_app_start', { appId: 'node_fixture' }]]);
  await act(async () => accepted.resolve(app({ state: 'running', runningRevision: 1 })));
  await click(byText('打开'));
  assert.deepEqual(calls[1], ['node_app_open', { appId: 'node_fixture' }]);
  assert.equal(calls.length, 2);
  assert.ok(byText('打开'));
});

test('Startup progress shows observed backend phases and elapsed time, and stops polling on success', async () => {
  await withDeletionStatusTimers(async timers => {
    const started = deferred(); const writes = [];
    let progress = { phase: 'waiting-ready', elapsedMs: 8000, phaseElapsedMs: 7600, timeoutMs: 30000, healthTimeoutMs: 10000 };
    await setup(baseMock(app(), async (command, args) => {
      if (command === 'node_app_status') return app({ state: 'starting', startupProgress: progress });
      if (command === 'node_app_start') { writes.push([command, args]); return started.promise; }
      throw new Error(command);
    }));
    await click(byText('打开'));
    assert.equal(timers.count(), 1);
    const status = () => document.querySelector('.node-startup-progress[role=status]');
    assert.equal(status().getAttribute('aria-live'), 'polite');
    assert.match(status().textContent, /正在等待应用准备完成/);
    assert.match(status().textContent, /已等待 8 秒.*总时限 30 秒/);
    assert.doesNotMatch(status().textContent, /检查本机连接|%/);
    progress = { ...progress, phase: 'checking-health', elapsedMs: 18000, phaseElapsedMs: 500 };
    await timers.tick();
    assert.match(status().textContent, /正在检查本机连接/);
    assert.match(status().textContent, /已等待 18 秒/);
    assert.match(status().textContent, /连接检查最多 10 秒，包含在启动总时限内/);
    assert.deepEqual(writes, [['node_app_start', { appId: 'node_fixture' }]]);
    await act(async () => started.resolve(app({ state: 'running', runningRevision: 1 })));
    assert.equal(status(), null);
    assert.equal(timers.count(), 0);
  });
});

test('Startup cleanup remains visible after the budget and failure releases its progress timer', async () => {
  await withDeletionStatusTimers(async timers => {
    const started = deferred();
    await setup(baseMock(app(), async command => {
      if (command === 'node_app_status') return app({ state: 'starting', startupProgress: { phase: 'cleanup', elapsedMs: 31000, phaseElapsedMs: 1000, timeoutMs: 30000, healthTimeoutMs: 10000 } });
      if (command === 'node_app_start') return started.promise;
      throw new Error(command);
    }));
    await click(byText('打开'));
    assert.match(document.querySelector('.node-startup-progress').textContent, /正在关闭本次启动资源/);
    assert.match(document.querySelector('.node-startup-progress').textContent, /已等待 31 秒/);
    assert.doesNotMatch(document.querySelector('.node-startup-progress').textContent, /启动总时限|%/);
    await act(async () => started.reject(new Error('人工启动总时限失败')));
    assert.equal(timers.count(), 0);
    assert.equal(document.querySelector('.node-startup-progress'), null);
    assert.match(document.body.textContent, /人工启动总时限失败/);
    assert.equal(byText('打开').disabled, false);
  });
});

test('A startup observation crossing an instance change cannot alter the next selected application', async () => {
  await withDeletionStatusTimers(async timers => {
    const first = app(); const second = app({ id: 'another-progress-fixture', name: 'Another progress fixture' });
    const started = deferred(); const observed = deferred(); const changes = [];
    const props = { enabled: true, instanceId: 'old-progress-instance', onDraftStateChange() {}, onAppChanged: value => changes.push(value), request: { app: first, intent: 'details', token: 1 } };
    await setup(async (command, args) => {
      if (command === 'node_app_configuration') return args.appId === second.id ? second : first;
      if (command === 'node_app_start') return started.promise;
      if (command === 'node_app_status') return observed.promise;
      throw new Error(command);
    }, props);
    await click(byText('打开'));
    assert.equal(timers.count(), 1);
    await act(async () => root.render(h(NodeAppsPanel, { ...props, instanceId: 'new-progress-instance' })));
    assert.equal(timers.count(), 0, 'An old-instance start never polls the replacement service');
    const count = changes.length;
    await act(async () => started.resolve(app({ name: 'Late old startup result', state: 'running' })));
    assert.equal(changes.length, count);
    await act(async () => root.render(h(NodeAppsPanel, { ...props, instanceId: 'new-progress-instance', request: { app: second, intent: 'details', token: 2 } })));
    await act(async () => observed.resolve(app({ state: 'starting', startupProgress: { phase: 'checking-health', elapsedMs: 27000, phaseElapsedMs: 1000, timeoutMs: 30000, healthTimeoutMs: 10000 } })));
    assert.equal(document.querySelector('[role=dialog] h2').textContent, second.name);
    assert.doesNotMatch(document.body.textContent, /Late old startup result|已等待 27 秒|正在检查本机连接/);
    assert.equal(document.querySelector('.node-startup-progress'), null);
  });
});

test('An old startup settling after a new same-app instance start cannot clear the new pending operation', async () => {
  for (const action of ['enter', 'restart']) for (const outcome of ['resolve', 'reject']) {
    await withDeletionStatusTimers(async timers => {
      const value = app(action === 'restart' ? { state: 'running', runningRevision: 1, restartRequired: true } : {});
      let configured = value;
      const firstStart = deferred(); const nextStart = deferred(); const changes = []; const states = []; const writes = [];
      const props = { enabled: true, instanceId: 'old-start-instance', onDraftStateChange: value => states.push(value), onAppChanged: value => changes.push(value), request: { app: value, intent: 'details', token: 1 } };
      await setup(async command => {
        if (command === 'node_app_configuration') return configured;
        if (command === 'node_app_start' || command === 'node_app_restart') { writes.push(command); return writes.length === 1 ? firstStart.promise : nextStart.promise; }
        if (command === 'node_app_status') return app({ state: 'starting', startupProgress: { phase: 'checking-health', elapsedMs: 11000, phaseElapsedMs: 500, timeoutMs: 30000, healthTimeoutMs: 10000 } });
        throw new Error(command);
      }, props);
      await showDetails();
      await click(byText(action === 'restart' ? '立即重启' : '打开'));
      configured = app();
      await act(async () => root.render(h(NodeAppsPanel, { ...props, instanceId: 'new-start-instance', request: { app: configured, intent: 'enter', token: 2 } })));
      assert.deepEqual(writes, [action === 'restart' ? 'node_app_restart' : 'node_app_start', 'node_app_start']);
      assert.equal(timers.count(), 1);
      assert.match(document.querySelector('.node-startup-progress').textContent, /正在检查本机连接/);
      const count = changes.length;
      await act(async () => outcome === 'resolve'
        ? firstStart.resolve(app({ name: 'Old startup must not publish', state: 'running' }))
        : firstStart.reject(new Error('Old startup must not publish')));
      assert.equal(changes.length, count);
      assert.equal(states.at(-1).pending, true);
      assert.equal(timers.count(), 1);
      assert.ok(document.querySelector('.node-startup-dialog'));
      await timers.tick();
      assert.match(document.querySelector('.node-startup-progress').textContent, /已等待 11 秒/);
      assert.doesNotMatch(document.body.textContent, /Old startup must not publish/);
      assert.equal(writes.length, 2, 'Neither a late result nor progress observation starts another backend');
      await act(async () => nextStart.resolve(app({ state: 'running' })));
      assert.equal(timers.count(), 0);
      assert.equal(states.at(-1).pending, false);
    });
  }
});

test('A failed old configuration refresh cannot publish a startup error after the replacement instance starts', async () => {
  await withDeletionStatusTimers(async timers => {
    const value = app(); const firstStart = deferred(); const nextStart = deferred(); const oldRefresh = deferred(); const states = [];
    let reads = 0; let starts = 0;
    const props = { enabled: true, instanceId: 'old-refresh-instance', onDraftStateChange: value => states.push(value), request: { app: value, intent: 'details', token: 1 } };
    await setup(async command => {
      if (command === 'node_app_configuration') return ++reads === 2 ? oldRefresh.promise : value;
      if (command === 'node_app_start') return ++starts === 1 ? firstStart.promise : nextStart.promise;
      if (command === 'node_app_status') return app({ state: 'starting', startupProgress: { phase: 'waiting-ready', elapsedMs: 4000, phaseElapsedMs: 3000, timeoutMs: 30000, healthTimeoutMs: 10000 } });
      throw new Error(command);
    }, props);
    await click(byText('打开'));
    await act(async () => firstStart.reject({ code: 'NODE_CONFIGURATION_REQUIRED', message: 'Old startup error must remain private' }));
    assert.equal(reads, 2);
    await act(async () => root.render(h(NodeAppsPanel, { ...props, instanceId: 'new-refresh-instance', request: { app: value, intent: 'enter', token: 2 } })));
    assert.equal(starts, 2);
    assert.equal(timers.count(), 1);
    await act(async () => oldRefresh.reject(new Error('Old refresh read failed')));
    assert.equal(states.at(-1).pending, true);
    assert.equal(timers.count(), 1);
    assert.doesNotMatch(document.body.textContent, /Old startup error|Old refresh read failed/);
    await act(async () => nextStart.resolve(app({ state: 'running' })));
    assert.equal(timers.count(), 0);
    assert.equal(states.at(-1).pending, false);
  });
});

test('Unmounting pending startup clears polling and ignores its late observation and result', async () => {
  await withDeletionStatusTimers(async timers => {
    const started = deferred(); const observed = deferred(); const changes = [];
    await setup(baseMock(app(), async command => {
      if (command === 'node_app_start') return started.promise;
      if (command === 'node_app_status') return observed.promise;
      throw new Error(command);
    }), { onAppChanged: value => changes.push(value) });
    await click(byText('打开'));
    assert.equal(timers.count(), 1);
    const count = changes.length;
    await act(async () => root.unmount()); root = null;
    assert.equal(timers.count(), 0);
    await act(async () => {
      observed.resolve(app({ state: 'starting', startupProgress: { phase: 'checking-health', elapsedMs: 9000, phaseElapsedMs: 500, timeoutMs: 30000, healthTimeoutMs: 10000 } }));
      started.resolve(app({ state: 'running' }));
    });
    assert.equal(changes.length, count);
    assert.equal(document.querySelector('.node-startup-progress'), null);
  });
});

test('Returning to an open application never starts a second instance and gates duplicate pending focus requests', async () => {
  const focus = deferred(); const calls = [];
  await setup(baseMock(app({ state: 'running', runningRevision: 1 }), async (command, args) => {
    calls.push([command, args]); assert.equal(command, 'node_app_open'); return focus.promise;
  }));
  await click(byText('打开')); await click(byText('打开'));
  assert.deepEqual(calls, [['node_app_open', { appId: 'node_fixture' }]]);
  await act(async () => focus.resolve());
  assert.equal(byText('打开').disabled, false);
});

test('Closing an app sends one stop request and returns the manager to its unopened state', async () => {
  const closed = deferred(); const calls = [];
  await setup(baseMock(app({ state: 'running', runningRevision: 1 }), async (command, args) => {
    calls.push([command, args]); assert.equal(command, 'node_app_stop'); return closed.promise;
  }));
  await showDetails();
  await click(byText('停止运行')); await click(byText('停止运行'));
  assert.deepEqual(calls, [['node_app_stop', { appId: 'node_fixture' }]]);
  await act(async () => closed.resolve(app()));
  assert.equal(byText('停止运行'), undefined);
  assert.equal(byText('打开').disabled, false);
  assert.match(document.body.textContent, /应用已关闭，相关服务已停止/);
});

test('A verified orphaned runtime can be stopped without opening a second instance', async () => {
  const value = app({ state: 'failed', lastError: '已核对上次运行的身份', runRecovery: { state: 'orphaned', canStop: true, message: '已核对上次运行的身份，可以停止此遗留运行。' }, configuration: { ...app().configuration, canStart: false } });
  const calls = [];
  await setup(baseMock(value, async (command, args) => {
    calls.push([command, args]);
    assert.equal(command, 'node_app_stop');
    return app({ runRecovery: null });
  }), { request: { app: value, intent: 'details', token: 1 } });
  await click(byText('打开'));
  assert.equal((document.querySelector('.node-configuration-form') ? '应用配置' : '详情'), '详情');
  assert.match(document.querySelector('.application-details').textContent, /可以停止此遗留运行/);
  assert.equal(byText('停止遗留运行').disabled, false);
  await click(byText('停止遗留运行'));
  assert.deepEqual(calls, [['node_app_stop', { appId: value.id }]]);
  assert.equal(byText('停止遗留运行'), undefined);
  assert.equal(byText('停止运行'), undefined);
  assert.equal(byText('打开').disabled, false);
  assert.doesNotMatch(document.querySelector('.application-details').textContent, /可以停止此遗留运行/);
});

test('An unconfirmed old runtime explains why stop is unavailable and routes entry to runtime status', async () => {
  const value = app({ state: 'failed', lastError: '运行身份无法确认，请重启电脑后重新核对。', runRecovery: { state: 'unconfirmed', canStop: false, message: '运行身份无法确认，请重启电脑后重新核对。' }, configuration: { ...app().configuration, missingRequired: ['AI_KEY'], secretStatus: { AI_KEY: 'missing' }, canStart: false } });
  const calls = [];
  await setup(baseMock(value, async (command, args) => { calls.push([command, args]); throw new Error(`Unexpected execution while identity is unknown: ${command}`); }), { request: { app: value, intent: 'details', token: 1 } });
  await click(byText('打开'));
  assert.equal((document.querySelector('.node-configuration-form') ? '应用配置' : '详情'), '详情');
  const runtime = document.querySelector('.application-details');
  assert.match(document.querySelector('.detail-status-badge').textContent, /运行待核对/);
  assert.match(runtime.querySelector('[role=status]').textContent, /请重启电脑后重新核对/);
  assert.equal(byText('停止运行'), undefined);
  assert.equal(byText('立即重启'), undefined);
  await click(byText('打开'));
  assert.deepEqual(calls, []);
});

test('Incomplete required configuration blocks opening an application and identifies the field to finish', async () => {
  await setup(baseMock(app({ configuration: { revision: 1, values: {}, secretStatus: { AI_KEY: 'missing' }, missingRequired: ['AI_KEY'], canStart: false } }), async (command) => { throw new Error(command); }));
  assert.equal(byText('打开').disabled, false);
  assert.equal(document.querySelector('.node-config-warning'), null);
  assert.equal(input('AI_KEY').getAttribute('aria-invalid'), 'true');
  assert.match(document.getElementById(input('AI_KEY').getAttribute('aria-describedby')).textContent, /请填写此项/);
  assert.equal(input('AI_URL').getAttribute('aria-invalid'), null, 'A required default address is already complete');
  await click(byText('打开'));
});

test('Saving configuration does not reopen a running window; applying it requires an explicit restart', async () => {
  const calls = [];
  await setup(baseMock(app({ state: 'running', runningRevision: 1 }), async (command, args) => {
    if (command === 'node_app_status') return app({ state: 'starting' });
    calls.push([command, args]);
    const configuration = { revision: 2, values: { MODEL: 'new-artificial-model' }, secretStatus: { AI_KEY: 'set' }, missingRequired: [], canStart: true };
    if (command === 'node_app_configuration_save') return app({ state: 'running', runningRevision: 1, restartRequired: true, configuration });
    assert.equal(command, 'node_app_restart');
    return app({ state: 'running', runningRevision: 2, configuration });
  }));
  await change(input('MODEL'), 'new-artificial-model'); await saveAndContinueEditing();
  assert.equal(calls.length, 1);
  assert.equal(calls[0][0], 'node_app_configuration_save');
  await showDetails(); assert.match(document.body.textContent, /已保存，重启后生效/);
  await click(byText('立即重启'));
  assert.deepEqual(calls[1], ['node_app_restart', { appId: 'node_fixture' }]);
  assert.equal(calls.length, 2);
  assert.ok(byText('打开'));
});

function directoryReview(state = 'ready', extra = {}) {
  return { sourcePath: '/artificial/node-source', state, expectedRevision: 'directory-revision-1', issues: [], reviewPrompt: state === 'needs-review' ? '完整后端说明\n请先读取 /artificial/node-source 再检查。\n不得直接运行。' : '', ...(state === 'ready' ? { inspection: { runtime: 'node', runtimeLabel: 'Node 服务', app: { id: 'org.example.preview', name: 'Artificial preview', version: '0.1.0' }, dataContract: { id: 'org.example.inspected-data', version: 1 }, configuration: { fields, missingRequired: ['AI_KEY'] } } } : {}), ...extra };
}
async function setupDirectory(mock, props = {}) {
  await setup(mock, { instanceId: 'directory-instance', onPendingChange() {}, onAdded() {}, ...props }, LocalDirectoryPanel);
}

test('Local web registration preserves the folder and directs incomplete entries to details without starting Node', async () => {
  for (const state of ['ready', 'needsBuild', 'entryMissing']) {
    const sourcePath = '/artificial/local-web';
    const value = { id: `prj_web_${state}`, runtime: 'web', ownership: 'user', name: 'Local web fixture', projectRoot: sourcePath, state };
    const added = []; const opened = []; const details = []; const calls = [];
    await setupDirectory(async (command, args) => {
      calls.push([command, args]);
      if (command === 'choose_path') return sourcePath;
      if (command === 'node_app_directory_inspect') return directoryReview('ready', { sourcePath, inspection: { runtime: 'web', runtimeLabel: '网页应用', app: { name: value.name }, configuration: { fields: [], missingRequired: [] } } });
      if (command === 'node_app_directory_register') return value;
      throw new Error(command);
    }, { onAdded() { assert.fail('Web registration must use its web callback'); }, onWebAdded: item => added.push(item), onWebEnter: item => opened.push(item), onWebDetails: item => details.push(item) });
    await click(byText('选择文件夹…')); await click(byText('添加'));
    assert.deepEqual(added, [value]);
    assert.match(document.body.textContent, /用户文件夹及其中的文件数据保留/);
    assert.match(document.body.textContent, /浏览器数据.*删除应用时清除/);
    assert.deepEqual(opened, []); assert.deepEqual(details, []);
    await click(byText(state === 'ready' ? '打开应用' : '查看应用详情'));
    assert.deepEqual(opened, state === 'ready' ? [value] : []);
    assert.deepEqual(details, state === 'ready' ? [] : [value]);
    assert.deepEqual(calls.map(([command]) => command), ['choose_path', 'node_app_directory_inspect', 'node_app_directory_register']);
    await act(async () => root.unmount()); root = null;
  }
});

test('Adding a ready local folder previews first and binds registration to the reviewed directory without starting it', async () => {
  const calls = [];
  await setup(async (command, args) => {
    calls.push([command, args]);
    if (command === 'node_app_list') return [];
    if (command === 'choose_path') return '/artificial/node-source';
    if (command === 'node_app_directory_inspect') return directoryReview();
    if (command === 'node_app_directory_register') return app({ sourcePath: '/artificial/node-source' });
    if (command === 'open_project_folder') return;
    throw new Error(command);
  }, { request: null, addRequest: 1 });
  await click(byText('选择文件夹…'));
  assert.deepEqual(calls, [['choose_path', { kind: 'nodeapp' }], ['node_app_directory_inspect', { sourcePath: '/artificial/node-source' }]]);
  assert.ok(byText('添加')); assert.equal(document.querySelector('[role="dialog"]').textContent.includes('Artificial preview'), true);
  const readyHeading = document.querySelector('.node-inspection-preview h3');
  assert.equal(readyHeading.textContent, '「Artificial preview」可以添加');
  assert.equal(document.activeElement, byText('添加'));
  assert.match(document.getElementById(document.activeElement.getAttribute('aria-describedby')).textContent, /核对下面的项目和文件夹/);
  const copyPath = byText('复制文件夹路径'); copyPath.focus(); await click(copyPath);
  assert.equal(document.activeElement, copyPath, 'A later local operation must not repeatedly pull focus back to the preview');
  await click(byText('添加'));
  assert.deepEqual(calls.at(-1), ['node_app_directory_register', { sourcePath: '/artificial/node-source', expectedRevision: 'directory-revision-1' }]);
  assert.ok(document.querySelector('[role="dialog"]'));
  assert.match(document.querySelector('[role="dialog"]').textContent, /添加成功/);
  assert.ok(document.querySelector('.local-directory-panel'), 'Adding keeps the result in the original progress dialog');
  assert.equal(document.querySelector('[role=tab]'), null);
  assert.equal(calls.some(([command]) => command === 'node_app_start' || command === 'node_app_configuration'), false, 'Adding does not silently navigate to configuration or open the app');
  await click(byText('打开文件夹'));
  assert.deepEqual(calls.at(-1), ['open_project_folder', { path: '/artificial/node-source' }]);
});

test('Local folder progress follows actual read and add operations, then waits for explicit configuration navigation', async () => {
  const reading = deferred(); const adding = deferred(); const enters = []; const completions = []; const registered = [];
  const value = app({ sourcePath: '/artificial/node-source', configuration: { revision: 1, values: {}, secretStatus: {}, missingRequired: ['AI_KEY'], canStart: false } });
  await setupDirectory(async command => {
    if (command === 'choose_path') return '/artificial/node-source';
    if (command === 'node_app_directory_inspect') return reading.promise;
    if (command === 'node_app_directory_register') return adding.promise;
    throw new Error(command);
  }, { onAdded: item => registered.push(item), onEnter: item => enters.push(item), onClose: () => completions.push(true) });
  const activeStep = () => document.querySelector('[aria-label="添加进度"] [aria-current=step]')?.textContent;
  assert.match(activeStep(), /选择文件夹/);
  await click(byText('选择文件夹…'));
  assert.match(activeStep(), /检查项目/);
  await act(async () => reading.resolve(directoryReview()));
  assert.match(activeStep(), /添加应用/);
  assert.deepEqual(registered, []);
  await click(byText('添加'));
  assert.match(activeStep(), /添加应用/);
  assert.deepEqual(enters, []);
  await act(async () => adding.resolve(value));
  assert.deepEqual(registered, [value]);
  assert.match(document.body.textContent, /添加成功/);
  assert.equal(document.querySelectorAll('[aria-label="添加进度"] .is-complete').length, 3);
  assert.deepEqual(enters, []); assert.deepEqual(completions, []);
  assert.equal(document.activeElement, document.querySelector('.import-result h3'));
  await click(byText('去设置'));
  assert.deepEqual(enters, [value]); assert.deepEqual(completions, []);
});

test('Local folder success is revoked after the platform instance changes and cannot open the old app', async () => {
  const enters = [];
  const props = { enabled: true, instanceId: 'old-directory-service', onPendingChange() {}, onAdded() {}, onEnter: value => enters.push(value), onClose() {} };
  await setupDirectory(async command => {
    if (command === 'choose_path') return '/artificial/node-source';
    if (command === 'node_app_directory_inspect') return directoryReview();
    if (command === 'node_app_directory_register') return app({ sourcePath: '/artificial/node-source' });
    throw new Error(command);
  }, props);
  await click(byText('选择文件夹…')); await click(byText('添加'));
  assert.ok(byText('打开应用'));
  await act(async () => root.render(h(LocalDirectoryPanel, { ...props, instanceId: 'new-directory-service' })));
  assert.equal(byText('打开应用'), undefined); assert.doesNotMatch(document.body.textContent, /添加成功/);
  assert.deepEqual(enters, []);
  assert.ok(byText('重新检查'));
});

test('Choosing configuration from a local add result never starts the app even if its fresh configuration is now complete', async () => {
  const calls = [];
  const ready = app({ sourcePath: '/artificial/node-source' });
  const missing = { ...ready, configuration: { ...ready.configuration, missingRequired: ['AI_KEY'], canStart: false } };
  await setup(async (command, args) => {
    calls.push([command, args]);
    if (command === 'choose_path') return '/artificial/node-source';
    if (command === 'node_app_directory_inspect') return directoryReview();
    if (command === 'node_app_directory_register') return missing;
    if (command === 'node_app_configuration') return ready;
    throw new Error(command);
  }, { request: null, addRequest: 1 });
  await click(byText('选择文件夹…')); await click(byText('添加'));
  assert.ok(byText('去设置')); assert.equal(calls.some(([command]) => command === 'node_app_configuration'), false);
  await click(byText('去设置'));
  assert.equal(document.querySelector('.local-directory-panel'), null);
  assert.equal((document.querySelector('.node-configuration-form') ? '应用配置' : '详情'), '应用配置');
  assert.equal(calls.filter(([command]) => command === 'node_app_configuration').length, 1);
  assert.equal(calls.some(([command]) => command === 'node_app_start' || command === 'node_app_open'), false);
});

test('An explicit configure request opens configuration without starting a ready application', async () => {
  const value = app(); const calls = [];
  await setup(async (command, args) => {
    calls.push([command, args]);
    if (command === 'node_app_configuration') return value;
    throw new Error(command);
  }, { request: { app: value, intent: 'configure', token: 1 } });
  assert.equal((document.querySelector('.node-configuration-form') ? '应用配置' : '详情'), '应用配置');
  assert.equal(calls.filter(([command]) => command === 'node_app_configuration').length, 1);
  assert.equal(calls.some(([command]) => command === 'node_app_start' || command === 'node_app_open'), false);
  assert.ok(byText('打开'));
});

test('An empty folder uses one explicit generate-and-add action with its reviewed revision and name', async () => {
  const calls = []; const added = [];
  await setupDirectory(async (command, args) => {
    calls.push([command, args]);
    if (command === 'choose_path') return '/artificial/node-source';
    if (command === 'node_app_directory_inspect') return directoryReview('empty');
    if (command === 'node_app_directory_initialize_and_register') return app({ sourcePath: '/artificial/node-source' });
    if (command === 'open_project_folder') return;
    throw new Error(command);
  }, { onAdded: value => added.push(value) });
  await click(byText('选择文件夹…'));
  const name = document.querySelector('.local-directory-empty input');
  assert.equal(name.value, 'node-source');
  assert.match(document.querySelector('.local-directory-empty').textContent, /生成.*添加/);
  assert.match(document.querySelector('.local-directory-empty').textContent, /文件夹.*保留/);
  assert.equal(document.querySelector('.local-directory-empty input[type=checkbox]'), null);
  await change(name, ''); assert.equal(byText('生成并添加项目').disabled, true);
  await change(name, 'My local project');
  assert.equal(calls.length, 2);
  await click(byText('生成并添加项目'));
  assert.deepEqual(calls[2], ['node_app_directory_initialize_and_register', { sourcePath: '/artificial/node-source', input: { expectedRevision: 'directory-revision-1', name: 'My local project' } }]);
  assert.equal(added.length, 1); assert.equal(calls.length, 3);
  await click(byText('打开文件夹'));
  assert.deepEqual(calls.at(-1), ['open_project_folder', { path: '/artificial/node-source' }]);
  assert.equal(byText('添加'), undefined, 'Successful generation and adding must not request a second add confirmation');
  assert.equal(calls.some(([command]) => ['node_app_directory_initialize', 'node_app_directory_register', 'node_app_start'].includes(command)), false);
});

test('Review-needed folders copy the shared contextual AI prompt only explicitly and retain manual fallback', async () => {
  const review = directoryReview('needs-review', { issues: ['人工检查问题，平台不据此判定不可适配。'], reviewPrompt: ' 保留首尾空白\n完整说明：请检查本机 /artificial/node-source。\n ' });
  const calls = []; const copied = [];
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { async writeText(value) { copied.push(value); throw new Error('Artificial clipboard denied'); } } });
  await setupDirectory(async (command, args) => {
    calls.push([command, args]);
    if (command === 'choose_path') return review.sourcePath;
    if (command === 'node_app_directory_inspect') return review;
    if (command === 'source_review_capabilities') return { codexAvailable: false };
    throw new Error(command);
  });
  await click(byText('选择文件夹…'));
  assert.match(document.body.textContent, /需要修改后才能添加/);
  assert.match(document.body.textContent, /文件夹保持原样/);
  assert.equal(byText('添加'), undefined); assert.equal(byText('生成并添加项目'), undefined);
  const retained = document.querySelector('[aria-label="给 AI 的检查说明"]');
  assert.equal(retained.readOnly, true); assert.equal(retained.value, review.reviewPrompt);
  assert.deepEqual(copied, []);
  await click(byText('复制说明给 AI'));
  assert.equal(copied.length, 1);
  assert.match(copied[0], /\/artificial\/node-source/); assert.match(copied[0], /人工检查问题/);
  assert.match(document.body.textContent, /Artificial clipboard denied/);
  const preview = document.querySelector('.project-ai-preview textarea');
  assert.equal(preview.value, copied[0]); assert.equal(preview.readOnly, true);
  await click(byText('选择完整说明')); assert.equal(preview.selectionStart, 0); assert.equal(preview.selectionEnd, preview.value.length);
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { async writeText(value) { copied.push(value); } } });
  await click(byText('复制完整说明')); assert.match(document.body.textContent, /已复制/);
  await click(document.querySelector('[aria-label="关闭把问题交给 AI"]'));
  await click(byText('复制文件夹路径')); assert.equal(copied.at(-1), review.sourcePath);
  assert.equal(calls.some(([command]) => /initialize|register|start|send/.test(command)), false, 'Copying never initializes, registers, starts or sends the project');
});

test('Rechecking revokes ready previews and empty-folder actions before reading and never reuses them on failure', async () => {
  for (const state of ['ready', 'empty']) {
    const deferredRead = deferred(); let reads = 0; const calls = [];
    await setupDirectory(async (command, args) => {
      calls.push([command, args]);
      if (command === 'choose_path') return '/artificial/node-source';
      if (command === 'node_app_directory_inspect') return ++reads === 1 ? directoryReview(state) : deferredRead.promise;
      throw new Error(command);
    });
    await click(byText('选择文件夹…'));
    if (state === 'empty') assert.equal(byText('生成并添加项目').disabled, false);
    await click(byText('重新检查'));
    assert.equal(document.querySelector('.node-inspection-preview'), null); assert.equal(document.querySelector('.local-directory-empty'), null);
    await act(async () => deferredRead.reject(new Error('Artificial inspection failed')));
    assert.match(document.body.textContent, /Artificial inspection failed/); assert.equal(byText('添加'), undefined); assert.equal(byText('生成并添加项目'), undefined);
    assert.equal(document.querySelector('.local-directory-path input').value, '/artificial/node-source');
    assert.equal(calls.length, 3);
    await act(async () => root.unmount()); root = null;
  }
});

test('Cancelling a replacement folder chooser preserves the path and prompt but revokes its old actionable preview', async () => {
  let chooses = 0; let inspections = 0;
  await setupDirectory(async command => {
    if (command === 'choose_path') return ++chooses === 1 ? '/artificial/node-source' : null;
    if (command === 'node_app_directory_inspect') { inspections++; return directoryReview('needs-review'); }
    throw new Error(command);
  });
  await click(byText('选择文件夹…')); const prompt = document.querySelector('textarea').value;
  await click(byText('换一个'));
  assert.match(document.body.textContent, /已取消选择/); assert.equal(document.querySelector('.local-directory-path input').value, '/artificial/node-source');
  assert.equal(document.querySelector('textarea').value, prompt); assert.equal(inspections, 1);
  await click(byText('重新检查')); assert.equal(inspections, 2);
});

test('Pending local-folder actions reject duplicates and adding does not reuse a failed revision', async () => {
  const choosing = deferred(); const registering = deferred(); let registrations = 0; let reads = 0; let choices = 0; const added = [];
  await setupDirectory(async command => {
    if (command === 'choose_path') { choices++; return choosing.promise; }
    if (command === 'node_app_directory_inspect') return directoryReview('ready', { expectedRevision: `review-${++reads}` });
    if (command === 'node_app_directory_register') { registrations++; return registering.promise; }
    throw new Error(command);
  }, { onAdded: value => added.push(value) });
  await click(byText('选择文件夹…')); await click(byText('正在选择与检查…')); assert.equal(choices, 1);
  await act(async () => choosing.resolve('/artificial/node-source'));
  await click(byText('添加')); assert.equal(byText('添加'), undefined);
  await act(async () => registering.reject(new Error('Reviewed directory changed')));
  assert.equal(registrations, 1); assert.equal(byText('添加'), undefined); assert.match(document.body.textContent, /Reviewed directory changed/);
  await click(byText('重新检查')); assert.equal(reads, 2); assert.deepEqual(added, []);
});

test('Service changes discard late chooser, inspection, combined generation and registration responses without replaying writes', async () => {
  for (const delayedOperation of ['choose', 'inspect', 'initialize', 'register']) {
    const delayed = deferred(); const calls = []; const added = []; const gates = [];
    const props = { enabled: true, instanceId: 'old-directory-service', onPendingChange: value => gates.push(value), onAdded: value => added.push(value) };
    await setupDirectory(async (command, args) => {
      calls.push([command, args]);
      if (command === 'choose_path') return delayedOperation === 'choose' ? delayed.promise : '/artificial/node-source';
      if (command === 'node_app_directory_inspect') return delayedOperation === 'inspect' ? delayed.promise : directoryReview(delayedOperation === 'initialize' ? 'empty' : 'ready');
      if (command === 'node_app_directory_initialize_and_register' || command === 'node_app_directory_register') return delayed.promise;
      throw new Error(command);
    }, props);
    await click(byText('选择文件夹…'));
    if (delayedOperation === 'initialize') {
      await click(byText('生成并添加项目'));
    }
    if (delayedOperation === 'register') await click(byText('添加'));
    const submitted = calls.length;
    await act(async () => root.render(h(LocalDirectoryPanel, { ...props, instanceId: 'new-directory-service' })));
    const result = delayedOperation === 'choose' ? '/artificial/late-chooser' : ['initialize', 'register'].includes(delayedOperation) ? app({ sourcePath: '/artificial/node-source' }) : directoryReview();
    await act(async () => delayed.resolve(result));
    assert.equal(calls.length, submitted); assert.deepEqual(added, []); assert.equal(gates.at(-1), false);
    assert.equal(byText('添加'), undefined); assert.equal(byText('生成并添加项目'), undefined);
    assert.doesNotMatch(document.body.textContent, /late-chooser/);
    assert.equal(byText(delayedOperation === 'choose' ? '选择文件夹…' : '换一个').disabled, false);
    await act(async () => root.unmount()); root = null;
  }
});

test('Disabled local-folder access invalidates its unfinished review and keeps the selected path for an explicit recheck', async () => {
  const delayed = deferred(); let reads = 0;
  const props = { enabled: true, instanceId: 'same-service', onPendingChange() {}, onAdded() {} };
  await setupDirectory(async command => {
    if (command === 'choose_path') return '/artificial/node-source';
    if (command === 'node_app_directory_inspect') return ++reads === 1 ? delayed.promise : directoryReview();
    throw new Error(command);
  }, props);
  await click(byText('选择文件夹…'));
  await act(async () => root.render(h(LocalDirectoryPanel, { ...props, enabled: false })));
  await act(async () => delayed.resolve(directoryReview()));
  assert.equal(byText('重新检查').disabled, true); assert.equal(byText('添加'), undefined);
  await act(async () => root.render(h(LocalDirectoryPanel, props)));
  assert.equal(document.querySelector('.local-directory-path input').value, '/artificial/node-source');
  assert.equal(byText('添加'), undefined);
  await click(byText('重新检查')); assert.equal(reads, 2); assert.ok(byText('添加'));
});

test('Closing local-folder content prevents a late chooser from inspecting or initializing after unmount', async () => {
  const delayed = deferred(); const calls = []; const gates = [];
  await setupDirectory(async (command, args) => { calls.push([command, args]); assert.equal(command, 'choose_path'); return delayed.promise; }, { onPendingChange: value => gates.push(value) });
  await click(byText('选择文件夹…'));
  await act(async () => root.unmount()); root = null;
  await act(async () => delayed.resolve('/artificial/closed-folder'));
  assert.deepEqual(calls, [['choose_path', { kind: 'nodeapp' }]]); assert.equal(gates.at(-1), false);
});

test('A generate-and-add error keeps directory and name and needs a fresh empty review before another explicit attempt', async () => {
  let inspections = 0; let initializations = 0;
  const writes = [];
  await setupDirectory(async (command, args) => {
    if (command === 'choose_path') return '/artificial/node-source';
    if (command === 'node_app_directory_inspect') return directoryReview('empty', { expectedRevision: `empty-${++inspections}` });
    if (command === 'node_app_directory_initialize_and_register') { initializations++; writes.push(args); throw new Error('Artificial initialization needs inspection'); }
    throw new Error(command);
  });
  await click(byText('选择文件夹…')); await change(document.querySelector('.local-directory-empty input'), 'Preserved project name');
  await click(byText('生成并添加项目'));
  assert.equal(initializations, 1); assert.equal(byText('生成并添加项目'), undefined); assert.match(document.body.textContent, /Artificial initialization needs inspection/);
  assert.match(document.querySelector('[role=alert]').textContent, /已生成的文件会保留.*重新检查.*不要重复生成/);
  await click(byText('重新检查'));
  assert.equal(document.querySelector('.local-directory-empty input').value, 'Preserved project name');
  assert.equal(initializations, 1, 'Reinspection must not replay a write');
  await click(byText('生成并添加项目'));
  assert.equal(initializations, 2);
  assert.deepEqual(writes, [
    { sourcePath: '/artificial/node-source', input: { expectedRevision: 'empty-1', name: 'Preserved project name' } },
    { sourcePath: '/artificial/node-source', input: { expectedRevision: 'empty-2', name: 'Preserved project name' } },
  ]);
});

test('A generated-folder add failure requires reinspection and an explicit ready-folder add without regenerating files', async () => {
  for (const outcome of ['initialized_not_registered', 'initialized_registration_unconfirmed', 'registered_needs_attention']) {
    const calls = []; const added = []; let generated = false; let inspections = 0;
    await setupDirectory(async (command, args) => {
      calls.push([command, args]);
      if (command === 'choose_path') return '/artificial/node-source';
      if (command === 'node_app_directory_inspect') return directoryReview(generated ? 'ready' : 'empty', { expectedRevision: `after-inspection-${++inspections}` });
      if (command === 'node_app_directory_initialize_and_register') {
        generated = true;
        throw { code: 'NODE_DIRECTORY_ADD_FAILED', message: '起始项目文件已生成，但添加未能完成或确认；请保留文件并重新检查，不要再次生成或直接重复添加。', details: { sourcePath: '/artificial/node-source', initializationCompleted: true, partialFilesRetained: true, requiresRecheck: true, operationOutcome: outcome } };
      }
      if (command === 'node_app_directory_register') return app({ sourcePath: '/artificial/node-source' });
      throw new Error(command);
    }, { onAdded: value => added.push(value) });
    await click(byText('选择文件夹…')); await click(byText('生成并添加项目'));
    assert.deepEqual(added, []);
    assert.match(document.querySelector('[role=alert]').textContent, /文件已生成.*保留文件.*重新检查/);
    assert.equal(document.querySelector('.local-directory-path input').value, '/artificial/node-source');
    assert.equal(byText('生成并添加项目'), undefined); assert.equal(byText('添加'), undefined);
    assert.deepEqual(calls.map(([command]) => command), ['choose_path', 'node_app_directory_inspect', 'node_app_directory_initialize_and_register'], 'An incomplete or uncertain operation must never silently retry its stages');
    await click(byText('重新检查'));
    assert.equal(byText('生成并添加项目'), undefined, 'The already generated nonempty folder must use the ordinary ready path');
    assert.equal(byText('添加').disabled, false);
    assert.equal(calls.filter(([command]) => command === 'node_app_directory_initialize_and_register').length, 1);
    assert.deepEqual(added, []);
    await click(byText('添加'));
    assert.deepEqual(calls.at(-1), ['node_app_directory_register', { sourcePath: '/artificial/node-source', expectedRevision: 'after-inspection-2' }]);
    assert.equal(added.length, 1);
    assert.equal(calls.filter(([command]) => command === 'node_app_directory_initialize_and_register').length, 1);
    await act(async () => root.unmount()); root = null;
  }
});

test('Unknown or mismatched combined results never claim that the generated folder was added', async () => {
  for (const result of [undefined, app({ sourcePath: '/artificial/different-folder' }), app({ sourcePath: '/artificial/node-source', runtime: 'static-web' }), app({ sourcePath: '/artificial/node-source', configuration: null })]) {
    const calls = []; const added = [];
    await setupDirectory(async (command, args) => {
      calls.push([command, args]);
      if (command === 'choose_path') return '/artificial/node-source';
      if (command === 'node_app_directory_inspect') return directoryReview('empty');
      if (command === 'node_app_directory_initialize_and_register') return result;
      throw new Error(command);
    }, { onAdded: value => added.push(value) });
    await click(byText('选择文件夹…')); await click(byText('生成并添加项目'));
    assert.deepEqual(added, []);
    assert.ok(document.querySelector('[role=alert]'));
    assert.match(document.querySelector('[role=alert]').textContent, /重新检查.*不要重复生成/);
    assert.equal(byText('添加'), undefined); assert.equal(byText('生成并添加项目'), undefined);
    assert.deepEqual(calls.map(([command]) => command), ['choose_path', 'node_app_directory_inspect', 'node_app_directory_initialize_and_register']);
    await act(async () => root.unmount()); root = null;
  }
});

test('Adding rejects malformed or mismatched directory previews instead of making older material actionable', async () => {
  for (const result of [directoryReview('ready', { sourcePath: '/artificial/different-folder' }), directoryReview('ready', { inspection: undefined }), directoryReview('needs-review', { reviewPrompt: '' }), directoryReview('empty', { expectedRevision: '' })]) {
    await setupDirectory(async command => {
      if (command === 'choose_path') return '/artificial/node-source';
      assert.equal(command, 'node_app_directory_inspect'); return result;
    });
    await click(byText('选择文件夹…'));
    assert.ok(document.querySelector('[role=alert]')); assert.equal(byText('添加'), undefined); assert.equal(byText('生成并添加项目'), undefined);
    await act(async () => root.unmount()); root = null;
  }
});

test('Local add offers a reusable evaluation before choosing a folder and never starts or modifies a project', async () => {
  const calls = [];
  await setupDirectory(async (command, args) => {
    calls.push([command, args]);
    if (command === 'source_review_capabilities') return { codexAvailable: false };
    throw new Error(command);
  });
  assert.equal(document.querySelectorAll('[aria-label="添加进度"] li').length, 3);
  assert.ok(byText('不确定能不能用？先复制说明问问 AI'));
  assert.deepEqual(calls, []);
  await click(byText('不确定能不能用？先复制说明问问 AI'));
  assert.ok(byText('复制评估说明'));
  assert.match(document.body.textContent, /不填写时.*当前对话/);
  assert.deepEqual(calls.map(([command]) => command), ['source_review_capabilities']);
});

test('A previously registered external folder opens the existing application without inspection or a second registration', async () => {
  const selected = app({ sourcePath: '/artificial/node-source' });
  const calls = []; const enters = []; const details = [];
  await setupDirectory(async (command, args) => {
    calls.push([command, args]);
    if (command === 'choose_path') return selected.sourcePath;
    throw new Error(command);
  }, { apps: [selected], onEnter: value => enters.push(value), onViewDetails: value => details.push(value) });
  await click(byText('选择文件夹…'));
  assert.match(document.body.textContent, /这个文件夹已经添加过了/);
  assert.equal(byText('添加'), undefined);
  assert.deepEqual(calls.map(([command]) => command), ['choose_path']);
  await click(byText('查看应用详情')); assert.deepEqual(details, [selected]);
  await click(byText(`打开「${selected.name}」`)); assert.deepEqual(enters, [selected]);
  assert.equal(calls.some(([command]) => /inspect|register|start/.test(command)), false);
});

test('A native directory-identity match also prevents duplicate addition when its path differs', async () => {
  const selected = app({ id: 'existing_alias', sourcePath: '/artificial/original-source' });
  const calls = []; const enters = [];
  await setupDirectory(async (command, args) => {
    calls.push([command, args]);
    if (command === 'choose_path') return '/artificial/node-source';
    if (command === 'node_app_directory_inspect') return directoryReview('needs-review', { existingApp: { id: selected.id, name: selected.name } });
    if (command === 'node_app_configuration') return selected;
    throw new Error(command);
  }, { onEnter: value => enters.push(value) });
  await click(byText('选择文件夹…'));
  assert.match(document.body.textContent, /这个文件夹已经添加过了/);
  await click(byText(`打开「${selected.name}」`));
  assert.deepEqual(enters, [selected]);
  assert.deepEqual(calls.at(-1), ['node_app_configuration', { appId: selected.id }]);
  assert.equal(calls.some(([command]) => command === 'node_app_directory_register'), false);
});

test('Local result categories preserve the native uncertainty and never invent an add action', async () => {
  for (const [category, label] of [['needs-changes', '需要修改'], ['unsupported', '当前不适合'], ['insufficient', '信息不足']]) {
    await setupDirectory(async command => {
      if (command === 'choose_path') return '/artificial/node-source';
      if (command === 'node_app_directory_inspect') return directoryReview('needs-review', { category, issues: ['缺少 local-app.json'] });
      throw new Error(command);
    });
    await click(byText('选择文件夹…'));
    assert.equal(document.querySelector('.local-directory-review .local-directory-result-label').textContent, label);
    assert.match(document.querySelector('.local-directory-location').textContent, /文件夹保持原样/);
    assert.equal(byText('添加'), undefined); assert.equal(byText('生成并添加项目'), undefined);
    assert.ok(byText(category === 'unsupported' ? '复制说明问 AI 有没有替代办法' : '复制说明给 AI'));
    await act(async () => root.unmount()); root = null;
  }
});

test('Recent folders retain only five paths in the same workspace and always perform a fresh inspection', async () => {
  const paths = Array.from({ length: 6 }, (_, index) => `/artificial/recent-folder-${index + 1}`);
  let choices = 0; const reads = [];
  const mock = async (command, args) => {
    if (command === 'choose_path') return paths[choices++];
    if (command === 'node_app_directory_inspect') { reads.push(args.sourcePath); return directoryReview('needs-review', { sourcePath: args.sourcePath, issues: ['缺少 local-app.json'] }); }
    throw new Error(command);
  };
  await setupDirectory(mock, { workspacePath: '/artificial/recent-workspace', instanceId: 'before-restart' });
  for (let index = 0; index < paths.length; index++) await click(byText(index ? '换一个' : '选择文件夹…'));
  assert.equal(reads.length, 6);
  await act(async () => root.unmount()); root = null;
  await setupDirectory(mock, { workspacePath: '/artificial/recent-workspace', instanceId: 'after-restart' });
  const recent = document.querySelectorAll('.local-directory-recent');
  assert.equal(recent.length, 5);
  assert.equal(recent[0].querySelector('button').title, paths[5]);
  assert.equal(recent[0].querySelector('strong').textContent, 'recent-folder-6');
  assert.doesNotMatch(document.querySelector('.local-directory-recents').textContent, /recent-folder-1/);
  await click(recent[0].querySelector('button'));
  assert.equal(reads.length, 7); assert.equal(reads.at(-1), paths[5]);
  assert.equal(choices, 6, 'Using recent choices should not reopen the native directory picker');
  await act(async () => root.unmount()); root = null;
  await setupDirectory(mock, { workspacePath: '/artificial/different-workspace' });
  assert.equal(document.querySelector('.local-directory-recents'), null);
});

test('A fresh recheck compares actual issues and never reuses the prior revision for adding', async () => {
  let reads = 0; const writes = [];
  await setupDirectory(async (command, args) => {
    if (command === 'choose_path') return '/artificial/node-source';
    if (command === 'node_app_directory_inspect') return ++reads === 1
      ? directoryReview('needs-review', { issues: ['缺少 local-app.json', '缺少构建结果'] })
      : directoryReview('ready', { expectedRevision: 'fresh-after-adaptation', issues: [] });
    if (command === 'node_app_directory_register') { writes.push(args); return app({ sourcePath: '/artificial/node-source' }); }
    throw new Error(command);
  });
  await click(byText('选择文件夹…')); await click(byText('重新检查'));
  assert.match(document.querySelector('.local-directory-diff').textContent, /已解决 2 项，还剩 0 项/);
  await click(byText('添加'));
  assert.deepEqual(writes, [{ sourcePath: '/artificial/node-source', expectedRevision: 'fresh-after-adaptation' }]);
});

test('Input conversion refuses unsafe integers and preserves an explicit false value', () => {
  const value = app();
  assert.throws(() => nodeConfigurationInput(value, { ordinary: { LIMIT: '9007199254740993' }, secrets: {} }), /精确保存的整数/);
  assert.deepEqual(nodeConfigurationInput(value, { ordinary: { LIMIT: '0', ENABLED: 'false', MODEL: ' ' }, secrets: {} }), { revision: 1, values: { LIMIT: 0, ENABLED: false }, secrets: { AI_KEY: { action: 'keep' } } });
});

test('A late configuration read from an old service instance cannot replace the same app draft or leak its error', async () => {
  for (const outcome of ['resolve', 'reject']) {
    const value = app(); const delayed = deferred(); const changes = []; let reads = 0;
    const props = { enabled: true, instanceId: 'old', onDraftStateChange() {}, onAppChanged: item => changes.push(item), request: { app: value, intent: 'details', token: 1 } };
    await setup(async (command) => {
      assert.equal(command, 'node_app_configuration');
      return ++reads === 1 ? delayed.promise : value;
    }, props);
    assert.equal(document.querySelector('.node-app-detail').getAttribute('aria-busy'), 'true');
    await act(async () => root.render(h(NodeAppsPanel, { ...props, instanceId: 'new' })));
    assert.equal(document.querySelector('.node-app-detail').getAttribute('aria-busy'), 'false');
    await click(byText('修改'));
    await change(input('MODEL'), 'current instance model');
    await change(input('AI_KEY'), 'artificial-current-instance-key');
    const count = changes.length;
    await act(async () => outcome === 'resolve'
      ? delayed.resolve(app({ name: 'Old service result', configuration: { revision: 99, values: { MODEL: 'old-service-model' }, secretStatus: { AI_KEY: 'set' }, missingRequired: [], canStart: true } }))
      : delayed.reject(new Error('Artificial old read error')));
    assert.equal(changes.length, count);
    assert.equal(input('MODEL').value, 'current instance model');
    assert.equal(input('AI_KEY').value, 'artificial-current-instance-key');
    assert.doesNotMatch(document.body.textContent, /Old service result|old-service-model|Artificial old read error/);
    assert.equal(reads, 1, 'An instance change never replays a configuration read');
    await click(document.querySelector('[aria-label^="关闭Disposable"]')); await click(byText('丢弃草稿并关闭'));
    await act(async () => root.render(h(NodeAppsPanel, { ...props, instanceId: 'new', request: { app: value, intent: 'configure', token: 2 } })));
    assert.equal(reads, 2);
    assert.equal(input('MODEL').value, ''); assert.equal(input('AI_KEY').value, '');
    await act(async () => root.unmount()); root = null;
  }
});

test('A save crossing a service-instance change remains single-flight and ignores its late result without discarding the draft', async () => {
  for (const outcome of ['resolve', 'reject']) {
    const value = app(); const delayed = deferred(); const changes = []; const writes = []; const states = [];
    const props = { enabled: true, instanceId: 'old', onDraftStateChange: item => states.push(item), onAppChanged: item => changes.push(item), request: { app: value, intent: 'details', token: 1 } };
    await setup(baseMock(value, async (command, args) => {
      assert.equal(command, 'node_app_configuration_save'); writes.push(args);
      if (writes.length === 1) return delayed.promise;
      throw new Error('Artificial explicit retry conflict');
    }), props);
    await change(input('MODEL'), 'draft across instance change');
    await change(input('AI_KEY'), 'artificial-cross-instance-key');
    await saveAndContinueEditing();
    assert.equal(writes.length, 1);
    await act(async () => root.render(h(NodeAppsPanel, { ...props, instanceId: 'new' })));
    assert.equal(input('MODEL').disabled, true);
    assert.equal(byText('保存中…').disabled, true);
    assert.deepEqual(states.at(-1), { dirty: true, pending: true });
    await saveAndContinueEditing();
    assert.equal(writes.length, 1, 'Pending native writes remain single-flight across an instance change');
    const count = changes.length;
    await act(async () => outcome === 'resolve'
      ? delayed.resolve(app({ configuration: { revision: 99, values: { MODEL: 'late-saved-value' }, secretStatus: { AI_KEY: 'set' }, missingRequired: [], canStart: true } }))
      : delayed.reject(new Error('Artificial old save error')));
    assert.equal(changes.length, count);
    assert.equal(input('MODEL').disabled, false);
    assert.equal(input('MODEL').value, 'draft across instance change');
    assert.equal(input('AI_KEY').value, 'artificial-cross-instance-key');
    assert.deepEqual(states.at(-1), { dirty: true, pending: false });
    assert.doesNotMatch(document.body.textContent, /late-saved-value|Artificial old save error|配置已保存/);
    assert.equal(writes.length, 1, 'No write is replayed automatically');
    await saveAndContinueEditing();
    assert.equal(writes.length, 2);
    assert.equal(writes[1].input.revision, 1);
    assert.equal(writes[1].input.values.MODEL, 'draft across instance change');
    assert.deepEqual(writes[1].input.secrets.AI_KEY, { action: 'replace', value: 'artificial-cross-instance-key' });
    assert.match(document.querySelector('[role=alert]').textContent, /Artificial explicit retry conflict/);
    await act(async () => root.unmount()); root = null;
  }
});

test('A configuration read with another application identity never replaces metadata or publishes that result', async () => {
  const value = app(); const changes = [];
  await setup(async command => {
    assert.equal(command, 'node_app_configuration');
    return app({ id: 'another-app', name: 'Unrelated app returned by fixture' });
  }, { request: { app: value, intent: 'details', token: 1 }, onAppChanged: item => changes.push(item) });
  assert.match(document.querySelector('[role=alert]').textContent, /应用身份不符/);
  assert.equal(document.querySelector('[role=dialog] h2').textContent, value.name);
  assert.doesNotMatch(document.body.textContent, /Unrelated app returned by fixture/);
  assert.deepEqual(changes, []);
});

test('A mismatched save result preserves ordinary and secret drafts and never claims the configuration was saved', async () => {
  const changes = [];
  await setup(baseMock(app(), async command => {
    assert.equal(command, 'node_app_configuration_save');
    return app({ id: 'another-app', configuration: { revision: 99, values: { MODEL: 'unrelated saved model' }, secretStatus: { AI_KEY: 'set' }, missingRequired: [], canStart: true } });
  }), { onAppChanged: item => changes.push(item) });
  const count = changes.length;
  await change(input('MODEL'), 'my model after mismatched save');
  await change(input('AI_KEY'), 'artificial-mismatched-save-key');
  await saveAndContinueEditing();
  assert.equal(changes.length, count);
  assert.match(document.querySelector('[role=alert]').textContent, /应用身份不符/);
  assert.equal(input('MODEL').value, 'my model after mismatched save');
  assert.equal(input('AI_KEY').value, 'artificial-mismatched-save-key');
  assert.doesNotMatch(document.body.textContent, /unrelated saved model|配置已保存/);
});

test('Status observations never replace a configuration draft or its original revision', async () => {
  const originalInterval = window.setInterval;
  const ticks = [];
  window.setInterval = (callback) => { ticks.push(callback); return ticks.length; };
  const calls = [];
  try {
    await setup(baseMock(app(), async (command, args) => {
      if (command === 'node_app_status') return app({ state: 'running', configuration: { revision: 99, values: { MODEL: 'other-window-model' }, secretStatus: { AI_KEY: 'set' }, missingRequired: [], canStart: true } });
      assert.equal(command, 'node_app_configuration_save'); calls.push(args); throw new Error('Fixture revision conflict');
    }));
    await change(input('MODEL'), 'my-draft'); await change(input('AI_KEY'), 'my-artificial-draft-key');
    await showDetails();
    await act(async () => { ticks.at(-1)(); });
    await click(byText('修改'));
    assert.equal(input('MODEL').value, 'my-draft');
    assert.equal(input('AI_KEY').value, 'my-artificial-draft-key');
    await saveAndContinueEditing();
    assert.equal(calls[0].input.revision, 1);
    assert.equal(calls[0].input.values.MODEL, 'my-draft');
  } finally { window.setInterval = originalInterval; }
});

test('Node detail tabs retain secret drafts and closing asks before discarding them', async () => {
  await setup(baseMock(app(), async (command) => { throw new Error(command); }));
  await change(input('AI_KEY'), 'artificial-unsaved-tab-key');
  await showDetails(); await showDetails(); await click(byText('修改'));
  assert.equal(input('AI_KEY').value, 'artificial-unsaved-tab-key');
  await click(document.querySelector('[aria-label^="关闭Disposable"]'));
  assert.ok(document.querySelector('[aria-label="关闭配置确认"]'));
  await click(byText('继续编辑'));
  assert.equal(input('AI_KEY').value, 'artificial-unsaved-tab-key');
  await click(document.querySelector('[aria-label^="关闭Disposable"]'));
  await click(byText('丢弃草稿并关闭'));
  assert.equal(document.querySelector('[role="dialog"]'), null);
});

function shellMock(state, action = async (command) => { throw new Error(command); }) {
  return async (command, args) => {
    if (command === 'bootstrap_info') return { displayName: '熊猫小抽屉', version: '0.0.0', identifier: 'dev.happypandaa.pandadrawer', instanceId: 'instance_dom_fixture', workspaceRoot: '/disposable/workspace', dataRoot: '/disposable/development', resources: { sdk: true, template: true, readme: true, cli: true } };
    if (command === 'project_list') return state.projects ?? [];
    if (command === 'workspace_info') return { workspaceRoot: '/disposable/workspace', migrationState: 'ready', retainedRoots: [] };
    if (command === 'maintenance_status') return { mode: 'ready', recoveryRequired: false };
    if (command === 'node_app_list') return state.nodes ?? [];
    if (command === 'node_app_configuration') return state.nodes.find(value => value.id === args.appId);
    return action(command, args);
  };
}

test('Restarted Node deletion journals remain visible with no registered app and reopen without reading its missing preview', async () => {
  const value = app({ name: 'Disposable interrupted removal' }); const state = { nodes: [] }; const calls = []; let journalReads = 0;
  const task = deletionTask(value, { phase: 'failed', journalRevision: 9, renamedAny: true, canAbandon: false, canRetry: true, error: { code: 'DISPOSABLE_RESTART', message: 'Deletion stopped after restart' } });
  await setup(shellMock(state, async (command, args) => { calls.push([command, args]); throw new Error(command); }), { deletionTasksMock: () => { journalReads++; return [task]; } }, App);
  assert.ok(journalReads > 0, 'Journal discovery must not depend on nodeApps.length');
  assert.equal(document.querySelectorAll('.project-card').length, 0);
  const notice = document.querySelector('[aria-label="未完成的应用删除"]');
  assert.ok(notice); assert.match(notice.textContent, /Disposable interrupted removal.*删除未完成/);
  await click(byText('处理删除'));
  assert.match(deleteDialog().textContent, /Deletion stopped after restart/);
  assert.match(deleteDialog().textContent, new RegExp(task.taskId));
  assert.equal(byText('重试删除').disabled, false); assert.equal(byText('放弃删除'), undefined);
  assert.equal(calls.some(([command]) => ['node_app_delete_preview', 'node_app_configuration', 'node_app_start'].includes(command)), false, 'Restoring a task must not recreate its instance or load a preview from a moved directory');
});

test('A journal-bound instance is excluded from launch cards while its independent deletion task stays visible', async () => {
  const value = app({ name: 'Disposable gated launcher' }); const task = deletionTask(value, { phase: 'stopping' });
  await setup(shellMock({ nodes: [value] }), { deletionTasksMock: () => [task] }, App);
  assert.equal(byLabel(`进入项目「${value.name}」`), undefined);
  assert.equal(document.querySelectorAll('.project-card').length, 0);
  assert.match(document.querySelector('[aria-label="未完成的应用删除"]').textContent, /Disposable gated launcher/);
  assert.equal(byText('查看删除进度').disabled, false);
});

test('One generate-and-add click puts an empty local folder in the App list and survives refresh', async () => {
  const sourcePath = '/artificial/node-source';
  const value = app({ name: 'Disposable initialized starter', sourcePath, fields: [], configuration: { revision: 1, values: {}, secretStatus: {}, missingRequired: [], canStart: true } });
  const completed = deferred();
  const state = { nodes: [] }; const calls = [];
  const mock = shellMock(state, async (command) => {
    if (command === 'choose_path') return sourcePath;
    if (command === 'node_app_directory_inspect') return directoryReview('empty', { reviewPrompt: 'Artificial retained empty-folder AI prompt' });
    if (command === 'node_app_directory_initialize_and_register') {
      const result = await completed.promise; state.nodes = [result]; return result;
    }
    throw new Error(command);
  });
  await setup(async (command, args) => { calls.push([command, args]); return mock(command, args); }, {}, App);
  assert.equal(document.querySelectorAll('.project-card').length, 0);
  await click(byText('本机文件夹')); await click(byText('选择文件夹…'));
  await change(document.querySelector('.local-directory-empty input'), value.name);
  await click(byText('生成并添加项目'));
  assert.deepEqual(calls.filter(([command]) => command === 'node_app_directory_initialize_and_register'), [['node_app_directory_initialize_and_register', { sourcePath, input: { expectedRevision: 'directory-revision-1', name: value.name } }]]);
  assert.equal(calls.some(([command]) => command === 'node_app_directory_register'), false);
  assert.equal(document.querySelectorAll('.project-card').length, 0);
  assert.equal(document.querySelector('[role="dialog"] [aria-label^="关闭"]').disabled, true);
  assert.equal(byText('重新检查').disabled, true);
  assert.match(document.querySelector('[role=dialog]').textContent, /正在生成并添加项目/);
  assert.equal(document.querySelector('.local-directory-prompt'), null);
  assert.doesNotMatch(document.querySelector('[role=dialog]').textContent, /Artificial retained empty-folder AI prompt/);
  const addButtons = [...document.querySelectorAll('.application-heading [aria-label="添加应用"] button')];
  assert.equal(addButtons.length, 3);
  assert.ok(addButtons.every(button => button.disabled));
  assert.equal(byText('添加'), undefined);
  await act(async () => completed.resolve(value));
  assert.equal(document.querySelectorAll('.project-card').length, 1, 'The registration callback must update the real App list immediately');
  assert.ok(byLabel(`进入项目「${value.name}」`));
  assert.match(document.querySelector('.application-count').textContent, /1 个/);
  assert.ok(document.querySelector('.local-directory-panel'), 'A successful combined command keeps the completion step visible');
  assert.match(document.querySelector('[role=dialog]').textContent, /添加成功/);
  assert.equal(document.querySelector('[role=tab]'), null);
  assert.equal(byText('添加'), undefined);
  assert.equal(byText('打开应用').disabled, false);
  assert.equal(byText('完成').disabled, false);
  assert.match(document.querySelector('[role=dialog]').textContent, /文件数据在这个文件夹的 data\//);
  await click(byText('完成'));
  assert.equal(document.querySelector('[role="dialog"]'), null);
  assert.equal(document.querySelectorAll('.project-card').length, 1);
  const listReads = calls.filter(([command]) => command === 'node_app_list').length;
  await act(async () => window.dispatchEvent(new window.Event('focus')));
  assert.ok(calls.filter(([command]) => command === 'node_app_list').length > listReads);
  assert.equal(document.querySelectorAll('.project-card').length, 1, 'A fresh backend list must retain the registered application');
  assert.ok(byLabel(`进入项目「${value.name}」`));
  assert.equal(calls.some(([command]) => ['node_app_start', 'node_app_restart', 'node_app_configuration_save'].includes(command)), false, 'Initialization and adding must not run the starter or require an empty configuration save');
  assert.equal(calls.filter(([command]) => command === 'node_app_directory_initialize_and_register').length, 1);
  assert.equal(calls.some(([command]) => ['node_app_directory_initialize', 'node_app_directory_register'].includes(command)), false, 'The UI must submit one native combined operation rather than replaying its stages');
});

test('Deleting from a Node launcher action removes the app only after an explicit deleted result', async () => {
  for (const metadataCleanupPending of [false, true]) {
    const value = app({ id: `node_delete_${metadataCleanupPending}` }); const state = { nodes: [value] }; const writes = []; const calls = []; const completed = deferred();
    const mock = shellMock(state, async (command, args) => {
      if (command === 'node_app_delete_preview') return deletionPreview(value, 'external');
      if (command === 'node_app_delete_submit') {
        writes.push(args); return completed.promise;
      }
      throw new Error(command);
    });
    await setup((command, args) => { calls.push([command, args]); return mock(command, args); }, {}, App);
    const entry = document.querySelector(`[aria-label='进入项目「${value.name}」']`);
    await act(async () => entry.closest('article').dispatchEvent(new window.MouseEvent('contextmenu', { bubbles: true, cancelable: true })));
    await click(document.querySelector(`[aria-label='删除项目「${value.name}」']`));
    assert.equal(writes.length, 0);
    assert.equal(document.querySelectorAll('.project-card').length, 1);
    assert.equal(document.querySelectorAll('[role=dialog]').length, 1);
    assert.equal(deleteDialog().querySelector('h2').textContent, '删除项目');
    assert.ok(deleteDialog().textContent.includes(value.name));
    assert.equal(document.querySelector('.node-detail-modal'), null);
    assert.equal(calls.some(([command]) => ['node_app_configuration', 'node_app_status', 'source_list'].includes(command)), false, 'Launcher deletion reads only its dedicated deletion preview');
    await click(document.querySelector('.node-delete-confirmation input')); await click(byText('确认永久删除项目'));
    assert.equal(writes.length, 1); assert.equal(document.querySelectorAll('.project-card').length, 1, 'A submitted deletion must keep its card until the result confirms deletion');
    state.nodes = [];
    await act(async () => completed.resolve({ appId: value.id, operationOutcome: 'deleted', ownership: 'external', sourcePreserved: true, evidenceRetained: false, metadataCleanupPending }));
    assert.equal(writes.length, 1); assert.equal(document.querySelectorAll('.project-card').length, 0);
    assert.equal(document.querySelector('[role=dialog]'), null);
    const feedback = document.querySelector(metadataCleanupPending ? '.notice-error' : '.notice-success');
    assert.match(feedback.textContent, /平台管理内容已永久删除.*外部原目录保持原样/);
    assert.doesNotMatch(feedback.textContent, /文件已永久删除/);
    if (metadataCleanupPending) {
      assert.equal(document.querySelector('.notice-success'), null);
      assert.match(feedback.textContent, /收尾尚未完成.*不要再次提交删除/);
    } else assert.doesNotMatch(feedback.textContent, /收尾尚未完成/);
    assert.equal(writes.length, 1);
    await act(async () => root.unmount()); root = null;
  }
});

test('All project types share My Applications and counts without navigation or search', async () => {
  const state = { nodes: [app()], projects: [{ id: 'prj_static', name: 'Static fixture', description: 'Artificial webpage', state: 'ready', issues: [], projectRoot: '/artificial/static', manifest: null }] };
  await setup(shellMock(state), {}, App);
  assert.equal(document.querySelectorAll('.project-card').length, 2);
  assert.match(document.querySelector('.application-heading-main').textContent, /2 个/);
  assert.equal(document.querySelector('.radio-primary-nav'), null);
  assert.equal(document.querySelector('input[type="search"]'), null);
  assert.match(document.querySelector('.radio-footer').textContent, /项目文件夹/);
});

test('Entering a Node project with a missing key opens its configuration, saving never opens it automatically', async () => {
  const calls = [];
  const state = { nodes: [app({ configuration: { revision: 1, values: {}, secretStatus: { AI_KEY: 'missing' }, missingRequired: ['AI_KEY'], canStart: false } })] };
  await setup(shellMock(state, async (command, args) => {
    calls.push([command, args]);
    if (command === 'node_app_configuration_save') {
      const value = app({ configuration: { revision: 2, values: {}, secretStatus: { AI_KEY: 'set' }, missingRequired: [], canStart: true } });
      state.nodes = [value]; return value;
    }
    if (command === 'node_app_start') return app({ state: 'running', runningRevision: 2, configuration: state.nodes[0].configuration });
    throw new Error(command);
  }), {}, App);
  const enter = document.querySelector('.project-card .primary-button');
  assert.equal(enter.disabled, false);
  await click(enter);
  assert.equal((document.querySelector('.node-configuration-form') ? '应用配置' : '详情'), '应用配置');
  assert.equal(calls.length, 0);
  await change(input('AI_KEY'), 'artificial-explicit-entry-key'); await saveAndContinueEditing();
  assert.equal(calls.length, 1); assert.equal(calls[0][0], 'node_app_configuration_save');
  await click([...document.querySelectorAll('[role=dialog] button')].find(value => value.textContent === '打开'));
  assert.deepEqual(calls[1], ['node_app_start', { appId: 'node_fixture' }]);
  assert.equal(input('AI_KEY').value, '');
});

test('Node details reopen without a secret and expose only supported tabs and actions', async () => {
  const state = { nodes: [app()] };
  await setup(shellMock(state), {}, App);
  await openDetails('Disposable Node fixture');
  assert.equal((document.querySelector('.node-configuration-form') ? '应用配置' : '详情'), '详情');
  assert.equal(document.querySelector('[role=tablist]'), null);
  assert.ok(byText('停止并删除…'));
  assert.equal([...document.querySelectorAll('[role=dialog] button')].some(value => /备份维护/.test(value.textContent)), false);
  await click(byText('修改')); await change(input('AI_KEY'), 'artificial-close-draft');
  await click(document.querySelector('[aria-label^="关闭Disposable"]'));
  await click(byText('丢弃草稿并关闭'));
  assert.equal(document.querySelector('[role=dialog]'), null);
  await openDetails('Disposable Node fixture'); await click(byText('修改'));
  assert.equal(input('AI_KEY').value, '');
});

test('Closing read-only details revokes a late configuration result before another project is selected', async () => {
  const first = app(); const second = app({ id: 'node_second', name: 'Second artificial fixture' });
  const delayed = deferred();
  const state = { nodes: [first, second] };
  await setup(async (command, args) => {
    if (command === 'node_app_configuration' && args.appId === first.id) return delayed.promise;
    return shellMock(state)(command, args);
  }, {}, App);
  await openDetails('Disposable Node fixture');
  await click(document.querySelector('[aria-label^="关闭Disposable"]'));
  assert.equal(document.querySelector('[role=dialog]'), null);
  assert.equal(document.querySelector('[aria-label="进入项目「Second artificial fixture」"]').disabled, false);
  assert.equal(document.querySelector('[aria-label="设置"]').disabled, false);
  await openDetails('Second artificial fixture');
  await act(async () => delayed.resolve(app({ name: 'Outdated first project', configuration: { revision: 99, values: { MODEL: 'stale-model' }, secretStatus: { AI_KEY: 'set' }, missingRequired: [], canStart: true } })));
  assert.equal(document.querySelector('[role=dialog] h2').textContent, second.name);
  await click(byText('修改'));
  assert.equal(input('MODEL').value, '');
});

test('Returning between details and configuration preserves drafts without tab navigation', async () => {
  await setup(baseMock(app(), async (command) => { throw new Error(command); }));
  await change(input('AI_KEY'), 'artificial-reload-draft');
  assert.equal(byText('重新读取配置'), undefined);
  await showDetails(); await click(byText('修改'));
  assert.equal(document.querySelectorAll('[role=dialog]').length, 1);
  assert.equal(input('AI_KEY').value, 'artificial-reload-draft');
  assert.equal(document.querySelector('[role=tablist]'), null);
});

test('Pending Node entry blocks competing create, import and static detail dialogs until its failure settles', async () => {
  const configuration = deferred();
  const state = { nodes: [app()], projects: [{ id: 'prj_static', name: 'Static fixture', description: '', state: 'ready', issues: [], projectRoot: '/artificial/static', manifest: null }] };
  await setup(async (command, args) => {
    if (command === 'node_app_configuration') return configuration.promise;
    return shellMock(state)(command, args);
  }, {}, App);
  await click(document.querySelector('[aria-label="进入项目「Disposable Node fixture」"]'));
  assert.equal(document.querySelector('[role=dialog]'), null);
  const staticTile = document.querySelector('[aria-label="进入项目「Static fixture」"]').closest('article');
  assert.equal(document.querySelector('.launcher-more'), null);
  assert.equal(byText('新建').disabled, true);
  assert.equal(byText('本机文件夹').disabled, true);
  assert.equal(byText('从 Git 添加').disabled, true);
  await act(async () => staticTile.dispatchEvent(new window.MouseEvent('contextmenu', { bubbles: true, cancelable: true })));
  await click(byText('新建')); await click(byText('本机文件夹')); await click(byText('从 Git 添加'));
  assert.equal(document.querySelector('[role=menu]'), null);
  assert.equal(document.querySelector('[role=dialog]'), null);
  await act(async () => configuration.reject(new Error('Artificial configuration read failure')));
  assert.equal(document.querySelectorAll('[role=dialog]').length, 1);
  assert.equal(document.querySelector('[role=dialog] h2').textContent, 'Disposable Node fixture');
  assert.match(document.querySelector('[role=dialog]').textContent, /Artificial configuration read failure/);
});



test('A Node handoff from a previous instance never reaches the clipboard or current preview', async () => {
  const delayed = deferred(); const copied = []; const value = app();
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async text => copied.push(text) } });
  const props = { enabled: true, instanceId: 'old', onDraftStateChange() {}, request: { app: value, intent: 'details', token: 1 } };
  await setup(baseMock(value, async command => { assert.equal(command, 'node_app_handoff'); return delayed.promise; }), props);
  await showDetails(); await click(byText('复制提示词'));
  assert.equal(byText('正在准备说明…').disabled, true);
  await act(async () => root.render(h(NodeAppsPanel, { ...props, instanceId: 'new' })));
  await act(async () => delayed.resolve({ instanceId: 'old', projectRoot: '/disposable/old', text: 'obsolete handoff' }));
  assert.deepEqual(copied, []); assert.equal(document.querySelector('.handoff-preview'), null);
  assert.equal(byText('打开').disabled, false);
});


test('Returning from a closed business window refreshes overview actions without overview polling', async () => {
  const value = app({ state: 'running' }); let reads = 0;
  await setup(baseMock(value, async command => { assert.equal(command, 'node_app_status'); reads++; return { ...value, state: 'stopped' }; }));
  await showDetails(); assert.equal(byText('应用修改'), undefined);
  await act(async () => window.dispatchEvent(new window.Event('focus')));
  assert.equal(reads, 1); assert.equal(byText('应用修改'), undefined);
  assert.equal(byText('打开').disabled, false);
});


test('Entering after a stopped save clears its success notice without offering an unnecessary restart', async () => {
  const configuration = { ...app().configuration, revision: 2, values: { MODEL: 'saved-before-entry' } };
  await setup(baseMock(app(), async command => {
    if (command === 'node_app_configuration_save') return app({ configuration });
    assert.equal(command, 'node_app_start'); return app({ state: 'running', runningRevision: 2, configuration });
  }));
  await change(input('MODEL'), 'saved-before-entry'); await saveAndContinueEditing();
  await showDetails(); assert.match(document.body.textContent, /已保存，下次进入时生效/);
  await click(byText('打开'));
  assert.equal(byText('立即重启'), undefined);
  assert.doesNotMatch(document.body.textContent, /重启后生效|下次进入时生效/);
});


function removedFixture(extra = {}) { return { version: 1, kind: 'node', id: 'node_deleted_A', instanceId: 'directory-instance', taskIds: ['task_deleted_A'], sourceIds: [], sourceRoots: ['/artificial/deleted-A'], ...extra }; }
test('A deletion notice revokes a pending local inspection and cannot recreate recent paths from its late reply', async () => {
  observeDeletionRuntime('directory-instance'); const inspection = deferred();
  await setupDirectory(async command => { if (command === 'choose_path') return '/artificial/deleted-A'; if (command === 'node_app_directory_inspect') return inspection.promise; throw new Error(command); });
  await click(byText('选择文件夹…'));
  await act(async () => acceptProjectDeletion(removedFixture()));
  await act(async () => inspection.resolve(directoryReview('needs-review', { sourcePath: '/artificial/deleted-A', issues: ['Private stale A issue'], reviewPrompt: 'Private stale A prompt' })));
  assert.doesNotMatch(document.body.textContent, /deleted-A|Private stale A/);
  assert.equal(window.localStorage.length, 0);
});
test('An unrelated deletion preserves a Node configuration draft and a matching deletion clears it', async () => {
  observeDeletionRuntime('directory-instance');
  const value = app({ id: 'node_B' });
  await setup(baseMock(value), { instanceId: 'directory-instance', request: { app: value, intent: 'configure', token: 1 } });
  await change(input('AI_KEY'), 'Keep B private draft');
  await act(async () => acceptProjectDeletion(removedFixture()));
  assert.equal(input('AI_KEY').value, 'Keep B private draft');
  await act(async () => acceptProjectDeletion(removedFixture({ id: value.id, sourceRoots: [] })));
  assert.equal(document.querySelector('[role=dialog]'), null);
});

test('Session recent folders forget only their deleted owner and never persist into a new manager context', async () => {
  observeDeletionRuntime('directory-instance'); const scope = '/artificial/recent-owned-workspace';
  for (const owner of ['A', 'B']) saveRecentProjectFolder(scope, { path: `/artificial/deleted-${owner}`, selectedAt: new Date().toISOString(), result: '已添加', issues: [], ownerId: `node_deleted_${owner}` });
  await setupDirectory(async command => { throw new Error(command); }, { workspacePath: scope });
  assert.equal(document.querySelectorAll('.local-directory-recent').length, 2);
  await act(async () => acceptProjectDeletion(removedFixture()));
  assert.deepEqual(readRecentProjectFolders(scope).map(item => item.ownerId), ['node_deleted_B']);
  assert.match(document.querySelector('.local-directory-recents').textContent, /deleted-B/);
  assert.doesNotMatch(document.querySelector('.local-directory-recents').textContent, /deleted-A/);
  assert.equal(window.localStorage.length, 0);
  await act(async () => root.unmount()); root = null; resetRecentProjectFoldersForTests();
  await setupDirectory(async command => { throw new Error(command); }, { workspacePath: scope });
  assert.equal(document.querySelector('.local-directory-recents'), null);
});

test('A new required configuration discovered at launch opens its refreshed configuration form', async () => {
  const initial=app(); const changed=app({configuration:{...initial.configuration,canStart:false,missingRequired:['AI_KEY'],secretStatus:{AI_KEY:'missing'}}});let reads=0;const calls=[];
  await setup(async(command,args)=>{
    calls.push(command);
    if(command==='node_app_configuration')return ++reads===1?initial:changed;
    if(command==='node_app_start')throw {code:'NODE_CONFIGURATION_REQUIRED',message:'需要配置'};
    if(command==='node_app_status')return initial;
    throw Error(command);
  });
  await click(byText('打开'));
  assert.equal((document.querySelector('.node-configuration-form') ? '应用配置' : '详情'),'应用配置');
  assert.equal(document.querySelector('.node-launch-error'),null);assert.equal(calls.filter(command=>command==='node_app_start').length,1);assert.equal(reads,2);
});
test('Configuration submission carries the displayed manifest revision', () => {
 const value=app({manifestDigest:'a'.repeat(64)});const input=nodeConfigurationInput(value,{ordinary:{},secrets:{}});assert.equal(input.manifestDigest,value.manifestDigest);
});

test('Local dependency warnings remain visible through adding without installing or opening the project', async () => {
  const warning = '应用所需的运行组件尚未安装，请先在这个项目文件夹中安装依赖。';
  const review = directoryReview();
  review.inspection.dependencyWarnings = [warning];
  const calls = [];
  await setupDirectory(async command => {
    calls.push(command);
    if (command === 'choose_path') return review.sourcePath;
    if (command === 'node_app_directory_inspect') return review;
    if (command === 'node_app_directory_register') return app({ sourcePath: review.sourcePath, dependencyWarnings: [warning] });
    throw new Error(command);
  });
  await click(byText('选择文件夹…'));
  assert.match(document.querySelector('[aria-label="运行组件提示"]').textContent, /尚未安装/);
  assert.equal(byText('添加').disabled, false, 'The advisory must not create a new registration gate');
  await click(byText('添加'));
  assert.match(document.querySelector('[aria-label="运行组件提示"]').textContent, /尚未安装/);
  assert.match(document.querySelector('.import-result [role="status"]').textContent, /请先准备/);
  assert.deepEqual(calls, ['choose_path', 'node_app_directory_inspect', 'node_app_directory_register']);
});

test('A refreshed dependency warning clears after preparation while keeping unsaved configuration drafts', async () => {
  const value = app({ dependencyWarnings: ['运行组件尚未安装。'] });
  const calls = [];
  await setup(baseMock(value, async command => {
    calls.push(command);
    assert.equal(command, 'node_app_status');
    return app({ dependencyWarnings: [] });
  }));
  assert.equal(byText('打开').disabled, false, 'A dependency warning is advisory');
  await change(input('MODEL'), 'keep this model draft');
  await change(input('AI_KEY'), 'artificial-keep-dependency-warning-draft');
  await showDetails();
  assert.match(document.querySelector('[aria-label="运行组件提示"]').textContent, /尚未安装/);
  await act(async () => window.dispatchEvent(new window.Event('focus')));
  assert.equal(document.querySelector('[aria-label="运行组件提示"]'), null);
  await click(byText('修改'));
  assert.equal(input('MODEL').value, 'keep this model draft');
  assert.equal(input('AI_KEY').value, 'artificial-keep-dependency-warning-draft');
  assert.deepEqual(calls, ['node_app_status']);
});


test('Saving application configuration returns to details without opening or restarting the app', async () => {
  const calls = [];
  await setup(baseMock(app(), async (command, args) => {
    calls.push([command, args]); assert.equal(command, 'node_app_configuration_save');
    return app({ configuration: { ...app().configuration, revision: 2, values: { MODEL: 'saved-model' } } });
  }));
  await change(input('MODEL'), 'saved-model');
  await act(async () => document.querySelector('.node-configuration-form').dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true })));
  assert.equal(document.querySelector('.node-configuration-form'), null);
  assert.equal(document.querySelector('[role=tablist]'), null);
  assert.match(document.querySelector('.application-detail-information').textContent, /应用配置已填写 2 项/);
  assert.match(document.body.textContent, /已保存，下次进入时生效/);
  assert.equal(calls.length, 1);
});

test('Node details use their matched storage measurement and copy both bounded runtime log paths', async () => {
  const copied = [];
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async text => copied.push(text) } });
  const value = app({ objectLayout: { controlPath: '/disposable/control', data: { root: '/disposable/data' } } });
  await setup(baseMock(value, async command => { throw new Error(command); }), {
    request: { app: value, intent: 'details', token: 1 },
    workspaceStorageMock: () => ({ workspace: {}, nodeApps: [{ scope: { appId: 'other-app' }, totalBytes: 99999, complete: true }, { scope: { appId: value.id }, totalBytes: 4000, complete: true }], otherLocalLocations: [] }),
  });
  await showDetails();
  const rows = [...document.querySelectorAll('.application-detail-row')];
  assert.equal(rows.find(row => row.querySelector('dt').textContent === '占用空间').querySelector('dd').textContent, '4.0 KB');
  await click(byText('复制路径'));
  assert.deepEqual(copied, ['/disposable/control/logs/current.log\n/disposable/control/logs/previous.log']);
  assert.equal(document.querySelector('[role=tablist]'), null);
});


test("Windows deletion review accepts exact native paths and keeps the original project acknowledgement", async () => {
  const value = app({ sourcePath: "D:\\临时 external\\source" });
  const preview = { appId: value.id, name: value.name, ownership: "external", expectedRevision: "disposable-native-revision", platformRoot: "C:\\临时 data\\instance", sourcePath: value.sourcePath,
    deletePaths: ["C:\\临时 data\\instance", "C:\\临时 data\\owner"], preservedPaths: [value.sourcePath] };
  const calls = [];
  await setup(async (command) => { calls.push(command); assert.equal(command, "node_app_delete_preview"); return preview; }, {
    frontendPlatform: frontendPlatformFromBootstrap({ build: { target: "x86_64-pc-windows-msvc" } }), instanceId: "windows-fixture",
    request: { app: value, intent: "delete", token: 1 },
  });
  assert.doesNotMatch(deleteDialog().textContent, /删除范围不完整或项目身份不符/);
  assert.equal(deleteDialog().querySelector("input[type=checkbox]").disabled, false);
  assert.equal(byText("确认永久删除项目").disabled, true);
  assert.match(deleteDialog().querySelector(".node-delete-base").textContent, /C:\\临时 data\\/);
  assert.deepEqual([...deleteDialog().querySelectorAll(".node-delete-names code")].map(el => el.textContent), ["instance", "owner"]);
  assert.deepEqual(calls, ["node_app_delete_preview"]);
});

test("Windows native review still refuses drive-relative paths before acknowledgement or deletion", async () => {
  const value = app({ sourcePath: "D:\\disposable-source" });
  await setup(async command => { assert.equal(command, "node_app_delete_preview"); return { appId: value.id, name: value.name, ownership: "external", expectedRevision: "disposable-native-revision", platformRoot: "C:relative-data", sourcePath: value.sourcePath, deletePaths: ["C:relative-data"], preservedPaths: [value.sourcePath] }; }, {
    frontendPlatform: frontendPlatformFromBootstrap({ build: { target: "x86_64-pc-windows-msvc" } }), instanceId: "windows-fixture",
    request: { app: value, intent: "delete", token: 1 },
  });
  assert.match(deleteDialog().textContent, /删除范围不完整或项目身份不符/);
  assert.equal(byText("确认永久删除项目")?.disabled ?? true, true);
});
