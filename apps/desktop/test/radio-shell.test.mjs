// Management-shell regressions use disposable DOM fixtures and mocked native
// commands only. They never open the real development workspace or a service.
import assert from 'node:assert/strict';
import { after, afterEach, test } from 'node:test';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';
import { JSDOM } from 'jsdom';

const repo = fileURLToPath(new URL('../../../', import.meta.url));
await mkdir(path.join(repo, '.test-artifacts'), { recursive: true });
const output = await mkdtemp(path.join(repo, '.test-artifacts/radio-shell-dom-'));
const mockPath = path.join(output, 'tauri-mock.mjs');
await writeFile(mockPath, 'export const invoke = (command, ...args) => command === "source_import_recover" ? Promise.resolve(globalThis.__RADIO_SHELL_RECOVERIES__ ?? []) : command === "node_app_delete_tasks" ? Promise.resolve([]) : globalThis.__RADIO_SHELL_INVOKE__(command, ...args); export const isTauri = () => true; export const listen = async () => () => {}; export const getAllWindows = async () => globalThis.__RADIO_SHELL_WINDOWS__ ?? [];\n');
const compiled = new Map();
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

const dom = new JSDOM('<!doctype html><html><body></body></html>', { url: 'http://localhost/', pretendToBeVisual: true });
for (const name of ['window', 'document', 'Node', 'HTMLElement', 'HTMLInputElement', 'HTMLSelectElement', 'Event', 'MouseEvent', 'KeyboardEvent', 'FocusEvent', 'MutationObserver']) Object.defineProperty(globalThis, name, { value: name === 'window' ? dom.window : dom.window[name], configurable: true });
Object.defineProperty(globalThis, 'navigator', { value: dom.window.navigator, configurable: true });
Object.defineProperty(globalThis, 'localStorage', { value: dom.window.localStorage, configurable: true });
globalThis.getComputedStyle = dom.window.getComputedStyle.bind(dom.window);
dom.window.HTMLElement.prototype.scrollIntoView = () => {};
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
let nextTimer = 0;
const intervals = new Map();
dom.window.setInterval = (callback, delay) => { const id = ++nextTimer; intervals.set(id, { callback, delay }); return id; };
dom.window.clearInterval = id => intervals.delete(id);
async function tick(delay = 2000) { await act(async () => { for (const timer of [...intervals.values()]) if (timer.delay === delay) await timer.callback(); }); }
globalThis.__RADIO_SHELL_INVOKE__ = async command => { throw new Error(`Unexpected fixture command: ${command}`); };
const { createElement: h, act } = await import('react');
const { createRoot } = await import('react-dom/client');
const { default: App } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/App.tsx'))).href);
const { resetProjectDeletionsForTests } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/project-deletion.ts'))).href);
const { APPEARANCE_KEY } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/appearance.ts'))).href);
const storagePrototype = dom.window.Storage.prototype;
const originalGetItem = Object.getOwnPropertyDescriptor(storagePrototype, 'getItem');
const originalSetItem = Object.getOwnPropertyDescriptor(storagePrototype, 'setItem');
let root;
async function unmount() { if (root) await act(async () => root.unmount()); root = null; }
afterEach(async () => {
  await unmount();
  resetProjectDeletionsForTests();
  intervals.clear();
  globalThis.__RADIO_SHELL_WINDOWS__ = [];
  globalThis.__RADIO_SHELL_RECOVERIES__ = [];
  Object.defineProperty(storagePrototype, 'getItem', originalGetItem);
  Object.defineProperty(storagePrototype, 'setItem', originalSetItem);
  dom.window.localStorage.clear();
  delete document.documentElement.dataset.theme;
  globalThis.__RADIO_SHELL_INVOKE__ = async command => { throw new Error(`Unexpected fixture command: ${command}`); };
});
after(async () => { dom.window.close(); await rm(output, { recursive: true, force: true }); });

const bootstrap = {
  displayName: 'Disposable Panda Drawer fixture', identifier: 'dev.fixture.radio-shell', environment: 'development', apiVersion: 1,
  instanceId: 'instance_radio_shell_fixture', baseUrl: 'http://127.0.0.1:1', workspaceRoot: '/disposable/radio-shell/workspace', dataRoot: '/disposable/radio-shell/data', sqliteVersion: 'fixture', version: '0.0.0-fixture',
  resources: { sdk: true, template: true, readme: true, cli: true }, cliPath: '/disposable/radio-shell/cli',
  platform: { build: { target: 'aarch64-apple-darwin' } },
};
const environment = { ready: true, node: { path: '/disposable/node', version: 'v24.0.0' }, npm: { path: '/disposable/npm', version: '11.0.0' }, selection: 'detected', configuredNodePath: null, supportedNodeRange: 'fixture', recommendedNode: 'fixture', downloadUrl: 'https://example.invalid/node', issues: [] };
const gitEnvironment = { ready: true, git: { path: '/disposable/platform/git/bin/git', version: '2.56.0' }, selection: 'managed', configuredGitPath: null, managedGitPath: '/disposable/platform/git/bin/git', ssh: { ready: true, selection: 'system', executable: { path: '/usr/bin/ssh', version: 'OpenSSH fixture' }, issues: [] }, authentication: { state: 'not-checked', reason: '只检测工具，不访问仓库。' }, issues: [] };
const nodeFixture = { id: 'node_radio_shell_fixture', name: 'Disposable shell Node', sourcePath: '/disposable/radio-shell/source', runtime: 'node', state: 'stopped', restartRequired: false, runningRevision: null, lastError: null, configuration: { revision: 1, values: {}, secretStatus: {}, missingRequired: ['KEY'], canStart: false }, fields: [] };
function deferred() { let resolve; let reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { resolve, reject, promise }; }
async function setup({ projects = [], nodes = [], overrides = {}, windows = [], recoveries = [] } = {}) {
  globalThis.__RADIO_SHELL_WINDOWS__ = windows;
  globalThis.__RADIO_SHELL_RECOVERIES__ = recoveries;
  const calls = [];
  globalThis.__RADIO_SHELL_INVOKE__ = async (command, args) => {
    calls.push([command, args]);
    if (overrides[command]) return overrides[command](args);
    if (command === 'detect_ai_tools') return [];
    if (command === 'app_catalog') return { version: 1, apps: [] };
    if (command === 'bootstrap_info') return bootstrap;
    if (command === 'project_list') return projects;
    if (command === 'node_app_list') return nodes;
    if (command === 'source_list') return [];
    if (command === 'workspace_info') return { workspaceRoot: bootstrap.workspaceRoot, migrationState: 'ready', activeTask: null, retainedRoots: [] };
    if (command === 'maintenance_status') return { mode: 'ready', recoveryRequired: false, activeTask: null };
    if (command === 'environment_doctor') return environment;
    if (command === 'environment_git_doctor') return gitEnvironment;
    if (command === 'workspace_storage') return { platformProgram: { totalBytes: 0, complete: true }, workspace: { totalBytes: 0, complete: true, categories: {} }, otherLocalLocations: [] };
    throw new Error(`Unexpected fixture command: ${command}`);
  };
  // These shell fixtures exercise an existing user's workspace, not first-run AI selection.
  try { window.localStorage.setItem('panda-drawer:ai-tools:v1', JSON.stringify({ acknowledged: true, selectedToolId: 'other', hiddenGuides: [] })); } catch { /* The storage-unavailable test dismisses the real first-run dialog below. */ }
  document.body.innerHTML = '<div id="root"></div>';
  root = createRoot(document.getElementById('root'));
  await act(async () => root.render(h(App)));
  const firstRun = document.querySelector('.ai-tool-chooser');
  if (firstRun) await click(firstRun.querySelector('.modal-close'));
  return calls;
}
const byText = (text, scope = document, selector = 'button') => [...scope.querySelectorAll(selector)].find(element => element.textContent === text);
const dialog = () => document.querySelector('[role="dialog"]');
const addGroup = () => document.querySelector('.application-heading-actions');
const welcomeLinks = () => document.querySelector('.app-library-links');
async function click(element) { assert.ok(element); await act(async () => element.click()); }
async function key(element, value, extra = {}) { assert.ok(element); await act(async () => element.dispatchEvent(new window.KeyboardEvent('keydown', { key: value, bubbles: true, cancelable: true, ...extra }))); }
async function change(element, value) {
  assert.ok(element);
  await act(async () => {
    Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set.call(element, value);
    element.dispatchEvent(new window.Event('input', { bubbles: true }));
  });
}

test('Windows shell preserves native tool arguments and current-folder controls without new migration', async () => {
  const windowsBootstrap = { ...bootstrap, workspaceRoot: String.raw`D:\可丢弃 工作区`, platform: { build: { target: 'x86_64-pc-windows-msvc' } } };
  const nodePath = String.raw`C:\Program Files\nodejs\node.exe`;
  const gitPath = String.raw`\\server\share\工具\git.exe`;
  const calls = await setup({ overrides: {
    bootstrap_info: () => windowsBootstrap,
    workspace_info: () => ({ workspaceRoot: windowsBootstrap.workspaceRoot, migrationState: 'ready', retainedRoots: [] }),
    open_project_folder: () => null,
    environment_node_set: ({ nodePath }) => ({ ...environment, selection: 'explicit', configuredNodePath: nodePath }),
    environment_git_set: ({ gitPath }) => ({ ...gitEnvironment, selection: 'explicit', configuredGitPath: gitPath }),
  } });
  await click(byText('在资源管理器中打开', document.querySelector('.radio-footer')));
  assert.deepEqual(calls.find(([command]) => command === 'open_project_folder'), ['open_project_folder', { path: windowsBootstrap.workspaceRoot }]);
  await click(document.querySelector('[aria-label="设置"]'));
  assert.match(document.querySelector('#settings-general-panel').textContent, /关闭主窗口或从菜单退出/);
  assert.doesNotMatch(document.querySelector('#settings-general-panel').textContent, /Dock|Cmd\+Q|macOS/);
  await click(document.querySelector('#settings-runtime-tab'));
  const nodePanel = document.querySelector('.build-environment');
  const gitPanel = document.querySelector('.git-environment');
  assert.match(document.getElementById('node-path').placeholder, /C:.*node\.exe/);
  assert.match(document.getElementById('git-path').placeholder, /C:.*git\.exe/);
  for (const [id, panel, path] of [['node-path', nodePanel, nodePath], ['git-path', gitPanel, gitPath]]) {
    await change(document.getElementById(id), 'C:relative.exe');
    assert.equal(byText('保存路径', panel).disabled, true);
    const before = calls.filter(([command]) => command === 'environment_node_set' || command === 'environment_git_set').length;
    await act(async () => panel.querySelector('form').dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true })));
    assert.equal(calls.filter(([command]) => command === 'environment_node_set' || command === 'environment_git_set').length, before);
    await change(document.getElementById(id), `  ${path}  `);
    assert.equal(byText('保存路径', panel).disabled, false);
    await click(byText('保存路径', panel));
  }
  assert.deepEqual(calls.find(([command]) => command === 'environment_node_set'), ['environment_node_set', { nodePath }]);
  assert.deepEqual(calls.find(([command]) => command === 'environment_git_set'), ['environment_git_set', { gitPath }]);
  await click(document.querySelector('#settings-storage-tab'));
  const location = document.querySelector('.workspace-location-card');
  await click(byText('在资源管理器中打开', location));
  assert.deepEqual(calls.filter(([command]) => command === 'open_project_folder'), [
    ['open_project_folder', { path: windowsBootstrap.workspaceRoot }],
    ['open_project_folder', { path: windowsBootstrap.workspaceRoot }],
  ]);
  assert.equal(byText('更换位置', location), undefined);
  assert.equal(location.querySelector('.settings-migration') === null, true);
  assert.equal(location.querySelector('.path-input input') === null, true);
  assert.match(location.textContent, /可丢弃 工作区/);
  const readsBefore = calls.filter(([command]) => command === 'workspace_info').length;
  await click(byText('刷新状态', location));
  assert.equal(calls.filter(([command]) => command === 'workspace_info').length, readsBefore + 1);
  assert.match(document.querySelector('.workspace-location-card').textContent, /可丢弃 工作区/);
  assert.equal(calls.some(([command]) => command === 'workspace_migrate' || command === 'choose_path'), false);
});

for (const [platformName, target, linkLabel, otherLabel, migrationAvailable] of [
  ['Windows', 'x86_64-pc-windows-msvc', '查看项目目录…', '更改项目目录…', false],
  ['Mac', 'aarch64-apple-darwin', '更改项目目录…', '查看项目目录…', true],
]) {
  test(`${platformName} Git addition labels its existing storage link accurately without changing its destination`, async () => {
    const platformBootstrap = { ...bootstrap, platform: { build: { target } } };
    const calls = await setup({ overrides: { bootstrap_info: () => platformBootstrap } });
    await click(byText('粘贴 GitHub / Gitee 链接', welcomeLinks()));
    const sourceDialog = document.querySelector('.source-acquisition-modal');
    assert.ok(byText(linkLabel, sourceDialog));
    assert.equal(byText(otherLabel, sourceDialog), undefined);
    await click(byText(linkLabel, sourceDialog));
    assert.equal(document.querySelector('.source-acquisition-modal') === null, true);
    assert.equal(document.querySelector('#settings-storage-tab').getAttribute('aria-selected'), 'true');
    const location = document.querySelector('.workspace-location-card');
    assert.ok(location.textContent.includes(bootstrap.workspaceRoot));
    assert.equal(Boolean(byText('更换位置', location)), migrationAvailable);
    assert.equal(calls.some(([command]) => /^(node_git_import_start|source_acquire|source_capture|workspace_migrate|choose_path)$/.test(command)), false);
  });
}

test('Missing bootstrap stays neutral and switches only after the real compiled target arrives', async () => {
  const read = deferred();
  await setup({ overrides: { bootstrap_info: () => read.promise } });
  const footer = document.querySelector('.radio-footer');
  assert.equal(byText('在文件管理器中打开', footer).disabled, true);
  assert.equal(byText('在访达中打开', footer), undefined);
  await click(document.querySelector('[aria-label="设置"]'));
  assert.doesNotMatch(document.querySelector('#settings-general-panel').textContent, /Dock|Cmd\+Q|macOS/);
  await act(async () => read.resolve({ ...bootstrap, platform: { build: { target: 'x86_64-pc-windows-msvc' } } }));
  assert.equal(byText('在资源管理器中打开', footer).disabled, false);
  assert.match(document.querySelector('#settings-general-panel').textContent, /关闭主窗口或从菜单退出/);
});

test('The real shell has one application home and the reorganized settings portal', async () => {
  const calls = await setup();
  assert.equal(document.querySelector('[aria-label="工作台导航"]') === null, true);
  assert.equal(document.querySelector('h1').textContent, '我的应用');
  assert.equal(document.querySelector('.build-environment') === null, true);
  assert.equal(document.querySelector('.about-platform') === null, true);
  assert.equal(document.querySelector('.workspace-hub') === null, true);
  assert.equal(document.querySelector('.radio-toolbar') === null, true);
  await click(document.querySelector('[aria-label="设置"]'));
  assert.equal(document.querySelectorAll('[role="dialog"]').length, 1);
  assert.equal(dialog().querySelector('.modal-header h2').textContent, '设置');
  assert.equal(dialog().closest('.radio-shell') === null, true);
  assert.equal(document.querySelector('.modal-layer').parentElement === document.body, true);
  assert.equal(document.querySelector('#root').hasAttribute('inert'), true);
  assert.ok(dialog().querySelector('[aria-labelledby="settings-runtime-title"]'));
  assert.ok(dialog().querySelector('[aria-labelledby="settings-about-title"]'));
  assert.deepEqual([...dialog().querySelectorAll('[role="tablist"] button')].map(item => item.textContent), ['通用', '存储与清理', '运行环境', '给 AI 的平台说明', '关于与更新', '卸载前清除数据']);
  assert.equal(document.querySelector('#settings-tools-tab') === null, true);
  assert.equal(document.querySelector('#settings-diagnostics-tab') === null, true);
  assert.match(dialog().textContent, /运行环境/);
  assert.match(dialog().textContent, /关于/);
  assert.match(dialog().textContent, /0\.0\.0-fixture/);
  await click(byText('完成', dialog()));
  assert.equal(dialog() === null, true);
  assert.equal(document.querySelector('h1').textContent, '我的应用');
  assert.equal(calls.some(([command]) => /project_create|workspace_migrate|environment_node_set/.test(command)), false);
});

test('Light and dark choices persist and apply to html so body-level settings portals share the theme', async () => {
  window.localStorage.setItem(APPEARANCE_KEY, 'dark');
  await setup();
  assert.equal(document.documentElement.dataset.theme, 'dark');
  assert.equal(document.querySelector('.radio-shell').dataset.theme, 'dark');
  assert.equal(document.querySelector('[aria-label="切换暗色主题"]') === null, true, 'Appearance lives in settings, not the header');
  await click(document.querySelector('[aria-label="设置"]'));
  assert.equal(document.querySelector('#settings-general-tab').getAttribute('aria-selected'), 'true');
  assert.equal(document.querySelector('[aria-label="切换暗色主题"]').getAttribute('aria-pressed'), 'true');
  await click(document.querySelector('[aria-label="切换亮色主题"]'));
  assert.equal(document.documentElement.dataset.theme, 'light');
  assert.equal(window.localStorage.getItem(APPEARANCE_KEY), 'light');
  await unmount();
  await setup();
  assert.equal(document.documentElement.dataset.theme, 'light');
  await click(document.querySelector('[aria-label="设置"]'));
  await click(document.querySelector('[aria-label="切换暗色主题"]'));
  assert.equal(window.localStorage.getItem(APPEARANCE_KEY), 'dark');
  assert.equal(dialog().closest('.radio-shell') === null, true);
  assert.equal(document.documentElement.dataset.theme, 'dark');
  await click(document.querySelector('[aria-label="关闭设置"]'));
  await unmount();
  await setup();
  assert.equal(document.documentElement.dataset.theme, 'dark');
});

test('Launcher items show icon, name and one type or status line, with contextual actions and normal opening preserved', async () => {
  const project = { id: 'prj_disposable_compact_launcher', name: 'Disposable long launcher name', description: 'Artificial project description', projectRoot: '/disposable/radio-shell/project', documentEpoch: 'fixture', state: 'ready', issues: [], manifest: null };
  const calls = await setup({ projects: [project], nodes: [nodeFixture], overrides: { project_open: () => ({ projectId: project.id, target: 'desktop', reused: false }) } });
  const entry = document.querySelector(`[aria-label='进入项目「${project.name}」']`);
  const tile = entry.closest('article');
  assert.equal(document.querySelector('.launcher-more') === null, true);
  assert.equal(document.querySelector('.launcher-statuses') === null, true);
  assert.equal(entry.children.length, 3);
  assert.ok(entry.children[0].classList.contains('launcher-icon'));
  assert.ok(entry.children[1].classList.contains('launcher-name'));
  assert.ok(entry.children[2].classList.contains('launcher-state'));
  assert.equal(entry.children[2].textContent, '网页');
  assert.equal(tile.querySelectorAll('button').length, 1);
  assert.equal(tile.querySelector('.launcher-name').title, project.name);
  const nodeEntry = document.querySelector(`[aria-label='进入项目「${nodeFixture.name}」']`);
  assert.equal(nodeEntry.querySelector('.launcher-node-badge') === null, true);
  assert.match(nodeEntry.title, /需要配置/);
  assert.equal(nodeEntry.querySelector('.launcher-state').textContent, '需要配置');
  assert.ok(nodeEntry.querySelector('.launcher-state').classList.contains('is-attention'));
  assert.equal(nodeEntry.disabled, false);
  await act(async () => entry.focus());
  await key(entry, 'F10', { shiftKey: true });
  const menu = document.querySelector('[role="menu"]');
  assert.ok(menu);
  assert.equal(document.activeElement.getAttribute('aria-label'), `查看项目「${project.name}」的详情`);
  await key(document.activeElement, 'Escape');
  assert.equal(document.querySelector('[role="menu"]') === null, true);
  assert.equal(document.activeElement === entry, true);
  await act(async () => tile.dispatchEvent(new window.MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 120, clientY: 140 })));
  assert.ok(document.querySelector('[role="menu"]'));
  await key(document.activeElement, 'Escape');
  await click(entry);
  assert.deepEqual(calls.filter(([command]) => command === 'project_open').map(([, args]) => args), [{ projectId: project.id, target: 'desktop', reload: false }]);
});

test('Unavailable browser storage falls back to light and still permits both theme switches', async () => {
  Object.defineProperty(storagePrototype, 'getItem', { configurable: true, value() { throw new Error('Artificial storage read failure'); } });
  Object.defineProperty(storagePrototype, 'setItem', { configurable: true, value() { throw new Error('Artificial storage write failure'); } });
  await setup();
  assert.equal(document.documentElement.dataset.theme, 'light');
  await click(document.querySelector('[aria-label="设置"]'));
  assert.ok(dialog());
  await click(document.querySelector('[aria-label="切换暗色主题"]'));
  assert.equal(document.documentElement.dataset.theme, 'dark');
  await click(document.querySelector('[aria-label="切换亮色主题"]'));
  assert.equal(document.documentElement.dataset.theme, 'light');
});

test('The heading offers unified addition and filters, with the platform AI introduction in settings', async () => {
  await setup();
  const filter = document.querySelector('[aria-label="筛选项目状态"]');
  assert.ok(filter.closest('.application-heading'));
  assert.equal(document.querySelector('.radio-toolbar') === null, true);
  assert.equal(document.querySelector('input[type="search"]') === null, true);
  assert.equal(document.querySelector('[aria-label="刷新项目和运行状态"]') === null, true);
  assert.equal(document.querySelector('.application-heading [aria-label="给 AI 的平台说明"]') === null, true);
  assert.deepEqual([...addGroup().querySelectorAll('button')].map(element => element.textContent), ['添加应用']);
  assert.deepEqual([...filter.querySelectorAll('button')].map(item => item.textContent), ['全部', '运行中', '需要处理']);
  assert.ok(byText('添加应用', addGroup()).classList.contains('primary-button'));
  assert.equal(document.querySelector('.radio-add-menu') === null, true);
  assert.equal(byText('导入项目'), undefined, 'ZIP import has no home entry');
  await click(document.querySelector('[aria-label="设置"]'));
  await click(document.getElementById('settings-ai-tab'));
  assert.ok(byText('复制平台说明给 AI', document.getElementById('settings-ai-panel')));
});

for (const [label, title] of [['新建', '新建应用'], ['选择本机文件夹', '添加本地项目'], ['粘贴 GitHub / Gitee 链接', '添加应用']]) {
  test(`The ${label} action opens its own dialog without submitting a native mutation`, async () => {
    const calls = await setup();
    await click(byText(label, welcomeLinks()));
    assert.equal(dialog().querySelector('h2').textContent, title);
    assert.equal(calls.some(([command]) => /^(choose_path|project_import|project_create|node_app_register|source_acquire|source_capture|node_install_prepare|node_install_apply)$/.test(command)), false);
  });
}

async function completedGitImport() {
 const source={url:'https://github.com/disposable/shell-reopen.git'};
 const app={...nodeFixture,id:'11111111-1111-4111-8111-111111111111',name:'Disposable Git app',gitSource:{url:source.url,branch:null}};
 const task={taskId:'task_shell_import',projectId:null,type:'nodeGitImport',status:'succeeded',final:true,phase:'completed',result:null,error:null};
 let nodes=[];let acquisitions=0;
 const calls=await setup({overrides:{node_app_list:()=>nodes,node_git_import_start:()=>{acquisitions++;nodes=[app];return task;},node_git_import_status:()=>({task,appId:app.id,sourcePath:bootstrap.workspaceRoot+'/project-sources/shell-reopen',commitState:'committed',cleanupPending:false,remainingPaths:[],app}),node_app_configuration:()=>app,node_app_status:()=>app}});
 await click(byText('粘贴 GitHub / Gitee 链接',welcomeLinks()));await change(document.getElementById('git-import-url'),source.url);await click(byText('开始添加',dialog()));assert.match(dialog().textContent,/已添加「Disposable Git app」/);
 assert.equal(acquisitions,1);return {calls,source,app,removeFromList:()=>{nodes=[];}};
}

test('A completed Git addition closes its session and reopens a fresh form after the installed app is removed', async () => {
  const fixture = await completedGitImport();
  await click(byText('完成', dialog()));
  assert.equal(dialog() === null, true);
  fixture.removeFromList();
  await tick();
  assert.equal(document.querySelector(`[aria-label='进入项目「${fixture.app.name}」']`) === null, true);
  assert.equal(document.querySelector('.radio-task-button') === null, true, 'Completed history does not keep a task entry visible');
  await click(byText('粘贴 GitHub / Gitee 链接', welcomeLinks()));
  assert.equal(dialog().querySelector('h2').textContent, '添加应用');
  assert.equal(document.getElementById('git-import-url')?.value, '');
  assert.ok(byText('开始添加', dialog()));
  assert.doesNotMatch(dialog().textContent, /已添加「|临时文件已清理/);
  assert.equal(fixture.calls.filter(([command]) => command === 'node_git_import_start').length, 1, 'Reopening is not a new submission');
  await change(document.getElementById('git-import-url'), fixture.source.url);
  await click(byText('开始添加', dialog()));
  assert.equal(fixture.calls.filter(([command]) => command === 'node_git_import_start').length, 2, 'A removed app must not survive in the cached duplicate-repository check');
  assert.doesNotMatch(dialog().textContent, /这个仓库已经添加过/);
});

test('Entering configuration from a completed Git addition also retires that session before opening a new form', async () => {
  const fixture = await completedGitImport();
  await click(byText('填写配置', dialog()));
  assert.ok(document.querySelector('.node-detail-modal'));
  assert.equal(document.querySelector('.git-import-modal') === null, true);
  await click(document.querySelector('.node-detail-modal .modal-close'));
  assert.equal(dialog() === null, true);
  await click(byText('添加应用', addGroup()));
  assert.equal(document.getElementById('add-application-link')?.value, '');
  assert.ok(byText('添加', dialog()));
  assert.doesNotMatch(dialog().textContent, /已添加「|临时文件已清理/);
  assert.equal(fixture.calls.filter(([command]) => command === 'node_git_import_start').length, 1);
  assert.equal(fixture.calls.filter(([command]) => command === 'node_app_start').length, 0);
});

test('Automatic refresh tracks actual web windows and Node state and the footer opens the actual workspace', async () => {
  const make = (id, name) => ({ id, name, description: '', projectRoot: `/disposable/${id}`, documentEpoch: 'fixture', state: 'ready', issues: [], manifest: null });
  const web = make('prj_window', 'Window app');
  const node = { ...nodeFixture, name: 'Service app', configuration: { ...nodeFixture.configuration, missingRequired: [], canStart: true } };
  let nodes = [node];
  const calls = await setup({ projects: [web], windows: [{ label: `project-${web.id}` }], overrides: { node_app_list: () => nodes, open_project_folder: () => null } });
  assert.match(document.querySelector('.application-count').textContent, /2 个 · 1 个运行中/);
  await click(byText('运行中', document.querySelector('[aria-label="筛选项目状态"]')));
  const names = () => [...document.querySelectorAll('.project-grid .launcher-name')].map(item => item.textContent);
  assert.deepEqual(names(), ['Window app']);
  globalThis.__RADIO_SHELL_WINDOWS__ = [];
  nodes = [{ ...node, state: 'running' }];await tick();
  assert.deepEqual(names(), ['Service app']);assert.match(document.querySelector('.application-count').textContent, /2 个 · 1 个运行中/);
  const reads = calls.filter(([command]) => command === 'project_list').length;
  await tick();assert.ok(calls.filter(([command]) => command === 'project_list').length > reads);
  await click(byText('在访达中打开', document.querySelector('.radio-footer')));
  assert.deepEqual(calls.filter(([command]) => command === 'open_project_folder'), [['open_project_folder', { path: bootstrap.workspaceRoot }]]);
});

test('Active management work appears only in the task capsule and opens its popover', async () => {
  const task = { taskId: 'disposable_pending_task', projectId: null, type: 'workspaceMigrationCleanup', status: 'running', final: false, phase: 'validation', result: null, error: null };
  await setup({ overrides: { workspace_info: () => ({ workspaceRoot: bootstrap.workspaceRoot, migrationState: 'ready', retainedRoots: [], migrationCleanupTask: task }), task_get: () => task } });
  assert.equal(document.querySelector('.radio-task-indicator') === null, true);
  assert.equal(document.querySelector('.radio-main .build-task') === null, true);
  assert.match(document.querySelector('.radio-task-button').textContent, /正在删除迁移原目录/);
  await click(document.querySelector('.radio-task-button'));
  const active = document.querySelector('.task-center-panel [aria-label="进行中的任务"]');
  await click(active.querySelector('summary'));
  assert.match(active.textContent, /删除迁移原目录 · 正在执行/);
  assert.equal(dialog() === null, true);
});

test('Add actions stay disabled before bootstrap and while a Node entry is pending', async () => {
  const bootstrapRead = deferred();
  await setup({ overrides: { bootstrap_info: () => bootstrapRead.promise } });
  const disabled = () => [...addGroup().querySelectorAll('button')].map(element => element.disabled);
  assert.deepEqual(disabled(), [true, true, true]);
  await click(byText('从 Git 添加', addGroup()));
  assert.equal(dialog() === null, true);
  await act(async () => bootstrapRead.resolve(bootstrap));
  assert.deepEqual(disabled(), [false]);
  await unmount();
  const nodeRead = deferred();
  await setup({ nodes: [nodeFixture], overrides: { node_app_configuration: () => nodeRead.promise } });
  await click(document.querySelector('[aria-label="进入项目「Disposable shell Node」"]'));
  assert.deepEqual(disabled(), [true]);
  await click(byText('添加应用', addGroup()));
  assert.equal(dialog() === null, true);
  await act(async () => nodeRead.resolve(nodeFixture));
  assert.ok(dialog());
});

test('Managed build tools show their verified paths separately from the default location', async () => {
  const managed = { ...environment, selection: 'managed', managedNodePath: '/disposable/platform/node/bin/node', node: { path: '/disposable/platform/node/bin/node', version: 'v24.0.0' }, npm: { path: '/disposable/platform/node/lib/node_modules/npm/bin/npm-cli.js', version: '11.0.0' } };
  await setup({ overrides: { environment_doctor: () => managed } });
  await click(document.querySelector('[aria-label="设置"]'));
  await click(document.querySelector('#settings-runtime-tab'));
  const panel = document.querySelector('.build-environment');
  assert.match(panel.textContent, /当前来源：平台受管 Node\/npm/);
  assert.match(panel.textContent, /应用运行 Node 不受这里的构建选择影响/);
  assert.match(panel.textContent, /默认受管位置只说明工具所在位置；是否可用以本次检测结果为准/);
  assert.match(panel.textContent, /可以构建/);
  assert.ok([...panel.querySelectorAll('code')].some(value => value.textContent === managed.npm.path));
  assert.equal(document.getElementById('node-path').value, '');
  assert.equal(panel.textContent.includes(environment.downloadUrl), false);
  assert.equal(byText('自动查找', panel), undefined);
});

test('An invalid managed location stays unavailable and recommends repairing the platform or selecting local tools', async () => {
  await setup({ overrides: { environment_doctor: () => ({ ...environment, ready: false, node: null, npm: null, selection: 'managed', managedNodePath: '/disposable/missing-managed-node', issues: [{ code: 'NODE_NOT_FOUND', message: '人工受管工具缺失' }] }) } });
  await click(document.querySelector('[aria-label="设置"]'));
  await click(document.querySelector('#settings-runtime-tab'));
  const panel = document.querySelector('.build-environment');
  assert.match(panel.textContent, /当前来源：平台受管 Node\/npm/);
  assert.match(panel.textContent, /构建环境需要配置/);
  assert.match(panel.textContent, /人工受管工具缺失/);
  assert.match(panel.textContent, /修复或重新安装平台，或明确选择兼容的本机 Node\/npm/);
  assert.equal(panel.textContent.includes(environment.downloadUrl), false);
  assert.doesNotMatch(panel.textContent, /可以构建/);
  assert.ok([...panel.querySelectorAll('code')].some(value => value.textContent === '/disposable/missing-managed-node'));
});

test('A failed explicit build tool choice persists until restoring the managed default sends null', async () => {
  const managedNodePath = '/disposable/platform/node/bin/node';
  const explicit = { ...environment, ready: false, node: null, npm: null, selection: 'explicit', configuredNodePath: '/disposable/old-explicit-node', managedNodePath, issues: [{ code: 'NODE_NOT_FOUND', message: '人工显式路径失效' }] };
  const calls = await setup({ overrides: { environment_doctor: () => explicit, environment_node_set: () => ({ ...environment, selection: 'managed', managedNodePath }) } });
  await click(document.querySelector('[aria-label="设置"]'));
  await click(document.querySelector('#settings-runtime-tab'));
  const panel = document.querySelector('.build-environment');
  assert.match(panel.textContent, /当前来源：明确选择的本机 Node\/npm/);
  assert.match(panel.textContent, /失效时不会自动改用其他工具/);
  assert.equal(document.getElementById('node-path').value, explicit.configuredNodePath);
  await click(byText('重新检测', panel));
  assert.equal(document.getElementById('node-path').value, explicit.configuredNodePath);
  assert.equal(calls.some(([command]) => command === 'environment_node_set'), false);
  assert.match(panel.textContent, /恢复默认工具会清除显式路径，使用平台配套 Node\/npm/);
  await click(byText('恢复默认工具', panel));
  assert.deepEqual(calls.filter(([command]) => command === 'environment_node_set'), [['environment_node_set', { nodePath: null }]]);
  assert.match(panel.textContent, /当前来源：平台受管 Node\/npm/);
  assert.equal(document.getElementById('node-path').value, '');
});

test('A development service without managed tools describes automatic detection and clears the explicit choice', async () => {
  const calls = await setup({ overrides: { environment_node_set: () => environment } });
  await click(document.querySelector('[aria-label="设置"]'));
  await click(document.querySelector('#settings-runtime-tab'));
  const panel = document.querySelector('.build-environment');
  assert.match(panel.textContent, /当前来源：本机自动探测/);
  assert.match(panel.textContent, /没有提供受管工具位置；恢复默认工具会清除显式路径并自动探测本机环境/);
  assert.doesNotMatch(panel.textContent, /默认受管 Node 位置/);
  await click(byText('恢复默认工具', panel));
  assert.deepEqual(calls.filter(([command]) => command === 'environment_node_set'), [['environment_node_set', { nodePath: null }]]);
});

test('Available Git and SSH tools keep authentication explicitly unverified and do not access a repository', async () => {
  const calls = await setup();
  await click(document.querySelector('[aria-label="设置"]'));
  await click(document.querySelector('#settings-runtime-tab'));
  const panel = document.querySelector('.git-environment');
  assert.equal(panel.querySelector('.project-state').textContent, 'Git 工具可用');
  assert.match(panel.textContent, /当前来源：平台受管 Git/);
  assert.match(panel.textContent, /已找到系统 SSH 工具/);
  assert.match(panel.textContent, /这不代表账号认证或仓库访问已通过/);
  assert.match(panel.querySelector('.git-authentication').textContent, /认证状态：未验证。只检测工具，不访问仓库/);
  assert.match(panel.textContent, /仓库获取与版本操作尚未接入/);
  assert.ok([...panel.querySelectorAll('code')].some(value => value.textContent === gitEnvironment.git.path));
  assert.ok([...panel.querySelectorAll('code')].some(value => value.textContent === gitEnvironment.ssh.executable.path));
  assert.deepEqual(calls.filter(([command]) => /git|ssh/.test(command)), [['environment_git_doctor', undefined]]);
});

test('Missing SSH does not hide a usable Git tool or suggest authentication succeeded', async () => {
  await setup({ overrides: { environment_git_doctor: () => ({ ...gitEnvironment, ssh: { ready: false, selection: 'system', executable: null, issues: [{ code: 'SSH_NOT_FOUND', message: '人工系统 SSH 缺失' }] } }) } });
  await click(document.querySelector('[aria-label="设置"]'));
  await click(document.querySelector('#settings-runtime-tab'));
  const panel = document.querySelector('.git-environment');
  assert.equal(panel.querySelector('.project-state').textContent, 'Git 工具可用');
  assert.match(panel.textContent, /系统 SSH 工具暂不可用/);
  assert.match(panel.textContent, /人工系统 SSH 缺失 \(SSH_NOT_FOUND\)/);
  assert.match(panel.textContent, /认证状态：未验证/);
  assert.doesNotMatch(panel.textContent, /Git 工具不可用|平台配套 Git 暂不可用/);
  assert.equal(panel.querySelector('.git-authentication code') === null, true);
  assert.equal(byText('恢复默认工具', panel).disabled, false);
});

test('A failed explicit Git choice remains selected until an explicit clear sends null', async () => {
  const explicit = { ...gitEnvironment, ready: false, git: null, selection: 'explicit', configuredGitPath: '/disposable/missing-explicit-git', issues: [{ code: 'GIT_NOT_FOUND', message: '人工显式 Git 缺失' }] };
  const calls = await setup({ overrides: { environment_git_doctor: () => explicit, environment_git_set: () => gitEnvironment } });
  await click(document.querySelector('[aria-label="设置"]'));
  await click(document.querySelector('#settings-runtime-tab'));
  const panel = document.querySelector('.git-environment');
  assert.equal(panel.querySelector('.project-state').textContent, 'Git 工具不可用');
  assert.match(panel.textContent, /当前来源：明确选择的本机 Git/);
  assert.match(panel.textContent, /已保留你明确选择的路径，失效时不会自动更换工具/);
  assert.equal(document.getElementById('git-path').value, explicit.configuredGitPath);
  await click(byText('重新检测', panel));
  assert.equal(document.getElementById('git-path').value, explicit.configuredGitPath);
  assert.equal(calls.some(([command]) => command === 'environment_git_set'), false);
  await click(byText('恢复默认工具', panel));
  assert.deepEqual(calls.filter(([command]) => command === 'environment_git_set'), [['environment_git_set', { gitPath: null }]]);
  assert.equal(document.getElementById('git-path').value, '');
  assert.match(panel.textContent, /当前来源：平台受管 Git/);
});

test('Unreadable Git settings keep the source unresolved and allow an explicit repair or clear', async () => {
  const calls = await setup({ overrides: {
    environment_git_doctor: () => ({ ...gitEnvironment, ready: false, git: null, selection: 'unresolved', issues: [{ code: 'GIT_TOOLCHAIN_INVALID', message: '人工 Git 选择设置无法解析' }] }),
    environment_git_set: ({ gitPath }) => gitPath === null ? gitEnvironment : ({ ...gitEnvironment, selection: 'explicit', configuredGitPath: gitPath, git: { ...gitEnvironment.git, path: gitPath } }),
  } });
  await click(document.querySelector('[aria-label="设置"]'));
  await click(document.querySelector('#settings-runtime-tab'));
  const panel = document.querySelector('.git-environment');
  const input = document.getElementById('git-path');
  assert.equal(panel.querySelector('.project-state').textContent, 'Git 工具尚未核验');
  assert.match(panel.textContent, /当前来源：尚未确定（选择设置无法读取）/);
  assert.match(panel.textContent, /人工 Git 选择设置无法解析/);
  assert.match(panel.textContent, /尚未选用默认工具/);
  assert.doesNotMatch(panel.textContent, /当前来源：平台受管 Git|当前来源：明确选择的本机 Git|平台配套 Git 暂不可用/);
  assert.equal(input.disabled, false);
  await change(input, '/disposable/repaired-git');
  await click(byText('保存路径', panel));
  assert.equal(input.value, '/disposable/repaired-git');
  assert.match(panel.textContent, /当前来源：明确选择的本机 Git/);
  await click(byText('恢复默认工具', panel));
  assert.deepEqual(calls.filter(([command]) => command === 'environment_git_set'), [
    ['environment_git_set', { gitPath: '/disposable/repaired-git' }],
    ['environment_git_set', { gitPath: null }],
  ]);
  assert.equal(input.value, '');
  assert.match(panel.textContent, /当前来源：平台受管 Git/);
});

test('Concurrent Git saves and Node probes keep settings blocked until both finish in either order', async () => {
  for (const first of ['git', 'node']) {
    const nodeRead = deferred();
    const gitSave = deferred();
    const calls = await setup({ overrides: { environment_doctor: () => nodeRead.promise, environment_git_set: () => gitSave.promise } });
    await click(document.querySelector('[aria-label="设置"]'));
    await click(document.querySelector('#settings-runtime-tab'));
    const panel = document.querySelector('.git-environment');
    await change(document.getElementById('git-path'), '/disposable/selected-git');
    await click(byText('保存路径', panel));
    assert.deepEqual(calls.filter(([command]) => command === 'environment_git_set'), [['environment_git_set', { gitPath: '/disposable/selected-git' }]]);
    const assertBlocked = async () => {
      assert.equal(document.querySelector('[aria-label="关闭设置"]').disabled, true);
      assert.equal(byText('完成', dialog()).disabled, true);
      await click(byText('完成', dialog()));
      await click(document.querySelector('[aria-label="关闭设置"]'));
      await key(dialog(), 'Escape');
      await click(document.querySelector('.modal-layer'));
      assert.equal(document.querySelectorAll('[role="dialog"]').length, 1);
      assert.equal(dialog().querySelector('.modal-header h2').textContent, '设置');
    };
    await assertBlocked();
    await act(async () => first === 'git' ? gitSave.resolve(gitEnvironment) : nodeRead.resolve(environment));
    await assertBlocked();
    assert.equal(document.getElementById('git-path').disabled, first !== 'git');
    assert.equal(document.getElementById('node-path').disabled, first !== 'node');
    await act(async () => first === 'git' ? nodeRead.resolve(environment) : gitSave.resolve(gitEnvironment));
    assert.equal(byText('完成', dialog()).disabled, false);
    assert.equal(document.querySelector('[aria-label="关闭设置"]').disabled, false);
    await click(byText('完成', dialog()));
    assert.equal(dialog() === null, true);
    await unmount();
  }
});

test('A failed Git save keeps its draft, rejects repeated pending submissions and does not retry itself', async () => {
  const save = deferred();
  const calls = await setup({ overrides: { environment_git_set: () => save.promise } });
  await click(document.querySelector('[aria-label="设置"]'));
  await click(document.querySelector('#settings-runtime-tab'));
  const panel = document.querySelector('.git-environment');
  const input = document.getElementById('git-path');
  await change(input, '/disposable/git-draft');
  await click(byText('保存路径', panel));
  await act(async () => panel.querySelector('form').dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true })));
  await click(byText('恢复默认工具', panel));
  assert.equal(input.disabled, true);
  assert.deepEqual(calls.filter(([command]) => command === 'environment_git_set'), [['environment_git_set', { gitPath: '/disposable/git-draft' }]]);
  await act(async () => save.reject(new Error('Artificial Git configuration failure')));
  assert.equal(input.value, '/disposable/git-draft');
  assert.match(panel.querySelector('[role="alert"]').textContent, /Artificial Git configuration failure/);
  assert.equal(input.disabled, false);
  assert.equal(byText('完成', dialog()).disabled, false);
  await click(document.querySelector('#settings-general-tab'));
  await click(document.querySelector('#settings-runtime-tab'));
  assert.equal(input.value, '/disposable/git-draft');
  assert.equal(calls.filter(([command]) => command === 'environment_git_set').length, 1);
  assert.equal(calls.filter(([command]) => command === 'environment_git_doctor').length, 1);
});

test('Pending environment configuration blocks every settings dismissal, rejects duplicate saves and keeps a failed draft', async () => {
  const configuration = deferred();
  const calls = await setup({ overrides: { environment_node_set: () => configuration.promise } });
  const settings = document.querySelector('[aria-label="设置"]');
  await act(async () => settings.focus());
  await click(settings);
  await click(document.querySelector('#settings-runtime-tab'));
  const input = document.getElementById('node-path');
  await change(input, '/disposable/custom-node');
  await click(byText('保存路径', dialog()));
  assert.equal(input.disabled, true);
  assert.equal(document.querySelector('[aria-label="关闭设置"]').disabled, true);
  assert.equal(byText('完成', dialog()).disabled, true);
  await click(byText('完成', dialog()));
  await click(document.querySelector('[aria-label="关闭设置"]'));
  await key(dialog(), 'Escape');
  await click(document.querySelector('.modal-layer'));
  assert.equal(document.querySelectorAll('[role="dialog"]').length, 1);
  assert.equal(dialog().querySelector('.modal-header h2').textContent, '设置');
  await click(document.querySelector('.radio-settings-guide summary'));
  assert.equal(byText('创建第一个项目', dialog()).disabled, true);
  // This guide action has no disabled attribute; the parent callback must
  // independently preserve the settings window while configuration is pending.
  await click(byText('存储与清理', document.querySelector('.radio-settings-guide')));
  assert.equal(dialog().querySelector('.modal-header h2').textContent, '设置');
  assert.equal(document.querySelector('[aria-label="工作台导航"]') === null, true);
  await act(async () => document.querySelector('.node-path-form').dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true })));
  assert.deepEqual(calls.filter(([command]) => command === 'environment_node_set'), [['environment_node_set', { nodePath: '/disposable/custom-node' }]]);
  await act(async () => configuration.reject(new Error('Artificial configuration failure')));
  assert.ok(dialog());
  assert.equal(input.value, '/disposable/custom-node');
  assert.match(dialog().querySelector('.build-environment [role="alert"]').textContent, /Artificial configuration failure/);
  assert.equal(byText('完成', dialog()).disabled, false);
  assert.equal(document.querySelector('[aria-label="关闭设置"]').disabled, false);
  await click(byText('完成', dialog()));
  assert.equal(dialog() === null, true);
  assert.equal(document.activeElement === settings, true);
});

test('Disposable validation projects remain in the final collapsible test-project group', async () => {
  const make = (id, name) => ({ id, name, description: '', projectRoot: `/disposable/radio-shell/${id}`, documentEpoch: 'fixture', state: 'ready', issues: [], manifest: null });
  await setup({ projects: [make('prj_disposable_kept', 'Everyday notes'), make('prj_disposable_a', '可丢弃 · 样本 A'), make('prj_disposable_b', 'M5 可丢弃 · 样本 B')] });
  const names = () => [...document.querySelectorAll('.project-grid .launcher-name')].map(element => element.textContent);
  const toggle = () => document.querySelector('.project-group-toggle');
  assert.deepEqual(names(), ['Everyday notes']);
  assert.equal(toggle().getAttribute('aria-expanded'), 'false');
  assert.match(toggle().textContent, /测试项目2/);
  assert.match(document.querySelector('.application-count').textContent, /3 个 · 0 个运行中/);
  assert.match(document.querySelector('.radio-footer').textContent, /项目文件夹.*\/disposable\/radio-shell\/workspace/);
  assert.doesNotMatch(document.querySelector('.radio-footer').textContent, /数据保存在本机|共 3/);
  await click(toggle());
  assert.deepEqual(names(), ['Everyday notes', '可丢弃 · 样本 A', 'M5 可丢弃 · 样本 B']);
  await click(toggle());
  assert.deepEqual(names(), ['Everyday notes']);
  assert.equal(document.querySelector('input[type="search"]') === null, true);
});

test('Full cleanup locks the real shell before native preference clearing and later status renders do not restore storage', async () => {
  const plan = { planId: 'disposable-native-clear', instanceId: bootstrap.instanceId, expiresAt: Date.now() + 60_000, confirmationText: '删除全部本机数据', dataRoot: bootstrap.dataRoot, workspaceRoot: bootstrap.workspaceRoot, deletePaths: [{ path: bootstrap.workspaceRoot, label: 'Disposable project files' }], preservedPaths: [], warnings: [] };
  let phase = 'stopping';
  const report = () => ({ operationId: plan.planId, status: phase, message: 'Disposable cleanup fixture', remainingPaths: [] });
  await setup({ overrides: {
    local_data_cleanup_preview: () => plan,
    local_data_cleanup_start: () => { window.localStorage.clear(); return report(); },
    local_data_cleanup_status: () => report(),
  } });
  assert.ok(window.localStorage.length > 0, 'normal shell preferences were initialized');
  await click(document.querySelector('[aria-label="设置"]'));
  await click(byText('卸载前清除数据', dialog()));
  await click(byText('清理所有本机项目内容'));
  await click(document.querySelector('.local-data-cleanup-modal input[type="checkbox"]'));
  await click(byText('已了解，继续第二次确认'));
  await change(document.querySelector('.local-data-cleanup-modal input:not([type="checkbox"])'), '删除全部本机数据');
  await click(byText('永久清理所有本机数据'));
  assert.equal(window.localStorage.length, 0);
  assert.equal(document.querySelector('[aria-label="切换暗色主题"]') === null, true);
  assert.equal(document.querySelector('.project-group-toggle') === null, true);
  phase = 'deleting';
  await click(byText('查看清理状态'));
  assert.equal(window.localStorage.length, 0);
  phase = 'completed';
  await click(byText('查看清理状态'));
  assert.match(document.querySelector('.local-data-cleanup-result').textContent, /项目网页存储已收到系统删除确认/);
  assert.equal(window.localStorage.length, 0);
  assert.equal(document.querySelector('[aria-label="设置"]').disabled, true);
});

test('The settings platform introduction copies the current full prompt without opening another dialog', async () => {
  const copies = [];
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async text => { copies.push(text); } } });
  await setup();
  await click(document.querySelector('[aria-label="设置"]'));
  await click(document.getElementById('settings-ai-tab'));
  await click(byText('复制平台说明给 AI', document.getElementById('settings-ai-panel')));
  assert.equal(copies.length, 1);
  assert.match(copies[0], /当前版本：0\.0\.0-fixture/);
  assert.equal(document.querySelectorAll('[role="dialog"]').length, 1);
  assert.match(document.querySelector('.platform-ai-intro-notice').textContent, /已复制/);
});
