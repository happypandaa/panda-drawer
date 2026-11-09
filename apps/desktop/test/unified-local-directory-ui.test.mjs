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
// These unrelated interaction fixtures have no interrupted Git additions.
await writeFile(mockPath, 'export const invoke = (command, ...args) => command === "source_import_recover" ? Promise.resolve([]) : globalThis.__NODE_FORM_INVOKE__(command, ...args); export const isTauri = () => true; export const getAllWindows = async () => globalThis.__UI_WINDOWS__ ?? []; export const listen = async (name, callback) => { const listeners = globalThis.__NODE_EVENT_LISTENERS__ ??= new Map(); listeners.set(name, callback); return () => listeners.delete(name); };\n');
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
    source = source.replaceAll(/(['"])@tauri-apps\/api\/(?:core|event|window)\1/g, JSON.stringify(pathToFileURL(mockPath).href));
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
const { nodeConfigurationInput } = await component('app-configuration.ts');
const { LocalDirectoryPanel } = await component('LocalDirectoryPanel.tsx');
const { AiToolsProvider, useAiTools } = await component('AiToolsProvider.tsx');
const { setLocale } = await component('i18n.ts');
const { AI_PROMPT_RULES, PROMPT_TEMPLATES } = await component('project-ai-requirements.ts');
const { default: App } = await component('App.tsx');
const { FrontendPlatformProvider, frontendPlatformFromBootstrap } = await component('frontend-platform.tsx');
const { acceptProjectDeletion, observeDeletionRuntime, resetProjectDeletionsForTests } = await component('project-deletion.ts');
const { readRecentProjectFolders, saveRecentProjectFolder, resetRecentProjectFoldersForTests } = await component('local-directory-review.ts');
let root;
afterEach(async () => { if (root) await act(async () => root.unmount()); root = null; resetProjectDeletionsForTests(); resetRecentProjectFoldersForTests(); dom.window.localStorage.clear(); delete navigator.clipboard; globalThis.__NODE_FORM_INVOKE__ = async (command) => { throw new Error(`Unexpected native request: ${command}`); }; });
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
function app(extra = {}) {
 const sourcePath = extra.projectRoot ?? extra.sourcePath ?? '/not-a-real-source';
 return {id: '10000000-0000-4000-8000-000000000001', name: 'Disposable application fixture', sourcePath, projectRoot: sourcePath, hasBackend: true, ownership: 'user', state: 'ready', restartRequired: false, runningRevision: null, lastError: null, configuration: {revision: 1, values: {}, secretStatus: {AI_KEY: 'set'}, missingRequired: [], canStart: true}, fields,
 ...extra, projectRoot: sourcePath, sourcePath, ...(!extra.state && extra.configuration?.missingRequired?.length ? {state: 'needsConfiguration'} : {})};
}

async function setup(mock, props = {}, Component = LocalDirectoryPanel) {
  document.body.innerHTML = '<div id="root"></div>';
  root = createRoot(document.getElementById('root'));
  const { frontendPlatform, withAiTools, ...componentProps } = props;
  globalThis.__NODE_FORM_INVOKE__ = async (command, args) => {
    if (command === 'workspace_storage') return {workspace: {}, otherLocalLocations: []};
    return mock(command, args);
  };
  const content = h(Component, {enabled: true, onPendingChange() {}, onAdded() {}, ...componentProps});
  const rendered = withAiTools ? h(AiToolsProvider, {instanceId: props.instanceId}, content) : content;
  await act(async () => root.render(frontendPlatform ? h(FrontendPlatformProvider, {value: frontendPlatform}, rendered) : rendered));
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
async function saveAndContinueEditing() { await act(async () => document.querySelector('.node-configuration-form').dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true }))); if (!document.querySelector('.node-configuration-form') && byText('修改')) await click(byText('修改')); }
async function showDetails() { if (byText('返回详情')) await click(byText('返回详情')); }
function deferred() { let resolve; let reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { resolve, reject, promise }; }

function directoryReview(state = 'ready', extra = {}) {
  return { sourcePath: '/artificial/node-source', state, expectedRevision: 'directory-revision-1', issues: [], reviewPrompt: state === 'needs-review' ? '完整后端说明\n请先读取 /artificial/node-source 再检查。\n不得直接运行。' : '', ...(state === 'ready' ? { inspection: { runtime: 'node', runtimeLabel: 'Node 服务', app: { id: 'org.example.preview', name: 'Artificial preview', version: '0.1.0' }, dataContract: { id: 'org.example.inspected-data', version: 1 }, configuration: { fields, missingRequired: ['AI_KEY'] } } } : {}), ...extra };
}
async function setupDirectory(mock, props = {}) {
  await setup(mock, { instanceId: 'directory-instance', onPendingChange() {}, onAdded() {}, ...props }, LocalDirectoryPanel);
}

test('Local web registration preserves the folder and directs incomplete entries to details without starting Node', async () => {
  for (const state of ['ready', 'needsBuild', 'entryMissing']) {
    const sourcePath = '/artificial/local-web';
    const value = { id: `prj_web_${state}`, hasBackend: false, ownership: 'user', name: 'Local web fixture', projectRoot: sourcePath, state };
    const added = []; const opened = []; const details = []; const calls = [];
    await setupDirectory(async (command, args) => {
      calls.push([command, args]);
      if (command === 'choose_path') return sourcePath;
      if (command === 'app_directory_inspect') return directoryReview('ready', { sourcePath, inspection: { runtime: 'web', runtimeLabel: '网页应用', app: { name: value.name }, configuration: { fields: [], missingRequired: [] } } });
      if (command === 'app_directory_register') return value;
      throw new Error(command);
    }, { onAdded() { assert.fail('Web registration must use its web callback'); }, onAdded: item => added.push(item), onEnter: item => opened.push(item), onViewDetails: item => details.push(item) });
    await click(byText('选择文件夹…')); await click(byText('添加'));
    assert.deepEqual(added, [value]);
    assert.match(document.body.textContent, /用户文件夹及其中的文件数据保留/);
    assert.match(document.body.textContent, /浏览器数据.*删除应用时清除/);
    assert.deepEqual(opened, []); assert.deepEqual(details, []);
    await click(byText(state === 'ready' ? '打开应用' : '查看应用详情'));
    assert.deepEqual(opened, state === 'ready' ? [value] : []);
    assert.deepEqual(details, state === 'ready' ? [] : [value]);
    assert.deepEqual(calls.map(([command]) => command), ['choose_path', 'app_directory_inspect', 'app_directory_register']);
    await act(async () => root.unmount()); root = null;
  }
});

test('Adding a ready local folder previews first and binds registration to the reviewed directory without starting it', async () => {
  const calls = [];
  await setup(async (command, args) => {
    calls.push([command, args]);
    if (command === 'node_app_list') return [];
    if (command === 'choose_path') return '/artificial/node-source';
    if (command === 'app_directory_inspect') return directoryReview();
    if (command === 'app_directory_register') return app({ projectRoot: '/artificial/node-source' });
    if (command === 'open_project_folder') return;
    throw new Error(command);
  }, { request: null, addRequest: 1 });
  await click(byText('选择文件夹…'));
  assert.deepEqual(calls, [['choose_path', { kind: 'nodeapp' }], ['app_directory_inspect', { sourcePath: '/artificial/node-source' }]]);
  assert.ok(byText('添加')); assert.equal(document.querySelector('.local-directory-panel').textContent.includes('Artificial preview'), true);
  const readyHeading = document.querySelector('.node-inspection-preview h3');
  assert.equal(readyHeading.textContent, '「Artificial preview」可以添加');
  assert.equal(document.activeElement, byText('添加'));
  assert.match(document.getElementById(document.activeElement.getAttribute('aria-describedby')).textContent, /核对下面的项目和文件夹/);
  const copyPath = byText('复制文件夹路径'); copyPath.focus(); await click(copyPath);
  assert.equal(document.activeElement, copyPath, 'A later local operation must not repeatedly pull focus back to the preview');
  await click(byText('添加'));
  assert.deepEqual(calls.at(-1), ['app_directory_register', { sourcePath: '/artificial/node-source', expectedRevision: 'directory-revision-1' }]);
  assert.ok(document.querySelector('.local-directory-panel'));
  assert.match(document.querySelector('.local-directory-panel').textContent, /添加成功/);
  assert.ok(document.querySelector('.local-directory-panel'), 'Adding keeps the result in the original progress dialog');
  assert.equal(document.querySelector('[role=tab]'), null);
  assert.equal(calls.some(([command]) => command === 'node_app_start' || command === 'node_app_configuration'), false, 'Adding does not silently navigate to configuration or open the app');
  await click(byText('打开文件夹'));
  assert.deepEqual(calls.at(-1), ['open_project_folder', { path: '/artificial/node-source' }]);
});

test('Local folder progress completes registration before automatically showing configuration with the success path retained', async () => {
  const reading = deferred(); const adding = deferred(); const enters = []; const completions = []; const registered = [];
  const value = app({ projectRoot: '/artificial/node-source', configuration: { revision: 1, values: {}, secretStatus: {}, missingRequired: ['AI_KEY'], canStart: false } });
  await setupDirectory(async command => {
    if (command === 'choose_path') return '/artificial/node-source';
    if (command === 'app_directory_inspect') return reading.promise;
    if (command === 'app_directory_register') return adding.promise;
    if (command === 'app_configuration') return {...value, manifestDigest: 'reviewed-manifest'};
    throw new Error(command);
  }, { onAdded: item => registered.push(item), onEnter: item => enters.push(item), onClose: () => completions.push(true) });
  const activeStep = () => document.querySelector('[aria-label="添加进度"] [aria-current=step]')?.textContent;
  assert.match(activeStep(), /选择文件夹/);
  await click(byText('选择文件夹…'));
  assert.match(activeStep(), /检查项目/);
  await act(async () => reading.resolve(directoryReview()));
  assert.match(activeStep(), /添加应用/);
  assert.deepEqual(registered, []);
  await click(byText('添加'));
  assert.match(activeStep(), /添加应用/);
  assert.deepEqual(enters, []);
  await act(async () => adding.resolve(value));
  assert.deepEqual(registered, [value]);
  assert.match(document.body.textContent, /添加成功/);
  assert.equal(document.querySelectorAll('[aria-label="添加进度"] .is-complete').length, 3);
  assert.deepEqual(enters, []); assert.deepEqual(completions, []);
  assert.equal(document.activeElement, document.querySelector('.web-app-configuration h3'));
  assert.ok(document.querySelector('.node-configuration-form'));
  assert.ok(document.body.textContent.includes(value.projectRoot));
  await click(byText('返回添加结果'));
  assert.equal(document.querySelector('.node-configuration-form'), null);
  assert.match(document.body.textContent, /添加成功/);
  assert.deepEqual(enters, []); assert.deepEqual(completions, []);
});

test('Local folder success is revoked after the platform instance changes and cannot open the old app', async () => {
  const enters = [];
  const props = { enabled: true, instanceId: 'old-directory-service', onPendingChange() {}, onAdded() {}, onEnter: value => enters.push(value), onClose() {} };
  await setupDirectory(async command => {
    if (command === 'choose_path') return '/artificial/node-source';
    if (command === 'app_directory_inspect') return directoryReview();
    if (command === 'app_directory_register') return app({ projectRoot: '/artificial/node-source' });
    throw new Error(command);
  }, props);
  await click(byText('选择文件夹…')); await click(byText('添加'));
  assert.ok(byText('打开应用'));
  await act(async () => root.render(h(LocalDirectoryPanel, { ...props, instanceId: 'new-directory-service' })));
  assert.equal(byText('打开应用'), undefined); assert.doesNotMatch(document.body.textContent, /添加成功/);
  assert.deepEqual(enters, []);
  assert.ok(byText('重新检查'));
});

test('An incomplete added app automatically configures once, then returns to its saved success without starting it', async () => {
 const calls = []; const opened = [];
 const value = app({projectRoot: '/artificial/node-source', state: 'needsConfiguration', fields: [field('NAME', 'string', {required: true})], configuration: {revision: 1, values: {}, secretStatus: {}, missingRequired: ['NAME'], canStart: false}});
 const ready = {...value, state: 'ready'};
 const detail = {...value, manifestDigest: 'reviewed-manifest'};
 await setupDirectory(async (command, args) => {
   calls.push([command,args]); if(command==='choose_path') return value.projectRoot;
   if(command==='app_directory_inspect') return directoryReview(); if(command==='app_directory_register') return value;
   if(command==='app_configuration') return detail;
   if(command==='app_configuration_save') {assert.deepEqual(args.input.values, {NAME: 'configured'}); return {...detail, configuration: {revision: 2, values: {NAME: 'configured'}, secretStatus: {}, missingRequired: [], canStart: true}};}
   if(command==='app_list') return [ready]; throw Error(command);
 }, {onEnter: item => opened.push(item)});
 await click(byText('选择文件夹…')); await click(byText('添加'));
 assert.ok(document.querySelector('.node-configuration-form')); assert.deepEqual(opened, []);
 await change(input('NAME'), 'configured');
 await act(async () => document.querySelector('.node-configuration-form').dispatchEvent(new window.Event('submit', {bubbles: true, cancelable: true})));
 assert.equal(document.querySelector('.node-configuration-form'), null);
 assert.match(document.body.textContent, /添加成功/); assert.ok(document.body.textContent.includes(value.projectRoot));
 assert.equal(calls.filter(([command]) => command === 'app_configuration').length, 1);
 assert.equal(calls.some(([command]) => /app_open|app_reload/.test(command)), false); assert.deepEqual(opened, []);
 await click(byText('打开应用')); assert.deepEqual(opened, [ready]);
});

test('An empty folder uses one explicit generate-and-add action with its reviewed revision and name', async () => {
  const calls = []; const added = [];
  await setupDirectory(async (command, args) => {
    calls.push([command, args]);
    if (command === 'choose_path') return '/artificial/node-source';
    if (command === 'app_directory_inspect') return directoryReview('empty');
    if (command === 'app_directory_initialize_and_register') return app({ projectRoot: '/artificial/node-source' });
    if (command === 'open_project_folder') return;
    throw new Error(command);
  }, { onAdded: value => added.push(value) });
  await click(byText('选择文件夹…'));
  const name = document.querySelector('.local-directory-empty input');
  assert.equal(name.value, 'node-source');
  assert.match(document.querySelector('.local-directory-empty').textContent, /生成.*添加/);
  assert.match(document.querySelector('.local-directory-empty').textContent, /文件夹.*保留/);
  assert.ok(document.querySelector('.local-directory-empty').textContent.includes('点击后会在这个空文件夹生成并准备起始应用，再添加到列表。以后删除应用时，这个文件夹和 data/ 会保留，平台配置和浏览器数据会清除。'));
  assert.doesNotMatch(document.querySelector('.local-directory-empty').textContent, /不安装依赖或自动运行/);
  assert.equal(document.querySelector('.local-directory-empty input[type=checkbox]'), null);
  await change(name, ''); assert.equal(byText('在这里新建').disabled, true);
  await change(name, 'My local project');
  assert.equal(calls.length, 2);
  await click(byText('在这里新建'));
  assert.deepEqual(calls[2], ['app_directory_initialize_and_register', { sourcePath: '/artificial/node-source', input: { expectedRevision: 'directory-revision-1', name: 'My local project' } }]);
  assert.equal(added.length, 1); assert.equal(calls.length, 3);
  await click(byText('打开文件夹'));
  assert.deepEqual(calls.at(-1), ['open_project_folder', { path: '/artificial/node-source' }]);
  assert.equal(byText('添加'), undefined, 'Successful generation and adding must not request a second add confirmation');
  assert.equal(calls.some(([command]) => ['app_directory_initialize', 'app_directory_register', 'node_app_start'].includes(command)), false);
});

test('A newly generated folder copies a create prompt while ordinary registered folders keep their existing result', async () => {
  const calls = []; const copied = [];
  const value = app({ projectRoot: '/artificial/node-source' });
  const handoff = { projectRoot: value.sourcePath, instanceId: 'directory-instance', text: '我想在熊猫小抽屉里做一个新应用' };
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { async writeText(text) { copied.push(text); } } });
  await setupDirectory(async (command, args) => {
    calls.push([command, args]);
    if (command === 'choose_path') return value.sourcePath;
    if (command === 'app_directory_inspect') return directoryReview('empty');
    if (command === 'app_directory_initialize_and_register') return value;
    if (command === 'app_handoff') return handoff;
    throw new Error(command);
  }, { frontendPlatform: frontendPlatformFromBootstrap({ build: { target: 'x86_64-pc-windows-msvc' } }) });
  await click(byText('选择文件夹…')); await click(byText('在这里新建'));
  assert.ok(byText('和你的 AI 一起做出来', 'h3'));
  await click(byText('只复制提示词'));
  assert.deepEqual(calls.at(-1), ['app_handoff', { appId: value.id, purpose: 'create' }]);
  assert.deepEqual(copied, [handoff.text]);
  assert.equal(byLabel('给 AI 的完整说明').value, handoff.text);

  await act(async () => root.unmount()); root = null;
  await setupDirectory(async command => {
    if (command === 'choose_path') return value.sourcePath;
    if (command === 'app_directory_inspect') return directoryReview('ready');
    if (command === 'app_directory_register') return value;
    throw new Error(command);
  });
  await click(byText('选择文件夹…')); await click(byText('添加'));
  assert.equal(byText('和你的 AI 一起做出来', 'h3'), undefined);
  assert.equal(document.querySelector('.detail-handoff'), null);
});

test('A new local app passes create purpose to native AI opening and its manual fallback', async () => {
  const calls = []; const copied = [];
  const value = app({ projectRoot: '/artificial/node-source' });
  window.localStorage.setItem('panda-drawer:ai-tools:v1', JSON.stringify({ acknowledged: true, selectedToolId: 'codex', hiddenGuides: [] }));
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { async writeText(text) { copied.push(text); } } });
  await setupDirectory(async (command, args) => {
    calls.push([command, args]);
    if (command === 'detect_ai_tools') return [];
    if (command === 'choose_path') return value.sourcePath;
    if (command === 'app_directory_inspect') return directoryReview('empty');
    if (command === 'app_directory_initialize_and_register') return value;
    if (command === 'open_ai_handoff') throw new Error('Artificial native open failure');
    if (command === 'app_handoff') return { projectRoot: value.sourcePath, instanceId: 'directory-instance', text: 'Create prompt for a new local app' };
    if (command === 'ai_handoff_show_manager') return;
    throw new Error(command);
  }, { withAiTools: true, frontendPlatform: frontendPlatformFromBootstrap({ build: { target: 'aarch64-apple-darwin' } }) });
  await click(byText('选择文件夹…')); await click(byText('在这里新建')); await click(byText('用 Codex 开始做'));
  assert.deepEqual(calls.find(([command]) => command === 'open_ai_handoff'), ['open_ai_handoff', { toolId: 'codex', projectId: null, appId: value.id, purpose: 'create', text: null, sourcePath: null }]);
  assert.deepEqual(calls.find(([command]) => command === 'app_handoff'), ['app_handoff', { appId: value.id, purpose: 'create' }]);
  assert.deepEqual(copied, ['Create prompt for a new local app']);
});

test('Titlebar AI handoff carries its window generation and an expired window cannot prepare or copy a fallback', async () => {
  const calls = []; const copied = []; const value = app();
  window.localStorage.setItem('panda-drawer:ai-tools:v1', JSON.stringify({ acknowledged: true, selectedToolId: 'codex', hiddenGuides: [] }));
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { async writeText(text) { copied.push(text); } } });
  function AiStatus() { const ai = useAiTools(); return h('p', {id: 'ai-state'}, `${ai.busy}|${ai.notice}`); }
  await setup(async (command, args) => {
    calls.push([command, args]);
    if (command === 'detect_ai_tools') return [];
    assert.equal(command, 'open_ai_handoff', 'An expired titlebar handoff cannot open the manager or prepare a fallback');
    if (args.expectedWindowGeneration === 82) throw {code: 'APP_WINDOW_ACTION_EXPIRED', message: 'Closed window'};
    return {toolId: 'codex', toolName: 'Codex', oneClick: true, opened: true, projectRoot: value.sourcePath, instanceId: 'directory-instance', appName: value.name, text: 'Artificial native handoff'};
  }, {instanceId: 'directory-instance', withAiTools: true, frontendPlatform: frontendPlatformFromBootstrap({build: {target: 'aarch64-apple-darwin'}})}, AiStatus);
  const handoff = globalThis.__NODE_EVENT_LISTENERS__.get('ai-handoff-request');
  assert.equal(typeof handoff, 'function');
  await act(async () => handoff({payload: {appId: value.id, appName: value.name, windowGeneration: 81}}));
  assert.equal(calls.find(([command]) => command === 'open_ai_handoff')[1].expectedWindowGeneration, 81);
  assert.deepEqual(copied, ['Artificial native handoff']);
  await act(async () => handoff({payload: {appId: value.id, appName: value.name, windowGeneration: 82}}));
  assert.deepEqual(copied, ['Artificial native handoff']);
  assert.equal(document.getElementById('ai-state').textContent, 'false|');
  assert.equal(document.querySelector('[role=dialog]'), null);
  await act(async () => handoff({payload: {appId: value.id, appName: value.name}}));
  assert.equal(Object.hasOwn(calls.at(-1)[1], 'expectedWindowGeneration'), false, 'An ordinary app AI request retains its existing API');
  assert.equal(copied.length, 2);
});

test('Mac create handoffs copy and preview the complete English scaffold', async () => {
  const copied = [];
  const value = app({ name: '中文应用 {instructions}', sourcePath: '/artificial/中文/{skillPath}' });
  const values = { nameLine: `应用：${value.name}\n`, descriptionLine: '', projectRoot: value.sourcePath, skillPath: '/artificial/平台说明/{projectRoot}/SKILL.md', cliPath: '/artificial/panda-cli' };
  const text = PROMPT_TEMPLATES.create.replace(/\{([A-Za-z][A-Za-z0-9_]*)\}/g, (whole, name) => AI_PROMPT_RULES[name] ?? values[name] ?? whole);
  window.localStorage.setItem('panda-drawer:ai-tools:v1', JSON.stringify({ acknowledged: true, selectedToolId: 'codex', hiddenGuides: [] }));
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { async writeText(text) { copied.push(text); } } });
  await setupDirectory(async (command, args) => {
    if (command === 'set_ui_locale') return args.locale;
    if (command === 'detect_ai_tools') return [];
    if (command === 'choose_path') return value.sourcePath;
    if (command === 'app_directory_inspect') return directoryReview('empty', { sourcePath: value.sourcePath });
    if (command === 'app_directory_initialize_and_register') return value;
    if (command === 'app_handoff') return { projectRoot: value.sourcePath, instanceId: 'directory-instance', text };
    if (command === 'open_ai_handoff') return { toolId: 'codex', toolName: 'Codex', oneClick: true, opened: false, projectRoot: value.sourcePath, instanceId: 'directory-instance', appName: value.name, text: args.text };
    if (command === 'ai_handoff_show_manager') return;
    throw new Error(command);
  }, { withAiTools: true, frontendPlatform: frontendPlatformFromBootstrap({ build: { target: 'aarch64-apple-darwin' } }) });
  try {
    await act(async () => setLocale('en'));
    await click(document.querySelector('.local-directory-dropzone button'));
    await click(document.querySelector('.local-directory-empty .primary-button'));
    await click(document.querySelector('.detail-handoff .ai-handoff-actions .refresh-button'));
    assert.equal(copied.length, 1);
    assert.match(copied[0], /^I want to make a new app in Panda Drawer/);
    assert.ok(copied[0].includes(`Application: ${value.name}`));
    assert.ok(copied[0].includes(value.sourcePath));
    assert.ok(copied[0].includes(values.skillPath));
    assert.doesNotMatch(copied[0], /修改前先用 Git|需要 Node、npm|暂时不要写代码/);
    assert.equal(document.querySelector('.handoff-preview textarea').value, copied[0]);
    // App-supplied content stays verbatim, even if it happens to match a platform scaffold.
    await act(async () => globalThis.__NODE_EVENT_LISTENERS__.get('ai-handoff-request')({ payload: { projectId: 'prj_disposable', appName: value.name, text } }));
    assert.equal(copied.at(-1), text);
    assert.equal(document.querySelector('.ai-tool-guide textarea').value, text);
  } finally { await act(async () => setLocale('zh-CN')); }
});

test('Review-needed folders copy the shared contextual AI prompt only explicitly and retain manual fallback', async () => {
  const review = directoryReview('needs-review', { issues: ['人工检查问题，平台不据此判定不可适配。'], reviewPrompt: ' 保留首尾空白\n完整说明：请检查本机 /artificial/node-source。\n ' });
  const calls = []; const copied = [];
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { async writeText(value) { copied.push(value); throw new Error('Artificial clipboard denied'); } } });
  await setupDirectory(async (command, args) => {
    calls.push([command, args]);
    if (command === 'choose_path') return review.sourcePath;
    if (command === 'app_directory_inspect') return review;
    if (command === 'source_review_capabilities') return { codexAvailable: false };
    throw new Error(command);
  });
  await click(byText('选择文件夹…'));
  assert.match(document.body.textContent, /需要修改后才能添加/);
  assert.match(document.body.textContent, /文件夹保持原样/);
  assert.equal(byText('添加'), undefined); assert.equal(byText('在这里新建'), undefined);
  const retained = document.querySelector('[aria-label="给 AI 的检查说明"]');
  assert.equal(retained.readOnly, true); assert.equal(retained.value, review.reviewPrompt);
  assert.deepEqual(copied, []);
  await click(byText('交给 AI 处理'));
  assert.equal(copied.length, 1);
  assert.match(copied[0], /\/artificial\/node-source/); assert.match(copied[0], /人工检查问题/);
  assert.match(document.body.textContent, /Artificial clipboard denied/);
  const preview = document.querySelector('.project-ai-preview textarea');
  assert.equal(preview.value, copied[0]); assert.equal(preview.readOnly, true);
  await click(byText('选择完整说明')); assert.equal(preview.selectionStart, 0); assert.equal(preview.selectionEnd, preview.value.length);
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { async writeText(value) { copied.push(value); } } });
  await click(byText('复制完整说明')); assert.match(document.body.textContent, /已复制/);
  await click(document.querySelector('[aria-label="关闭把问题交给 AI"]'));
  await click(byText('复制文件夹路径')); assert.equal(copied.at(-1), review.sourcePath);
  assert.equal(calls.some(([command]) => /initialize|register|start|send/.test(command)), false, 'Copying never initializes, registers, starts or sends the project');
});

test('Rechecking revokes ready previews and empty-folder actions before reading and never reuses them on failure', async () => {
  for (const state of ['ready', 'empty']) {
    const deferredRead = deferred(); let reads = 0; const calls = [];
    await setupDirectory(async (command, args) => {
      calls.push([command, args]);
      if (command === 'choose_path') return '/artificial/node-source';
      if (command === 'app_directory_inspect') return ++reads === 1 ? directoryReview(state) : deferredRead.promise;
      throw new Error(command);
    });
    await click(byText('选择文件夹…'));
    if (state === 'empty') assert.equal(byText('在这里新建').disabled, false);
    await click(byText('重新检查'));
    assert.equal(document.querySelector('.node-inspection-preview'), null); assert.equal(document.querySelector('.local-directory-empty'), null);
    await act(async () => deferredRead.reject(new Error('Artificial inspection failed')));
    assert.match(document.body.textContent, /Artificial inspection failed/); assert.equal(byText('添加'), undefined); assert.equal(byText('在这里新建'), undefined);
    assert.equal(document.querySelector('.local-directory-path input').value, '/artificial/node-source');
    assert.equal(calls.length, 3);
    await act(async () => root.unmount()); root = null;
  }
});

test('Cancelling a replacement folder chooser preserves the path and prompt but revokes its old actionable preview', async () => {
  let chooses = 0; let inspections = 0;
  await setupDirectory(async command => {
    if (command === 'choose_path') return ++chooses === 1 ? '/artificial/node-source' : null;
    if (command === 'app_directory_inspect') { inspections++; return directoryReview('needs-review'); }
    throw new Error(command);
  });
  await click(byText('选择文件夹…')); const prompt = document.querySelector('textarea').value;
  await click(byText('换一个'));
  assert.match(document.body.textContent, /已取消选择/); assert.equal(document.querySelector('.local-directory-path input').value, '/artificial/node-source');
  assert.equal(document.querySelector('textarea').value, prompt); assert.equal(inspections, 1);
  await click(byText('重新检查')); assert.equal(inspections, 2);
});

test('Pending local-folder actions reject duplicates and adding does not reuse a failed revision', async () => {
  const choosing = deferred(); const registering = deferred(); let registrations = 0; let reads = 0; let choices = 0; const added = [];
  await setupDirectory(async command => {
    if (command === 'choose_path') { choices++; return choosing.promise; }
    if (command === 'app_directory_inspect') return directoryReview('ready', { expectedRevision: `review-${++reads}` });
    if (command === 'app_directory_register') { registrations++; return registering.promise; }
    throw new Error(command);
  }, { onAdded: value => added.push(value) });
  await click(byText('选择文件夹…')); await click(byText('正在选择与检查…')); assert.equal(choices, 1);
  await act(async () => choosing.resolve('/artificial/node-source'));
  await click(byText('添加')); assert.equal(byText('添加'), undefined);
  await act(async () => registering.reject(new Error('Reviewed directory changed')));
  assert.equal(registrations, 1); assert.equal(byText('添加'), undefined); assert.match(document.body.textContent, /Reviewed directory changed/);
  await click(byText('重新检查')); assert.equal(reads, 2); assert.deepEqual(added, []);
});

test('Service changes discard late chooser, inspection, combined generation and registration responses without replaying writes', async () => {
  for (const delayedOperation of ['choose', 'inspect', 'initialize', 'register']) {
    const delayed = deferred(); const calls = []; const added = []; const gates = [];
    const props = { enabled: true, instanceId: 'old-directory-service', onPendingChange: value => gates.push(value), onAdded: value => added.push(value) };
    await setupDirectory(async (command, args) => {
      calls.push([command, args]);
      if (command === 'choose_path') return delayedOperation === 'choose' ? delayed.promise : '/artificial/node-source';
      if (command === 'app_directory_inspect') return delayedOperation === 'inspect' ? delayed.promise : directoryReview(delayedOperation === 'initialize' ? 'empty' : 'ready');
      if (command === 'app_directory_initialize_and_register' || command === 'app_directory_register') return delayed.promise;
      throw new Error(command);
    }, props);
    await click(byText('选择文件夹…'));
    if (delayedOperation === 'initialize') {
      await click(byText('在这里新建'));
    }
    if (delayedOperation === 'register') await click(byText('添加'));
    const submitted = calls.length;
    await act(async () => root.render(h(LocalDirectoryPanel, { ...props, instanceId: 'new-directory-service' })));
    const result = delayedOperation === 'choose' ? '/artificial/late-chooser' : ['initialize', 'register'].includes(delayedOperation) ? app({ projectRoot: '/artificial/node-source' }) : directoryReview();
    await act(async () => delayed.resolve(result));
    assert.equal(calls.length, submitted); assert.deepEqual(added, []); assert.equal(gates.at(-1), false);
    assert.equal(byText('添加'), undefined); assert.equal(byText('在这里新建'), undefined);
    assert.doesNotMatch(document.body.textContent, /late-chooser/);
    assert.equal(byText(delayedOperation === 'choose' ? '选择文件夹…' : '换一个').disabled, false);
    await act(async () => root.unmount()); root = null;
  }
});

test('Disabled local-folder access invalidates its unfinished review and keeps the selected path for an explicit recheck', async () => {
  const delayed = deferred(); let reads = 0;
  const props = { enabled: true, instanceId: 'same-service', onPendingChange() {}, onAdded() {} };
  await setupDirectory(async command => {
    if (command === 'choose_path') return '/artificial/node-source';
    if (command === 'app_directory_inspect') return ++reads === 1 ? delayed.promise : directoryReview();
    throw new Error(command);
  }, props);
  await click(byText('选择文件夹…'));
  await act(async () => root.render(h(LocalDirectoryPanel, { ...props, enabled: false })));
  await act(async () => delayed.resolve(directoryReview()));
  assert.equal(byText('重新检查').disabled, true); assert.equal(byText('添加'), undefined);
  await act(async () => root.render(h(LocalDirectoryPanel, props)));
  assert.equal(document.querySelector('.local-directory-path input').value, '/artificial/node-source');
  assert.equal(byText('添加'), undefined);
  await click(byText('重新检查')); assert.equal(reads, 2); assert.ok(byText('添加'));
});

test('Closing local-folder content prevents a late chooser from inspecting or initializing after unmount', async () => {
  const delayed = deferred(); const calls = []; const gates = [];
  await setupDirectory(async (command, args) => { calls.push([command, args]); assert.equal(command, 'choose_path'); return delayed.promise; }, { onPendingChange: value => gates.push(value) });
  await click(byText('选择文件夹…'));
  await act(async () => root.unmount()); root = null;
  await act(async () => delayed.resolve('/artificial/closed-folder'));
  assert.deepEqual(calls, [['choose_path', { kind: 'nodeapp' }]]); assert.equal(gates.at(-1), false);
});

test('A generate-and-add error keeps directory and name and needs a fresh empty review before another explicit attempt', async () => {
  let inspections = 0; let initializations = 0;
  const writes = [];
  await setupDirectory(async (command, args) => {
    if (command === 'choose_path') return '/artificial/node-source';
    if (command === 'app_directory_inspect') return directoryReview('empty', { expectedRevision: `empty-${++inspections}` });
    if (command === 'app_directory_initialize_and_register') { initializations++; writes.push(args); throw new Error('Artificial initialization needs inspection'); }
    throw new Error(command);
  });
  await click(byText('选择文件夹…')); await change(document.querySelector('.local-directory-empty input'), 'Preserved project name');
  await click(byText('在这里新建'));
  assert.equal(initializations, 1); assert.equal(byText('在这里新建'), undefined); assert.match(document.body.textContent, /Artificial initialization needs inspection/);
  assert.match(document.querySelector('[role=alert]').textContent, /已生成的文件会保留.*重新检查.*不要重复生成/);
  await click(byText('重新检查'));
  assert.equal(document.querySelector('.local-directory-empty input').value, 'Preserved project name');
  assert.equal(initializations, 1, 'Reinspection must not replay a write');
  await click(byText('在这里新建'));
  assert.equal(initializations, 2);
  assert.deepEqual(writes, [
    { sourcePath: '/artificial/node-source', input: { expectedRevision: 'empty-1', name: 'Preserved project name' } },
    { sourcePath: '/artificial/node-source', input: { expectedRevision: 'empty-2', name: 'Preserved project name' } },
  ]);
});

test('A generated-folder add failure requires reinspection and an explicit ready-folder add without regenerating files', async () => {
  for (const outcome of ['initialized_not_registered', 'initialized_registration_unconfirmed', 'registered_needs_attention']) {
    const calls = []; const added = []; let generated = false; let inspections = 0;
    await setupDirectory(async (command, args) => {
      calls.push([command, args]);
      if (command === 'choose_path') return '/artificial/node-source';
      if (command === 'app_directory_inspect') return directoryReview(generated ? 'ready' : 'empty', { expectedRevision: `after-inspection-${++inspections}` });
      if (command === 'app_directory_initialize_and_register') {
        generated = true;
        throw { code: 'NODE_DIRECTORY_ADD_FAILED', message: '起始项目文件已生成，但添加未能完成或确认；请保留文件并重新检查，不要再次生成或直接重复添加。', details: { sourcePath: '/artificial/node-source', initializationCompleted: true, partialFilesRetained: true, requiresRecheck: true, operationOutcome: outcome } };
      }
      if (command === 'app_directory_register') return app({ projectRoot: '/artificial/node-source' });
      throw new Error(command);
    }, { onAdded: value => added.push(value) });
    await click(byText('选择文件夹…')); await click(byText('在这里新建'));
    assert.deepEqual(added, []);
    assert.match(document.querySelector('[role=alert]').textContent, /文件已生成.*保留文件.*重新检查/);
    assert.equal(document.querySelector('.local-directory-path input').value, '/artificial/node-source');
    assert.equal(byText('在这里新建'), undefined); assert.equal(byText('添加'), undefined);
    assert.deepEqual(calls.map(([command]) => command), ['choose_path', 'app_directory_inspect', 'app_directory_initialize_and_register'], 'An incomplete or uncertain operation must never silently retry its stages');
    await click(byText('重新检查'));
    assert.equal(byText('在这里新建'), undefined, 'The already generated nonempty folder must use the ordinary ready path');
    assert.equal(byText('添加').disabled, false);
    assert.equal(calls.filter(([command]) => command === 'app_directory_initialize_and_register').length, 1);
    assert.deepEqual(added, []);
    await click(byText('添加'));
    assert.deepEqual(calls.at(-1), ['app_directory_register', { sourcePath: '/artificial/node-source', expectedRevision: 'after-inspection-2' }]);
    assert.equal(added.length, 1);
    assert.equal(calls.filter(([command]) => command === 'app_directory_initialize_and_register').length, 1);
    await act(async () => root.unmount()); root = null;
  }
});

test('Unknown or mismatched combined results never claim that the generated folder was added', async () => {
  for (const result of [undefined, app({ projectRoot: '/artificial/different-folder' }), app({ projectRoot: '/artificial/node-source', hasBackend: 'invalid' }), app({ projectRoot: '/artificial/node-source', state: 'unknown' })]) {
    const calls = []; const added = [];
    await setupDirectory(async (command, args) => {
      calls.push([command, args]);
      if (command === 'choose_path') return '/artificial/node-source';
      if (command === 'app_directory_inspect') return directoryReview('empty');
      if (command === 'app_directory_initialize_and_register') return result;
      throw new Error(command);
    }, { onAdded: value => added.push(value) });
    await click(byText('选择文件夹…')); await click(byText('在这里新建'));
    assert.deepEqual(added, []);
    assert.ok(document.querySelector('[role=alert]'));
    assert.match(document.querySelector('[role=alert]').textContent, /重新检查.*不要重复生成/);
    assert.equal(byText('添加'), undefined); assert.equal(byText('在这里新建'), undefined);
    assert.deepEqual(calls.map(([command]) => command), ['choose_path', 'app_directory_inspect', 'app_directory_initialize_and_register']);
    await act(async () => root.unmount()); root = null;
  }
});

test('Adding rejects malformed or mismatched directory previews instead of making older material actionable', async () => {
  for (const result of [directoryReview('ready', { sourcePath: '/artificial/different-folder' }), directoryReview('ready', { inspection: undefined }), directoryReview('needs-review', { reviewPrompt: '' }), directoryReview('empty', { expectedRevision: '' })]) {
    await setupDirectory(async command => {
      if (command === 'choose_path') return '/artificial/node-source';
      assert.equal(command, 'app_directory_inspect'); return result;
    });
    await click(byText('选择文件夹…'));
    assert.ok(document.querySelector('[role=alert]')); assert.equal(byText('添加'), undefined); assert.equal(byText('在这里新建'), undefined);
    await act(async () => root.unmount()); root = null;
  }
});

test('Local add offers a reusable evaluation before choosing a folder and never starts or modifies a project', async () => {
  const calls = [];
  await setupDirectory(async (command, args) => {
    calls.push([command, args]);
    if (command === 'source_review_capabilities') return { codexAvailable: false };
    throw new Error(command);
  });
  assert.equal(document.querySelectorAll('[aria-label="添加进度"] li').length, 3);
  assert.ok(byText('不确定能不能用？先复制说明问问 AI'));
  assert.deepEqual(calls, []);
  await click(byText('不确定能不能用？先复制说明问问 AI'));
  assert.ok(byText('复制评估说明'));
  assert.match(document.body.textContent, /不填写时.*当前对话/);
  assert.deepEqual(calls.map(([command]) => command), ['source_review_capabilities']);
});

test('A previously registered external folder opens the existing application without inspection or a second registration', async () => {
  const selected = app({ projectRoot: '/artificial/node-source' });
  const calls = []; const enters = []; const details = [];
  await setupDirectory(async (command, args) => {
    calls.push([command, args]);
    if (command === 'choose_path') return selected.sourcePath;
    throw new Error(command);
  }, { apps: [selected], onEnter: value => enters.push(value), onViewDetails: value => details.push(value) });
  await click(byText('选择文件夹…'));
  assert.match(document.body.textContent, /这个文件夹已经添加过了/);
  assert.equal(byText('添加'), undefined);
  assert.deepEqual(calls.map(([command]) => command), ['choose_path']);
  await click(byText('查看应用详情')); assert.deepEqual(details, [selected]);
  await click(byText(`打开「${selected.name}」`)); assert.deepEqual(enters, [selected]);
  assert.equal(calls.some(([command]) => /inspect|register|start/.test(command)), false);
});

test('A native directory-identity match also prevents duplicate addition when its path differs', async () => {
  const selected = app({ id: 'existing_alias', sourcePath: '/artificial/original-source' });
  const calls = []; const enters = [];
  await setupDirectory(async (command, args) => {
    calls.push([command, args]);
    if (command === 'choose_path') return '/artificial/node-source';
    if (command === 'app_directory_inspect') return directoryReview('needs-review', { existingApp: { id: selected.id, name: selected.name } });
    if (command === 'app_list') return [selected];
    throw new Error(command);
  }, { onEnter: value => enters.push(value) });
  await click(byText('选择文件夹…'));
  assert.match(document.body.textContent, /这个文件夹已经添加过了/);
  await click(byText(`打开「${selected.name}」`));
  assert.deepEqual(enters, [selected]);
  assert.deepEqual(calls.at(-1), ['app_list', undefined]);
  assert.equal(calls.some(([command]) => command === 'app_directory_register'), false);
});

test('Local result categories preserve the native uncertainty and never invent an add action', async () => {
  for (const [category, label] of [['needs-changes', '需要修改'], ['unsupported', '当前不适合'], ['insufficient', '信息不足']]) {
    await setupDirectory(async command => {
      if (command === 'choose_path') return '/artificial/node-source';
      if (command === 'app_directory_inspect') return directoryReview('needs-review', { category, issues: ['缺少 local-app.json'] });
      throw new Error(command);
    });
    await click(byText('选择文件夹…'));
    assert.equal(document.querySelector('.local-directory-review .local-directory-result-label').textContent, label);
    assert.match(document.querySelector('.local-directory-location').textContent, /文件夹保持原样/);
    assert.equal(byText('添加'), undefined); assert.equal(byText('在这里新建'), undefined);
    assert.ok(byText('交给 AI 处理'));
    await act(async () => root.unmount()); root = null;
  }
});

test('Recent folders retain only five paths in the same workspace and always perform a fresh inspection', async () => {
  const paths = Array.from({ length: 6 }, (_, index) => `/artificial/recent-folder-${index + 1}`);
  let choices = 0; const reads = [];
  const mock = async (command, args) => {
    if (command === 'choose_path') return paths[choices++];
    if (command === 'app_directory_inspect') { reads.push(args.sourcePath); return directoryReview('needs-review', { sourcePath: args.sourcePath, issues: ['缺少 local-app.json'] }); }
    throw new Error(command);
  };
  await setupDirectory(mock, { workspacePath: '/artificial/recent-workspace', instanceId: 'before-restart' });
  for (let index = 0; index < paths.length; index++) await click(byText(index ? '换一个' : '选择文件夹…'));
  assert.equal(reads.length, 6);
  await act(async () => root.unmount()); root = null;
  await setupDirectory(mock, { workspacePath: '/artificial/recent-workspace', instanceId: 'after-restart' });
  const recent = document.querySelectorAll('.local-directory-recent');
  assert.equal(recent.length, 5);
  assert.equal(recent[0].querySelector('button').title, paths[5]);
  assert.equal(recent[0].querySelector('strong').textContent, 'recent-folder-6');
  assert.doesNotMatch(document.querySelector('.local-directory-recents').textContent, /recent-folder-1/);
  await click(recent[0].querySelector('button'));
  assert.equal(reads.length, 7); assert.equal(reads.at(-1), paths[5]);
  assert.equal(choices, 6, 'Using recent choices should not reopen the native directory picker');
  await act(async () => root.unmount()); root = null;
  await setupDirectory(mock, { workspacePath: '/artificial/different-workspace' });
  assert.equal(document.querySelector('.local-directory-recents'), null);
});

test('A fresh recheck compares actual issues and never reuses the prior revision for adding', async () => {
  let reads = 0; const writes = [];
  await setupDirectory(async (command, args) => {
    if (command === 'choose_path') return '/artificial/node-source';
    if (command === 'app_directory_inspect') return ++reads === 1
      ? directoryReview('needs-review', { issues: ['缺少 local-app.json', '缺少构建结果'] })
      : directoryReview('ready', { expectedRevision: 'fresh-after-adaptation', issues: [] });
    if (command === 'app_directory_register') { writes.push(args); return app({ projectRoot: '/artificial/node-source' }); }
    throw new Error(command);
  });
  await click(byText('选择文件夹…')); await click(byText('重新检查'));
  assert.match(document.querySelector('.local-directory-diff').textContent, /已解决 2 项，还剩 0 项/);
  await click(byText('添加'));
  assert.deepEqual(writes, [{ sourcePath: '/artificial/node-source', expectedRevision: 'fresh-after-adaptation' }]);
});

test('Input conversion refuses unsafe integers and preserves an explicit false value', () => {
  const value = app();
  assert.throws(() => nodeConfigurationInput(value, { ordinary: { LIMIT: '9007199254740993' }, secrets: {} }), /精确保存的整数/);
  assert.deepEqual(nodeConfigurationInput(value, { ordinary: { LIMIT: '0', ENABLED: 'false', MODEL: ' ' }, secrets: {} }), { revision: 1, values: { LIMIT: 0, ENABLED: false }, secrets: { AI_KEY: { action: 'keep' } } });
});

function removedFixture(extra = {}) { return { version: 1, kind: 'app', id: 'node_deleted_A', instanceId: 'directory-instance', taskIds: ['task_deleted_A'], sourceIds: [], sourceRoots: ['/artificial/deleted-A'], ...extra }; }
test('A deletion notice revokes a pending local inspection and cannot recreate recent paths from its late reply', async () => {
  observeDeletionRuntime('directory-instance'); const inspection = deferred();
  await setupDirectory(async command => { if (command === 'choose_path') return '/artificial/deleted-A'; if (command === 'app_directory_inspect') return inspection.promise; throw new Error(command); });
  await click(byText('选择文件夹…'));
  await act(async () => acceptProjectDeletion(removedFixture()));
  await act(async () => inspection.resolve(directoryReview('needs-review', { sourcePath: '/artificial/deleted-A', issues: ['Private stale A issue'], reviewPrompt: 'Private stale A prompt' })));
  assert.doesNotMatch(document.body.textContent, /deleted-A|Private stale A/);
  assert.equal(window.localStorage.length, 0);
});
test('Session recent folders forget only their deleted owner and never persist into a new manager context', async () => {
  observeDeletionRuntime('directory-instance'); const scope = '/artificial/recent-owned-workspace';
  for (const owner of ['A', 'B']) saveRecentProjectFolder(scope, { path: `/artificial/deleted-${owner}`, selectedAt: new Date().toISOString(), result: '已添加', issues: [], ownerId: `node_deleted_${owner}` });
  await setupDirectory(async command => { throw new Error(command); }, { workspacePath: scope });
  assert.equal(document.querySelectorAll('.local-directory-recent').length, 2);
  await act(async () => acceptProjectDeletion(removedFixture()));
  assert.deepEqual(readRecentProjectFolders(scope).map(item => item.ownerId), ['node_deleted_B']);
  assert.match(document.querySelector('.local-directory-recents').textContent, /deleted-B/);
  assert.doesNotMatch(document.querySelector('.local-directory-recents').textContent, /deleted-A/);
  assert.equal(window.localStorage.length, 0);
  await act(async () => root.unmount()); root = null; resetRecentProjectFoldersForTests();
  await setupDirectory(async command => { throw new Error(command); }, { workspacePath: scope });
  assert.equal(document.querySelector('.local-directory-recents'), null);
});

test('Configuration submission carries the displayed manifest revision', () => {
 const value=app({manifestDigest:'a'.repeat(64)});const input=nodeConfigurationInput(value,{ordinary:{},secrets:{}});assert.equal(input.manifestDigest,value.manifestDigest);
});
