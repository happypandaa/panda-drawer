// Real-service acceptance: prepare one disposable project, restart the desktop
// separately, then verify it. Never starts an app, migrates, repairs, or purges.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { lstat, mkdir, readFile, realpath, rename, writeFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import { dirname, isAbsolute, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const usage = `Usage: node scripts/workspace-identity-live-test.mjs prepare|verify

prepare creates exactly one project named M5 可丢弃 · 身份修复验收 · 20261019.
It saves SQL and an attachment, creates a backup, deletes to trash, and restores
that same project. The sample, backup, trash record, and task evidence are kept.
verify requires a different service instance and only reads that saved sample.
Neither phase starts the desktop, migrates/repairs a workspace, or purges data.
This HTTP/CLI check does not prove GUI behavior or an actual OS device remount.

IDENTITY_TEST_CLI (or M1_TEST_CLI): absolute path to the actual package CLI.
Default: target/release/bundle/macos/Local Projects Dev.app/Contents/MacOS/local-projects-dev
IDENTITY_TEST_ARTIFACT_DIR: evidence directory inside this repo's .test-artifacts.
Default: .test-artifacts/workspace-identity-live-2026-10-19

prepare refuses an existing state file or a same-named project; it never retries
a mutation or creates a second sample after an interrupted/failed preparation.
Use the saved project/task IDs for manual inspection if a phase fails.`;

const phase = process.argv[2];
if (['--help', '-h'].includes(phase)) {
  console.log(usage);
  process.exit(0);
}
assert.ok(process.argv.length === 3 && ['prepare', 'verify'].includes(phase), usage);
assert.equal(process.platform, 'darwin', 'This probe targets the fixed macOS development installation');
const repository = resolve(fileURLToPath(new URL('..', import.meta.url)));
const artifactBase = join(repository, '.test-artifacts');
const evidence = resolve(process.env.IDENTITY_TEST_ARTIFACT_DIR || join(artifactBase, 'workspace-identity-live-2026-10-19'));
const evidenceRelative = relative(artifactBase, evidence);
assert.ok(evidenceRelative && !isAbsolute(evidenceRelative) && !evidenceRelative.startsWith('..'), 'Evidence must be inside .test-artifacts');
const cliPath = process.env.IDENTITY_TEST_CLI || process.env.M1_TEST_CLI
  || join(repository, 'target/release/bundle/macos/Local Projects Dev.app/Contents/MacOS/local-projects-dev');
assert.ok(isAbsolute(cliPath), 'Choose an absolute CLI path');
const dataRoot = join(homedir(), 'Library/Application Support/dev.localprojects.platform.m0');
const identifier = 'dev.localprojects.platform.m0';
const sampleName = 'M5 可丢弃 · 身份修复验收 · 20261019';
const statePath = join(evidence, 'state.json');
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const readJson = async path => JSON.parse(await readFile(path, 'utf8'));
const now = () => new Date().toISOString();
let state;

function cli(args, input) {
  const result = spawnSync(cliPath, [...args, '--json'], {
    input: input === undefined ? undefined : JSON.stringify(input),
    encoding: 'utf8', maxBuffer: 16_000_000, timeout: 40_000,
  });
  assert.ifError(result.error);
  let wire;
  try { wire = JSON.parse(result.stdout); }
  catch { throw new Error(`CLI returned no JSON for ${args.join(' ')}: ${result.stderr}`); }
  assert.equal(result.status, 0, JSON.stringify(wire));
  assert.equal(wire.ok, true, JSON.stringify(wire));
  return wire.data;
}
async function saveJson(path, value, exclusive = false) {
  const bytes = `${JSON.stringify(value, null, 2)}\n`;
  if (exclusive) return writeFile(path, bytes, { mode: 0o600, flag: 'wx' });
  const temporary = `${path}.${randomUUID()}.tmp`;
  await writeFile(temporary, bytes, { mode: 0o600, flag: 'wx' });
  await rename(temporary, path);
}
const checkpoint = async step => {
  state.step = step;
  state.updatedAt = now();
  await saveJson(statePath, state);
};
async function currentRuntime() {
  const runtime = cli(['service', 'status']).runtime;
  const settings = await readJson(join(dataRoot, 'settings.json'));
  const discovery = await readJson(join(dataRoot, 'runtime/connection.json'));
  for (const value of [runtime, settings, discovery]) {
    assert.equal(value.identifier, identifier);
    assert.equal(value.environment, 'development');
    assert.equal(value.workspaceRoot, settings.workspaceRoot);
  }
  assert.equal(runtime.dataRoot, dataRoot);
  assert.ok(isAbsolute(runtime.workspaceRoot));
  assert.equal(runtime.instanceId, discovery.instanceId);
  const base = new URL(runtime.baseUrl);
  assert.equal(base.protocol, 'http:');
  assert.equal(base.hostname, '127.0.0.1');
  assert.equal(base.pathname, '/');
  return { ...runtime, pid: discovery.pid };
}
function projectMembership(sampleId) {
  return cli(['project', 'list']).filter(project => project.id !== sampleId)
    .map(({ id, name, projectRoot }) => ({ id, name, projectRoot })).sort((a, b) => a.id.localeCompare(b.id));
}
function trashMembership(sampleTrashId) {
  return cli(['trash', 'list']).filter(item => item.trashId !== sampleTrashId)
    .map(({ trashId, projectId, name, state: status }) => ({ trashId, projectId, name, state: status }))
    .sort((a, b) => a.trashId.localeCompare(b.trashId));
}
function assertMembership() {
  assert.deepEqual(projectMembership(state.projectId), state.beforeProjects, 'Pre-existing project membership changed');
  assert.deepEqual(trashMembership(state.trashId), state.beforeTrash, 'Pre-existing trash membership changed');
}
function sampleInfo() {
  assert.match(state.projectId, /^prj_[a-f0-9]{32}$/);
  const info = cli(['project', 'info', '--project', state.projectId]);
  assert.equal(info.id, state.projectId);
  assert.equal(info.name, sampleName);
  assert.equal(info.projectRoot, join(state.workspaceRoot, 'projects', state.projectId));
  assert.equal(info.state, 'ready');
  assert.equal(info.maintenance.mode, 'ready');
  assert.equal(info.maintenance.recoveryRequired, false);
  return info;
}
// Accept both historical wire shapes when inspecting records, but require a
// stable UUID on every newly created registration/trash/operation identity.
function identity(value, label, requireStable = true) {
  const parsed = Array.isArray(value) ? { device: value[0], inode: value[1] } : value;
  assert.ok(parsed && typeof parsed === 'object', `${label}: identity missing`);
  assert.ok(Number.isSafeInteger(parsed.device) && parsed.device >= 0, `${label}: unsafe device`);
  assert.ok(Number.isSafeInteger(parsed.inode) && parsed.inode > 0, `${label}: unsafe inode`);
  if (requireStable) {
    assert.ok(!Array.isArray(value), `${label}: new identity must use the stable object format`);
    assert.match(parsed.volumeUuid, /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i, `${label}: stable volume UUID missing`);
    assert.notEqual(parsed.volumeUuid.toLowerCase(), '00000000-0000-0000-0000-000000000000');
  }
  return { device: parsed.device, inode: parsed.inode, volumeUuid: parsed.volumeUuid?.toLowerCase() ?? null };
}
async function matchesCurrentInode(expected, path, directory) {
  const actual = await lstat(path, { bigint: true });
  assert.equal(actual.isSymbolicLink(), false, `Unexpected symlink: ${path}`);
  assert.ok(directory ? actual.isDirectory() : actual.isFile(), `Unexpected object: ${path}`);
  assert.equal(BigInt(expected.inode), actual.ino, `Directory/file identity changed: ${path}`);
  // Device is evidence only here: the live backend independently validates the
  // persistent volume UUID, and st_dev may legitimately differ after reboot.
  return { device: actual.dev.toString(), inode: actual.ino.toString() };
}
async function registration(info) {
  const path = join(state.workspaceRoot, '.platform/project-state', `${state.projectId}.json`);
  const bytes = await readFile(path);
  const record = JSON.parse(bytes);
  assert.equal(record.id, state.projectId);
  assert.equal(record.projectRoot, state.projectRoot);
  assert.equal(record.databasePath, info.manifest.database);
  assert.equal(record.documentEpoch, info.documentEpoch);
  const root = identity(record.rootIdentity, 'registration.rootIdentity');
  const database = identity(record.databaseIdentity, 'registration.databaseIdentity');
  assert.equal(root.volumeUuid, database.volumeUuid, 'Fixture database must share its project volume');
  return { path, sha256: hash(bytes), root, database,
    observedRoot: await matchesCurrentInode(root, state.projectRoot, true),
    observedDatabase: await matchesCurrentInode(database, join(state.projectRoot, record.databasePath), false),
    documentEpoch: record.documentEpoch };
}
async function document(runtime) {
  const launch = cli(['project', 'url', '--project', state.projectId]);
  const url = new URL(launch.launchUrl);
  assert.equal(url.origin, new URL(runtime.baseUrl).origin);
  assert.equal(new URL(launch.url).origin, url.origin);
  assert.equal(new URL(launch.url).pathname, `/p/${state.projectId}/`);
  const ticket = new URLSearchParams(url.hash.slice(1)).get('ticket');
  assert.ok(ticket);
  const response = await fetch(`${runtime.baseUrl}/_platform/session`, {
    method: 'POST', headers: { Origin: runtime.baseUrl, 'Content-Type': 'application/json' },
    body: JSON.stringify({ ticket }), signal: AbortSignal.timeout(10_000),
  });
  assert.equal(response.status, 200, await response.clone().text());
  const cookie = response.headers.get('set-cookie')?.split(';')[0];
  assert.ok(cookie);
  const page = await fetch(launch.url, { headers: { Cookie: cookie }, signal: AbortSignal.timeout(10_000) });
  assert.equal(page.status, 200, await page.clone().text());
  const epoch = /name="platform-document-epoch" content="([^"]+)"/.exec(await page.text())?.[1];
  assert.ok(epoch);
  // Cookies/tickets are deliberately never written to evidence.
  return { cookie, epoch };
}
function business(runtime, doc, route, method = 'GET', body) {
  const json = body !== undefined && !Buffer.isBuffer(body);
  return fetch(`${runtime.baseUrl}/p/${state.projectId}/api/v1/${route}`, {
    method, headers: { Cookie: doc.cookie, Origin: runtime.baseUrl, 'X-Platform-Document-Epoch': doc.epoch,
      ...(json ? { 'Content-Type': 'application/json' } : {}) },
    body: body === undefined ? undefined : json ? JSON.stringify(body) : body,
    signal: AbortSignal.timeout(10_000),
  });
}
async function browserSql(runtime, doc, sql, params = []) {
  const response = await business(runtime, doc, 'db/execute', 'POST', { sql, params });
  assert.equal(response.status, 200, await response.clone().text());
  const wire = await response.json();
  assert.equal(wire.ok, true, JSON.stringify(wire));
  return wire.data;
}
async function task(operation, args, input) {
  await checkpoint(`submitting:${operation}`);
  const submitted = cli(args, input);
  assert.match(submitted.taskId, /^task_[a-f0-9]{32}$/);
  const record = { operation, taskId: submitted.taskId, submittedAt: now() };
  state.tasks.push(record);
  await checkpoint(`waiting:${operation}`);
  const deadline = Date.now() + 120_000;
  let completed;
  do {
    assert.ok(Date.now() < deadline, `Task timed out; inspect ${submitted.taskId}; do not resubmit`);
    completed = cli(['task', 'wait', '--task', submitted.taskId, '--timeout', '10']);
  } while (!completed.final);
  record.completed = completed;
  await checkpoint(`completed:${operation}`);
  assert.equal(completed.projectId, state.projectId);
  assert.equal(completed.status, 'succeeded', JSON.stringify(completed));
  return completed.result;
}
async function operationIdentities() {
  const result = [];
  for (const task of state.tasks.filter(task => ['delete', 'trashRestore'].includes(task.operation))) {
    const path = join(state.workspaceRoot, '.platform/operations', task.taskId, 'lifecycle.json');
    const bytes = await readFile(path);
    const record = JSON.parse(bytes);
    assert.equal(record.taskId, task.taskId);
    assert.equal(record.projectId, state.projectId);
    assert.equal(record.trashId, state.trashId);
    assert.equal(record.kind, task.operation === 'delete' ? 'delete' : 'trashRestore');
    assert.equal(record.committed, true);
    assert.equal(record.resolved, true);
    const directory = identity(record.directoryIdentity, `${task.operation}.directoryIdentity`);
    result.push({ taskId: task.taskId, path, sha256: hash(bytes), directory });
  }
  assert.equal(result.length, 2);
  return result;
}
async function sampleEvidence(runtime) {
  const info = sampleInfo();
  const doc = await document(runtime);
  const query = 'SELECT id, business_uuid, value FROM identity_repair_probe ORDER BY id';
  assert.deepEqual((await browserSql(runtime, doc, query)).rows, [state.row]);
  assert.deepEqual(cli(['db', 'execute', '--project', state.projectId, '--input', '-'], { sql: query }).rows, [state.row]);
  const attachment = await business(runtime, doc, `file?path=${encodeURIComponent(state.attachment.path)}`);
  assert.equal(attachment.status, 200, await attachment.clone().text());
  assert.equal(hash(Buffer.from(await attachment.arrayBuffer())), state.attachment.sha256);
  const backup = cli(['backup', 'info', '--project', state.projectId, '--backup', state.backupId]);
  assert.equal(backup.manifest.projectId, state.projectId);
  assert.equal(backup.manifest.backupId, state.backupId);
  const archive = join(state.workspaceRoot, 'backups', state.projectId, `${state.backupId}.zip`);
  assert.equal(hash(await readFile(archive)), state.backupSha256);
  const trashPath = join(state.workspaceRoot, '.platform/trash', state.trashId, 'record.json');
  const trashBytes = await readFile(trashPath);
  const trashRecord = JSON.parse(trashBytes);
  assert.equal(trashRecord.trashId, state.trashId);
  assert.equal(trashRecord.projectId, state.projectId);
  assert.equal(trashRecord.state, 'restored');
  const trashRoot = identity(trashRecord.rootIdentity, 'trash.rootIdentity');
  const registered = await registration(info);
  assert.equal(trashRoot.volumeUuid, registered.root.volumeUuid);
  assert.equal(trashRoot.inode, registered.root.inode);
  const controlPath = join(state.workspaceRoot, '.platform/project-state/maintenance', `${state.projectId}.json`);
  const control = await readJson(controlPath);
  assert.equal(control.mode, 'ready');
  assert.equal(control.recoveryRequired, false);
  const operations = await operationIdentities();
  for (const operation of operations) {
    assert.equal(operation.directory.volumeUuid, registered.root.volumeUuid);
    assert.equal(operation.directory.inode, registered.root.inode);
  }
  assertMembership();
  return { runtime, registration: registered, row: state.row, attachment: state.attachment,
    backup: { backupId: state.backupId, path: archive, sha256: state.backupSha256 },
    trash: { trashId: state.trashId, path: trashPath, sha256: hash(trashBytes), state: trashRecord.state, root: trashRoot },
    operations, control: { path: controlPath, mode: control.mode, recoveryRequired: control.recoveryRequired },
    protectionScope: 'Existing project/trash membership only; no original project writes or source/DB integrity claim',
    limitations: ['CLI and loopback HTTP, not GUI', 'Different instance proves service restart, not OS reboot or a changed device number'] };
}

try {
  await mkdir(evidence, { recursive: true, mode: 0o700 });
  const canonicalBase = await realpath(artifactBase);
  const canonicalEvidence = await realpath(evidence);
  const canonicalRelative = relative(canonicalBase, canonicalEvidence);
  assert.ok(canonicalRelative && !isAbsolute(canonicalRelative) && !canonicalRelative.startsWith('..'), 'Evidence symlink leaves .test-artifacts');
  const runtime = await currentRuntime();
  if (phase === 'prepare') {
    assert.ok(!projectMembership().some(project => project.name === sampleName), 'Same-named sample already exists; do not create another');
    assert.ok(!trashMembership().some(item => item.name === sampleName), 'Same-named sample is retained in trash; inspect it instead');
    state = { schemaVersion: 1, test: 'workspace-identity-live', status: 'preparing', startedAt: now(),
      workspaceRoot: runtime.workspaceRoot, cliPath, beforeRuntime: runtime,
      beforeProjects: projectMembership(), beforeTrash: trashMembership(), tasks: [],
      row: { id: 73, business_uuid: `identity-repair-${randomUUID()}`, value: '本轮可丢弃样本：保持业务主键与附件' } };
    // This exclusive reservation survives failures, including an ambiguous
    // create response. A second prepare cannot silently create another sample.
    await saveJson(statePath, state, true);
    const created = cli(['project', 'create', '--input', '-'], { name: sampleName,
      description: '20261019 身份修复真实验收专用可丢弃项目；保留供重启核验，不触碰现有开发项目。' });
    state.projectId = created.id;
    state.projectRoot = created.projectRoot;
    await checkpoint('created');
    const info = sampleInfo();
    state.createdRegistration = await registration(info);
    const doc = await document(runtime);
    await browserSql(runtime, doc, 'CREATE TABLE identity_repair_probe(id INTEGER PRIMARY KEY, business_uuid TEXT NOT NULL UNIQUE, value TEXT NOT NULL)');
    await browserSql(runtime, doc, 'INSERT INTO identity_repair_probe VALUES (?, ?, ?)', [state.row.id, state.row.business_uuid, state.row.value]);
    const content = Buffer.from(`Identity acceptance attachment\n${state.row.business_uuid}\n仅本轮可丢弃样本\n`);
    state.attachment = { path: 'identity-repair/acceptance-20261019.txt', bytes: content.length, sha256: hash(content) };
    await checkpoint('saving-business-data');
    const uploaded = await business(runtime, doc, `file?path=${encodeURIComponent(state.attachment.path)}`, 'PUT', content);
    assert.equal(uploaded.status, 200, await uploaded.clone().text());
    const saved = await business(runtime, doc, `file?path=${encodeURIComponent(state.attachment.path)}`);
    assert.equal(saved.status, 200, await saved.clone().text());
    assert.equal(hash(Buffer.from(await saved.arrayBuffer())), state.attachment.sha256);
    const snapshot = await task('backup', ['backup', 'create', '--project', state.projectId]);
    state.backupId = snapshot.backupId;
    assert.match(state.backupId, /^backup_[a-f0-9]{32}$/);
    state.backupSha256 = hash(await readFile(join(state.workspaceRoot, 'backups', state.projectId, `${state.backupId}.zip`)));
    await checkpoint('backed-up');
    const deleted = await task('delete', ['project', 'delete', '--project', state.projectId]);
    state.trashId = deleted.trashId;
    assert.match(state.trashId, /^trash_[a-f0-9]{32}$/);
    await checkpoint('deleted-to-trash');
    assert.ok(!cli(['project', 'list']).some(project => project.id === state.projectId));
    const retained = cli(['trash', 'info', '--trash', state.trashId]);
    assert.equal(retained.projectId, state.projectId);
    assert.equal(retained.state, 'trashed');
    assert.equal(retained.restoreAvailable, true);
    const trashRecord = await readJson(join(state.workspaceRoot, '.platform/trash', state.trashId, 'record.json'));
    state.trashedIdentity = identity(trashRecord.rootIdentity, 'new trash.rootIdentity');
    await matchesCurrentInode(state.trashedIdentity, join(state.workspaceRoot, '.platform/trash', state.trashId, 'project'), true);
    assertMembership();
    await task('trashRestore', ['trash', 'restore', '--trash', state.trashId]);
    state.prepared = await sampleEvidence(runtime);
    assert.notEqual(state.createdRegistration.documentEpoch, state.prepared.registration.documentEpoch);
    for (const kind of ['root', 'database']) {
      assert.equal(state.createdRegistration[kind].volumeUuid, state.prepared.registration[kind].volumeUuid);
      assert.equal(state.createdRegistration[kind].inode, state.prepared.registration[kind].inode);
    }
    state.status = 'prepared';
    state.preparedAt = now();
    await checkpoint('ready-for-restart');
    await saveJson(join(evidence, 'prepare.json'), { ok: true, phase, projectId: state.projectId, projectRoot: state.projectRoot,
      ...state.prepared, tasks: state.tasks, retainedForVerification: true });
    console.log(`PASS prepare: ${state.projectId}; sample and backup retained. Restart separately, then run verify. Evidence: ${evidence}`);
  } else {
    state = await readJson(statePath);
    assert.equal(state.schemaVersion, 1);
    assert.equal(state.test, 'workspace-identity-live');
    assert.equal(state.status, 'prepared', 'Preparation did not finish; inspect state/tasks without resubmitting mutations');
    assert.equal(runtime.workspaceRoot, state.workspaceRoot, 'This test never migrates the workspace');
    assert.notEqual(runtime.instanceId, state.beforeRuntime.instanceId, 'Restart the service separately before verify');
    const verified = await sampleEvidence(runtime);
    assert.equal(verified.registration.sha256, state.prepared.registration.sha256, 'New sample registration was unexpectedly rewritten');
    assert.equal(verified.trash.sha256, state.prepared.trash.sha256, 'Restored trash record changed');
    assert.deepEqual(verified.operations, state.prepared.operations, 'Completed lifecycle journals changed on restart');
    for (const item of state.tasks) {
      const persisted = cli(['task', 'get', '--task', item.taskId]);
      assert.equal(persisted.status, 'succeeded');
      assert.equal(persisted.projectId, state.projectId);
      assert.deepEqual(persisted.result, item.completed.result);
    }
    const path = join(evidence, `verify-${runtime.instanceId}.json`);
    await saveJson(path, { ok: true, phase, verifiedAt: now(), projectId: state.projectId, projectRoot: state.projectRoot,
      beforeInstanceId: state.beforeRuntime.instanceId, afterInstanceId: runtime.instanceId, ...verified,
      tasks: state.tasks.map(({ operation, taskId }) => ({ operation, taskId })), retainedForInspection: true });
    console.log(`PASS verify: ${state.projectId}; SQL, attachment, backup, identities and completed tasks survived service restart. Evidence: ${path}`);
  }
} catch (error) {
  // Only private evidence is written on failure; no repair or data cleanup runs.
  await saveJson(join(evidence, `${phase}-failure-${Date.now()}.json`), {
    ok: false, phase, at: now(), projectId: state?.projectId ?? null, step: state?.step ?? 'preflight',
    taskIds: state?.tasks?.map(task => task.taskId) ?? [], message: error.message,
    sampleRetained: Boolean(state?.projectId), action: 'Inspect saved IDs; do not repeat prepare or purge automatically',
  }).catch(() => {});
  throw error;
}
