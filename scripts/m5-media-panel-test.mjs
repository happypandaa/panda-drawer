// Real React component + browser helper in jsdom; no native UI, devices, or service.
import assert from 'node:assert/strict';
import { after, afterEach, test } from 'node:test';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';
import { JSDOM } from 'jsdom';

const repo = fileURLToPath(new URL('..', import.meta.url));
await mkdir(path.join(repo, '.test-artifacts'), { recursive: true });
const output = await mkdtemp(path.join(repo, '.test-artifacts/media-panel-dom-'));
const sdkMock = path.join(output, 'sdk.mjs');
await writeFile(sdkMock, 'export const platform = { files: { put: (...args) => globalThis.__MEDIA_PANEL_TEST__.put(...args) }, db: { transaction: (...args) => globalThis.__MEDIA_PANEL_TEST__.transaction(...args) } };\n');
const compiled = new Map();
async function compile(file) {
  if (compiled.has(file)) return compiled.get(file);
  const target = path.join(output, `${path.basename(file).replace(/\.[cm]?[jt]sx?$/, '')}.mjs`);
  compiled.set(file, target);
  let source = ts.transpileModule(await readFile(file, 'utf8'), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX }, fileName: file,
  }).outputText;
  for (const match of [...source.matchAll(/from\s+(['"])(\.[^'"]+)\1/g)]) {
    const base = path.resolve(path.dirname(file), match[2]);
    let dependency;
    for (const candidate of [base, `${base}.tsx`, `${base}.ts`]) {
      try { await readFile(candidate); dependency = candidate; break; } catch { /* Try the next extension. */ }
    }
    assert.ok(dependency, `Cannot resolve ${match[2]}`);
    source = source.replaceAll(`${match[1]}${match[2]}${match[1]}`, JSON.stringify(pathToFileURL(await compile(dependency)).href));
  }
  source = source.replaceAll(/(['"])@local-projects\/sdk\1/g, JSON.stringify(pathToFileURL(sdkMock).href));
  await writeFile(target, source);
  return target;
}

const dom = new JSDOM('<!doctype html><html><body></body></html>', { url: 'http://127.0.0.1:4321/p/prj_media_test/', pretendToBeVisual: true });
for (const name of ['window', 'document', 'Node', 'HTMLElement', 'HTMLInputElement', 'HTMLTextAreaElement', 'Event', 'CustomEvent']) {
  Object.defineProperty(globalThis, name, { value: name === 'window' ? dom.window : dom.window[name], configurable: true });
}
Object.defineProperty(globalThis, 'navigator', { value: dom.window.navigator, configurable: true });
Object.defineProperty(globalThis, 'isSecureContext', { value: true, configurable: true });
for (const name of ['addEventListener', 'removeEventListener']) globalThis[name] = dom.window[name].bind(dom.window);
dom.window.HTMLMediaElement.prototype.play = async function () {};
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
const { createElement: h, act } = await import('react');
const { createRoot } = await import('react-dom/client');
const { MediaToolsPanel } = await import(pathToFileURL(await compile(path.join(repo, 'templates/react/app/src/components/MediaToolsPanel.tsx'))).href);
const normal = { mode: 'normal', busy: false, reloadRequired: false, offline: false, expired: false, recoveryRequired: false, trashed: false, workspaceMigrating: false, instanceChanged: false };
const context = () => ({ projectId: 'prj_media_test', name: 'Disposable DOM sample', state: { mode: 'normal', busy: false, reloadRequired: false }, limits: { fileUploadMaxBytes: 1024 * 1024 }, runtimeRequirements: { devices: { camera: { purpose: 'Disposable photo' }, microphone: { purpose: 'Disposable recording' } } } });
let root;
let calls;
let captures;
let tracks;
let currentContext;
async function setup() {
  document.body.innerHTML = '<div id="root"></div>';
  Object.defineProperty(document, 'hidden', { value: false, configurable: true });
  calls = { puts: [], transactions: [] }; captures = []; tracks = [];
  globalThis.__MEDIA_PANEL_TEST__ = {
    async put(input) { calls.puts.push(input); return { path: input.path }; },
    async transaction(input) { calls.transactions.push(input); return { results: [] }; },
  };
  Object.defineProperty(navigator, 'userActivation', { value: { isActive: true }, configurable: true });
  Object.defineProperty(navigator, 'mediaDevices', { value: { async getUserMedia(constraints) {
    captures.push(constraints);
    const track = Object.assign(new dom.window.EventTarget(), { stops: 0, stop() { this.stops++; } });
    tracks.push(track); return { getTracks: () => [track] };
  } }, configurable: true });
  root = createRoot(document.getElementById('root'));
  currentContext = context();
  await render(currentContext);
  await act(async () => { document.querySelector('details').open = true; });
}
async function render(value) { currentContext = value; await act(async () => { root.render(h(MediaToolsPanel, { context: value })); }); }
function button(text) { const found = [...document.querySelectorAll('button')].find(element => element.textContent === text); assert.ok(found, `Missing button: ${text}`); return found; }
async function click(text) { await act(async () => { button(text).click(); }); }
function emitRuntime(value) { window.dispatchEvent(new CustomEvent('platform:state', { detail: value })); }
async function runtime(value) { await act(async () => { emitRuntime(value); }); }
async function choose(name = 'disposable-note.txt', text = 'Disposable file bytes', type = 'text/plain') {
  const input = document.querySelector('input[type="file"]');
  Object.defineProperty(input, 'files', { value: [new File([text], name, { type })], configurable: true });
  await act(async () => { input.dispatchEvent(new Event('change', { bubbles: true })); });
}
function assertBlocked() {
  for (const label of ['开始录音', '打开摄像头 / 重拍', '保存素材示例']) assert.equal(button(label).disabled, true, label);
  assert.equal(document.querySelector('input[type="file"]').disabled, false);
  for (const label of ['下载本地副本', '清除页面预览', '复制文字']) assert.equal(button(label).disabled, false, label);
  assert.match(document.body.textContent, /项目当前暂停采集和保存素材/);
}
afterEach(async () => { if (root) await act(async () => { root.unmount(); }); root = undefined; });
after(async () => { dom.window.close(); await rm(output, { recursive: true, force: true }); });

test('opening and selecting a file performs no writes or device request and does not claim a device was used', async () => {
  await setup(); await choose();
  assert.equal(captures.length, 0); assert.equal(calls.puts.length, 0); assert.equal(calls.transactions.length, 0);
  assert.equal(button('保存素材示例').disabled, false);
  assert.match(document.body.textContent, /文件仅在当前页面预览/);
  assert.doesNotMatch(document.body.textContent, /设备已释放/);
});

for (const [name, update] of Object.entries({
  maintenance: { mode: 'maintenance' }, busy: { busy: true }, reload: { reloadRequired: true }, offline: { offline: true }, expired: { expired: true }, recovery: { recoveryRequired: true }, trash: { trashed: true }, migration: { workspaceMigrating: true }, instance: { instanceChanged: true },
})) test(`${name} notice immediately gates capture and persistence while local draft tools remain available`, async () => {
  await setup(); await choose(); await runtime({ ...normal, ...update }); assertBlocked();
  await click('保存素材示例');
  assert.equal(calls.puts.length, 0); assert.equal(calls.transactions.length, 0);
  await choose('maintenance-photo.png', 'PNG draft', 'image/png');
  assertBlocked(); assert.ok(document.querySelector('img[alt="所选素材预览"]'));
  assert.match(document.body.textContent, /9 字节/);
});

test('a runtime notice arriving before React renders cannot submit a previously enabled save or capture button', async () => {
  await setup(); await choose();
  const save = button('保存素材示例'); const camera = button('打开摄像头 / 重拍');
  assert.equal(save.disabled, false); assert.equal(camera.disabled, false);
  await act(async () => {
    emitRuntime({ ...normal, mode: 'maintenance' });
    assert.equal(save.disabled, false, 'Deliberately exercise the old enabled DOM before React commits');
    save.click(); camera.click();
  });
  assertBlocked(); assert.equal(calls.puts.length, 0); assert.equal(captures.length, 0);
  assert.match(document.querySelector('[role="alert"]').textContent, /项目当前不能采集或保存素材/);
});

test('late normal context cannot override a blocked notice; an authoritative normal notice restores controls without replay or draft loss', async () => {
  await setup(); await choose(); await runtime({ ...normal, mode: 'maintenance' });
  await render({ ...context(), name: 'Late context response' }); assertBlocked();
  assert.match(document.body.textContent, /disposable-note.txt/);
  await runtime(normal);
  for (const label of ['开始录音', '打开摄像头 / 重拍', '保存素材示例']) assert.equal(button(label).disabled, false, label);
  assert.equal(calls.puts.length, 0); assert.equal(captures.length, 0);
  await click('保存素材示例');
  assert.equal(calls.puts.length, 1); assert.equal(calls.transactions.length, 1);
  assert.equal(await calls.puts[0].content.text(), 'Disposable file bytes');
  assert.match(document.body.textContent, /附件和业务记录均已保存/);
});

test('native offline gates persist through a late context until the runtime reports a successful connection', async () => {
  await setup(); await choose();
  await act(async () => { window.dispatchEvent(new Event('offline')); });
  assertBlocked(); await render(context()); assertBlocked();
  await runtime(normal); assert.equal(button('保存素材示例').disabled, false);
  assert.equal(calls.puts.length, 0);
});

test('maintenance and document hiding release an active camera without automatically restarting it', async () => {
  await setup(); await click('打开摄像头 / 重拍');
  assert.equal(captures.length, 1); assert.equal(tracks[0].stops, 0);
  assert.equal(document.querySelector('video').hidden, false);
  await runtime({ ...normal, mode: 'maintenance' });
  assert.ok(tracks[0].stops > 0); assert.equal(document.querySelector('video').hidden, true);
  await runtime(normal); assert.equal(captures.length, 1);
  await click('打开摄像头 / 重拍');
  await act(async () => { Object.defineProperty(document, 'hidden', { value: true, configurable: true }); document.dispatchEvent(new Event('visibilitychange')); });
  assert.ok(tracks[1].stops > 0); assert.equal(document.querySelector('video').hidden, true);
  await act(async () => { Object.defineProperty(document, 'hidden', { value: false, configurable: true }); document.dispatchEvent(new Event('visibilitychange')); });
  assert.equal(captures.length, 2); assert.equal(calls.puts.length, 0);
});

test('maintenance between attachment and SQL preserves the file receipt without attempting SQL or retrying on resume', async () => {
  await setup(); await choose(); let completeUpload;
  globalThis.__MEDIA_PANEL_TEST__.put = input => { calls.puts.push(input); return new Promise(resolve => { completeUpload = () => resolve({ path: input.path }); }); };
  await click('保存素材示例'); assert.equal(calls.puts.length, 1);
  await runtime({ ...normal, mode: 'maintenance' });
  await act(async () => { completeUpload(); });
  assert.equal(calls.transactions.length, 0);
  assert.match(document.body.textContent, /附件已保存，业务记录未得到成功确认/);
  assert.match(document.querySelector('.save-state code').textContent, /^media-samples\//);
  await runtime(normal);
  assert.equal(calls.puts.length, 1); assert.equal(calls.transactions.length, 0);
  assert.equal(button('保存素材示例').disabled, true, 'Uncertain outcome still requires explicit inspection, not replay');
});

test('two clicks before React commits accept only one upload and one record', async () => {
  await setup(); await choose(); const save = button('保存素材示例');
  await act(async () => { save.click(); save.click(); });
  assert.equal(calls.puts.length, 1); assert.equal(calls.transactions.length, 1);
});
