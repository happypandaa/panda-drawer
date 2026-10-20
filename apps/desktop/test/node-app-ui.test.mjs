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
  await act(async () => root.render(h(Component, { enabled: true, onDraftStateChange() {}, ...props })));
}
function byText(text, selector = 'button') { return [...document.querySelectorAll(selector)].find((element) => element.textContent === text); }
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
async function submit() { await act(async () => document.querySelector('.node-configuration-form').dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true }))); }
function deferred() { let resolve; let reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { resolve, reject, promise }; }
function baseMock(value, action) { return async (command, args) => { if (command === 'node_app_list') return [value]; if (command === 'node_app_configuration') return value; return action(command, args); }; }

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
  assert.equal(byText('打开应用').disabled, true);
});

test('Opening an app makes one native start request, then returning delegates only its app ID', async () => {
  const accepted = deferred(); const calls = [];
  await setup(baseMock(app(), async (command, args) => { calls.push([command, args]); if (command === 'node_app_start') return accepted.promise; if (command === 'node_app_open') return; throw new Error(command); }));
  assert.equal(byText('打开应用页面'), undefined);
  assert.equal(byText('启动应用'), undefined);
  await click(byText('打开应用')); await click(byText('打开应用'));
  assert.deepEqual(calls, [['node_app_start', { appId: 'node_fixture' }]]);
  await act(async () => accepted.resolve(app({ state: 'running', runningRevision: 1 })));
  await click(byText('返回应用窗口'));
  assert.deepEqual(calls[1], ['node_app_open', { appId: 'node_fixture' }]);
  assert.equal(calls.length, 2);
  assert.equal(byText('打开应用'), undefined);
});

test('Returning to an open application never starts a second instance and gates duplicate pending focus requests', async () => {
  const focus = deferred(); const calls = [];
  await setup(baseMock(app({ state: 'running', runningRevision: 1 }), async (command, args) => {
    calls.push([command, args]); assert.equal(command, 'node_app_open'); return focus.promise;
  }));
  await click(byText('返回应用窗口')); await click(byText('返回应用窗口'));
  assert.deepEqual(calls, [['node_app_open', { appId: 'node_fixture' }]]);
  await act(async () => focus.resolve());
  assert.equal(byText('返回应用窗口').disabled, false);
});

test('Closing an app sends one stop request and returns the manager to its unopened state', async () => {
  const closed = deferred(); const calls = [];
  await setup(baseMock(app({ state: 'running', runningRevision: 1 }), async (command, args) => {
    calls.push([command, args]); assert.equal(command, 'node_app_stop'); return closed.promise;
  }));
  await click(byText('关闭应用')); await click(byText('关闭应用'));
  assert.deepEqual(calls, [['node_app_stop', { appId: 'node_fixture' }]]);
  await act(async () => closed.resolve(app()));
  assert.equal(byText('关闭应用').disabled, true);
  assert.equal(byText('打开应用').disabled, false);
  assert.match(document.body.textContent, /应用已关闭，相关服务已停止/);
});

test('Incomplete required configuration blocks opening an application and identifies the field to finish', async () => {
  await setup(baseMock(app({ configuration: { revision: 1, values: {}, secretStatus: { AI_KEY: 'missing' }, missingRequired: ['AI_KEY'], canStart: false } }), async (command) => { throw new Error(command); }));
  assert.equal(byText('打开应用').disabled, true);
  assert.match(document.querySelector('.node-config-warning').textContent, /请先配置：设置 AI_KEY/);
  await click(byText('打开应用'));
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
  await click(byText('重启并应用配置'));
  assert.deepEqual(calls[1], ['node_app_restart', { appId: 'node_fixture' }]);
  assert.equal(calls.length, 2);
  assert.ok(byText('返回应用窗口'));
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
  });
  await click(byText('添加 Node 应用')); await click(byText('选择应用目录'));
  assert.deepEqual(calls.slice(1), [['choose_path', { kind: 'nodeapp' }], ['node_app_inspect', { sourcePath: '/artificial/node-source' }]]);
  assert.ok(byText('登记此应用')); assert.equal(document.querySelector('[role="dialog"]').textContent.includes('Artificial preview'), true);
  await click(byText('登记此应用'));
  assert.deepEqual(calls.at(-1), ['node_app_register', { sourcePath: '/artificial/node-source' }]);
  assert.equal(document.querySelector('[role="dialog"]'), null);
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

test('Switching applications preserves a draft on cancel and clears it only after explicit discard', async () => {
  const first = app(); const second = app({ id: 'node_second', name: 'Second artificial fixture' });
  await setup(async (command, args) => {
    if (command === 'node_app_list') return [first, second];
    if (command === 'node_app_configuration') return args.appId === second.id ? second : first;
    throw new Error(command);
  });
  await change(input('AI_KEY'), 'artificial-unsubmitted-key');
  await click(document.querySelectorAll('.node-app-list-item')[1]);
  assert.ok(document.querySelector('[role="dialog"]'));
  await click(byText('继续编辑'));
  assert.equal(input('AI_KEY').value, 'artificial-unsubmitted-key');
  await click(document.querySelectorAll('.node-app-list-item')[1]);
  await click(byText('丢弃草稿并切换'));
  assert.equal(input('AI_KEY').value, '');
  assert.equal(document.querySelector('.node-app-heading h2').textContent, second.name);
});

test('Leaving Node settings asks before discarding secrets and a return loads an empty password input', async () => {
  await setup(async (command) => {
    if (command === 'bootstrap_info') return { displayName: 'Local Projects Dev', version: '0.0.0', identifier: 'dev.localprojects.platform.m0', instanceId: 'instance_dom_fixture', workspaceRoot: '/disposable/workspace', dataRoot: '/disposable/development', resources: { sdk: true, template: true, readme: true, cli: true } };
    if (command === 'project_list') return [];
    if (command === 'workspace_info') return { workspaceRoot: '/disposable/workspace', migrationState: 'ready', retainedRoots: [] };
    if (command === 'node_app_list') return [app()];
    if (command === 'node_app_configuration') return app();
    throw new Error(command);
  }, {}, App);
  await click(byText('Node 应用'));
  await change(input('AI_KEY'), 'artificial-navigation-draft');
  await click(byText('关于平台'));
  assert.equal(document.querySelector('[role="dialog"] h2').textContent, '离开应用配置');
  await click(byText('继续编辑'));
  assert.equal(input('AI_KEY').value, 'artificial-navigation-draft');
  await click(byText('关于平台')); await click(byText('丢弃草稿并离开'));
  assert.equal(document.querySelector('.node-apps-panel'), null);
  await click(byText('Node 应用'));
  assert.equal(input('AI_KEY').value, '');
});
