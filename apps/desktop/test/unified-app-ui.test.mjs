// Artificial DOM/DTO regression only: no runtime, native window, credentials,
// physical device, database or real application directory is opened.
import assert from 'node:assert/strict';
import { after, afterEach, test } from 'node:test';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';
import { JSDOM } from 'jsdom';

const repo = fileURLToPath(new URL('../../../', import.meta.url));
await mkdir(path.join(repo, '.test-artifacts'), { recursive: true });
const output = await mkdtemp(path.join(repo, '.test-artifacts/disposable-node-admission-dom-'));
const mock = path.join(output, 'tauri-mock.mjs');
await writeFile(mock, 'export const invoke = (...args) => globalThis.__NODE_ADMISSION_INVOKE__(...args); export const isTauri = () => true; export const listen = async (name, callback) => { globalThis.__APP_EVENTS__ ??= new Map(); globalThis.__APP_EVENTS__.set(name, callback); return () => globalThis.__APP_EVENTS__.delete(name); };\n');
const compiled = new Map();
async function compile(file) {
  if (compiled.has(file)) return compiled.get(file);
  const target = path.join(output, `${path.basename(file).replace(/\.tsx?$/, '')}.mjs`);
  compiled.set(file, target);
  let source = ts.transpileModule(await readFile(file, 'utf8'), { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX }, fileName: file }).outputText;
  for (const match of [...source.matchAll(/from\s+(['"])(\.[^'"]+)\1/g)]) {
    const base = path.resolve(path.dirname(file), match[2]); let dependency;
    for (const candidate of [base, `${base}.tsx`, `${base}.ts`]) {
      try { await readFile(candidate); dependency = candidate; break; } catch { /* Next extension. */ }
    }
    assert.ok(dependency);
    source = source.replaceAll(`${match[1]}${match[2]}${match[1]}`, JSON.stringify(pathToFileURL(await compile(dependency)).href));
  }
  source = source.replaceAll(/(['"])@tauri-apps\/api\/(?:core|event|window)\1/g, JSON.stringify(pathToFileURL(mock).href));
  await writeFile(target, source); return target;
}

const dom = new JSDOM('<!doctype html><html><body></body></html>', { url: 'http://localhost/', pretendToBeVisual: true });
// These interaction fixtures assert Chinese copy; OS defaults are tested separately.
dom.window.localStorage.setItem('panda-drawer:language:v1', 'zh-CN');
for (const name of ['window', 'document', 'Node', 'HTMLElement', 'HTMLInputElement', 'HTMLSelectElement', 'Event', 'MouseEvent', 'KeyboardEvent', 'FocusEvent', 'CustomEvent', 'MutationObserver']) Object.defineProperty(globalThis, name, { value: name === 'window' ? dom.window : dom.window[name], configurable: true });
Object.defineProperty(globalThis, 'navigator', { value: dom.window.navigator, configurable: true });
globalThis.getComputedStyle = dom.window.getComputedStyle.bind(dom.window);
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
const { createElement: h, act } = await import('react');
const { createRoot } = await import('react-dom/client');
const { ProjectCard } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/ProjectCard.tsx'))).href);
const { ProjectOverview } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/ProjectOverview.tsx'))).href);
const { ManagementConfirmation, checkedManagementRequest } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/ManagementConfirmation.tsx'))).href);
let root;
afterEach(async () => { if (root) await act(async () => root.unmount()); root = null; globalThis.__APP_EVENTS__?.clear(); globalThis.__NODE_ADMISSION_INVOKE__ = undefined; });
after(async () => { dom.window.close(); await rm(output, { recursive: true, force: true }); });
const field = (name, extra = {}) => ({ name, label: `设置 ${name}`, type: 'string', required: false, secret: false, description: '', howToGet: [], ...extra });
function app(extra = {}) {
  return { id: '10000000-0000-4000-8000-000000000001', name: '可丢弃·统一应用', description: '', projectRoot: '/disposable/source', hasBackend: false, ownership: 'user', state: 'ready', issues: [],
    manifest: {manifestVersion: 2, app: {name: '可丢弃·统一应用'}, web: {root: '.', entry: 'index.html'}}, ...extra };
}
async function render(Component, props) {
  if (!root) { document.body.innerHTML = '<div id="root"></div>'; root = createRoot(document.getElementById('root')); }
  await act(async () => root.render(h(Component, props)));
}
const button = label => [...document.querySelectorAll('button')].find(value => value.textContent.trim() === label);
async function click(value) { assert.ok(value); await act(async () => value.click()); }
const request = (extra = {}) => ({requestId: 'disposable-request', requester: 'local CLI', action: '删除应用', impact: '删除可丢弃目录', expiresAt: Date.now() + 10000, ...extra});
async function emit(value) { const listener = globalThis.__APP_EVENTS__?.get('panda-management-confirmation'); assert.ok(listener); await act(async () => listener({payload: value})); }

test('One launcher handles optional backend capability with the same actions and application label', async () => {
  for (const hasBackend of [false, true]) {
    let entered = 0;
    await render(ProjectCard, {project: app({hasBackend}), blocked: false, opening: false, deletingDisabled: false, onEnter: () => entered++, onDetails() {}, onDelete() {}});
    const entry = document.querySelector('.launcher-open');
    assert.equal(entry.disabled, false); await click(entry); assert.equal(entered, 1);
    await act(async () => entry.closest('article').dispatchEvent(new window.MouseEvent('contextmenu', {bubbles: true, cancelable: true})));
    assert.equal(document.querySelectorAll('[role=menuitem]').length, 2);
    assert.doesNotMatch(document.querySelector('[role=menu]').textContent, /Node 服务|网页项目/);
  }
});

test('A failed unified runtime is visible and does not turn into a missing configuration claim', async () => {
  await render(ProjectCard, {project: app({webRuntime: {state: 'failed', error: {message: 'Artificial startup failure'}}}), blocked: false, opening: false, deletingDisabled: false, onEnter() {}, onDetails() {}, onDelete() {}});
  assert.match(document.querySelector('article').textContent, /关闭或运行异常/);
  assert.doesNotMatch(document.querySelector('article').textContent, /需要配置/);
});

test('Backend status and log access appear only for an app declaring a backend', async () => {
  const commands = [];
  globalThis.__NODE_ADMISSION_INVOKE__ = async (command, args) => {commands.push([command, args]); if (command === 'workspace_storage') return {workspace: {}, otherLocalLocations: []}; if (command === 'app_logs') return {current: '/disposable/current.log', lines: ['Artificial bounded log']}; throw Error(command);};
  const props = {disabled: false, onTask() {}, onChanged: async () => {}, onClose() {}};
  await render(ProjectOverview, {...props, project: app()}); assert.equal(button('查看'), undefined);
  await render(ProjectOverview, {...props, project: app({hasBackend: true, manifest: {...app().manifest, backend: {entry: 'server.mjs'}}})});
  await click(button('查看')); assert.match(document.querySelector('pre').textContent, /Artificial bounded log/);
  assert.deepEqual(commands.find(([command]) => command === 'app_logs')[1], {appId: app().id});
});

test('A CLI request never approves itself; rejection sends only the request identity and false', async () => {
  const calls = []; globalThis.__NODE_ADMISSION_INVOKE__ = async (...args) => calls.push(args);
  await render(ManagementConfirmation, {instanceId: 'disposable-instance'}); await emit(request());
  assert.equal(calls.length, 0); assert.match(document.querySelector('[role=dialog]').textContent, /local CLI|删除可丢弃目录/);
  await click(button('拒绝')); assert.deepEqual(calls, [['management_request_resolve', {requestId: 'disposable-request', approved: false}]]);
  assert.equal(document.querySelector('[role=dialog]'), null);
});

test('Only an explicit confirmation approves; duplicate events and clicks produce one resolution', async () => {
  const calls = []; let complete;
  globalThis.__NODE_ADMISSION_INVOKE__ = (...args) => {calls.push(args); return new Promise(resolve => {complete = resolve;});};
  await render(ManagementConfirmation, {instanceId: 'disposable-instance'}); const value = request(); await emit(value); await emit(value);
  await click(button('确认执行')); await click(button('确认执行'));
  assert.deepEqual(calls, [['management_request_resolve', {requestId: value.requestId, approved: true}]]);
  await act(async () => complete()); assert.equal(document.querySelector('[role=dialog]'), null);
});

test('Expired or malformed requests cannot become actionable and a service change clears pending requests', async () => {
  assert.equal(checkedManagementRequest(request({expiresAt: Date.now() - 1})), false);
  assert.equal(checkedManagementRequest(request({impact: null})), false);
  const calls = []; globalThis.__NODE_ADMISSION_INVOKE__ = async (...args) => calls.push(args);
  await render(ManagementConfirmation, {instanceId: 'one'}); await emit(request());
  await render(ManagementConfirmation, {instanceId: 'two'});
  assert.equal(document.querySelector('[role=dialog]'), null); assert.deepEqual(calls, []);
});
