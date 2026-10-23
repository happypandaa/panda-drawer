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
const { default: App } = await component('App.tsx');
let root;
afterEach(async () => { if (root) await act(async () => root.unmount()); root = null; globalThis.__NODE_FORM_INVOKE__ = async (command) => { throw new Error(`Unexpected native request: ${command}`); }; });
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
function exposedText(element) {
  function text(node) {
    if (!node) return '';
    if (node.nodeType === window.Node.TEXT_NODE) return node.textContent;
    if (!(node instanceof HTMLElement) || node.hidden || getComputedStyle(node).display === 'none') return '';
    if (node.tagName === 'DETAILS' && !node.open) return text(node.querySelector(':scope > summary'));
    return [...node.childNodes].map(text).join(' ');
  }
  return text(element).replace(/\s+/g, ' ').trim();
}
function input(name) { const metadata = fields.find((field) => field.name === name); const label = [...document.querySelectorAll('label')].find((element) => element.textContent === `${metadata.label}${metadata.required ? '必填' : '选填'}`); assert.ok(label, name); return document.getElementById(label.htmlFor); }
async function change(element, value) {
  assert.ok(element);
  await act(async () => {
    const prototype = element.tagName === 'SELECT' ? window.HTMLSelectElement.prototype : window.HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(prototype, 'value').set.call(element, value);
    element.dispatchEvent(new window.Event(element.tagName === 'SELECT' ? 'change' : 'input', { bubbles: true }));
  });
}
async function click(element) { assert.ok(element); await act(async () => element.click()); }
async function requestUpdate() {
  const disclosure = document.querySelector('.node-update-details');
  assert.ok(disclosure);
  if (!disclosure.open) await click(disclosure.querySelector('summary'));
  await click(byText('选择更新来源或查看候选'));
}
async function openDetails(name) {
  const entry = document.querySelector(`[aria-label='进入项目「${name}」']`);
  assert.ok(entry);
  await act(async () => entry.closest('article').dispatchEvent(new window.MouseEvent('contextmenu', { bubbles: true, cancelable: true })));
  await click(document.querySelector(`[aria-label='查看项目「${name}」的详情']`));
}
async function submit() { await act(async () => document.querySelector('.node-configuration-form').dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true }))); }
function deferred() { let resolve; let reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { resolve, reject, promise }; }
function baseMock(value, action) { return async (command, args) => { if (command === 'node_app_list') return [value]; if (command === 'node_app_configuration') return value; return action(command, args); }; }

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
  await requestUpdate();
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

test('Managed update rejects an old instance source list and releases its pending gate for a fresh read', async () => {
  const { value, source } = managedInstallFixture(); const older = deferred(); let reads = 0;
  const props = { enabled: true, instanceId: 'old', onDraftStateChange() {}, request: { app: value, intent: 'details', token: 1 } };
  await setup(baseMock(value, async command => {
    if (command === 'source_list') return ++reads === 1 ? older.promise : [source];
    if (command === 'node_install_list') return { candidates: [], issues: [] };
    throw new Error(command);
  }), props);
  await click(byText('概览')); await requestUpdate();
  assert.equal(byText('选择更新来源或查看候选').disabled, true);
  await act(async () => root.render(h(NodeAppsPanel, { ...props, instanceId: 'new' })));
  assert.equal(byText('选择更新来源或查看候选').disabled, false);
  await requestUpdate();
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
  await click(byText('概览')); await requestUpdate();
  await change(document.querySelector('.node-install-panel select'), source.sourceId);
  await click(byText('准备安装材料'));
  assert.ok(byText('取消准备'));
  const disclosure = document.querySelector('.node-update-details');
  assert.equal(disclosure.open, true);
  await click(disclosure.querySelector('summary'));
  assert.equal(disclosure.open, true, 'An active update cannot be folded away');
  assert.match(exposedText(disclosure), /取消准备/);
  await click(byText('应用配置')); await change(input('MODEL'), 'unsaved model'); await change(input('AI_KEY'), 'artificial unsaved key');
  const taskCount = tasks.length;
  await act(async () => root.render(h(NodeAppsPanel, { ...props, instanceId: 'new' })));
  assert.equal(input('MODEL').value, 'unsaved model'); assert.equal(input('AI_KEY').value, 'artificial unsaved key');
  assert.equal(document.querySelector('.node-install-panel'), null); assert.equal(tasks.length, taskCount);
  await change(input('MODEL'), ''); await change(input('AI_KEY'), ''); await click(byText('概览'));
  await requestUpdate();
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
    await click(byText('概览')); await requestUpdate();
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
  await click(byText('概览')); await requestUpdate(); await click(byText('核验已安装结果'));
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
  assert.match(document.body.textContent, /已设置 · 保存值不会显示/);
  assert.match(document.body.textContent, /每个应用的密钥分别保存在本机私有配置文件中，仅当前用户可读写/);
  assert.doesNotMatch(document.body.textContent, /系统凭据库|钥匙串|加密保存/);
  assert.equal(input('MODEL').value, '');
  assert.equal(input('MODEL').placeholder, 'test-model');
  assert.match(document.body.textContent, /留空使用作者默认值/);
  assert.equal(input('ENABLED').tagName, 'SELECT');
  assert.equal(input('LIMIT').type, 'number');
  assert.deepEqual(calls, []);
  await click(byText('打开官方说明'));
  assert.deepEqual(calls, [['node_app_open_help', { appId: 'node_fixture', fieldName: 'AI_KEY' }]]);
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
  await setup(baseMock(app(), async (command, args) => { calls.push([command, args]); return app({ configuration: { revision: 2, values: { MODEL: 'manual-model' }, secretStatus: { AI_KEY: 'set' }, missingRequired: [], canStart: true } }); }));
  await change(input('MODEL'), 'manual-model'); await submit();
  assert.deepEqual(calls[0][1].input.secrets, { AI_KEY: { action: 'keep' } });
  await click(byText('移除此应用的密钥'));
  assert.equal(input('AI_KEY').disabled, true);
  await submit();
  assert.deepEqual(calls[1][1].input.secrets, { AI_KEY: { action: 'unbind' } });
});

test('A changed endpoint needs an explicit rebind, and editing the target again clears that confirmation', async () => {
  const calls = [];
  await setup(baseMock(app(), async (command, args) => { calls.push([command, args]); throw { code: 'ENDPOINT_CHANGED', message: '请确认新的服务地址' }; }));
  await change(input('AI_URL'), 'https://new.example.invalid/api');
  const checkbox = () => document.querySelector('.node-endpoint-confirmation input[type="checkbox"]');
  assert.ok(checkbox()); assert.equal(checkbox().checked, false);
  await submit();
  assert.deepEqual(calls[0][1].input.secrets, { AI_KEY: { action: 'keep' } });
  await click(checkbox()); await submit();
  assert.deepEqual(calls[1][1].input.secrets, { AI_KEY: { action: 'rebind' } });
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
  assert.match(document.querySelector('[id$="-runtime-panel"]').textContent, /已停止/);
  assert.equal(document.querySelector('.workspace-inline-notice.node-inline-message'), null);
});

test('Incomplete required configuration blocks opening an application and identifies the field to finish', async () => {
  await setup(baseMock(app({ configuration: { revision: 1, values: {}, secretStatus: { AI_KEY: 'missing' }, missingRequired: ['AI_KEY'], canStart: false } }), async (command) => { throw new Error(command); }));
  assert.equal(byText('进入项目').disabled, false);
  assert.match(document.querySelector('.node-config-warning').textContent, /请先配置：设置 AI_KEY/);
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

test('Adding a source folder previews first and registration never starts it', async () => {
  const calls = [];
  await setup(async (command, args) => {
    calls.push([command, args]);
    if (command === 'node_app_list') return [];
    if (command === 'choose_path') return '/artificial/node-source';
    if (command === 'node_app_inspect') return { runtime: 'node-service', runtimeLabel: 'Node 服务', app: { name: 'Artificial preview', version: '0.1.0' }, configuration: { fields, missingRequired: ['AI_KEY'] } };
    if (command === 'node_app_register') return app();
    throw new Error(command);
  }, { request: null, addRequest: 1 });
  await click(byText('选择应用目录'));
  assert.deepEqual(calls, [['choose_path', { kind: 'nodeapp' }], ['node_app_inspect', { sourcePath: '/artificial/node-source' }]]);
  assert.ok(byText('登记此应用')); assert.equal(document.querySelector('[role="dialog"]').textContent.includes('Artificial preview'), true);
  await click(byText('登记此应用'));
  assert.deepEqual(calls.at(-1), ['node_app_register', { sourcePath: '/artificial/node-source' }]);
  assert.ok(document.querySelector('[role="dialog"]'));
  assert.equal(document.querySelector('[role=tab][aria-selected=true]').textContent, '应用配置');
  assert.equal(calls.some(([command]) => command === 'node_app_start'), false);
});

test('Input conversion refuses unsafe integers and preserves an explicit false value', () => {
  const value = app();
  assert.throws(() => nodeConfigurationInput(value, { ordinary: { LIMIT: '9007199254740993' }, secrets: {} }), /精确保存的整数/);
  assert.deepEqual(nodeConfigurationInput(value, { ordinary: { LIMIT: '0', ENABLED: 'false', MODEL: ' ' }, secrets: {} }), { revision: 1, values: { LIMIT: 0, ENABLED: false }, secrets: { AI_KEY: { action: 'keep' } } });
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
  assert.equal([...document.querySelectorAll('[role=dialog] button')].some(value => /备份维护/.test(value.textContent)), false);
  assert.ok(byText('删除项目'));
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

function deletionPreview(value = app(), extra = {}) {
  return { appId: value.id, name: value.name, path: `/disposable/node-apps/${value.id}`, externalSourcePath: value.sourcePath, canDelete: true, blockedReasons: [], previewId: 'delete_preview_disposable', git: { state: 'unverified', remoteSync: 'unverified' }, ...extra };
}

test('The compact deletion view keeps one app title and cancellation restores draft and focus', async () => {
  const value = app(); let previews = 0;
  await setup(baseMock(value, async command => {
    assert.equal(command, 'node_app_delete_preview'); previews++; return deletionPreview(value);
  }));
  const dialog = document.querySelector('[role=dialog]');
  assert.equal(exposedText(dialog).split(value.name).length - 1, 1);
  await change(input('MODEL'), 'keep this configuration draft');
  await click(byText('删除项目'));
  assert.equal(dialog.querySelector('h2').textContent, '删除项目？');
  await click(document.querySelector('.project-delete-confirmation input[type=checkbox]'));
  await click(byText('取消'));
  assert.equal(dialog.querySelector('h2').textContent, value.name);
  assert.equal(input('MODEL').value, 'keep this configuration draft');
  assert.equal(document.activeElement, byText('删除项目'));
  await click(byText('删除项目'));
  assert.equal(document.querySelector('.project-delete-confirmation input[type=checkbox]').checked, false);
  assert.equal(previews, 2);
});

test('Node deletion previews the owned folder without needing a readable source and warns about local-only Git', async () => {
  const value = app({ lastError: 'Source is missing' }); const calls = [];
  await setup(async (command, args) => {
    calls.push([command, args]);
    assert.equal(command, 'node_app_delete_preview'); return deletionPreview(value);
  }, { request: { app: value, intent: 'delete', token: 1 } });
  const confirm = document.querySelector('[aria-label="删除 Node 应用确认"]');
  assert.ok(confirm); assert.match(confirm.textContent, /Git/); assert.match(confirm.textContent, /未同步|未提交|未推送/);
  assert.match(confirm.textContent, /远程.*同步|同步.*尚未/);
  assert.match(confirm.textContent, /数据库.*附件.*配置/);
  const dialog = document.querySelector('[role=dialog]');
  assert.equal(dialog.querySelector('h2').textContent, '删除项目？');
  assert.equal(exposedText(dialog).split(value.name).length - 1, 1, 'The application name is shown once');
  assert.equal([...dialog.querySelectorAll('.danger-button')].filter(button => !button.closest('[hidden]')).length, 1);
  assert.equal([...dialog.querySelectorAll('button')].some(button => !button.closest('[hidden]') && button.textContent === '进入项目'), false);
  const disclosure = [...confirm.querySelectorAll('summary')].find(summary => summary.textContent === '查看删除范围')?.parentElement;
  assert.ok(disclosure); assert.equal(disclosure.open, false);
  assert.ok(exposedText(confirm).length < 300, 'The default confirmation stays concise');
  assert.equal(exposedText(confirm).includes(`/disposable/node-apps/${value.id}`), false);
  await click(disclosure.querySelector('summary'));
  assert.equal(disclosure.open, true);
  assert.match(exposedText(disclosure), /数据库.*附件.*配置/);
  assert.equal(confirm.querySelector('.project-delete-path').textContent, `/disposable/node-apps/${value.id}`);
  assert.match(disclosure.textContent, /外部源码.*保留|保留.*外部源码/);
  assert.equal(byText('移入废纸篓').disabled, true);
  assert.deepEqual(calls, [['node_app_delete_preview', { appId: value.id }]]);
  await click(byText('取消'));
  assert.equal(document.querySelector('[aria-label="删除 Node 应用确认"]'), null);
  assert.equal(calls.length, 1);
});

test('Node deletion requires acknowledgement, rejects duplicate submissions, and removes the card only on confirmed success', async () => {
  const value = app(); const result = deferred(); const deleted = []; const pending = []; let submissions = 0;
  await setup(async (command, args) => {
    if (command === 'node_app_delete_preview') return deletionPreview(value);
    assert.equal(command, 'node_app_delete'); assert.deepEqual(args, { appId: value.id, input: { previewId: 'delete_preview_disposable' } }); submissions++; return result.promise;
  }, { request: { app: value, intent: 'delete', token: 1 }, onAppDeleted: id => deleted.push(id), onDraftStateChange: state => pending.push(state.pending) });
  await click(document.querySelector('.project-delete-confirmation input[type=checkbox]'));
  const submitDelete = byText('移入废纸篓'); await click(submitDelete); await click(submitDelete);
  assert.equal(submissions, 1); assert.deepEqual(deleted, []); assert.ok(document.querySelector('[role=dialog]'));
  const recheck = byText('重新核对删除范围'); if (recheck) assert.equal(recheck.disabled, true);
  assert.equal(byText('取消').disabled, true); assert.equal(document.querySelector('.modal-close').disabled, true);
  assert.equal(pending.at(-1), true);
  await act(async () => result.resolve({ appId: value.id, operationOutcome: 'system_trashed' }));
  assert.deepEqual(deleted, [value.id]); assert.equal(document.querySelector('[role=dialog]'), null); assert.equal(pending.at(-1), false);
});

test('Failed Node trash keeps the application visible and requires a new preview rather than an implicit permanent delete', async () => {
  const value = app(); const calls = []; const deleted = []; let previews = 0;
  await setup(async (command) => {
    calls.push(command);
    if (command === 'node_app_delete_preview') return deletionPreview(value, { previewId: `preview_${++previews}` });
    if (command === 'node_app_delete') throw new Error('System trash denied fixture');
    throw new Error(command);
  }, { request: { app: value, intent: 'delete', token: 1 }, onAppDeleted: id => deleted.push(id) });
  await click(document.querySelector('.project-delete-confirmation input[type=checkbox]')); await click(byText('移入废纸篓'));
  assert.match(document.querySelector('[role=dialog]').textContent, /System trash denied fixture/);
  assert.ok(document.querySelector('[role=dialog]')); assert.deepEqual(deleted, []);
  assert.equal(byText('移入废纸篓').disabled, true);
  await click(byText('重新核对删除范围')); assert.equal(previews, 2);
  assert.equal(document.querySelector('.project-delete-confirmation input[type=checkbox]').checked, false);
  assert.deepEqual(calls, ['node_app_delete_preview', 'node_app_delete', 'node_app_delete_preview']);
});

test('A running Node application cannot be deleted until its window and service are stopped', async () => {
  const value = app({ state: 'running' }); const calls = [];
  await setup(async (command) => { calls.push(command); assert.equal(command, 'node_app_delete_preview'); return deletionPreview(value, { canDelete: false, blockedReasons: ['请先关闭应用窗口。'] }); }, { request: { app: value, intent: 'delete', token: 1 } });
  assert.match(document.querySelector('.project-delete-confirmation').textContent, /请先关闭应用窗口/);
  assert.equal(document.querySelector('.project-delete-confirmation input[type=checkbox]').disabled, true);
  assert.equal(byText('移入废纸篓').disabled, true);
  assert.deepEqual(calls, ['node_app_delete_preview']);
});

test('An old deletion preview cannot be confirmed after the platform instance changes', async () => {
  const value = app(); const old = deferred(); let reads = 0;
  const props = { enabled: true, instanceId: 'old', onDraftStateChange() {}, request: { app: value, intent: 'delete', token: 1 } };
  await setup(async command => { assert.equal(command, 'node_app_delete_preview'); return ++reads === 1 ? old.promise : deletionPreview(value, { previewId: 'current_preview' }); }, props);
  await act(async () => root.render(h(NodeAppsPanel, { ...props, instanceId: 'new' })));
  await act(async () => old.resolve(deletionPreview(value, { previewId: 'stale_preview' })));
  assert.equal(byText('移入废纸篓').disabled, true);
  await click(byText('重新核对删除范围'));
  assert.equal(reads, 2); assert.equal(byText('移入废纸篓').disabled, true);
  await click(document.querySelector('.project-delete-confirmation input[type=checkbox]'));
  assert.equal(byText('移入废纸篓').disabled, false);
});

test('The shared project shell removes a Node card only after a confirmed system-trash response', async () => {
  const value = app(); const state = { nodes: [value] }; const result = deferred(); let submissions = 0;
  await setup(shellMock(state, async (command) => {
    if (command === 'node_app_delete_preview') return deletionPreview(value);
    if (command === 'node_app_delete') { submissions++; return result.promise; }
    throw new Error(command);
  }), {}, App);
  const entry = document.querySelector(`[aria-label='进入项目「${value.name}」']`);
  await act(async () => entry.closest('article').dispatchEvent(new window.MouseEvent('contextmenu', { bubbles: true, cancelable: true })));
  await click(document.querySelector(`[aria-label='删除项目「${value.name}」']`));
  await click(document.querySelector('.project-delete-confirmation input[type=checkbox]')); await click(byText('移入废纸篓'));
  assert.equal(submissions, 1); assert.equal(document.querySelectorAll('.project-card').length, 1);
  assert.equal(document.querySelector('.radio-toolbar .primary-button').disabled, true);
  await act(async () => { state.nodes = []; result.resolve({ appId: value.id, operationOutcome: 'system_trashed' }); });
  assert.equal(document.querySelectorAll('.project-card').length, 0); assert.equal(document.querySelector('[role=dialog]'), null);
  assert.equal(document.querySelector('.notice-success'), null);
  await click(document.querySelector('.radio-messages-button'));
  assert.match(document.querySelector('[role=dialog]').textContent, /应用及其本机数据已移入系统废纸篓/);
});


test('Node overview keeps ordinary facts visible and source details and updates folded by default', async () => {
  const { value } = managedInstallFixture();
  await setup(baseMock(value, async command => { throw new Error(command); }), { request: { app: value, intent: 'details', token: 1 } });
  await click(byText('概览'));
  const dialog = document.querySelector('[role=dialog]');
  const visible = exposedText(dialog);
  assert.match(visible, /Node 服务.*已停止/);
  assert.match(visible, /配置完整/);
  assert.match(visible, /受管安装/);
  assert.match(visible, /打开本地目录/);
  assert.doesNotMatch(visible, /项目详情与应用配置|保存配置不会自动打开|在独立应用窗口中使用/);
  assert.doesNotMatch(visible, /not-a-real-source|11111111-1111-4111-8111-111111111111|尚未读取或未能核验/);
  assert.equal(document.querySelector('.node-source-details').open, false);
  assert.equal(document.querySelector('.node-update-details').open, false);
  assert.equal([...dialog.querySelectorAll('button')].filter(button => button.textContent === '删除项目').length, 1);
  await click(document.querySelector('.node-source-details > summary'));
  assert.match(exposedText(dialog), /not-a-real-source/);
  assert.match(exposedText(dialog), /暂不支持 Git 版本管理/);
});

test('External Node overview keeps its source dependency visible without exposing long technical details', async () => {
  await setup(baseMock(app(), async command => { throw new Error(command); }));
  await click(byText('概览'));
  const visible = exposedText(document.querySelector('[role=dialog]'));
  assert.match(visible, /依赖来源目录/);
  assert.match(visible, /运行依赖原目录，请勿移动或删除/);
  assert.doesNotMatch(visible, /not-a-real-source/);
});

test('Opening a Node local directory sends only its identity, preserves drafts and never starts a service', async () => {
  const opened = deferred(); const calls = []; const drafts = [];
  await setup(baseMock(app(), async (command, args) => {
    calls.push([command, args]); assert.equal(command, 'node_app_reveal'); return opened.promise;
  }), { onDraftStateChange: value => drafts.push(value) });
  await change(input('MODEL'), 'unfinished directory draft');
  await change(input('AI_KEY'), 'artificial unsaved directory key');
  await click(byText('打开本地目录')); await click(byText('打开本地目录'));
  assert.deepEqual(calls, [['node_app_reveal', { appId: 'node_fixture' }]]);
  assert.equal(input('MODEL').value, 'unfinished directory draft');
  assert.equal(input('AI_KEY').value, 'artificial unsaved directory key');
  assert.deepEqual(drafts.at(-1), { dirty: true, pending: true });
  assert.equal(byText('进入项目').disabled, true);
  assert.equal(document.querySelector('[aria-label^="关闭Disposable"]').disabled, true);
  await act(async () => opened.resolve());
  assert.deepEqual(drafts.at(-1), { dirty: true, pending: false });
  assert.equal(input('MODEL').value, 'unfinished directory draft');
  assert.equal(input('AI_KEY').value, 'artificial unsaved directory key');
  assert.match(document.querySelector('.node-process-state').textContent, /已停止/);
  assert.equal(byText('打开本地目录').disabled, false);
  assert.equal(calls.length, 1);
});

test('Directory opening failure stays in the Node detail and leaves running state and draft intact', async () => {
  const calls = [];
  await setup(baseMock(app({ state: 'running', runningRevision: 1 }), async (command, args) => {
    calls.push([command, args]); assert.equal(command, 'node_app_reveal'); throw new Error('Artificial directory opening failure');
  }));
  await change(input('MODEL'), 'keep my model');
  await click(byText('打开本地目录'));
  assert.deepEqual(calls, [['node_app_reveal', { appId: 'node_fixture' }]]);
  assert.match(document.querySelector('[role=dialog] [role=alert]').textContent, /Artificial directory opening failure/);
  assert.match(document.querySelector('.node-process-state').textContent, /运行中/);
  assert.equal(input('MODEL').value, 'keep my model');
  assert.equal(byText('进入项目').disabled, false);
});

test('A directory result from an old connection cannot contaminate another selected Node detail', async () => {
  const opened = deferred(); const first = app(); const second = app({ id: 'node_second', name: 'Second directory fixture' });
  const props = { enabled: true, instanceId: 'old', onDraftStateChange() {}, request: { app: first, intent: 'details', token: 1 } };
  await setup(async (command, args) => {
    if (command === 'node_app_configuration') return args.appId === first.id ? first : second;
    assert.equal(command, 'node_app_reveal'); return opened.promise;
  }, props);
  await click(byText('打开本地目录'));
  await act(async () => root.render(h(NodeAppsPanel, { ...props, instanceId: 'new' })));
  assert.equal(byText('打开本地目录').disabled, false);
  await act(async () => root.render(h(NodeAppsPanel, { ...props, instanceId: 'new', request: { app: second, intent: 'details', token: 2 } })));
  await act(async () => opened.reject(new Error('Outdated directory failure')));
  assert.equal(document.querySelector('[role=dialog] h2').textContent, second.name);
  assert.doesNotMatch(document.querySelector('[role=dialog]').textContent, /Outdated directory failure/);
  assert.equal(byText('打开本地目录').disabled, false);
});

test('Node overview offers one quick close action while runtime keeps its detailed controls', async () => {
  await setup(baseMock(app({ state: 'running', runningRevision: 1 }), async command => { throw new Error(command); }));
  await click(byText('概览'));
  assert.match(exposedText(document.querySelector('[role=dialog]')), /关闭应用/);
  assert.equal([...document.querySelectorAll('button')].filter(button => button.textContent === '关闭应用').length, 1);
  await click(byText('运行状态'));
  assert.equal([...document.querySelectorAll('button')].filter(button => button.textContent === '关闭应用').length, 1);
  assert.match(exposedText(document.querySelector('[role=dialog]')), /重启并应用配置/);
});

test('A failed Node update remains expanded with its task result exposed', async () => {
  const { value, source } = managedInstallFixture();
  const failed = { taskId: 'task_failed_update', type: 'nodeInstallPrepare', projectId: null, status: 'failed', final: true, phase: 'dependencies', error: { code: 'DISPOSABLE_FAILURE', message: 'Artificial update failure, current application preserved' } };
  await setup(baseMock(value, async command => {
    if (command === 'source_list') return [source];
    if (command === 'node_install_list') return { candidates: [], issues: [] };
    if (command === 'node_install_prepare') return failed;
    throw new Error(command);
  }), { request: { app: value, intent: 'details', token: 1 } });
  await click(byText('概览')); await requestUpdate();
  await change(document.querySelector('.node-install-panel select'), source.sourceId);
  await click(byText('准备安装材料'));
  const disclosure = document.querySelector('.node-update-details');
  assert.equal(disclosure.open, true);
  await click(disclosure.querySelector('summary'));
  assert.equal(disclosure.open, true);
  assert.match(exposedText(disclosure), /Artificial update failure/);
  assert.match(exposedText(disclosure), /准备应用安装 · 失败/);
  assert.match(document.querySelector('.node-process-state').textContent, /已停止/);
});
