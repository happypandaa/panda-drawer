// Disposable DOM fixtures only; these tests do not prove native Git operations.
import assert from 'node:assert/strict';
import { after, afterEach, test } from 'node:test';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';
import { JSDOM } from 'jsdom';

const repo = fileURLToPath(new URL('../../../', import.meta.url));
await mkdir(path.join(repo, '.test-artifacts'), { recursive: true });
const output = await mkdtemp(path.join(repo, '.test-artifacts/disposable-project-git-dom-'));
const mockPath = path.join(output, 'tauri-mock.mjs');
await writeFile(mockPath, 'export const invoke = (...args) => globalThis.__PROJECT_GIT_INVOKE__(...args);\n');
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
      try { await readFile(candidate); dependency = candidate; break; } catch { /* Next extension. */ }
    }
    assert.ok(dependency);
    source = source.replaceAll(`${match[1]}${match[2]}${match[1]}`, JSON.stringify(pathToFileURL(await compile(dependency)).href));
  }
  source = source.replaceAll(/(['"])@tauri-apps\/api\/core\1/g, JSON.stringify(pathToFileURL(mockPath).href));
  await writeFile(target, source);
  return target;
}

const dom = new JSDOM('<!doctype html><html><body></body></html>', { url: 'http://localhost/', pretendToBeVisual: true });
for (const name of ['window', 'document', 'Node', 'HTMLElement', 'HTMLInputElement', 'HTMLTextAreaElement', 'Event', 'MouseEvent']) Object.defineProperty(globalThis, name, { value: name === 'window' ? dom.window : dom.window[name], configurable: true });
Object.defineProperty(globalThis, 'navigator', { value: dom.window.navigator, configurable: true });
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
const { createElement: h, act, StrictMode } = await import('react');
const { createRoot } = await import('react-dom/client');
const { ProjectGitPanel } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/ProjectGitPanel.tsx'))).href);
let root;
const timers = new Map();
let nextTimer = 0;
dom.window.setInterval = fn => { const id = ++nextTimer; timers.set(id, fn); return id; };
dom.window.clearInterval = id => { timers.delete(id); };
async function tick() { await act(async () => { for (const fn of [...timers.values()]) fn(); }); }
async function unmount() { if (root) await act(async () => root.unmount()); root = null; }
afterEach(async () => { await unmount(); timers.clear(); });
after(async () => { dom.window.close(); await rm(output, { recursive: true, force: true }); });

const oid = 'a'.repeat(40);
const nextOid = 'b'.repeat(40);
const digest = 'c'.repeat(64);
function status(overrides = {}) {
  return {
    repository: { state: 'project-root', root: '/disposable/project', markers: [{ path: '/disposable/project/.git', kind: 'directory', scope: 'project' }], head: { oid, branch: 'refs/heads/main' }, indexState: 'matched', git: { path: '/disposable/git', version: '2.56.0' } },
    scope: { policy: 'project-source-v1', includedRoots: ['project.json', 'app', 'docs'], includedPaths: ['project.json', 'app/index.ts'], excluded: [{ path: 'data', reason: '活跃数据库和附件未纳入版本' }, { path: 'app/.env', reason: '本机凭据' }], unclassifiedPaths: [], protectedTrackedPaths: [], complete: true },
    changes: [{ path: 'app/index.ts', kind: 'modify', staged: false }], snapshotDigest: digest, canInitialize: false, canSave: true, canListVersions: true, canCreateBranch: false, branchCreationIssue: null,
    database: { state: 'not-supported', reason: '本步未纳入数据快照。' }, issues: [], ...overrides,
  };
}
function task(overrides = {}) {
  return { taskId: 'task_git_disposable', projectId: 'prj_a', type: 'gitSourceRestoreApply', status: 'running', final: false, phase: 'snapshot', result: null, error: null, ...overrides };
}
function deferred() { let resolve; let reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { resolve, reject, promise }; }
async function setup({ overrides = {}, props = {}, strict = false } = {}) {
  const calls = []; const tasks = []; const drafts = [];
  globalThis.__PROJECT_GIT_INVOKE__ = async (command, args) => {
    calls.push([command, args]);
    if (overrides[command]) return overrides[command](args);
    if (command === 'project_git_status') return status();
    if (command === 'project_git_source_restore_status') return recoveryStatus();
    if (command === 'project_git_data_save_status') return { state: 'clear', operations: [], issues: [] };
    if (command === 'project_git_data_restore_status') return { state: 'clear', businessBlocked: false, operations: [], issues: [] };
    throw new Error(`Unexpected disposable command: ${command}`);
  };
  let currentProps = { projectId: 'prj_a', instanceId: 'instance_a', onTask: value => tasks.push(value), onDraftStateChange: value => drafts.push(value), ...props };
  document.body.innerHTML = '<div id="root"></div>';
  root = createRoot(document.getElementById('root'));
  async function render(next = {}) { currentProps = { ...currentProps, ...next }; await act(async () => root.render(strict ? h(StrictMode, null, h(ProjectGitPanel, currentProps)) : h(ProjectGitPanel, currentProps))); }
  await render();
  return { calls, tasks, drafts, render };
}
const byText = text => [...document.querySelectorAll('button')].find(element => element.textContent === text);
const field = text => document.getElementById([...document.querySelectorAll('label')].find(element => element.textContent === text)?.htmlFor);
const save = () => byText('保存源码版本');
const refresh = () => byText('刷新状态');
async function click(element) { assert.ok(element); await act(async () => element.click()); }
async function change(element, value) {
  assert.ok(element);
  await act(async () => {
    Object.getOwnPropertyDescriptor(element instanceof dom.window.HTMLTextAreaElement ? dom.window.HTMLTextAreaElement.prototype : dom.window.HTMLInputElement.prototype, 'value').set.call(element, value);
    element.dispatchEvent(new window.Event('input', { bubbles: true }));
  });
}
const commandCalls = (calls, command) => calls.filter(([name]) => name === command);

test('Opening the panel only previews local status and shows scope, exclusions and the confirmed HEAD', async () => {
  const { calls, drafts } = await setup();
  assert.deepEqual(calls, [['project_git_data_restore_status', { id: 'prj_a' }], ['project_git_source_restore_status', { id: 'prj_a' }], ['project_git_data_save_status', { id: 'prj_a' }], ['project_git_status', { id: 'prj_a' }]]);
  assert.ok(document.body.textContent.includes(oid));
  assert.match(document.body.textContent, /分支：main/);
  assert.doesNotMatch(document.body.textContent, /refs\/heads\/main/);
  assert.match(document.body.textContent, /让外部 AI/);
  assert.match(document.body.textContent, /预览后取回已有源码、数据版本/);
  assert.match(document.body.textContent, /app\/\.env/);
  assert.equal(document.querySelector('[aria-label="创建定制分支"]'), null);
  assert.equal(byText('保存源码版本'), undefined);
  assert.deepEqual(drafts.at(-1), { dirty: false, pending: false });
});

test('Unclassified and protected tracked paths keep writes unavailable and remain visible', async () => {
  const value = status();
  value.scope = { ...value.scope, complete: false, unclassifiedPaths: ['other-source'], protectedTrackedPaths: ['data/main.sqlite'] };
  value.canSave = false;
  value.issues = [{ code: 'PROJECT_GIT_SCOPE_INCOMPLETE', message: '请先核对未分类文件' }];
  const { calls } = await setup({ overrides: { project_git_status: () => value } });
  assert.equal(document.querySelector('form[aria-label="创建定制分支"]'), null);
  assert.equal(save(), undefined);
  assert.equal(commandCalls(calls, 'project_git_save').length, 0);
  assert.match(document.body.textContent, /other-source/);
  assert.match(document.body.textContent, /data\/main.sqlite/);
  assert.match(document.body.textContent, /不会自行移除跟踪或改写历史/);
});

test('An unavailable status never becomes an empty repository or enables initialization', async () => {
  const { calls } = await setup({ overrides: {
    project_git_status: () => { throw { code: 'PROJECT_BUSY', message: '项目正在执行其他操作，请稍后核对' }; },
  } });
  assert.match(document.body.textContent, /PROJECT_BUSY/);
  assert.equal(byText('初始化本项目 Git'), undefined);
  assert.doesNotMatch(document.body.textContent, /尚未初始化本项目仓库/);
  assert.equal(commandCalls(calls, 'project_git_initialize').length, 0);
  assert.equal(commandCalls(calls, 'project_git_save').length, 0);
});

function versions(overrides = {}) {
  return { repositoryRoot: '/disposable/project', head: status().repository.head, versions: [{ oid, parents: [nextOid], subject: 'Disposable saved version', author: { name: 'Fixture Author', email: 'fixture@example.invalid' }, authoredAt: '2026-10-22T08:00:00+00:00' }], limit: 50, hasMore: false, ...overrides };
}
function branchStatus(overrides = {}) { return status({ changes: [], canCreateBranch: true, ...overrides }); }

test('Local versions read only after an explicit click and selection alone does not mutate source', async () => {
  const { calls } = await setup({ overrides: { project_git_versions: () => versions({ hasMore: true }) } });
  assert.equal(commandCalls(calls, 'project_git_versions').length, 0);
  await click(byText('查看本机版本'));
  assert.deepEqual(commandCalls(calls, 'project_git_versions'), [['project_git_versions', { id: 'prj_a', limit: 50 }]]);
  const history = document.querySelector('[aria-label="本机源码版本列表"]');
  assert.match(history.textContent, /Disposable saved version/);
  assert.match(history.textContent, /Fixture Author.*fixture@example.invalid/);
  assert.match(history.textContent, /最近 50 个.*更早版本/);
  assert.match(history.textContent, /选择版本只填写取回目标，仍须预览并明确确认/);
  assert.equal(commandCalls(calls, 'project_git_branch_create').length, 0);
  assert.equal(history.querySelectorAll('button').length, 2);
  assert.equal(commandCalls(calls, 'project_git_source_restore_preview').length, 0);
});

function recoveryStatus(overrides = {}) { return { state: 'clear', operations: [], issues: [], database: { state: 'not-supported' }, ...overrides }; }
function recoveryOperation(overrides = {}) { return { restoreTaskId: 'restore_original', targetCommit: nextOid, phase: 'applying', planDigest: 'd'.repeat(64), journalDigest: 'e'.repeat(64), canContinue: true, canUndo: true, changes: [{ path: 'app/index.ts', kind: 'modify', state: 'after' }], issues: [], ...overrides }; }
function restorePreview(overrides = {}) { return { repositoryRoot: '/disposable/project', head: { oid, branch: 'refs/heads/main' }, targetCommit: nextOid, snapshotDigest: digest, planDigest: 'd'.repeat(64), changes: [{ path: 'app/index.ts', kind: 'modify', before: { sha256: '1'.repeat(64), size: 10, executable: false }, after: { sha256: '2'.repeat(64), size: 20, executable: false } }], totalFiles: 1, totalBytes: 20, canApply: true, issues: [], git: { path: '/disposable/git', version: '2.56.0' }, database: { state: 'not-supported' }, ...overrides }; }
function restoreTask(overrides = {}) { return task({ type: 'gitSourceRestoreApply', phase: 'source-restore-apply', ...overrides }); }
const stoppedCheckbox = () => [...document.querySelectorAll('label')].find(label => label.textContent === '我已停止本项目的外部编辑和 Git 操作')?.querySelector('input');
const confirmCheckbox = () => [...document.querySelectorAll('label')].find(label => label.textContent === '我已核对文件范围，确认取回为待保存修改')?.querySelector('input');
const targetField = () => field('要取回的本机提交');
async function prepareRestore(target = nextOid) { await change(targetField(), target); if (!stoppedCheckbox().checked) await click(stoppedCheckbox()); await click(byText('预览取回范围')); }

test('Recovery status remains readable when ordinary Git inspection fails and never starts a recovery automatically', async () => {
  const { calls } = await setup({ overrides: {
    project_git_source_restore_status: () => recoveryStatus({ state: 'blocked', operations: [recoveryOperation({ canContinue: false, canUndo: false, changes: [{ path: 'app/index.ts', kind: 'modify', state: 'unknown' }], issues: [{ code: 'SOURCE_RESTORE_BLOCKED', message: '外部修改需人工核对' }] })] }),
    project_git_status: () => { throw new Error('普通 Git 检查暂时不可用'); },
  } });
  assert.match(document.body.textContent, /普通 Git 检查暂时不可用/);
  assert.match(document.body.textContent, /外部修改需人工核对/);
  assert.match(document.body.textContent, /内容尚不能确认/);
  assert.match(document.body.textContent, /当前程序和业务数据仍可使用/);
  assert.equal(byText('确认继续取回'), undefined); assert.equal(byText('确认撤回这次取回'), undefined);
  assert.deepEqual(calls.map(([command]) => command), ['project_git_data_restore_status', 'project_git_source_restore_status', 'project_git_data_save_status', 'project_git_status']);
});

test('Selecting a listed source version does not preview or apply until explicit review and confirmation', async () => {
  const accepted = deferred();
  const { calls, drafts } = await setup({ overrides: {
    project_git_status: () => branchStatus(),
    project_git_versions: () => versions({ versions: [{ ...versions().versions[0], oid: nextOid }] }),
    project_git_source_restore_preview: () => restorePreview({ totalFiles: 8, totalBytes: 320 }),
    project_git_source_restore_apply: () => accepted.promise,
  } });
  await click(byText('查看本机版本')); await click(byText('选择此版本取回'));
  assert.equal(targetField().value, nextOid);
  assert.equal(commandCalls(calls, 'project_git_source_restore_preview').length, 0);
  assert.equal(byText('预览取回范围').disabled, true);
  await click(stoppedCheckbox()); await click(byText('预览取回范围'));
  assert.deepEqual(commandCalls(calls, 'project_git_source_restore_preview'), [['project_git_source_restore_preview', { id: 'prj_a', input: { targetCommit: nextOid } }]]);
  assert.match(document.querySelector('[aria-label="源码取回预览"]').textContent, /目标源码范围 8 个文件.*本次变更 1 个文件/);
  assert.equal(byText('确认取回源码').disabled, true);
  await click(confirmCheckbox());
  const applyButton = byText('确认取回源码');
  await act(async () => { applyButton.click(); applyButton.click(); });
  assert.deepEqual(commandCalls(calls, 'project_git_source_restore_apply'), [['project_git_source_restore_apply', { id: 'prj_a', input: { targetCommit: nextOid, expectedHead: oid, expectedBranch: 'refs/heads/main', expectedSnapshot: digest, expectedPlanDigest: 'd'.repeat(64) } }]]);
  assert.equal(drafts.at(-1).pending, true);
  await act(async () => accepted.resolve(restoreTask()));
  assert.equal(drafts.at(-1).pending, true, 'An accepted restore remains a close guard until its final task');
  assert.match(document.body.textContent, /源码操作结束前请保留此面板/);
  assert.equal(commandCalls(calls, 'project_git_save').length, 0);
});

test('Source preview, recovery status and history reads are serialized; a late preview cannot survive an instance change', async () => {
  const check = deferred(); const preview = deferred(); let recoveryReads = 0;
  const { calls, drafts, render } = await setup({ overrides: {
    project_git_source_restore_status: () => ++recoveryReads === 1 ? check.promise : recoveryStatus(),
    project_git_status: () => branchStatus(), project_git_source_restore_preview: () => preview.promise,
  } });
  assert.equal(commandCalls(calls, 'project_git_status').length, 0);
  await change(targetField(), nextOid); await click(stoppedCheckbox()); await click(byText('预览取回范围'));
  assert.equal(commandCalls(calls, 'project_git_source_restore_preview').length, 0);
  await act(async () => check.resolve(recoveryStatus()));
  await click(byText('预览取回范围'));
  assert.equal(drafts.at(-1).pending, true);
  await click(refresh()); await click(byText('重新核对恢复状态')); await click(byText('查看本机版本'));
  assert.equal(commandCalls(calls, 'project_git_status').length, 1);
  assert.equal(commandCalls(calls, 'project_git_versions').length, 0);
  assert.equal(commandCalls(calls, 'project_git_source_restore_status').length, 1);
  await render({ instanceId: 'instance_new' });
  await act(async () => preview.resolve(restorePreview()));
  assert.equal(targetField().value, nextOid);
  assert.equal(document.querySelector('[aria-label="源码取回预览"]'), null);
  assert.equal(stoppedCheckbox().checked, false);
  assert.equal(drafts.at(-1).pending, false);
});

test('Editing the target or refreshing status expires the preview and its confirmation without discarding drafts', async () => {
  const { calls } = await setup({ overrides: { project_git_status: () => branchStatus(), project_git_source_restore_preview: ({ input }) => restorePreview({ targetCommit: input.targetCommit }) } });
  await prepareRestore(); await click(confirmCheckbox());
  await change(targetField(), oid);
  assert.equal(document.querySelector('[aria-label="源码取回预览"]'), null);
  await click(byText('预览取回范围')); assert.equal(confirmCheckbox().checked, false);
  await click(confirmCheckbox()); await click(refresh());
  assert.equal(document.querySelector('[aria-label="源码取回预览"]'), null);
  assert.equal(targetField().value, oid);
  await click(byText('预览取回范围'));
  assert.equal(confirmCheckbox().checked, false, 'A refreshed preview cannot reuse the previous apply confirmation');
  assert.equal(commandCalls(calls, 'project_git_source_restore_preview').length, 3);
  assert.equal(commandCalls(calls, 'project_git_source_restore_apply').length, 0);
});

test('Unsupported commit IDs and a preview with unknown paths cannot be applied', async () => {
  const { calls } = await setup({ overrides: { project_git_source_restore_preview: () => restorePreview({ canApply: false, issues: [{ code: 'SOURCE_RESTORE_SCOPE', message: '未知文件不允许替换' }] }) } });
  await click(stoppedCheckbox());
  for (const value of ['main', 'abc123', 'A'.repeat(40), 'a'.repeat(64)]) { await change(targetField(), value); assert.equal(byText('预览取回范围').disabled, true); await click(byText('预览取回范围')); }
  assert.match(document.body.textContent, /40 位小写 SHA-1/); assert.match(document.body.textContent, /64 位提交仍可在版本列表中查看/);
  assert.equal(commandCalls(calls, 'project_git_source_restore_preview').length, 0);
  await prepareRestore(); assert.match(document.body.textContent, /未知文件不允许替换/);
  assert.equal(byText('确认取回源码').disabled, true); assert.equal(confirmCheckbox().disabled, true);
  assert.equal(commandCalls(calls, 'project_git_source_restore_apply').length, 0);
});

test('A stale apply preserves the target, requires a fresh preview and never retries or rolls back automatically', async () => {
  let previews = 0;
  const { calls } = await setup({ overrides: {
    project_git_status: () => branchStatus(),
    project_git_source_restore_preview: () => restorePreview({ planDigest: ++previews === 1 ? 'd'.repeat(64) : 'f'.repeat(64) }),
    project_git_source_restore_apply: () => { throw { code: 'SOURCE_RESTORE_STALE', message: '预览已过期' }; },
  } });
  await prepareRestore(); await click(confirmCheckbox()); await click(byText('确认取回源码'));
  assert.equal(targetField().value, nextOid); assert.match(document.body.textContent, /预览已过期/);
  assert.equal(document.querySelector('[aria-label="源码取回预览"]'), null);
  assert.equal(byText('预览取回范围').disabled, true);
  await tick(); assert.equal(commandCalls(calls, 'project_git_source_restore_apply').length, 1);
  assert.equal(commandCalls(calls, 'project_git_source_restore_recover').length, 0);
  await click(byText('重新核对恢复状态')); await click(byText('预览取回范围')); await click(confirmCheckbox()); await click(byText('确认取回源码'));
  assert.equal(commandCalls(calls, 'project_git_source_restore_apply')[1][1].input.expectedPlanDigest, 'f'.repeat(64));
});

test('Pending recovery blocks development while local history remains available and only verified recovery actions are offered', async () => {
  const { calls } = await setup({ overrides: {
    project_git_status: () => branchStatus({ changes: status().changes }), project_git_versions: () => versions(),
    project_git_source_restore_status: () => recoveryStatus({ state: 'pending', operations: [recoveryOperation({ canUndo: false })] }),
  } });
  assert.equal(byText('创建并切换到新分支'), undefined);
  assert.equal(save(), undefined); assert.equal(byText('预览取回范围').disabled, true);
  await click(byText('查看本机版本'));
  assert.equal(commandCalls(calls, 'project_git_versions').length, 1);
  assert.equal(byText('选择此版本取回').disabled, true);
  assert.equal(byText('确认撤回这次取回'), undefined); assert.equal(byText('确认继续取回').disabled, true);
  await click(stoppedCheckbox()); assert.equal(byText('确认继续取回').disabled, false);
});

test('Recovery actions bind fresh journal digests; failures retain evidence and require an explicit recheck', async () => {
  let reads = 0;
  const { calls, drafts } = await setup({ overrides: {
    project_git_source_restore_status: () => recoveryStatus({ state: 'pending', operations: [recoveryOperation({ journalDigest: ++reads === 1 ? 'e'.repeat(64) : 'f'.repeat(64) })] }),
    project_git_status: () => { throw new Error('Ordinary status remains unavailable'); },
    project_git_source_restore_recover: () => { throw { code: 'SOURCE_RESTORE_CHANGED', message: '现场已变化' }; },
  } });
  await click(stoppedCheckbox()); await click(byText('确认继续取回'));
  assert.deepEqual(commandCalls(calls, 'project_git_source_restore_recover')[0], ['project_git_source_restore_recover', { id: 'prj_a', input: { restoreTaskId: 'restore_original', expectedPlanDigest: 'd'.repeat(64), expectedJournalDigest: 'e'.repeat(64), action: 'continue' } }]);
  assert.equal(byText('确认继续取回').disabled, true); assert.equal(byText('确认撤回这次取回').disabled, true);
  assert.match(document.body.textContent, /现场已变化/); assert.equal(drafts.at(-1).pending, false);
  await tick(); assert.equal(commandCalls(calls, 'project_git_source_restore_recover').length, 1);
  await click(byText('重新核对恢复状态')); await click(byText('确认撤回这次取回'));
  assert.equal(commandCalls(calls, 'project_git_source_restore_recover')[1][1].input.expectedJournalDigest, 'f'.repeat(64));
  assert.equal(commandCalls(calls, 'project_git_source_restore_recover')[1][1].input.action, 'undo');
});

test('An interrupted restore keeps closing blocked while active, then exposes recovery without claiming an automatic undo', async () => {
  let final = false;
  const { calls, drafts } = await setup({ props: { task: restoreTask() }, overrides: {
    task_get: () => final ? restoreTask({ final: true, status: 'interrupted', error: { code: 'SOURCE_RESTORE_INTERRUPTED', message: '源码取回已中断', details: { operationOutcome: 'unknown', restoreTaskId: 'restore_original', pending: true } } }) : restoreTask(),
    project_git_cancel: () => restoreTask(),
    project_git_source_restore_status: () => recoveryStatus({ state: 'pending', operations: [recoveryOperation()] }),
  } });
  assert.equal(drafts.at(-1).pending, true);
  assert.equal(commandCalls(calls, 'project_git_source_restore_status').length, 0);
  await click(byText('取消当前操作')); assert.match(document.body.textContent, /不会自动撤回已经替换的文件/);
  assert.equal(drafts.at(-1).pending, true);
  final = true; await tick();
  assert.equal(drafts.at(-1).pending, false);
  assert.match(document.body.textContent, /操作结果尚不能确认/); assert.match(document.body.textContent, /源码取回尚未处理完成/);
  assert.doesNotMatch(document.body.textContent, /已回退到恢复前|已撤回这次源码取回/);
  assert.equal(commandCalls(calls, 'project_git_source_restore_recover').length, 0);
});

for (const [type, state, expected] of [['gitSourceRestoreApply', 'completed', /源码已取回为待保存修改/], ['gitSourceRestoreContinue', 'not-applied', /本次没有应用源码修改/], ['gitSourceRestoreUndo', 'undone', /这次源码取回已撤回/]]) {
  test(`A final ${type} renders its actual ${state} outcome without claiming a saved version, database restore or build`, async () => {
    await setup({ props: { task: restoreTask({ type, final: true, status: 'succeeded', result: { operationOutcome: 'committed', state, targetCommit: nextOid, restoreTaskId: 'restore_original' } }) } });
    const result = document.querySelector('.build-task');
    assert.match(result.textContent, expected); assert.match(result.textContent, /取回来源提交/);
    assert.doesNotMatch(result.textContent, /源码版本已保存|备份任务已完成|项目仍保持维护|新网页已发布/);
  });
}

test('The independent Git component reports pending during restore preview, submission and accepted running work', async () => {
  const preview = deferred(); const accepted = deferred();
  const { drafts } = await setup({ overrides: {
    project_git_status: () => branchStatus(),
    project_git_source_restore_preview: () => preview.promise,
    project_git_source_restore_apply: () => accepted.promise,
    task_get: () => restoreTask(),
  } });
  await prepareRestore();
  assert.equal(drafts.at(-1).pending, true);
  assert.equal(byText('预览取回范围').disabled, true);
  await act(async () => preview.resolve(restorePreview()));
  assert.equal(drafts.at(-1).pending, false);
  await click(confirmCheckbox()); await click(byText('确认取回源码'));
  assert.equal(drafts.at(-1).pending, true);
  await act(async () => accepted.resolve(restoreTask()));
  assert.equal(drafts.at(-1).pending, true);
  assert.match(document.body.textContent, /源码操作结束前请保留此面板/);
});

test('Recovery neither accepts an unapplied target draft nor clears a target accepted by a separate apply', async () => {
  let needsRecovery = true; let recoveryCalls = 0;
  const selectedTarget = 'f'.repeat(40);
  const { drafts } = await setup({ overrides: {
    project_git_status: () => branchStatus(),
    project_git_source_restore_status: () => needsRecovery ? recoveryStatus({ state: 'pending', operations: [recoveryOperation()] }) : recoveryStatus(),
    project_git_source_restore_preview: ({ input }) => restorePreview({ targetCommit: input.targetCommit }),
    project_git_source_restore_apply: () => restoreTask({ taskId: 'apply_selected_target', final: true, status: 'succeeded', result: { operationOutcome: 'committed', state: 'completed', targetCommit: selectedTarget } }),
    project_git_source_restore_recover: ({ input }) => {
      if (++recoveryCalls === 1) {
        needsRecovery = false;
        return restoreTask({ taskId: 'continue_previous_restore', type: 'gitSourceRestoreContinue', final: true, status: 'succeeded', result: { operationOutcome: 'committed', state: 'completed', targetCommit: nextOid } });
      }
      assert.equal(input.action, 'undo');
      return restoreTask({ taskId: 'undo_previous_restore', type: 'gitSourceRestoreUndo', final: true, status: 'failed', error: { code: 'SOURCE_RESTORE_INTERRUPTED', message: '人工恢复失败', details: { operationOutcome: 'unknown', pending: true } } });
    },
  } });
  await change(targetField(), selectedTarget); await click(stoppedCheckbox());
  await click(byText('确认继续取回'));
  assert.equal(targetField().value, selectedTarget);
  assert.deepEqual(drafts.at(-1), { dirty: true, pending: false }, 'Recovering the previous operation did not submit this target draft');
  await click(byText('预览取回范围')); await click(confirmCheckbox()); await click(byText('确认取回源码'));
  assert.deepEqual(drafts.at(-1), { dirty: false, pending: false }, 'Only applying the selected target accepts this draft');
  needsRecovery = true; await click(byText('重新核对恢复状态')); await click(byText('确认撤回这次取回'));
  assert.match(document.body.textContent, /人工恢复失败/);
  assert.equal(targetField().value, selectedTarget);
  assert.deepEqual(drafts.at(-1), { dirty: false, pending: false }, 'An unrelated failed recovery does not clear the accepted apply draft');
});

const durabilityWarning = { code: 'SOURCE_RESTORE_DURABILITY_WARNING', message: '源码终态已读回确认，但磁盘同步未确认；请保留现场。' };
for (const retained of [false, true]) {
  test(`A ${retained ? 'retained committed' : 'successful'} restore result displays its durability warning without denying the verified source outcome`, async () => {
    const result = { operation: 'source-restore', operationOutcome: 'committed', state: 'completed', pending: false, targetCommit: nextOid, issues: [durabilityWarning], durabilityWarning };
    const value = retained
      ? restoreTask({ final: true, status: 'failed', error: { code: 'TASK_FINALIZATION_FAILED', message: '任务记录收尾失败', details: { operationOutcome: 'committed', result, durabilityWarning } } })
      : restoreTask({ final: true, status: 'succeeded', result });
    await setup({ props: { task: value } });
    const rendered = document.querySelector('.build-task');
    assert.match(rendered.textContent, retained ? /源码操作已应用，但收尾未全部确认/ : /源码已取回为待保存修改/);
    assert.match(rendered.textContent, /磁盘同步未确认/);
    assert.equal([...rendered.querySelectorAll('[role="alert"]')].filter(element => element.textContent.includes('SOURCE_RESTORE_DURABILITY_WARNING')).length, 1, 'A retained warning is visible once, even if also present in error details');
    assert.doesNotMatch(rendered.textContent, /本次没有应用源码修改|源码版本已保存|备份任务已完成/);
  });
}

test('A verified not-applied failure also shows its terminal durability warning outside a retained result', async () => {
  await setup({ props: { task: restoreTask({ final: true, status: 'interrupted', error: { code: 'SOURCE_RESTORE_INTERRUPTED', message: '已停止本次取回', details: { operationOutcome: 'not_committed', pending: false, durabilityWarning } } }) } });
  const rendered = document.querySelector('.build-task');
  assert.match(rendered.textContent, /已确认本次没有应用源码修改/);
  assert.match(rendered.textContent, /磁盘同步未确认/);
  assert.ok([...rendered.querySelectorAll('[role="alert"]')].some(element => element.textContent.includes('SOURCE_RESTORE_DURABILITY_WARNING')));
  assert.doesNotMatch(rendered.textContent, /源码已取回为待保存修改|源码版本已保存/);
});

test('Committed data-save finalization remains available when ordinary Git inspection fails', async () => {
  const operation = { saveTaskId: 'task_saved_data', phase: 'committed', journalDigest: digest, canFinalize: true, targetCommit: nextOid, issues: [] };
  const { calls } = await setup({ overrides: {
    project_git_data_save_status: async () => ({ state: 'pending', operations: [operation], issues: [] }),
    project_git_status: async () => { throw new Error('普通仓库状态暂不可读取'); },
    project_git_data_save_finalize: async () => task({ taskId: 'task_finalize_data', type: 'gitDataSaveFinalize' }),
  } });
  assert.match(document.body.textContent, /普通仓库状态暂不可读取/);
  await click(byText('核对并完成数据版本保存'));
  assert.deepEqual(commandCalls(calls, 'project_git_data_save_finalize'), [['project_git_data_save_finalize', { id: 'prj_a', input: { saveTaskId: 'task_saved_data', expectedJournalDigest: digest } }]]);
  assert.equal(commandCalls(calls, 'project_git_data_save').length, 0);
  assert.doesNotMatch(document.body.textContent, /备份任务已完成/);
});

test('Finalizing an interrupted save never claims a data version was saved when its ref did not commit', async () => {
  await setup({ props: { task: task({ type: 'gitDataSaveFinalize', final: true, status: 'succeeded', result: { operation: 'save-data-finalize', operationOutcome: 'not_committed' } }) } });
  const result = document.querySelector('.build-task').textContent;
  assert.match(result, /已确认此前保存没有提交/);
  assert.doesNotMatch(result, /数据版本已保存|数据版本的保存收尾|备份任务已完成/);
});

function dataPreview(overrides = {}) {
  return { repositoryRoot: '/disposable/project', head: { oid, branch: 'refs/heads/main' }, targetCommit: nextOid,
    documentEpoch: 'disposable_epoch', snapshotDigest: digest, liveDataDigest: 'd'.repeat(64), targetSnapshotDigest: 'e'.repeat(64), planDigest: 'f'.repeat(64),
    current: { databaseBytes: 4096, attachmentCount: 2, attachmentBytes: 100, directoryCount: 1 },
    target: { databaseBytes: 8192, attachmentCount: 1, attachmentBytes: 80, directoryCount: 1 },
    attachmentChanges: [{ path: 'example.txt', kind: 'delete' }], directoryChanges: [{ path: 'old-empty', kind: 'delete' }, { path: 'new-empty', kind: 'add' }], databaseChanged: true, canApply: false, applyImplemented: false,
    reason: '人工数据预览已核对。', issues: [], ...overrides };
}

test('Data preview is explicit, reads the selected local commit, and never submits a source or data mutation', async () => {
  const { calls } = await setup({ overrides: { project_git_data_restore_preview: async () => dataPreview() } });
  assert.equal(commandCalls(calls, 'project_git_data_restore_preview').length, 0);
  assert.equal(byText('预览数据版本').disabled, true);
  await change(targetField(), nextOid); await click(byText('预览数据版本'));
  assert.deepEqual(commandCalls(calls, 'project_git_data_restore_preview'), [['project_git_data_restore_preview', { id: 'prj_a', input: { targetCommit: nextOid } }]]);
  const section = document.querySelector('section[aria-label="预览业务数据版本"]');
  assert.match(section.textContent, /数据库内容与当前数据不同/);
  assert.match(section.textContent, /example\.txt.*删除/);
  assert.match(section.textContent, /old-empty.*删除目录/);
  assert.match(section.textContent, /new-empty.*新增目录/);
  assert.match(section.textContent, /尚不能用它替换业务数据/);
  assert.equal(commandCalls(calls, 'project_git_source_restore_apply').length, 0);
  assert.equal(commandCalls(calls, 'project_git_data_save').length, 0);
});

test('Refreshing or changing the selected commit revokes the previous data comparison', async () => {
  await setup({ overrides: { project_git_data_restore_preview: async ({ input }) => dataPreview({ targetCommit: input.targetCommit }) } });
  await change(targetField(), nextOid); await click(byText('预览数据版本'));
  assert.ok(document.querySelector('section[aria-label="预览业务数据版本"] table'));
  await change(targetField(), oid);
  assert.equal(document.querySelector('section[aria-label="预览业务数据版本"] table'), null);
  await click(byText('预览数据版本')); await click(refresh());
  assert.equal(document.querySelector('section[aria-label="预览业务数据版本"] table'), null);
});

test('Pending data preview blocks duplicate operations and ignores a response from a previous platform instance', async () => {
  const response = deferred();
  const { calls, drafts, render } = await setup({ overrides: { project_git_data_restore_preview: () => response.promise } });
  await change(targetField(), nextOid); await click(byText('预览数据版本'));
  assert.equal(drafts.at(-1).pending, true);
  assert.equal(byText('正在核对数据…').disabled, true);
  await click(byText('正在核对数据…'));
  assert.equal(commandCalls(calls, 'project_git_data_restore_preview').length, 1);
  assert.equal(commandCalls(calls, 'project_git_save').length, 0);
  await render({ instanceId: 'instance_new' });
  await act(async () => response.resolve(dataPreview()));
  assert.equal(targetField().value, nextOid);
  assert.equal(document.querySelector('section[aria-label="预览业务数据版本"] table'), null);
  assert.equal(drafts.at(-1).pending, false);
});

test('A mismatched data target or a pending recovery cannot become an actionable preview', async () => {
  let pending = false;
  const { calls } = await setup({ overrides: {
    project_git_data_restore_preview: async () => dataPreview({ targetCommit: oid }),
    project_git_data_save_status: async () => ({ state: pending ? 'blocked' : 'clear', operations: [], issues: [] }),
  } });
  await change(targetField(), nextOid); await click(byText('预览数据版本'));
  assert.match(document.querySelector('section[aria-label="预览业务数据版本"]').textContent, /返回的数据版本与选择不符/);
  assert.equal(document.querySelector('section[aria-label="预览业务数据版本"] table'), null);
  pending = true; await click(refresh());
  assert.equal(byText('预览数据版本').disabled, true);
  await click(byText('预览数据版本'));
  assert.equal(commandCalls(calls, 'project_git_data_restore_preview').length, 1);
});

const dataConfirmation = text => [...document.querySelectorAll('section[aria-label="预览业务数据版本"] label')].find(label => label.textContent === text)?.querySelector('input');
async function confirmDataRestore() {
  await click(dataConfirmation('已停止外部编辑器和其他数据写入'));
  await click(dataConfirmation('已核对数据库和附件变化，确认替换当前业务数据'));
}

test('Data restore requires both confirmations and submits the exact preview bindings only once', async () => {
  const accepted = deferred();
  const preview = dataPreview({ canApply: true, applyImplemented: true });
  const { calls, drafts } = await setup({ overrides: {
    project_git_data_restore_preview: () => preview,
    project_git_data_restore_apply: () => accepted.promise,
  } });
  await change(targetField(), nextOid); await click(byText('预览数据版本'));
  assert.equal(byText('确认取回业务数据').disabled, true);
  assert.equal(commandCalls(calls, 'project_git_data_restore_apply').length, 0);
  await click(dataConfirmation('已停止外部编辑器和其他数据写入'));
  assert.equal(byText('确认取回业务数据').disabled, true);
  await click(dataConfirmation('已核对数据库和附件变化，确认替换当前业务数据'));
  const applyButton = byText('确认取回业务数据');
  await act(async () => { applyButton.click(); applyButton.click(); });
  assert.equal(drafts.at(-1).pending, true);
  assert.deepEqual(commandCalls(calls, 'project_git_data_restore_apply'), [['project_git_data_restore_apply', { id: 'prj_a', input: {
    targetCommit: nextOid, expectedHead: oid, expectedBranch: 'refs/heads/main', expectedSnapshot: digest,
    expectedDocumentEpoch: preview.documentEpoch, expectedLiveDataDigest: preview.liveDataDigest,
    expectedTargetSnapshotDigest: preview.targetSnapshotDigest, expectedPlanDigest: preview.planDigest,
  } }]]);
  await act(async () => accepted.resolve(task({ type: 'gitDataRestoreApply' })));
  assert.equal(drafts.at(-1).pending, true);
  assert.equal(commandCalls(calls, 'project_git_save').length, 0);
});

test('A changed data selection revokes confirmations and a mismatched source binding cannot apply', async () => {
  let mismatched = false;
  const { calls } = await setup({ overrides: {
    project_git_data_restore_preview: ({ input }) => dataPreview({ targetCommit: input.targetCommit, canApply: true, applyImplemented: true, snapshotDigest: mismatched ? '9'.repeat(64) : digest }),
  } });
  await change(targetField(), nextOid); await click(byText('预览数据版本')); await confirmDataRestore();
  assert.equal(byText('确认取回业务数据').disabled, false);
  await change(targetField(), oid); await click(byText('预览数据版本'));
  assert.equal(dataConfirmation('已停止外部编辑器和其他数据写入').checked, false);
  assert.equal(dataConfirmation('已核对数据库和附件变化，确认替换当前业务数据').checked, false);
  assert.equal(byText('确认取回业务数据').disabled, true);
  mismatched = true; await click(byText('预览数据版本')); await confirmDataRestore();
  assert.equal(byText('确认取回业务数据').disabled, true);
  assert.match(document.body.textContent, /当前源码状态与数据预览不符/);
  await click(byText('确认取回业务数据'));
  assert.equal(commandCalls(calls, 'project_git_data_restore_apply').length, 0);
});

function dataRestoreOperation(overrides = {}) {
  return { restoreTaskId: 'task_original_restore', targetCommit: nextOid, planDigest: digest, journalDigest: 'd'.repeat(64),
    phase: 'data-restore-apply', operationOutcome: 'not_committed', dataState: 'unknown', documentEpoch: null,
    canContinue: true, canUndo: false, completionPublished: false, originalRetained: true,
    retainedPath: '/disposable/data-restore/original', slots: [{ kind: 'database', path: 'data/main.sqlite', state: 'displaced' }], issues: [], ...overrides };
}

test('Data recovery remains independent of a missing database and Git state and uses fresh operation guards', async () => {
  const operation = dataRestoreOperation();
  const { calls, drafts } = await setup({ overrides: {
    project_git_data_restore_status: () => ({ state: 'pending', businessBlocked: true, operations: [operation], issues: [] }),
    project_git_source_restore_status: () => { throw new Error('人工数据库暂缺，源码状态不可用'); },
    project_git_data_save_status: () => { throw new Error('人工数据库暂缺，保存状态不可用'); },
    project_git_status: () => { throw new Error('人工数据库暂缺，仓库状态不可用'); },
    project_git_data_restore_recover: () => task({ type: 'gitDataRestoreContinue' }),
  } });
  assert.equal(calls[0][0], 'project_git_data_restore_status');
  assert.match(document.body.textContent, /人工数据库暂缺/);
  assert.equal(byText('继续并核对数据取回').disabled, true);
  const checkbox = document.querySelector('section[aria-label="业务数据取回恢复"] input[type="checkbox"]');
  await click(checkbox);
  assert.equal(byText('撤回本次数据取回').disabled, true);
  await click(byText('继续并核对数据取回'));
  assert.equal(drafts.at(-1).pending, true);
  assert.deepEqual(commandCalls(calls, 'project_git_data_restore_recover'), [['project_git_data_restore_recover', { id: 'prj_a', input: {
    restoreTaskId: operation.restoreTaskId, expectedPlanDigest: operation.planDigest, expectedJournalDigest: operation.journalDigest, action: 'continue',
  } }]]);
  assert.equal(commandCalls(calls, 'project_git_data_restore_apply').length, 0);
});

test('An unavailable recovery refresh revokes an earlier data recovery confirmation', async () => {
  const operation = dataRestoreOperation(); let failed = false;
  const { calls } = await setup({ overrides: {
    project_git_data_restore_status: () => { if (failed) throw new Error('人工恢复读取失败'); return { state: 'pending', businessBlocked: true, operations: [operation], issues: [] }; },
  } });
  await click(document.querySelector('section[aria-label="业务数据取回恢复"] input[type="checkbox"]'));
  assert.equal(byText('继续并核对数据取回').disabled, false);
  failed = true; await click(refresh());
  assert.equal(byText('继续并核对数据取回').disabled, true);
  await click(byText('继续并核对数据取回'));
  assert.equal(commandCalls(calls, 'project_git_data_restore_recover').length, 0);
  assert.match(document.body.textContent, /人工恢复读取失败/);
});

for (const changedSlots of [false, true]) {
  test(`A successful data recovery refresh revokes confirmation with ${changedSlots ? 'changed' : 'identical'} slots and fixed journal digests`, async () => {
    let refreshed = false;
    const operation = dataRestoreOperation({ canUndo: true, dataState: 'before' });
    const { calls } = await setup({ overrides: {
      project_git_data_restore_status: () => ({ state: 'pending', businessBlocked: true, operations: [{ ...operation,
        slots: [{ kind: 'database', path: 'data/main.sqlite', state: refreshed && changedSlots ? 'applied' : 'displaced' }],
      }], issues: [] }),
      project_git_data_restore_recover: ({ input }) => task({ type: input.action === 'undo' ? 'gitDataRestoreUndo' : 'gitDataRestoreContinue' }),
    } });
    await change(targetField(), nextOid);
    await click(document.querySelector('section[aria-label="业务数据取回恢复"] input[type="checkbox"]'));
    assert.equal(byText('继续并核对数据取回').disabled, false);
    assert.equal(byText('撤回本次数据取回').disabled, false);
    refreshed = true; await click(refresh());
    assert.equal(document.querySelector('section[aria-label="业务数据取回恢复"] input[type="checkbox"]').checked, false);
    assert.equal(byText('继续并核对数据取回').disabled, true);
    assert.equal(byText('撤回本次数据取回').disabled, true);
    await click(byText('继续并核对数据取回')); await click(byText('撤回本次数据取回'));
    assert.equal(commandCalls(calls, 'project_git_data_restore_recover').length, 0);
    assert.equal(targetField().value, nextOid);
    await click(document.querySelector('section[aria-label="业务数据取回恢复"] input[type="checkbox"]'));
    await click(byText('撤回本次数据取回'));
    assert.equal(commandCalls(calls, 'project_git_data_restore_recover')[0][1].input.action, 'undo');
  });
}

test('A failed data recovery submission and explicit recheck cannot reuse its confirmation', async () => {
  const operation = dataRestoreOperation({ canUndo: true });
  const { calls } = await setup({ overrides: {
    project_git_data_restore_status: () => ({ state: 'pending', businessBlocked: true, operations: [operation], issues: [] }),
    project_git_data_restore_recover: () => { throw new Error('人工恢复提交结果未知，请重新核对'); },
  } });
  await change(targetField(), nextOid);
  await click(document.querySelector('section[aria-label="业务数据取回恢复"] input[type="checkbox"]'));
  await click(byText('继续并核对数据取回'));
  assert.match(document.body.textContent, /人工恢复提交结果未知/);
  assert.equal(document.querySelector('section[aria-label="业务数据取回恢复"] input[type="checkbox"]').checked, false);
  await click(refresh());
  assert.equal(byText('继续并核对数据取回').disabled, true);
  assert.equal(byText('撤回本次数据取回').disabled, true);
  assert.equal(commandCalls(calls, 'project_git_data_restore_recover').length, 1);
  assert.equal(targetField().value, nextOid);
});

test('A platform instance change discards data recovery confirmation while retaining project drafts', async () => {
  const operation = dataRestoreOperation({ canUndo: true });
  const { calls, render } = await setup({ overrides: {
    project_git_data_restore_status: () => ({ state: 'pending', businessBlocked: true, operations: [operation], issues: [] }),
  } });
  await change(targetField(), nextOid);
  await click(document.querySelector('section[aria-label="业务数据取回恢复"] input[type="checkbox"]'));
  assert.equal(byText('继续并核对数据取回').disabled, false);
  await render({ instanceId: 'instance_after_restart' });
  assert.equal(document.querySelector('section[aria-label="业务数据取回恢复"] input[type="checkbox"]').checked, false);
  assert.equal(byText('继续并核对数据取回').disabled, true);
  assert.equal(byText('撤回本次数据取回').disabled, true);
  assert.equal(commandCalls(calls, 'project_git_data_restore_recover').length, 0);
  assert.equal(targetField().value, nextOid);
});

for (const outcome of ['not_committed', 'committed', 'unknown']) {
  test(`Failed data restore reports ${outcome} without claiming unchanged data or a saved source version`, async () => {
    const result = { operation: 'data-restore-apply', operationOutcome: outcome, dataState: 'unknown', pending: true, businessBlocked: true, retainedPath: '/disposable/retained-data', restoreTaskId: 'task_original_restore', targetCommit: nextOid };
    await setup({ props: { task: task({ type: 'gitDataRestoreApply', final: true, status: 'failed', error: { code: 'DATA_RESTORE_RECOVERY_REQUIRED', message: '人工恢复待核对', details: { operationOutcome: outcome, result } } }) } });
    const section = document.querySelector('.build-task');
    assert.match(section.textContent, /数据取回|数据切换/);
    assert.match(section.textContent, /disposable\/retained-data/);
    assert.doesNotMatch(section.textContent, /源码和数据保持原状|原内容保持不变|源码版本已保存|备份任务已完成/);
    if (outcome === 'not_committed') assert.match(section.textContent, /部分数据可能已移动/);
  });
}

test('A flat backend data recovery error retains its task identity and evidence path in the task result', async () => {
  await setup({ props: { task: task({ type: 'gitDataRestoreApply', final: true, status: 'interrupted', error: {
    code: 'PROJECT_DATA_RESTORE_INTERRUPTED', message: '人工中断后需核对', details: {
      operationOutcome: 'not_committed', dataState: 'before', pending: true, businessBlocked: true,
      liveDataChanged: true, originalRetained: true, restoreTaskId: 'task_flat_original', retainedPath: '/disposable/flat-retained-data',
    },
  } }) } });
  const section = document.querySelector('.build-task');
  assert.match(section.textContent, /部分数据可能已移动/);
  assert.match(section.textContent, /数据取回任务：task_flat_original/);
  assert.match(section.textContent, /保留的数据恢复现场：\/disposable\/flat-retained-data/);
  assert.doesNotMatch(section.textContent, /源码和数据保持原状|原内容保持不变/);
});
