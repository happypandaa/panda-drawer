// Disposable DOM fixtures and mocked native commands only. These checks do not
// exercise an OS save dialog, a real export archive, or a development project.
import assert from 'node:assert/strict';
import { after, afterEach, test } from 'node:test';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';
import { JSDOM } from 'jsdom';

const repo = fileURLToPath(new URL('../../../', import.meta.url));
await mkdir(path.join(repo, '.test-artifacts'), { recursive: true });
const output = await mkdtemp(path.join(repo, '.test-artifacts/disposable-project-export-dom-'));
const mockPath = path.join(output, 'tauri-mock.mjs');
await writeFile(mockPath, 'export const invoke = (...args) => globalThis.__PROJECT_EXPORT_INVOKE__(...args);\n');
const compiled = new Map();
async function compile(file) {
  if (compiled.has(file)) return compiled.get(file);
  const target = path.join(output, `${path.basename(file).replace(/\.tsx?$/, '')}.mjs`);
  compiled.set(file, target);
  let source = ts.transpileModule(await readFile(file, 'utf8'), { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX }, fileName: file }).outputText;
  for (const match of [...source.matchAll(/from\s+(['"])(\.[^'"]+)\1/g)]) {
    const base = path.resolve(path.dirname(file), match[2]);
    let dependency;
    for (const candidate of [base, `${base}.tsx`, `${base}.ts`]) {
      try { await readFile(candidate); dependency = candidate; break; } catch { /* Next extension. */ }
    }
    assert.ok(dependency, `Cannot resolve ${match[2]}`);
    source = source.replaceAll(`${match[1]}${match[2]}${match[1]}`, JSON.stringify(pathToFileURL(await compile(dependency)).href));
  }
  source = source.replaceAll(/(['"])@tauri-apps\/api\/core\1/g, JSON.stringify(pathToFileURL(mockPath).href));
  await writeFile(target, source);
  return target;
}
const dom = new JSDOM('<!doctype html><html><body></body></html>', { url: 'http://localhost/', pretendToBeVisual: true });
for (const name of ['window', 'document', 'Node', 'HTMLElement', 'HTMLInputElement', 'Event', 'MouseEvent', 'KeyboardEvent', 'FocusEvent', 'MutationObserver']) Object.defineProperty(globalThis, name, { value: name === 'window' ? dom.window : dom.window[name], configurable: true });
Object.defineProperty(globalThis, 'navigator', { value: dom.window.navigator, configurable: true });
globalThis.getComputedStyle = dom.window.getComputedStyle.bind(dom.window);
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
const { createElement: h, act, StrictMode } = await import('react');
const { createRoot } = await import('react-dom/client');
const { ProjectLifecyclePanel } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/LifecyclePanels.tsx'))).href);
const { Modal } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/Modal.tsx'))).href);
let root;
afterEach(async () => { if (root) await act(async () => root.unmount()); root = null; });
after(async () => { dom.window.close(); await rm(output, { recursive: true, force: true }); });

const project = { id: 'prj_disposable_export', name: 'Disposable app', projectRoot: '/disposable/export-project', documentEpoch: '11111111-1111-4111-8111-111111111111' };
const ready = { mode: 'ready', maintenanceId: null, activeTask: null, recoveryRequired: false };
const task = { taskId: 'task_export_disposable', type: 'export', projectId: project.id, status: 'queued', final: false, phase: 'queued', result: null, error: null };
const deferred = () => { let resolve; let reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const button = (text, parent = document) => [...parent.querySelectorAll('button')].find(element => element.textContent === text);
const exportDialog = () => document.querySelector('.project-export-modal');
async function click(element) { assert.ok(element); await act(async () => element.click()); }
async function key(value, shiftKey = false) { await act(async () => document.activeElement.dispatchEvent(new window.KeyboardEvent('keydown', { key: value, shiftKey, bubbles: true, cancelable: true }))); }
async function setup({ commands = {}, props = {}, strict = false, parentModal = false } = {}) {
  const calls = []; const tasks = []; const drafts = []; const parentCloses = [];
  globalThis.__PROJECT_EXPORT_INVOKE__ = async (command, args) => {
    if (command === 'project_delete_preview' && !commands[command]) return {
      projectId: current.project.id, instanceId: current.instanceId,
      projectRoot: current.project.projectRoot,
      expectedDocumentEpoch: current.project.documentEpoch,
      mode: current.status.mode, maintenanceId: current.status.maintenanceId,
      deletePaths: [current.project.projectRoot], alreadyAbsent: false, cleanupOnly: false,
    };
    calls.push([command, args]);
    if (commands[command]) return commands[command](args);
    throw new Error(`Unexpected disposable command: ${command}`);
  };
  document.body.innerHTML = '<div id="root"></div>';
  root = createRoot(document.getElementById('root'));
  let current = { project, status: ready, disabled: false, active: true, instanceId: 'instance_export_a', onTask: value => tasks.push(value), onDraftStateChange: value => drafts.push(value), ...props };
  async function render(next = {}) {
    current = { ...current, ...next };
    let element = h(ProjectLifecyclePanel, current);
    if (parentModal) element = h(Modal, { title: '项目详情', onClose: () => parentCloses.push(true) }, element);
    if (strict) element = h(StrictMode, null, element);
    await act(async () => root.render(element));
  }
  await render();
  return { calls, tasks, drafts, render, parentCloses };
}

test('More actions contains two concise rows and only explains export after opening it', async () => {
  const { calls, drafts } = await setup();
  assert.equal(document.querySelectorAll('.project-more-row').length, 2);
  assert.equal(document.querySelectorAll('form').length, 0);
  assert.doesNotMatch(document.body.textContent, /创建独立副本|副本名称|允许替换/);
  const trigger = button('导出 ZIP'); trigger.focus(); await click(trigger);
  assert.deepEqual(calls, []);
  assert.match(exportDialog().textContent, /源码与项目资料.*当前应用文件.*数据库和用户文件/);
  assert.match(exportDialog().textContent, /不包含 Git 历史、开发依赖、缓存和平台历史备份/);
  assert.match(exportDialog().textContent, /包含当前业务数据/);
  assert.match(exportDialog().textContent, /导出的 ZIP 保存在你选择的位置，删除项目时会保留/);
  assert.doesNotMatch(exportDialog().textContent, /属于本项目|归原项目独占|一并删除/);
  assert.equal(exportDialog().getAttribute('aria-modal'), 'true');
  assert.equal(document.activeElement.tagName, 'H2');
  assert.equal(drafts.at(-1).pending, true);
  await click(button('取消', exportDialog()));
  assert.equal(exportDialog(), null); assert.equal(document.activeElement === trigger, true);
  assert.equal(drafts.at(-1).pending, false);
});

test('Cancelling the native save chooser never submits an export and restores the dialog controls', async () => {
  const { calls, drafts } = await setup({ commands: { choose_path: () => null } });
  await click(button('导出 ZIP')); await click(button('选择位置并导出'));
  assert.deepEqual(calls, [['choose_path', { kind: 'export', suggestedName: `${project.id}.zip` }]]);
  assert.ok(exportDialog()); assert.equal(button('选择位置并导出').disabled, false);
  await key('Escape'); assert.equal(exportDialog(), null); assert.equal(drafts.at(-1).pending, false);
});

test('Chooser and submission lock duplicate actions and closing until the real task is accepted', async () => {
  const choosing = deferred(); const accepting = deferred();
  const { calls, tasks } = await setup({ strict: true, commands: { choose_path: () => choosing.promise, project_export: () => accepting.promise } });
  await click(button('导出 ZIP')); await click(button('选择位置并导出')); await click(button('选择中…')); await key('Escape');
  assert.ok(exportDialog()); assert.equal(button('取消').disabled, true); assert.equal(calls.length, 1);
  await act(async () => choosing.resolve('/disposable/reviewed-export.zip'));
  assert.deepEqual(calls.at(-1), ['project_export', { projectId: project.id, input: { destinationPath: '/disposable/reviewed-export.zip', overwrite: false } }]);
  await click(button('正在提交…')); assert.equal(calls.length, 2); assert.deepEqual(tasks, []);
  await act(async () => accepting.resolve(task));
  assert.deepEqual(tasks, [task]); assert.equal(exportDialog(), null);
  assert.doesNotMatch(document.body.textContent, /导出成功/);
});

test('A destination collision requires a new filename and never offers overwrite', async () => {
  let exports = 0;
  const { calls, tasks } = await setup({ commands: {
    choose_path: () => exports ? '/disposable/new-exclusive-export.zip' : '/disposable/already-exists.zip',
    project_export: ({ input }) => {
      assert.equal(input.overwrite, false);
      if (++exports === 1) throw { code: 'EXPORT_DESTINATION_EXISTS', message: '请选择新文件名' };
      return task;
    },
  } });
  await click(button('导出 ZIP'));
  assert.equal(document.querySelector('[aria-label="替换已有 ZIP 确认"]'), null);
  await click(button('选择位置并导出'));
  assert.equal(tasks.length, 0); assert.equal(exports, 1);
  const confirmation = document.querySelector('[aria-label="导出位置已占用"]');
  assert.match(confirmation.textContent, /\/disposable\/already-exists.zip/);
  assert.match(confirmation.textContent, /不会覆盖已有文件，请另选文件名或位置/);
  assert.doesNotMatch(confirmation.textContent, /归原项目独占|随.*删除/);
  assert.equal(button('确认替换并导出'), undefined);
  await click(button('选择其他位置'));
  assert.equal(exports, 2); assert.equal(exportDialog(), null);
  assert.deepEqual(calls.filter(([name]) => name === 'project_export').map(([, args]) => args.input.overwrite), [false, false]);
});

test('Chooser errors, request errors and failed tasks stay visible and never pretend the archive succeeded', async () => {
  let choosing = 0; let exporting = 0;
  const failed = { ...task, status: 'failed', final: true, error: { code: 'DATABASE_SNAPSHOT_FAILED', message: 'Disposable database snapshot failed' } };
  const { tasks } = await setup({ commands: {
    choose_path: () => { if (++choosing === 1) throw new Error('Disposable chooser failed'); return '/disposable/error.zip'; },
    project_export: () => { if (++exporting === 1) throw new Error('Disposable request failed'); return failed; },
  } });
  await click(button('导出 ZIP')); await click(button('选择位置并导出'));
  assert.match(exportDialog().querySelector('[role="alert"]').textContent, /Disposable chooser failed/);
  await click(button('选择位置并导出'));
  assert.match(exportDialog().querySelector('[role="alert"]').textContent, /Disposable request failed/);
  assert.equal(document.querySelector('[aria-label="替换已有 ZIP 确认"]'), null);
  await click(button('选择位置并导出'));
  assert.deepEqual(tasks, [failed]); assert.ok(exportDialog());
  assert.match(exportDialog().querySelector('[role="alert"]').textContent, /Disposable database snapshot failed/);
  assert.equal(button('选择位置并导出').disabled, false);
});

for (const mismatch of [{ projectId: 'prj_other' }, { type: 'delete' }]) test(`An export result with a mismatched ${Object.keys(mismatch)[0]} never reaches the current application`, async () => {
  const { tasks } = await setup({ commands: { choose_path: () => '/disposable/mismatch.zip', project_export: () => ({ ...task, ...mismatch }) } });
  await click(button('导出 ZIP')); await click(button('选择位置并导出'));
  assert.deepEqual(tasks, []); assert.ok(exportDialog());
  assert.match(exportDialog().querySelector('[role="alert"]').textContent, /导出任务身份不符/);
});

test('Status gates and the current maintenance identity also apply to export', async () => {
  const { calls, render } = await setup({ commands: { choose_path: () => '/disposable/maintenance.zip', project_export: () => task } });
  for (const state of [{ disabled: true }, { disabled: false, status: undefined }, { status: { ...ready, activeTask: task } }, { status: { ...ready, recoveryRequired: true } }, { active: false, status: ready }]) {
    await render(state); assert.equal(button('导出 ZIP').disabled, true); await click(button('导出 ZIP')); assert.equal(exportDialog(), null);
  }
  assert.deepEqual(calls, []);
  await render({ active: true, status: { ...ready, mode: 'maintenance', maintenanceId: 'maintenance_disposable' } });
  await click(button('导出 ZIP')); await click(button('选择位置并导出'));
  assert.equal(calls.at(-1)[1].input.maintenanceId, 'maintenance_disposable');
});

for (const delayed of ['choose_path', 'project_export']) test(`A late ${delayed} response cannot affect another detail instance`, async () => {
  const waiting = deferred();
  const { calls, tasks, render } = await setup({ commands: { choose_path: () => delayed === 'choose_path' ? waiting.promise : '/disposable/late.zip', project_export: () => waiting.promise } });
  await click(button('导出 ZIP')); await click(button('选择位置并导出'));
  await render({ instanceId: 'instance_export_b' });
  assert.equal(exportDialog(), null);
  await act(async () => waiting.resolve(delayed === 'choose_path' ? '/disposable/late.zip' : task));
  assert.deepEqual(tasks, []);
  assert.equal(calls.filter(([name]) => name === 'project_export').length, delayed === 'choose_path' ? 0 : 1);
  await click(button('导出 ZIP')); assert.ok(exportDialog()); assert.equal(button('选择位置并导出').disabled, false);
});

test('The nested export dialog owns focus and Escape while preserving the application detail', async () => {
  const { parentCloses } = await setup({ parentModal: true });
  const trigger = button('导出 ZIP'); trigger.focus(); await click(trigger);
  const dialog = exportDialog();
  assert.equal(document.querySelectorAll('[role="dialog"]').length, 2);
  assert.ok(dialog.contains(document.activeElement));
  await key('Tab'); assert.ok(dialog.contains(document.activeElement));
  await key('Escape');
  assert.equal(exportDialog(), null); assert.equal(document.querySelectorAll('[role="dialog"]').length, 1);
  assert.equal(document.activeElement === trigger, true); assert.deepEqual(parentCloses, []);
});

test('Failed permanent deletion stays in its own dialog and requires renewed acknowledgement before retrying', async () => {
  const failed = { ...task, type: 'delete', status: 'failed', final: true, error: { code: 'DELETE_FAILED', message: 'Disposable delete was rejected' } };
  const { calls, tasks } = await setup({ commands: { project_delete: () => failed } });
  await click(button('删除应用')); await click(document.querySelector('.project-delete-confirmation input')); await click(button('确认永久删除项目'));
  assert.deepEqual(calls, [['project_delete', { projectId: project.id, input: { expectedDocumentEpoch: project.documentEpoch } }]]);
  assert.deepEqual(tasks, [failed]);
  assert.ok(document.querySelector('.project-delete-confirmation'));
  assert.equal(document.querySelectorAll('[role="dialog"]').length, 1);
  assert.ok(document.querySelector('[role="dialog"]').textContent.includes(project.projectRoot));
  assert.match(document.querySelector('[role="alert"]').textContent, /Disposable delete was rejected/);
  assert.equal(document.querySelector('.project-delete-confirmation input').checked, false);
  assert.equal(button('确认永久删除项目').disabled, true);
  await click(button('取消，保留项目'));
  assert.equal(document.querySelector('[role="dialog"]'), null);
  await click(button('删除应用'));
  assert.equal(document.querySelector('.project-delete-confirmation input').checked, false);
  assert.equal(button('确认永久删除项目').disabled, true);
});

test('An accepted nested deletion survives name and maintenance refreshes without unlocking either dialog', async () => {
  const accepted = { ...task, type: 'delete' };
  const { calls, tasks, drafts, parentCloses, render } = await setup({ parentModal: true, commands: { project_delete: () => accepted } });
  await click(button('删除应用')); await click(document.querySelector('.project-delete-confirmation input')); await click(button('确认永久删除项目'));
  for (const change of [
    { project: { ...project, name: 'Refreshed nested deletion name' } },
    { status: { ...ready, mode: 'maintenance', maintenanceId: 'maintenance_nested_first' } },
    { status: { ...ready, mode: 'maintenance', maintenanceId: 'maintenance_nested_second' } },
    { status: ready },
  ]) {
    await render({ ...change, task: accepted });
    const nested = document.querySelector('.project-delete-modal');
    assert.ok(nested); assert.equal(document.querySelectorAll('[role="dialog"]').length, 2);
    const close = nested.querySelector('button[aria-label^="关闭"]');
    assert.equal(close.disabled, true); assert.equal(button('取消，保留项目', nested).disabled, true);
    await key('Escape'); await click(nested.parentElement); await click(close); await click(button('取消，保留项目', nested));
    assert.ok(document.querySelector('.project-delete-modal'));
    assert.equal(drafts.at(-1).pending, true); assert.deepEqual(parentCloses, []);
    assert.deepEqual(tasks, [accepted]); assert.equal(calls.length, 1);
  }
  await render({ task: { ...accepted, status: 'failed', final: true, phase: 'failed', result: { operationOutcome: 'not_deleted' } } });
  await click(button('取消，保留项目'));
  assert.equal(document.querySelector('.project-delete-modal'), null);
  assert.equal(document.querySelectorAll('[role="dialog"]').length, 1);
  assert.equal(drafts.at(-1).pending, false); assert.deepEqual(parentCloses, []);
});
