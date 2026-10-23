// Disposable DOM fixtures only; these tests do not prove native installation.
import assert from 'node:assert/strict';
import { after, afterEach, test } from 'node:test';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';
import { JSDOM } from 'jsdom';

const repo = fileURLToPath(new URL('../../../', import.meta.url));
await mkdir(path.join(repo, '.test-artifacts'), { recursive: true });
const output = await mkdtemp(path.join(repo, '.test-artifacts/disposable-node-install-dom-'));
const mockPath = path.join(output, 'tauri-mock.mjs');
await writeFile(mockPath, 'export const invoke = (...args) => globalThis.__NODE_INSTALL_INVOKE__(...args);\n');
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
    assert.ok(dependency);
    source = source.replaceAll(`${match[1]}${match[2]}${match[1]}`, JSON.stringify(pathToFileURL(await compile(dependency)).href));
  }
  source = source.replaceAll(/(['"])@tauri-apps\/api\/core\1/g, JSON.stringify(pathToFileURL(mockPath).href));
  await writeFile(target, source);
  return target;
}

const dom = new JSDOM('<!doctype html><html><body></body></html>', { url: 'http://localhost/', pretendToBeVisual: true });
for (const name of ['window', 'document', 'Node', 'HTMLElement', 'HTMLInputElement', 'HTMLTextAreaElement', 'HTMLSelectElement', 'Event', 'MouseEvent']) Object.defineProperty(globalThis, name, { value: name === 'window' ? dom.window : dom.window[name], configurable: true });
Object.defineProperty(globalThis, 'navigator', { value: dom.window.navigator, configurable: true });
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
const { createElement: h, act, StrictMode } = await import('react');
const { createRoot } = await import('react-dom/client');
const { NodeInstallPanel } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/NodeInstallPanel.tsx'))).href);
let root;
const timers = new Map();
let nextTimer = 0;
dom.window.setInterval = fn => { const id = ++nextTimer; timers.set(id, fn); return id; };
dom.window.clearInterval = id => { timers.delete(id); };
async function tick() { await act(async () => { for (const fn of [...timers.values()]) fn(); }); }
async function unmount() { if (root) await act(async () => root.unmount()); root = null; }
afterEach(async () => { await unmount(); timers.clear(); });
after(async () => { dom.window.close(); await rm(output, { recursive: true, force: true }); });

const sourceId = `src_${'1'.repeat(32)}`;
const otherSource = `src_${'2'.repeat(32)}`;
const candidateId = `install_${'3'.repeat(32)}`;
const appId = '11111111-1111-4111-8111-111111111111';
const otherApp = '22222222-2222-4222-8222-222222222222';
const digest = 'a'.repeat(64);
const generation = `gen_${'4'.repeat(32)}`;
const source = { sourceId, name: 'Disposable Node Source', status: 'ready', contentDigest: digest, inspection: { kind: 'node-service-source', name: 'Disposable App', issues: [] } };
const prepared = { candidateId, appId, generation, manifest: { runtime: 'node-service', app: { id: 'org.example.disposable', name: 'Disposable App', version: '1.2.0' } }, programDigest: 'b'.repeat(64), provenance: { sourceId, sourceDigest: digest, sourceCommit: 'c'.repeat(40), node: { path: '/disposable/node', version: '24.22.0' }, npm: { path: '/disposable/npm', version: '11.19.1' }, preparedAt: 1 } };
const installed = { candidateId, appId, generation, operationOutcome: 'committed' };
function task(overrides = {}) { return { taskId: 'task_prepare', projectId: null, type: 'nodeInstallPrepare', status: 'running', final: false, phase: 'dependencies', result: null, error: null, ...overrides }; }
function readyTask(overrides = {}) { return task({ final: true, status: 'succeeded', result: { state: 'prepared', candidate: prepared, installed: false }, ...overrides }); }
function applyTask(overrides = {}) { return task({ taskId: 'task_apply', type: 'nodeInstallApply', final: true, status: 'succeeded', result: installed, ...overrides }); }
function history(overrides = {}) { return { candidateId, appId, targetAppId: null, state: 'prepared', prepareTaskId: 'task_prepare', applyTaskId: null, prepared, error: null, issues: [], ...overrides }; }
function deferred() { let resolve; let reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { resolve, reject, promise }; }
async function setup({ overrides = {}, props = {}, strict = false } = {}) {
  const calls = []; const tasks = []; const installs = []; const pending = [];
  globalThis.__NODE_INSTALL_INVOKE__ = async (command, args) => {
    calls.push([command, args]);
    if (overrides[command]) return overrides[command](args);
    if (command === 'node_install_list') return { candidates: [], issues: [] };
    if (command === 'node_install_candidate') return { state: 'prepared', candidate: prepared };
    throw new Error(`Unexpected disposable command: ${command}`);
  };
  let currentProps = { sources: [source], disabled: false, instanceId: 'instance_a', onTask: value => tasks.push(value), onInstalled: value => installs.push(value), onPendingChange: value => pending.push(value), ...props };
  document.body.innerHTML = '<div id="root"></div>';
  root = createRoot(document.getElementById('root'));
  async function render(next = {}) { currentProps = { ...currentProps, ...next }; await act(async () => root.render(strict ? h(StrictMode, null, h(NodeInstallPanel, currentProps)) : h(NodeInstallPanel, currentProps))); }
  await render();
  return { calls, tasks, installs, pending, render };
}
const byText = text => [...document.querySelectorAll('button')].find(element => element.textContent === text);
const select = () => document.querySelector('select');
const commandCalls = (calls, command) => calls.filter(([name]) => name === command);
async function click(element) { assert.ok(element); await act(async () => element.click()); }
async function choose(value = sourceId) {
  await act(async () => {
    Object.getOwnPropertyDescriptor(dom.window.HTMLSelectElement.prototype, 'value').set.call(select(), value);
    select().dispatchEvent(new window.Event('change', { bubbles: true }));
  });
}
async function prepareClick() { await click(byText('准备安装材料') ?? byText('重新准备安装材料')); }
async function submit() { await act(async () => document.querySelector('form').dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true }))); }
async function confirmAll() { for (const input of document.querySelectorAll('input[type="checkbox"]')) if (!input.checked) await click(input); }

test('Only ready Node source candidates can be selected; opening stays read-only and accepts no path or digest input', async () => {
  const { calls } = await setup({ props: { sources: [source, { ...source, sourceId: otherSource, status: 'failed' }, { ...source, sourceId: `src_${'5'.repeat(32)}`, inspection: { kind: 'project-v1-source' } }, { ...source, sourceId: `src_${'6'.repeat(32)}`, contentDigest: 'forged' }] } });
  assert.equal(select().options.length, 2);
  assert.deepEqual(calls, [['node_install_list', undefined]]);
  assert.equal(document.querySelector('input'), null);
  assert.equal(byText('准备安装材料').disabled, true);
  assert.match(document.body.textContent, /跳过项目与依赖的安装脚本/);
  await submit();
  assert.equal(commandCalls(calls, 'node_install_prepare').length, 0);
});

test('Preparation passes only selected source ID, recorded digest and explicit null target; it never applies automatically', async () => {
  const { calls, pending } = await setup({ overrides: { node_install_prepare: () => readyTask() } });
  await choose(); await prepareClick();
  assert.deepEqual(commandCalls(calls, 'node_install_prepare'), [['node_install_prepare', { input: { sourceId, expectedSourceDigest: digest, targetAppId: null } }]]);
  assert.deepEqual(commandCalls(calls, 'node_install_candidate'), [['node_install_candidate', { candidateId }]]);
  assert.equal(commandCalls(calls, 'node_install_apply').length, 0);
  assert.equal(byText('确认安装').disabled, true);
  assert.match(document.body.textContent, /尚未安装/);
  assert.match(document.body.textContent, /Disposable App · 1.2.0/);
  assert.equal(pending.at(-1), false);
});

test('Confirmed installation sends only the verified candidate ID and never starts or stops an app', async () => {
  const { calls, installs } = await setup({ overrides: { node_install_prepare: () => readyTask(), node_install_apply: () => applyTask() } });
  await choose(); await prepareClick(); await confirmAll(); await click(byText('确认安装'));
  assert.deepEqual(commandCalls(calls, 'node_install_apply'), [['node_install_apply', { candidateId }]]);
  assert.deepEqual(installs, [installed]);
  assert.match(document.body.textContent, /应用未自动启动/);
  assert.ok(calls.every(([name]) => !['node_app_open', 'node_app_start', 'node_app_stop', 'choose_path'].includes(name)));
  assert.equal(byText('确认安装'), undefined);
});

test('Updating keeps its fixed target and requires the closed-window acknowledgement; native rejection requires re-verification', async () => {
  const { calls } = await setup({ props: { targetAppId: appId }, overrides: {
    node_install_prepare: () => readyTask(),
    node_install_apply: () => { throw { code: 'NODE_INSTALL_BUSY', message: '请先正常关闭应用窗口' }; },
  } });
  await choose(); await prepareClick();
  assert.equal(commandCalls(calls, 'node_install_prepare')[0][1].input.targetAppId, appId);
  const checks = document.querySelectorAll('input[type="checkbox"]');
  await click(checks[1]); assert.equal(byText('确认更新').disabled, true);
  await click(checks[0]); await click(byText('确认更新'));
  assert.match(document.body.textContent, /NODE_INSTALL_BUSY/);
  assert.match(document.body.textContent, /更新保留数据和配置/);
  assert.equal(byText('确认更新').disabled, true);
  assert.equal(commandCalls(calls, 'node_install_apply').length, 1);
  await click(byText('核验准备结果'));
  assert.equal(byText('确认更新').disabled, true);
});

test('A pending prepare blocks duplicate submission and late cancellation acknowledgement cannot downgrade completion', async () => {
  const request = deferred(); const cancelling = deferred(); let polls = 0;
  const { calls, tasks, pending } = await setup({ overrides: {
    node_install_prepare: () => request.promise,
    node_install_cancel: () => cancelling.promise,
    task_get: () => ++polls === 1 ? task() : readyTask(),
  } });
  await choose(); await prepareClick(); await submit();
  assert.equal(commandCalls(calls, 'node_install_prepare').length, 1);
  assert.equal(pending.at(-1), true);
  await act(async () => request.resolve(task()));
  assert.equal(pending.at(-1), false);
  await click(byText('取消准备')); await tick();
  await act(async () => cancelling.resolve(task()));
  assert.equal(tasks.at(-1).final, true);
  assert.equal(byText('取消准备'), undefined);
  assert.deepEqual(commandCalls(calls, 'node_install_cancel'), [['node_install_cancel', { taskId: 'task_prepare' }]]);
  assert.equal(commandCalls(calls, 'node_install_apply').length, 0);
});

test('Cancellation stays pending until confirmed, then labels the task and retained preparation without retrying or changing API status', async () => {
  let finished = false;
  const error = { code: 'NODE_INSTALL_CANCELLED', message: '安装准备已取消，当前程序及数据未改变。' };
  const cancelled = task({ status: 'failed', final: true, phase: 'failed', error, result: { candidateId, installed: false, partialFilesRetained: true } });
  const { calls, tasks, installs } = await setup({ overrides: {
    node_install_prepare: () => task(),
    node_install_cancel: () => task(),
    task_get: () => finished ? cancelled : task(),
    node_install_list: () => ({ candidates: finished ? [history({ state: 'failed', prepared: null, error })] : [], issues: [] }),
  } });
  await choose(); await prepareClick(); await click(byText('取消准备'));
  assert.equal(document.querySelector('.build-task-heading strong').textContent, '准备应用安装 · 正在执行');
  assert.match(document.body.textContent, /已请求取消准备，正在等待任务停止/);
  assert.doesNotMatch(document.querySelector('.build-task').textContent, /已取消|安装准备已停止/);
  assert.equal(byText('准备安装材料').disabled, true);
  finished = true; await tick();
  assert.equal(document.querySelector('.build-task-heading strong').textContent, '准备应用安装 · 已取消');
  assert.equal(document.querySelector('.build-task-heading span').textContent, '安装准备已停止');
  assert.match(document.querySelector('.build-task').textContent, /安装准备已取消，当前程序及数据未改变。 \(NODE_INSTALL_CANCELLED\)/);
  assert.match(document.querySelector('[aria-label="保留的安装记录"] article p').textContent, /准备已取消，现场保留/);
  assert.equal(select().value, sourceId);
  assert.equal(byText('准备安装材料').disabled, false);
  assert.equal(byText('取消准备'), undefined);
  assert.equal(byText('确认安装'), undefined);
  assert.equal(tasks.at(-1), cancelled);
  assert.equal(cancelled.status, 'failed');
  assert.equal(cancelled.phase, 'failed');
  assert.deepEqual(installs, []);
  assert.equal(commandCalls(calls, 'node_install_prepare').length, 1);
  assert.equal(commandCalls(calls, 'node_install_cancel').length, 1);
  assert.equal(commandCalls(calls, 'node_install_apply').length, 0);
});

test('Unconfirmed cleanup, ordinary failure, shutdown and apply errors keep their original task and retained-record states', async () => {
  for (const [type, status, code, heading, recordLabel] of [
    ['nodeInstallPrepare', 'failed', 'NODE_INSTALL_CLEANUP_PENDING', '准备应用安装 · 失败', '未完成，现场保留'],
    ['nodeInstallPrepare', 'failed', 'NODE_INSTALL_NPM_FAILED', '准备应用安装 · 失败', '未完成，现场保留'],
    ['nodeInstallPrepare', 'interrupted', 'NODE_INSTALL_INTERRUPTED', '准备应用安装 · 已中断', '已中断，现场保留'],
    ['nodeInstallApply', 'failed', 'NODE_INSTALL_CANCELLED', '安装或更新应用 · 失败', '未完成，现场保留'],
  ]) {
    const error = { code, message: '人工保留诊断' };
    const terminal = task({ type, status, final: true, phase: status, error });
    const { calls } = await setup({ props: { task: terminal }, overrides: {
      node_install_list: () => ({ candidates: [history({ state: status, prepared: null, error, applyTaskId: type === 'nodeInstallApply' ? 'task_apply' : null })], issues: [] }),
    } });
    assert.equal(document.querySelector('.build-task-heading strong').textContent, heading);
    assert.equal(document.querySelector('.build-task-heading span').textContent, status);
    assert.ok(document.querySelector('[aria-label="保留的安装记录"] article p').textContent.includes(recordLabel));
    assert.match(document.querySelector('.build-task').textContent, new RegExp(code));
    assert.equal(commandCalls(calls, 'node_install_prepare').length, 0);
    assert.equal(commandCalls(calls, 'node_install_apply').length, 0);
    await unmount();
  }
});

test('Polling failure retains the active prepare and selected source without allowing duplicate work', async () => {
  const { calls } = await setup({ overrides: {
    node_install_prepare: () => task(), task_get: () => { throw new Error('人工状态失败'); },
  } });
  await choose(); await prepareClick();
  assert.match(document.body.textContent, /任务状态暂时无法确认/);
  assert.equal(select().value, sourceId);
  assert.equal(byText('准备安装材料').disabled, true);
  await submit();
  assert.equal(commandCalls(calls, 'node_install_prepare').length, 1);
});

test('Candidate identity, source digest and target mismatches cannot reach the apply step', async () => {
  for (const altered of [
    { ...prepared, provenance: { ...prepared.provenance, sourceDigest: 'd'.repeat(64) } },
    { ...prepared, appId: otherApp },
  ]) {
    const { calls } = await setup({ props: { targetAppId: appId }, overrides: { node_install_prepare: () => readyTask({ result: { state: 'prepared', candidate: altered, installed: false } }) } });
    await choose(); await prepareClick();
    assert.match(document.body.textContent, /不一致/);
    assert.equal(byText('确认更新'), undefined);
    assert.equal(commandCalls(calls, 'node_install_apply').length, 0);
    await unmount();
  }
});

test('Changing source records invalidates an already checked candidate rather than sending a stale digest', async () => {
  const { render, calls } = await setup({ overrides: { node_install_prepare: () => readyTask() } });
  await choose(); await prepareClick(); await confirmAll();
  assert.equal(byText('确认安装').disabled, false);
  await render({ sources: [{ ...source, contentDigest: 'd'.repeat(64) }] });
  assert.equal(byText('确认安装').disabled, true);
  assert.match(document.body.textContent, /不能直接安装/);
  assert.equal(commandCalls(calls, 'node_install_apply').length, 0);
});

test('Target and instance changes reject late task and candidate results', async () => {
  const request = deferred();
  const { render, tasks, installs } = await setup({ overrides: { node_install_prepare: () => request.promise } });
  await choose(); await prepareClick();
  await render({ targetAppId: otherApp, instanceId: 'instance_b' });
  await act(async () => request.resolve(readyTask()));
  assert.equal(tasks.length, 0); assert.equal(installs.length, 0);
  assert.equal(byText('确认更新'), undefined);
  assert.equal(select().value, '');
});

test('Prepared history resumes only after explicit selection and a fresh candidate lookup', async () => {
  const { calls } = await setup({ overrides: { node_install_list: () => ({ candidates: [history()], issues: [] }) } });
  assert.equal(commandCalls(calls, 'node_install_candidate').length, 0);
  assert.equal(commandCalls(calls, 'node_install_apply').length, 0);
  await click(byText('核验并继续安装'));
  assert.equal(select().value, sourceId);
  assert.equal(commandCalls(calls, 'node_install_candidate').length, 1);
  assert.equal(byText('确认安装').disabled, true);
});

test('History separates new installs from update targets and preserves unavailable records', async () => {
  const badId = `install_${'9'.repeat(32)}`;
  const entries = [history(), history({ candidateId: `install_${'8'.repeat(32)}`, targetAppId: appId }), history({ candidateId: badId, appId: null, state: 'unavailable', prepared: null, issues: [{ message: '人工损坏记录已保留' }] })];
  const { render } = await setup({ overrides: { node_install_list: () => ({ candidates: entries, issues: [] }) } });
  assert.ok(document.body.textContent.includes(candidateId));
  assert.ok(!document.body.textContent.includes(`install_${'8'.repeat(32)}`));
  assert.match(document.body.textContent, /人工损坏记录已保留/);
  await render({ targetAppId: appId });
  assert.ok(!document.body.textContent.includes(candidateId));
  assert.ok(document.body.textContent.includes(`install_${'8'.repeat(32)}`));
  assert.ok(document.body.textContent.includes(badId));
});

test('A retained active task can be resumed and cancelled, including after remount', async () => {
  const { calls } = await setup({ overrides: {
    node_install_list: () => ({ candidates: [history({ state: 'preparing', prepared: null })], issues: [] }),
    task_get: () => task(), node_install_cancel: () => task(),
  } });
  await click(byText('继续查看任务'));
  assert.ok(byText('取消准备'));
  await click(byText('取消准备'));
  assert.equal(commandCalls(calls, 'node_install_cancel').length, 1);
});

test('An unknown apply outcome requires explicit reconciliation and never reports installed or retries', async () => {
  const { calls, installs } = await setup({ overrides: {
    node_install_prepare: () => readyTask(),
    node_install_apply: () => applyTask({ status: 'failed', result: { candidateId, operationOutcome: 'unknown' }, error: { code: 'NODE_INSTALL_RECOVERY_REQUIRED', message: '安装结果待核对', details: { operationOutcome: 'unknown' } } }),
  } });
  await choose(); await prepareClick(); await confirmAll(); await click(byText('确认安装'));
  assert.match(document.body.textContent, /安装结果尚不能确认/);
  assert.equal(installs.length, 0);
  assert.equal(byText('确认安装').disabled, true);
  assert.equal(commandCalls(calls, 'node_install_apply').length, 1);
});

test('Resuming a retained task for another source discards the previous candidate binding', async () => {
  const nextCandidate = `install_${'7'.repeat(32)}`;
  const nextPrepared = { ...prepared, candidateId: nextCandidate, appId: otherApp, generation: `gen_${'8'.repeat(32)}`, provenance: { ...prepared.provenance, sourceId: otherSource } };
  let reads = 0;
  const { calls } = await setup({ props: { sources: [source, { ...source, sourceId: otherSource, name: 'Second source' }] }, overrides: {
    node_install_list: () => ({ candidates: ++reads === 1 ? [] : [history({ candidateId: nextCandidate, appId: otherApp, state: 'preparing', prepared: null, prepareTaskId: 'task_resumed' })], issues: [] }),
    node_install_prepare: () => readyTask(),
    node_install_candidate: ({ candidateId: id }) => ({ state: 'prepared', candidate: id === nextCandidate ? nextPrepared : prepared }),
    task_get: () => readyTask({ taskId: 'task_resumed', result: { state: 'prepared', candidate: nextPrepared, installed: false } }),
  } });
  await choose(); await prepareClick();
  assert.equal(select().value, sourceId);
  await click(byText('继续查看任务'));
  assert.equal(select().value, otherSource);
  assert.equal(document.querySelector('[role="alert"]'), null);
  assert.match(document.querySelector('[aria-label="安装候选确认"]').textContent, new RegExp(otherSource));
  await confirmAll();
  assert.equal(byText('确认安装').disabled, false);
  assert.equal(commandCalls(calls, 'node_install_apply').length, 0);
});

test('Committed apply with failed finalization reports the actual installation once without restarting it', async () => {
  const { installs } = await setup({ overrides: {
    node_install_prepare: () => readyTask(),
    node_install_apply: () => applyTask({ status: 'failed', error: { code: 'NODE_INSTALL_RECOVERY_REQUIRED', message: '收尾失败', details: { operationOutcome: 'committed' } } }),
  } });
  await choose(); await prepareClick(); await confirmAll(); await click(byText('确认安装'));
  assert.deepEqual(installs, [installed]);
  assert.match(document.body.textContent, /安装已提交，但收尾需要检查/);
  assert.equal(byText('确认安装'), undefined);
});

test('Disabled state and StrictMode reject writes and stale initial history without losing diagnostics', async () => {
  const old = deferred(); let reads = 0;
  const { calls, render } = await setup({ strict: true, overrides: { node_install_list: () => ++reads === 1 ? old.promise : ({ candidates: [], issues: [{ message: '人工安装环境诊断' }] }) } });
  await act(async () => old.resolve({ candidates: [history()], issues: [] }));
  assert.equal(byText('核验并继续安装'), undefined);
  assert.match(document.body.textContent, /人工安装环境诊断/);
  await choose(); await render({ disabled: true }); await submit();
  assert.equal(commandCalls(calls, 'node_install_prepare').length, 0);
});

function exposedText(element) {
  if (!element) return '';
  if (element.nodeType === window.Node.TEXT_NODE) return element.textContent;
  if (!(element instanceof HTMLElement) || element.hidden) return '';
  if (element.tagName === 'DETAILS' && !element.open) return exposedText(element.querySelector(':scope > summary'));
  return [...element.childNodes].map(exposedText).join(' ').replace(/\s+/g, ' ').trim();
}

test('Installation leads with the application and explicit confirmation while technical records stay folded', async () => {
  await setup({ props: { targetAppId: appId }, overrides: { node_install_prepare: () => readyTask() } });
  const method = document.querySelector('.node-install-method');
  assert.equal(method.open, false);
  assert.match(exposedText(document.querySelector('.node-install-panel')), /更新保留数据和配置.*确认新版兼容现有数据/);
  assert.doesNotMatch(exposedText(document.querySelector('.node-install-panel')), /npm|ignore-scripts|下载生产依赖/);
  assert.equal(select().options[1].textContent, 'Disposable App');
  await choose(); await prepareClick();
  const candidate = document.querySelector('[aria-label="安装候选确认"]');
  const verification = candidate.querySelector('.node-install-verification');
  assert.equal(verification.open, false);
  assert.match(exposedText(candidate), /Disposable App.*1\.2\.0.*我已正常关闭目标应用窗口.*确认\s*更新此应用/);
  assert.doesNotMatch(exposedText(candidate), /org\.example|src_|install_|gen_|准备环境|源码基线提交/);
  await click(verification.querySelector('summary'));
  assert.match(exposedText(candidate), /org\.example\.disposable/);
  assert.ok(exposedText(candidate).includes(prepared.provenance.sourceDigest));
  assert.ok(exposedText(candidate).includes(prepared.programDigest));
  assert.ok(exposedText(candidate).includes(prepared.provenance.sourceCommit));
  assert.match(exposedText(candidate), /Node 24\.22\.0/);
});
