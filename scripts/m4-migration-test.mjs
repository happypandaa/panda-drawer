// prepare -> coordinator performs PUBLIC migration and complete app restart -> verify.
// This script never changes settings, renames a workspace, or starts migration itself.
import assert from 'node:assert/strict';
import { lstat, readFile, readdir, readlink, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import {
  archivePath, artifactRoot, backup, backupIdOf, begin, business, cli, createProject, dataRoot, expectBusinessError,
  info, maintenance, operation, readReport, runtime, saveReport, session, sha256, sql,
  legacyTrashEvidence, requireSystemTrashDeletion, succeeded, verifySystemTrashed,
} from './m4-test-utils.mjs';

const phase = process.argv[2]; assert.ok(['prepare', 'verify'].includes(phase));
const current = runtime(); const tasks = [];
requireSystemTrashDeletion();
async function exactTree(root, relative = '') {
  const result = {};
  for (const name of (await readdir(join(root, relative))).sort()) {
    const path = relative ? `${relative}/${name}` : name; const absolute = join(root, path); const meta = await lstat(absolute);
    if (meta.isSymbolicLink()) result[path] = { kind: 'symlink', target: await readlink(absolute) };
    else if (meta.isDirectory()) { result[path] = { kind: 'directory' }; Object.assign(result, await exactTree(root, path)); }
    else if (meta.isFile()) result[path] = { kind: 'file', bytes: meta.size, sha256: sha256(await readFile(absolute)) };
    else throw new Error(`Unsupported retained entry ${absolute}`);
  }
  return result;
}
async function protectedTrees(workspace) {
  const trees = {};
  for (const relative of ['backups', '.platform/trash']) {
    try { trees[relative] = await exactTree(join(workspace, relative)); }
    catch (error) { if (error.code !== 'ENOENT') throw error; }
  }
  const operations = join(workspace, '.platform/operations');
  for (const entry of await readdir(operations, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    for (const retained of ['original']) {
      const relative = `.platform/operations/${entry.name}/${retained}`;
      try { trees[relative] = await exactTree(join(workspace, relative)); }
      catch (error) { if (error.code !== 'ENOENT') throw error; }
    }
  }
  return trees;
}
async function compareMigratedTree(sourceWorkspace, targetWorkspace, relative, before) {
  assert.deepEqual(await exactTree(join(sourceWorkspace, relative)), before, `Original retained tree changed: ${relative}`);
  const target = await exactTree(join(targetWorkspace, relative));
  const expected = structuredClone(before); let rebound = 0;
  if (relative === '.platform/trash') {
    for (const path of Object.keys(before).filter(path => /^trash_[^/]+\/record\.json$/.test(path))) {
      const sourceRecord = JSON.parse(await readFile(join(sourceWorkspace, relative, path), 'utf8'));
      const targetRecord = JSON.parse(await readFile(join(targetWorkspace, relative, path), 'utf8'));
      const projectPath = path.replace(/record\.json$/, 'project');
      if (!before[projectPath]) { assert.deepEqual(targetRecord, sourceRecord); continue; }
      const sourceIdentity = await lstat(join(sourceWorkspace, relative, projectPath));
      const targetIdentity = await lstat(join(targetWorkspace, relative, projectPath));
      assert.ok(sourceIdentity.isDirectory() && !sourceIdentity.isSymbolicLink());
      assert.ok(targetIdentity.isDirectory() && !targetIdentity.isSymbolicLink());
      const sourcePair = Array.isArray(sourceRecord.rootIdentity) ? sourceRecord.rootIdentity : [sourceRecord.rootIdentity.device, sourceRecord.rootIdentity.inode];
      assert.equal(sourcePair[1], sourceIdentity.ino, `Source trash inode was not valid: ${path}`);
      if (sourceRecord.rootIdentity.volumeUuid) assert.match(sourceRecord.rootIdentity.volumeUuid, /^[0-9a-f-]{36}$/i);
      else assert.equal(sourcePair[0], sourceIdentity.dev, `Legacy source trash device was not valid: ${path}`);
      assert.deepEqual([targetRecord.rootIdentity.device, targetRecord.rootIdentity.inode], [targetIdentity.dev, targetIdentity.ino], `Target trash identity not rebound: ${path}`);
      assert.match(targetRecord.rootIdentity.volumeUuid, /^[0-9a-f-]{36}$/i);
      if (sourceRecord.rootIdentity.volumeUuid && sourceIdentity.dev === targetIdentity.dev) assert.equal(targetRecord.rootIdentity.volumeUuid, sourceRecord.rootIdentity.volumeUuid);
      assert.deepEqual({ ...targetRecord, rootIdentity: sourceRecord.rootIdentity }, sourceRecord, `Non-identity trash metadata changed: ${path}`);
      expected[path] = target[path];
      if (JSON.stringify(sourceRecord.rootIdentity) !== JSON.stringify(targetRecord.rootIdentity)) rebound++;
    }
  }
  assert.deepEqual(target, expected, `Protected migration content changed: ${relative}`);
  return rebound;
}
if (phase === 'prepare') {
  await assert.rejects(lstat(join(artifactRoot, 'm4-system-trash-migration-state.json')), { code: 'ENOENT' },
    'Existing system-trash migration evidence is retained; inspect it before preparing another run.');
  const ready = createProject('迁移后继续使用');
  sql(ready.id, 'CREATE TABLE m4_migration(id INTEGER PRIMARY KEY, value TEXT NOT NULL)');
  sql(ready.id, 'INSERT INTO m4_migration VALUES (?, ?)', [1, 'before migration']);
  await writeFile(join(ready.projectRoot, 'data/attachments/migration.txt'), 'saved before migration');
  const oldDocument = await session(current, ready.id);
  const oldControlToken = (await readFile(join(dataRoot, 'runtime/control-token'), 'utf8')).trim();
  assert.ok(/^[a-f0-9]{64}$/.test(oldControlToken), 'Expected the fixed development management credential format');
  const maintained = createProject('迁移保留维护');
  sql(maintained.id, 'CREATE TABLE m4_migration(id INTEGER PRIMARY KEY, value TEXT NOT NULL)');
  sql(maintained.id, 'INSERT INTO m4_migration VALUES (?, ?)', [1, 'maintenance data']);
  succeeded(begin(maintained.id, tasks)); const maintainedState = maintenance(maintained.id);
  const deleted = createProject('迁移前系统废纸篓');
  const history = backup(deleted.id, tasks); succeeded(history);
  const deleteTask = operation('delete', deleted.id, {}, tasks);
  const systemTrashPath = await verifySystemTrashed(deleteTask, deleted);
  const systemTrashTree = await exactTree(systemTrashPath);
  const projects = cli(['project', 'list']).map(project => ({ id: project.id, name: project.name, documentEpoch: project.documentEpoch }));
  const preserved = await protectedTrees(current.workspaceRoot);
  await saveReport('m4-system-trash-migration-state.json', {
    schemaVersion: 2, deletionMode: 'system-trash', capturedAt: new Date().toISOString(), instanceId: current.instanceId, workspaceRoot: current.workspaceRoot,
    ready, maintained, maintainedState, oldDocument, oldControlToken, systemTrashPath, systemTrashTree, deletedId: deleted.id,
    deletedBackupId: backupIdOf(history), projects, preserved, tasks,
  }, true, true);
  console.log(`PREPARED migration: ready ${ready.id}, maintenance ${maintained.id}, system Trash ${systemTrashPath}; coordinator may now migrate using the public API.`);
} else {
  const saved = await readReport('m4-system-trash-migration-state.json');
  assert.equal(saved.schemaVersion, 2, 'Historical M4 migration evidence has different delete semantics; retain it and prepare a fresh disposable system-trash run.');
  assert.equal(saved.deletionMode, 'system-trash');
  assert.notEqual(current.workspaceRoot, saved.workspaceRoot, 'Run verify after public migration to a new root');
  assert.notEqual(current.instanceId, saved.instanceId, 'Migration/restart must refresh runtime identity');
  const selected = cli(['workspace', 'info']);
  const state = maintenance(saved.maintained.id);
  assert.equal(state.mode, 'maintenance'); assert.equal(state.maintenanceId, saved.maintainedState.maintenanceId);
  assert.equal(state.baselineBackupId, saved.maintainedState.baselineBackupId); assert.equal(state.readyForChanges, true);
  assert.equal(sql(saved.maintained.id, 'SELECT value FROM m4_migration', [], state.maintenanceId).rows[0].value, 'maintenance data');
  for (const project of saved.projects) {
    const moved = info(project.id); assert.equal(moved.projectRoot, join(current.workspaceRoot, 'projects', project.id));
    assert.notEqual(moved.documentEpoch, project.documentEpoch, `Project epoch did not advance: ${project.id}`);
  }
  let reboundTrashRecords = 0;
  for (const [relative, tree] of Object.entries(saved.preserved)) reboundTrashRecords += await compareMigratedTree(saved.workspaceRoot, current.workspaceRoot, relative, tree);
  const oldSessionResponse = await business(current, saved.oldDocument, 'context'); assert.equal(oldSessionResponse.status, 401);
  const selectedToken = (await readFile(join(dataRoot, 'runtime/control-token'), 'utf8')).trim();
  assert.ok(selectedToken !== saved.oldControlToken, 'Migration/restart must rotate the management credential');
  const oldManagement = await fetch(`${current.baseUrl}/api/v1/projects`, { headers: { Authorization: `Bearer ${saved.oldControlToken}` } });
  assert.equal(oldManagement.status, 401, 'Old management credential must not authorize the new runtime');
  const fresh = await session(current, saved.ready.id);
  await expectBusinessError(await business(current, { ...fresh, epoch: saved.oldDocument.epoch }, 'db/execute', 'POST', { sql: 'SELECT 1' }), 'PROJECT_RELOAD_REQUIRED');
  assert.equal(sql(saved.ready.id, 'SELECT value FROM m4_migration WHERE id=?', [1]).rows[0].value, 'before migration');
  assert.equal(await readFile(join(current.workspaceRoot, 'projects', saved.ready.id, 'data/attachments/migration.txt'), 'utf8'), 'saved before migration');
  const originalDatabase = join(saved.workspaceRoot, 'projects', saved.ready.id, 'data/main.sqlite');
  const retainedDatabaseDigest = sha256(await readFile(originalDatabase));
  const existingWrite = sql(saved.ready.id, 'SELECT value FROM m4_migration WHERE id=?', [2]).rows;
  if (!existingWrite.length) sql(saved.ready.id, 'INSERT INTO m4_migration VALUES (?, ?)', [2, 'written only in new workspace']);
  else assert.equal(existingWrite[0].value, 'written only in new workspace');
  assert.equal(sha256(await readFile(originalDatabase)), retainedDatabaseDigest, 'New runtime writes must leave the retained source database unchanged');
  const handoff = cli(['project', 'handoff', '--project', saved.ready.id]); assert.ok(JSON.stringify(handoff).includes(current.workspaceRoot));
  assert.ok(!cli(['project', 'list']).some(project => project.id === saved.deletedId));
  assert.deepEqual(await exactTree(saved.systemTrashPath), saved.systemTrashTree, 'Migration must not move or modify OS-owned Trash entries');
  const migrations = await readdir(join(dataRoot, 'migrations')); assert.ok(migrations.length > 0);
  await saveReport(`m4-system-trash-migration-static-${current.instanceId}.json`, { ok: true, source: saved.workspaceRoot, destination: current.workspaceRoot, comparedActiveProjects: saved.projects.length, comparedProtectedTrees: Object.keys(saved.preserved).length, reboundTrashRecords, capturedAt: new Date().toISOString(), lifecycleProbeStillPending: true });
  // Complete static comparison before creating another explicitly disposable
  // sample in the migrated root. Existing legacy trash is only read/compared.
  const retainedBackup = archivePath(saved.deletedId, saved.deletedBackupId);
  const historyDigest = sha256(await readFile(retainedBackup));
  const legacyTrashBefore = await legacyTrashEvidence();
  const probe = createProject('迁移后系统废纸篓验证');
  sql(probe.id, 'CREATE TABLE m4_after_migration(id INTEGER PRIMARY KEY, value TEXT)');
  sql(probe.id, 'INSERT INTO m4_after_migration VALUES (?, ?)', [1, 'deleted from the migrated workspace']);
  await writeFile(join(probe.projectRoot, 'data/attachments/system-trash.txt'), 'disposable post-migration attachment');
  const probeDocument = await session(current, probe.id);
  const probeBackupTask = backup(probe.id, tasks); succeeded(probeBackupTask);
  const probeBackup = archivePath(probe.id, backupIdOf(probeBackupTask));
  const probeHistoryDigest = sha256(await readFile(probeBackup));
  const deletedAgain = operation('delete', probe.id, {}, tasks);
  const finalSystemTrashPath = await verifySystemTrashed(deletedAgain, probe);
  assert.equal(await readFile(join(finalSystemTrashPath, 'data/attachments/system-trash.txt'), 'utf8'), 'disposable post-migration attachment');
  assert.ok(!(await business(current, probeDocument, 'context')).ok, 'Deleted migrated project must reject its old session');
  assert.deepEqual(await legacyTrashEvidence(), legacyTrashBefore, 'Post-migration deletion must not create or change legacy platform trash');
  assert.equal(sha256(await readFile(retainedBackup)), historyDigest, 'Post-migration delete must retain earlier independent history');
  assert.equal(sha256(await readFile(probeBackup)), probeHistoryDigest, 'Post-migration delete must retain its independent historical backup');
  assert.deepEqual(await exactTree(saved.systemTrashPath), saved.systemTrashTree, 'New deletion must not touch the earlier OS Trash entry');
  const report = { ok: true, deletionMode: 'system-trash', source: saved.workspaceRoot, destination: current.workspaceRoot, beforeInstanceId: saved.instanceId, afterInstanceId: current.instanceId, readyProjectId: saved.ready.id, maintenanceProjectId: saved.maintained.id, deletedBeforeMigrationProjectId: saved.deletedId, systemTrashPath: saved.systemTrashPath, postMigrationProbeId: probe.id, finalSystemTrashPath, systemTrashRetainedAsEvidence: true, comparedActiveProjects: saved.projects.length, comparedProtectedTrees: Object.keys(saved.preserved).length, reboundTrashRecords, workspaceInfo: selected, tasks, checks: ['original tree retained byte-for-byte', 'all projects rebound to selected root with fresh epochs', 'maintenance ID and baseline preserved', 'all backups, existing legacy trash and protected originals copied byte-for-byte including symlinks', 'legacy trash metadata changes only validated physical root identity', 'old management credential, sessions and documents rejected', 'saved data and attachment readable', 'new writes use selected root', 'handoff reflects new location', 'migration journal remains in fixed development data root', 'system Trash is outside migration and its retained payload stays unchanged', 'new disposable migrated project deletes to system Trash and rejects old session while retaining historical backups'] };
  await saveReport(`m4-system-trash-migration-${current.instanceId}.json`, report);
  console.log(`PASS migration and restart; ${saved.projects.length} active projects, ${Object.keys(saved.preserved).length} protected trees verified.`);
}
