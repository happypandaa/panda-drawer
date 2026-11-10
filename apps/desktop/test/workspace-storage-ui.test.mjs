// Disposable DOM fixtures verify detail UI state; they do not touch development project data.
import assert from 'node:assert/strict';
import { after, afterEach, test } from 'node:test';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';
import { JSDOM } from 'jsdom';

const repo = fileURLToPath(new URL('../../../', import.meta.url));
await mkdir(path.join(repo, '.test-artifacts'), { recursive: true });
const output = await mkdtemp(path.join(repo, '.test-artifacts/disposable-workspace-storage-dom-'));
const mockPath = path.join(output, 'tauri-mock.mjs');
await writeFile(mockPath, 'export const invoke = (...args) => globalThis.__WORKSPACE_STORAGE_INVOKE__(...args); export const isTauri = () => true;\n');
const compiled = new Map();
async function compile(file) {
  if (compiled.has(file)) return compiled.get(file);
  const target = path.join(output, `${path.basename(file).replace(/\.tsx?$/, '')}.mjs`);
  compiled.set(file, target);
  let source = ts.transpileModule(await readFile(file, 'utf8'), { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX }, fileName: file }).outputText;
  for (const match of [...source.matchAll(/from\s+(['"])(\.[^'"]+)\1/g)]) {
    const base = path.resolve(path.dirname(file), match[2]);
    let dependency;
    for (const candidate of [base, `${base}.tsx`, `${base}.ts`]) { try { await readFile(candidate); dependency = candidate; break; } catch { /* Next extension. */ } }
    assert.ok(dependency);
    source = source.replaceAll(`${match[1]}${match[2]}${match[1]}`, JSON.stringify(pathToFileURL(await compile(dependency)).href));
  }
  source = source.replaceAll(/(['"])@tauri-apps\/api\/core\1/g, JSON.stringify(pathToFileURL(mockPath).href));
  await writeFile(target, source);
  return target;
}
const dom = new JSDOM('<!doctype html><html><body></body></html>', { url: 'http://localhost/', pretendToBeVisual: true });
// These interaction fixtures assert Chinese copy; OS defaults are tested separately.
dom.window.localStorage.setItem('panda-drawer:language:v1', 'zh-CN');
for (const name of ['window', 'document', 'Node', 'HTMLElement', 'HTMLInputElement', 'HTMLSelectElement', 'Event', 'MouseEvent', 'MutationObserver', 'KeyboardEvent']) Object.defineProperty(globalThis, name, { value: name === 'window' ? dom.window : dom.window[name], configurable: true });
Object.defineProperty(globalThis, 'navigator', { value: dom.window.navigator, configurable: true });
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
globalThis.getComputedStyle = dom.window.getComputedStyle.bind(dom.window);
dom.window.HTMLElement.prototype.scrollIntoView = () => {};
const { createElement: h, act } = await import('react');
const { createRoot } = await import('react-dom/client');
const { WorkspaceStoragePanel } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/WorkspaceStoragePanel.tsx'))).href);
const { FrontendPlatformProvider, frontendPlatformFromBootstrap } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/frontend-platform.tsx'))).href);
const macPlatform = frontendPlatformFromBootstrap({ build: { target: 'aarch64-apple-darwin' } });
let root;
afterEach(async () => { if (root) await act(async () => root.unmount()); root = null; });
after(async () => { dom.window.close(); await rm(output, { recursive: true, force: true }); });

const makeTask = (id, extra = {}) => ({ taskId: `task_${id}`, type: 'storageCleanup', projectId: id, status: 'succeeded', final: true, phase: 'completed', error: null, result: { completed: true }, ...extra });
const project = id => ({ id, name: `Disposable ${id}`, projectRoot: `/disposable/${id}`, documentEpoch: 'epoch_fixture', state: 'ready', issues: [], manifest: {}, storage: { currentProject: { status: 'completed', totalBytes: 100, complete: true, categories: { dependencies: { bytes: 20 }, cache: { bytes: 15 } } } } });
async function setup(overrides = {}) {
  const calls = []; const tasks = []; const pending = []; const projects = [project('web_one'), project('web_two')];
  globalThis.__WORKSPACE_STORAGE_INVOKE__ = async (command, args) => {
    calls.push([command, args]);
    if (overrides[command]) return overrides[command](args);
    if (command === 'workspace_storage') return { workspace: { projects: projects.map(value => ({ scope: { projectId: value.id }, totalBytes: 100, complete: true })) }, nodeApps: [], disks: [] };
    if (command === 'storage_cleanup') return makeTask(args.projectId);
    throw new Error(`Unexpected disposable command ${command}`);
  };
  document.body.innerHTML = '<div id="root"></div>'; root = createRoot(document.getElementById('root'));
  const props = { workspace: { workspaceRoot: '/disposable/workspace', migrationState: 'ready', retainedRoots: [] }, instanceId: 'disposable-instance', revision: 1, enabled: true, active: true, migrating: false, projects, nodeApps: [], onTask() {}, onProjectTask: task => tasks.push(task), onRefresh: async () => {}, onPendingChange: value => pending.push(value) };
  await act(async () => root.render(h(FrontendPlatformProvider, { value: macPlatform }, h(WorkspaceStoragePanel, props))));
  return { calls, tasks, pending };
}
const button = text => [...document.querySelectorAll('button')].find(value => value.textContent === text);
const click = async element => { assert.ok(element); await act(async () => element.click()); };
async function selectAll() {
  await click(button('选择要清理的应用'));
  for (const checkbox of document.querySelectorAll('[role="dialog"] input[type="checkbox"]')) await click(checkbox);
}

test('storage settings retain measured application totals without historical cleanup requests', async () => {
  const { calls } = await setup();
  assert.equal(calls.filter(([command]) => command === 'workspace_storage').length, 1);
  assert.equal(calls.some(([command]) => command.startsWith('workspace_cleanup')), false);
  assert.equal(document.body.textContent.includes('取回前的原件'), false);
  assert.match(document.body.textContent, /可重新生成的依赖、缓存和日志/);
});

test('explicit cleanup only submits selected current projects and reports final tasks', async () => {
  const { calls, tasks, pending } = await setup();
  await selectAll();
  assert.equal(calls.some(([command]) => command === 'storage_cleanup'), false);
  await click(button('清理所选应用'));
  assert.deepEqual(calls.filter(([command]) => command === 'storage_cleanup').map(([, args]) => args), [
    { projectId: 'web_one', input: { categories: ['dependencies', 'buildCache', 'logs'] } },
    { projectId: 'web_two', input: { categories: ['dependencies', 'buildCache', 'logs'] } },
  ]);
  assert.deepEqual(tasks.map(task => task.projectId), ['web_one', 'web_two']);
  assert.deepEqual(pending, [true, false]);
});

test('a failed cleanup keeps later projects unsubmitted', async () => {
  const { calls } = await setup({ storage_cleanup: args => makeTask(args.projectId, { status: 'failed', error: { code: 'DISPOSABLE_FAILURE', message: 'retained fixture' }, result: { completed: false } }) });
  await selectAll(); await click(button('清理所选应用'));
  assert.equal(calls.filter(([command]) => command === 'storage_cleanup').length, 1);
  assert.match(document.body.textContent, /后续项目尚未提交/);
});

test('an invalid acknowledgement prevents replay and later submissions', async () => {
  const { calls } = await setup({ storage_cleanup: () => makeTask('wrong_project') });
  await selectAll(); await click(button('清理所选应用'));
  assert.equal(calls.filter(([command]) => command === 'storage_cleanup').length, 1);
  assert.match(document.body.textContent, /回执身份或格式不符/);
  assert.equal(button('选择要清理的应用').disabled, true);
});
