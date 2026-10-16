// Non-destructive M3 acceptance: all mutations are confined to one newly created,
// explicitly disposable project. Existing development projects are never edited.
import assert from 'node:assert/strict';
import { mkdir, readFile, rename, rm, stat, symlink, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import {
  archivePath, backup, backupIdOf, begin, business, cli, corruptArchivedDatabase, createProject, end,
  expectBusinessError, info, inspectArchive, invoke, maintenance, restore,
  runtime, saveReport, session, sha256, sql, succeeded, waitTask,
} from './m3-test-utils.mjs';

const current = runtime();
const project = createProject('备份恢复边界验证');
const { id, projectRoot: root } = project;
console.log(`Created disposable M3 probe ${id} at ${root}`);
const taskRecords = [];
const checks = [];
const check = label => { checks.push(label); console.log(`PASS ${label}`); };
const read = relative => readFile(join(root, relative));
const write = async (relative, bytes) => {
  const path = join(root, relative);
  await mkdir(join(path, '..'), { recursive: true });
  await writeFile(path, bytes);
};
const digest = async relative => sha256(await read(relative));
const build = maintenanceId => waitTask(cli(['project', 'build', '--project', id,
  ...(maintenanceId ? ['--maintenance', maintenanceId] : [])]), taskRecords);
function failedSubmission(args, input) {
  const wire = invoke(args, input, true);
  if (!wire.ok) return wire.error;
  const task = waitTask(wire.data, taskRecords);
  assert.equal(task.status, 'failed', JSON.stringify(task));
  return task.error;
}
async function rejectedBackup() {
  const task = backup(id, taskRecords);
  assert.equal(task.status, 'failed', JSON.stringify(task));
  assert.equal(maintenance(id).mode, 'ready');
  return task;
}

let document = await session(current, id);
assert.equal(info(id).buildInfo.origin, 'bundledTemplate');
sql(id, 'CREATE TABLE m3_probe (id INTEGER PRIMARY KEY, value TEXT NOT NULL)');
sql(id, 'INSERT INTO m3_probe VALUES (?, ?)', [1, 'baseline-row']);
const walBytesBeforeSnapshot = (await stat(join(root, 'data/main.sqlite-wal'))).size;
assert.ok(walBytesBeforeSnapshot > 0, 'The SQLite snapshot must include committed WAL data');
const originalManifest = JSON.parse(await read('project.json'));
originalManifest.description = 'M3 snapshot metadata';
originalManifest.icon = 'meta/icon.png';
await write('project.json', JSON.stringify(originalManifest, null, 2));
const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/lS0AAAAASUVORK5CYII=', 'base64');
await write('meta/icon.png', png);
await write('meta/private-note.txt', 'persistent metadata');
await write('.hidden-m3/notes.txt', 'hidden persistent project notes');
for (const relative of ['node_modules/keep.txt', '.git/keep.txt', '.platform/keep.txt', 'nested/.hidden/keep.txt']) {
  const response = await business(current, document, `file?path=${encodeURIComponent(relative)}`, 'PUT', Buffer.from(`attachment ${relative}`));
  assert.equal(response.status, 200, await response.clone().text());
}
await write('app/node_modules/m3-must-exclude.txt', 'only the known dependency root is excluded');
await write('.git/m3-must-exclude.txt', 'only project VCS metadata is excluded');
const preservedPaths = [
  'project.json', 'README.md', 'AGENTS.md', 'CLAUDE.md', '.agents/skills/local-projects-app/SKILL.md',
  'app/src/App.tsx', 'app/package-lock.json', 'app/vendor/local-projects-sdk-0.1.0.tgz',
  'meta/icon.png', 'meta/private-note.txt', '.hidden-m3/notes.txt', 'dist/index.html',
  ...['node_modules/keep.txt', '.git/keep.txt', '.platform/keep.txt', 'nested/.hidden/keep.txt'].map(path => `data/attachments/${path}`),
];
const hashes = Object.fromEntries(await Promise.all(preservedPaths.map(async path => [path, await digest(path)])));
const starterTask = backup(id, taskRecords);
succeeded(starterTask);
const starterBackupId = backupIdOf(starterTask);
const archive = inspectArchive(archivePath(id, starterBackupId));
const archived = new Map(archive.entries.map(entry => [entry.path, entry]));
for (const [path, hash] of Object.entries(hashes)) assert.equal(archived.get(`project/${path}`)?.sha256, hash, path);
assert.ok(archived.has('project/data/main.sqlite'));
for (const path of archived.keys()) {
  assert.ok(!path.startsWith('project/.platform/'), path);
  assert.ok(!path.startsWith('project/app/node_modules/'), path);
  assert.ok(!path.startsWith('project/.git/'), path);
  assert.ok(!path.endsWith('main.sqlite-wal') && !path.endsWith('main.sqlite-shm'), path);
}
assert.ok(archive.manifest.buildProvenance, 'Starter provenance must be portable outside excluded .platform');
assert.ok(!JSON.stringify(archive.manifest.buildProvenance).includes(root), 'Portable provenance excludes local absolute paths');
assert.equal(maintenance(id).mode, 'ready');
assert.equal(info(id).documentEpoch, document.epoch, 'Ordinary backup must not advance epoch');
const details = cli(['backup', 'info', '--project', id, '--backup', starterBackupId]);
const listed = cli(['backup', 'list', '--project', id]);
assert.ok(JSON.stringify(listed).includes(starterBackupId));
check('starter archive preserves source, SDK, skill, icon, hidden files, SQLite and reserved-name attachments');

// A required symlink is an explicit backup error; no successful archive is published.
await symlink('meta/private-note.txt', join(root, 'm3-required-link'));
const badBackup = await rejectedBackup();
assert.ok(badBackup.error);
await rm(join(root, 'm3-required-link'));
assert.equal((await business(current, document, 'db/execute', 'POST', { sql: 'SELECT value FROM m3_probe' })).status, 200);
check('failed ordinary backup releases transient freeze and preserves original data');

// Make baseline preparation fail, then retry the same persistent maintenance ID.
await symlink('meta/private-note.txt', join(root, 'm3-baseline-link'));
const failedBaseline = begin(id, taskRecords);
assert.equal(failedBaseline.status, 'failed', JSON.stringify(failedBaseline));
let status = maintenance(id);
assert.equal(status.mode, 'maintenance');
assert.equal(status.readyForChanges, false);
const maintenanceId = status.maintenanceId;
const forbidden = invoke(['db', 'execute', '--project', id, '--input', '-'], {
  sql: 'INSERT INTO m3_probe VALUES (99, ?)', params: ['must-not-write'], maintenanceId,
}, true);
assert.equal(forbidden.ok, false);
await expectBusinessError(await business(current, document, 'db/execute', 'POST', { sql: 'SELECT 1' }), 'PROJECT_MAINTENANCE');
await rm(join(root, 'm3-baseline-link'));
succeeded(begin(id, taskRecords, maintenanceId));
status = maintenance(id);
assert.equal(status.maintenanceId, maintenanceId);
assert.equal(status.readyForChanges, true);
assert.ok(status.baselineBackupId);
check('baseline failure persists maintenance, blocks changes and retries under the same maintenance ID');

// Metadata-only maintenance does not require Node or a build.
await write('meta/private-note.txt', 'changed metadata');
await write('README.md', Buffer.concat([await read('README.md'), Buffer.from('\nM3 metadata-only edit.\n')]));
sql(id, 'UPDATE m3_probe SET value=? WHERE id=1', ['after-maintenance'], maintenanceId);
succeeded(end(id, maintenanceId, taskRecords));
const afterEnd = await session(current, id);
assert.notEqual(afterEnd.epoch, document.epoch);
await expectBusinessError(await business(current, { ...afterEnd, epoch: document.epoch }, 'db/execute', 'POST', {
  sql: 'INSERT INTO m3_probe VALUES (98, ?)', params: ['old-document'],
}), 'PROJECT_RELOAD_REQUIRED');
document = afterEnd;
check('metadata-only maintenance ends without a build; a new cookie cannot revive an old document');

// Restore the starter while the selected Node path is deliberately unavailable.
// Always restore the previous user selection in finally.
const environment = cli(['env', 'doctor']).buildEnvironment;
assert.equal(environment.ready, true, 'This probe needs Node only for the later projectBuild case');
const testNode = join(root, '.platform/m3-test-node');
await symlink(environment.node.path, testNode);
try {
  cli(['env', 'node', 'set', '--path', testNode]);
  await rename(testNode, `${testNode}.offline`);
  assert.equal(cli(['env', 'doctor']).buildEnvironment.ready, false);
  await write('app/src/App.tsx', Buffer.concat([await read('app/src/App.tsx'), Buffer.from('\n// post-snapshot source modification\n')]));
  await write('meta/icon.png', Buffer.from('intentionally changed icon'));
  await write('data/attachments/node_modules/keep.txt', 'changed attachment');
  sql(id, 'INSERT INTO m3_probe VALUES (?, ?)', [2, 'post-snapshot-row']);
  const beforeRestoreSource = await digest('app/src/App.tsx');
  const restored = restore(id, starterBackupId, taskRecords);
  const restoreResult = succeeded(restored);
  assert.equal(restoreResult.originalRetained, true);
  assert.equal(sha256(await readFile(join(restoreResult.originalPath, 'app/src/App.tsx'))), beforeRestoreSource);
  assert.equal(await readFile(join(restoreResult.originalPath, 'data/attachments/node_modules/keep.txt'), 'utf8'), 'changed attachment');
  status = maintenance(id);
  assert.equal(status.mode, 'maintenance');
  assert.equal(status.readyForChanges, true);
  assert.ok(JSON.stringify(restoreResult).includes(starterBackupId));
  for (const [path, hash] of Object.entries(hashes)) assert.equal(await digest(path), hash, path);
  assert.deepEqual(sql(id, 'SELECT id, value FROM m3_probe ORDER BY id', [], status.maintenanceId).rows, [{ id: 1, value: 'baseline-row' }]);
  assert.equal(info(id).buildInfo.origin, 'bundledTemplate');
  assert.equal(info(id).buildInfo.sourceState, 'matched');
  succeeded(end(id, status.maintenanceId, taskRecords));
  const fresh = await session(current, id);
  assert.notEqual(fresh.epoch, document.epoch);
  await expectBusinessError(await business(current, { ...fresh, epoch: document.epoch }, 'db/execute', 'POST', { sql: 'SELECT 1' }), 'PROJECT_RELOAD_REQUIRED');
  document = fresh;
  check('starter restoration without Node restores all files and rows and rebuilds portable source identity');
} finally {
  if (environment.configuredNodePath) cli(['env', 'node', 'set', '--path', environment.configuredNodePath]);
  else cli(['env', 'node', 'clear']);
  // The original root may have moved into protected operation storage; do not remove that original.
  await rm(testNode, { force: true });
  await rm(`${testNode}.offline`, { force: true });
}

// A real user build must also round-trip without relying on its original task record.
await rm(join(root, 'app/node_modules'), { recursive: true, force: true }); // only the new disposable project's dependencies
succeeded(build());
assert.equal(info(id).buildInfo.origin, 'projectBuild');
const userBuildHtml = await digest('dist/index.html');
const builtTask = backup(id, taskRecords);
succeeded(builtTask);
const builtBackupId = backupIdOf(builtTask);
const builtArchive = inspectArchive(archivePath(id, builtBackupId));
assert.ok(builtArchive.manifest.buildProvenance);
assert.ok(!JSON.stringify(builtArchive.manifest.buildProvenance).includes(root));
await write('app/src/App.tsx', Buffer.concat([await read('app/src/App.tsx'), Buffer.from('\n// ready source differs from published webpage\n')]));
assert.equal(info(id).buildInfo.sourceState, 'modified');
const modifiedTask = backup(id, taskRecords);
succeeded(modifiedTask);
const modifiedBackupId = backupIdOf(modifiedTask);
await write('app/src/App.tsx', Buffer.concat([await read('app/src/App.tsx'), Buffer.from('\n// another later edit\n')]));
succeeded(restore(id, modifiedBackupId, taskRecords));
status = maintenance(id);
succeeded(end(id, status.maintenanceId, taskRecords));
assert.equal(info(id).buildInfo.sourceState, 'modified');
assert.equal(await digest('dist/index.html'), userBuildHtml);
check('ready snapshot with modified source retains its valid projectBuild webpage without forced rebuild');

// Source modified during maintenance must not silently clear the required build.
succeeded(begin(id, taskRecords));
status = maintenance(id);
await write('app/src/App.tsx', Buffer.concat([await read('app/src/App.tsx'), Buffer.from('\n// maintenance requires a matching build\n')]));
const endError = failedSubmission(['maintenance', 'end', '--project', id, '--input', '-'], { maintenanceId: status.maintenanceId });
assert.ok(endError);
assert.equal(maintenance(id).mode, 'maintenance');
const pendingTask = backup(id, taskRecords, status.maintenanceId);
succeeded(pendingTask);
const pendingBackupId = backupIdOf(pendingTask);
succeeded(restore(id, pendingBackupId, taskRecords, status.maintenanceId));
status = maintenance(id);
assert.ok(failedSubmission(['maintenance', 'end', '--project', id, '--input', '-'], { maintenanceId: status.maintenanceId }));
succeeded(build(status.maintenanceId));
succeeded(end(id, status.maintenanceId, taskRecords));
assert.equal(info(id).buildInfo.sourceState, 'matched');
check('maintenance input changes require a successful matching build, including after restoring their snapshot');

// Corrupt only this project's own archive and always put its original bytes back.
const badPath = archivePath(id, builtBackupId);
const validArchiveBytes = await readFile(badPath);
const beforeBadRoot = await digest('app/src/App.tsx');
const beforeBadEpoch = info(id).documentEpoch;
try {
  await writeFile(badPath, Buffer.from('not a ZIP archive'));
  const badRestore = restore(id, builtBackupId, taskRecords);
  assert.equal(badRestore.status, 'failed', JSON.stringify(badRestore));
  assert.equal(maintenance(id).mode, 'ready');
  assert.equal(info(id).documentEpoch, beforeBadEpoch);
  assert.equal(await digest('app/src/App.tsx'), beforeBadRoot);
} finally { await writeFile(badPath, validArchiveBytes); }
check('invalid ZIP fails before maintenance and preserves current root and document identity');

try {
  corruptArchivedDatabase(badPath);
  const badDatabase = restore(id, builtBackupId, taskRecords);
  assert.equal(badDatabase.status, 'failed', JSON.stringify(badDatabase));
  assert.equal(maintenance(id).mode, 'ready');
  assert.equal(info(id).documentEpoch, beforeBadEpoch);
  assert.equal(await digest('app/src/App.tsx'), beforeBadRoot);
} finally { await writeFile(badPath, validArchiveBytes); }
check('valid ZIP with matching checksums but corrupt SQLite is rejected without replacing the project');

// Managed mkdir repairs permissions, so chmod is not a reliable failure injection.
// Retain this new project's complete backup directory and place a regular-file
// blocker at its expected directory path. No pre-existing project's path is touched.
const backupDirectory = join(current.workspaceRoot, 'backups', id);
const heldBackupDirectory = `${backupDirectory}.m3-retained`;
await rename(backupDirectory, heldBackupDirectory);
try {
  await writeFile(backupDirectory, 'M3 disposable archive destination failure probe');
  await rejectedBackup();
  assert.equal(await digest('app/src/App.tsx'), beforeBadRoot);
} finally {
  await rm(backupDirectory, { force: true });
  await rename(heldBackupDirectory, backupDirectory);
}
check('archive publication write failure reports failure and keeps the original project usable');

const finalInfo = info(id);
const report = {
  ok: true, projectId: id, projectRoot: root, instanceId: current.instanceId, cliPath: process.env.M3_TEST_CLI ?? 'bundled CLI',
  starterBackupId, builtBackupId, modifiedBackupId, pendingBackupId, walBytesBeforeSnapshot, checks, taskRecords,
  originalHashes: hashes, finalBuildInfo: finalInfo.buildInfo, backupDetails: details,
};
await saveReport('m3-live.json', report);
console.log(`M3 live checks passed; project ${id}; evidence .test-artifacts/m3-live.json`);
