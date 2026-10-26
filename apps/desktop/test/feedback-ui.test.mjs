// Feedback regressions use a disposable DOM, fake window timers and mocked
// native commands. They never open an application, service or real workspace.
import assert from 'node:assert/strict';
import { after, afterEach, test } from 'node:test';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';
import { JSDOM } from 'jsdom';

const repo = fileURLToPath(new URL('../../../', import.meta.url));
await mkdir(path.join(repo, '.test-artifacts'), { recursive: true });
const output = await mkdtemp(path.join(repo, '.test-artifacts/disposable-feedback-dom-'));
const mockPath = path.join(output, 'tauri-mock.mjs');
await writeFile(mockPath, 'export const invoke = (...args) => globalThis.__FEEDBACK_INVOKE__(...args); export const isTauri = () => true;\n');
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
      try { await readFile(candidate); dependency = candidate; break; } catch { /* Try the next source extension. */ }
    }
    assert.ok(dependency, `Cannot resolve ${match[2]}`);
    source = source.replaceAll(`${match[1]}${match[2]}${match[1]}`, JSON.stringify(pathToFileURL(await compile(dependency)).href));
  }
  source = source.replaceAll(/(['"])@tauri-apps\/api\/core\1/g, JSON.stringify(pathToFileURL(mockPath).href));
  await writeFile(target, source);
  return target;
}

const dom = new JSDOM('<!doctype html><html><body></body></html>', { url: 'http://localhost/', pretendToBeVisual: true });
for (const name of ['window', 'document', 'Node', 'HTMLElement', 'HTMLInputElement', 'HTMLTextAreaElement', 'HTMLSelectElement', 'Event', 'MouseEvent', 'KeyboardEvent', 'FocusEvent', 'CustomEvent', 'MutationObserver']) Object.defineProperty(globalThis, name, { value: name === 'window' ? dom.window : dom.window[name], configurable: true });
Object.defineProperty(globalThis, 'navigator', { value: dom.window.navigator, configurable: true });
Object.defineProperty(globalThis, 'localStorage', { value: dom.window.localStorage, configurable: true });
globalThis.getComputedStyle = dom.window.getComputedStyle.bind(dom.window);
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
dom.window.HTMLElement.prototype.scrollIntoView = () => {};

let clock = 0; let timerId = 0;
const timers = new Map(); const timerHistory = new Map();
function schedule(callback, delay, interval, args) {
  const id = ++timerId; const ms = Math.max(interval ? 1 : 0, Number(delay) || 0);
  const timer = { id, callback, delay: ms, due: clock + ms, interval, args };
  timers.set(id, timer); timerHistory.set(id, timer);
  return id;
}
window.setTimeout = (callback, delay, ...args) => schedule(callback, delay, false, args);
window.clearTimeout = id => timers.delete(id);
window.setInterval = (callback, delay, ...args) => schedule(callback, delay, true, args);
window.clearInterval = id => timers.delete(id);

globalThis.__FEEDBACK_INVOKE__ = async command => { throw new Error(`Unexpected disposable native command: ${command}`); };
const { createElement: h, act } = await import('react');
const { createRoot } = await import('react-dom/client');
const { default: App } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/App.tsx'))).href);
let root;
async function unmount() { if (root) await act(async () => root.unmount()); root = null; }
afterEach(async () => {
  await unmount(); timers.clear(); timerHistory.clear(); clock = 0;
  delete navigator.clipboard; localStorage.clear();
  globalThis.__FEEDBACK_INVOKE__ = async command => { throw new Error(`Unexpected disposable native command: ${command}`); };
});
after(async () => { dom.window.close(); await rm(output, { recursive: true, force: true }); });

async function advance(milliseconds) {
  const target = clock + milliseconds; let executions = 0;
  while (true) {
    const next = [...timers.values()].filter(timer => timer.due <= target).sort((a, b) => a.due - b.due || a.id - b.id)[0];
    if (!next) break;
    assert.ok(++executions < 1000, 'Fake timers must not spin indefinitely');
    clock = next.due;
    if (next.interval) next.due += next.delay; else timers.delete(next.id);
    await act(async () => next.callback(...next.args));
  }
  clock = target;
  await act(async () => {});
}
function deferred() { let resolve; let reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; }
const bootstrap = { displayName: 'Disposable feedback fixture', identifier: 'dev.disposable.feedback', environment: 'development', apiVersion: 1, instanceId: 'feedback-instance', workspaceRoot: '/disposable/feedback/workspace', dataRoot: '/disposable/feedback/data', baseUrl: 'http://127.0.0.1:1', sqliteVersion: 'fixture', version: 'fixture', resources: { sdk: true, template: true, readme: true, cli: true }, cliPath: '/disposable/feedback/cli' };
const web = (id = 'prj_feedback_a', name = 'Fixture Web A') => ({ id, name, description: '', projectRoot: `${bootstrap.workspaceRoot}/projects/${id}`, documentEpoch: '11111111-1111-4111-8111-111111111111', state: 'ready', issues: [], manifest: { database: 'data/main.sqlite', sourceDir: 'app', outputDir: 'dist', attachmentsDir: 'data/attachments', docs: { readme: 'README.md', database: 'docs/database.md', platform: 'docs/platform.md' } } });
const node = { id: 'feedback_node_fixture', name: 'Fixture Node', sourcePath: '/disposable/feedback/external-node', runtime: 'node-service', state: 'stopped', restartRequired: false, runningRevision: null, lastError: null, fields: [], configuration: { revision: 1, values: {}, secretStatus: {}, missingRequired: [], canStart: true } };
const storage = { platformProgram: { totalBytes: 0, complete: true }, workspace: { totalBytes: 0, complete: true, categories: {} }, otherLocalLocations: [] };
const task = (extra = {}) => ({ taskId: 'task_feedback_fixture', projectId: null, type: 'workspaceMigration', status: 'running', final: false, phase: 'copying', result: null, error: null, ...extra });

async function setup({ projects = [web(), web('prj_feedback_b', 'Fixture Web B')], nodes = [], workspace = { workspaceRoot: bootstrap.workspaceRoot, migrationState: 'ready', activeTask: null, retainedRoots: [] }, overrides = {} } = {}) {
  const calls = [];
  globalThis.__FEEDBACK_INVOKE__ = async (command, args) => {
    calls.push([command, args]);
    if (overrides[command]) return overrides[command](args);
    if (command === 'bootstrap_info') return bootstrap;
    if (command === 'project_list') return projects;
    if (command === 'workspace_info') return workspace;
    if (command === 'workspace_storage') return storage;
    if (command === 'node_app_list') return nodes;
    if (command === 'node_app_configuration' || command === 'node_app_status') return nodes.find(value => value.id === args.appId);
    if (command === 'project_git_source_restore_status') return { state: 'clear', operations: [], issues: [], database: { state: 'not-supported' } };
    if (command === 'maintenance_status') return { mode: 'ready', recoveryRequired: false, activeTask: null };
    if (command === 'project_metadata') {
      const project = projects.find(value => value.id === args.projectId);
      return { name: project.name, description: '', manifest: project.manifest, metadataRevision: 'feedback-metadata', metadataIssues: [], iconStatus: { state: 'default' } };
    }
    if (command === 'project_open') return { projectId: args.projectId, target: 'desktop', reused: false };
    if (command === 'project_handoff') return { text: 'Disposable complete AI handoff', projectRoot: projects.find(value => value.id === args.projectId).projectRoot, instanceId: bootstrap.instanceId };
    throw new Error(`Unexpected disposable native command: ${command}`);
  };
  document.body.innerHTML = '<div id="root"></div>';
  root = createRoot(document.getElementById('root'));
  await act(async () => root.render(h(App)));
  return calls;
}
const byText = (text, scope = document) => [...scope.querySelectorAll('button')].find(button => button.textContent.trim() === text);
const dialog = () => document.querySelector('[role="dialog"]');
const success = () => document.querySelector('.notice-success');
const failure = () => document.querySelector('.notice-error');
async function click(element) { assert.ok(element); await act(async () => element.click()); }
async function change(element, value) {
  assert.ok(element);
  await act(async () => {
    Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set.call(element, value);
    element.dispatchEvent(new window.Event('input', { bubbles: true }));
  });
}
async function enter(name = 'Fixture Web A') { await click(document.querySelector(`[aria-label='进入项目「${name}」']`)); }
async function navigate(name) { await click(byText(name, document.querySelector('[aria-label="工作台导航"]'))); }
async function details(name = 'Fixture Web A') {
  const tile = document.querySelector(`[aria-label='进入项目「${name}」']`).closest('article');
  await act(async () => tile.dispatchEvent(new window.MouseEvent('contextmenu', { bubbles: true, cancelable: true })));
  await click(document.querySelector(`[aria-label='查看项目「${name}」的详情']`));
}
async function closeDialog() { await click(dialog().querySelector('[aria-label^="关闭"]')); }

const buildTask = (extra = {}) => task({ taskId: `task_${'a'.repeat(32)}`, projectId: 'prj_feedback_a', type: 'build', status: 'interrupted', final: true, phase: 'build', error: { code: 'BUILD_INTERRUPTED', message: 'Artificial retained interrupted build', details: { processResourceClosed: true } }, ...extra });
const scanTask = () => task({ taskId: `task_${'c'.repeat(32)}`, projectId: 'prj_feedback_a', type: 'storageScan', status: 'succeeded', final: true, phase: 'completed' });
const latestBuild = (build = buildTask(), extra = {}) => ({ task: build, selection: 'recorded-submission', issues: [], ...extra });

test('Reopening restores the persisted interrupted build even when the last task is a successful automatic scan', async () => {
  await setup({ overrides: {
    maintenance_status: () => ({ projectId: web().id, mode: 'ready', recoveryRequired: false, activeTask: null, lastTaskId: scanTask().taskId, documentEpoch: web().documentEpoch, latestBuild: latestBuild() }),
    task_get: () => scanTask(),
  } });
  await details();
  assert.match(dialog().textContent, /构建 · 已中断/); assert.match(dialog().textContent, /BUILD_INTERRUPTED/);
  assert.match(dialog().textContent, new RegExp(buildTask().taskId));
  assert.doesNotMatch(dialog().querySelector('.build-task').textContent, /扫描已完成/);
  await closeDialog(); await click(document.querySelector('[aria-label="刷新项目和运行状态"]')); await details();
  assert.match(dialog().textContent, /Artificial retained interrupted build/);
});

test('A recovered build remains visible without an active or last maintenance task', async () => {
  await setup({ projects: [web()], overrides: {
    maintenance_status: () => ({ mode: 'ready', recoveryRequired: false, activeTask: null, documentEpoch: web().documentEpoch, latestBuild: latestBuild() }),
  } });
  await details(); assert.match(dialog().textContent, /Artificial retained interrupted build/);
});

test('An active build takes precedence over both a retained older result and an automatic scan', async () => {
  const active = buildTask({ taskId: `task_${'b'.repeat(32)}`, status: 'running', final: false, error: null });
  await setup({ projects: [{ ...web(), storage: { activeTask: scanTask() } }], overrides: {
    maintenance_status: () => ({ mode: 'ready', recoveryRequired: false, activeTask: active.taskId, documentEpoch: web().documentEpoch, latestBuild: latestBuild() }),
    task_get: () => active,
  } });
  await details(); assert.match(dialog().textContent, /构建 · 正在执行/);
  assert.match(dialog().querySelector('.build-task').textContent, new RegExp(active.taskId));
  assert.doesNotMatch(dialog().textContent, /Artificial retained interrupted build/);
});

test('Legacy recovery always explains uncertain submission order instead of claiming the latest build', async () => {
  await setup({ projects: [web()], overrides: {
    maintenance_status: () => ({ mode: 'ready', recoveryRequired: false, activeTask: null, documentEpoch: web().documentEpoch, latestBuild: latestBuild(buildTask(), { selection: 'legacy-modified-time' }) }),
  } });
  await details(); assert.match(dialog().textContent, /不代表最新提交顺序/); assert.match(dialog().textContent, /BUILD_INTERRUPTED/);
});

test('A damaged persistent pointer exposes its issue and clears the cached build instead of guessing a different task', async () => {
  let damaged = false;
  await setup({ projects: [web()], overrides: {
    maintenance_status: () => ({ mode: 'ready', recoveryRequired: false, activeTask: null, lastTaskId: scanTask().taskId,
      documentEpoch: web().documentEpoch, latestBuild: damaged ? latestBuild(null, { selection: 'unavailable', issues: [{ code: 'BUILD_RESULT_UNAVAILABLE', message: 'Artificial damaged pointer preserved' }] }) : latestBuild() }),
    task_get: () => scanTask(),
  } });
  await details(); assert.match(dialog().textContent, /Artificial retained interrupted build/); await closeDialog();
  damaged = true; await click(document.querySelector('[aria-label="刷新项目和运行状态"]')); await details();
  assert.match(dialog().textContent, /Artificial damaged pointer preserved/); assert.match(dialog().textContent, /BUILD_RESULT_UNAVAILABLE/);
  assert.doesNotMatch(dialog().textContent, /Artificial retained interrupted build/);
});

test('An unavailable build pointer cannot fall back to a historical build referenced as the last maintenance task', async () => {
  await setup({ projects: [web()], overrides: {
    maintenance_status: () => ({ mode: 'ready', recoveryRequired: false, activeTask: null, lastTaskId: buildTask().taskId,
      documentEpoch: web().documentEpoch, latestBuild: latestBuild(null, { selection: 'unavailable', issues: [{ code: 'BUILD_RESULT_ORDER_UNKNOWN', message: 'Artificial ambiguous historical order' }] }) }),
    task_get: () => buildTask(),
  } });
  await details(); assert.match(dialog().textContent, /Artificial ambiguous historical order/);
  assert.doesNotMatch(dialog().textContent, /Artificial retained interrupted build|BUILD_INTERRUPTED/);
});

test('Foreign, mismatched, malformed and unknown recovered build records are never rendered as project results', async () => {
  for (const recovered of [latestBuild(buildTask({ projectId: 'prj_other' })), latestBuild(buildTask({ type: 'storageScan' })), latestBuild(buildTask({ taskId: '../task' })), latestBuild(buildTask({ final: false })), latestBuild(buildTask({ status: ['interrupted'] })), latestBuild(buildTask(), { selection: 'guess' }), latestBuild(buildTask(), { selection: ['recorded-submission'] }), latestBuild(buildTask(), { issues: ['untyped issue'] }), latestBuild(buildTask(), { selection: 'none' })]) {
    await setup({ projects: [web()], overrides: { maintenance_status: () => ({ mode: 'ready', recoveryRequired: false, activeTask: null, documentEpoch: web().documentEpoch, latestBuild: recovered }) } });
    await details(); assert.match(dialog().textContent, /构建结果身份或格式不符/); assert.doesNotMatch(dialog().textContent, /Artificial retained interrupted build/);
    await unmount(); timers.clear(); timerHistory.clear(); clock = 0;
  }
});

test('A refresh started before a new build cannot replace its accepted result with an older persisted build', async () => {
  const reading = deferred(); let reads = 0;
  const status = () => ({ mode: 'ready', recoveryRequired: false, activeTask: null, lastTaskId: scanTask().taskId, documentEpoch: web().documentEpoch, latestBuild: latestBuild() });
  const accepted = buildTask({ taskId: `task_${'d'.repeat(32)}`, status: 'failed', error: { code: 'NEW_BUILD_FAILURE', message: 'Artificial newer accepted build' } });
  await setup({ projects: [web()], overrides: {
    maintenance_status: () => ++reads === 2 ? reading.promise : status(), task_get: () => scanTask(), project_build: () => accepted,
  } });
  await click(document.querySelector('[aria-label="刷新项目和运行状态"]')); await details();
  await click(byText('构建更新', dialog())); assert.match(dialog().textContent, /Artificial newer accepted build/);
  await act(async () => reading.resolve(status()));
  assert.match(dialog().textContent, /Artificial newer accepted build/); assert.doesNotMatch(dialog().textContent, /Artificial retained interrupted build/);
});

test('A build acceptance from a previous service cannot populate the replacement project result', async () => {
  const accepted = deferred(); let old = true;
  await setup({ projects: [web()], overrides: {
    bootstrap_info: () => ({ ...bootstrap, instanceId: old ? bootstrap.instanceId : 'replacement-before-build-reply' }),
    maintenance_status: () => ({ mode: 'ready', recoveryRequired: false, activeTask: null, documentEpoch: web().documentEpoch, latestBuild: latestBuild(null, { selection: 'none' }) }),
    project_build: () => accepted.promise,
  } });
  await details(); await click(byText('构建更新', dialog())); await closeDialog(); old = false;
  await click(document.querySelector('[aria-label="刷新项目和运行状态"]'));
  await act(async () => accepted.resolve(buildTask())); await details();
  assert.doesNotMatch(dialog().textContent, /Artificial retained interrupted build|BUILD_INTERRUPTED/);
  assert.equal(byText('构建更新', dialog()).disabled, false);
});

for (const changeInstance of [false, true]) {
  test(`A late build poll cannot publish after ${changeInstance ? 'service instance' : 'project root'} replacement`, async () => {
    const reading = deferred(); let old = true; let gets = 0;
    const active = buildTask({ status: 'running', final: false, error: null });
    const runtime = () => ({ ...bootstrap, instanceId: old || !changeInstance ? bootstrap.instanceId : 'replacement-service' });
    const project = () => ({ ...web(), projectRoot: old || changeInstance ? web().projectRoot : '/disposable/replaced-project' });
    await setup({ projects: [web()], overrides: {
      bootstrap_info: runtime, project_list: () => [project()],
      maintenance_status: () => ({ mode: 'ready', recoveryRequired: false, activeTask: old ? active.taskId : null, documentEpoch: web().documentEpoch, latestBuild: old ? latestBuild(active) : latestBuild(null, { selection: 'none' }) }),
      task_get: () => ++gets === 1 ? active : reading.promise,
    } });
    await details(); await advance(1000); await closeDialog(); old = false;
    await click(document.querySelector('[aria-label="刷新项目和运行状态"]'));
    await act(async () => reading.resolve(buildTask())); await details();
    assert.doesNotMatch(dialog().textContent, /Artificial retained interrupted build|BUILD_INTERRUPTED/);
  });
}

test('A service switch during one recovery read invalidates the result before it reaches the details', async () => {
  const reading = deferred(); let reads = 0; let switched = false;
  const ready = () => ({ mode: 'ready', recoveryRequired: false, activeTask: null, documentEpoch: web().documentEpoch, latestBuild: latestBuild() });
  await setup({ projects: [web()], overrides: {
    bootstrap_info: () => ({ ...bootstrap, instanceId: switched ? 'replacement-during-read' : bootstrap.instanceId }),
    maintenance_status: () => ++reads === 2 ? reading.promise : ready(),
  } });
  await click(document.querySelector('[aria-label="刷新项目和运行状态"]')); switched = true;
  await act(async () => reading.resolve(ready()));
  assert.match(document.body.textContent, /读取构建结果时工作区服务已切换/);
  await details();
  assert.doesNotMatch(dialog().textContent, /Artificial retained interrupted build|BUILD_INTERRUPTED/);
});

for (const activeReference of [true, false]) {
  test(`A later task_get terminal result supersedes the same build in the earlier ${activeReference ? 'active' : 'last-task'} snapshot and retains its issues`, async () => {
    const running = buildTask({ status: 'running', final: false, error: null });
    await setup({ projects: [web()], overrides: {
      maintenance_status: () => ({ projectId: web().id, documentEpoch: web().documentEpoch, mode: 'ready', recoveryRequired: false,
        activeTask: activeReference ? running.taskId : null, lastTaskId: activeReference ? null : running.taskId,
        latestBuild: latestBuild(running, { issues: [{ code: 'BUILD_RESULT_LEGACY_ORDER', message: 'Artificial retained selection uncertainty' }] }) }),
      task_get: () => buildTask(),
    } });
    await details();
    assert.match(dialog().querySelector('.build-task').textContent, /构建 · 已中断/);
    assert.match(dialog().textContent, /BUILD_INTERRUPTED/); assert.match(dialog().textContent, /Artificial retained selection uncertainty/);
    assert.doesNotMatch(dialog().querySelector('.build-task').textContent, /构建 · 正在执行/);
  });
}

test('New recovery snapshots require a present string epoch matching the same project-list batch', async () => {
  for (const epoch of [undefined, null, 17, '', '22222222-2222-4222-8222-222222222222']) {
    const calls = await setup({ projects: [web()], overrides: {
      maintenance_status: () => ({ projectId: web().id, ...(epoch === undefined ? {} : { documentEpoch: epoch }), mode: 'ready', recoveryRequired: false,
        activeTask: buildTask().taskId, latestBuild: latestBuild() }),
      task_get: () => buildTask(),
    } });
    await details();
    assert.match(dialog().textContent, /项目数据代次已变化或无法核对/);
    assert.doesNotMatch(dialog().textContent, /Artificial retained interrupted build|BUILD_INTERRUPTED/);
    assert.equal(byText('构建更新', dialog()).disabled, true);
    assert.equal(calls.filter(([command]) => command === 'task_get').length, 0);
    await unmount(); timers.clear(); timerHistory.clear(); clock = 0;
  }
});

test('A changed maintenance epoch invalidates a previously cached build and keeps a readable recheck issue', async () => {
  let epoch = web().documentEpoch;
  await setup({ projects: [web()], overrides: {
    maintenance_status: () => ({ projectId: web().id, documentEpoch: epoch, mode: 'ready', recoveryRequired: false, activeTask: null, latestBuild: latestBuild() }),
  } });
  await details(); assert.match(dialog().textContent, /Artificial retained interrupted build/); await closeDialog();
  epoch = '22222222-2222-4222-8222-222222222222';
  await click(document.querySelector('[aria-label="刷新项目和运行状态"]')); await details();
  assert.match(dialog().textContent, /项目数据代次已变化或无法核对/);
  assert.doesNotMatch(dialog().textContent, /Artificial retained interrupted build|BUILD_INTERRUPTED/);
});

test('An old snapshot without the latestBuild field keeps its earlier epoch-free command compatibility', async () => {
  await setup({ projects: [web()], overrides: {
    maintenance_status: () => ({ mode: 'ready', recoveryRequired: false, activeTask: buildTask().taskId }),
    task_get: () => buildTask(),
  } });
  await details(); assert.match(dialog().textContent, /Artificial retained interrupted build/);
  assert.doesNotMatch(dialog().textContent, /项目数据代次已变化或无法核对/);
});

test('An active project operation keeps display precedence over recovered build history', async () => {
  const active = buildTask({ type: 'export', status: 'running', final: false, error: null });
  await setup({ projects: [web()], overrides: {
    maintenance_status: () => ({ projectId: web().id, documentEpoch: web().documentEpoch, mode: 'ready', recoveryRequired: false,
      activeTask: active.taskId, latestBuild: latestBuild() }),
    task_get: () => active,
  } });
  await details(); assert.match(dialog().querySelector('.build-task').textContent, /导出项目 · 正在执行/);
  assert.doesNotMatch(dialog().querySelector('.build-task').textContent, /Artificial retained interrupted build/);
});

test('A new epoch seen by build preflight blocks submission from the older project view', async () => {
  let reads = 0;
  const calls = await setup({ projects: [web()], overrides: {
    maintenance_status: () => ({ projectId: web().id, documentEpoch: ++reads === 1 ? web().documentEpoch : '22222222-2222-4222-8222-222222222222',
      mode: 'ready', recoveryRequired: false, activeTask: null, latestBuild: latestBuild() }),
    project_build: () => assert.fail('a stale project view submitted a build'),
  } });
  await details(); await click(byText('构建更新', dialog()));
  assert.match(dialog().textContent, /项目数据代次已变化或无法核对/);
  assert.equal(calls.filter(([command]) => command === 'project_build').length, 0);
});

test('An ordinary global success expires after five controlled seconds', async () => {
  await setup(); await enter();
  assert.match(success().textContent, /Fixture Web A/);
  await advance(4999); assert.ok(success());
  await advance(1); assert.equal(success(), null);
});

test('Creating a project carries its success into the new overview for five seconds', async () => {
  const projects = [web()];
  const calls = await setup({ projects, overrides: { project_create: ({ input }) => {
    const created = web('prj_feedback_created', input.name);
    projects.push(created);
    return created;
  } } });
  await click(byText('新建项目'));
  await change(document.getElementById('project-name'), 'Fixture Created Web');
  await click(byText('创建项目', dialog()));
  assert.equal(calls.filter(([command]) => command === 'project_create').length, 1);
  assert.match(dialog().textContent, /Fixture Created Web/);
  assert.doesNotMatch(dialog().textContent, /项目已不在当前工作区列表中/);
  assert.match(success().textContent, /已创建「Fixture Created Web」/);
  assert.ok(dialog().contains(success()));
  await advance(4999); assert.ok(success());
  await advance(1); assert.equal(success(), null);
});

test('A stale success timer cannot dismiss a newer success', async () => {
  await setup(); await enter();
  const first = [...timerHistory.values()].find(timer => !timer.interval && timer.delay === 5000);
  assert.ok(first, 'A normal success needs a five-second timeout');
  await advance(1000); await enter('Fixture Web B');
  await act(async () => first.callback(...first.args));
  assert.match(success().textContent, /Fixture Web B/);
  await advance(4999); assert.ok(success());
  await advance(1); assert.equal(success(), null);
});

test('A global error survives five seconds, refresh and a later success until its own X is clicked', async () => {
  await setup({ overrides: { project_open: ({ projectId }) => {
    if (projectId === 'prj_feedback_a') throw new Error('Artificial sticky open failure');
    return { projectId, target: 'desktop', reused: false };
  } } });
  await enter(); assert.match(failure().textContent, /Artificial sticky open failure/);
  await advance(10000); assert.ok(failure());
  await click(document.querySelector('[aria-label="刷新项目和运行状态"]')); assert.ok(failure());
  await enter('Fixture Web B'); assert.ok(success()); assert.ok(failure());
  await advance(5000); assert.equal(success(), null); assert.ok(failure());
  await click(document.querySelector('[aria-label="关闭错误提示"]')); assert.equal(failure(), null);
});

test('Dismissing a polled error does not let the next identical poll immediately resurrect it', async () => {
  let reads = 0;
  await setup({ projects: [], workspace: { workspaceRoot: bootstrap.workspaceRoot, migrationState: 'migrating', activeTask: { taskId: 'task_feedback_poll' }, retainedRoots: [] }, overrides: {
    task_get: () => { if (++reads === 1) return task({ taskId: 'task_feedback_poll' }); throw new Error('Artificial repeated poll outage'); },
  } });
  await advance(1000); assert.match(failure().textContent, /Artificial repeated poll outage/);
  await click(document.querySelector('[aria-label="关闭错误提示"]'));
  await advance(1000); assert.equal(failure(), null);
});

test('Leaving a management page discards its old success and error instead of showing them on return', async () => {
  await setup({ overrides: { project_open: ({ projectId }) => { if (projectId === 'prj_feedback_a') throw new Error('Artificial old-page error'); return { projectId, target: 'desktop', reused: false }; } } });
  await enter(); await enter('Fixture Web B'); assert.ok(failure()); assert.ok(success());
  await navigate('空间'); assert.equal(failure(), null); assert.equal(success(), null);
  await navigate('我的项目'); assert.equal(failure(), null); assert.equal(success(), null);
});

test('Opening a modal discards the list page feedback instead of moving it into that modal', async () => {
  await setup({ overrides: { project_open: ({ projectId }) => {
    if (projectId === 'prj_feedback_a') throw new Error('Artificial list-only error');
    return { projectId, target: 'desktop', reused: false };
  } } });
  await enter(); await enter('Fixture Web B'); assert.ok(failure()); assert.ok(success());
  await click(byText('新建项目'));
  assert.ok(dialog()); assert.equal(failure(), null); assert.equal(success(), null);
  assert.doesNotMatch(dialog().textContent, /Artificial list-only error/);
  await closeDialog(); assert.equal(failure(), null); assert.equal(success(), null);
});

test('A rejected build submission stays beside the overview build result and is discarded on close', async () => {
  const calls = await setup({ overrides: { project_build: () => { throw new Error('Artificial build submission failure'); } } });
  await details(); await click(byText('构建更新', dialog()));
  assert.equal(calls.filter(([command]) => command === 'project_build').length, 1);
  const error = failure();
  assert.match(error.textContent, /Artificial build submission failure/);
  assert.ok(dialog().contains(error));
  assert.equal(document.querySelector('.radio-notices').querySelector('.notice-error'), null);
  assert.equal(dialog().querySelector('.detail-actions').contains(error), false);
  assert.equal(dialog().querySelector('.detail-actions').compareDocumentPosition(error) & window.Node.DOCUMENT_POSITION_FOLLOWING, window.Node.DOCUMENT_POSITION_FOLLOWING);
  await advance(5000); assert.ok(failure());
  await closeDialog(); assert.equal(failure(), null);
  await details(); assert.equal(failure(), null);
});

test('A build submission rejected after closing its overview cannot become a list page error', async () => {
  const building = deferred();
  await setup({ overrides: { project_build: () => building.promise } });
  await details(); await click(byText('构建更新', dialog())); await closeDialog();
  await act(async () => building.reject(new Error('Artificial late-build submission failure')));
  assert.equal(failure(), null); assert.doesNotMatch(document.body.textContent, /Artificial late-build submission failure/);
  await details(); assert.equal(failure(), null);
});

test('Closing a Web overview discards the error and success owned by that overview', async () => {
  let opens = 0;
  await setup({ overrides: { project_open: ({ projectId }) => { if (++opens === 1) throw new Error('Artificial old-overview error'); return { projectId, target: 'desktop', reused: false }; } } });
  await details(); await click(byText('进入应用', dialog())); await click(byText('进入应用', dialog()));
  assert.ok(failure()); assert.ok(success());
  await closeDialog(); assert.equal(failure(), null); assert.equal(success(), null);
  await details(); assert.equal(failure(), null); assert.equal(success(), null);
});

test('A late Web-open success or error does not flow into another page or return later', async () => {
  for (const rejected of [false, true]) {
    const opening = deferred();
    await setup({ overrides: { project_open: () => opening.promise } });
    await enter(); await navigate('空间');
    await act(async () => rejected ? opening.reject(new Error('Artificial late-open error')) : opening.resolve({ projectId: 'prj_feedback_a', target: 'desktop', reused: false }));
    assert.equal(failure(), null); assert.equal(success(), null);
    await navigate('我的项目'); assert.equal(failure(), null); assert.equal(success(), null);
    await unmount(); timers.clear(); timerHistory.clear(); clock = 0;
  }
});

test('Returning to the same page before an old Web-open settles does not restore its expired context', async () => {
  for (const rejected of [false, true]) {
    const opening = deferred();
    await setup({ overrides: { project_open: () => opening.promise } });
    await enter(); await navigate('空间'); await navigate('我的项目');
    await act(async () => rejected ? opening.reject(new Error('Artificial returned-page late-open error')) : opening.resolve({ projectId: 'prj_feedback_a', target: 'desktop', reused: false }));
    assert.equal(failure(), null); assert.equal(success(), null);
    await unmount(); timers.clear(); timerHistory.clear(); clock = 0;
  }
});

test('A workspace or service identity switch discards old messages and rejects late Web-open feedback on the same page', async () => {
  for (const switchedInstance of [false, true]) for (const rejected of [false, true]) {
    let runtime = bootstrap; let opens = 0;
    const opening = deferred();
    await setup({ overrides: {
      bootstrap_info: () => runtime,
      workspace_info: () => ({ workspaceRoot: runtime.workspaceRoot, migrationState: 'ready', activeTask: null, retainedRoots: [] }),
      project_list: () => [web(), web('prj_feedback_b', 'Fixture Web B')].map(project => ({ ...project, projectRoot: `${runtime.workspaceRoot}/projects/${project.id}` })),
      project_open: ({ projectId }) => {
        if (++opens === 1) throw new Error('Artificial old-service error');
        if (opens === 2) return { projectId, target: 'desktop', reused: false };
        return opening.promise;
      },
    } });
    await enter(); await enter('Fixture Web B'); assert.ok(failure()); assert.ok(success());
    await enter();
    runtime = { ...bootstrap, instanceId: switchedInstance ? 'feedback-replacement-instance' : bootstrap.instanceId, workspaceRoot: '/disposable/feedback/replacement-workspace' };
    await click(document.querySelector('[aria-label="刷新项目和运行状态"]'));
    assert.doesNotMatch(document.body.textContent, /Artificial old-service error/); assert.equal(success(), null);
    await act(async () => rejected ? opening.reject(new Error('Artificial old-service late-open error')) : opening.resolve({ projectId: 'prj_feedback_a', target: 'desktop', reused: false }));
    assert.doesNotMatch(document.body.textContent, /Artificial old-service late-open error/); assert.equal(success(), null);
    await unmount(); timers.clear(); timerHistory.clear(); clock = 0;
  }
});

test('A late clipboard result after closing its overview never creates a new global message', async () => {
  const writing = deferred();
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: () => writing.promise } });
  await setup(); await details(); await click(byText('复制提示词', dialog()));
  await closeDialog(); await act(async () => writing.resolve());
  assert.equal(success(), null); assert.equal(failure(), null);
  await details(); assert.equal(success(), null); assert.equal(failure(), null);
});

test('Node and local-directory errors remain in their original panels and never become global feedback', async () => {
  await setup({ nodes: [node], overrides: {
    node_app_start: () => { throw new Error('Artificial local Node error'); },
    choose_path: () => '/disposable/feedback/review-folder',
    node_app_directory_inspect: () => { throw new Error('Artificial local directory error'); },
  } });
  await details(node.name); await click(byText('进入项目', dialog()));
  assert.match(dialog().textContent, /Artificial local Node error/); assert.equal(failure(), null);
  await closeDialog(); assert.equal(failure(), null);
  await click(byText('添加本地应用'));
  await click(byText('选择本地项目文件夹'));
  assert.match(dialog().textContent, /Artificial local directory error/); assert.equal(failure(), null); assert.equal(success(), null);
  await closeDialog(); assert.equal(failure(), null); assert.equal(success(), null);
});

test('Local Node and directory success messages remain local instead of entering the global success timer', async () => {
  const runningNode = { ...node, state: 'running' };
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { async writeText() {} } });
  await setup({ nodes: [runningNode], overrides: {
    node_app_stop: () => ({ ...node, state: 'stopped' }),
    choose_path: () => '/disposable/feedback/review-folder',
    node_app_directory_inspect: () => ({ sourcePath: '/disposable/feedback/review-folder', state: 'needs-review', expectedRevision: 'feedback-directory-revision', issues: [], reviewPrompt: 'Disposable complete local review prompt' }),
  } });
  await details(node.name); await click(byText('运行状态', dialog())); await click(byText('关闭应用', dialog()));
  assert.match(dialog().textContent, /应用已关闭，相关服务已停止/); assert.equal(success(), null);
  await advance(6000); assert.match(dialog().textContent, /应用已关闭，相关服务已停止/); assert.equal(success(), null);
  await closeDialog();
  await click(byText('添加本地应用')); await click(byText('选择本地项目文件夹')); await click(byText('复制给 AI 检查'));
  assert.match(dialog().textContent, /完整检查说明已复制/); assert.equal(success(), null);
  await closeDialog(); assert.equal(success(), null);
});

for (const type of ['workspaceMigration', 'workspaceCleanup']) {
  test(`${type} feedback follows task.final even while the service snapshot retains an old active task`, async () => {
    let finished = false;
    const running = task({ type });
    const workspace = { workspaceRoot: bootstrap.workspaceRoot, migrationState: type === 'workspaceMigration' ? 'migrating' : 'ready', activeTask: type === 'workspaceMigration' ? { taskId: running.taskId } : null, cleanupTask: type === 'workspaceCleanup' ? running : null, retainedRoots: [] };
    await setup({ workspace, overrides: { task_get: () => finished ? { ...running, status: 'succeeded', final: true, phase: 'completed', result: { completed: true } } : running } });
    const phrase = type === 'workspaceMigration' ? /工作区正在迁移，新操作暂时暂停/ : /历史清理正在进行，管理写操作暂时停用/;
    assert.match(document.querySelector('.radio-notices').textContent, phrase);
    await advance(10000); assert.match(document.querySelector('.radio-notices').textContent, phrase);
    finished = true; await advance(1000);
    assert.doesNotMatch(document.querySelector('.radio-notices').textContent, phrase);
  });
}

const dataRecoveryProject = (state = 'pending') => ({ ...web(), state: 'error', issues: ['Artificial data recovery pending'],
  buildInfo: { sourceState: 'unknown', stale: true, dataRecoveryRequired: true, dataRecoveryState: state } });
const dataRecoveryMaintenance = () => ({ mode: 'ready', recoveryRequired: false, documentEpoch: null, activeTask: buildTask().taskId,
  lastTaskId: scanTask().taskId, latestBuild: latestBuild(buildTask(), { issues: [{ code: 'DO_NOT_READ_OLD_SCOPE', message: 'Artificial unverified old build' }] }) });

test('Expected pending or blocked data recovery keeps diagnostic maintenance without consuming tasks or build results', async () => {
  for (const state of ['pending', 'blocked']) {
    const calls = await setup({ projects: [dataRecoveryProject(state)], overrides: { maintenance_status: dataRecoveryMaintenance,
      task_get: () => assert.fail('diagnostic null epoch must not read a task') } });
    await details();
    assert.doesNotMatch(document.body.textContent, /维护状态读取失败|项目数据代次已变化或无法核对|Artificial retained interrupted build|Artificial unverified old build/);
    assert.match(dialog().textContent, /数据取回尚未处理完成/);
    assert.equal(byText('构建更新', dialog()).disabled, true);
    assert.equal(calls.filter(([command]) => command === 'task_get').length, 0);
    await unmount(); timers.clear(); timerHistory.clear(); clock = 0;
  }
});

test('Entering data recovery clears earlier same-scope build and scan display then reloads a verified new epoch', async () => {
  let recovery = false;
  let completed = false;
  const current = () => completed ? { ...web(), documentEpoch: '22222222-2222-4222-8222-222222222222' } : recovery ? dataRecoveryProject() : web();
  const calls = await setup({ projects: [web()], overrides: {
    project_list: () => [current()],
    maintenance_status: () => recovery ? dataRecoveryMaintenance() : ({ mode: 'ready', recoveryRequired: false, documentEpoch: current().documentEpoch,
      activeTask: null, lastTaskId: completed ? null : scanTask().taskId, latestBuild: latestBuild(completed ? buildTask({ error: { code: 'NEW_VERIFIED_EPOCH', message: 'Artificial verified later build' } }) : buildTask()) }),
    task_get: () => scanTask(),
  } });
  await details(); assert.match(dialog().textContent, /Artificial retained interrupted build/); await closeDialog();
  recovery = true;
  await click(document.querySelector('[aria-label="刷新项目和运行状态"]')); await details();
  assert.doesNotMatch(dialog().textContent, /Artificial retained interrupted build|Artificial unverified old build|BUILD_INTERRUPTED|空间扫描 · 已完成|维护状态读取失败/);
  assert.match(dialog().textContent, /数据取回尚未处理完成/);
  assert.equal(calls.filter(([command]) => command === 'task_get').length, 1);
  await closeDialog(); recovery = false; completed = true;
  await click(document.querySelector('[aria-label="刷新项目和运行状态"]')); await details();
  assert.match(dialog().textContent, /Artificial verified later build/);
  assert.doesNotMatch(dialog().textContent, /维护状态读取失败|项目数据代次已变化或无法核对/);
});

test('Only the complete explicit data recovery diagnostic permits a null epoch and malformed epochs remain rejected', async () => {
  const bad = [web(), { ...dataRecoveryProject(), state: 'ready' }, { ...dataRecoveryProject(), documentEpoch: '' },
    { ...dataRecoveryProject(), buildInfo: { sourceState: 'unknown', dataRecoveryRequired: false, dataRecoveryState: 'pending' } }, dataRecoveryProject('clear')];
  const cases = [...bad.map(project => [project, null]), ...[undefined, 17, '', '22222222-2222-4222-8222-222222222222'].map(epoch => [dataRecoveryProject(), epoch])];
  for (const [project, epoch] of cases) {
    const calls = await setup({ projects: [project], overrides: {
      maintenance_status: () => { const status = dataRecoveryMaintenance(); if (epoch === undefined) delete status.documentEpoch; else status.documentEpoch = epoch; return status; },
      task_get: () => assert.fail('unmatched epoch must not read tasks'),
    } });
    await details(); assert.match(dialog().textContent, /项目数据代次已变化或无法核对/);
    assert.equal(calls.filter(([command]) => command === 'task_get').length, 0);
    await unmount(); timers.clear(); timerHistory.clear(); clock = 0;
  }
});

test('Build preflight continues to reject a null epoch before submitting any build', async () => {
  let reads = 0;
  const calls = await setup({ projects: [web()], overrides: {
    maintenance_status: () => ({ mode: 'ready', recoveryRequired: false, documentEpoch: ++reads === 1 ? web().documentEpoch : null,
      activeTask: null, latestBuild: latestBuild() }),
    project_build: () => assert.fail('null epoch must not submit a build'),
  } });
  await details(); await click(byText('构建更新', dialog()));
  assert.match(dialog().textContent, /项目数据代次已变化或无法核对/);
  assert.equal(calls.filter(([command]) => command === 'project_build').length, 0);
});

test('A late old build poll cannot repopulate while a pending diagnostic maintenance read waits', async () => {
  const poll = deferred(); const diagnostic = deferred(); let pending = false; let gets = 0;
  const active = buildTask({ status: 'running', final: false, phase: 'artificial-old-build-running', error: null });
  const calls = await setup({ projects: [web()], overrides: {
    project_list: () => [pending ? dataRecoveryProject() : web()],
    maintenance_status: () => pending ? diagnostic.promise : ({ mode: 'ready', recoveryRequired: false, documentEpoch: web().documentEpoch, activeTask: active.taskId, latestBuild: latestBuild(active) }),
    task_get: () => ++gets === 1 ? active : poll.promise,
  } });
  await advance(1000); assert.equal(gets, 2);
  pending = true; await click(document.querySelector('[aria-label="刷新项目和运行状态"]'));
  await act(async () => poll.resolve(active));
  await act(async () => diagnostic.resolve(dataRecoveryMaintenance()));
  await details();
  assert.doesNotMatch(dialog().textContent, /artificial-old-build-running|维护状态读取失败/);
  assert.match(dialog().textContent, /数据取回尚未处理完成/);
  const count = calls.filter(([command]) => command === 'task_get').length;
  await advance(2000);
  assert.equal(calls.filter(([command]) => command === 'task_get').length, count, 'old build no longer polls');
  await click([...dialog().querySelectorAll('[role="tab"]')].find(button => button.textContent.includes('更多操作')));
  assert.equal(byText('导出 ZIP', dialog()).disabled, true);
  assert.equal(byText('删除应用', dialog()).disabled, true);
});

for (const operation of ['gitDataRestoreContinue', 'gitDataRestoreUndo']) {
test(`Diagnostic refresh keeps an already accepted ${operation} task visible and polling`, async () => {
  let pending = false; let gets = 0;
  const recovery = buildTask({ type: operation, status: 'running', final: false, phase: 'artificial-real-data-recovery-running', error: null });
  await setup({ projects: [web()], overrides: {
    project_list: () => [pending ? dataRecoveryProject() : web()],
    maintenance_status: () => pending ? dataRecoveryMaintenance() : ({ mode: 'ready', recoveryRequired: false, documentEpoch: web().documentEpoch,
      activeTask: recovery.taskId, latestBuild: latestBuild(null, { selection: 'none' }) }),
    task_get: () => { gets++; return recovery; },
  } });
  assert.equal(gets, 1);
  pending = true; await click(document.querySelector('[aria-label="刷新项目和运行状态"]')); await details();
  assert.match(dialog().textContent, /artificial-real-data-recovery-running/);
  const beforePoll = gets;
  await advance(1000);
  assert.ok(gets > beforePoll, 'accepted recovery task still polls');
  assert.match(dialog().textContent, /artificial-real-data-recovery-running/);
  assert.equal(byText('构建更新', dialog()).disabled, true);
});
}
