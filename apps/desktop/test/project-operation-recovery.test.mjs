// Disposable DOM fixtures; no user project or native recovery operation is accessed.
import assert from 'node:assert/strict';
import { after, afterEach, test } from 'node:test';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';
import { JSDOM } from 'jsdom';

const repo = fileURLToPath(new URL('../../../', import.meta.url));
await mkdir(path.join(repo, '.test-artifacts'), { recursive: true });
const output = await mkdtemp(path.join(repo, '.test-artifacts/disposable-operation-recovery-dom-'));
const mockPath = path.join(output, 'tauri-mock.mjs');
await writeFile(mockPath, 'export const invoke = (...args) => globalThis.__OPERATION_RECOVERY_INVOKE__(...args);\n');
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
      try { await readFile(candidate); dependency = candidate; break; } catch { /* Try the next extension. */ }
    }
    assert.ok(dependency);
    source = source.replaceAll(`${match[1]}${match[2]}${match[1]}`, JSON.stringify(pathToFileURL(await compile(dependency)).href));
  }
  source = source.replaceAll(/(['"])@tauri-apps\/api\/core\1/g, JSON.stringify(pathToFileURL(mockPath).href));
  await writeFile(target, source);
  return target;
}
const dom = new JSDOM('<!doctype html><html><body></body></html>', { url: 'http://localhost/', pretendToBeVisual: true });
for (const name of ['window', 'document', 'Node', 'HTMLElement', 'HTMLInputElement', 'Event', 'MouseEvent']) Object.defineProperty(globalThis, name, { value: name === 'window' ? dom.window : dom.window[name], configurable: true });
Object.defineProperty(globalThis, 'navigator', { value: dom.window.navigator, configurable: true });
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
const { createElement: h, act, StrictMode } = await import('react');
const { createRoot } = await import('react-dom/client');
const { ProjectOperationRecovery } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/ProjectOperationRecovery.tsx'))).href);
const timers = new Map(); let timerId = 0; let root;
dom.window.setInterval = fn => { const id = ++timerId; timers.set(id, fn); return id; };
dom.window.clearInterval = id => timers.delete(id);
async function tick() { await act(async () => { for (const fn of [...timers.values()]) fn(); }); }
async function unmount() { if (root) await act(async () => root.unmount()); root = null; }
afterEach(async () => { await unmount(); timers.clear(); });
after(async () => { dom.window.close(); await rm(output, { recursive: true, force: true }); });
function deferred() { let resolve; let reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { resolve, reject, promise }; }
const status = (overrides = {}) => ({ projectId: 'prj_a', mode: 'ready', maintenanceId: null, reason: null, startedAt: null, readyForChanges: false, recoveryRequired: false, documentEpoch: 'disposable_epoch', activeTask: null, ...overrides });
const operation = (overrides = {}) => ({ restoreTaskId: 'restore_original', targetCommit: 'a'.repeat(40), phase: 'applying', planDigest: 'd'.repeat(64), journalDigest: 'e'.repeat(64), canContinue: true, canUndo: true, changes: [{ path: 'app/index.ts', kind: 'modify', state: 'after' }], issues: [], ...overrides });
const recovery = (overrides = {}) => ({ state: 'clear', operations: [], issues: [], database: { state: 'not-supported' }, ...overrides });
const pendingRecovery = (overrides = {}) => recovery({ state: 'pending', operations: [operation()], ...overrides });
const task = (overrides = {}) => ({ taskId: 'task_recovery', projectId: 'prj_a', type: 'gitSourceRestoreContinue', status: 'running', final: false, phase: 'source-restore-apply', result: null, error: null, ...overrides });
async function setup({ overrides = {}, props = {}, strict = false } = {}) {
  const calls = []; const tasks = []; const pending = []; let changed = 0;
  globalThis.__OPERATION_RECOVERY_INVOKE__ = async (command, args) => {
    calls.push([command, args]);
    if (overrides[command]) return overrides[command](args);
    if (command === 'project_git_source_restore_status') return recovery();
    if (command === 'task_get') return task({ taskId: args.taskId, type: 'gitSourceRestoreApply', final: true, status: 'interrupted' });
    throw new Error(`Unexpected recovery fixture command: ${command}`);
  };
  let currentProps = { projectId: 'prj_a', instanceId: 'instance_a', status: status(), onTask: value => tasks.push(value), onPendingChange: value => pending.push(value), onChanged: async () => { changed++; }, ...props };
  document.body.innerHTML = '<div id="root"></div>'; root = createRoot(document.getElementById('root'));
  async function render(next = {}) { currentProps = { ...currentProps, ...next }; await act(async () => root.render(strict ? h(StrictMode, null, h(ProjectOperationRecovery, currentProps)) : h(ProjectOperationRecovery, currentProps))); }
  await render();
  return { calls, tasks, pending, render, changed: () => changed };
}
const byText = text => [...document.querySelectorAll('button')].find(button => button.textContent === text);
const commandCalls = (calls, command) => calls.filter(([name]) => name === command);
async function click(element) { assert.ok(element); await act(async () => element.click()); }
async function open() { await click(byText('查看并处理')); }
const stopped = () => document.querySelector('input[type="checkbox"]');

test('A normal application performs only the read-only recovery check and has no old maintenance or Git panel', async () => {
  const { calls, pending } = await setup();
  assert.deepEqual(calls, [['project_git_source_restore_status', { id: 'prj_a' }]]);
  assert.equal(document.querySelector('.operation-recovery'), null);
  assert.equal(pending.at(-1), false);
});

test('An unknown state stays visible, requires an explicit recheck and never starts recovery', async () => {
  let reads = 0;
  const { calls } = await setup({ overrides: { project_git_source_restore_status: () => { if (++reads < 3) throw new Error('记录暂不可读'); return recovery(); } } });
  assert.match(document.body.textContent, /操作状态暂未核实/);
  await open(); assert.match(document.body.textContent, /记录暂不可读/);
  await tick(); assert.equal(reads, 2);
  await click(byText('重新核对操作状态'));
  assert.equal(document.querySelector('.operation-recovery'), null);
  assert.equal(commandCalls(calls, 'project_git_source_restore_recover').length, 0);
});

test('A retired maintenance state offers no end action, and recoveryRequired shows the recovery note', async () => {
  const { calls } = await setup({ props: { status: status({ mode: 'maintenance', maintenanceId: 'maintenance_a' }) } });
  await open();
  assert.equal(byText('检查并结束维护'), undefined);
  assert.match(document.body.textContent, /已停用的维护状态/);
  assert.equal(commandCalls(calls, 'maintenance_end').length, 0);
  await unmount();
  await setup({ props: { status: status({ mode: 'maintenance', maintenanceId: 'maintenance_a', recoveryRequired: true }) } });
  await open();
  assert.doesNotMatch(document.body.textContent, /已停用的维护状态/);
  assert.match(document.body.textContent, /有尚未核对完成的恢复操作/);
});

test('Recovery actions use the rechecked digests and require external editing to stop plus action confirmation', async () => {
  let reads = 0; const accepted = deferred();
  const { calls, tasks, pending } = await setup({ overrides: {
    project_git_source_restore_status: () => pendingRecovery({ operations: [operation({ journalDigest: (++reads === 1 ? 'e' : 'f').repeat(64), canUndo: false })] }),
    project_git_source_restore_recover: () => accepted.promise,
  } });
  await open();
  assert.equal(byText('继续这次取回').disabled, true); assert.equal(byText('撤回这次取回'), undefined);
  await click(stopped()); await click(byText('继续这次取回'));
  assert.equal(commandCalls(calls, 'project_git_source_restore_recover').length, 0);
  const confirm = byText('确认继续取回');
  await act(async () => { confirm.click(); confirm.click(); });
  assert.deepEqual(commandCalls(calls, 'project_git_source_restore_recover'), [['project_git_source_restore_recover', { id: 'prj_a', input: { restoreTaskId: 'restore_original', expectedPlanDigest: 'd'.repeat(64), expectedJournalDigest: 'f'.repeat(64), action: 'continue' } }]]);
  assert.equal(pending.at(-1), true);
  await act(async () => accepted.resolve(task()));
  assert.equal(tasks.length, 1); assert.equal(pending.at(-1), true, 'An accepted running task keeps details guarded');
  assert.match(document.body.textContent, /等待最终结果后再关闭详情/);
});

test('Failed recovery preserves evidence, expires confirmation and requires an explicit fresh recheck without retry', async () => {
  const { calls, pending } = await setup({ overrides: { project_git_source_restore_status: () => pendingRecovery(), project_git_source_restore_recover: () => { throw { code: 'SOURCE_RESTORE_CHANGED', message: '现场已变化' }; } } });
  await open(); await click(stopped()); await click(byText('继续这次取回')); await click(byText('确认继续取回'));
  assert.match(document.body.textContent, /现场已变化/); assert.equal(byText('继续这次取回').disabled, true); assert.equal(pending.at(-1), false);
  await tick(); assert.equal(commandCalls(calls, 'project_git_source_restore_recover').length, 1);
  await click(byText('重新核对操作状态'));
  assert.equal(stopped().checked, false); await click(stopped()); await click(byText('撤回这次取回'));
  assert.equal(byText('确认撤回这次取回').disabled, false);
});

test('Blocked recovery displays the unresolved file and offers no unverified action', async () => {
  await setup({ overrides: { project_git_source_restore_status: () => pendingRecovery({ state: 'blocked', operations: [operation({ canContinue: false, canUndo: false, changes: [{ path: 'app/index.ts', kind: 'modify', state: 'unknown' }], issues: [{ message: '外部修改需人工核对' }] })] }) } });
  await open(); assert.match(document.body.textContent, /内容尚不能确认/); assert.match(document.body.textContent, /外部修改需人工核对/);
  assert.equal(byText('继续这次取回'), undefined); assert.equal(byText('撤回这次取回'), undefined);
});

for (const mismatch of [{ projectId: 'prj_other' }, { type: 'gitSave' }]) {
  test(`A task with mismatched ${Object.keys(mismatch)[0]} is rejected and never polled as an accepted recovery`, async () => {
    const { calls, tasks } = await setup({ overrides: { project_git_source_restore_status: () => pendingRecovery(), project_git_source_restore_recover: () => task(mismatch) } });
    await open(); await click(stopped()); await click(byText('继续这次取回')); await click(byText('确认继续取回'));
    assert.equal(tasks.length, 0); assert.match(document.body.textContent, /处理任务身份不符/);
    const readCount = commandCalls(calls, 'task_get').length; await tick(); assert.equal(commandCalls(calls, 'task_get').length, readCount);
  });
}

test('A final task releases the guard only after refreshing both application and recovery state', async () => {
  let finished = false; let reads = 0;
  const { calls, pending, changed } = await setup({ props: { task: task() }, overrides: {
    project_git_source_restore_status: () => { reads++; return recovery(); },
    task_get: () => finished ? task({ final: true, status: 'succeeded', result: { operationOutcome: 'committed', state: 'completed' } }) : task(),
  } });
  assert.equal(reads, 0); assert.equal(pending.at(-1), true);
  finished = true; await tick();
  assert.equal(changed(), 1); assert.equal(reads, 1); assert.equal(pending.at(-1), false);
  assert.equal(document.querySelector('.operation-recovery'), null);
  assert.equal(commandCalls(calls, 'project_git_source_restore_recover').length, 0);
});

test('Task logs remain accessible for an unresolved operation without exposing creation or full version controls', async () => {
  const { calls } = await setup({ overrides: { project_git_source_restore_status: () => pendingRecovery(), task_logs: ({ taskId }) => ({ taskId, cursor: 0, nextCursor: 6, text: '记录保留', truncated: false, gaps: [], eof: true }) } });
  await open(); await click(byText('查看任务日志'));
  assert.deepEqual(commandCalls(calls, 'task_logs'), [['task_logs', { taskId: 'restore_original', cursor: 0 }]]);
  assert.match(document.body.textContent, /记录保留/); assert.doesNotMatch(document.body.textContent, /创建备份|准备基线|保存源码版本|要取回的本机提交/);
});

test('Late reads and submissions cannot publish into another project, instance or unmounted detail', async () => {
  const oldRead = deferred(); let reads = 0;
  const first = await setup({ overrides: { project_git_source_restore_status: () => ++reads === 1 ? oldRead.promise : recovery() } });
  await first.render({ projectId: 'prj_b', status: status({ projectId: 'prj_b' }) });
  await act(async () => oldRead.resolve(pendingRecovery()));
  assert.equal(document.querySelector('.operation-recovery'), null); assert.equal(first.pending.at(-1), false);
  await unmount();
  const accepted = deferred();
  const second = await setup({ overrides: { project_git_source_restore_status: () => pendingRecovery(), project_git_source_restore_recover: () => accepted.promise } });
  await open(); await click(stopped()); await click(byText('继续这次取回')); await click(byText('确认继续取回'));
  await second.render({ instanceId: 'instance_b' });
  await act(async () => accepted.resolve(task()));
  assert.equal(second.tasks.length, 0); assert.equal(second.pending.at(-1), false);
  const delayed = deferred();
  await unmount(); const third = await setup({ overrides: { project_git_source_restore_status: () => delayed.promise } });
  await unmount(); await act(async () => delayed.resolve(pendingRecovery()));
  assert.equal(third.pending.at(-1), false); assert.equal(third.tasks.length, 0);
});

test('Strict Mode effect replay leaves a usable current recovery check', async () => {
  await setup({ strict: true, overrides: { project_git_source_restore_status: () => pendingRecovery() } });
  await open(); assert.equal(byText('重新核对操作状态').disabled, false); assert.match(document.body.textContent, /未完成的源码取回/);
});
