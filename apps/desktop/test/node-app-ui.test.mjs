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
await writeFile(mockPath, 'export const invoke = (...args) => globalThis.__NODE_FORM_INVOKE__(...args); export const isTauri = () => true;\n');
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
    source = source.replaceAll(/(['"])@tauri-apps\/api\/core\1/g, JSON.stringify(pathToFileURL(mockPath).href));
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
let root;
afterEach(async () => { if (root) await act(async () => root.unmount()); root = null; delete navigator.clipboard; globalThis.__NODE_FORM_INVOKE__ = async (command) => { throw new Error(`Unexpected native request: ${command}`); }; });
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
function app(extra = {}) { return { id: 'node_fixture', name: 'Disposable Node fixture', sourcePath: '/not-a-real-source', runtime: 'node-service', state: 'stopped', restartRequired: false, runningRevision: null, lastError: null, configuration: { revision: 1, values: {}, secretStatus: { AI_KEY: 'set' }, missingRequired: [], canStart: true }, fields, ...extra }; }
async function setup(mock, props = {}, Component = NodeAppsPanel) {
  document.body.innerHTML = '<div id="root"></div>';
  root = createRoot(document.getElementById('root'));
  globalThis.__NODE_FORM_INVOKE__ = mock;
  await act(async () => root.render(h(Component, { enabled: true, onDraftStateChange() {}, ...(Component === NodeAppsPanel ? { request: { app: app(), intent: 'details', token: 1 } } : {}), ...props })));
  if (Component === NodeAppsPanel && props.request !== null && props.request?.intent !== 'delete' && document.querySelector('[role=dialog]')) await click(byText('应用配置'));
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
async function submit() { await act(async () => document.querySelector('.node-configuration-form').dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true }))); }
function deferred() { let resolve; let reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { resolve, reject, promise }; }
function baseMock(value, action) { return async (command, args) => { if (command === 'node_app_list') return [value]; if (command === 'node_app_configuration') return value; return action(command, args); }; }

function deletionPreview(value, ownership = 'managed', expectedRevision = 'fresh-disposable-revision') {
  return { appId: value.id, name: value.name, ownership, expectedRevision, platformRoot: `/disposable/platform/${value.id}`, sourcePath: '/disposable/original-source', deletePaths: [`/disposable/platform/${value.id}/data`, `/disposable/platform/${value.id}/configuration`, ...(ownership === 'managed' ? [`/disposable/platform/${value.id}/generations`] : [])], preservedPaths: ['/disposable/original-source', '/disposable/install-evidence'] };
}

test('Node deletion reads ownership-specific scope, requires acknowledgment and sends no filesystem paths', async () => {
  for (const ownership of ['managed', 'external']) {
    const value = app(); const preview = deletionPreview(value, ownership); const finished = deferred(); const calls = []; const deleted = [];
    await setup(baseMock(value, async (command, args) => {
      calls.push([command, args]);
      if (command === 'node_app_delete_preview') return preview;
      if (command === 'node_app_delete') return finished.promise;
      throw new Error(command);
    }), { instanceId: 'same', request: { app: value, intent: 'delete', token: 1 }, onAppDeleted: id => deleted.push(id) });
    const confirmation = document.querySelector('[aria-label="永久删除 Node 项目确认"]');
    assert.match(confirmation.textContent, /不会为本次删除保存可恢复副本/);
    assert.match(confirmation.textContent, ownership === 'managed' ? /将永久删除本实例的程序、业务数据、应用配置与密钥/ : /原始应用目录及其中的全部文件保持原样/);
    for (const path of [...preview.deletePaths, ...preview.preservedPaths]) assert.ok(confirmation.textContent.includes(path));
    assert.equal(byText('确认永久删除项目').disabled, true);
    assert.deepEqual(calls, [['node_app_delete_preview', { appId: value.id }]]);
    await click(confirmation.querySelector('input[type=checkbox]'));
    await click(byText('确认永久删除项目'));
    assert.equal(byText('正在永久删除…').disabled, true);
    await click(byText('正在永久删除…'));
    assert.deepEqual(calls.at(-1), ['node_app_delete', { appId: value.id, input: { expectedRevision: preview.expectedRevision } }]);
    assert.equal(calls.filter(([command]) => command === 'node_app_delete').length, 1);
    await act(async () => finished.resolve({ appId: value.id, operationOutcome: 'deleted', ownership, sourcePreserved: true, evidenceRetained: true }));
    assert.deepEqual(deleted, [value.id]); assert.equal(document.querySelector('[role=dialog]'), null);
    await act(async () => root.unmount()); root = null;
  }
});

test('Failed Node deletion preserves the scope and error, then requires a new preview and acknowledgment', async () => {
  const value = app({ state: 'running' }); let previews = 0; const writes = []; const deleted = [];
  await setup(baseMock(value, async (command, args) => {
    if (command === 'node_app_delete_preview') return deletionPreview(value, 'external', `revision-${++previews}`);
    if (command === 'node_app_delete') { writes.push(args); throw { code: 'NODE_APP_WINDOW_OPEN', message: '请先关闭项目窗口，再删除。' }; }
    throw new Error(command);
  }), { request: { app: value, intent: 'delete', token: 1 }, onAppDeleted: id => deleted.push(id) });
  await click(document.querySelector('.node-delete-confirmation input'));
  await click(byText('确认永久删除项目'));
  assert.match(document.querySelector('.node-delete-confirmation').textContent, /请先关闭项目窗口/);
  assert.equal(byText('确认永久删除项目').disabled, true);
  assert.equal(document.querySelector('.node-delete-confirmation input').checked, false);
  assert.match(document.querySelector('.node-delete-confirmation').textContent, /disposable\/original-source/);
  await click(byText('重新核对删除范围'));
  assert.equal(document.querySelector('.node-delete-confirmation input').checked, false);
  await click(document.querySelector('.node-delete-confirmation input')); await click(byText('确认永久删除项目'));
  assert.deepEqual(writes.map(value => value.input.expectedRevision), ['revision-1', 'revision-2']);
  assert.deepEqual(deleted, []);
});

test('Leaving Node deletion discards acknowledgment and configuration drafts block starting deletion', async () => {
  const value = app(); let previews = 0;
  await setup(baseMock(value, async command => {
    assert.equal(command, 'node_app_delete_preview'); previews++; return deletionPreview(value);
  }), { request: { app: value, intent: 'delete', token: 1 } });
  await click(document.querySelector('.node-delete-confirmation input'));
  await click(byText('应用配置'));
  assert.equal(document.querySelector('.node-delete-confirmation'), null);
  await change(input('AI_KEY'), 'artificial-private-draft');
  assert.equal(byText('删除项目').disabled, true);
  assert.equal(previews, 1);
  await change(input('AI_KEY'), ''); await click(byText('删除项目'));
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
  await click(byText('删除项目'));
  assert.match(document.querySelector('.node-delete-confirmation').textContent, /平台安装副本/);
  assert.equal(document.querySelector('.node-delete-confirmation input').checked, false);
});

test('Instance changes release a pending Node deletion locally and ignore its late result without replay', async () => {
  const value = app(); const delayed = deferred(); const deleted = []; let writes = 0; let previews = 0;
  const props = { enabled: true, instanceId: 'old', onDraftStateChange() {}, onAppDeleted: id => deleted.push(id), request: { app: value, intent: 'delete', token: 1 } };
  await setup(baseMock(value, async command => {
    if (command === 'node_app_delete_preview') return deletionPreview(value, 'managed', `revision-${++previews}`);
    if (command === 'node_app_delete') { writes++; return delayed.promise; }
    throw new Error(command);
  }), props);
  await click(document.querySelector('.node-delete-confirmation input')); await click(byText('确认永久删除项目'));
  assert.equal(writes, 1);
  await act(async () => root.render(h(NodeAppsPanel, { ...props, instanceId: 'new' })));
  assert.equal(document.querySelector('.node-delete-confirmation'), null);
  assert.equal(byText('删除项目').disabled, false);
  await act(async () => delayed.resolve({ appId: value.id, operationOutcome: 'deleted', ownership: 'managed', sourcePreserved: true, evidenceRetained: true }));
  assert.deepEqual(deleted, []); assert.equal(writes, 1); assert.ok(document.querySelector('[role=dialog]'));
  await click(byText('删除项目'));
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
  assert.equal(document.querySelector('.node-delete-confirmation'), null); assert.equal(byText('删除项目').disabled, false);
  await click(byText('删除项目'));
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
      if (command === 'node_app_delete') return { appId: value.id, operationOutcome: 'deleted', ownership: 'managed', sourcePreserved: true, evidenceRetained: true };
      throw new Error(command);
    }), { instanceId: 'same', request: { app: value, intent: 'details', token: 1 }, onAppChanged: value => changes.push(value), onAppDeleted: id => deleted.push(id) });
    await act(async () => { for (const callback of timers.values()) callback(); });
    await click(byText('删除项目')); await click(document.querySelector('.node-delete-confirmation input')); await click(byText('确认永久删除项目'));
    const observations = changes.length;
    await act(async () => delayed.resolve({ ...value, state: 'running' }));
    assert.deepEqual(deleted, [value.id]); assert.equal(changes.length, observations); assert.equal(document.querySelector('[role=dialog]'), null);
  } finally { window.setInterval = originalInterval; window.clearInterval = originalClear; }
});

test('A mismatched Node deletion preview or unknown deletion result never removes the app', async () => {
  const value = app(); const deleted = []; let previews = 0;
  await setup(baseMock(value, async command => {
    if (command === 'node_app_delete_preview') return { ...deletionPreview(value), appId: ++previews === 1 ? 'another-app' : value.id };
    if (command === 'node_app_delete') return { appId: value.id, operationOutcome: 'delete_incomplete' };
    throw new Error(command);
  }), { request: { app: value, intent: 'delete', token: 1 }, onAppDeleted: id => deleted.push(id) });
  assert.match(document.querySelector('.node-delete-confirmation').textContent, /项目身份不符/);
  assert.equal(byText('确认永久删除项目').disabled, true);
  await click(byText('重新核对删除范围')); await click(document.querySelector('.node-delete-confirmation input')); await click(byText('确认永久删除项目'));
  assert.match(document.querySelector('.node-delete-confirmation').textContent, /删除结果尚不能确认/);
  assert.deepEqual(deleted, []); assert.ok(document.querySelector('[role=dialog]'));
});

test('Managed app update preserves configuration drafts and only reads candidates on explicit request', async () => {
  const value = app({ id: '11111111-1111-4111-8111-111111111111', installation: { kind: 'managed', generation: 'gen_' + 'a'.repeat(32), sourceId: 'src_' + 'b'.repeat(32), sourceDigest: 'c'.repeat(64) } });
  const calls = [];
  await setup(baseMock(value, async command => {
    calls.push(command);
    if (command === 'source_list') return [];
    if (command === 'node_install_list') return { candidates: [], issues: [] };
    throw new Error(command);
  }), { request: { app: value, intent: 'details', token: 1 } });
  await change(input('MODEL'), 'unfinished configuration');
  await click(byText('概览'));
  assert.equal(byText('选择更新来源或查看候选').disabled, true);
  assert.deepEqual(calls, []);
  assert.match(document.querySelector('[role=dialog]').textContent, /运行不依赖原作者目录/);
  await click(byText('应用配置')); await change(input('MODEL'), ''); await click(byText('概览'));
  await click(byText('选择更新来源或查看候选'));
  assert.deepEqual(calls, ['source_list', 'node_install_list']);
  assert.ok(document.querySelector('.node-install-panel'));
});

function managedInstallFixture() {
  const value = app({ id: '11111111-1111-4111-8111-111111111111', installation: { kind: 'managed', generation: `gen_${'a'.repeat(32)}`, sourceId: `src_${'b'.repeat(32)}`, sourceDigest: 'c'.repeat(64) } });
  const source = { sourceId: `src_${'d'.repeat(32)}`, name: 'current-source', status: 'ready', contentDigest: 'e'.repeat(64), inspection: { kind: 'node-service-source', issues: [] } };
  const prepared = { candidateId: `install_${'f'.repeat(32)}`, appId: value.id, generation: `gen_${'1'.repeat(32)}`, manifest: { runtime: 'node-service', app: { id: 'org.example.fixture', name: 'Managed fixture', version: '2.0.0' } }, programDigest: '2'.repeat(64), provenance: { sourceId: source.sourceId, sourceDigest: source.contentDigest, sourceCommit: null, node: { path: '/fixture/node', version: '24.22.0' }, npm: { path: '/fixture/npm', version: '11.19.1' }, preparedAt: 1 } };
  const installed = { candidateId: prepared.candidateId, appId: value.id, generation: prepared.generation, operationOutcome: 'committed' };
  const history = { candidateId: prepared.candidateId, appId: value.id, targetAppId: value.id, state: 'committed', prepareTaskId: 'task_prepare', applyTaskId: 'task_apply', prepared, error: null, issues: [] };
  return { value, source, prepared, installed, history };
}

test('Managed details distinguish declared, legacy and unreadable data metadata without disabling existing app use', async () => {
  for (const [extra, expected, legacy] of [
    [{ dataContract: { id: 'org.example.guestbook-data', version: 2 } }, /org.example.guestbook-data · 版本 2/, false],
    [{ dataContract: null }, /数据声明：未声明/, true],
    [{ dataContract: null, state: 'failed', lastError: '人工声明读取失败' }, /数据声明：暂无法核对/, false],
    [{}, /数据声明：暂无法核对/, false],
  ]) {
    const { value: original } = managedInstallFixture();
    const value = { ...original, ...extra }; const calls = [];
    await setup(baseMock(value, async (command, args) => {
      calls.push([command, args]);
      if (command === 'node_app_start') return { ...value, state: 'running' };
      throw new Error(command);
    }), { request: { app: value, intent: 'details', token: 1 } });
    await click(byText('概览'));
    const declaration = document.querySelector('[aria-label="数据声明"]');
    assert.match(declaration.textContent, expected);
    if (extra.dataContract) assert.match(declaration.textContent, /平台未验证实际数据格式/);
    if (legacy) {
      assert.match(declaration.textContent, /后续普通更新无法核对数据契约，将被阻止；现有应用仍可使用/);
      await click(byText('进入项目'));
      assert.deepEqual(calls, [['node_app_start', { appId: value.id }]]);
    } else {
      assert.doesNotMatch(declaration.textContent, /后续普通更新无法核对数据契约，将被阻止/);
      assert.deepEqual(calls, []);
    }
    await act(async () => root.unmount()); root = null;
  }
});

test('Managed update rejects an old instance source list and releases its pending gate for a fresh read', async () => {
  const { value, source } = managedInstallFixture(); const older = deferred(); let reads = 0;
  const props = { enabled: true, instanceId: 'old', onDraftStateChange() {}, request: { app: value, intent: 'details', token: 1 } };
  await setup(baseMock(value, async command => {
    if (command === 'source_list') return ++reads === 1 ? older.promise : [source];
    if (command === 'node_install_list') return { candidates: [], issues: [] };
    throw new Error(command);
  }), props);
  await click(byText('概览')); await click(byText('选择更新来源或查看候选'));
  assert.equal(byText('选择更新来源或查看候选').disabled, true);
  await act(async () => root.render(h(NodeAppsPanel, { ...props, instanceId: 'new' })));
  assert.equal(byText('选择更新来源或查看候选').disabled, false);
  await click(byText('选择更新来源或查看候选'));
  await act(async () => older.resolve([{ ...source, name: 'old-instance-source' }]));
  assert.equal(reads, 2);
  assert.match(document.querySelector('.node-install-panel').textContent, /current-source/);
  assert.doesNotMatch(document.body.textContent, /old-instance-source/);
});

test('Managed update drops old installation state on instance changes while retaining configuration drafts', async () => {
  const { value, source } = managedInstallFixture(); const tasks = [];
  const task = { taskId: 'old-install-task', type: 'nodeInstallPrepare', projectId: null, status: 'running', final: false, phase: 'dependencies' };
  const props = { enabled: true, instanceId: 'old', onDraftStateChange() {}, onTask: value => tasks.push(value), request: { app: value, intent: 'details', token: 1 } };
  await setup(baseMock(value, async command => {
    if (command === 'source_list') return [source];
    if (command === 'node_install_list') return { candidates: [], issues: [] };
    if (command === 'node_install_prepare' || command === 'task_get') return task;
    throw new Error(command);
  }), props);
  await click(byText('概览')); await click(byText('选择更新来源或查看候选'));
  await change(document.querySelector('.node-install-panel select'), source.sourceId);
  await click(byText('准备安装材料'));
  assert.ok(byText('取消准备'));
  await click(byText('应用配置')); await change(input('MODEL'), 'unsaved model'); await change(input('AI_KEY'), 'artificial unsaved key');
  const taskCount = tasks.length;
  await act(async () => root.render(h(NodeAppsPanel, { ...props, instanceId: 'new' })));
  assert.equal(input('MODEL').value, 'unsaved model'); assert.equal(input('AI_KEY').value, 'artificial unsaved key');
  assert.equal(document.querySelector('.node-install-panel'), null); assert.equal(tasks.length, taskCount);
  await change(input('MODEL'), ''); await change(input('AI_KEY'), ''); await click(byText('概览'));
  await click(byText('选择更新来源或查看候选'));
  assert.equal(byText('取消准备'), undefined); assert.equal(tasks.length, taskCount);
});

test('An old status poll cannot replace application metadata after an installation refresh', async () => {
  const { value, source, installed, history } = managedInstallFixture(); const older = deferred(); const changes = [];
  const updated = { ...value, sourcePath: '/new-managed-generation', installation: { ...value.installation, generation: installed.generation } };
  let configurations = 0; let polls = 0;
  const originalInterval = window.setInterval; const originalClear = window.clearInterval; const timers = new Map(); let timerId = 0;
  window.setInterval = callback => { const id = ++timerId; timers.set(id, callback); return id; };
  window.clearInterval = id => timers.delete(id);
  try {
    await setup(async command => {
      if (command === 'node_app_configuration') return ++configurations === 1 ? value : updated;
      if (command === 'source_list') return [source];
      if (command === 'node_install_list') return { candidates: [history], issues: [] };
      if (command === 'node_install_candidate') return { state: 'committed', result: installed };
      assert.equal(command, 'node_app_status'); polls++; return older.promise;
    }, { instanceId: 'same', onAppChanged: value => changes.push(value), request: { app: value, intent: 'details', token: 1 } });
    await click(byText('概览')); await click(byText('选择更新来源或查看候选'));
    await act(async () => { for (const callback of timers.values()) callback(); });
    assert.equal(polls, 1);
    await click(byText('核验已安装结果'));
    assert.equal(changes.at(-1).installation.generation, installed.generation);
    await act(async () => older.resolve({ ...value, state: 'running' }));
    assert.equal(changes.at(-1).installation.generation, installed.generation);
    assert.match(document.body.textContent, /new-managed-generation/);
    assert.match(document.querySelector('.node-process-state').textContent, /已停止/);
  } finally { window.setInterval = originalInterval; window.clearInterval = originalClear; }
});

test('An installation refresh from a previous service instance cannot overwrite current detail or draft', async () => {
  const { value, source, installed, history } = managedInstallFixture(); const older = deferred(); const changes = []; let reads = 0;
  const props = { enabled: true, instanceId: 'old', onDraftStateChange() {}, onAppChanged: value => changes.push(value), request: { app: value, intent: 'details', token: 1 } };
  await setup(async command => {
    if (command === 'node_app_configuration') return ++reads === 1 ? value : older.promise;
    if (command === 'source_list') return [source];
    if (command === 'node_install_list') return { candidates: [history], issues: [] };
    if (command === 'node_install_candidate') return { state: 'committed', result: installed };
    throw new Error(command);
  }, props);
  await click(byText('概览')); await click(byText('选择更新来源或查看候选')); await click(byText('核验已安装结果'));
  assert.equal(reads, 2);
  await click(byText('应用配置')); await change(input('MODEL'), 'draft across restart');
  await act(async () => root.render(h(NodeAppsPanel, { ...props, instanceId: 'new' })));
  const changeCount = changes.length;
  await act(async () => older.resolve({ ...value, sourcePath: '/old-instance-late-result', installation: { ...value.installation, generation: installed.generation } }));
  assert.equal(changes.length, changeCount);
  assert.equal(input('MODEL').value, 'draft across restart');
  assert.doesNotMatch(document.body.textContent, /old-instance-late-result/);
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
  await click(byText('应用配置'));
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
  assert.equal(byText('进入项目').disabled, false, 'Returning to an already running app does not need to apply this draft');
  await click(byText('进入项目'));
  assert.deepEqual(calls, [['node_app_open', { appId: 'node_fixture' }]]);
  assert.equal(input('MODEL').disabled, true);
  await click(byLabel('设置 MODEL说明'));
  const explanation = document.querySelector('.node-field-help-modal');
  assert.ok(explanation);
  assert.equal(explanation.closest('[hidden]'), null);
  assert.equal(document.querySelectorAll('[role=dialog]').length, 2);
  assert.ok(explanation.contains(document.activeElement));

  await act(async () => entered.reject(new Error('Artificial entry focus failure')));
  assert.equal(document.querySelector('[role=tab][aria-selected=true]').textContent, '运行状态');
  assert.equal(document.querySelector('.node-field-help-modal'), null, 'A hidden configuration panel cannot retain a child modal on the stack');
  assert.equal(document.querySelectorAll('[role=dialog]').length, 1);
  assert.match(document.querySelector('[role=alert]').textContent, /Artificial entry focus failure/);
  assert.equal(document.activeElement.closest('[hidden], [inert]'), null);
  await click(byText('应用配置'));
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
  await submit();
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
  await submit();
  assert.deepEqual(calls, []);
  assert.equal(document.activeElement, input('AI_URL'));
  let invalidUrlValid;
  await act(async () => { invalidUrlValid = input('AI_URL').checkValidity(); });
  assert.equal(invalidUrlValid, false);
  await change(input('AI_URL'), 'https://corrected.example.invalid/api');
  assert.equal(input('AI_URL').getAttribute('aria-invalid'), null);
  assert.equal(input('AI_URL').checkValidity(), true);
  await submit();
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
    await submit();
    assert.deepEqual(calls, []);
    assert.equal(document.activeElement, input('LIMIT'));
    assert.equal(input('LIMIT').getAttribute('aria-invalid'), 'true');
    assert.match(document.getElementById(input('LIMIT').getAttribute('aria-describedby')).textContent, /整数/);
  }
  await change(input('LIMIT'), '0');
  assert.equal(input('LIMIT').getAttribute('aria-invalid'), null);
  assert.equal(input('LIMIT').checkValidity(), true);
  await submit();
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
    await submit();
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
    await submit();
    assert.equal(calls.length, 1);
    assert.deepEqual(calls[0][1].input.values, { MODEL: 'draft for native numeric validation' });
    assert.equal(Object.hasOwn(calls[0][1].input.values, 'LIMIT'), false, 'Clearing uses the declared default instead of saving an unfinished number');
    assert.equal(input('LIMIT').getAttribute('aria-invalid'), null);
    assert.equal(input('LIMIT').validity.customError, false);
  });
}

test('Explicitly rereading an unchanged revision replaces invalid browser input without starting or saving the app', async () => {
  const value = app(); const calls = [];
  await setup(async (command, args) => {
    calls.push([command, args]);
    if (command === 'node_app_list') return [value];
    if (command === 'node_app_configuration') return value;
    throw new Error(command);
  });
  const number = input('LIMIT');
  const nativeValidity = number.validity;
  Object.defineProperty(number, 'validity', {
    configurable: true,
    get: () => new Proxy(nativeValidity, {
      get: (target, key) => key === 'badInput' ? true : Reflect.get(target, key, target),
    }),
  });
  await act(async () => {
    number.dispatchEvent(new window.Event('input', { bubbles: true }));
    number.focus();
    number.blur();
  });
  assert.equal(number.value, '', 'An unfinished numeric entry can expose the same empty semantic value');
  assert.equal(number.getAttribute('aria-invalid'), 'true');
  assert.equal(number.validity.customError, true);
  assert.equal(byText('保存配置').disabled, true, 'Native invalid text alone is not an ordinary draft change');
  await click(byText('重新读取配置'));
  assert.equal(document.querySelector('[aria-label="重新读取配置确认"]'), null);
  const reloaded = input('LIMIT');
  assert.notEqual(reloaded, number);
  assert.equal(number.isConnected, false);
  assert.equal(reloaded.value, '');
  assert.equal(reloaded.getAttribute('aria-invalid'), null);
  assert.equal(reloaded.validity.badInput, false);
  assert.equal(reloaded.validity.customError, false);
  assert.equal(document.querySelector('[role=tab][aria-selected=true]').textContent, '应用配置');
  assert.equal(document.querySelectorAll('[role=dialog]').length, 1);
  assert.equal(calls.filter(([command]) => command === 'node_app_configuration').length, 2);
  assert.equal(calls.some(([command]) => ['node_app_start', 'node_app_open', 'node_app_restart', 'node_app_configuration_save'].includes(command)), false);
});

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
  await submit();
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
  await change(input('MODEL'), 'manual-model'); await submit();
  assert.deepEqual(calls[0][1].input.secrets, { AI_KEY: { action: 'keep' } });
  assert.equal(input('AI_KEY').getAttribute('aria-invalid'), null, 'A stored required key does not need to be typed again');
  await click(byLabel('移除此应用的密钥'));
  assert.equal(input('AI_KEY').disabled, true);
  await submit();
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
  await submit();
  assert.equal(calls.length, 0, 'An unconfirmed credential target is rejected before any native save');
  assert.equal(input('AI_KEY').getAttribute('aria-invalid'), 'true');
  assert.equal(document.activeElement, input('AI_KEY'));
  await click(checkbox());
  assert.equal(input('AI_KEY').getAttribute('aria-invalid'), null);
  await submit();
  assert.deepEqual(calls[0][1].input.secrets, { AI_KEY: { action: 'rebind' } });
  await change(input('AI_URL'), 'https://third.example.invalid/api');
  assert.equal(checkbox().checked, false);
  assert.equal(input('AI_URL').value, 'https://third.example.invalid/api');
});

test('Failed saves preserve the draft and reject duplicate pending submissions', async () => {
  const accepted = deferred(); let saves = 0;
  await setup(baseMock(app(), async (command) => { assert.equal(command, 'node_app_configuration_save'); saves++; return accepted.promise; }));
  await change(input('AI_KEY'), 'fixture-draft');
  await submit(); await submit(); assert.equal(saves, 1);
  await act(async () => accepted.reject({ code: 'CONFIGURATION_REVISION_CHANGED', message: '配置已被其他窗口修改，请重新核对' }));
  assert.equal(input('AI_KEY').value, 'fixture-draft');
  assert.match(document.querySelector('[role="alert"]').textContent, /其他窗口/);
  assert.equal(byText('进入项目').disabled, true);
});

test('Opening an app makes one native start request, then returning delegates only its app ID', async () => {
  const accepted = deferred(); const calls = [];
  await setup(baseMock(app(), async (command, args) => { calls.push([command, args]); if (command === 'node_app_start') return accepted.promise; if (command === 'node_app_open') return; throw new Error(command); }));
  assert.equal(byText('打开应用页面'), undefined);
  assert.equal(byText('启动应用'), undefined);
  await click(byText('进入项目')); await click(byText('进入项目'));
  assert.deepEqual(calls, [['node_app_start', { appId: 'node_fixture' }]]);
  await act(async () => accepted.resolve(app({ state: 'running', runningRevision: 1 })));
  await click(byText('进入项目'));
  assert.deepEqual(calls[1], ['node_app_open', { appId: 'node_fixture' }]);
  assert.equal(calls.length, 2);
  assert.ok(byText('进入项目'));
});

test('Returning to an open application never starts a second instance and gates duplicate pending focus requests', async () => {
  const focus = deferred(); const calls = [];
  await setup(baseMock(app({ state: 'running', runningRevision: 1 }), async (command, args) => {
    calls.push([command, args]); assert.equal(command, 'node_app_open'); return focus.promise;
  }));
  await click(byText('进入项目')); await click(byText('进入项目'));
  assert.deepEqual(calls, [['node_app_open', { appId: 'node_fixture' }]]);
  await act(async () => focus.resolve());
  assert.equal(byText('进入项目').disabled, false);
});

test('Closing an app sends one stop request and returns the manager to its unopened state', async () => {
  const closed = deferred(); const calls = [];
  await setup(baseMock(app({ state: 'running', runningRevision: 1 }), async (command, args) => {
    calls.push([command, args]); assert.equal(command, 'node_app_stop'); return closed.promise;
  }));
  await click(byText('运行状态'));
  await click(byText('关闭应用')); await click(byText('关闭应用'));
  assert.deepEqual(calls, [['node_app_stop', { appId: 'node_fixture' }]]);
  await act(async () => closed.resolve(app()));
  assert.equal(byText('关闭应用').disabled, true);
  assert.equal(byText('进入项目').disabled, false);
  assert.match(document.body.textContent, /应用已关闭，相关服务已停止/);
});

test('Incomplete required configuration blocks opening an application and identifies the field to finish', async () => {
  await setup(baseMock(app({ configuration: { revision: 1, values: {}, secretStatus: { AI_KEY: 'missing' }, missingRequired: ['AI_KEY'], canStart: false } }), async (command) => { throw new Error(command); }));
  assert.equal(byText('进入项目').disabled, false);
  assert.equal(document.querySelector('.node-config-warning'), null);
  assert.equal(input('AI_KEY').getAttribute('aria-invalid'), 'true');
  assert.match(document.getElementById(input('AI_KEY').getAttribute('aria-describedby')).textContent, /请填写此项/);
  assert.equal(input('AI_URL').getAttribute('aria-invalid'), null, 'A required default address is already complete');
  await click(byText('进入项目'));
});

test('Saving configuration does not reopen a running window; applying it requires an explicit restart', async () => {
  const calls = [];
  await setup(baseMock(app({ state: 'running', runningRevision: 1 }), async (command, args) => {
    calls.push([command, args]);
    const configuration = { revision: 2, values: { MODEL: 'new-artificial-model' }, secretStatus: { AI_KEY: 'set' }, missingRequired: [], canStart: true };
    if (command === 'node_app_configuration_save') return app({ state: 'running', runningRevision: 1, restartRequired: true, configuration });
    assert.equal(command, 'node_app_restart');
    return app({ state: 'running', runningRevision: 2, configuration });
  }));
  await change(input('MODEL'), 'new-artificial-model'); await submit();
  assert.equal(calls.length, 1);
  assert.equal(calls[0][0], 'node_app_configuration_save');
  assert.match(document.body.textContent, /正在运行的应用仍使用旧配置/);
  await click(byText('运行状态'));
  await click(byText('重启并应用配置'));
  assert.deepEqual(calls[1], ['node_app_restart', { appId: 'node_fixture' }]);
  assert.equal(calls.length, 2);
  assert.ok(byText('进入项目'));
});

function directoryReview(state = 'ready', extra = {}) {
  return { sourcePath: '/artificial/node-source', state, expectedRevision: 'directory-revision-1', issues: [], reviewPrompt: state === 'needs-review' ? '完整后端说明\n请先读取 /artificial/node-source 再检查。\n不得直接运行。' : '', ...(state === 'ready' ? { inspection: { runtime: 'node-service', runtimeLabel: 'Node 服务', app: { id: 'org.example.preview', name: 'Artificial preview', version: '0.1.0' }, dataContract: { id: 'org.example.inspected-data', version: 1 }, configuration: { fields, missingRequired: ['AI_KEY'] } } } : {}), ...extra };
}
async function setupDirectory(mock, props = {}) {
  await setup(mock, { instanceId: 'directory-instance', onPendingChange() {}, onAdded() {}, ...props }, LocalDirectoryPanel);
}

test('Adding a ready local folder previews first and binds registration to the reviewed directory without starting it', async () => {
  const calls = [];
  await setup(async (command, args) => {
    calls.push([command, args]);
    if (command === 'node_app_list') return [];
    if (command === 'choose_path') return '/artificial/node-source';
    if (command === 'node_app_directory_inspect') return directoryReview();
    if (command === 'node_app_directory_register') return app({ sourcePath: '/artificial/node-source' });
    throw new Error(command);
  }, { request: null, addRequest: 1 });
  await click(byText('选择本地项目文件夹'));
  assert.deepEqual(calls, [['choose_path', { kind: 'nodeapp' }], ['node_app_directory_inspect', { sourcePath: '/artificial/node-source' }]]);
  assert.ok(byText('确认添加此项目')); assert.equal(document.querySelector('[role="dialog"]').textContent.includes('Artificial preview'), true);
  assert.match(document.querySelector('[aria-label="数据声明"]').textContent, /org.example.inspected-data · 版本 1/);
  assert.match(document.querySelector('[aria-label="数据声明"]').textContent, /平台未验证实际数据格式/);
  await click(byText('确认添加此项目'));
  assert.deepEqual(calls.at(-1), ['node_app_directory_register', { sourcePath: '/artificial/node-source', expectedRevision: 'directory-revision-1' }]);
  assert.ok(document.querySelector('[role="dialog"]'));
  assert.equal(document.querySelector('[role=tab][aria-selected=true]').textContent, '应用配置');
  assert.equal(calls.some(([command]) => command === 'node_app_start'), false);
});

test('An empty folder requires a name and separate acknowledged initialization before explicit registration', async () => {
  const calls = []; const added = [];
  await setupDirectory(async (command, args) => {
    calls.push([command, args]);
    if (command === 'choose_path') return '/artificial/node-source';
    if (command === 'node_app_directory_inspect') return directoryReview('empty');
    if (command === 'node_app_directory_initialize') return directoryReview('ready', { expectedRevision: 'initialized-revision' });
    if (command === 'node_app_directory_register') return app({ sourcePath: '/artificial/node-source' });
    throw new Error(command);
  }, { onAdded: value => added.push(value) });
  await click(byText('选择本地项目文件夹'));
  const name = document.querySelector('.local-directory-empty input');
  assert.equal(name.value, 'node-source'); assert.match(document.body.textContent, /不会添加项目、安装依赖或启动应用/);
  await change(name, ''); assert.equal(byText('在空目录生成起始项目…').disabled, true);
  await change(name, 'My local project'); await click(byText('在空目录生成起始项目…'));
  assert.equal(byText('确认生成起始项目').disabled, true);
  await click(byText('取消，保留空目录'));
  assert.equal(document.querySelector('[aria-label="生成起始项目确认"]'), null); assert.equal(calls.length, 2);
  await click(byText('在空目录生成起始项目…')); await click(document.querySelector('[aria-label="生成起始项目确认"] input')); await click(byText('确认生成起始项目'));
  assert.deepEqual(calls[2], ['node_app_directory_initialize', { sourcePath: '/artificial/node-source', input: { expectedRevision: 'directory-revision-1', name: 'My local project' } }]);
  assert.deepEqual(added, []); assert.equal(calls.length, 3); assert.ok(byText('确认添加此项目'));
  assert.match(document.body.textContent, /尚未添加或运行/);
  await click(byText('确认添加此项目'));
  assert.deepEqual(calls[3], ['node_app_directory_register', { sourcePath: '/artificial/node-source', expectedRevision: 'initialized-revision' }]);
  assert.equal(added.length, 1); assert.equal(calls.length, 4);
});

test('Review-needed folders expose the complete backend prompt, copy it only explicitly and retain manual fallback', async () => {
  const review = directoryReview('needs-review', { issues: ['人工检查问题，平台不据此判定不可适配。'], reviewPrompt: ' 保留首尾空白\n完整说明：请检查本机 /artificial/node-source。\n ' });
  const calls = []; const copied = [];
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { async writeText(value) { copied.push(value); throw new Error('Artificial clipboard denied'); } } });
  await setupDirectory(async (command, args) => {
    calls.push([command, args]);
    if (command === 'choose_path') return review.sourcePath;
    if (command === 'node_app_directory_inspect') return review;
    throw new Error(command);
  });
  await click(byText('选择本地项目文件夹'));
  assert.match(document.body.textContent, /平台尚未判断这个目录是否适配/);
  assert.match(document.body.textContent, /能读取本机目录的 AI/);
  assert.equal(byText('确认添加此项目'), undefined); assert.equal(byText('在空目录生成起始项目…'), undefined);
  const prompt = document.querySelector('[aria-label="给 AI 的检查说明"]'); assert.equal(prompt.readOnly, true); assert.equal(prompt.value, review.reviewPrompt);
  assert.deepEqual(copied, []);
  await click(byText('复制给 AI 检查'));
  assert.deepEqual(copied, [review.reviewPrompt]); assert.match(document.body.textContent, /复制未完成.*手动复制/);
  assert.equal(prompt.value, review.reviewPrompt); assert.equal(document.querySelector('.local-directory-path input').value, review.sourcePath);
  await click(byText('选择完整说明')); assert.equal(prompt.selectionStart, 0); assert.equal(prompt.selectionEnd, review.reviewPrompt.length);
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { async writeText(value) { copied.push(value); } } });
  await click(byText('复制给 AI 检查')); assert.match(document.body.textContent, /完整检查说明已复制/);
  await click(byText('复制目录路径')); assert.equal(copied.at(-1), review.sourcePath);
  assert.equal(calls.length, 2, 'Copying never initializes, registers or sends anything through native APIs');
});

test('Rechecking revokes ready previews and initialization confirmation before reading and never reuses them on failure', async () => {
  for (const state of ['ready', 'empty']) {
    const deferredRead = deferred(); let reads = 0; const calls = [];
    await setupDirectory(async (command, args) => {
      calls.push([command, args]);
      if (command === 'choose_path') return '/artificial/node-source';
      if (command === 'node_app_directory_inspect') return ++reads === 1 ? directoryReview(state) : deferredRead.promise;
      throw new Error(command);
    });
    await click(byText('选择本地项目文件夹'));
    if (state === 'empty') { await click(byText('在空目录生成起始项目…')); await click(document.querySelector('[aria-label="生成起始项目确认"] input')); }
    await click(byText('重新检查'));
    assert.equal(document.querySelector('.node-inspection-preview'), null); assert.equal(document.querySelector('[aria-label="生成起始项目确认"]'), null);
    await act(async () => deferredRead.reject(new Error('Artificial inspection failed')));
    assert.match(document.body.textContent, /Artificial inspection failed/); assert.equal(byText('确认添加此项目'), undefined); assert.equal(byText('确认生成起始项目'), undefined);
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
  await click(byText('选择本地项目文件夹')); const prompt = document.querySelector('textarea').value;
  await click(byText('重新选择文件夹'));
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
  await click(byText('选择本地项目文件夹')); await click(byText('正在选择与检查…')); assert.equal(choices, 1);
  await act(async () => choosing.resolve('/artificial/node-source'));
  await click(byText('确认添加此项目')); assert.equal(byText('确认添加此项目'), undefined);
  await act(async () => registering.reject(new Error('Reviewed directory changed')));
  assert.equal(registrations, 1); assert.equal(byText('确认添加此项目'), undefined); assert.match(document.body.textContent, /Reviewed directory changed/);
  await click(byText('重新检查')); assert.equal(reads, 2); assert.deepEqual(added, []);
});

test('Service changes discard late chooser, inspection, initialization and registration responses without replaying writes', async () => {
  for (const delayedOperation of ['choose', 'inspect', 'initialize', 'register']) {
    const delayed = deferred(); const calls = []; const added = []; const gates = [];
    const props = { enabled: true, instanceId: 'old-directory-service', onPendingChange: value => gates.push(value), onAdded: value => added.push(value) };
    await setupDirectory(async (command, args) => {
      calls.push([command, args]);
      if (command === 'choose_path') return delayedOperation === 'choose' ? delayed.promise : '/artificial/node-source';
      if (command === 'node_app_directory_inspect') return delayedOperation === 'inspect' ? delayed.promise : directoryReview(delayedOperation === 'initialize' ? 'empty' : 'ready');
      if (command === 'node_app_directory_initialize' || command === 'node_app_directory_register') return delayed.promise;
      throw new Error(command);
    }, props);
    await click(byText('选择本地项目文件夹'));
    if (delayedOperation === 'initialize') {
      await click(byText('在空目录生成起始项目…')); await click(document.querySelector('[aria-label="生成起始项目确认"] input')); await click(byText('确认生成起始项目'));
    }
    if (delayedOperation === 'register') await click(byText('确认添加此项目'));
    const submitted = calls.length;
    await act(async () => root.render(h(LocalDirectoryPanel, { ...props, instanceId: 'new-directory-service' })));
    const result = delayedOperation === 'choose' ? '/artificial/late-chooser' : delayedOperation === 'register' ? app({ sourcePath: '/artificial/node-source' }) : directoryReview();
    await act(async () => delayed.resolve(result));
    assert.equal(calls.length, submitted); assert.deepEqual(added, []); assert.equal(gates.at(-1), false);
    assert.equal(byText('确认添加此项目'), undefined); assert.equal(byText('确认生成起始项目'), undefined);
    assert.doesNotMatch(document.body.textContent, /late-chooser/);
    assert.equal(byText(delayedOperation === 'choose' ? '选择本地项目文件夹' : '重新选择文件夹').disabled, false);
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
  await click(byText('选择本地项目文件夹'));
  await act(async () => root.render(h(LocalDirectoryPanel, { ...props, enabled: false })));
  await act(async () => delayed.resolve(directoryReview()));
  assert.equal(byText('重新检查').disabled, true); assert.equal(byText('确认添加此项目'), undefined);
  await act(async () => root.render(h(LocalDirectoryPanel, props)));
  assert.equal(document.querySelector('.local-directory-path input').value, '/artificial/node-source');
  assert.equal(byText('确认添加此项目'), undefined);
  await click(byText('重新检查')); assert.equal(reads, 2); assert.ok(byText('确认添加此项目'));
});

test('Closing local-folder content prevents a late chooser from inspecting or initializing after unmount', async () => {
  const delayed = deferred(); const calls = []; const gates = [];
  await setupDirectory(async (command, args) => { calls.push([command, args]); assert.equal(command, 'choose_path'); return delayed.promise; }, { onPendingChange: value => gates.push(value) });
  await click(byText('选择本地项目文件夹'));
  await act(async () => root.unmount()); root = null;
  await act(async () => delayed.resolve('/artificial/closed-folder'));
  assert.deepEqual(calls, [['choose_path', { kind: 'nodeapp' }]]); assert.equal(gates.at(-1), false);
});

test('An initialization error keeps directory and name, requires reinspection and revokes every prior acknowledgment', async () => {
  let inspections = 0; let initializations = 0;
  await setupDirectory(async command => {
    if (command === 'choose_path') return '/artificial/node-source';
    if (command === 'node_app_directory_inspect') return directoryReview('empty', { expectedRevision: `empty-${++inspections}` });
    if (command === 'node_app_directory_initialize') { initializations++; throw new Error('Artificial initialization needs inspection'); }
    throw new Error(command);
  });
  await click(byText('选择本地项目文件夹')); await change(document.querySelector('.local-directory-empty input'), 'Preserved project name');
  await click(byText('在空目录生成起始项目…')); await click(document.querySelector('[aria-label="生成起始项目确认"] input')); await click(byText('确认生成起始项目'));
  assert.equal(initializations, 1); assert.equal(byText('确认生成起始项目'), undefined); assert.match(document.body.textContent, /Artificial initialization needs inspection/);
  await click(byText('重新检查'));
  assert.equal(document.querySelector('.local-directory-empty input').value, 'Preserved project name');
  await click(byText('在空目录生成起始项目…'));
  assert.equal(document.querySelector('[aria-label="生成起始项目确认"] input').checked, false); assert.equal(initializations, 1);
});

test('Adding rejects malformed or mismatched directory previews instead of making older material actionable', async () => {
  for (const result of [directoryReview('ready', { sourcePath: '/artificial/different-folder' }), directoryReview('ready', { inspection: undefined }), directoryReview('needs-review', { reviewPrompt: '' }), directoryReview('empty', { expectedRevision: '' })]) {
    await setupDirectory(async command => {
      if (command === 'choose_path') return '/artificial/node-source';
      assert.equal(command, 'node_app_directory_inspect'); return result;
    });
    await click(byText('选择本地项目文件夹'));
    assert.ok(document.querySelector('[role=alert]')); assert.equal(byText('确认添加此项目'), undefined); assert.equal(byText('在空目录生成起始项目…'), undefined);
    await act(async () => root.unmount()); root = null;
  }
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
    await click(byText('应用配置'));
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
    await click(byText('重新读取配置')); await click(byText('丢弃草稿并重新读取'));
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
    await submit();
    assert.equal(writes.length, 1);
    await act(async () => root.render(h(NodeAppsPanel, { ...props, instanceId: 'new' })));
    assert.equal(input('MODEL').disabled, true);
    assert.equal(byText('保存中…').disabled, true);
    assert.deepEqual(states.at(-1), { dirty: true, pending: true });
    await submit();
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
    await submit();
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
  await submit();
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
    await act(async () => { ticks.at(-1)(); });
    assert.equal(input('MODEL').value, 'my-draft');
    assert.equal(input('AI_KEY').value, 'my-artificial-draft-key');
    await submit();
    assert.equal(calls[0].input.revision, 1);
    assert.equal(calls[0].input.values.MODEL, 'my-draft');
  } finally { window.setInterval = originalInterval; }
});

test('Node detail tabs retain secret drafts and closing asks before discarding them', async () => {
  await setup(baseMock(app(), async (command) => { throw new Error(command); }));
  await change(input('AI_KEY'), 'artificial-unsaved-tab-key');
  await click(byText('概览')); await click(byText('运行状态')); await click(byText('应用配置'));
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
    if (command === 'bootstrap_info') return { displayName: 'Local Projects Dev', version: '0.0.0', identifier: 'dev.localprojects.platform.m0', instanceId: 'instance_dom_fixture', workspaceRoot: '/disposable/workspace', dataRoot: '/disposable/development', resources: { sdk: true, template: true, readme: true, cli: true } };
    if (command === 'project_list') return state.projects ?? [];
    if (command === 'workspace_info') return { workspaceRoot: '/disposable/workspace', migrationState: 'ready', retainedRoots: [] };
    if (command === 'maintenance_status') return { mode: 'ready', recoveryRequired: false };
    if (command === 'node_app_list') return state.nodes ?? [];
    if (command === 'node_app_configuration') return state.nodes.find(value => value.id === args.appId);
    return action(command, args);
  };
}

test('Deleting from a Node launcher action removes the app only after an explicit deleted result', async () => {
  for (const metadataCleanupPending of [false, true]) {
    const value = app(); const state = { nodes: [value] }; const writes = [];
    await setup(shellMock(state, async (command, args) => {
      if (command === 'node_app_delete_preview') return deletionPreview(value, 'external');
      if (command === 'node_app_delete') {
        writes.push(args); state.nodes = [];
        return { appId: value.id, operationOutcome: 'deleted', ownership: 'external', sourcePreserved: true, evidenceRetained: true, metadataCleanupPending };
      }
      throw new Error(command);
    }), {}, App);
    const entry = document.querySelector(`[aria-label='进入项目「${value.name}」']`);
    await act(async () => entry.closest('article').dispatchEvent(new window.MouseEvent('contextmenu', { bubbles: true, cancelable: true })));
    await click(document.querySelector(`[aria-label='删除项目「${value.name}」']`));
    assert.equal(writes.length, 0);
    assert.equal(document.querySelectorAll('.project-card').length, 1);
    await click(document.querySelector('.node-delete-confirmation input')); await click(byText('确认永久删除项目'));
    assert.equal(writes.length, 1); assert.equal(document.querySelectorAll('.project-card').length, 0);
    assert.equal(document.querySelector('[role=dialog]'), null);
    const feedback = document.querySelector(metadataCleanupPending ? '.notice-error' : '.notice-success');
    assert.match(feedback.textContent, /项目已从平台移除.*原目录和文件未改动/);
    assert.doesNotMatch(feedback.textContent, /文件已永久删除/);
    if (metadataCleanupPending) {
      assert.equal(document.querySelector('.notice-success'), null);
      assert.match(feedback.textContent, /收尾尚未完成.*不要再次提交删除/);
    } else assert.doesNotMatch(feedback.textContent, /收尾尚未完成/);
    assert.equal(writes.length, 1);
    await act(async () => root.unmount()); root = null;
  }
});

test('All project types share My Projects, counts and search without a separate Node navigation', async () => {
  const state = { nodes: [app()], projects: [{ id: 'prj_static', name: 'Static fixture', description: 'Artificial webpage', state: 'ready', issues: [], projectRoot: '/artificial/static', manifest: null }] };
  await setup(shellMock(state), {}, App);
  assert.equal(document.querySelectorAll('.project-card').length, 2);
  assert.match(document.querySelector('.radio-footer').textContent, /2 APPS/);
  assert.deepEqual([...document.querySelectorAll('.radio-primary-nav button')].map(value => value.textContent), ['我的项目', '空间与工作区']);
  assert.match(document.querySelector('.project-grid').textContent, /网页项目/);
  assert.match(document.querySelector('.project-grid').textContent, /Node 服务项目/);
  await change(document.querySelector('input[type="search"]'), 'Disposable');
  assert.equal(document.querySelectorAll('.project-card').length, 1);
  assert.match(document.querySelector('.radio-footer').textContent, /1 \/ 2 APPS/);
  await change(document.querySelector('input[type="search"]'), 'no match artificial');
  assert.match(document.querySelector('.project-search-empty').textContent, /没有找到匹配/);
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
  assert.equal(document.querySelector('[role=tab][aria-selected=true]').textContent, '应用配置');
  assert.equal(calls.length, 0);
  await change(input('AI_KEY'), 'artificial-explicit-entry-key'); await submit();
  assert.equal(calls.length, 1); assert.equal(calls[0][0], 'node_app_configuration_save');
  await click([...document.querySelectorAll('[role=dialog] button')].find(value => value.textContent === '进入项目'));
  assert.deepEqual(calls[1], ['node_app_start', { appId: 'node_fixture' }]);
  assert.equal(input('AI_KEY').value, '');
});

test('Node details reopen without a secret and expose only supported tabs and actions', async () => {
  const state = { nodes: [app()] };
  await setup(shellMock(state), {}, App);
  await openDetails('Disposable Node fixture');
  assert.equal(document.querySelector('[role=tab][aria-selected=true]').textContent, '概览');
  assert.deepEqual([...document.querySelectorAll('[role=tab]')].map(value => value.textContent), ['概览', '应用配置', '运行状态']);
  assert.ok(byText('删除项目'));
  assert.equal([...document.querySelectorAll('[role=dialog] button')].some(value => /备份维护/.test(value.textContent)), false);
  await click(byText('应用配置')); await change(input('AI_KEY'), 'artificial-close-draft');
  await click(document.querySelector('[aria-label^="关闭Disposable"]'));
  await click(byText('丢弃草稿并关闭'));
  assert.equal(document.querySelector('[role=dialog]'), null);
  await openDetails('Disposable Node fixture'); await click(byText('应用配置'));
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
  await click(byText('应用配置'));
  assert.equal(input('MODEL').value, '');
});

test('Node tabs support keyboard navigation and discarded reloads use one existing modal', async () => {
  await setup(baseMock(app(), async (command) => { throw new Error(command); }));
  const configurationTab = document.querySelector('[role=tab][aria-selected=true]');
  await act(async () => configurationTab.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true, cancelable: true })));
  assert.equal(document.querySelector('[role=tab][aria-selected=true]').textContent, '运行状态');
  assert.equal(document.activeElement.textContent, '运行状态');
  await click(byText('应用配置')); await change(input('AI_KEY'), 'artificial-reload-draft');
  await click(byText('重新读取配置'));
  assert.equal(document.querySelectorAll('[role=dialog]').length, 1);
  await click(byText('丢弃草稿并重新读取'));
  assert.equal(input('AI_KEY').value, '');
  assert.equal(document.querySelector('[role=tab][aria-selected=true]').textContent, '应用配置');
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
  assert.equal(byText('新建项目').disabled, true);
  const addMenu = document.querySelector('.radio-add-menu');
  const addTrigger = addMenu.querySelector('summary');
  assert.equal(addTrigger.getAttribute('aria-disabled'), 'true');
  assert.equal(byText('导入项目').disabled, true);
  await click(addTrigger);
  assert.equal(addMenu.open, false);
  await act(async () => staticTile.dispatchEvent(new window.MouseEvent('contextmenu', { bubbles: true, cancelable: true })));
  await click(byText('新建项目')); await click(byText('导入项目'));
  assert.equal(document.querySelector('[role=menu]'), null);
  assert.equal(document.querySelector('[role=dialog]'), null);
  await act(async () => configuration.reject(new Error('Artificial configuration read failure')));
  assert.equal(document.querySelectorAll('[role=dialog]').length, 1);
  assert.equal(document.querySelector('[role=dialog] h2').textContent, 'Disposable Node fixture');
  assert.match(document.querySelector('[role=dialog]').textContent, /Artificial configuration read failure/);
});
