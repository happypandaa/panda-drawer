import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import ts from 'typescript';

const source = await readFile(new URL('../src/workspace-task-state.ts', import.meta.url), 'utf8');
const compiled = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext } }).outputText;
const { deriveWorkspaceTaskState: derive, mergeLifecycleTask: merge } = await import(`data:text/javascript;base64,${Buffer.from(compiled).toString('base64')}`);
const task = (taskId, type = 'workspaceMigration', extra = {}) => ({ taskId, type, projectId: null, status: 'running', final: false, phase: 'copy', result: null, error: null, ...extra });
const completed = (taskId, type = 'workspaceMigration', extra = {}) => task(taskId, type, { final: true, status: 'succeeded', phase: 'completed', ...extra });
const workspace = (extra = {}) => ({ workspaceRoot: '/disposable/workspace', activeTask: null, migrationState: 'ready', retainedRoots: [], ...extra });
const map = (...tasks) => Object.fromEntries(tasks.map(value => [value.taskId, value]));

test('same-ID final is monotonic while a different new task is accepted', () => {
  const final = completed('old');
  const current = map(final);
  assert.equal(merge(current, task('old')), current);
  const next = merge(current, task('new'));
  assert.equal(next.old, final);
  assert.equal(next.new.final, false);
  assert.equal(derive(workspace(), next).migrating, true);
});

for (const activeTask of ['migration', { taskId: 'migration' }, task('migration')]) {
  test(`matching final overrides stale active migration reference ${JSON.stringify(activeTask)}`, () => {
    const state = derive(workspace({ activeTask, migrationState: 'migrating' }), map(completed('migration')));
    assert.equal(state.migrating, false);
    assert.equal(state.paused, false);
    assert.equal(state.migrationTask.taskId, 'migration');
  });
}

test('unknown snapshot activity or running phase keeps protection', () => {
  assert.equal(derive(workspace({ activeTask: 'unknown' }), {}).migrating, true);
  assert.equal(derive(workspace({ migrationState: 'migrating' }), {}).migrating, true);
  assert.equal(derive(workspace({ migrationState: 'running' }), {}).migrating, true);
  assert.equal(derive(workspace({ migrationState: 'paused' }), {}).paused, true);
});

test('an unrelated historical final cannot clear a different active snapshot', () => {
  const state = derive(workspace({ activeTask: 'new', migrationState: 'migrating', lastTaskId: 'old' }), map(completed('old')));
  assert.equal(state.migrating, true);
  assert.equal(state.migrationTask, null);
});

test('the matching last task can resolve an old phase when no active reference exists', () => {
  assert.equal(derive(workspace({ lastTaskId: 'migration', migrationState: 'migrating' }), map(completed('migration'))).migrating, false);
  assert.equal(derive(workspace({ lastTaskId: 'migration', migrationState: 'paused' }), map(completed('migration'))).paused, false);
  assert.equal(derive(workspace({ lastTaskId: 'migration', migrationState: 'migrating' }), {}).migrating, true);
});

test('a final snapshot overrides a stale same-ID local pending task', () => {
  const state = derive(workspace({ activeTask: completed('migration'), migrationState: 'migrating' }), map(task('migration')));
  assert.equal(state.migrating, false);
  assert.equal(state.migrationTask.final, true);
});

test('a committed failure ends running notice while preserving the real paused gate', () => {
  const final = completed('migration', 'workspaceMigration', { status: 'failed', error: { code: 'WORKSPACE_REOPEN_FAILED', message: 'fixture', details: { operationOutcome: 'committed_needs_attention' } } });
  const state = derive(workspace({ activeTask: 'migration', migrationState: 'paused' }), map(final));
  assert.equal(state.migrating, false);
  assert.equal(state.paused, true);
  const unknownFailure = completed('migration', 'workspaceMigration', { status: 'failed' });
  assert.equal(derive(workspace({ activeTask: 'migration', migrationState: 'paused' }), map(unknownFailure)).paused, true);
});

test('explicit not-committed failure has already resumed the old workspace', () => {
  const final = completed('migration', 'workspaceMigration', { status: 'failed', error: { code: 'COPY_FAILED', message: 'fixture', details: { operationOutcome: 'not_committed' } } });
  const state = derive(workspace({ activeTask: 'migration', migrationState: 'paused' }), map(final));
  assert.equal(state.migrating, false);
  assert.equal(state.paused, false);
});

test('current different cleanup snapshot wins over later inserted historical final', () => {
  const active = task('new_cleanup', 'workspaceCleanup');
  const old = completed('old_cleanup', 'workspaceCleanup');
  const state = derive(workspace({ cleanupTask: active }), map(old));
  assert.equal(state.cleanupBusy, true);
  assert.equal(state.cleanupTask.taskId, 'new_cleanup');
  assert.equal(state.migrating, false);
});

test('same cleanup final clears old snapshot without hiding a different new pending task', () => {
  const oldSnapshot = task('old_cleanup', 'workspaceCleanup');
  const final = completed('old_cleanup', 'workspaceCleanup');
  let state = derive(workspace({ cleanupTask: oldSnapshot }), map(final));
  assert.equal(state.cleanupBusy, false);
  assert.equal(state.cleanupTask.final, true);
  state = derive(workspace({ cleanupTask: oldSnapshot }), map(final, task('new_cleanup', 'workspaceCleanup')));
  assert.equal(state.cleanupBusy, true);
  assert.equal(state.cleanupTask.taskId, 'new_cleanup');
});

test('same-ID snapshot final cannot be revived by an older local cleanup record', () => {
  const state = derive(workspace({ cleanupTask: completed('cleanup', 'workspaceCleanup') }), map(task('cleanup', 'workspaceCleanup')));
  assert.equal(state.cleanupBusy, false);
  assert.equal(state.cleanupTask.final, true);
});

test('unrelated completed management work creates no workspace busy notice', () => {
  const state = derive(workspace(), map(completed('import', 'import'), completed('cleanup', 'workspaceCleanup')));
  assert.equal(state.migrating, false);
  assert.equal(state.cleanupBusy, false);
  assert.equal(state.paused, false);
});
