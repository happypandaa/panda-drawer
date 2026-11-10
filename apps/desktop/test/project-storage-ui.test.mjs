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
const output = await mkdtemp(path.join(repo, '.test-artifacts/disposable-project-storage-dom-'));
const mockPath = path.join(output, 'tauri-mock.mjs');
await writeFile(mockPath, 'export const invoke = (...args) => globalThis.__PROJECT_STORAGE_INVOKE__(...args); export const isTauri = () => true;\n');
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
for (const name of ['window', 'document', 'Node', 'HTMLElement', 'HTMLInputElement', 'HTMLSelectElement', 'Event', 'MouseEvent']) Object.defineProperty(globalThis, name, { value: name === 'window' ? dom.window : dom.window[name], configurable: true });
Object.defineProperty(globalThis, 'navigator', { value: dom.window.navigator, configurable: true });
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
const { createElement: h, act } = await import('react');
const { createRoot } = await import('react-dom/client');
const { ProjectAttachmentBrowser } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/StoragePanels.tsx'))).href);
let root;
afterEach(async () => { if (root) await act(async () => root.unmount()); root = null; });
after(async () => { dom.window.close(); await rm(output, { recursive: true, force: true }); });
const project = { id: 'prj_storage_disposable', name: 'Disposable storage fixture', documentEpoch: 'epoch_a', projectRoot: '/disposable/project', state: 'ready', manifest: {}, issues: [] };
const ready = { mode: 'ready', readyForChanges: false, recoveryRequired: false, activeTask: null, maintenanceId: null };
function storage(overrides = {}) {
  return { currentProject: { totalBytes: 280, complete: true, stale: false, status: 'completed', startedAt: 1700000000000, completedAt: 1700000001000, categories: Object.fromEntries(['source', 'output', 'attachments', 'database', 'dependencies', 'cache', 'other'].map((key, i) => [key, { totalBytes: (i + 1) * 10, complete: true }])), ...overrides }, relatedHistory: { entries: [{ path: '/disposable/history', totalBytes: 900 }] }, activeTask: null };
}
const fixtureFiles = [
  { path: 'first.png', mimeType: 'image/png', sizeBytes: 12, modifiedAt: 1700000000000 },
  { path: 'second.pdf', mimeType: 'application/pdf', sizeBytes: 20, modifiedAt: 1700000000000 },
  { path: 'third.webm', mimeType: 'video/webm', sizeBytes: 30, modifiedAt: 1700000000000 },
];
function files(input = {}) { return { files: fixtureFiles.slice(0, input.limit ?? 40), nextCursor: input.limit === 2 ? 'next' : null, complete: true, issues: [], completedAt: 1700000000000 }; }
function deferred() { let resolve; let reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { resolve, reject, promise }; }
function accepted(type = 'storageCleanup') { return { taskId: 'task_disposable_storage', projectId: project.id, type, status: 'queued', final: false, phase: 'queued', result: null, error: null }; }
async function setup({ props = {}, overrides = {} } = {}) {
  const calls = []; const tasks = []; const drafts = [];
  globalThis.__PROJECT_STORAGE_INVOKE__ = async (command, args) => {
    calls.push([command, args]);
    if (overrides[command]) return overrides[command](args);
    if (command === 'storage_info') return storage();
    if (command === 'storage_files') return files(args.input);
    if (command === 'storage_scan') return accepted('storageScan');
    if (command === 'storage_cleanup') return accepted();
    if (command === 'storage_preview') return { dataUrl: 'data:image/png;base64,AAAA', mimeType: 'image/png' };
    if (command === 'storage_download') return { savedPath: '/disposable/saved.png' };
    throw new Error(`Unexpected disposable command: ${command}`);
  };
  document.body.innerHTML = '<div id="root"></div>';
  root = createRoot(document.getElementById('root'));
  let current = { project, status: ready, disabled: false, blocked: false, onBusyChange: pending => drafts.push({ pending }), onTask: value => tasks.push(value), onChanged: async () => {}, onDraftStateChange: value => drafts.push(value), ...props };
  const render = async changes => { current = { ...current, ...changes }; await act(async () => root.render(h(ProjectAttachmentBrowser, current))); };
  await render({});
  return { calls, tasks, drafts, render };
}
const byText = text => [...document.querySelectorAll('button')].find(button => button.textContent === text);
const commandCalls = (calls, command) => calls.filter(([name]) => name === command);
const click = async element => { assert.ok(element); await act(async () => element.click()); };
const checkbox = text => [...document.querySelectorAll('label')].find(label => label.textContent.includes(text))?.querySelector('input[type=checkbox]');
const field = text => [...document.querySelectorAll('label')].find(label => label.textContent.startsWith(text))?.querySelector('input,select');
async function change(element, value) { await act(async () => { const prototype = element.tagName === 'SELECT' ? dom.window.HTMLSelectElement.prototype : dom.window.HTMLInputElement.prototype; Object.getOwnPropertyDescriptor(prototype, 'value').set.call(element, value); element.dispatchEvent(new dom.window.Event(element.tagName === 'SELECT' ? 'change' : 'input', { bubbles: true })); }); }
async function reviewCleanup() { await click(byText('查看可清理内容')); await click(checkbox('开发依赖')); await click(byText('检查清理范围')); await click(checkbox('我已核对应用和清理范围')); }

test('User files start with two real rows and expand into filtering and serialized pagination', async () => {
  const page = deferred();
  const { calls } = await setup({ overrides: { storage_files: ({ input }) => input.cursor ? page.promise : { ...files(input), nextCursor: 'next' } } });
  assert.equal(document.querySelectorAll('.project-storage-file-row').length, 2);
  assert.equal(commandCalls(calls, 'storage_files')[0][1].input.limit, 2);
  await click(byText('浏览全部文件'));
  assert.equal(document.querySelectorAll('.project-storage-file-row').length, 3);
  assert.equal(commandCalls(calls, 'storage_files').at(-1)[1].input.limit, 40);
  await change(field('名称搜索'), 'first');
  await click(byText('查询'));
  assert.equal(commandCalls(calls, 'storage_files').at(-1)[1].input.query, 'first');
  const next = byText('继续读取下一页');
  await act(async () => { next.click(); next.click(); });
  assert.equal(commandCalls(calls, 'storage_files').filter(([, args]) => args.input.cursor).length, 1);
  await act(async () => page.resolve({ files: [{ ...fixtureFiles[0], path: 'fourth.png' }], nextCursor: null, complete: true, issues: [] }));
  assert.equal(document.querySelectorAll('.project-storage-file-row').length, 4);
});

test('Preview closes on hidden state and a late result cannot reopen it', async () => {
  const preview = deferred();
  const { drafts, render } = await setup({ overrides: { storage_preview: () => preview.promise } });
  await click(byText('浏览全部文件'));
  await click(document.querySelector('[aria-label="预览 first.png"]'));
  assert.equal(drafts.at(-1).pending, true);
  await render({ blocked: true });
  await act(async () => preview.resolve({ dataUrl: 'data:image/png;base64,AAAA', mimeType: 'image/png' }));
  await render({ blocked: false });
  assert.equal(document.querySelector('.attachment-preview'), null);
  assert.equal(drafts.at(-1).pending, false);
});

test('Invalid preview media is rejected and downloads are serialized with an epoch-bound request', async () => {
  const download = deferred();
  const { calls, drafts } = await setup({ overrides: { storage_preview: () => ({ dataUrl: 'data:text/html;base64,AAAA', mimeType: 'text/html' }), storage_download: () => download.promise } });
  await click(byText('浏览全部文件'));
  await click(document.querySelector('[aria-label="预览 first.png"]'));
  assert.equal(document.querySelector('.attachment-preview'), null);
  assert.match(document.body.textContent, /不支持安全预览/);
  const button = document.querySelector('[aria-label="下载 first.png"]');
  await act(async () => { button.click(); button.click(); });
  assert.deepEqual(commandCalls(calls, 'storage_download'), [['storage_download', { projectId: project.id, path: 'first.png', documentEpoch: 'epoch_a' }]]);
  assert.equal(drafts.at(-1).pending, true);
  await act(async () => download.resolve({ savedPath: '/disposable/saved.png' }));
  assert.match(document.body.textContent, /已保存到 \/disposable\/saved.png/);
  assert.equal(drafts.at(-1).pending, false);
});
