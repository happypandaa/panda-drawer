// DOM interaction fixtures only; these do not establish native Git/install behavior.
import assert from 'node:assert/strict';
import { after, afterEach, test } from 'node:test';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';
import { JSDOM } from 'jsdom';

const repo = fileURLToPath(new URL('../../../', import.meta.url));
await mkdir(path.join(repo, '.test-artifacts'), { recursive: true });
const output = await mkdtemp(path.join(repo, '.test-artifacts/disposable-source-import-dom-'));
const mockPath = path.join(output, 'tauri-mock.mjs');
await writeFile(mockPath, 'export const invoke = (...args) => globalThis.__SOURCE_IMPORT_INVOKE__(...args);\n');
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
    assert.ok(dependency, `Missing dependency ${base}`);
    source = source.replaceAll(`${match[1]}${match[2]}${match[1]}`, JSON.stringify(pathToFileURL(await compile(dependency)).href));
  }
  source = source.replaceAll(/(['"])@tauri-apps\/api\/core\1/g, JSON.stringify(pathToFileURL(mockPath).href));
  await writeFile(target, source); return target;
}
const dom = new JSDOM('<!doctype html><html><body></body></html>', { url: 'http://localhost/', pretendToBeVisual: true });
for (const name of ['window', 'document', 'Node', 'HTMLElement', 'HTMLInputElement', 'HTMLSelectElement', 'Event', 'MouseEvent', 'KeyboardEvent', 'MutationObserver']) Object.defineProperty(globalThis, name, { value: name === 'window' ? dom.window : dom.window[name], configurable: true });
Object.defineProperty(globalThis, 'navigator', { value: dom.window.navigator, configurable: true });
globalThis.getComputedStyle = dom.window.getComputedStyle.bind(dom.window);
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
dom.window.HTMLElement.prototype.scrollIntoView = function () {};
const { createElement: h, act } = await import('react');
const { createRoot } = await import('react-dom/client');
const { SourceAcquisitionPanel } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/SourceAcquisitionPanel.tsx'))).href);
let root; let nextTimer = 0;
const timeouts = new Map(); const intervals = new Map();
dom.window.setTimeout = fn => { const id = ++nextTimer; timeouts.set(id, fn); return id; };
dom.window.clearTimeout = id => timeouts.delete(id);
dom.window.setInterval = fn => { const id = ++nextTimer; intervals.set(id, fn); return id; };
dom.window.clearInterval = id => intervals.delete(id);
afterEach(async () => { if (root) await act(async () => root.unmount()); root = null; timeouts.clear(); intervals.clear(); dom.window.localStorage.clear(); });
after(async () => { dom.window.close(); await rm(output, { recursive: true, force: true }); });

const sourceId = `src_${'1'.repeat(32)}`;
const candidateId = `install_${'3'.repeat(32)}`;
const appId = '11111111-1111-4111-8111-111111111111';
const digest = 'a'.repeat(64); const generation = `gen_${'4'.repeat(32)}`;
const source = { sourceId, taskId: 'task_source', name: 'Disposable source', status: 'ready', phase: 'completed', url: 'https://github.com/disposable/source.git', evidencePath: '/disposable/source', contentDigest: digest, commit: 'c'.repeat(40), inspection: { kind: 'node-service-source', name: 'Disposable app', requiresBuild: false, issues: [], executionState: 'not-executed' } };
const prepared = { candidateId, appId, generation, manifest: { runtime: 'node-service', app: { id: 'org.example.disposable', name: 'Disposable app', version: '1.0.0' } }, programDigest: 'b'.repeat(64), provenance: { sourceId, sourceDigest: digest, sourceCommit: source.commit, node: { path: '/disposable/node', version: '24.22.0' }, npm: { path: '/disposable/npm', version: '11.19.1' }, preparedAt: 1 } };
const installed = { candidateId, appId, generation, operationOutcome: 'committed' };
const app = { id: appId, name: 'Disposable app', sourcePath: '/disposable/program', runtime: 'node-service', state: 'stopped', restartRequired: false, runningRevision: null, lastError: null, fields: [], configuration: { revision: 1, values: {}, secretStatus: {}, missingRequired: ['API_KEY'], canStart: false } };
const cleaned = { completed: true, sourceId, sourceRetained: true, removedPaths: ['/disposable/temp'], failures: [], receiptPath: '/disposable/receipt.json' };
const task = (type, overrides = {}) => ({ taskId: type === 'sourceAcquire' ? 'task_source' : type === 'nodeInstallPrepare' ? 'task_prepare' : 'task_apply', projectId: null, type, status: 'succeeded', final: true, phase: 'completed', result: type === 'sourceAcquire' ? source : type === 'nodeInstallPrepare' ? { state: 'prepared', candidate: prepared, installed: false } : installed, error: null, ...overrides });
function deferred() { let resolve; let reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { resolve, reject, promise }; }
async function setup(overrides = {}, initialProps = {}) {
  const calls = []; const entered = []; const tasks = []; let closed = 0; let registered = 0;
  const defaults = {
    source_acquire: () => task('sourceAcquire'), node_install_prepare: () => task('nodeInstallPrepare'),
    node_install_candidate: () => ({ state: 'prepared', candidate: prepared }), node_install_apply: () => task('nodeInstallApply'),
    source_import_cleanup: () => cleaned, source_import_publish: () => ({ published: true, sourcePath: '/disposable/projects/source', projectDirectory: '/disposable/projects' }),
    source_import_build_info: () => ({ scriptName: 'build', script: 'node build.mjs' }), task_logs: () => ({ text: '', nextCursor: 0, eof: true, truncated: false, gaps: [] }), source_review_capabilities: () => ({codexAvailable:false}), node_app_configuration: () => app, source_list: () => [],
  };
  globalThis.__SOURCE_IMPORT_INVOKE__ = async (command, args) => {
    calls.push([command, args]); const run = overrides[command] ?? defaults[command];
    assert.ok(run, `Unexpected native command ${command}`); return run(args);
  };
  document.body.innerHTML = '<button id="trigger">Import</button><div id="root"></div>';
  document.getElementById('trigger').focus(); root = createRoot(document.getElementById('root'));
  let props = { disabled: false, instanceId: 'instance_one', projectDirectory: '/disposable/projects', ...initialProps, onTask: value => tasks.push(value), onClose: () => { closed++; }, onInstalled: () => { registered++; }, onEnter: value => entered.push(value) };
  async function render(next = {}) { props = { ...props, ...next }; await act(async () => root.render(h(SourceAcquisitionPanel, props))); }
  await render();
  return { calls, entered, tasks, render, get closed() { return closed; }, get registered() { return registered; } };
}
const byText = text => [...document.querySelectorAll('button')].find(element => element.textContent === text);
const commands = value => value.calls.map(([name]) => name);
async function click(element) { assert.ok(element); await act(async () => element.click()); }
async function change(element, value) { assert.ok(element); await act(async () => { Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set.call(element, value); element.dispatchEvent(new window.Event('input', { bubbles: true })); }); }
async function begin() { await change(document.getElementById('git-import-url'), source.url); await click(byText('开始添加')); }
async function escape() { await act(async () => document.activeElement.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }))); }
async function tick() { await act(async () => { const pending = [...timeouts.entries()]; timeouts.clear(); for (const [, fn] of pending) fn(); }); }

test('The Git form explains all three file locations and validates supported repository URLs before starting', async () => {
  const fixture = await setup();
  assert.equal(document.querySelector('[role=dialog] h2').textContent, '从 Git 添加项目');
  assert.equal(byText('开始添加').disabled, true);
  assert.equal(document.querySelector('.git-import-options').open, false);
  assert.equal(document.querySelectorAll('.add-project-storage-cards > div').length, 3);
  assert.deepEqual(fixture.calls, []);
  await change(document.getElementById('git-import-url'), 'https://github.com/disposable/source/issues');
  assert.equal(byText('开始添加').disabled, true);
  assert.equal(document.getElementById('git-import-url').getAttribute('aria-invalid'), 'true');
  await change(document.getElementById('git-import-url'), source.url);
  assert.equal(byText('开始添加').disabled, false);
  assert.ok(document.body.textContent.includes('GitHub 公开仓库'));
  assert.ok(document.body.textContent.includes('/disposable/projects/source'));
});

test('A completed add publishes editable source before cleanup and offers explicit configuration navigation', async () => {
  const fixture = await setup(); await begin();
  assert.ok(document.body.textContent.includes('已添加「Disposable app」'));
  assert.equal(fixture.registered, 1);
  assert.equal(fixture.entered.length, 0);
  assert.equal(fixture.closed, 0);
  assert.deepEqual(commands(fixture), ['source_acquire', 'node_install_prepare', 'node_install_candidate', 'node_install_apply', 'source_import_publish', 'source_import_cleanup', 'node_app_configuration']);
  assert.ok(document.body.textContent.includes('/disposable/projects/source'));
  await click(byText('去设置'));
  assert.deepEqual(fixture.entered, [app]);
  assert.equal(fixture.closed, 1);
  assert.ok(!commands(fixture).some(name => ['node_app_start', 'node_app_open'].includes(name)));
});

test('Unsupported repositories are automatically cleaned, and the result never claims removal before confirmation', async () => {
  const removal = deferred();
  const unsupported = { ...source, inspection: { ...source.inspection, kind: 'needs-adaptation', issues: [{ message: 'Missing local-app.json' }] } };
  const fixture = await setup({ source_acquire: () => task('sourceAcquire', { result: unsupported }), source_import_cleanup: () => removal.promise });
  await begin();
  assert.ok(document.body.textContent.includes('正在清理本次下载的临时文件'));
  assert.ok(!document.body.textContent.includes('下载的文件已经删除'));
  assert.equal(document.querySelector('.modal-close').disabled, true);
  assert.equal(fixture.closed, 0);
  await act(async () => removal.resolve({ ...cleaned, sourceRetained: false }));
  assert.ok(document.body.textContent.includes('下载的文件已经删除'));
  assert.equal(fixture.closed, 0);
  assert.ok(byText('复制说明给 AI'));
  assert.ok(!commands(fixture).includes('node_install_prepare'));
  await click(byText('关闭'));
  assert.equal(fixture.closed, 1);
});

test('Failed cleanup stays visible and retries only the same task without adding a second app', async () => {
  let removals = 0;
  const fixture = await setup({
    source_acquire: () => task('sourceAcquire', { status: 'failed', result: null, error: { code: 'SOURCE_GIT_FAILED', message: 'Disposable download failure', details: { sourceId } } }),
    source_import_cleanup: () => ++removals === 1 ? { ...cleaned, completed: false, failures: [{ path: '/disposable/temp', error: { message: 'Disposable removal failure' } }] } : { ...cleaned, sourceRetained: false },
  });
  await begin();
  assert.equal(fixture.closed, 0);
  assert.ok(document.body.textContent.includes('Disposable removal failure'));
  assert.equal(byText('重试下载').disabled, true);
  await click(byText('重试清理'));
  assert.equal(removals, 2);
  assert.ok(document.body.textContent.includes('下载的文件已经删除'));
  assert.equal(commands(fixture).filter(name => name === 'source_acquire').length, 1);
});

test('A structured rejection keeps the editable address and does not invent an accepted task', async () => {
  const fixture = await setup({ source_acquire: () => { throw { code: 'SOURCE_URL_INVALID', message: 'Disposable bad address' }; } });
  await begin(); assert.ok(document.body.textContent.includes('Disposable bad address'));
  assert.ok(document.body.textContent.includes('尚未下载项目文件'));
  await click(byText('修改地址'));
  assert.equal(document.getElementById('git-import-url').value, source.url);
  assert.ok(byText('开始添加'));
  assert.deepEqual(commands(fixture), ['source_acquire']);
});

test('Build scripts are shown and run only after explicit consent', async () => {
  const buildSource = { ...source, inspection: { ...source.inspection, requiresBuild: true } };
  const fixture = await setup({ source_acquire: () => task('sourceAcquire', { result: buildSource }) });
  await begin();
  assert.ok(document.body.textContent.includes('这个项目需要先构建一次'));
  assert.ok(document.body.textContent.includes('node build.mjs'));
  assert.deepEqual(commands(fixture), ['source_acquire', 'source_import_build_info']);
  await click(byText('同意并继续'));
  assert.equal(fixture.calls.find(([name]) => name === 'node_install_prepare')[1].input.buildSource, true);
  assert.ok(document.body.textContent.includes('已添加「Disposable app」'));
});

test('A build whose script cannot be read cannot run or be silently approved', async () => {
  const buildSource = { ...source, inspection: { ...source.inspection, requiresBuild: true } };
  const fixture = await setup({ source_acquire: () => task('sourceAcquire', { result: buildSource }), source_import_build_info: () => { throw new Error('Read denied'); } });
  await begin();
  assert.equal(byText('同意并继续').disabled, true);
  await click(byText('同意并继续'));
  assert.ok(!commands(fixture).includes('node_install_prepare'));
  await click(byText('不构建，关闭'));
  assert.equal(fixture.closed, 1);
  assert.equal(commands(fixture).filter(name => name === 'source_import_cleanup').length, 1);
});

test('A source response from a previous platform instance cannot trigger preparation or cleanup', async () => {
  const accepted = deferred(); const fixture = await setup({ source_acquire: () => accepted.promise });
  await begin(); await fixture.render({ instanceId: 'instance_two' });
  await act(async () => accepted.resolve(task('sourceAcquire')));
  assert.ok(byText('开始添加'));
  assert.equal(fixture.registered, 0);
  assert.deepEqual(commands(fixture), ['source_acquire']);
});

test('Cancellation is confirmed separately and waits for the exact backend stop before cleanup', async () => {
  const cancelling = deferred();
  const fixture = await setup({
    source_acquire: () => task('sourceAcquire', { status: 'running', final: false, phase: 'downloading', result: null }),
    source_cancel: () => cancelling.promise,
  });
  await begin();
  assert.equal(document.querySelector('.modal-close').disabled, true);
  await escape(); assert.equal(fixture.closed, 0);
  await click(byText('取消下载'));
  assert.ok(byText('继续下载'));
  await click(byText('停止下载'));
  assert.equal(byText('取消下载').disabled, true);
  assert.ok(!commands(fixture).includes('source_import_cleanup'));
  await act(async () => cancelling.resolve(task('sourceAcquire', { status: 'failed', result: null, error: { code: 'SOURCE_ACQUISITION_CANCELLED', message: 'Cancelled', details: { sourceId } } })));
  await tick();
  assert.ok(document.body.textContent.includes('添加已取消'));
  assert.ok(commands(fixture).includes('source_import_cleanup'));
  assert.ok(!commands(fixture).includes('node_install_prepare'));
});

test('Historical sources stay behind an explicit advanced action and never auto install', async () => {
  const fixture = await setup(); assert.deepEqual(fixture.calls, []);
  await click(byText('查看源码记录与恢复'));
  assert.deepEqual(commands(fixture), ['source_list']);
  assert.ok(document.body.textContent.includes('检查本机源码目录'));
  assert.ok(!commands(fixture).some(name => name === 'node_install_prepare' || name === 'node_install_apply'));
});

test('A failed preparation automatically cleans its stopped material and gives an AI handoff without reinstalling', async () => {
  const fixture = await setup({ node_install_prepare: () => task('nodeInstallPrepare', { status: 'failed', result: null, error: { code: 'DEPENDENCY_FAILED', message: 'Disposable dependency failure' } }) });
  await begin();
  assert.ok(document.body.textContent.includes('准备应用时出错'));
  assert.ok(byText('复制说明给 AI'));
  assert.equal(byText('重试下载'), undefined);
  assert.ok(!commands(fixture).includes('node_install_apply'));
  assert.equal(commands(fixture).filter(name => name === 'source_import_cleanup').length, 1);
});

test('Unknown installation offers verification, and reconciliation publishes once without applying twice', async () => {
  let lookups = 0;
  const fixture = await setup({
    node_install_candidate: () => ++lookups === 1 ? { state: 'prepared', candidate: prepared } : { state: 'committed', result: installed },
    node_install_apply: () => task('nodeInstallApply', { status: 'failed', result: { candidateId, operationOutcome: 'unknown' }, error: { code: 'NODE_INSTALL_RECOVERY_REQUIRED', message: 'Disposable uncertain commit', details: { operationOutcome: 'unknown' } } }),
  });
  await begin();
  assert.ok(document.body.textContent.includes('还不能确定应用是否已经添加'));
  assert.equal(byText('重试下载'), undefined);
  assert.ok(!commands(fixture).includes('source_import_cleanup'));
  assert.ok(!commands(fixture).includes('source_import_publish'));
  await click(byText('重新核对'));
  assert.ok(document.body.textContent.includes('已添加「Disposable app」'));
  assert.equal(commands(fixture).filter(name => name === 'node_install_apply').length, 1);
  assert.equal(fixture.registered, 1);
});

test('Registered applications stay in progress until source publication and cleanup are both confirmed', async () => {
  const removal = deferred();
  const fixture = await setup({ source_import_cleanup: () => removal.promise });
  await begin();
  assert.equal(document.querySelector('.git-import-result h3').textContent, '正在清理本次临时文件…');
  const stages = [...document.querySelectorAll('.import-progress li')];
  assert.deepEqual(stages.map(item => item.textContent.replace(/^\d/, '')), ['下载源码', '检查项目', '准备应用', '添加应用', '清理临时文件']);
  assert.equal(stages[4].getAttribute('aria-current'), 'step');
  assert.equal(stages[4].classList.contains('is-complete'), false);
  assert.equal(stages.slice(0, 4).every(item => item.classList.contains('is-complete')), true);
  assert.equal(byText('完成'), undefined);
  await escape(); assert.equal(fixture.closed, 0);
  assert.equal(document.querySelector('.modal-close').disabled, true);
  await act(async () => removal.resolve(cleaned));
  assert.equal([...document.querySelectorAll('.import-progress li')].every(item => item.classList.contains('is-complete')), true);
  assert.equal(fixture.registered, 1);
});

test('A cleanup failure after installation retries only cleanup and never repeats installation or source publication', async () => {
  let removals = 0;
  const removal = deferred();
  const fixture = await setup({ source_import_cleanup: () => ++removals === 1 ? { ...cleaned, completed: false, failures: [{ path: '/disposable/tmp', error: { message: 'Removal needs retry' } }] } : removal.promise });
  await begin();
  assert.equal(document.querySelector('.git-import-result h3').textContent, '应用已添加，临时文件还没清理完');
  assert.equal(byText('完成'), undefined);
  await click(byText('重试清理'));
  assert.equal(document.querySelector('.git-import-result h3').textContent, '正在清理本次临时文件…');
  await act(async () => removal.resolve(cleaned));
  assert.ok(document.body.textContent.includes('已添加「Disposable app」'));
  assert.equal(commands(fixture).filter(name => name === 'node_install_apply').length, 1);
  assert.equal(commands(fixture).filter(name => name === 'source_import_publish').length, 1);
  assert.equal(fixture.registered, 1);
});

test('Failed download cleanup does not mark skipped checks and installation as completed', async () => {
  const removal = deferred();
  const fixture = await setup({ source_acquire: () => task('sourceAcquire', { status: 'failed', result: null, error: { code: 'SOURCE_GIT_FAILED', message: 'Download failed' } }), source_import_cleanup: () => removal.promise });
  await begin();
  const stages = [...document.querySelectorAll('.import-progress li')];
  assert.equal(stages[4].getAttribute('aria-current'), 'step');
  assert.equal(stages.slice(0, 4).some(item => item.classList.contains('is-complete')), false);
  await act(async () => removal.resolve({ ...cleaned, sourceRetained: false }));
  assert.equal(fixture.closed, 0);
  assert.equal(document.querySelector('.import-progress li:nth-child(2)').classList.contains('is-complete'), false);
});

test('A publication failure retains the installed app and retries source publication without reinstalling', async () => {
  let publications = 0;
  const fixture = await setup({ source_import_publish: () => { if (++publications === 1) throw new Error('Cannot publish yet'); return { published: true, sourcePath: '/disposable/projects/source', projectDirectory: '/disposable/projects' }; } });
  await begin();
  assert.ok(document.body.textContent.includes('应用已添加，源码位置还需处理'));
  assert.ok(!commands(fixture).includes('source_import_cleanup'));
  await click(byText('重试整理源码'));
  assert.ok(document.body.textContent.includes('已添加「Disposable app」'));
  assert.equal(publications, 2);
  assert.equal(commands(fixture).filter(name => name === 'node_install_apply').length, 1);
  assert.equal(fixture.registered, 1);
});

test('Background continuation hides the dialog but retains the exact session and installs once', async () => {
  const acquisition = deferred(); let background = 0; let attention = 0;
  const fixture = await setup({ source_acquire: () => acquisition.promise }, { onBackground: () => { background++; }, onAttention: () => { attention++; } });
  await begin();
  assert.equal(document.querySelector('.modal-close').disabled, false);
  await click(byText('在后台继续'));
  assert.equal(background, 1); assert.equal(fixture.closed, 0);
  await fixture.render({ open: false });
  assert.equal(document.querySelector('[role=dialog]'), null);
  await act(async () => acquisition.resolve(task('sourceAcquire')));
  assert.equal(fixture.registered, 1);
  assert.equal(commands(fixture).filter(name => name === 'source_acquire').length, 1);
  assert.equal(commands(fixture).filter(name => name === 'node_install_apply').length, 1);
  assert.equal(attention, 1);
  await fixture.render({ open: true });
  assert.ok(document.body.textContent.includes('已添加「Disposable app」'));
});

test('A repository already added requires an explicit decision before downloading another independent copy', async () => {
  const fixture = await setup({}, { existingRepositories: [{ url: source.url.replace(/\.git$/, ''), appId, name: 'Existing app' }] });
  await begin();
  assert.ok(document.body.textContent.includes('这个仓库已经添加过：「Existing app」'));
  assert.deepEqual(fixture.calls, []);
  await click(byText('仍然另外添加一份'));
  assert.equal(commands(fixture).filter(name => name === 'source_acquire').length, 1);
  assert.equal(fixture.registered, 1);
});

test('The last submitted address survives closing and reopening the form', async () => {
  const fixture = await setup({ source_acquire: () => { throw { code: 'SOURCE_URL_INVALID', message: 'Address unavailable' }; } });
  await begin();
  await act(async () => root.unmount()); root = null;
  await setup();
  assert.equal(document.getElementById('git-import-url').value, source.url);
  assert.equal(fixture.calls.filter(([command]) => command === 'source_acquire').length, 1);
});
