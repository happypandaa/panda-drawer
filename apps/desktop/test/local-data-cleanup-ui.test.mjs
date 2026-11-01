// Disposable DOM fixtures and mocked native commands only. Never opens or
// deletes real projects, application data, native windows, or service state.
import assert from 'node:assert/strict';
import { after, afterEach, test } from 'node:test';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';
import { JSDOM } from 'jsdom';

const repo = fileURLToPath(new URL('../../../', import.meta.url));
await mkdir(path.join(repo, '.test-artifacts'), { recursive: true });
const output = await mkdtemp(path.join(repo, '.test-artifacts/disposable-local-data-cleanup-dom-'));
const mockPath = path.join(output, 'tauri-mock.mjs');
// These unrelated interaction fixtures have no interrupted Git additions.
await writeFile(mockPath, 'export const invoke = (command, ...args) => ["source_import_recover", "node_app_delete_tasks"].includes(command) ? Promise.resolve([]) : globalThis.__CLEANUP_INVOKE__(command, ...args); export const isTauri = () => true;\n');
const compiled = new Map();
async function compile(file) {
  if (compiled.has(file)) return compiled.get(file);
  const target = path.join(output, `${path.basename(file).replace(/\.tsx?$/, '')}.mjs`);
  compiled.set(file, target);
  let source = ts.transpileModule(await readFile(file, 'utf8'), { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX }, fileName: file }).outputText;
  for (const match of [...source.matchAll(/from\s+(['"])(\.[^'"]+)\1/g)]) {
    const base = path.resolve(path.dirname(file), match[2]); let dependency;
    for (const candidate of [base, `${base}.tsx`, `${base}.ts`]) { try { await readFile(candidate); dependency = candidate; break; } catch { /* Next extension. */ } }
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
const { createElement: h, act } = await import('react');
const { createRoot } = await import('react-dom/client');
const { LocalDataCleanupPanel } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/LocalDataCleanupPanel.tsx'))).href);
const { FrontendPlatformProvider, frontendPlatformFromBootstrap } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/frontend-platform.tsx'))).href);
const { default: App } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/App.tsx'))).href);
const originalSetInterval = window.setInterval.bind(window);
const originalClearInterval = window.clearInterval.bind(window);
let root;
afterEach(async () => {
  if (root) await act(async () => root.unmount()); root = null;
  window.setInterval = originalSetInterval; window.clearInterval = originalClearInterval;
  window.localStorage.clear();
});
after(async () => { dom.window.close(); await rm(output, { recursive: true, force: true }); });
const phrase = '删除全部本机数据';
function plan(overrides = {}) { return { planId: 'disposable-plan', instanceId: 'disposable-instance', expiresAt: Date.now() + 60_000, confirmationText: phrase, deletePaths: [{ path: '/disposable/data', label: '本机数据根' }, { path: '/disposable/migrated-workspace', label: '迁移工作区' }], preservedPaths: ['外部原作者源码、独立导出包及必要环境身份文件'], warnings: ['包括旧备份、旧回收项与恢复现场。'], ...overrides }; }
function state(status = 'stopping', extra = {}) { return { operationId: 'disposable-plan', status, ...extra }; }
function deferred() { let resolve; let reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; }
const button = text => [...document.querySelectorAll('button')].find(element => element.textContent === text);
const activeDialog = () => [...document.querySelectorAll('[role="dialog"]')].at(-1);
const writeCalls = calls => calls.filter(([command]) => command === 'local_data_cleanup_start');
async function click(element) { assert.ok(element); await act(async () => element.click()); }
async function type(value) {
  const input = document.querySelector('.local-data-cleanup-phrase input'); assert.ok(input);
  await act(async () => { Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set.call(input, value); input.dispatchEvent(new window.Event('input', { bubbles: true })); });
}
async function key(value) { await act(async () => activeDialog().dispatchEvent(new window.KeyboardEvent('keydown', { key: value, bubbles: true, cancelable: true }))); }
async function render(props = {}, overrides = {}, target) {
  const calls = []; const locks = []; const pending = [];
  globalThis.__CLEANUP_INVOKE__ = async (command, args) => {
    calls.push([command, args]);
    if (overrides[command]) return overrides[command](args);
    if (command === 'local_data_cleanup_preview') return plan();
    if (command === 'local_data_cleanup_start') return state();
    if (command === 'local_data_cleanup_status') return state();
    if (command === 'local_data_cleanup_quit') return;
    throw new Error(`Unexpected cleanup fixture command: ${command}`);
  };
  document.body.innerHTML = '<div id="root"></div>';
  root = createRoot(document.getElementById('root'));
  const values = { instanceId: 'disposable-instance', scopeIdentity: '/disposable/data:/disposable/workspace', disabled: false, onPendingChange: value => pending.push(value), onTerminalLock: () => locks.push(true), ...props };
  const panel = props => h(FrontendPlatformProvider, { value: frontendPlatformFromBootstrap(target ? { build: { target } } : null) }, h(LocalDataCleanupPanel, props));
  await act(async () => root.render(panel(values)));
  return { calls, locks, pending, values, rerender: props => act(async () => root.render(panel({ ...values, ...props }))) };
}
async function firstConfirmation() {
  await click(button('清理所有本机项目内容'));
  assert.match(activeDialog().textContent, /第一次确认/);
  await click(document.querySelector('.local-data-cleanup-modal input[type="checkbox"]'));
  await click(button('已了解，继续第二次确认'));
}
async function confirmAll() { await firstConfirmation(); await type(phrase); await click(button('永久清理所有本机数据')); }

test('Opening, checking the first warning and cancelling never submits cleanup', async () => {
  const { calls, locks } = await render();
  assert.match(document.body.textContent, /通常在卸载熊猫小抽屉之前使用/);
  await click(button('清理所有本机项目内容'));
  assert.match(activeDialog().textContent, /旧备份、旧回收项与恢复现场/);
  assert.match(activeDialog().textContent, /配置和保存的 Key/);
  assert.match(activeDialog().textContent, /\/disposable\/migrated-workspace/);
  assert.match(activeDialog().textContent, /外部原作者源码/);
  assert.match(activeDialog().textContent, /项目网页存储会等待系统确认删除，管理器网页偏好也会清空/);
  assert.match(activeDialog().textContent, /系统级应用缓存及偏好文件仍可能保留/);
  assert.equal(button('已了解，继续第二次确认').disabled, true);
  await click(document.querySelector('.local-data-cleanup-modal input[type="checkbox"]'));
  await click(button('取消，保留全部内容'));
  assert.equal(activeDialog(), undefined);
  assert.equal(writeCalls(calls).length, 0); assert.deepEqual(locks, []);
  await click(button('清理所有本机项目内容'));
  assert.equal(document.querySelector('.local-data-cleanup-modal input[type="checkbox"]').checked, false);
});

const platformCopyPlan = () => plan({
  deletePaths: [
    { path: String.raw`C:\disposable\AppData\platform`, label: '本机应用数据与项目资料' },
    { path: String.raw`D:\disposable\平台生成的源码`, label: '已登记的迁移工作区' },
    { path: String.raw`D:\disposable\自定义内容`, label: '其他原始标签' },
  ],
  preservedPaths: [String.raw`D:\disposable\外部原始目录`],
  warnings: ['永久删除，不进入废纸篓，无法撤销。', '项目 WebKit 网页存储会等待系统确认删除。', '人工警告：原目录被占用，保留现场。'],
});

test('Windows cleanup copy distinguishes generated source from preserved originals without changing the backend plan', async () => {
  const preview = platformCopyPlan();
  const original = structuredClone(preview);
  const { calls } = await render({ externalProjectCount: 2 }, { local_data_cleanup_preview: () => preview }, 'x86_64-pc-windows-msvc');
  await click(button('清理所有本机项目内容'));
  const text = activeDialog().textContent;
  assert.match(text, /你选择的文件夹及 data\/ 会保留；平台下载的项目文件夹、平台设置和所有应用的浏览器数据会删除/);
  assert.doesNotMatch(text, /外部项目的文件夹不会被删除：2 个/);
  assert.doesNotMatch(text, /已登记的迁移工作区|WebKit|废纸篓/);
  assert.deepEqual([...document.querySelectorAll('.local-data-cleanup-paths strong')].map(element => element.textContent), ['本机应用数据与项目资料', '平台管理的项目内容', '其他原始标签']);
  assert.deepEqual([...document.querySelectorAll('.local-data-cleanup-warnings li')].map(element => element.textContent), ['永久删除，不进入回收站，无法撤销。', '项目 WebView2 网页存储会等待系统确认删除。', '人工警告：原目录被占用，保留现场。']);
  assert.deepEqual([...document.querySelectorAll('.local-data-cleanup-paths code')].map(element => element.textContent), preview.deletePaths.map(item => item.path));
  assert.match(text, /D:\\disposable\\外部原始目录/);
  assert.deepEqual(preview, original);
  await click(button('取消，保留全部内容'));
  assert.equal(writeCalls(calls).length, 0);
});

test('Mac cleanup retains the exact external count, backend labels and warning terminology', async () => {
  const preview = platformCopyPlan();
  const { calls } = await render({ externalProjectCount: 2 }, { local_data_cleanup_preview: () => preview }, 'aarch64-apple-darwin');
  await click(button('清理所有本机项目内容'));
  assert.match(activeDialog().textContent, /外部项目的文件夹不会被删除：2 个。平台设置和浏览器数据会删除，文件夹里的 data\/ 会保留。/);
  assert.deepEqual([...document.querySelectorAll('.local-data-cleanup-paths strong')].map(element => element.textContent), preview.deletePaths.map(item => item.label));
  assert.deepEqual([...document.querySelectorAll('.local-data-cleanup-warnings li')].map(element => element.textContent), preview.warnings);
  assert.deepEqual([...document.querySelectorAll('.local-data-cleanup-paths code')].map(element => element.textContent), preview.deletePaths.map(item => item.path));
  assert.doesNotMatch(activeDialog().textContent, /平台管理的项目内容|WebView2/);
  await click(button('取消，保留全部内容'));
  assert.equal(writeCalls(calls).length, 0);
});

test('Two explicit confirmations and the exact phrase are required before one bound request', async () => {
  const { calls, locks } = await render();
  await firstConfirmation();
  assert.equal(writeCalls(calls).length, 0);
  assert.match(activeDialog().textContent, /第二次确认/);
  await type('删除所有数据'); assert.equal(button('永久清理所有本机数据').disabled, true);
  await type(phrase); await click(button('永久清理所有本机数据'));
  assert.deepEqual(writeCalls(calls), [['local_data_cleanup_start', { input: { planId: 'disposable-plan', firstConfirmation: true, confirmation: phrase } }]]);
  assert.deepEqual(locks, [true]);
});

test('Identity changes, temporary gates and expiry invalidate both confirmations', async () => {
  const { calls, rerender } = await render();
  await firstConfirmation(); await type(phrase);
  await rerender({ scopeIdentity: '/disposable/changed-workspace' });
  assert.equal(document.querySelector('.local-data-cleanup-phrase input'), null);
  assert.equal(button('永久清理所有本机数据'), undefined);
  await click(button('重新核对清理范围'));
  await click(document.querySelector('.local-data-cleanup-modal input[type="checkbox"]'));
  await click(button('已了解，继续第二次确认'));
  await rerender({ disabled: true }); await rerender({ disabled: false });
  assert.equal(button('永久清理所有本机数据'), undefined);
  assert.equal(writeCalls(calls).length, 0);
});

test('Expired plans and mismatched preview identities cannot enable a deletion', async () => {
  let next = plan({ expiresAt: Date.now() - 1 });
  const { calls } = await render({}, { local_data_cleanup_preview: () => next });
  await click(button('清理所有本机项目内容'));
  assert.match(activeDialog().textContent, /无法核对/);
  assert.equal(button('已了解，继续第二次确认'), undefined);
  next = plan({ instanceId: 'wrong-instance' }); await click(button('重新核对清理范围'));
  assert.equal(button('已了解，继续第二次确认'), undefined);
  next = plan({ expiresAt: Date.now() + 25 }); await click(button('重新核对清理范围'));
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 45)); });
  assert.equal(button('已了解，继续第二次确认').disabled, true);
  assert.match(activeDialog().textContent, /已过期/);
  assert.equal(writeCalls(calls).length, 0);
});

test('A gate during the first confirmation clears its acknowledgement and requires a fresh preview', async () => {
  const { calls, rerender } = await render();
  await click(button('清理所有本机项目内容'));
  await click(document.querySelector('.local-data-cleanup-modal input[type="checkbox"]'));
  assert.equal(button('已了解，继续第二次确认').disabled, false);
  await rerender({ disabled: true }); await rerender({ disabled: false });
  assert.equal(button('已了解，继续第二次确认'), undefined);
  await click(button('重新核对清理范围'));
  assert.equal(document.querySelector('.local-data-cleanup-modal input[type="checkbox"]').checked, false);
  assert.equal(writeCalls(calls).length, 0);
});

test('A preview from an earlier scope and malformed native ranges never create a usable confirmation', async () => {
  const preview = deferred(); let next = preview.promise;
  const { calls, rerender } = await render({}, { local_data_cleanup_preview: () => next });
  await click(button('清理所有本机项目内容'));
  await rerender({ instanceId: 'replacement-instance' });
  await act(async () => preview.resolve(plan()));
  assert.equal(button('已了解，继续第二次确认'), undefined);
  await rerender({ instanceId: 'disposable-instance' });
  next = plan({ deletePaths: [{ path: null, label: 'broken range' }] });
  await click(button('重新核对清理范围'));
  assert.match(activeDialog().textContent, /无法核对/);
  assert.equal(button('已了解，继续第二次确认'), undefined);
  assert.equal(writeCalls(calls).length, 0);
});

test('A pending submission blocks duplicate writes, Escape, background and close; unknown results stay locked', async () => {
  const start = deferred();
  const { calls, locks } = await render({}, { local_data_cleanup_start: () => start.promise });
  await firstConfirmation(); await type(phrase);
  const submit = button('永久清理所有本机数据');
  await act(async () => { submit.click(); submit.click(); });
  assert.equal(writeCalls(calls).length, 1); assert.deepEqual(locks, [true]);
  await key('Escape'); await click(document.querySelector('.modal-layer')); await click(activeDialog().querySelector('.modal-close'));
  assert.ok(activeDialog()); assert.equal(activeDialog().querySelector('.modal-close').disabled, true);
  await act(async () => start.reject(new Error('Artificial unknown submission')));
  assert.match(activeDialog().textContent, /提交结果尚不能确认/);
  assert.equal(button('永久清理所有本机数据'), undefined);
  assert.equal(writeCalls(calls).length, 1);
  assert.ok(button('查看清理状态'));
});

test('Failed and blocked results keep the ordinary UI locked and show remaining paths without retrying', async () => {
  for (const terminalStatus of ['blocked', 'failed']) {
    const { calls, locks } = await render({}, { local_data_cleanup_start: () => state(terminalStatus, { remainingPaths: ['/disposable/remaining'], error: { message: 'Artificial retained content' } }) });
    await confirmAll();
    assert.match(activeDialog().textContent, /\/disposable\/remaining/);
    assert.match(activeDialog().textContent, /Artificial retained content/);
    assert.doesNotMatch(activeDialog().textContent, /清理回执|receipt.json/);
    assert.equal(button('退出软件').disabled, false);
    await click(button('查看清理状态'));
    assert.equal(writeCalls(calls).length, 1); assert.deepEqual(locks, [true]);
    await act(async () => root.unmount()); root = null;
  }
});

test('Status identity mismatches do not report success; confirmed completion only offers quit', async () => {
  let latest = state('completed', { operationId: 'wrong-operation' });
  const { calls } = await render({}, { local_data_cleanup_start: () => state('blocked'), local_data_cleanup_status: () => latest });
  await confirmAll(); await click(button('查看清理状态'));
  assert.match(activeDialog().textContent, /任务身份变化/);
  assert.doesNotMatch(activeDialog().textContent, /本机项目内容已清理/);
  latest = state('completed'); await click(button('查看清理状态'));
  assert.match(activeDialog().textContent, /本机项目内容已清理/);
  assert.match(activeDialog().textContent, /项目网页存储已收到系统删除确认，管理器网页偏好已清空/);
  assert.equal(button('查看清理状态'), undefined);
  assert.deepEqual([...activeDialog().querySelectorAll('.confirmation-actions button')].map(value => value.textContent), ['退出软件']);
  await click(button('退出软件'));
  assert.equal(calls.filter(([command]) => command === 'local_data_cleanup_quit').length, 1);
  assert.equal(writeCalls(calls).length, 1);
});

test('Idle after an unaccepted submission remains locked, explains zero deletion and only permits status checks or quit', async () => {
  const { calls, locks } = await render({}, { local_data_cleanup_start: () => { throw new Error('Artificial rejected plan'); }, local_data_cleanup_status: () => ({ operationId: null, status: 'idle' }) });
  await confirmAll(); await click(button('查看清理状态'));
  assert.match(activeDialog().textContent, /本次没有受理清理，未开始删除/);
  assert.doesNotMatch(activeDialog().textContent, /本次清理已完成/);
  assert.equal(button('永久清理所有本机数据'), undefined);
  assert.equal(button('退出软件').disabled, false);
  assert.deepEqual(locks, [true]); assert.equal(writeCalls(calls).length, 1);
});

test('Malformed submission replies and incorrect operation identities cannot produce a success state', async () => {
  for (const reply of [state('completed', { operationId: 'another-plan' }), { operationId: 'disposable-plan', status: 'invented' }, state('completed', { message: { invalid: true } })]) {
    const { calls } = await render({}, { local_data_cleanup_start: () => reply });
    await confirmAll();
    assert.match(activeDialog().textContent, /提交结果尚不能确认/);
    assert.doesNotMatch(activeDialog().textContent, /本机项目内容已清理/);
    assert.equal(writeCalls(calls).length, 1);
    await act(async () => root.unmount()); root = null;
  }
});

test('The settings integration locks its parent, stops ordinary polling and does not rebuild the project UI', async () => {
  const calls = []; const timers = new Map(); let nextTimer = 1;
  window.setInterval = callback => { const value = nextTimer++; timers.set(value, callback); return value; };
  window.clearInterval = value => timers.delete(value);
  const bootstrap = { instanceId: 'disposable-instance', dataRoot: '/disposable/data', workspaceRoot: '/disposable/workspace', identifier: 'disposable-cleanup', environment: 'development', resources: { sdk: true, cli: true, template: true, readme: true }, version: 'fixture', sqliteVersion: 'fixture', apiVersion: 1, baseUrl: 'http://127.0.0.1:1' };
  globalThis.__CLEANUP_INVOKE__ = async (command, args) => {
    calls.push([command, args]);
    if (command === 'bootstrap_info') return bootstrap;
    if (command === 'project_list') return [];
    if (command === 'workspace_info') return { workspaceRoot: bootstrap.workspaceRoot, migrationState: 'ready', retainedRoots: [] };
    if (command === 'node_app_list') return [{ id: 'disposable-node', name: 'Disposable application', runtime: 'node', state: 'stopped', sourcePath: '/disposable/source', fields: [], configuration: { canStart: true, missingRequired: [], secretStatus: {}, values: {}, revision: 1 } }];
    if (command === 'environment_doctor') return { ready: false, issues: [], supportedNodeRange: 'fixture', recommendedNode: 'fixture', selection: 'detected' };
    if (command === 'environment_git_doctor') return { ready: false, issues: [], selection: 'detected', authentication: { state: 'not-checked' }, ssh: { ready: false, issues: [] } };
    if (command === 'local_data_cleanup_preview') return plan();
    if (command === 'local_data_cleanup_start') return state('completed');
    throw new Error(`Unexpected settings cleanup fixture command: ${command}`);
  };
  document.body.innerHTML = '<div id="root"></div>'; root = createRoot(document.getElementById('root'));
  await act(async () => root.render(h(App)));
  const staleTimers = [...timers.values()];
  await click(document.querySelector('[aria-label="设置"]'));
  await confirmAll();
  assert.equal(document.querySelector('[aria-label="关闭设置"]').disabled, true);
  assert.equal(button('完成'), undefined);
  assert.equal(document.querySelector('.radio-toolbar'), null);
  assert.equal(document.querySelector('.workspace-storage-panel'), null);
  assert.equal(document.querySelector('.runtime-view'), null);
  assert.match(activeDialog().textContent, /本机项目内容已清理/);
  const count = calls.length;
  await act(async () => { staleTimers.forEach(callback => callback()); });
  assert.equal(calls.length, count);
  assert.equal(timers.size, 0);
  assert.equal(writeCalls(calls).length, 1);
});
