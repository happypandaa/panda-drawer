// Management-shell regressions use disposable DOM fixtures and mocked native
// commands only. They never open the real development workspace or a service.
import assert from 'node:assert/strict';
import { after, afterEach, test } from 'node:test';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';
import { JSDOM } from 'jsdom';

const repo = fileURLToPath(new URL('../../../', import.meta.url));
await mkdir(path.join(repo, '.test-artifacts'), { recursive: true });
const output = await mkdtemp(path.join(repo, '.test-artifacts/radio-shell-dom-'));
const mockPath = path.join(output, 'tauri-mock.mjs');
await writeFile(mockPath, 'export const invoke = (...args) => globalThis.__RADIO_SHELL_INVOKE__(...args); export const isTauri = () => true;\n');
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
for (const name of ['window', 'document', 'Node', 'HTMLElement', 'HTMLInputElement', 'HTMLSelectElement', 'Event', 'MouseEvent', 'KeyboardEvent', 'FocusEvent', 'MutationObserver']) Object.defineProperty(globalThis, name, { value: name === 'window' ? dom.window : dom.window[name], configurable: true });
Object.defineProperty(globalThis, 'navigator', { value: dom.window.navigator, configurable: true });
Object.defineProperty(globalThis, 'localStorage', { value: dom.window.localStorage, configurable: true });
globalThis.getComputedStyle = dom.window.getComputedStyle.bind(dom.window);
dom.window.HTMLElement.prototype.scrollIntoView = () => {};
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
globalThis.__RADIO_SHELL_INVOKE__ = async command => { throw new Error(`Unexpected fixture command: ${command}`); };
const { createElement: h, act } = await import('react');
const { createRoot } = await import('react-dom/client');
const { default: App } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/App.tsx'))).href);
const { APPEARANCE_KEY } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/appearance.ts'))).href);
const storagePrototype = dom.window.Storage.prototype;
const originalGetItem = Object.getOwnPropertyDescriptor(storagePrototype, 'getItem');
const originalSetItem = Object.getOwnPropertyDescriptor(storagePrototype, 'setItem');
let root;
async function unmount() { if (root) await act(async () => root.unmount()); root = null; }
afterEach(async () => {
  await unmount();
  Object.defineProperty(storagePrototype, 'getItem', originalGetItem);
  Object.defineProperty(storagePrototype, 'setItem', originalSetItem);
  dom.window.localStorage.clear();
  delete document.documentElement.dataset.theme;
  globalThis.__RADIO_SHELL_INVOKE__ = async command => { throw new Error(`Unexpected fixture command: ${command}`); };
});
after(async () => { dom.window.close(); await rm(output, { recursive: true, force: true }); });

const bootstrap = {
  displayName: 'Disposable Panda Drawer fixture', identifier: 'dev.fixture.radio-shell', environment: 'development', apiVersion: 1,
  instanceId: 'instance_radio_shell_fixture', baseUrl: 'http://127.0.0.1:1', workspaceRoot: '/disposable/radio-shell/workspace', dataRoot: '/disposable/radio-shell/data', sqliteVersion: 'fixture', version: '0.0.0-fixture',
  resources: { sdk: true, template: true, readme: true, cli: true }, cliPath: '/disposable/radio-shell/cli',
};
const environment = { ready: true, node: { path: '/disposable/node', version: 'v24.0.0' }, npm: { path: '/disposable/npm', version: '11.0.0' }, selection: 'detected', configuredNodePath: null, supportedNodeRange: 'fixture', recommendedNode: 'fixture', downloadUrl: 'https://example.invalid/node', issues: [] };
const nodeFixture = { id: 'node_radio_shell_fixture', name: 'Disposable shell Node', sourcePath: '/disposable/radio-shell/source', runtime: 'node-service', state: 'stopped', restartRequired: false, runningRevision: null, lastError: null, configuration: { revision: 1, values: {}, secretStatus: {}, missingRequired: ['KEY'], canStart: false }, fields: [] };
function deferred() { let resolve; let reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { resolve, reject, promise }; }
async function setup({ projects = [], nodes = [], overrides = {} } = {}) {
  const calls = [];
  globalThis.__RADIO_SHELL_INVOKE__ = async (command, args) => {
    calls.push([command, args]);
    if (overrides[command]) return overrides[command](args);
    if (command === 'bootstrap_info') return bootstrap;
    if (command === 'project_list') return projects;
    if (command === 'node_app_list') return nodes;
    if (command === 'workspace_info') return { workspaceRoot: bootstrap.workspaceRoot, migrationState: 'ready', activeTask: null, retainedRoots: [] };
    if (command === 'maintenance_status') return { mode: 'ready', recoveryRequired: false, activeTask: null };
    if (command === 'environment_doctor') return environment;
    if (command === 'workspace_storage') return { platformProgram: { totalBytes: 0, complete: true }, workspace: { totalBytes: 0, complete: true, categories: {} }, otherLocalLocations: [] };
    throw new Error(`Unexpected fixture command: ${command}`);
  };
  document.body.innerHTML = '<div id="root"></div>';
  root = createRoot(document.getElementById('root'));
  await act(async () => root.render(h(App)));
  return calls;
}
const byText = (text, scope = document, selector = 'button') => [...scope.querySelectorAll(selector)].find(element => element.textContent === text);
const dialog = () => document.querySelector('[role="dialog"]');
const addDisclosure = () => document.querySelector('.radio-add-menu');
const addTrigger = () => addDisclosure().querySelector('summary');
async function click(element) { assert.ok(element); await act(async () => element.click()); }
async function key(element, value, extra = {}) { assert.ok(element); await act(async () => element.dispatchEvent(new window.KeyboardEvent('keydown', { key: value, bubbles: true, cancelable: true, ...extra }))); }
async function change(element, value) {
  assert.ok(element);
  await act(async () => {
    Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set.call(element, value);
    element.dispatchEvent(new window.Event('input', { bubbles: true }));
  });
}

test('The real shell has two navigation choices, with runtime and about in a single settings portal', async () => {
  const calls = await setup();
  const navigation = document.querySelector('[aria-label="工作台导航"]');
  assert.deepEqual([...navigation.querySelectorAll('button')].map(element => element.textContent), ['我的项目', '空间与工作区']);
  assert.equal(navigation.querySelector('[aria-current="page"]').textContent, '我的项目');
  assert.equal(document.querySelector('.build-environment'), null);
  assert.equal(document.querySelector('.about-platform'), null);
  await click(byText('空间与工作区', navigation));
  assert.equal(navigation.querySelector('[aria-current="page"]').textContent, '空间与工作区');
  assert.ok(document.querySelector('.workspace-hub'));
  assert.equal(document.querySelector('.radio-toolbar'), null);
  await click(document.querySelector('[aria-label="设置"]'));
  assert.equal(document.querySelectorAll('[role="dialog"]').length, 1);
  assert.equal(dialog().querySelector('.modal-header h2').textContent, '设置');
  assert.equal(dialog().closest('.radio-shell'), null);
  assert.equal(document.querySelector('.modal-layer').parentElement, document.body);
  assert.equal(document.querySelector('#root').hasAttribute('inert'), true);
  assert.ok(dialog().querySelector('[aria-labelledby="settings-runtime-title"]'));
  assert.ok(dialog().querySelector('[aria-labelledby="settings-about-title"]'));
  assert.match(dialog().textContent, /运行环境/);
  assert.match(dialog().textContent, /关于/);
  assert.match(dialog().textContent, /0\.0\.0-fixture/);
  await click(byText('完成', dialog()));
  assert.equal(dialog(), null);
  assert.equal(navigation.querySelector('[aria-current="page"]').textContent, '空间与工作区');
  await click(byText('我的项目', navigation));
  assert.ok(document.querySelector('.radio-toolbar'));
  assert.equal(calls.some(([command]) => /project_create|workspace_migrate|environment_node_set/.test(command)), false);
});

test('Light and dark choices persist and apply to html so body-level settings portals share the theme', async () => {
  window.localStorage.setItem(APPEARANCE_KEY, 'dark');
  await setup();
  assert.equal(document.documentElement.dataset.theme, 'dark');
  assert.equal(document.querySelector('.radio-shell').dataset.theme, 'dark');
  assert.equal(document.querySelector('[aria-label="切换暗色主题"]').getAttribute('aria-pressed'), 'true');
  await click(document.querySelector('[aria-label="切换亮色主题"]'));
  assert.equal(document.documentElement.dataset.theme, 'light');
  assert.equal(window.localStorage.getItem(APPEARANCE_KEY), 'light');
  await unmount();
  await setup();
  assert.equal(document.documentElement.dataset.theme, 'light');
  await click(document.querySelector('[aria-label="切换暗色主题"]'));
  assert.equal(window.localStorage.getItem(APPEARANCE_KEY), 'dark');
  await click(document.querySelector('[aria-label="设置"]'));
  assert.equal(dialog().closest('.radio-shell'), null);
  assert.equal(document.documentElement.dataset.theme, 'dark');
  await click(document.querySelector('[aria-label="关闭设置"]'));
  await unmount();
  await setup();
  assert.equal(document.documentElement.dataset.theme, 'dark');
});

test('Launcher items expose only their icon and name, with contextual actions and normal opening preserved', async () => {
  const project = { id: 'prj_disposable_compact_launcher', name: 'Disposable long launcher name', description: 'Artificial project description', projectRoot: '/disposable/radio-shell/project', documentEpoch: 'fixture', state: 'ready', issues: [], manifest: null };
  const calls = await setup({ projects: [project], nodes: [nodeFixture], overrides: { project_open: () => ({ projectId: project.id, target: 'desktop', reused: false }) } });
  const entry = document.querySelector(`[aria-label='进入项目「${project.name}」']`);
  const tile = entry.closest('article');
  assert.equal(document.querySelector('.launcher-more'), null);
  assert.equal(document.querySelector('.launcher-statuses'), null);
  assert.equal(entry.children.length, 2);
  assert.ok(entry.children[0].classList.contains('launcher-icon'));
  assert.ok(entry.children[1].classList.contains('launcher-name'));
  assert.equal(tile.querySelectorAll('button').length, 1);
  assert.equal(tile.querySelector('.launcher-name').title, project.name);
  const nodeEntry = document.querySelector(`[aria-label='进入项目「${nodeFixture.name}」']`);
  assert.ok(nodeEntry.querySelector('.launcher-node-badge'));
  assert.match(nodeEntry.title, /需要配置/);
  assert.equal(nodeEntry.disabled, false);
  await act(async () => entry.focus());
  await key(entry, 'F10', { shiftKey: true });
  const menu = document.querySelector('[role="menu"]');
  assert.ok(menu);
  assert.equal(document.activeElement.getAttribute('aria-label'), `查看项目「${project.name}」的详情`);
  await key(document.activeElement, 'Escape');
  assert.equal(document.querySelector('[role="menu"]'), null);
  assert.equal(document.activeElement, entry);
  await act(async () => tile.dispatchEvent(new window.MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 120, clientY: 140 })));
  assert.ok(document.querySelector('[role="menu"]'));
  await key(document.activeElement, 'Escape');
  await click(entry);
  assert.deepEqual(calls.filter(([command]) => command === 'project_open').map(([, args]) => args), [{ projectId: project.id, target: 'desktop', reload: false }]);
});

test('Unavailable browser storage falls back to light and still permits both theme switches', async () => {
  Object.defineProperty(storagePrototype, 'getItem', { configurable: true, value() { throw new Error('Artificial storage read failure'); } });
  Object.defineProperty(storagePrototype, 'setItem', { configurable: true, value() { throw new Error('Artificial storage write failure'); } });
  await setup();
  assert.equal(document.documentElement.dataset.theme, 'light');
  await click(document.querySelector('[aria-label="切换暗色主题"]'));
  assert.equal(document.documentElement.dataset.theme, 'dark');
  await click(document.querySelector('[aria-label="切换亮色主题"]'));
  assert.equal(document.documentElement.dataset.theme, 'light');
  await click(document.querySelector('[aria-label="设置"]'));
  assert.ok(dialog());
});

test('The add disclosure closes on Escape with focus return and on outside pointers', async () => {
  const calls = await setup();
  await click(addTrigger());
  assert.equal(addDisclosure().open, true);
  const importButton = byText('导入项目', addDisclosure());
  await act(async () => importButton.focus());
  await key(importButton, 'Escape');
  assert.equal(addDisclosure().open, false);
  assert.equal(document.activeElement, addTrigger());
  await click(addTrigger());
  assert.equal(addDisclosure().open, true);
  await act(async () => document.querySelector('.radio-brand').dispatchEvent(new window.Event('pointerdown', { bubbles: true })));
  assert.equal(addDisclosure().open, false);
  assert.equal(calls.some(([command]) => /choose_path|project_import|node_app_register/.test(command)), false);
});

test('The toolbar separates filtering and refresh from the two add actions', async () => {
  const calls = await setup();
  const filter = document.querySelector('[aria-label="筛选项目状态"], .project-filter select');
  const refresh = document.querySelector('[aria-label="刷新项目和运行状态"]');
  assert.ok(filter.closest('.radio-toolbar'));
  assert.ok(refresh.closest('.radio-toolbar'));
  assert.equal(filter.closest('.radio-add-menu'), null);
  assert.equal(refresh.closest('.radio-add-menu'), null);
  await click(addTrigger());
  assert.deepEqual([...addDisclosure().querySelectorAll('button')].map(element => element.textContent), ['添加本地应用', '导入项目']);
  const before = calls.filter(([command]) => command === 'project_list').length;
  await click(refresh);
  assert.equal(calls.filter(([command]) => command === 'project_list').length, before + 1);
});

test('Completed management history lives in settings without occupying the home notice area', async () => {
  const task = { taskId: 'disposable_completed_task', projectId: null, type: 'workspaceCleanup', status: 'succeeded', final: true, phase: 'completed', result: {}, error: null };
  await setup({ overrides: { workspace_info: () => ({ workspaceRoot: bootstrap.workspaceRoot, migrationState: 'ready', retainedRoots: [], cleanupTask: task }), task_logs: () => ({ nextCursor: 1, text: 'Disposable completed task log', gaps: [], eof: true, truncated: false }) } });
  assert.equal(document.querySelector('.radio-main .recent-tasks'), null);
  assert.equal(document.querySelector('.radio-main .build-task'), null);
  assert.equal(document.querySelector('.radio-notices').childElementCount, 0);
  await click(document.querySelector('[aria-label="设置"]'));
  const history = dialog().querySelector('.radio-settings-tasks');
  assert.ok(history.querySelector('[aria-label="最近的管理任务"]'));
  await click(history.querySelector('summary'));
  assert.match(history.textContent, /清理历史 · 已完成/);
  await click(byText('查看任务日志', history));
  assert.equal(history.querySelector('.task-logs').textContent, 'Disposable completed task log');
});

test('Active management work keeps a compact home indicator that opens its full task in settings', async () => {
  const task = { taskId: 'disposable_pending_task', projectId: null, type: 'workspaceCleanup', status: 'running', final: false, phase: 'validation', result: null, error: null };
  await setup({ overrides: { workspace_info: () => ({ workspaceRoot: bootstrap.workspaceRoot, migrationState: 'ready', retainedRoots: [], cleanupTask: task }), task_get: () => task } });
  assert.match(document.querySelector('.radio-task-indicator').textContent, /1 项管理任务正在进行/);
  assert.equal(document.querySelector('.radio-main .build-task'), null);
  await click(byText('查看任务'));
  const history = dialog().querySelector('.radio-settings-tasks');
  assert.equal(history.open, true);
  assert.match(history.querySelector('[aria-label="进行中的管理任务"]').textContent, /清理历史 · 正在执行/);
});

test('The add disclosure cannot open before bootstrap, and closes when Node entry becomes pending', async () => {
  const bootstrapRead = deferred();
  await setup({ overrides: { bootstrap_info: () => bootstrapRead.promise } });
  assert.equal(addTrigger().getAttribute('aria-disabled'), 'true');
  await click(addTrigger());
  assert.equal(addDisclosure().open, false);
  await act(async () => bootstrapRead.resolve(bootstrap));
  assert.equal(addTrigger().getAttribute('aria-disabled'), 'false');
  await unmount();
  const nodeRead = deferred();
  await setup({ nodes: [nodeFixture], overrides: { node_app_configuration: () => nodeRead.promise } });
  await click(addTrigger());
  assert.equal(addDisclosure().open, true);
  await click(document.querySelector('[aria-label="进入项目「Disposable shell Node」"]'));
  assert.equal(addTrigger().getAttribute('aria-disabled'), 'true');
  assert.equal(addDisclosure().open, false);
  await click(addTrigger());
  assert.equal(addDisclosure().open, false);
  await act(async () => nodeRead.resolve(nodeFixture));
  assert.ok(dialog());
});

test('Pending environment configuration blocks every settings dismissal, rejects duplicate saves and keeps a failed draft', async () => {
  const configuration = deferred();
  const calls = await setup({ overrides: { environment_node_set: () => configuration.promise } });
  const settings = document.querySelector('[aria-label="设置"]');
  await act(async () => settings.focus());
  await click(settings);
  await click(document.querySelector('.radio-runtime-details summary'));
  const input = document.getElementById('node-path');
  await change(input, '/disposable/custom-node');
  await click(byText('保存路径', dialog()));
  assert.equal(input.disabled, true);
  assert.equal(document.querySelector('[aria-label="关闭设置"]').disabled, true);
  assert.equal(byText('完成', dialog()).disabled, true);
  await click(byText('完成', dialog()));
  await click(document.querySelector('[aria-label="关闭设置"]'));
  await key(dialog(), 'Escape');
  await click(document.querySelector('.modal-layer'));
  assert.equal(document.querySelectorAll('[role="dialog"]').length, 1);
  assert.equal(dialog().querySelector('.modal-header h2').textContent, '设置');
  await click(document.querySelector('.radio-settings-guide summary'));
  assert.equal(byText('创建第一个项目', dialog()).disabled, true);
  // This guide action has no disabled attribute; the parent callback must
  // independently preserve the settings window while configuration is pending.
  await click(byText('查看工作区与回收站', dialog()));
  assert.equal(dialog().querySelector('.modal-header h2').textContent, '设置');
  assert.equal(document.querySelector('[aria-label="工作台导航"] [aria-current="page"]').textContent, '我的项目');
  await act(async () => document.querySelector('.node-path-form').dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true })));
  assert.deepEqual(calls.filter(([command]) => command === 'environment_node_set'), [['environment_node_set', { nodePath: '/disposable/custom-node' }]]);
  await act(async () => configuration.reject(new Error('Artificial configuration failure')));
  assert.ok(dialog());
  assert.equal(input.value, '/disposable/custom-node');
  assert.match(dialog().querySelector('.build-environment [role="alert"]').textContent, /Artificial configuration failure/);
  assert.equal(byText('完成', dialog()).disabled, false);
  assert.equal(document.querySelector('[aria-label="关闭设置"]').disabled, false);
  await click(byText('完成', dialog()));
  assert.equal(dialog(), null);
  assert.equal(document.activeElement, settings);
});
