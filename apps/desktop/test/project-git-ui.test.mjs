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
  return { taskId: 'task_git_disposable', projectId: 'prj_a', type: 'gitSave', status: 'running', final: false, phase: 'snapshot', result: null, error: null, ...overrides };
}
function deferred() { let resolve; let reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { resolve, reject, promise }; }
async function setup({ overrides = {}, props = {}, strict = false } = {}) {
  const calls = []; const tasks = []; const drafts = [];
  globalThis.__PROJECT_GIT_INVOKE__ = async (command, args) => {
    calls.push([command, args]);
    if (overrides[command]) return overrides[command](args);
    if (command === 'project_git_status') return status();
    if (command === 'project_git_source_restore_status') return recoveryStatus();
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
async function fill() { await change(field('版本说明'), '保存本机源码修改'); await change(field('作者姓名'), 'Disposable Author'); await change(field('作者邮箱'), 'disposable@example.invalid'); }
async function submit() { await act(async () => document.querySelector('form').dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true }))); }
const commandCalls = (calls, command) => calls.filter(([name]) => name === command);

test('Opening the panel only previews local status and shows scope, exclusions and the confirmed HEAD', async () => {
  const { calls, drafts } = await setup();
  assert.deepEqual(calls, [['project_git_source_restore_status', { id: 'prj_a' }], ['project_git_status', { id: 'prj_a' }]]);
  assert.ok(document.body.textContent.includes(oid));
  assert.match(document.body.textContent, /分支：main/);
  assert.doesNotMatch(document.body.textContent, /refs\/heads\/main/);
  assert.match(document.body.textContent, /本次不保存数据库、附件或数据快照/);
  assert.match(document.body.textContent, /预览后将已保存源码取回为待保存修改/);
  assert.match(document.body.textContent, /app\/\.env/);
  assert.equal(field('作者姓名').value, '');
  assert.equal(field('作者邮箱').value, '');
  assert.equal(save().disabled, true);
  assert.deepEqual(drafts.at(-1), { dirty: false, pending: false });
});

test('Explicit initialization sends the reviewed snapshot and main, prevents duplicates and refreshes after its task', async () => {
  const pending = deferred(); let reads = 0;
  const { calls, tasks, drafts } = await setup({ overrides: {
    project_git_status: () => ++reads === 1 ? status({ repository: { ...status().repository, state: 'none', root: null, head: null, markers: [], indexState: 'not-inspected' }, canInitialize: true, canSave: false }) : status(),
    project_git_initialize: () => pending.promise,
    task_get: () => task({ type: 'gitInitialize', final: true, status: 'succeeded', result: { operation: 'initialize', operationOutcome: 'committed', head: null, branch: 'main' } }),
  } });
  await click(byText('初始化本项目 Git'));
  assert.deepEqual(drafts.at(-1), { dirty: false, pending: true });
  await click(byText('初始化本项目 Git'));
  await submit();
  assert.deepEqual(commandCalls(calls, 'project_git_initialize'), [['project_git_initialize', { id: 'prj_a', input: { expectedSnapshot: digest, initialBranch: 'main' } }]]);
  assert.equal(commandCalls(calls, 'project_git_save').length, 0);
  await act(async () => pending.resolve(task({ type: 'gitInitialize' })));
  assert.equal(tasks.length, 1);
  assert.equal(drafts.at(-1).pending, false);
  await tick();
  assert.deepEqual(commandCalls(calls, 'task_get'), [['task_get', { taskId: 'task_git_disposable' }]]);
  assert.equal(reads, 2);
  assert.match(document.body.textContent, /尚未保存首个版本/);
  assert.doesNotMatch(document.body.textContent, /备份任务已完成/);
});

test('Saving requires explicit author and message, sends an explicit null unborn HEAD, and preserves accepted inputs', async () => {
  const { calls, drafts } = await setup({ overrides: {
    project_git_status: () => status({ repository: { ...status().repository, head: { oid: null, branch: 'refs/heads/main' } } }),
    project_git_save: () => task({ final: true, status: 'succeeded', result: { operationOutcome: 'committed', commit: nextOid } }),
  } });
  await submit();
  await change(field('版本说明'), '保存本机源码修改');
  await change(field('作者姓名'), 'Disposable Author');
  await submit();
  assert.equal(commandCalls(calls, 'project_git_save').length, 0);
  await change(field('作者邮箱'), 'disposable@example.invalid');
  assert.equal(save().disabled, false);
  await submit();
  assert.deepEqual(commandCalls(calls, 'project_git_save'), [['project_git_save', { id: 'prj_a', input: { expectedHead: null, expectedSnapshot: digest, message: '保存本机源码修改', author: { name: 'Disposable Author', email: 'disposable@example.invalid' } } }]]);
  assert.equal(field('版本说明').value, '保存本机源码修改');
  assert.equal(field('作者邮箱').value, 'disposable@example.invalid');
  assert.deepEqual(drafts.at(-1), { dirty: false, pending: false });
  assert.match(document.body.textContent, /源码版本已保存/);
});

test('A clean source scope disables saving even when the repository supports saves, including direct form submission', async () => {
  let changed = false;
  const { calls } = await setup({ overrides: { project_git_status: () => status({ changes: changed ? status().changes : [], canSave: true }) } });
  await fill();
  assert.equal(save().disabled, true);
  assert.match(document.body.textContent, /没有未保存的源码变更/);
  await submit();
  assert.equal(commandCalls(calls, 'project_git_save').length, 0);
  changed = true; await click(refresh());
  assert.equal(save().disabled, false);
  assert.equal(field('版本说明').value, '保存本机源码修改');
});

for (const readFails of [false, true]) {
  test(`The post-save recheck notice ends when the status read ${readFails ? 'fails' : 'completes with a clean scope'}`, async () => {
    const recheck = deferred(); let reads = 0;
    const { calls } = await setup({ overrides: {
      project_git_status: () => ++reads === 1 ? status() : recheck.promise,
      project_git_save: () => task(),
      task_get: () => task({ final: true, status: 'succeeded', result: { operationOutcome: 'committed', commit: nextOid } }),
    } });
    await fill(); await submit(); await tick();
    assert.match(document.body.textContent, /任务已结束，正在重新核对/);
    assert.equal(save().disabled, true);
    await act(async () => {
      if (readFails) recheck.reject(new Error('人工核对失败，请保留已提交版本'));
      else recheck.resolve(status({ changes: [], repository: { ...status().repository, head: { oid: nextOid, branch: 'refs/heads/main' } } }));
    });
    assert.doesNotMatch(document.body.textContent, /任务已结束，正在重新核对/);
    assert.match(document.body.textContent, /源码版本已保存/);
    assert.ok(document.body.textContent.includes(nextOid));
    if (readFails) assert.match(document.body.textContent, /人工核对失败/);
    else assert.match(document.body.textContent, /没有未保存的源码变更/);
    assert.equal(save().disabled, true);
    await submit(); assert.equal(commandCalls(calls, 'project_git_save').length, 1);
  });
}

test('Finishing a status recheck preserves a newer cancellation result notice', async () => {
  const cancellation = deferred(); const recheck = deferred(); let reads = 0;
  const finalTask = task({ final: true, status: 'failed', error: { code: 'CANCELLED', message: '人工取消结果', details: { operationOutcome: 'not_committed' } } });
  await setup({ overrides: {
    project_git_status: () => ++reads === 1 ? status() : recheck.promise,
    project_git_save: () => task(),
    project_git_cancel: () => cancellation.promise,
    task_get: () => finalTask,
  } });
  await fill(); await submit(); await click(byText('取消当前操作')); await tick();
  assert.match(document.body.textContent, /任务已结束，正在重新核对/);
  await act(async () => cancellation.resolve(finalTask));
  assert.match(document.body.textContent, /任务已经结束，请核对最终结果/);
  await act(async () => recheck.resolve(status()));
  assert.match(document.body.textContent, /任务已经结束，请核对最终结果/);
  assert.match(document.body.textContent, /人工取消结果/);
});

test('A rejected save retains draft/error and requires a refreshed snapshot before explicit retry', async () => {
  let reads = 0; let writes = 0;
  const { calls, drafts } = await setup({ overrides: {
    project_git_status: () => status({ snapshotDigest: ++reads === 1 ? digest : 'd'.repeat(64) }),
    project_git_save: () => { writes += 1; throw { code: 'PROJECT_GIT_STALE', message: '源码已变化，请重新预览' }; },
  } });
  await fill(); await submit();
  assert.equal(writes, 1);
  assert.equal(save().disabled, true);
  assert.equal(field('版本说明').value, '保存本机源码修改');
  assert.deepEqual(drafts.at(-1), { dirty: true, pending: false });
  assert.match(document.body.textContent, /源码已变化，请重新预览/);
  await submit(); assert.equal(writes, 1);
  await click(refresh());
  assert.match(document.body.textContent, /源码已变化，请重新预览/);
  assert.equal(save().disabled, false);
  await submit();
  assert.equal(writes, 2);
  assert.equal(commandCalls(calls, 'project_git_save')[1][1].input.expectedSnapshot, 'd'.repeat(64));
  assert.equal(commandCalls(calls, 'project_git_save')[0][1].input.expectedHead, oid);
});

test('Unclassified and protected tracked paths keep writes unavailable and remain visible', async () => {
  const value = status();
  value.scope = { ...value.scope, complete: false, unclassifiedPaths: ['other-source'], protectedTrackedPaths: ['data/main.sqlite'] };
  value.canSave = false;
  value.issues = [{ code: 'PROJECT_GIT_SCOPE_INCOMPLETE', message: '请先核对未分类文件' }];
  const { calls } = await setup({ overrides: { project_git_status: () => value } });
  await fill(); await submit();
  assert.equal(save().disabled, true);
  assert.equal(commandCalls(calls, 'project_git_save').length, 0);
  assert.match(document.body.textContent, /other-source/);
  assert.match(document.body.textContent, /data\/main.sqlite/);
  assert.match(document.body.textContent, /不会自行移除跟踪或改写历史/);
});

test('A polling failure keeps the task active, inputs retained and duplicate submissions blocked', async () => {
  const { calls, drafts } = await setup({ overrides: {
    project_git_save: () => task(),
    task_get: () => { throw new Error('人工状态读取失败'); },
  } });
  await fill(); await submit(); await tick();
  assert.match(document.body.textContent, /任务状态暂时无法读取/);
  assert.equal(save().disabled, true);
  assert.equal(field('作者姓名').value, 'Disposable Author');
  assert.deepEqual(drafts.at(-1), { dirty: false, pending: false });
  await submit(); await click(refresh());
  assert.equal(commandCalls(calls, 'project_git_save').length, 1);
  assert.equal(commandCalls(calls, 'task_get').length, 2);
});

test('Cancellation is only a request; a later stale acknowledgement cannot replace a successful final task', async () => {
  const cancellation = deferred();
  const { calls, tasks } = await setup({ overrides: {
    project_git_save: () => task(),
    project_git_cancel: () => cancellation.promise,
    task_get: () => task({ final: true, status: 'succeeded', result: { operationOutcome: 'committed', commit: nextOid } }),
  } });
  await fill(); await submit(); await click(byText('取消当前操作'));
  await tick();
  await act(async () => cancellation.resolve(task()));
  assert.equal(tasks.at(-1).final, true);
  assert.equal(tasks.at(-1).status, 'succeeded');
  assert.equal(byText('取消当前操作'), undefined);
  assert.deepEqual(commandCalls(calls, 'project_git_cancel'), [['project_git_cancel', { id: 'prj_a', taskId: 'task_git_disposable' }]]);
  assert.match(document.body.textContent, /源码版本已保存/);
});

test('A final unknown outcome preserves the draft and never resubmits automatically', async () => {
  let reads = 0;
  const { calls, drafts } = await setup({ overrides: {
    project_git_save: () => task(),
    task_get: () => task({ final: true, status: 'failed', error: { code: 'PROJECT_GIT_RECOVERY_REQUIRED', message: '需要检查保留现场', details: { operationOutcome: 'unknown' } } }),
    project_git_status: () => ++reads === 1 ? status() : status({ canSave: false, issues: [{ code: 'PROJECT_GIT_RECOVERY_REQUIRED', message: '先检查操作现场' }] }),
  } });
  await fill(); await submit(); await tick();
  assert.match(document.body.textContent, /尚未确认|尚不能确认|无法确认|结果不明|结果尚/);
  assert.doesNotMatch(document.body.textContent, /任务已结束，正在重新核对/);
  assert.equal(commandCalls(calls, 'project_git_save').length, 1);
  assert.deepEqual(drafts.at(-1), { dirty: true, pending: false });
  assert.equal(field('版本说明').value, '保存本机源码修改');
  assert.equal(save().disabled, true);
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

test('A late task poll cannot publish results or callbacks after changing projects', async () => {
  const late = deferred();
  const { render, tasks } = await setup({ overrides: {
    project_git_save: () => task(),
    task_get: () => late.promise,
  } });
  await fill(); await submit(); await tick();
  assert.equal(tasks.length, 1);
  await render({ projectId: 'prj_b' });
  await act(async () => late.resolve(task({ final: true, status: 'succeeded', result: { operationOutcome: 'committed', commit: nextOid } })));
  assert.equal(tasks.length, 1);
  assert.doesNotMatch(document.body.textContent, /源码版本已保存/);
  assert.equal(byText('取消当前操作'), undefined);
});

test('A committed version with failed finalization exposes its actual commit and recovery outcome', async () => {
  let reads = 0;
  const { calls } = await setup({ overrides: {
    project_git_status: () => ++reads === 1 ? status() : status({ canSave: false, issues: [{ code: 'PROJECT_GIT_RECOVERY_REQUIRED', message: '需要检查收尾现场' }] }),
    project_git_save: () => task({ final: true, status: 'failed', error: { code: 'PROJECT_GIT_RECOVERY_REQUIRED', message: '索引收尾未完成', details: { operationOutcome: 'committed', result: { operation: 'save', operationOutcome: 'committed', commit: nextOid } } } }),
  } });
  await fill(); await submit();
  assert.match(document.body.textContent, /已提交|已保存/);
  assert.match(document.body.textContent, /收尾/);
  assert.ok(document.body.textContent.includes(nextOid));
  assert.equal(commandCalls(calls, 'project_git_save').length, 1);
  assert.equal(save().disabled, true);
});

test('Project and platform-instance changes discard late status, task responses and callbacks', async () => {
  const lateStatus = deferred(); const lateSave = deferred();
  let first = true;
  const { render, tasks } = await setup({ overrides: {
    project_git_status: ({ id }) => first ? (first = false, lateStatus.promise) : status({ repository: { ...status().repository, root: `/disposable/${id}` } }),
    project_git_save: () => lateSave.promise,
  } });
  await render({ projectId: 'prj_b' });
  await act(async () => lateStatus.resolve(status({ repository: { ...status().repository, root: '/disposable/old-project' } })));
  assert.match(document.body.textContent, /\/disposable\/prj_b/);
  assert.doesNotMatch(document.body.textContent, /old-project/);
  await fill(); await submit();
  await render({ instanceId: 'instance_b' });
  await act(async () => lateSave.resolve(task({ projectId: 'prj_b' })));
  assert.equal(tasks.length, 0);
  assert.equal(field('作者姓名').value, 'Disposable Author');
  assert.equal(byText('取消当前操作'), undefined);
});

test('Reopening a matching active Git task restores polling and cancellation without a conflicting status read', async () => {
  const pollResult = deferred();
  const { calls } = await setup({ props: { task: task() }, overrides: {
    task_get: () => pollResult.promise,
    project_git_cancel: () => task(),
  } });
  assert.deepEqual(calls, [['task_get', { taskId: 'task_git_disposable' }]]);
  assert.ok(byText('取消当前操作'));
  await click(byText('取消当前操作'));
  assert.equal(commandCalls(calls, 'project_git_cancel').length, 1);
  await act(async () => pollResult.resolve(task({ final: true, status: 'succeeded', result: { operationOutcome: 'committed', commit: nextOid } })));
  assert.equal(commandCalls(calls, 'project_git_status').length, 1);
  assert.equal(byText('取消当前操作'), undefined);
});

test('External task updates reject other projects, unrelated operations and a stale final-to-running downgrade', async () => {
  const { render, calls, tasks } = await setup({ props: { task: task({ projectId: 'prj_other' }) } });
  assert.equal(commandCalls(calls, 'task_get').length, 0);
  await render({ task: task({ type: 'build' }) });
  assert.equal(byText('取消当前操作'), undefined);
  await render({ task: task({ final: true, status: 'succeeded', result: { operationOutcome: 'committed', commit: nextOid } }) });
  assert.match(document.body.textContent, /源码版本已保存/);
  await render({ task: task() });
  assert.equal(byText('取消当前操作'), undefined);
  assert.equal(tasks.length, 0);
  assert.equal(commandCalls(calls, 'task_get').length, 0);
});

test('An unknown candidate commit is labeled unconfirmed and cannot be presented as an already saved result', async () => {
  const unknown = task({ final: true, status: 'failed', error: { code: 'PROJECT_GIT_RECOVERY_REQUIRED', message: '引用事务结果不明', details: { operationOutcome: 'unknown', candidateCommit: nextOid, result: { operation: 'save', operationOutcome: 'committed', commit: nextOid } } } });
  await setup({ props: { task: unknown } });
  assert.match(document.body.textContent, /候选提交（是否已保存尚未确认）/);
  assert.ok(document.body.textContent.includes(nextOid));
  assert.doesNotMatch(document.body.textContent, /源码版本已保存/);
});

test('StrictMode remounting rejects a stale initial read and disabled props block writes without dropping drafts', async () => {
  const old = deferred(); let reads = 0;
  const { render, calls, drafts } = await setup({ strict: true, overrides: {
    project_git_source_restore_status: () => ++reads === 1 ? old.promise : recoveryStatus(),
  } });
  await act(async () => old.resolve(recoveryStatus({ issues: [{ message: 'stale-strict-read' }] })));
  assert.doesNotMatch(document.body.textContent, /stale-strict-read/);
  await fill(); await render({ disabled: true }); await submit();
  assert.equal(save().disabled, true);
  assert.equal(commandCalls(calls, 'project_git_save').length, 0);
  assert.equal(field('作者姓名').value, 'Disposable Author');
  await unmount();
  assert.deepEqual(drafts.at(-1), { dirty: false, pending: false });
});

function versions(overrides = {}) {
  return { repositoryRoot: '/disposable/project', head: status().repository.head, versions: [{ oid, parents: [nextOid], subject: 'Disposable saved version', author: { name: 'Fixture Author', email: 'fixture@example.invalid' }, authoredAt: '2026-10-22T08:00:00+00:00' }], limit: 50, hasMore: false, ...overrides };
}
function branchStatus(overrides = {}) { return status({ changes: [], canCreateBranch: true, ...overrides }); }
function branchTask(overrides = {}) { return task({ type: 'gitBranchCreate', ...overrides }); }
const branchButton = () => byText('创建并切换到新分支');
async function submitBranch() { await act(async () => document.querySelector('form[aria-label="创建定制分支"]').dispatchEvent(new window.Event('submit', { bubbles: true, cancelable: true }))); }

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

test('A pending version read blocks competing status and writes; an instance change rejects its late list but keeps drafts', async () => {
  const delayed = deferred();
  const { calls, render, drafts } = await setup({ overrides: { project_git_status: () => branchStatus(), project_git_versions: () => delayed.promise } });
  await fill(); await change(field('新分支名称'), 'custom/preserved');
  await click(byText('查看本机版本')); await click(byText('读取版本中…')); await click(refresh()); await submitBranch();
  assert.equal(commandCalls(calls, 'project_git_versions').length, 1);
  assert.equal(commandCalls(calls, 'project_git_status').length, 1);
  assert.equal(commandCalls(calls, 'project_git_branch_create').length, 0);
  assert.equal(drafts.at(-1).pending, false, 'Read-only history does not prevent closing');
  await render({ instanceId: 'instance_b' });
  await act(async () => delayed.resolve(versions({ versions: [{ ...versions().versions[0], subject: 'old-instance-history' }] })));
  assert.doesNotMatch(document.body.textContent, /old-instance-history/);
  assert.equal(field('版本说明').value, '保存本机源码修改');
  assert.equal(field('新分支名称').value, 'custom/preserved');
  assert.equal(commandCalls(calls, 'project_git_versions').length, 1, 'The new instance requires a new explicit history read');
});

test('History read failures stay visible and an observed HEAD change disables writes until status is refreshed', async () => {
  let reads = 0;
  const { calls } = await setup({ overrides: {
    project_git_status: () => branchStatus(),
    project_git_versions: () => { if (++reads === 1) throw new Error('Artificial history failure'); return versions({ head: { oid: nextOid, branch: 'refs/heads/other' } }); },
  } });
  await change(field('新分支名称'), 'custom/new'); await click(byText('查看本机版本'));
  assert.match(document.body.textContent, /Artificial history failure/);
  await click(byText('查看本机版本'));
  assert.match(document.body.textContent, /当前分支或版本已变化/);
  assert.equal(branchButton().disabled, true);
  await submitBranch(); assert.equal(commandCalls(calls, 'project_git_branch_create').length, 0);
});

test('Creating a branch requires an explicit valid name and a clean supported HEAD, independent of author inputs', async () => {
  const { calls, render } = await setup({ overrides: { project_git_status: () => branchStatus() } });
  for (const invalid of ['', 'HEAD', '-topic', 'topic//part', 'topic/..', 'topic/.hidden', 'topic/a.lock', 'topic..name', 'topic.', 'topic/', 'two words', '中文']) {
    await change(field('新分支名称'), invalid); await submitBranch(); assert.equal(branchButton().disabled, true, invalid);
  }
  assert.equal(commandCalls(calls, 'project_git_branch_create').length, 0);
  await change(field('新分支名称'), 'custom/my_changes-v2');
  assert.equal(field('作者姓名').value, ''); assert.equal(branchButton().disabled, false);
  await render({ disabled: true }); await submitBranch();
  assert.equal(commandCalls(calls, 'project_git_branch_create').length, 0);
});

test('Dirty sources, staged content, missing HEAD and unavailable branch capability cannot submit even with a valid name', async () => {
  for (const value of [branchStatus({ changes: status().changes }), branchStatus({ repository: { ...status().repository, indexState: 'staged-changes' } }), branchStatus({ repository: { ...status().repository, head: { oid: null, branch: 'refs/heads/main' } } }), branchStatus({ canCreateBranch: false })]) {
    const { calls } = await setup({ overrides: { project_git_status: () => value } });
    await change(field('新分支名称'), 'custom/new'); await submitBranch();
    assert.equal(branchButton().disabled, true); assert.equal(commandCalls(calls, 'project_git_branch_create').length, 0);
    await unmount();
  }
});

test('Branch creation sends the checked HEAD, full branch and snapshot once, then refreshes status and an opened history', async () => {
  const accepted = deferred(); let changed = false;
  const result = { operation: 'create-branch', operationOutcome: 'committed', head: oid, previousBranch: 'refs/heads/main', branch: 'refs/heads/custom/my_changes', name: 'custom/my_changes' };
  const { calls, drafts } = await setup({ overrides: {
    project_git_status: () => branchStatus({ repository: { ...status().repository, head: { oid, branch: changed ? result.branch : 'refs/heads/main' } } }),
    project_git_versions: () => versions({ head: { oid, branch: changed ? result.branch : 'refs/heads/main' } }),
    project_git_branch_create: () => accepted.promise,
  } });
  await fill(); await change(field('新分支名称'), result.name); await click(byText('查看本机版本'));
  await submitBranch(); await submitBranch();
  assert.deepEqual(commandCalls(calls, 'project_git_branch_create'), [['project_git_branch_create', { id: 'prj_a', input: { name: result.name, expectedHead: oid, expectedBranch: 'refs/heads/main', expectedSnapshot: digest } }]]);
  assert.equal(drafts.at(-1).pending, true);
  changed = true; await act(async () => accepted.resolve(branchTask({ final: true, status: 'succeeded', result })));
  assert.equal(drafts.at(-1).pending, false);
  assert.equal(drafts.at(-1).dirty, true, 'Unsubmitted author and message remain a draft');
  assert.equal(field('版本说明').value, '保存本机源码修改');
  assert.equal(field('作者姓名').value, 'Disposable Author');
  assert.match(document.body.textContent, /分支：custom\/my_changes/);
  assert.match(document.body.textContent, /业务数据和运行产物保持不变/);
  assert.equal(commandCalls(calls, 'project_git_versions').length, 2);
  const commands = calls.map(([command]) => command);
  assert.deepEqual(commands, ['project_git_source_restore_status', 'project_git_status', 'project_git_versions', 'project_git_branch_create', 'project_git_source_restore_status', 'project_git_status', 'project_git_versions']);
});

test('A rejected branch keeps its name and author draft, requires rechecking and never retries itself', async () => {
  const { calls, drafts } = await setup({ overrides: {
    project_git_status: () => branchStatus(),
    project_git_branch_create: () => { throw { code: 'PROJECT_GIT_BRANCH_EXISTS', message: '分支名称已存在' }; },
  } });
  await fill(); await change(field('新分支名称'), 'custom/existing'); await submitBranch(); await submitBranch();
  assert.equal(commandCalls(calls, 'project_git_branch_create').length, 1);
  assert.match(document.body.textContent, /分支名称已存在/);
  assert.equal(field('新分支名称').value, 'custom/existing'); assert.equal(field('作者邮箱').value, 'disposable@example.invalid');
  assert.deepEqual(drafts.at(-1), { dirty: true, pending: false });
  assert.equal(branchButton().disabled, true);
  await click(refresh()); assert.equal(branchButton().disabled, false);
});

test('Restored branch tasks support polling and cancellation without automatic history access or duplicate branching', async () => {
  const ended = deferred();
  const { calls, tasks } = await setup({ props: { task: branchTask() }, overrides: {
    task_get: () => ended.promise,
    project_git_cancel: () => branchTask(),
    project_git_status: () => branchStatus(),
  } });
  assert.deepEqual(calls.map(([command]) => command), ['task_get']);
  await click(byText('取消当前操作'));
  await act(async () => ended.resolve(branchTask({ final: true, status: 'failed', error: { code: 'PROJECT_GIT_RECOVERY_REQUIRED', message: '人工分支结果不明', details: { operationOutcome: 'unknown' } } })));
  assert.equal(tasks.at(-1).final, true); assert.match(document.body.textContent, /人工分支结果不明/);
  assert.equal(commandCalls(calls, 'project_git_branch_create').length, 0);
  assert.equal(commandCalls(calls, 'project_git_versions').length, 0);
});

test('A pending branch request blocks closing and its late response cannot publish across a service change', async () => {
  const accepted = deferred();
  const { calls, render, drafts, tasks } = await setup({ overrides: { project_git_status: () => branchStatus(), project_git_branch_create: () => accepted.promise } });
  await change(field('新分支名称'), 'custom/pending'); await submitBranch();
  assert.deepEqual(drafts.at(-1), { dirty: true, pending: true });
  await render({ instanceId: 'new-instance' });
  await act(async () => accepted.resolve(branchTask()));
  assert.equal(tasks.length, 0); assert.equal(byText('取消当前操作'), undefined);
  assert.equal(field('新分支名称').value, 'custom/pending');
  assert.deepEqual(drafts.at(-1), { dirty: true, pending: false });
  assert.equal(commandCalls(calls, 'project_git_branch_create').length, 1);
});

test('An accepted branch can continue in the background while an unknown final outcome restores its draft guard', async () => {
  const { calls, drafts } = await setup({ overrides: {
    project_git_status: () => branchStatus(),
    project_git_branch_create: () => branchTask(),
    task_get: () => branchTask({ final: true, status: 'failed', error: { code: 'PROJECT_GIT_RECOVERY_REQUIRED', message: '人工分支引用结果未知', details: { operationOutcome: 'unknown' } } }),
  } });
  await change(field('新分支名称'), 'custom/uncertain'); await submitBranch();
  assert.deepEqual(drafts.at(-1), { dirty: false, pending: false });
  await tick();
  assert.deepEqual(drafts.at(-1), { dirty: true, pending: false });
  assert.equal(field('新分支名称').value, 'custom/uncertain');
  assert.match(document.body.textContent, /人工分支引用结果未知/);
  assert.equal(commandCalls(calls, 'project_git_branch_create').length, 1);
});

test('An explicit Git version without branch support retains saving and history capabilities with a branch-specific explanation', async () => {
  const { calls } = await setup({ overrides: {
    project_git_status: () => status({ branchCreationIssue: { code: 'PROJECT_GIT_BRANCH_VERSION_UNSUPPORTED', message: '当前 Git 版本尚未验证新分支操作；源码保存和版本读取仍可使用。' } }),
    project_git_versions: () => versions(),
  } });
  await fill(); await change(field('新分支名称'), 'custom/new');
  assert.equal(save().disabled, false); assert.equal(branchButton().disabled, true);
  assert.match(document.body.textContent, /PROJECT_GIT_BRANCH_VERSION_UNSUPPORTED/);
  await submitBranch(); await click(byText('查看本机版本'));
  assert.equal(commandCalls(calls, 'project_git_branch_create').length, 0);
  assert.equal(commandCalls(calls, 'project_git_versions').length, 1);
  assert.equal(save().disabled, false);
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
  assert.deepEqual(calls.map(([command]) => command), ['project_git_source_restore_status', 'project_git_status']);
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
  await fill(); await click(byText('预览取回范围'));
  assert.equal(drafts.at(-1).pending, true);
  await click(refresh()); await click(byText('重新核对恢复状态')); await click(byText('查看本机版本')); await submitBranch();
  assert.equal(commandCalls(calls, 'project_git_status').length, 1);
  assert.equal(commandCalls(calls, 'project_git_versions').length, 0);
  assert.equal(commandCalls(calls, 'project_git_source_restore_status').length, 1);
  await render({ instanceId: 'instance_new' });
  await act(async () => preview.resolve(restorePreview()));
  assert.equal(targetField().value, nextOid); assert.equal(field('版本说明').value, '保存本机源码修改');
  assert.equal(document.querySelector('[aria-label="源码取回预览"]'), null);
  assert.equal(stoppedCheckbox().checked, false);
  assert.equal(drafts.at(-1).pending, false);
});

test('Editing the target or refreshing status expires the preview and its confirmation without discarding drafts', async () => {
  const { calls } = await setup({ overrides: { project_git_status: () => branchStatus(), project_git_source_restore_preview: ({ input }) => restorePreview({ targetCommit: input.targetCommit }) } });
  await fill(); await prepareRestore(); await click(confirmCheckbox());
  await change(targetField(), oid);
  assert.equal(document.querySelector('[aria-label="源码取回预览"]'), null);
  await click(byText('预览取回范围')); assert.equal(confirmCheckbox().checked, false);
  await click(confirmCheckbox()); await click(refresh());
  assert.equal(document.querySelector('[aria-label="源码取回预览"]'), null);
  assert.equal(targetField().value, oid); assert.equal(field('作者姓名').value, 'Disposable Author');
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
  await fill(); await change(field('新分支名称'), 'custom/new');
  assert.equal(save().disabled, true); assert.equal(branchButton().disabled, true); assert.equal(byText('预览取回范围').disabled, true);
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
