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
  await navigate('空间与工作区'); assert.equal(failure(), null); assert.equal(success(), null);
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
    await enter(); await navigate('空间与工作区');
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
    await enter(); await navigate('空间与工作区'); await navigate('我的项目');
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
  await click(document.querySelector('.radio-add-menu summary')); await click(byText('添加本地应用'));
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
  await click(document.querySelector('.radio-add-menu summary')); await click(byText('添加本地应用')); await click(byText('选择本地项目文件夹')); await click(byText('复制给 AI 检查'));
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
