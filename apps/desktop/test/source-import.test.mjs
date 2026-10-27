// Pure command orchestration fixtures; no native install or network is exercised.
import assert from 'node:assert/strict';
import { after, test } from 'node:test';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';

const repo = fileURLToPath(new URL('../../../', import.meta.url));
await mkdir(path.join(repo, '.test-artifacts'), { recursive: true });
const output = await mkdtemp(path.join(repo, '.test-artifacts/disposable-source-import-'));
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
    assert.ok(dependency, `Missing dependency ${base}`);
    source = source.replaceAll(`${match[1]}${match[2]}${match[1]}`, JSON.stringify(pathToFileURL(await compile(dependency)).href));
  }
  await writeFile(target, source);
  return target;
}
after(async () => { await rm(output, { recursive: true, force: true }); });
const { SourceImportSession, SourceImportError } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/source-import.ts'))).href);

const sourceId = `src_${'1'.repeat(32)}`;
const otherSourceId = `src_${'2'.repeat(32)}`;
const candidateId = `install_${'3'.repeat(32)}`;
const appId = '11111111-1111-4111-8111-111111111111';
const digest = 'a'.repeat(64);
const generation = `gen_${'4'.repeat(32)}`;
const source = { sourceId, taskId: 'task_source', name: 'Disposable source', status: 'ready', phase: 'completed', url: 'https://github.com/disposable/source.git', evidencePath: '/disposable/source', contentDigest: digest, commit: 'c'.repeat(40), inspection: { kind: 'node-service-source', name: 'Disposable app', requiresBuild: false, issues: [], executionState: 'not-executed' } };
const prepared = { candidateId, appId, generation, manifest: { runtime: 'node-service', app: { id: 'org.example.disposable', name: 'Disposable app', version: '1.0.0' } }, programDigest: 'b'.repeat(64), provenance: { sourceId, sourceDigest: digest, sourceCommit: 'c'.repeat(40), node: { path: '/disposable/node', version: '24.22.0' }, npm: { path: '/disposable/npm', version: '11.19.1' }, preparedAt: 1 } };
const installed = { candidateId, appId, generation, operationOutcome: 'committed' };
const task = (type, overrides = {}) => ({ taskId: type === 'sourceAcquire' ? 'task_source' : type === 'nodeInstallPrepare' ? 'task_prepare' : 'task_apply', projectId: null, type, status: 'succeeded', final: true, phase: 'completed', result: type === 'sourceAcquire' ? source : type === 'nodeInstallPrepare' ? { state: 'prepared', candidate: prepared, installed: false } : installed, error: null, ...overrides });
const input = { url: source.url, branch: null };
function fixture(overrides = {}, options = {}) {
  const calls = []; const tasks = [];
  const defaults = {
    source_acquire: () => task('sourceAcquire'),
    node_install_prepare: () => task('nodeInstallPrepare'),
    node_install_candidate: () => ({ state: 'prepared', candidate: prepared }),
    node_install_apply: () => task('nodeInstallApply'),
    source_import_cleanup: () => ({ completed: true, sourceId, sourceRetained: false, removedPaths: ['/disposable/source'], failures: [], receiptPath: '/disposable/receipt.json' }),
  };
  const invoke = async (command, args) => {
    calls.push([command, args]);
    const run = overrides[command] ?? defaults[command];
    assert.ok(run, `Unexpected command ${command}`);
    return run(args);
  };
  const session = new SourceImportSession(invoke, { onTask: value => tasks.push(value), onChange() {}, isCurrent: () => true, pause: async () => {}, ...options });
  return { session, calls, tasks };
}
const commands = calls => calls.map(([name]) => name);

test('One import binds the acquired source, verifies the prepared candidate, and commits once', async () => {
  const { session, calls } = fixture();
  assert.deepEqual(await session.acquire(input), source);
  assert.deepEqual(await session.install(false), installed);
  assert.equal(session.phase, 'cleanup');
  assert.equal(session.sourceTaskId, 'task_source');
  assert.deepEqual(commands(calls), ['source_acquire', 'node_install_prepare', 'node_install_candidate', 'node_install_apply']);
  assert.deepEqual(calls[1][1], { input: { sourceId, expectedSourceDigest: digest, targetAppId: null } });
  assert.deepEqual(calls[3][1], { candidateId });
});

test('Download and preparation polling use only the accepted task identities', async () => {
  const reads = [];
  const { session, tasks } = fixture({
    source_acquire: () => task('sourceAcquire', { status: 'running', final: false, phase: 'downloading', result: null }),
    node_install_prepare: () => task('nodeInstallPrepare', { status: 'running', final: false, phase: 'dependencies', result: null }),
    task_get: ({ taskId }) => { reads.push(taskId); return task(taskId === 'task_source' ? 'sourceAcquire' : 'nodeInstallPrepare'); },
  });
  await session.acquire(input); await session.install(false);
  assert.deepEqual(reads, ['task_source', 'task_prepare']);
  assert.ok(tasks.some(value => value.phase === 'downloading' && !value.final));
  assert.ok(tasks.some(value => value.phase === 'dependencies' && !value.final));
});

test('A source requiring a build cannot prepare until the user approves that build', async () => {
  const buildSource = { ...source, inspection: { ...source.inspection, requiresBuild: true } };
  const { session, calls } = fixture({ source_acquire: () => task('sourceAcquire', { result: buildSource }) });
  await session.acquire(input);
  await assert.rejects(session.install(false), SourceImportError);
  assert.deepEqual(commands(calls), ['source_acquire']);
  await session.install(true);
  assert.deepEqual(calls.find(([name]) => name === 'node_install_prepare')[1], { input: { sourceId, expectedSourceDigest: digest, targetAppId: null, buildSource: true } });
});

test('Downloaded unsupported source is not silently prepared or installed', async () => {
  const unsupported = { ...source, inspection: { ...source.inspection, kind: 'needs-adaptation', issues: [{ message: 'Missing local-app.json' }] } };
  const { session, calls } = fixture({ source_acquire: () => task('sourceAcquire', { result: unsupported }) });
  await assert.rejects(session.acquire(input), SourceImportError);
  assert.equal(session.source, unsupported);
  await assert.rejects(session.install(false), SourceImportError);
  assert.deepEqual(commands(calls), ['source_acquire']);
});

test('A terminal download failure retains its exact task for scoped cleanup', async () => {
  const { session, calls } = fixture({ source_acquire: () => task('sourceAcquire', { status: 'failed', result: null, error: { code: 'SOURCE_GIT_FAILED', message: 'Disposable download failed', details: { sourceId } } }) });
  await assert.rejects(session.acquire(input), /Disposable download failed/);
  assert.equal(session.sourceTaskId, 'task_source');
  await session.cleanup();
  assert.deepEqual(calls.at(-1), ['source_import_cleanup', { sourceTaskId: 'task_source' }]);
  assert.ok(!commands(calls).includes('node_install_prepare'));
});

for (const change of [
  { appId: '22222222-2222-4222-8222-222222222222' },
  { programDigest: 'd'.repeat(64) },
  { generation: `gen_${'5'.repeat(32)}` },
  { provenance: { ...prepared.provenance, sourceId: otherSourceId } },
]) test(`Changed preparation identity prevents installation (${Object.keys(change)[0]})`, async () => {
  const { session, calls } = fixture({ node_install_candidate: () => ({ state: 'prepared', candidate: { ...prepared, ...change } }) });
  await session.acquire(input);
  await assert.rejects(session.install(false), SourceImportError);
  assert.ok(!commands(calls).includes('node_install_apply'));
});

test('A prepare result belonging to another source is never applied', async () => {
  const altered = { ...prepared, provenance: { ...prepared.provenance, sourceId: otherSourceId } };
  const { session, calls } = fixture({ node_install_prepare: () => task('nodeInstallPrepare', { result: { state: 'prepared', candidate: altered, installed: false } }) });
  await session.acquire(input);
  await assert.rejects(session.install(false), SourceImportError);
  assert.ok(!commands(calls).includes('node_install_apply'));
});

test('A committed apply with a failed finishing step remains installed and warns', async () => {
  const { session, calls } = fixture({ node_install_apply: () => task('nodeInstallApply', { status: 'failed', error: { code: 'NODE_INSTALL_RECOVERY_REQUIRED', message: 'Disposable finishing warning', details: { operationOutcome: 'committed' } } }) });
  await session.acquire(input);
  assert.deepEqual(await session.install(false), installed);
  assert.deepEqual(session.installed, installed);
  assert.ok(session.warning);
  assert.equal(commands(calls).filter(name => name === 'node_install_apply').length, 1);
});

test('An uncertain apply can reconcile a committed result without applying it again', async () => {
  let queried = 0;
  const { session, calls } = fixture({
    node_install_candidate: () => ++queried === 1 ? { state: 'prepared', candidate: prepared } : { state: 'committed', result: installed },
    node_install_apply: () => task('nodeInstallApply', { status: 'failed', result: { candidateId, operationOutcome: 'unknown' }, error: { code: 'NODE_INSTALL_RECOVERY_REQUIRED', message: 'Disposable uncertain install', details: { operationOutcome: 'unknown' } } }),
  });
  await session.acquire(input);
  await assert.rejects(session.install(false), error => error instanceof SourceImportError && error.recovery);
  assert.equal(session.installed, null);
  assert.deepEqual(await session.reconcile(), installed);
  assert.equal(commands(calls).filter(name => name === 'node_install_apply').length, 1);
});

test('Changing instance after acquisition prevents the next mutating command', async () => {
  let current = true;
  const { session, calls } = fixture({}, { isCurrent: () => current });
  await session.acquire(input);
  current = false;
  await assert.rejects(session.install(false));
  assert.deepEqual(commands(calls), ['source_acquire']);
});

test('A temporary polling failure keeps the existing task and continues to its final result', async () => {
  let reads = 0; const warnings = []; let session;
  ({ session } = fixture({
    source_acquire: () => task('sourceAcquire', { status: 'running', final: false, phase: 'downloading', result: null }),
    task_get: () => { if (++reads === 1) throw new Error('Disposable progress unavailable'); return task('sourceAcquire'); },
  }, { onChange: () => { if (session?.warning) warnings.push(session.warning); } }));
  await session.acquire(input);
  assert.equal(reads, 2);
  assert.ok(warnings.some(value => value.includes('Disposable progress unavailable')));
  assert.equal(session.warning, '');
});

test('Cancellation waits for the exact task terminal state and never starts preparation', async () => {
  let session;
  const created = fixture({
    source_acquire: () => task('sourceAcquire', { status: 'running', final: false, phase: 'downloading', result: null }),
    source_cancel: ({ taskId }) => {
      assert.equal(taskId, 'task_source');
      return task('sourceAcquire', { status: 'failed', result: null, error: { code: 'SOURCE_ACQUISITION_CANCELLED', message: 'Cancelled', details: { sourceId } } });
    },
  }, { pause: async () => { await session.cancel(); } });
  session = created.session;
  await assert.rejects(session.acquire(input), SourceImportError);
  assert.equal(session.cancelled, true);
  await session.cleanup();
  assert.deepEqual(commands(created.calls), ['source_acquire', 'source_cancel', 'source_import_cleanup']);
});

test('A late acquisition response honors a cancellation already requested by the user', async () => {
  let release;
  const accepted = new Promise(resolve => { release = resolve; });
  const { session, calls } = fixture({
    source_acquire: () => accepted,
    source_cancel: () => task('sourceAcquire', { status: 'failed', result: null, error: { code: 'SOURCE_ACQUISITION_CANCELLED', message: 'Cancelled', details: { sourceId } } }),
  });
  const acquisition = session.acquire(input);
  await session.cancel();
  release(task('sourceAcquire', { status: 'running', final: false, result: null }));
  await assert.rejects(acquisition, SourceImportError);
  assert.equal(session.cancelled, true);
  assert.deepEqual(commands(calls), ['source_acquire', 'source_cancel']);
});

test('An IPC failure during apply blocks cleanup and requires result reconciliation', async () => {
  const { session, calls } = fixture({ node_install_apply: () => { throw new Error('Disposable lost apply response'); } });
  await session.acquire(input);
  await assert.rejects(session.install(false), error => error instanceof SourceImportError && error.recovery);
  await assert.rejects(session.cleanup(), error => error instanceof SourceImportError && error.recovery);
  assert.ok(!commands(calls).includes('source_import_cleanup'));
  assert.equal(commands(calls).filter(name => name === 'node_install_apply').length, 1);
});

test('A cleanup failure remains actionable and a second cleanup uses the same task', async () => {
  let cleanups = 0;
  const { session, calls } = fixture({ source_import_cleanup: () => ({ completed: ++cleanups > 1, sourceId, sourceRetained: true, removedPaths: [], failures: cleanups === 1 ? [{ path: '/disposable/temp', error: { message: 'Disposable removal denied' } }] : [], receiptPath: '/disposable/receipt.json' }) });
  await session.acquire(input); await session.install(false);
  await assert.rejects(session.cleanup(), /Disposable removal denied/);
  assert.equal((await session.cleanup()).completed, true);
  assert.deepEqual(calls.filter(([name]) => name === 'source_import_cleanup').map(([, args]) => args), [{ sourceTaskId: 'task_source' }, { sourceTaskId: 'task_source' }]);
});

test('A mismatched task returned by polling cannot trigger installation', async () => {
  const { session, calls } = fixture({
    source_acquire: () => task('sourceAcquire', { status: 'running', final: false, result: null }),
    task_get: () => task('sourceAcquire', { taskId: 'task_other' }),
  });
  await assert.rejects(session.acquire(input), error => error instanceof SourceImportError && error.recovery);
  assert.ok(!commands(calls).includes('node_install_prepare'));
  await assert.rejects(session.cleanup());
});

test('An acquisition already cancelled by the backend is identified as cancellation', async () => {
  const { session } = fixture({ source_acquire: () => task('sourceAcquire', { status: 'failed', result: null, error: { code: 'SOURCE_ACQUISITION_CANCELLED', message: 'Cancelled', details: { sourceId } } }) });
  await assert.rejects(session.acquire(input), SourceImportError);
  assert.equal(session.cancelled, true);
});

for (const detail of ['processResourceClosed', 'candidateOwnershipConfirmed']) test(`Unconfirmed acquisition ${detail} requires recovery`, async () => {
  const { session, calls } = fixture({ source_acquire: () => task('sourceAcquire', { status: 'failed', result: null, error: { code: 'SOURCE_GIT_FAILED', message: 'Unknown acquisition outcome', details: { sourceId, [detail]: false } } }) });
  await assert.rejects(session.acquire(input), error => error instanceof SourceImportError && error.recovery);
  assert.ok(!commands(calls).includes('node_install_prepare'));
});

test('Editable source publication is bound to the installed application and reused on cleanup retries', async () => {
  const published = { published: true, sourcePath: '/disposable/projects/source', projectDirectory: '/disposable/projects' };
  const { session, calls } = fixture({ source_import_publish: () => published });
  await assert.rejects(session.publishSource(), error => error instanceof SourceImportError && error.recovery);
  assert.deepEqual(calls, []);
  await session.acquire(input); await session.install(false);
  assert.deepEqual(await session.publishSource(), published);
  assert.deepEqual(await session.publishSource(), published);
  assert.deepEqual(calls.filter(([command]) => command === 'source_import_publish'), [['source_import_publish', { sourceTaskId: 'task_source', appId }]]);
});

test('An unconfirmed publication keeps the installed result and is retried without another apply', async () => {
  let publications = 0;
  const { session, calls } = fixture({ source_import_publish: () => ++publications === 1 ? { published: false, sourcePath: '/disposable/projects/source', projectDirectory: '/disposable/projects' } : { published: true, sourcePath: '/disposable/projects/source', projectDirectory: '/disposable/projects' } });
  await session.acquire(input); await session.install(false);
  await assert.rejects(session.publishSource(), error => error instanceof SourceImportError && error.recovery);
  assert.deepEqual(session.installed, installed);
  assert.equal(session.published, null);
  await session.publishSource();
  assert.equal(publications, 2);
  assert.equal(commands(calls).filter(command => command === 'node_install_apply').length, 1);
});
