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
const output = await mkdtemp(path.join(repo, '.test-artifacts/git-remote-check-dom-'));
const mockPath = path.join(output, 'tauri-mock.mjs');
// These unrelated interaction fixtures have no interrupted Git additions.
await writeFile(mockPath, 'export const invoke = (command, ...args) => command === "source_import_recover" ? Promise.resolve([]) : globalThis.__GIT_REMOTE_INVOKE__(command, ...args); export const isTauri = () => true;\n');
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
  source = source.replaceAll(/(['"])@tauri-apps\/api\/core\1/g, JSON.stringify(pathToFileURL(mockPath).href));
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
globalThis.__GIT_REMOTE_INVOKE__ = async command => { throw new Error(`Unexpected fixture command: ${command}`); };
const { createElement: h, act } = await import('react');
const { createRoot } = await import('react-dom/client');
const { default: App } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/App.tsx'))).href);
const storagePrototype = dom.window.Storage.prototype;
const originalGetItem = Object.getOwnPropertyDescriptor(storagePrototype, 'getItem');
const originalSetItem = Object.getOwnPropertyDescriptor(storagePrototype, 'setItem');
let root;
async function unmount() { if (root) await act(async () => root.unmount()); root = null; }
afterEach(async () => {
  await unmount();
  Object.defineProperty(storagePrototype, 'getItem', originalGetItem);
  Object.defineProperty(storagePrototype, 'setItem', originalSetItem);
  dom.window.localStorage.clear();
  delete document.documentElement.dataset.theme;
  globalThis.__GIT_REMOTE_INVOKE__ = async command => { throw new Error(`Unexpected fixture command: ${command}`); };
});
after(async () => { dom.window.close(); await rm(output, { recursive: true, force: true }); });

const bootstrap = {
  displayName: 'Disposable Panda Drawer fixture', identifier: 'dev.fixture.radio-shell', environment: 'development', apiVersion: 1,
  instanceId: 'instance_radio_shell_fixture', baseUrl: 'http://127.0.0.1:1', workspaceRoot: '/disposable/radio-shell/workspace', dataRoot: '/disposable/radio-shell/data', sqliteVersion: 'fixture', version: '0.0.0-fixture',
  resources: { sdk: true, template: true, readme: true, cli: true }, cliPath: '/disposable/radio-shell/cli',
};
const environment = { ready: true, node: { path: '/disposable/node', version: 'v24.0.0' }, npm: { path: '/disposable/npm', version: '11.0.0' }, selection: 'detected', configuredNodePath: null, supportedNodeRange: 'fixture', recommendedNode: 'fixture', downloadUrl: 'https://example.invalid/node', issues: [] };
const gitEnvironment = { ready: true, git: { path: '/disposable/platform/git/bin/git', version: '2.56.0' }, selection: 'managed', configuredGitPath: null, managedGitPath: '/disposable/platform/git/bin/git', ssh: { ready: true, selection: 'system', executable: { path: '/usr/bin/ssh', version: 'OpenSSH fixture' }, issues: [] }, authentication: { state: 'not-checked', reason: '只检测工具，不访问仓库。' }, issues: [] };
const nodeFixture = { id: 'node_radio_shell_fixture', name: 'Disposable shell Node', sourcePath: '/disposable/radio-shell/source', runtime: 'node-service', state: 'stopped', restartRequired: false, runningRevision: null, lastError: null, configuration: { revision: 1, values: {}, secretStatus: {}, missingRequired: ['KEY'], canStart: false }, fields: [] };
function deferred() { let resolve; let reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { resolve, reject, promise }; }
async function setup({ projects = [], nodes = [], overrides = {} } = {}) {
  const calls = [];
  globalThis.__GIT_REMOTE_INVOKE__ = async (command, args) => {
    calls.push([command, args]);
    if (overrides[command]) return overrides[command](args);
    if (command === 'bootstrap_info') return bootstrap;
    if (command === 'project_list') return projects;
    if (command === 'node_app_list') return nodes;
    if (command === 'workspace_info') return { workspaceRoot: bootstrap.workspaceRoot, migrationState: 'ready', activeTask: null, retainedRoots: [] };
    if (command === 'maintenance_status') return { mode: 'ready', recoveryRequired: false, activeTask: null };
    if (command === 'environment_doctor') return environment;
    if (command === 'environment_git_doctor') return gitEnvironment;
    if (command === 'workspace_storage') return { platformProgram: { totalBytes: 0, complete: true }, workspace: { totalBytes: 0, complete: true, categories: {} }, otherLocalLocations: [] };
    throw new Error(`Unexpected fixture command: ${command}`);
  };
  document.body.innerHTML = '<div id="root"></div>';
  root = createRoot(document.getElementById('root'));
  await act(async () => root.render(h(App)));
  return calls;
}
const byText = (text, scope = document, selector = 'button') => [...scope.querySelectorAll(selector)].find(element => element.textContent === text);
const dialog = () => document.querySelector('[role="dialog"]');
async function click(element) { assert.ok(element); await act(async () => element.click()); }
async function key(element, value, extra = {}) { assert.ok(element); await act(async () => element.dispatchEvent(new window.KeyboardEvent('keydown', { key: value, bubbles: true, cancelable: true, ...extra }))); }
async function change(element, value) {
  assert.ok(element);
  await act(async () => {
    Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set.call(element, value);
    element.dispatchEvent(new window.Event('input', { bubbles: true }));
  });
}

const githubUrl = 'https://github.com/disposable-fixture/public-project.git';
const giteeUrl = 'https://gitee.com/disposable-fixture/public-project.git';
const remoteResult = {
  url: githubUrl, provider: 'github', transport: 'public-https', state: 'readable',
  head: { oid: '1234567890abcdef1234567890abcdef12345678', symbolicRef: 'refs/heads/main' },
  git: gitEnvironment.git, checkedAt: '2026-10-21T08:30:00Z',
};
const gitPanel = () => document.querySelector('.git-environment');
const remotePanel = () => document.querySelector('.git-remote-check');
const addressInput = () => remotePanel().querySelector('input');
const checkButton = () => remotePanel().querySelector('button[type="submit"]');
const resultPanel = () => remotePanel().querySelector('.operation-result');
const remoteCalls = calls => calls.filter(([command]) => command === 'environment_git_remote_check');
async function openSettings(options) {
  const calls = await setup(options);
  await click(document.querySelector('[aria-label="设置"]'));
  await click(document.querySelector('#settings-tools-tab'));
  return calls;
}
async function submitRemote() {
  await act(async () => remotePanel().querySelector('form').dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true })));
}

test('Opening Git settings and editing a valid public address stay offline until an explicit check', async () => {
  const calls = await openSettings({ overrides: { environment_git_remote_check: () => remoteResult } });
  assert.equal(remoteCalls(calls).length, 0);
  assert.match(remotePanel().textContent, /只读取远端 HEAD/);
  assert.match(remotePanel().textContent, /不下载仓库、不推送/);
  assert.match(remotePanel().textContent, /最长等待 20 秒/);
  await change(addressInput(), githubUrl);
  assert.equal(remoteCalls(calls).length, 0);
  assert.equal(checkButton().disabled, false);
  await click(checkButton());
  assert.deepEqual(remoteCalls(calls), [['environment_git_remote_check', { input: { url: githubUrl } }]]);
  assert.match(resultPanel().textContent, /此公开仓库可读取/);
  assert.match(resultPanel().textContent, /不代表账号认证或写入权限已验证/);
  assert.ok(resultPanel().textContent.includes(githubUrl));
  assert.ok(resultPanel().textContent.includes(remoteResult.head.oid));
  assert.ok(resultPanel().textContent.includes('refs/heads/main'));
  assert.equal(resultPanel().querySelector('time').dateTime, remoteResult.checkedAt);
  assert.match(gitPanel().querySelector('.git-authentication').textContent, /认证状态：未验证/);
});

test('Only standard GitHub and Gitee public HTTPS repository addresses can submit', async () => {
  const calls = await openSettings();
  for (const url of [
    'http://github.com/owner/repo', 'git@github.com:owner/repo.git', 'https://example.invalid/owner/repo',
    'https://github.com.evil.invalid/owner/repo', 'https://user:secret@github.com/owner/repo',
    'https://github.com:443/owner/repo', 'https://github.com/owner/repo?token=secret',
    'https://github.com/owner/repo#main', 'https://github.com/owner/repo/tree/main',
    'https://github.com/owner/%72epo', 'https://github.com/owner/../repo',
    'https://github.com/owner/repo name', `https://github.com/owner/${'r'.repeat(256)}`,
  ]) {
    await change(addressInput(), url);
    assert.equal(checkButton().disabled, true, url);
    await submitRemote();
  }
  assert.equal(remoteCalls(calls).length, 0);
  for (const url of [githubUrl, giteeUrl, 'https://github.com/Owner/repo-name_1/', 'https://gitee.com/owner/repo']) {
    await change(addressInput(), url);
    assert.equal(checkButton().disabled, false, url);
  }
});

test('A successful read without HEAD is described accurately and does not depend on SSH', async () => {
  const calls = await openSettings({ overrides: {
    environment_git_doctor: () => ({ ...gitEnvironment, ssh: { ready: false, selection: 'system', executable: null, issues: [{ code: 'SSH_NOT_FOUND', message: '人工 SSH 缺失' }] } }),
    environment_git_remote_check: () => ({ ...remoteResult, url: giteeUrl, provider: 'gitee', state: 'readable-no-head', head: null }),
  } });
  await change(addressInput(), giteeUrl);
  await click(checkButton());
  assert.equal(remoteCalls(calls).length, 1);
  assert.match(resultPanel().textContent, /此公开仓库可读取，但本次未返回 HEAD/);
  assert.doesNotMatch(resultPanel().textContent, /空仓库|HEAD 提交|HEAD 引用/);
  assert.match(gitPanel().textContent, /系统 SSH 工具暂不可用/);
});

test('Editing the address invalidates completed results and discards a pending old-address response', async () => {
  const later = deferred();
  let attempts = 0;
  const calls = await openSettings({ overrides: { environment_git_remote_check: () => ++attempts === 1 ? remoteResult : later.promise } });
  await change(addressInput(), githubUrl);
  await click(checkButton());
  assert.ok(resultPanel());
  await change(addressInput(), giteeUrl);
  assert.equal(resultPanel(), null);
  await change(addressInput(), githubUrl);
  assert.equal(resultPanel(), null);
  await click(checkButton());
  await change(addressInput(), giteeUrl);
  assert.equal(checkButton().disabled, true);
  assert.equal(document.querySelector('[aria-label="关闭设置"]').disabled, true);
  assert.ok(remotePanel().querySelector('[role="status"]').textContent.includes(githubUrl));
  await act(async () => later.resolve(remoteResult));
  assert.equal(addressInput().value, giteeUrl);
  assert.equal(resultPanel(), null);
  assert.equal(checkButton().disabled, false);
  assert.equal(remoteCalls(calls).length, 2);
});

test('Pending checks prevent duplicates, Git changes and all settings close routes until the result', async () => {
  const pending = deferred();
  const calls = await openSettings({ overrides: { environment_git_remote_check: () => pending.promise } });
  await change(document.getElementById('git-path'), '/disposable/another-git');
  await change(addressInput(), githubUrl);
  await click(checkButton());
  await submitRemote();
  assert.equal(remoteCalls(calls).length, 1);
  for (const label of ['重新检测', '保存路径', '恢复默认工具']) {
    const button = byText(label, gitPanel());
    assert.equal(button.disabled, true, label);
    await click(button);
  }
  assert.equal(document.getElementById('git-path').disabled, true);
  assert.equal(calls.filter(([command]) => command === 'environment_git_doctor').length, 1);
  assert.equal(calls.some(([command]) => command === 'environment_git_set'), false);
  assert.equal(byText('取消检查', remotePanel()), undefined);
  await click(document.querySelector('[aria-label="关闭设置"]'));
  await click(byText('完成', dialog()));
  await key(dialog(), 'Escape');
  await click(document.querySelector('.modal-layer'));
  assert.ok(dialog());
  await act(async () => pending.resolve(remoteResult));
  assert.equal(document.querySelector('[aria-label="关闭设置"]').disabled, false);
  await click(byText('完成', dialog()));
  assert.equal(dialog(), null);
});

test('Node and public Git requests keep settings blocked until both finish in either order', async () => {
  for (const first of ['node', 'git']) {
    const node = deferred();
    const git = deferred();
    let probes = 0;
    await openSettings({ overrides: {
      environment_doctor: () => ++probes === 1 ? environment : node.promise,
      environment_git_remote_check: () => git.promise,
    } });
    await click(byText('重新检测', document.querySelector('.build-environment')));
    await change(addressInput(), githubUrl);
    await click(checkButton());
    await act(async () => first === 'node' ? node.resolve(environment) : git.resolve(remoteResult));
    assert.equal(document.querySelector('[aria-label="关闭设置"]').disabled, true, `${first} completed first`);
    await key(dialog(), 'Escape');
    assert.ok(dialog());
    await act(async () => first === 'node' ? git.resolve(remoteResult) : node.resolve(environment));
    assert.equal(document.querySelector('[aria-label="关闭设置"]').disabled, false);
    await click(byText('完成', dialog()));
    await unmount();
  }
});

test('Failure retains the address and allows explicit retry without resubmitting on its own', async () => {
  let attempts = 0;
  const calls = await openSettings({ overrides: { environment_git_remote_check: () => {
    attempts += 1;
    if (attempts === 1) throw { code: 'GIT_REMOTE_CHECK_FAILED', message: '无法读取此公开仓库，请核对地址或网络后重试' };
    return remoteResult;
  } } });
  await change(addressInput(), githubUrl);
  await click(checkButton());
  assert.equal(addressInput().value, githubUrl);
  assert.match(remotePanel().querySelector('[role="alert"]').textContent, /无法读取此公开仓库/);
  assert.equal(checkButton().textContent, '重新检查');
  assert.equal(remoteCalls(calls).length, 1);
  assert.equal(document.querySelector('[aria-label="关闭设置"]').disabled, false);
  await click(checkButton());
  assert.equal(remoteCalls(calls).length, 2);
  assert.ok(resultPanel());
  assert.equal(remotePanel().querySelector('[role="alert"]'), null);
});

test('Refreshing, saving and clearing Git tools each discard a previous check without rechecking', async () => {
  const pendingTools = [];
  let probes = 0;
  const calls = await openSettings({ overrides: {
    environment_git_doctor: () => ++probes === 1 ? gitEnvironment : pendingTools.shift().promise,
    environment_git_set: () => pendingTools.shift().promise,
    environment_git_remote_check: () => remoteResult,
  } });
  await change(addressInput(), githubUrl);
  for (const label of ['重新检测', '保存路径', '恢复默认工具']) {
    await click(checkButton());
    assert.ok(resultPanel());
    if (label === '保存路径') await change(document.getElementById('git-path'), '/disposable/another-git');
    const tool = deferred();
    pendingTools.push(tool);
    await click(byText(label, gitPanel()));
    assert.equal(resultPanel(), null, label);
    assert.equal(checkButton().disabled, true);
    assert.equal(addressInput().value, githubUrl);
    await act(async () => tool.resolve(gitEnvironment));
    assert.equal(resultPanel(), null);
    assert.equal(checkButton().disabled, false);
  }
  assert.equal(remoteCalls(calls).length, 3);
  assert.deepEqual(calls.filter(([command]) => command === 'environment_git_set').map(([, args]) => args), [{ gitPath: '/disposable/another-git' }, { gitPath: null }]);
});

test('An unavailable explicit Git tool cannot start a remote check or silently restore default', async () => {
  const calls = await openSettings({ overrides: { environment_git_doctor: () => ({ ...gitEnvironment, ready: false, git: null, selection: 'explicit', configuredGitPath: '/disposable/missing-git', issues: [{ code: 'GIT_NOT_FOUND', message: '人工 Git 缺失' }] }) } });
  await change(addressInput(), githubUrl);
  assert.equal(checkButton().disabled, true);
  await submitRemote();
  assert.equal(remoteCalls(calls).length, 0);
  assert.equal(calls.some(([command]) => command === 'environment_git_set'), false);
  assert.equal(document.getElementById('git-path').value, '/disposable/missing-git');
});
