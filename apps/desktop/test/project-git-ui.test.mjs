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
  assert.deepEqual(calls, [['project_git_status', { id: 'prj_a' }]]);
  assert.ok(document.body.textContent.includes(oid));
  assert.match(document.body.textContent, /分支：main/);
  assert.doesNotMatch(document.body.textContent, /refs\/heads\/main/);
  assert.match(document.body.textContent, /本次不保存数据库、附件或数据快照/);
  assert.match(document.body.textContent, /不提供历史版本取回或切换到已有分支/);
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
    project_git_status: () => ++reads === 1 ? old.promise : status(),
  } });
  await act(async () => old.resolve(status({ repository: { ...status().repository, root: '/disposable/stale-strict-read' } })));
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

test('Local versions read only after an explicit click, use the fixed limit and display bounded history without actions', async () => {
  const { calls } = await setup({ overrides: { project_git_versions: () => versions({ hasMore: true }) } });
  assert.equal(commandCalls(calls, 'project_git_versions').length, 0);
  await click(byText('查看本机版本'));
  assert.deepEqual(commandCalls(calls, 'project_git_versions'), [['project_git_versions', { id: 'prj_a', limit: 50 }]]);
  const history = document.querySelector('[aria-label="本机源码版本列表"]');
  assert.match(history.textContent, /Disposable saved version/);
  assert.match(history.textContent, /Fixture Author.*fixture@example.invalid/);
  assert.match(history.textContent, /最近 50 个.*更早版本/);
  assert.match(history.textContent, /不会创建分支、取回源码或访问远端/);
  assert.equal(commandCalls(calls, 'project_git_branch_create').length, 0);
  assert.equal(history.querySelectorAll('button').length, 1);
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
  assert.deepEqual(commands, ['project_git_status', 'project_git_versions', 'project_git_branch_create', 'project_git_status', 'project_git_versions']);
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
