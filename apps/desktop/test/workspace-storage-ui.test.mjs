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
await writeFile(mockPath, 'export const invoke = (...args) => globalThis.__WORKSPACE_STORAGE_INVOKE__(...args);\n');
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
for (const name of ['window', 'document', 'Node', 'HTMLElement', 'HTMLInputElement', 'HTMLSelectElement', 'Event', 'MouseEvent', 'MutationObserver', 'KeyboardEvent']) Object.defineProperty(globalThis, name, { value: name === 'window' ? dom.window : dom.window[name], configurable: true });
Object.defineProperty(globalThis, 'navigator', { value: dom.window.navigator, configurable: true });
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
globalThis.getComputedStyle = dom.window.getComputedStyle.bind(dom.window);
dom.window.HTMLElement.prototype.scrollIntoView = () => {};
const { createElement: h, act, useState } = await import('react');
const { createRoot } = await import('react-dom/client');
const { WorkspaceStoragePanel } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/WorkspaceStoragePanel.tsx'))).href);
const { resetHistoryScanForTests } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/history-scan-store.ts'))).href);
const { SettingsDialog } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/SettingsDialog.tsx'))).href);
let root;
afterEach(async () => { if (root) await act(async () => root.unmount()); root = null; });
after(async () => { dom.window.close(); await rm(output, { recursive: true, force: true }); });
function deferred() { let resolve; const promise = new Promise(yes => { resolve = yes; }); return { promise, resolve }; }
const makeTask = (id, type, extra = {}) => ({ taskId: id, type, projectId: null, status: 'succeeded', final: true, phase: 'completed', error: null, result: { completed: true }, ...extra });
const project = (id, bytes = 100) => ({ id, name: `Disposable ${id}`, projectRoot: `/disposable/${id}`, documentEpoch: 'epoch_fixture', state: 'ready', issues: [], manifest: {}, storage: { currentProject: { status: 'completed', totalBytes: bytes, complete: true, categories: { dependencies: { bytes: 20 }, cache: { bytes: 10 } } } } });
function plan(extra = {}) { return { planId: 'cleanup_plan_disposable', workspaceRoot: '/disposable/workspace', expiresAt: Date.now() + 60000, confirmationText: '永久清理所选内容', preserved: ['当前源码与数据'], items: [{ id: 'original_disposable', kind: 'restoreOriginal', name: 'Disposable recovered project', label: '源码取回原件', projectId: 'web_one', path: '/disposable/original', totalBytes: 40, fileCount: 1, canClean: true, blockedReasons: [] }, { id: 'protected_disposable', kind: 'restoreOriginal', name: 'Protected fixture', label: '数据取回原件', path: '/disposable/protected', totalBytes: 99, fileCount: 2, canClean: false, blockedReasons: ['Recovery still active'] }], ...extra }; }
async function setup({ props = {}, overrides = {} } = {}) {
  resetHistoryScanForTests(); const calls = []; const pending = []; const tasks = [];
  const projects = [project('web_one'), project('web_two')];
  globalThis.__WORKSPACE_STORAGE_INVOKE__ = async (command, args) => {
    calls.push([command, args]);
    if (overrides[command]) return overrides[command](args);
    if (command === 'environment_doctor') return { ready: true, node: null, npm: null, selection: 'managed', issues: [] };
    if (command === 'environment_git_doctor') return { ready: true, git: null, selection: 'managed', ssh: { ready: false, executable: null, issues: [] }, authentication: { reason: 'Disposable fixture' }, issues: [] };
    if (command === 'workspace_storage') return { platformProgram: { totalBytes: 999999 }, knownLocalTotal: { totalBytes: 9999999 }, workspace: { projects: projects.map((value, i) => ({ scope: { projectId: value.id }, totalBytes: 200 + i * 100, complete: true })) }, nodeApps: [{ scope: { appId: 'node_one' }, totalBytes: 400, complete: true }], disks: [{ exampleRoot: '/disposable/workspace', availableBytes: 5000 }], otherLocalLocations: [] };
    if (command === 'workspace_cleanup_preview') return makeTask('preview', 'workspaceCleanupPreview', { result: plan() });
    if (command === 'workspace_cleanup') return makeTask('history', 'workspaceCleanup');
    if (command === 'storage_cleanup') return makeTask(args.projectId, 'storageCleanup', { projectId: args.projectId });
    if (command === 'open_project_folder') return null;
    throw new Error(`Unexpected disposable command ${command}`);
  };
  document.body.innerHTML = '<div id="root"></div>'; root = createRoot(document.getElementById('root'));
  let current = { workspace: { workspaceRoot: '/disposable/workspace', migrationState: 'ready', retainedRoots: [] }, revision: 1, enabled: true, active: true, migrating: false, cleanupBusy: false, projects, nodeApps: [{ id: 'node_one', name: 'Disposable node' }], onTask: task => tasks.push(task), onProjectTask: task => tasks.push(task), onRefresh: async () => {}, onPendingChange: value => pending.push(value), ...props };
  const render = async changes => { current = { ...current, ...changes }; await act(async () => root.render(h(WorkspaceStoragePanel, current))); };
  await render({}); return { calls, pending, tasks, render };
}
const click = async element => { assert.ok(element); await act(async () => element.click()); };
const button = text => [...document.querySelectorAll('button')].find(value => value.textContent === text);
const mutations = calls => calls.filter(([command]) => ['workspace_cleanup', 'storage_cleanup'].includes(command));
async function review() {
  await click(button('清理'));
  await click(document.querySelector('[aria-label="选择「Disposable web_one」的临时文件"]'));
  await click(document.querySelector('[aria-label="选择「Disposable web_two」的临时文件"]'));
  await click(document.querySelector('.workspace-cleanup-item input[aria-label*="original_disposable"], .workspace-cleanup-item input[aria-label*="Disposable recovered project"]'));
  await click(button('清理所选…'));
  await click(document.querySelector('.checkbox-label input'));
}
const settle = async () => { await act(async () => { await new Promise(resolve => setTimeout(resolve, 550)); }); };

test('Storage settings show application-only totals, descending rows, disk space and the actual Finder path without history scanning', async () => {
  const { calls } = await setup();
  assert.match(document.querySelector('.space-totals').textContent, /应用共占用900 B.*可以清理约 60 B.*磁盘剩余5\.0 KB/);
  assert.deepEqual([...document.querySelectorAll('.storage-application-row')].map(value => value.querySelector('.space-project-name strong').textContent), ['Disposable node', 'Disposable web_two', 'Disposable web_one']);
  assert.equal(document.querySelector('.workspace-hub'), null);
  assert.doesNotMatch(document.body.textContent, /软件本身与记录/);
  assert.equal(calls.filter(([command]) => command === 'workspace_cleanup_preview').length, 0);
  await click(button('在访达中打开'));
  assert.deepEqual(calls.find(([command]) => command === 'open_project_folder'), ['open_project_folder', { path: '/disposable/workspace' }]);
});

test('Inactive storage settings perform no global scan; application list initially shows five and expands all', async () => {
  const projects = Array.from({ length: 7 }, (_, i) => project(`web_${i}`, 100 - i));
  const { calls, render } = await setup({ props: { active: false, projects, nodeApps: [] } });
  assert.equal(calls.length, 0);
  await render({ active: true });
  assert.equal(document.querySelectorAll('.storage-application-row').length, 5);
  await click(button('查看全部 7 个'));
  assert.equal(document.querySelectorAll('.storage-application-row').length, 7);
});

test('Cleanup reviews project, source, path and estimated sizes before any write; protected originals remain disabled', async () => {
  const { calls } = await setup(); await click(button('清理'));
  assert.equal(mutations(calls).length, 0);
  const dialog = document.querySelector('[role="dialog"]');
  assert.match(dialog.textContent, /Disposable web_one.*来源：开发依赖、构建缓存与可清理的任务日志.*\/disposable\/web_one.*约 30 B/s);
  assert.match(dialog.textContent, /Disposable recovered project.*\/disposable\/original.*40 B/s);
  const protectedItem = [...document.querySelectorAll('.workspace-cleanup-item')].find(value => value.textContent.includes('Protected fixture'));
  assert.equal(protectedItem.querySelector('input').disabled, true);
  assert.match(protectedItem.textContent, /Recovery still active/);
});

test('One confirmation executes history first and waits for each durable final result before the next application', async () => {
  const history = deferred(); const first = deferred(); const second = deferred();
  const { calls, pending } = await setup({ overrides: {
    workspace_cleanup: () => makeTask('history', 'workspaceCleanup', { final: false, status: 'running' }),
    storage_cleanup: args => makeTask(args.projectId, 'storageCleanup', { projectId: args.projectId, final: false, status: 'running' }),
    task_get: args => args.taskId === 'history' ? history.promise : args.taskId === 'web_one' ? first.promise : second.promise,
  } });
  await review();
  const confirm = [...document.querySelectorAll('button')].find(value => value.textContent.startsWith('永久清理 '));
  assert.match(document.querySelector('[role="dialog"]').textContent, /来源：取回前的原件.*\/disposable\/original/s);
  await click(confirm);
  assert.deepEqual(mutations(calls).map(([command]) => command), ['workspace_cleanup']);
  assert.deepEqual(pending, [true]);
  await settle(); await act(async () => history.resolve(makeTask('history', 'workspaceCleanup'))); await settle();
  assert.deepEqual(mutations(calls).map(([command, args]) => [command, args.projectId]), [['workspace_cleanup', undefined], ['storage_cleanup', 'web_one']]);
  await act(async () => first.resolve(makeTask('web_one', 'storageCleanup', { projectId: 'web_one' }))); await settle();
  assert.equal(mutations(calls).at(-1)[1].projectId, 'web_two');
  assert.deepEqual(pending, [true]);
  await act(async () => second.resolve(makeTask('web_two', 'storageCleanup', { projectId: 'web_two' }))); await settle();
  assert.deepEqual(pending, [true, false]);
  assert.deepEqual(mutations(calls).filter(([command]) => command === 'storage_cleanup').map(([, args]) => args.input.categories), [['dependencies', 'buildCache', 'logs'], ['dependencies', 'buildCache', 'logs']]);
  assert.equal(mutations(calls).length, 3);
});

test('Failed history cleanup stops the batch and never submits later temporary cleanup', async () => {
  const { calls, pending } = await setup({ overrides: { workspace_cleanup: () => makeTask('failed_history', 'workspaceCleanup', { status: 'failed', error: { code: 'FIXTURE', message: 'Disposable failed history' }, result: { completed: false, items: [] } }) } });
  await review(); await click([...document.querySelectorAll('button')].find(value => value.textContent.startsWith('永久清理 ')));
  assert.deepEqual(mutations(calls).map(([command]) => command), ['workspace_cleanup']);
  assert.deepEqual(pending, [true, false]);
  assert.match(document.body.textContent, /后续临时文件清理未提交.*不会自动重复提交/s);
});

test('Changing storage revision cancels a reviewed confirmation before deletion', async () => {
  const { calls, render } = await setup(); await review();
  assert.ok([...document.querySelectorAll('button')].some(value => value.textContent.startsWith('永久清理 ')));
  await render({ revision: 2 });
  assert.equal([...document.querySelectorAll('button')].some(value => value.textContent.startsWith('永久清理 ')), false);
  assert.equal(mutations(calls).length, 0);
});

test('Unknown accepted task status stops later submissions and unlocks settings without replaying the original request', async () => {
  const { calls, pending } = await setup({ overrides: {
    workspace_cleanup: () => makeTask('history_unknown', 'workspaceCleanup', { final: false, status: 'running' }),
    task_get: () => { throw new Error('Disposable response unavailable'); },
  } });
  await review(); await click([...document.querySelectorAll('button')].find(value => value.textContent.startsWith('永久清理 '))); await settle();
  assert.deepEqual(mutations(calls).map(([command]) => command), ['workspace_cleanup']);
  assert.deepEqual(pending, [true, false]);
  assert.match(document.body.textContent, /后续项目未提交.*不会重复提交/s);
  await settle();
  assert.equal(mutations(calls).length, 1);
});

test('A mismatched accepted cleanup receipt cannot advance to later projects', async () => {
  const { calls, pending } = await setup({ overrides: { workspace_cleanup: () => makeTask('wrong_type', 'storageCleanup') } });
  await review(); await click([...document.querySelectorAll('button')].find(value => value.textContent.startsWith('永久清理 ')));
  assert.deepEqual(mutations(calls).map(([command]) => command), ['workspace_cleanup']);
  assert.deepEqual(pending, [true, false]);
  assert.match(document.body.textContent, /清理任务回执身份或格式不符/);
});

test('A same-root service change invalidates review and cannot advance an already accepted history task', async () => {
  const final = deferred();
  const { calls, pending, render } = await setup({ props: { instanceId: 'instance_a' }, overrides: {
    workspace_cleanup: () => makeTask('history_restart', 'workspaceCleanup', { final: false, status: 'running' }), task_get: () => final.promise,
  } });
  await review(); await render({ instanceId: 'instance_b' });
  assert.equal([...document.querySelectorAll('button')].some(value => value.textContent.startsWith('永久清理 ')), false);
  await render({ instanceId: 'instance_a' });
  await click(button('清理所选…')); await click(document.querySelector('.checkbox-label input'));
  await click([...document.querySelectorAll('button')].find(value => value.textContent.startsWith('永久清理 '))); await settle();
  await render({ instanceId: 'instance_b' });
  await act(async () => final.resolve(makeTask('history_restart', 'workspaceCleanup'))); await settle();
  assert.deepEqual(mutations(calls).map(([command]) => command), ['workspace_cleanup']);
  assert.deepEqual(pending, [true, false]);
});

test('Replacing a selected project identity clears confirmation before any cleanup request', async () => {
  const { calls, render } = await setup(); await review();
  await render({ projects: [{ ...project('web_one'), documentEpoch: 'replacement_epoch' }, project('web_two')] });
  assert.equal([...document.querySelectorAll('button')].some(value => value.textContent.startsWith('永久清理 ')), false);
  assert.equal(mutations(calls).length, 0);
});

test('Settings use the new classifications and keep close and classification navigation locked for an executing batch', async () => {
  const completion = deferred(); const pending = []; let closes = 0;
  await setup({ overrides: { workspace_cleanup: () => makeTask('history_settings', 'workspaceCleanup', { final: false, status: 'running' }), task_get: () => completion.promise } });
  function Harness() {
    const [section, onSection] = useState('storage');
    return h(SettingsDialog, {
      info: { workspaceRoot: '/disposable/workspace', dataRoot: '/disposable/data', instanceId: 'instance_settings', resources: { sdk: true, template: true, readme: true, cli: true } },
      projects: [project('web_one'), project('web_two')], nodeApps: [], loading: false, checkedAt: null, theme: 'light', onTheme() {}, section, onSection,
      workspace: { workspaceRoot: '/disposable/workspace', migrationState: 'ready', retainedRoots: [] }, workspaceBlocked: false, cleanupBusy: false, closeDisabled: false, localDataLocked: false, localDataUnavailable: false, guideEnabled: true, feedback: null,
      revision: 1, onTask() {}, onProjectTask() {}, onRefresh: async () => {}, onStoragePending: value => pending.push(value), onClose: () => { closes++; }, onCreateProject() {}, onEnvironmentPending() {}, onGitPending() {}, onLocalDataPending() {}, onLocalDataLock() {},
    });
  }
  await act(async () => root.render(h(Harness)));
  assert.deepEqual([...document.querySelectorAll('.settings-nav button')].map(value => value.textContent), ['通用', '存储与清理', '运行环境', '给 AI 的平台说明', '关于与更新', '卸载前清除数据']);
  assert.equal(document.querySelector('#settings-tools-tab, #settings-diagnostics-tab'), null);
  await review(); await click([...document.querySelectorAll('button')].find(value => value.textContent.startsWith('永久清理 ')));
  assert.equal(button('完成').disabled, true);
  assert.equal([...document.querySelectorAll('.settings-nav button')].every(value => value.disabled), true);
  assert.equal(document.querySelector('[aria-label="关闭设置"]').disabled, true);
  await click(document.querySelector('#settings-general-tab'));
  assert.equal(document.querySelector('#settings-storage-tab').getAttribute('aria-selected'), 'true');
  assert.equal(closes, 0); assert.deepEqual(pending, [true]);
  await settle(); await act(async () => completion.resolve(makeTask('history_settings', 'workspaceCleanup'))); await settle();
  assert.equal(button('完成').disabled, false);
  assert.deepEqual(pending, [true, false]);
});
