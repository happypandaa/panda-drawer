// DOM interaction fixtures only; these do not establish native archive/import behavior.
import assert from 'node:assert/strict';
import { after, afterEach, test } from 'node:test';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';
import { JSDOM } from 'jsdom';

const repo = fileURLToPath(new URL('../../../', import.meta.url));
await mkdir(path.join(repo, '.test-artifacts'), { recursive: true });
const output = await mkdtemp(path.join(repo, '.test-artifacts/disposable-archive-import-dom-'));
const mockPath = path.join(output, 'tauri-mock.mjs');
await writeFile(mockPath, 'export const invoke = (...args) => globalThis.__ARCHIVE_IMPORT_INVOKE__(...args);\n');
const compiled = new Map();
async function compile(file) {
  if (compiled.has(file)) return compiled.get(file);
  const target = path.join(output, `${path.basename(file).replace(/\.tsx?$/, '')}.mjs`);
  compiled.set(file, target);
  let source = ts.transpileModule(await readFile(file, 'utf8'), { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX }, fileName: file }).outputText;
  for (const match of [...source.matchAll(/from\s+(['"])(\.[^'"]+)\1/g)]) {
    const base = path.resolve(path.dirname(file), match[2]); let dependency;
    for (const candidate of [base, `${base}.tsx`, `${base}.ts`]) {
      try { await readFile(candidate); dependency = candidate; break; } catch { /* Next extension. */ }
    }
    assert.ok(dependency, `Missing dependency ${base}`);
    source = source.replaceAll(`${match[1]}${match[2]}${match[1]}`, JSON.stringify(pathToFileURL(await compile(dependency)).href));
  }
  source = source.replaceAll(/(['"])@tauri-apps\/api\/core\1/g, JSON.stringify(pathToFileURL(mockPath).href));
  await writeFile(target, source); return target;
}
const dom = new JSDOM('<!doctype html><html><body></body></html>', { url: 'http://localhost/', pretendToBeVisual: true });
for (const name of ['window', 'document', 'Node', 'HTMLElement', 'HTMLInputElement', 'HTMLSelectElement', 'Event', 'MouseEvent', 'KeyboardEvent', 'MutationObserver']) Object.defineProperty(globalThis, name, { value: name === 'window' ? dom.window : dom.window[name], configurable: true });
Object.defineProperty(globalThis, 'navigator', { value: dom.window.navigator, configurable: true });
globalThis.getComputedStyle = dom.window.getComputedStyle.bind(dom.window);
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
dom.window.HTMLElement.prototype.scrollIntoView = function () {};
const { createElement: h, act } = await import('react');
const { createRoot } = await import('react-dom/client');
const { ImportPanel } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/ArchiveImportPanel.tsx'))).href);
let root; let nextTimer = 0;
const timeouts = new Map(); const intervals = new Map();
dom.window.setTimeout = fn => { const id = ++nextTimer; timeouts.set(id, fn); return id; };
dom.window.clearTimeout = id => timeouts.delete(id);
dom.window.setInterval = fn => { const id = ++nextTimer; intervals.set(id, fn); return id; };
dom.window.clearInterval = id => intervals.delete(id);
afterEach(async () => { if (root) await act(async () => root.unmount()); root = null; timeouts.clear(); intervals.clear(); });
after(async () => { dom.window.close(); await rm(output, { recursive: true, force: true }); });

const projectId = 'prj_disposable_import';
const result = { projectId, targetProjectId: projectId, operationOutcome: 'imported', cleanupPending: false, cleanup: { completed: true, removedPaths: ['/disposable/internal/source.zip'], failures: [] } };
const task = overrides => ({ taskId: 'task_disposable_import', projectId, type: 'import', status: 'succeeded', final: true, phase: 'completed', result, error: null, ...overrides });
const removed = { projectId, completed: true, operationOutcome: 'imported', removedPaths: ['/disposable/internal/source.zip'], failures: [] };
function deferred() { let resolve; let reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { resolve, reject, promise }; }
async function setup(overrides = {}, props = {}) {
  const calls = []; const tasks = []; const opened = []; let closed = 0; let completed = 0;
  const defaults = { project_import: () => task(), project_import_cleanup: () => removed, project_list: () => [{ id: projectId, name: 'Disposable archive' }] };
  globalThis.__ARCHIVE_IMPORT_INVOKE__ = async (command, args) => { calls.push([command, args]); const run = overrides[command] ?? defaults[command]; assert.ok(run, `Unexpected command ${command}`); return run(args); };
  document.body.innerHTML = '<button id="trigger">Import</button><div id="root"></div>';
  document.getElementById('trigger').focus(); root = createRoot(document.getElementById('root'));
  let currentProps = { disabled: false, instanceId: 'instance_one', onTask: value => tasks.push(value), onClose: () => closed++, onCompleted: () => completed++, onImported: value => opened.push(value), ...props };
  async function render(next = {}) { currentProps = { ...currentProps, ...next }; await act(async () => root.render(h(ImportPanel, currentProps))); }
  await render(); return { calls, tasks, opened, render, get closed() { return closed; }, get completed() { return completed; } };
}
const byText = text => [...document.querySelectorAll('button')].find(element => element.textContent === text);
const commands = fixture => fixture.calls.map(([name]) => name);
async function click(element) { assert.ok(element); await act(async () => element.click()); }
async function change(element, value) { assert.ok(element); await act(async () => { Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set.call(element, value); element.dispatchEvent(new window.Event('input', { bubbles: true })); }); }
async function begin() { await change(document.querySelector('.path-input input'), '/disposable/project.zip'); await click(byText('校验并导入')); }
async function tick() { await act(async () => { for (const fn of [...intervals.values()]) fn(); }); }
async function escape() { await act(async () => document.activeElement.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }))); }

test('Archive import always creates a new identity with scoped cleanup', async () => {
  const fixture = await setup();
  assert.equal(document.querySelector('[role=dialog] h2').textContent, '导入完整项目');
  assert.equal(document.querySelector('input[name=import-mode]'), null);
  assert.doesNotMatch(document.body.textContent, /保留原 ID/);
  await change(document.querySelector('form > label input'), 'Independent copy'); await begin();
  assert.deepEqual(fixture.calls[0], ['project_import', { input: { archivePath: '/disposable/project.zip', cleanupTemporaryFiles: true, name: 'Independent copy' } }]);
  assert.ok(document.body.textContent.includes('导入成功')); assert.equal(fixture.completed, 1); assert.equal(fixture.opened.length, 0);
  assert.deepEqual(commands(fixture), ['project_import']);
});
test('Repeated archive imports expose no original-identity option', async () => {
  const fixture = await setup();
  assert.equal(document.querySelector('input[value=original]'), null);
  await begin();
  assert.deepEqual(fixture.calls[0][1], { input: { archivePath: '/disposable/project.zip', cleanupTemporaryFiles: true } });
});
test('The modal tracks real cleanup before showing success and blocks closing or duplicate submissions', async () => {
  let latest = task({ final: false, status: 'running', phase: 'archive-copy', result: { targetProjectId: projectId } });
  const fixture = await setup({ project_import: () => latest, task_get: () => latest }); await begin();
  assert.equal(document.querySelector('.modal-close').disabled, true); await escape(); assert.equal(fixture.closed, 0);
  latest = task({ final: false, status: 'running', phase: 'cleanup', result: null }); await tick();
  assert.equal(document.querySelector('[aria-current=step]').textContent.includes('清理临时文件'), true);
  assert.equal(document.body.textContent.includes('导入成功'), false); assert.equal(fixture.completed, 0);
  latest = task(); await tick();
  assert.ok(document.body.textContent.includes('导入成功')); assert.equal(fixture.completed, 1);
  assert.equal(commands(fixture).filter(name => name === 'project_import').length, 1);
});
test('A cleanup failure keeps the last step incomplete and retries cleanup without another import', async () => {
  const cleaning = deferred(); const fixture = await setup({ project_import: () => task({ result: { ...result, cleanupPending: true, cleanup: { completed: false, failures: [] } } }), project_import_cleanup: () => cleaning.promise });
  await begin(); assert.ok(document.body.textContent.includes('项目已导入，临时文件待清理')); assert.equal(byText('重试导入'), undefined);
  assert.equal(document.querySelector('[aria-current=step]').textContent.includes('清理临时文件'), true);
  await click(byText('重试清理')); assert.equal(document.body.textContent.includes('导入成功'), false); assert.equal(document.querySelector('.modal-close').disabled, true);
  await act(async () => cleaning.resolve(removed));
  assert.ok(document.body.textContent.includes('导入成功')); assert.deepEqual(commands(fixture), ['project_import', 'project_import_cleanup']);
});
test('An incomplete retry remains actionable with no false success or duplicate project', async () => {
  let retries = 0; const fixture = await setup({ project_import: () => task({ result: { ...result, cleanupPending: true } }), project_import_cleanup: () => ++retries === 1 ? { ...removed, completed: false, failures: [{ path: '/disposable/tmp', error: { message: 'Artificial removal failure' } }] } : removed });
  await begin(); await click(byText('重试清理')); assert.ok(document.body.textContent.includes('Artificial removal failure')); assert.equal(document.body.textContent.includes('导入成功'), false);
  await click(byText('重试清理')); assert.ok(document.body.textContent.includes('导入成功')); assert.equal(commands(fixture).filter(name => name === 'project_import').length, 1);
});
test('A polling outage keeps the accepted task active and cannot enable a second import', async () => {
  let polls = 0; const fixture = await setup({ project_import: () => task({ status: 'running', final: false, phase: 'archive-validate', result: null }), task_get: () => ++polls === 1 ? Promise.reject(new Error('Artificial read outage')) : task() });
  await begin(); await tick(); assert.ok(document.body.textContent.includes('Artificial read outage')); assert.equal(document.querySelector('.modal-close').disabled, true); assert.equal(byText('重试导入'), undefined);
  await tick(); assert.ok(document.body.textContent.includes('导入成功')); assert.equal(commands(fixture).filter(name => name === 'project_import').length, 1);
});
test('An unknown outcome exposes verification and never cleanup or reimport', async () => {
  const fixture = await setup({ project_import: () => task({ status: 'failed', result: null, error: { code: 'RECOVERY_REQUIRED', message: 'Uncertain install', details: { operationOutcome: 'needs_attention' } } }), task_get: () => task() });
  await begin(); assert.ok(byText('核对导入结果')); assert.equal(byText('重试导入'), undefined); assert.equal(byText('清理并关闭'), undefined);
  await click(byText('核对导入结果')); assert.ok(document.body.textContent.includes('导入成功')); assert.equal(commands(fixture).filter(name => name === 'project_import').length, 1);
});
test('Definite failure cleans only its task before closing', async () => {
  const removal = deferred(); const fixture = await setup({ project_import: () => task({ status: 'failed', result: null, error: { code: 'IMPORT_FAILED', message: 'Artificial invalid archive', details: { operationOutcome: 'not_registered' } } }), project_import_cleanup: () => removal.promise });
  await begin(); await click(byText('清理并关闭')); assert.equal(fixture.closed, 0);
  assert.deepEqual(fixture.calls.at(-1), ['project_import_cleanup', { taskId: 'task_disposable_import' }]);
  await act(async () => removal.resolve({ ...removed, operationOutcome: 'not_registered' })); assert.equal(fixture.closed, 1);
});
test('A late source task response cannot publish success into a new platform instance', async () => {
  const accepted = deferred(); const fixture = await setup({ project_import: () => accepted.promise }); await begin();
  await fixture.render({ instanceId: 'instance_two' }); await act(async () => accepted.resolve(task()));
  assert.ok(byText('校验并导入')); assert.equal(fixture.completed, 0); assert.equal(fixture.tasks.length, 0);
});
test('The selected project opens only after explicit action with a fresh project lookup', async () => {
  const fixture = await setup(); await begin(); assert.equal(fixture.opened.length, 0);
  await click(byText('查看项目')); assert.equal(fixture.closed, 1); assert.equal(fixture.opened[0].id, projectId);
  assert.deepEqual(commands(fixture), ['project_import', 'project_list']);
});
test('A switched task identity cannot announce success', async () => {
  const fixture = await setup({ project_import: () => task({ final: false, status: 'running', phase: 'archive-validate' }), task_get: () => task({ taskId: 'task_other' }) });
  await begin(); await tick(); assert.ok(document.body.textContent.includes('导入结果需要核对')); assert.equal(fixture.completed, 0); assert.equal(byText('重试导入'), undefined);
});
test('If cleanup discovers the task already imported, a retry resolves that result instead of importing again', async () => {
  const fixture = await setup({ project_import: () => task({ status: 'failed', result: null, error: { code: 'IMPORT_FAILED', message: 'Artificial initially unregistered', details: { operationOutcome: 'not_registered' } } }) });
  await begin(); await click(byText('重试导入')); assert.ok(document.body.textContent.includes('导入成功'));
  assert.equal(fixture.completed, 1); assert.deepEqual(commands(fixture), ['project_import', 'project_import_cleanup']);
});

test('Cleaning after a failed validation does not mark the unfinished installation stage complete', async () => {
  const cleanup = deferred(); let latest = task({ final: false, status: 'running', phase: 'archive-validate', result: null });
  const fixture = await setup({ project_import: () => latest, task_get: () => latest, project_import_cleanup: () => cleanup.promise });
  await begin(); latest = task({ status: 'failed', phase: 'failed', result: null, error: { code: 'IMPORT_FAILED', message: 'Invalid archive', details: { operationOutcome: 'not_registered' } } }); await tick();
  await click(byText('清理并关闭'));
  const stages = [...document.querySelectorAll('.import-progress li')];
  assert.equal(stages[0].classList.contains('is-complete'), true);
  assert.equal(stages[1].classList.contains('is-complete'), false);
  assert.equal(stages[2].classList.contains('is-complete'), false);
  await act(async () => cleanup.resolve({ ...removed, operationOutcome: 'not_registered' })); assert.equal(fixture.closed, 1);
});
test('A committed material recovery error cannot be disguised as a cleanup-only warning', async () => {
  const fixture = await setup({ project_import: () => task({ status: 'failed', result: null, error: { code: 'LIFECYCLE_RECOVERY_REQUIRED', message: 'Material journal needs review', details: { operationOutcome: 'imported', lifecycleResult: result } } }) });
  await begin(); assert.ok(document.body.textContent.includes('导入结果需要核对'));
  assert.equal(byText('重试清理'), undefined); assert.equal(byText('重试导入'), undefined); assert.equal(fixture.completed, 1);
});
