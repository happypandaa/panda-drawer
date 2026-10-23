// Synthetic UI/IPC fixtures only. No source scripts, dependencies, archives,
// native tools or existing project files are read or executed by these tests.
import assert from 'node:assert/strict';
import { after, afterEach, test } from 'node:test';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';
import { JSDOM } from 'jsdom';

const repo = fileURLToPath(new URL('../../../', import.meta.url));
await mkdir(path.join(repo, '.test-artifacts'), { recursive: true });
const output = await mkdtemp(path.join(repo, '.test-artifacts/source-build-dom-'));
const mockPath = path.join(output, 'tauri-mock.mjs');
await writeFile(mockPath, 'export const invoke = (...args) => globalThis.__SOURCE_BUILD_INVOKE__(...args);\n');
const compiled = new Map();
async function compile(file) {
  if (compiled.has(file)) return compiled.get(file);
  const target = path.join(output, `${path.basename(file).replace(/\.tsx?$/, '')}.mjs`); compiled.set(file, target);
  let source = ts.transpileModule(await readFile(file, 'utf8'), { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX }, fileName: file }).outputText;
  for (const match of [...source.matchAll(/from\s+(['"])(\.[^'"]+)\1/g)]) {
    const base = path.resolve(path.dirname(file), match[2]); let dependency;
    for (const candidate of [base, `${base}.tsx`, `${base}.ts`]) { try { await readFile(candidate); dependency = candidate; break; } catch { /* next extension */ } }
    assert.ok(dependency); source = source.replaceAll(`${match[1]}${match[2]}${match[1]}`, JSON.stringify(pathToFileURL(await compile(dependency)).href));
  }
  source = source.replaceAll(/(['"])@tauri-apps\/api\/core\1/g, JSON.stringify(pathToFileURL(mockPath).href));
  await writeFile(target, source); return target;
}
const dom = new JSDOM('<!doctype html><html><body></body></html>', { url: 'http://tauri.localhost/windows.html', pretendToBeVisual: true });
for (const name of ['window', 'document', 'Node', 'HTMLElement', 'HTMLInputElement', 'HTMLTextAreaElement', 'HTMLSelectElement', 'Event', 'MouseEvent', 'KeyboardEvent', 'MutationObserver']) Object.defineProperty(globalThis, name, { value: name === 'window' ? dom.window : dom.window[name], configurable: true });
Object.defineProperty(globalThis, 'navigator', { value: dom.window.navigator, configurable: true });
globalThis.getComputedStyle = dom.window.getComputedStyle.bind(dom.window);
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
const { createElement: h, act } = await import('react'); const { createRoot } = await import('react-dom/client');
const { SourceBuildPanel } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/SourceBuildPanel.tsx'))).href);
const { SourceAcquisitionPanel } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/SourceAcquisitionPanel.tsx'))).href);
const timers = new Map(); let timerId = 0;
dom.window.setInterval = callback => { const id = ++timerId; timers.set(id, callback); return id; };
dom.window.clearInterval = id => timers.delete(id);
let root;
afterEach(async () => { if (root) await act(async () => root.unmount()); root = null; timers.clear(); });
after(async () => { dom.window.close(); await rm(output, { recursive: true, force: true }); });
const digest = 'a'.repeat(64); const sourceId = `src_${'1'.repeat(32)}`;
const source = { sourceId, taskId: 'acquire-fixture', name: 'Disposable web source', status: 'ready', url: 'https://example.invalid/source.git', sourcePath: 'C:\\artificial-source-copy', evidencePath: 'C:\\artificial-source-evidence', contentDigest: digest, inspection: { kind: 'project-v1-source', issues: [], executionState: 'not-executed' } };
function task(extra = {}) { return { taskId: 'source-build-fixture', projectId: null, type: 'sourceBuild', status: 'running', final: false, phase: 'dependencies', result: null, error: null, ...extra }; }
function success(extra = {}) { return task({ status: 'succeeded', final: true, phase: 'completed', result: { sourceId, sourceDigest: digest, candidatePath: 'C:\\artificial-candidate', artifactPath: 'C:\\artificial-candidate\\artifact.zip', artifactDigest: 'b'.repeat(64), operationOutcome: 'candidate-ready', installed: false, published: false }, ...extra }); }
function deferred() { let resolve; let reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; }
const button = text => [...document.querySelectorAll('button')].find(element => element.textContent === text);
async function click(element) { assert.ok(element); await act(async () => element.click()); }
async function choose(value = sourceId) { const select = document.querySelector('.source-build-panel select'); await act(async () => { Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set.call(select, value); select.dispatchEvent(new Event('change', { bubbles: true })); }); }
async function acknowledge() { await click(document.querySelector('.source-build-panel input[type="checkbox"]')); }
async function tick() { await act(async () => { for (const callback of [...timers.values()]) callback(); }); }
async function setup({ props = {}, mocks = {}, Component = SourceBuildPanel } = {}) {
  const calls = []; const tasks = []; const pending = []; let closed = 0;
  globalThis.__SOURCE_BUILD_INVOKE__ = async (command, args) => {
    calls.push([command, args]); if (mocks[command]) return mocks[command](args);
    if (command === 'source_build_list') return []; if (command === 'source_list') return [source];
    throw new Error(`Unexpected synthetic command: ${command}`);
  };
  let current = { sources: [source], disabled: false, instanceId: 'artificial-instance', onPendingChange: value => pending.push(value), onTask: value => tasks.push(value), onClose: () => { closed++; }, ...props };
  document.body.innerHTML = '<div id="root"></div>'; root = createRoot(document.getElementById('root'));
  async function render(next = {}) { current = { ...current, ...next }; await act(async () => root.render(h(Component, current))); }
  await render(); return { calls, tasks, pending, render, closed: () => closed };
}

test('Only ready project-v1 sources expose candidate construction and execution requires a separate acknowledgment', async () => {
  const fixtures = [source, { ...source, sourceId: 'node-source', inspection: { kind: 'node-service-source' } }, { ...source, sourceId: 'adapt-source', inspection: { kind: 'needs-adaptation' } }, { ...source, sourceId: 'failed-source', status: 'failed' }, { ...source, sourceId: 'unverified-source', contentDigest: '' }];
  const { calls } = await setup({ props: { sources: fixtures } });
  assert.deepEqual(calls, [['source_build_list', undefined]]); assert.equal(document.querySelector('select').options.length, 2);
  await choose(); assert.equal(button('准备独立构建候选').disabled, true);
  assert.match(document.body.textContent, /依赖与构建脚本/); assert.match(document.body.textContent, /平台将源码复制到独立目录，候选不会自动安装或发布/); assert.match(document.body.textContent, /以当前用户权限运行，只执行可信源码/); assert.doesNotMatch(document.body.textContent, /不会修改现有项目|现有项目保持原状/);
  assert.equal(document.querySelector('input:not([type="checkbox"])'), null);
});

test('Confirmed construction sends only the selected ID and digest, gates duplicates, and shows independent retained artifacts', async () => {
  const delayed = deferred(); const { calls, pending } = await setup({ mocks: { source_build_prepare: () => delayed.promise } });
  await choose(); await acknowledge(); const submit = button('准备独立构建候选');
  await act(async () => { submit.click(); submit.click(); });
  assert.deepEqual(calls.at(-1), ['source_build_prepare', { input: { sourceId, expectedDigest: digest, acknowledgeScripts: true } }]);
  assert.equal(calls.filter(([name]) => name === 'source_build_prepare').length, 1); assert.equal(button('正在提交候选构建…').disabled, true);
  assert.equal(pending.at(-1), true); await act(async () => delayed.resolve(success()));
  assert.equal(pending.at(-1), false); assert.match(document.body.textContent, /独立构建候选已生成/);
  assert.match(document.body.textContent, /尚未安装或发布/); assert.match(document.body.textContent, /artificial-candidate\\artifact.zip/);
  assert.doesNotMatch(document.body.textContent, /备份任务已完成|新网页已发布/);
  assert.equal(calls.some(([name]) => /install|project_build|project_import/.test(name)), false);
});

test('Changing source digests revokes script acknowledgment and old service responses cannot restore it', async () => {
  const delayed = deferred(); const { calls, render } = await setup({ mocks: { source_build_prepare: () => delayed.promise } });
  await choose(); await acknowledge(); await render({ sources: [{ ...source, contentDigest: 'c'.repeat(64) }] });
  assert.equal(document.querySelector('input[type="checkbox"]').checked, false); assert.equal(button('准备独立构建候选').disabled, true);
  await acknowledge(); await click(button('准备独立构建候选')); await render({ instanceId: 'new-instance' });
  await act(async () => delayed.resolve(success())); assert.equal(document.querySelector('.build-task'), null);
  assert.equal(calls.filter(([name]) => name === 'source_build_prepare').length, 1);
});

test('Reopened candidate history recovers running work, polls only its ID, and keeps final results monotonic with explicit logs', async () => {
  let recorded = [task()]; const { calls } = await setup({ mocks: { source_build_list: () => recorded, task_get: () => success(), task_logs: () => ({ taskId: 'source-build-fixture', cursor: 0, nextCursor: 8, text: 'artificial build log', truncated: false, gaps: [], eof: true }) } });
  assert.equal(document.querySelector('.source-build-panel select').disabled, true); await tick();
  assert.deepEqual(calls.find(([name]) => name === 'task_get'), ['task_get', { taskId: 'source-build-fixture' }]);
  assert.match(document.body.textContent, /独立构建候选已生成/);
  recorded = [task()]; await click(button('刷新候选构建任务')); assert.match(document.body.textContent, /独立构建候选已生成/);
  assert.equal(calls.some(([name]) => name === 'task_logs'), false); await click(button('查看任务日志'));
  assert.deepEqual(calls.at(-1), ['task_logs', { taskId: 'source-build-fixture', cursor: 0 }]); assert.match(document.body.textContent, /artificial build log/);
});

test('Unknown construction acceptance requires list reconciliation and never replays after a refresh', async () => {
  let accepted = false; const { calls } = await setup({ mocks: { source_build_prepare: () => { accepted = true; throw { message: 'Artificial response lost', details: { operationOutcome: 'unknown' } }; }, source_build_list: () => accepted ? [task()] : [] } });
  await choose(); await acknowledge(); await click(button('准备独立构建候选'));
  assert.match(document.body.textContent, /先刷新候选构建任务/); assert.equal(button('准备独立构建候选').disabled, true);
  await click(button('刷新候选构建任务')); assert.match(document.body.textContent, /准备独立源码候选 · 正在执行/);
  assert.equal(calls.filter(([name]) => name === 'source_build_prepare').length, 1);
});

test('Incomplete success and failed builds retain honest candidate messages and paths without backup or deployment claims', async () => {
  const invalid = success({ result: { ...success().result, installed: true } });
  const failed = task({ taskId: 'failed-candidate', status: 'failed', final: true, error: { code: 'DISPOSABLE_BUILD_FAILURE', message: 'Artificial failure', details: { operationOutcome: 'not_committed', candidatePath: 'C:\\retained-failed-candidate' } } });
  await setup({ mocks: { source_build_list: () => [invalid, failed] } });
  assert.match(document.body.textContent, /候选结果尚不能确认/); assert.match(document.body.textContent, /retained-failed-candidate/);
  assert.doesNotMatch(document.body.textContent, /备份任务已完成|新网页已发布|独立构建候选已生成/);
  assert.match(document.body.textContent, /未确认可用的构建候选/); assert.doesNotMatch(document.body.textContent, /迁移尚未提交/);
});

test('Windows source acquisition shares the main workflow, uses its own chooser and never mounts managed installation', async () => {
  const inspection = { kind: 'node-service-source', issues: [], executionState: 'not-executed', nextStep: 'Artificial next step: keep Node source for review' };
  const { calls } = await setup({ Component: SourceAcquisitionPanel, props: { chooseSourceCommand: 'source_choose_directory', managedInstallation: false, candidateBuild: true }, mocks: {
    source_choose_directory: () => 'C:\\artificial-external-source', source_inspect: () => ({ sourcePath: 'C:\\artificial-external-source', inspection }),
    source_capture: () => task({ taskId: 'source-capture-fixture', type: 'sourceCapture', status: 'succeeded', final: true, result: {} }),
  } });
  assert.equal(button('查看安装候选与准备安装'), undefined); await click(button('检查本机源码目录'));
  assert.deepEqual(calls.find(([name]) => name === 'source_choose_directory'), ['source_choose_directory', undefined]);
  assert.match(document.body.textContent, /Artificial next step/); assert.equal(calls.some(([name]) => name === 'source_capture'), false);
  await click(button('接入此目录的源码副本'));
  assert.deepEqual(calls.find(([name]) => name === 'source_capture'), ['source_capture', { input: { sourcePath: 'C:\\artificial-external-source' } }]);
  assert.equal(calls.some(([name]) => /node_install|source_build_prepare|choose_path/.test(name)), false);
});

test('Revoked source capability prevents a late chooser from inspecting and prevents initial background reads', async () => {
  const delayed = deferred(); const { calls, render } = await setup({ Component: SourceAcquisitionPanel, props: { chooseSourceCommand: 'source_choose_directory', managedInstallation: false, candidateBuild: true, disabled: true }, mocks: { source_choose_directory: () => delayed.promise } });
  assert.deepEqual(calls, []); await render({ disabled: false }); await click(button('检查本机源码目录'));
  await render({ disabled: true }); await act(async () => delayed.resolve('C:\\late-selection'));
  assert.equal(calls.some(([name]) => name === 'source_inspect'), false);
});

test('Accepted candidate submission holds every modal close route until its acknowledgment, then permits background continuation', async () => {
  const delayed = deferred(); const { calls, closed } = await setup({ Component: SourceAcquisitionPanel,
    props: { chooseSourceCommand: 'source_choose_directory', managedInstallation: false, candidateBuild: true },
    mocks: { source_build_prepare: () => delayed.promise },
  });
  await choose(); await acknowledge();
  await act(async () => {
    button('准备独立构建候选').click();
    document.querySelector('button[aria-label^="关闭"]').click();
    document.activeElement.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
    document.querySelector('.modal-layer').click();
  });
  assert.equal(closed(), 0); assert.equal(document.querySelector('button[aria-label^="关闭"]').disabled, true);
  assert.equal(button('检查本机源码目录').disabled, true);
  assert.equal(calls.filter(([name]) => name === 'source_build_prepare').length, 1);
  await act(async () => delayed.resolve(task()));
  assert.equal(document.querySelector('button[aria-label^="关闭"]').disabled, false);
  await click(document.querySelector('button[aria-label^="关闭"]')); assert.equal(closed(), 1);
});

test('Malformed recovered task identities fail closed and revoked capability closes task log commands', async () => {
  let values = [task({ type: 'build' })]; const { calls, render } = await setup({ mocks: { source_build_list: () => values } });
  assert.match(document.body.textContent, /任务身份或状态不完整/);
  await choose(); assert.equal(document.querySelector('input[type="checkbox"]').disabled, true);
  values = [task()]; await click(button('刷新候选构建任务')); await render({ disabled: true });
  assert.equal(button('查看任务日志').disabled, true); await click(button('查看任务日志'));
  assert.equal(calls.some(([name]) => name === 'task_logs'), false);
});

test('Candidate cancellation sends its exact task ID once, holds modal closure until acknowledgment and then waits for the final task', async () => {
  const delayed = deferred(); const cancelled = task({ status: 'failed', final: true, error: { code: 'TASK_CANCELLED', message: 'Artificial cancellation retained' } });
  const { calls, closed } = await setup({ Component: SourceAcquisitionPanel,
    props: { chooseSourceCommand: 'source_choose_directory', managedInstallation: false, candidateBuild: true },
    mocks: { source_build_list: () => [task()], source_cancel: () => delayed.promise, task_get: () => cancelled },
  });
  await act(async () => {
    const cancel = button('取消候选构建'); cancel.click(); cancel.click();
    document.querySelector('button[aria-label^="关闭"]').click();
    document.activeElement.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
    document.querySelector('.modal-layer').click();
  });
  assert.deepEqual(calls.filter(([name]) => name === 'source_cancel'), [['source_cancel', { taskId: 'source-build-fixture' }]]);
  assert.equal(button('正在提交取消…').disabled, true); assert.equal(closed(), 0);
  await act(async () => delayed.resolve(task()));
  assert.match(document.body.textContent, /已请求取消候选构建，正在等待最终状态/);
  assert.equal(document.querySelector('button[aria-label^="关闭"]').disabled, false);
  await tick(); assert.match(document.body.textContent, /Artificial cancellation retained/);
  assert.equal(button('取消候选构建'), undefined);
  assert.equal(calls.filter(([name]) => name === 'source_build_prepare').length, 0);
});

test('Unknown cancellation responses require an explicit task reread and never automatically replay cancellation', async () => {
  let observed = [task()]; const { calls } = await setup({ mocks: { source_build_list: () => observed,
    source_cancel: () => { throw { message: 'Artificial cancellation reply lost', details: { operationOutcome: 'unknown' } }; },
    task_get: () => task(),
  } });
  await click(button('取消候选构建')); assert.match(document.body.textContent, /取消回执尚不能确认/);
  assert.equal(button('取消候选构建').disabled, true);
  await tick(); await click(button('取消候选构建'));
  assert.equal(calls.filter(([name]) => name === 'source_cancel').length, 1);
  observed = [task({ status: 'interrupted', final: true })]; await click(button('刷新候选构建任务'));
  assert.equal(button('取消候选构建'), undefined);
  assert.equal(calls.filter(([name]) => name === 'source_cancel').length, 1);
});

test('Native source next-step codes and candidate build phases have concrete Chinese instructions', async () => {
  const { render } = await setup({ Component: SourceAcquisitionPanel,
    props: { chooseSourceCommand: 'source_choose_directory', managedInstallation: false, candidateBuild: true },
    mocks: {
      source_list: () => [
        { ...source, sourceId: 'node-only', inspection: { kind: 'node-service-source', nextStep: 'prepare-node-runtime-before-registration', issues: [] } },
        { ...source, sourceId: 'review-only', inspection: { kind: 'needs-adaptation', nextStep: 'review-and-adapt-source', issues: [] } },
        { ...source, inspection: { ...source.inspection, nextStep: 'review-project-source-before-integration' } },
      ],
      source_build_list: () => ['checking-source', 'installing-dependencies', 'building-candidate', 'checking-artifact'].map((phase, index) => task({ taskId: `phase-${index}`, phase })),
    },
  });
  assert.match(document.body.textContent, /Windows 安装与更新尚未开放/); assert.match(document.body.textContent, /AI 只读检查公开材料/);
  for (const text of ['核对独立候选源码', '运行候选依赖脚本', '构建独立候选网页', '核对候选产物']) assert.match(document.body.textContent, new RegExp(text));
  assert.doesNotMatch(document.body.textContent, /prepare-node-runtime-before-registration|review-and-adapt-source|checking-source|installing-dependencies|building-candidate|checking-artifact/);
  await render({ disabled: true });
});
