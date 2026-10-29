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
await writeFile(mock, 'export const invoke = (...args) => globalThis.__NODE_ADMISSION_INVOKE__(...args); export const isTauri = () => true;\n');
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
  source = source.replaceAll(/(['"])@tauri-apps\/api\/core\1/g, JSON.stringify(pathToFileURL(mock).href));
  await writeFile(target, source); return target;
}

const dom = new JSDOM('<!doctype html><html><body></body></html>', { url: 'http://localhost/', pretendToBeVisual: true });
for (const name of ['window', 'document', 'Node', 'HTMLElement', 'HTMLInputElement', 'HTMLSelectElement', 'Event', 'MouseEvent', 'KeyboardEvent', 'FocusEvent', 'CustomEvent', 'MutationObserver']) Object.defineProperty(globalThis, name, { value: name === 'window' ? dom.window : dom.window[name], configurable: true });
Object.defineProperty(globalThis, 'navigator', { value: dom.window.navigator, configurable: true });
globalThis.getComputedStyle = dom.window.getComputedStyle.bind(dom.window);
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
const { createElement: h, act } = await import('react');
const { createRoot } = await import('react-dom/client');
const { NodeAppsPanel } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/NodeAppsPanel.tsx'))).href);
const { NodeProjectCard } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/ProjectCard.tsx'))).href);
let root;
afterEach(async () => { if (root) await act(async () => root.unmount()); root = null; globalThis.__NODE_ADMISSION_INVOKE__ = undefined; });
after(async () => { dom.window.close(); await rm(output, { recursive: true, force: true }); });
const field = (name, extra = {}) => ({ name, label: `设置 ${name}`, type: 'string', required: false, secret: false, description: '', howToGet: [], ...extra });
function app(extra = {}) {
  return { id: 'disposable_node_admission', name: '可丢弃·运行准入界面样本', sourcePath: '/disposable/nonexistent-source', runtime: 'node-service', state: 'stopped', restartRequired: false, runningRevision: null, lastError: null,
    fields: [field('GREETING', { default: '人工默认值' })], configuration: { revision: 1, values: {}, secretStatus: {}, missingRequired: [], canStart: false }, ...extra };
}
async function render(Component, props) {
  document.body.innerHTML = '<div id="root"></div>'; root = createRoot(document.getElementById('root'));
  await act(async () => root.render(h(Component, props)));
}
const button = label => [...document.querySelectorAll('button')].find(value => value.textContent.trim() === label);
async function click(value) { assert.ok(value); await act(async () => value.click()); }
const selectedTab = () => document.querySelector('[role=tab][aria-selected=true]')?.textContent;
const facts = () => document.querySelector('.detail-summary-facts').textContent;
async function panel(value, intent = 'enter') {
  const calls = [];
  globalThis.__NODE_ADMISSION_INVOKE__ = async (command, args) => {
    calls.push([command, args]);
    if (command === 'node_app_configuration' || command === 'node_app_status') return value;
    if (command === 'node_app_start') return { ...value, state: 'running' };
    throw new Error(`Unexpected artificial command: ${command}`);
  };
  await render(NodeAppsPanel, { enabled: true, instanceId: 'disposable-admission-instance', request: { app: value, intent, token: 1 }, onDraftStateChange() {} });
  return calls;
}
function noLaunch(calls) { assert.equal(calls.some(([command]) => ['node_app_start', 'node_app_restart', 'node_app_open'].includes(command)), false); }

test('An unfinished runtime intent is shown as a runtime failure with complete configuration on its launcher', async () => {
  const value = app({ state: 'failed', lastError: '人工保留：上次运行未完成正常关闭，已阻止再次启动。' });
  await render(NodeProjectCard, { app: value, blocked: false, opening: false, onEnter() {}, onDetails() {}, onDelete() {} });
  const card = document.querySelector('article');
  assert.match(card.textContent, /需要检查/);
  assert.match(card.textContent, /上次运行未完成正常关闭/);
  assert.doesNotMatch(card.textContent, /需要配置|缺少 0/);
  await act(async () => card.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true })));
  const menu = document.querySelector('[role=menu]');
  assert.match(menu.textContent, /配置完整/);
  assert.doesNotMatch(menu.textContent, /需要配置|缺少 0/);
});

test('A complete but runtime-blocked entry opens runtime status and cannot issue start/open/restart', async () => {
  const value = app({ state: 'failed', lastError: '人工保留的 active intent 故障' });
  const calls = await panel(value);
  assert.equal(selectedTab(), '运行状态'); assert.match(document.querySelector('[role=dialog]').textContent, /人工保留的 active intent 故障/);
  await click(button('概览')); assert.match(facts(), /已完整（1 项）/); assert.doesNotMatch(facts(), /缺少 0/);
  assert.ok(button('修改配置'));
  await click(button('进入项目')); assert.equal(selectedTab(), '运行状态'); noLaunch(calls);
});

test('A stopped complete configuration with another admission restriction also opens runtime status', async () => {
  const calls = await panel(app()); assert.equal(selectedTab(), '运行状态'); noLaunch(calls);
});

test('Missing required fields still lead to configuration and show their actual positive count', async () => {
  const value = app({ fields: [field('REQUIRED', { required: true })], configuration: { revision: 1, values: {}, secretStatus: {}, missingRequired: ['REQUIRED'], canStart: false } });
  const calls = await panel(value); assert.equal(selectedTab(), '应用配置');
  await click(button('概览')); assert.match(facts(), /缺少 1 项必填/);
  assert.ok(button('修改配置'));
  await click(button('进入项目')); assert.equal(selectedTab(), '应用配置'); noLaunch(calls);
});

test('An optional saved secret with a changed endpoint remains a configuration problem despite no required omissions', async () => {
  const value = app({ fields: [field('API_URL', { type: 'url', default: 'https://example.invalid/api' }), field('API_KEY', { secret: true, endpointField: 'API_URL' })], configuration: { revision: 1, values: {}, secretStatus: { API_KEY: 'endpoint-changed' }, missingRequired: [], canStart: false } });
  const calls = await panel(value); assert.equal(selectedTab(), '应用配置');
  assert.match(document.querySelector('.node-endpoint-confirmation').textContent, /确认密钥的使用目标/);
  await click(button('概览')); assert.match(facts(), /密钥地址需确认/); assert.doesNotMatch(facts(), /已完整|缺少 0/);
  await click(button('进入项目')); noLaunch(calls);
});

test('Invalid saved field formats cannot be mistaken for complete configuration when the required list is empty', async () => {
  const value = app({ fields: [field('API_URL', { type: 'url' })], configuration: { revision: 1, values: { API_URL: 'http://example.invalid/api' }, secretStatus: {}, missingRequired: [], canStart: false } });
  const calls = await panel(value); assert.equal(selectedTab(), '应用配置');
  assert.equal(document.querySelector('input[type=url]').checkValidity(), false);
  await click(button('概览')); assert.match(facts(), /需要核对配置/); assert.doesNotMatch(facts(), /已完整|缺少 0/); noLaunch(calls);
});

test('An unavailable configuration remains unknown and directs the user to the preserved runtime error', async () => {
  const value = app({ state: 'failed', fields: [], lastError: '人工配置读取或来源校验错误', configuration: { unavailable: true, revision: 0, values: {}, secretStatus: {}, missingRequired: [], canStart: false } });
  const calls = await panel(value); assert.equal(selectedTab(), '运行状态');
  assert.match(document.querySelector('[role=dialog]').textContent, /人工配置读取或来源校验错误/);
  await click(button('概览')); assert.match(facts(), /暂不可核对/); assert.doesNotMatch(facts(), /已完整|缺少 0/); noLaunch(calls);
});

test('A blocked optional missing secret is conservatively reviewed without inventing missing required fields', async () => {
  const value = app({ fields: [field('OPTIONAL_KEY', { secret: true })], configuration: { revision: 1, values: {}, secretStatus: { OPTIONAL_KEY: 'missing' }, missingRequired: [], canStart: false } });
  const calls = await panel(value); assert.equal(selectedTab(), '应用配置');
  await click(button('概览')); assert.match(facts(), /配置待核对/); assert.doesNotMatch(facts(), /已完整|缺少 0/); noLaunch(calls);
});

test('A runtime error still takes the entry tab when optional-secret provenance is ambiguous', async () => {
  const value = app({ state: 'failed', lastError: '人工运行故障仍待处理', fields: [field('OPTIONAL_KEY', { secret: true })], configuration: { revision: 1, values: {}, secretStatus: { OPTIONAL_KEY: 'missing' }, missingRequired: [], canStart: false } });
  const calls = await panel(value); assert.equal(selectedTab(), '运行状态');
  await click(button('概览')); assert.match(facts(), /配置待核对/); noLaunch(calls);
});

test('An unset optional secret does not prevent entry when the backend allows starting', async () => {
  const value = app({ fields: [field('OPTIONAL_KEY', { secret: true })], configuration: { revision: 1, values: {}, secretStatus: { OPTIONAL_KEY: 'missing' }, missingRequired: [], canStart: true } });
  const calls = await panel(value);
  assert.equal(calls.filter(([command]) => command === 'node_app_start').length, 1);
  assert.equal(document.querySelector('[role=dialog]'), null);
});

test('A runtime restriction keeps restart disabled and does not ask to fill nonexistent required fields', async () => {
  const value = app({ state: 'running', restartRequired: true, runningRevision: 0 });
  const calls = await panel(value, 'details');
  assert.match(document.querySelector('.node-config-warning').textContent, /当前暂不能重启，请先查看运行状态/);
  assert.doesNotMatch(document.querySelector('.node-config-warning').textContent, /补齐必填/);
  await click(button('运行状态')); assert.equal(button('重启').disabled, true); noLaunch(calls);
});

test('Retired Node registrations explain their unsupported installation directly in the launcher',async()=>{
 const value={id:'retired-fixture',name:'Retired fixture',runtime:'node-service',sourcePath:'/disposable/retired',installation:{kind:'managed',retired:true},state:'failed',restartRequired:false,lastError:'这个应用使用旧的安装方式，已不再支持。请删除后重新添加。',fields:[],configuration:{unavailable:true,revision:0,values:{},secretStatus:{},missingRequired:[],canStart:false}};
 await render(NodeProjectCard,{app:value,blocked:false,opening:false,onEnter(){},onDetails(){},onDelete(){}});
 const notice=document.querySelector('.launcher-retired-note');assert.ok(notice);assert.match(notice.textContent,/请删除后重新添加/);
});
