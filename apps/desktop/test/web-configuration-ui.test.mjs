// Public web configuration and draft races in jsdom only. No service or native window is opened.
import assert from 'node:assert/strict';
import { after, afterEach, test } from 'node:test';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';
import { JSDOM } from 'jsdom';

const repo = fileURLToPath(new URL('../../../', import.meta.url));
await mkdir(path.join(repo, '.test-artifacts'), { recursive: true });
const output = await mkdtemp(path.join(repo, '.test-artifacts/web-configuration-dom-'));
const mockPath = path.join(output, 'tauri-mock.mjs');
// These unrelated interaction fixtures have no interrupted Git additions.
await writeFile(mockPath, 'export const invoke = (command, ...args) => command === "source_import_recover" ? Promise.resolve([]) : globalThis.__WEB_CONFIGURATION_INVOKE__(command, ...args); export const isTauri = () => true; export const getAllWindows = async () => globalThis.__UI_WINDOWS__ ?? []; export const listen = async (name, callback) => { const listeners = globalThis.__WEB_CONFIGURATION_EVENTS__ ??= new Map(); listeners.set(name, callback); return () => listeners.delete(name); };\n');
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
globalThis.__WEB_CONFIGURATION_INVOKE__ = async (command) => { throw new Error(`Unexpected native request: ${command}`); };
const { createElement: h, act } = await import('react');
const { createRoot } = await import('react-dom/client');

const { ProjectOverview } = await component('ProjectOverview.tsx');
const { default: App } = await component('App.tsx');
const { resetProjectDeletionsForTests } = await component('project-deletion.ts');
let root;
afterEach(async () => { if (root) await act(async () => root.unmount()); root = null; resetProjectDeletionsForTests(); dom.window.localStorage.clear(); globalThis.__UI_WINDOWS__ = []; });
after(async () => { dom.window.close(); await rm(output, { recursive: true, force: true }); });

const fields = [
  { name: 'TITLE', label: '公开名称', type: 'string', required: true },
  { name: 'COUNT', label: '显示数量', type: 'integer' },
  { name: 'ENABLED', label: '启用提醒', type: 'boolean' },
].map(value => ({ secret: false, description: '', howToGet: [], ...value }));
const configuration = (values = { TITLE: 'saved title' }, extra = {}) => ({ revision: 4, values, secretStatus: {}, missingRequired: [], invalidFields: [], canStart: true, ...extra });
const detail = (values, extra = {}) => ({ id: 'prj_web_configuration', name: 'Disposable public configuration', runtime: 'web', projectRoot: '/disposable/web', manifestDigest: 'fixture-manifest-digest', fields, configuration: configuration(values), ...extra });
function project(extra = {}) {
  return { id: 'prj_web_configuration', name: 'Disposable public configuration', description: '', runtime: 'web', ownership: 'user', sourcePreservedOnDelete: true, projectRoot: '/disposable/web', documentEpoch: 'fixture-web-epoch', state: 'ready', issues: [], buildInfo: {},
    manifest: { manifestVersion: 1, runtime: 'web', app: { name: 'Disposable public configuration' }, web: { root: '.', entry: 'index.html' }, configuration: { version: 1, fields } }, ...extra };
}
function deferred() { let resolve; const promise = new Promise(yes => { resolve = yes; }); return { resolve, promise }; }
async function setup(invoke) {
  if (root) await act(async () => root.unmount());
  document.body.innerHTML = '<div id="root"></div>';
  root = createRoot(document.getElementById('root'));
  globalThis.__UI_WINDOWS__ = [];
  globalThis.__WEB_CONFIGURATION_INVOKE__ = invoke;
}
const render = tree => act(async () => root.render(tree));
function overview(props = {}) { return h(ProjectOverview, { project: project(), instanceId: 'service-one', configureRequest: 1, disabled: false, onTask() {}, onChanged: async () => {}, onClose() {}, ...props }); }
const button = text => [...document.querySelectorAll('button')].find(value => value.textContent === text);
function field(label) { const value = [...document.querySelectorAll('.node-field-label')].find(item => item.textContent === label); assert.ok(value, label); return document.getElementById(value.htmlFor); }
async function change(label, value) {
  const control = field(label);
  const prototype = control.tagName === 'SELECT' ? window.HTMLSelectElement.prototype : window.HTMLInputElement.prototype;
  await act(async () => { Object.getOwnPropertyDescriptor(prototype, 'value').set.call(control, value); control.dispatchEvent(new window.Event(control.tagName === 'SELECT' ? 'change' : 'input', { bubbles: true })); });
}
async function click(value) { assert.ok(value); await act(async () => value.click()); }
async function save() { const form = document.querySelector('.node-configuration-form'); assert.ok(form); await act(async () => form.dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true }))); }
function baseInvoke(action) {
  return async (command, args) => command === 'workspace_storage' ? { workspace: {}, nodeApps: [], projects: [], otherLocalLocations: [] } : action(command, args);
}

test('ordinary web page without SDK saves typed public values and never starts an application', async () => {
  const calls = []; let saved = 0;
  await setup(baseInvoke(async (command, args) => {
    calls.push([command, args]);
    if (command === 'web_app_configuration') return detail();
    if (command === 'web_app_configuration_save') return detail(args.input.values, { configuration: { ...configuration(args.input.values), revision: 5 } });
    throw Error(`Unexpected native write: ${command}`);
  }));
  await render(overview({ onChanged: async () => { saved++; } }));
  assert.equal(project().manifest.platform, undefined);
  await change('公开名称', 'my public preference');
  await change('显示数量', '12'); await change('启用提醒', 'false');
  await save();
  const writes = calls.filter(([command]) => command === 'web_app_configuration_save');
  assert.equal(writes.length, 1);
  assert.deepEqual(writes[0][1].input.values, { TITLE: 'my public preference', COUNT: 12, ENABLED: false });
  assert.deepEqual(writes[0][1].input.secrets, {});
  assert.equal(saved, 1);
  assert.equal(document.querySelector('.web-app-configuration'), null, 'Saving returns to details, not an application window');
  assert.deepEqual(calls.map(([command]) => command), ['web_app_configuration', 'web_app_configuration_save']);
});

test('a needs-configuration launcher card enters its form without starting the application', async () => {
  const calls = []; const value = project({ state: 'needsConfiguration' });
  await setup(async (command, args) => {
    calls.push([command, args]);
    if (command === 'bootstrap_info') return { displayName: '熊猫小抽屉', version: '0.0.0', identifier: 'dev.happypandaa.pandadrawer', instanceId: 'web-card-service', workspaceRoot: '/disposable/workspace', dataRoot: '/disposable/development', resources: { sdk: true, template: true, readme: true, cli: true } };
    if (command === 'project_list') return [value];
    if (command === 'workspace_info') return { workspaceRoot: '/disposable/workspace', migrationState: 'ready', retainedRoots: [] };
    if (['node_app_list', 'node_app_delete_tasks'].includes(command)) return [];
    if (command === 'workspace_storage') return { workspace: {}, nodeApps: [], otherLocalLocations: [] };
    if (command === 'maintenance_status') return { mode: 'ready', recoveryRequired: false };
    if (command === 'web_app_configuration') return detail({}, { configuration: configuration({}, { canStart: false, missingRequired: ['TITLE'] }) });
    throw Error(`Unexpected application start or request: ${command}`);
  });
  await render(h(App));
  await click(document.querySelector(`[aria-label='进入项目「${value.name}」']`));
  assert.ok(document.querySelector('.web-app-configuration'));
  assert.equal(field('公开名称').value, '');
  assert.equal(calls.some(([command]) => ['project_open', 'node_app_start', 'web_app_configuration_save'].includes(command)), false);
  assert.equal(calls.filter(([command]) => command === 'web_app_configuration').length, 1);
});

test('service switch keeps the editable draft and locks writes until explicit discard and reread', async () => {
  const calls = []; let reads = 0;
  await setup(baseInvoke(async (command, args) => {
    calls.push([command, args]); assert.equal(command, 'web_app_configuration');
    return detail({ TITLE: ++reads === 1 ? 'original saved title' : 'new service value' });
  }));
  await render(overview()); await change('公开名称', 'draft must survive');
  await render(overview({ instanceId: 'service-two' }));
  assert.equal(field('公开名称').value, 'draft must survive');
  assert.equal(field('公开名称').disabled, true);
  assert.equal(button('保存配置').disabled, true);
  assert.match(document.body.textContent, /当前配置草稿仍保留/);
  assert.equal(reads, 1, 'A service switch must not implicitly reread and replace the draft');
  await save(); assert.equal(calls.length, 1, 'Programmatic submits cannot bypass the scope gate');
  await click(button('放弃草稿并重新读取'));
  assert.equal(reads, 2); assert.equal(field('公开名称').value, 'new service value'); assert.equal(field('公开名称').disabled, false);
});

test('a late save response from the previous service preserves the draft and stays on the form', async () => {
  const reply = deferred(); let refreshed = 0; let writes = 0;
  await setup(baseInvoke(async (command) => {
    if (command === 'web_app_configuration') return detail();
    assert.equal(command, 'web_app_configuration_save'); writes++; return reply.promise;
  }));
  const props = { onChanged: async () => { refreshed++; } };
  await render(overview(props)); await change('公开名称', 'unsaved across service switch'); await save();
  await render(overview({ ...props, instanceId: 'service-two' }));
  await act(async () => reply.resolve(detail({ TITLE: 'obsolete response' })));
  assert.equal(field('公开名称').value, 'unsaved across service switch');
  assert.equal(button('保存配置').disabled, true);
  assert.equal(writes, 1); assert.equal(refreshed, 0);
  assert.match(document.body.textContent, /保存结果需要重新核对/);
});

test('switch during post-save refresh does not dismiss the configuration session', async () => {
  const refreshed = deferred(); let refreshStarted = 0;
  await setup(baseInvoke(async (command, args) => {
    if (command === 'web_app_configuration') return detail();
    assert.equal(command, 'web_app_configuration_save');
    return detail(args.input.values, { configuration: { ...configuration(args.input.values), revision: 5 } });
  }));
  const props = { onChanged: () => { refreshStarted++; return refreshed.promise; } };
  await render(overview(props)); await change('公开名称', 'confirmed old-service value'); await save();
  assert.equal(refreshStarted, 1);
  await render(overview({ ...props, instanceId: 'service-two' }));
  await act(async () => refreshed.resolve());
  assert.ok(document.querySelector('.web-app-configuration'), 'A late refresh must not navigate away from the switched session');
  assert.equal(field('公开名称').value, 'confirmed old-service value'); assert.equal(button('保存配置').disabled, true);
  assert.match(document.body.textContent, /当前配置草稿仍保留/);
});
