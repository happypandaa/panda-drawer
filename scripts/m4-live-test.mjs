// Creates only M4 可丢弃 projects. It never deletes or edits an original project.
// Run only after the current system-trash app is explicitly launched. New disposable
// folders remain in the macOS Trash as evidence; this script never restores or purges them.
import assert from 'node:assert/strict';
import { copyFile, mkdir, readFile, rename, rm, stat, symlink, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import {
  archivePath, artifactRoot, assertDisposable, backup, backupIdOf, begin, business, cli,
  corruptArchivedDatabase, createProject, end, expectBusinessError, expectFailure, info, inspectArchive,
  maintenance, operation, projectIdOf, restore, runtime, saveReport, session, sha256,
  legacyTrashEvidence, requireSystemTrashDeletion, sql, succeeded, verifySystemTrashed, waitTask,
} from './m4-test-utils.mjs';

const current = runtime();
// Fail before creating a fixture if the running package has older delete semantics.
requireSystemTrashDeletion();
const source = createProject('导入导出复制删除验证');
const root = source.projectRoot, id = source.id, tasks = [], checks = [], projects = [id];
const check = text => { checks.push(text); console.log(`PASS ${text}`); };
const output = join(artifactRoot, 'm4-exports', id); await mkdir(output, { recursive: true });
const exportedPath = join(output, 'source.zip');
console.log(`Created ${id} at ${root}`);
const row = { id: 73, uuid: 'm4-business-identity-do-not-replace', value: 'snapshot business row' };
sql(id, 'CREATE TABLE m4_probe(id INTEGER PRIMARY KEY, uuid TEXT NOT NULL UNIQUE, value TEXT NOT NULL)');
sql(id, 'INSERT INTO m4_probe VALUES (?, ?, ?)', [row.id, row.uuid, row.value]);
const originalDocument = await session(current, id);
const attachments = ['node_modules/keep.txt', '.git/keep.txt', '.platform/keep.txt', '.hidden/keep.txt'];
for (const path of attachments) {
  const response = await business(current, originalDocument, `file?path=${encodeURIComponent(path)}`, 'PUT', Buffer.from(`M4 attachment ${path}`));
  assert.equal(response.status, 200, await response.clone().text());
}
await mkdir(join(root, 'meta'), { recursive: true });
await mkdir(join(root, '.persistent'), { recursive: true });
await writeFile(join(root, '.persistent/notes.txt'), 'hidden M4 persistent material');
await writeFile(join(root, 'meta/icon.png'), Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/lS0AAAAASUVORK5CYII=', 'base64'));
const manifest = JSON.parse(await readFile(join(root, 'project.json'), 'utf8'));
manifest.icon = 'meta/icon.png'; manifest.description = 'M4 copied metadata description';
await writeFile(join(root, 'project.json'), JSON.stringify(manifest, null, 2));
const userText = `USER AUTHORED: retain this literal old project ID ${id}`;
await writeFile(join(root, 'README.md'), (await readFile(join(root, 'README.md'), 'utf8')) + `\n${userText}\n`);
const stableFiles = ['app/src/App.tsx', 'app/package-lock.json', 'app/vendor/local-projects-sdk-0.1.0.tgz', '.agents/skills/local-projects-app/SKILL.md', '.persistent/notes.txt', 'meta/icon.png', 'dist/index.html', ...attachments.map(path => `data/attachments/${path}`)];
const hashes = Object.fromEntries(await Promise.all(stableFiles.map(async path => [path, sha256(await readFile(join(root, path)))])));
const manual = backup(id, tasks); succeeded(manual); const manualBackupId = backupIdOf(manual);
const manualBackupHash = sha256(await readFile(archivePath(id, manualBackupId)));
succeeded(operation('export', id, { destinationPath: exportedPath }, tasks));
const exportedHash = sha256(await readFile(exportedPath));
const archived = inspectArchive(exportedPath);
assert.equal(archived.manifest.projectId, id);
assert.equal(archived.manifest.snapshotState, 'ready');
for (const [path, hash] of Object.entries(hashes)) assert.equal(archived.entries.find(entry => entry.path === `project/${path}`)?.sha256, hash, path);
check('export contains source, fixed SDK, skill, metadata, hidden files, webpage and reserved-name attachments');

const exportFailure = input => expectFailure(['project', 'export', '--project', id, '--input', '-'], input, tasks);
assert.ok(exportFailure({ destinationPath: exportedPath }));
assert.equal(sha256(await readFile(exportedPath)), exportedHash, 'No implicit overwrite');
assert.ok(exportFailure({ destinationPath: join(root, 'self-containing.zip') }));
const destinationBlocker = join(output, 'not-a-directory'); await writeFile(destinationBlocker, 'preserve target blocker');
assert.ok(exportFailure({ destinationPath: join(destinationBlocker, 'failure.zip') }));
succeeded(operation('export', id, { destinationPath: exportedPath, overwrite: true }, tasks));
assert.ok(inspectArchive(exportedPath).manifest.projectId === id);
check('export rejects implicit overwrite, self-contained destination and unusable target; explicit overwrite succeeds');

const archiveBeforeImport = sha256(await readFile(exportedPath));
const importFailure = input => expectFailure(['project', 'import', '--input', '-'], input, tasks);
assert.ok(importFailure({ archivePath: exportedPath, mode: 'original' }));
const badZip = join(output, 'invalid.zip'); await writeFile(badZip, 'not a ZIP');
assert.ok(importFailure({ archivePath: badZip, mode: 'copy', name: 'M4 可丢弃 不应登记坏ZIP' }));
const badDatabaseZip = join(output, 'invalid-database.zip'); await copyFile(exportedPath, badDatabaseZip);
corruptArchivedDatabase(badDatabaseZip);
assert.ok(importFailure({ archivePath: badDatabaseZip, mode: 'copy', name: 'M4 可丢弃 不应登记坏归档数据库' }));
check('original-ID conflict, malformed ZIP and checksum-valid archive with corrupt SQLite are rejected');
const copies = [];
const environment = cli(['env', 'doctor']).buildEnvironment;
const temporaryNode = join(root, '.platform/m4-node-probe');
assert.equal(environment.ready, true, 'Available Node required only for the later changed-input fixture');
await symlink(environment.node.path, temporaryNode);
try {
  cli(['env', 'node', 'set', '--path', temporaryNode]); await rename(temporaryNode, `${temporaryNode}.offline`);
  assert.equal(cli(['env', 'doctor']).buildEnvironment.ready, false);
  const imported = operation('import', undefined, { archivePath: exportedPath, mode: 'copy', name: 'M4 可丢弃 ZIP副本' }, tasks);
  succeeded(imported); const importedId = projectIdOf(imported); projects.push(importedId); copies.push(importedId);
  const copied = operation('copy', id, { name: 'M4 可丢弃 直接副本' }, tasks);
  succeeded(copied); const copiedId = projectIdOf(copied); projects.push(copiedId); copies.push(copiedId);
  for (const copyId of copies) {
    assert.notEqual(copyId, id); const project = assertDisposable(info(copyId));
    assert.deepEqual(sql(copyId, 'SELECT id, uuid, value FROM m4_probe').rows, [row]);
    for (const [path, hash] of Object.entries(hashes)) assert.equal(sha256(await readFile(join(project.projectRoot, path))), hash, path);
    assert.ok((await readFile(join(project.projectRoot, 'README.md'), 'utf8')).includes(userText), 'User prose is not globally rewritten');
    assert.notEqual((await stat(join(root, 'meta/icon.png'))).ino, (await stat(join(project.projectRoot, 'meta/icon.png'))).ino);
    assert.equal(project.buildInfo.origin, 'bundledTemplate');
    assert.equal(project.buildInfo.sourceState, 'matched');
    await session(current, copyId);
  }
} finally {
  if (environment.configuredNodePath) cli(['env', 'node', 'set', '--path', environment.configuredNodePath]); else cli(['env', 'node', 'clear']);
  await rm(temporaryNode, { force: true }); await rm(`${temporaryNode}.offline`, { force: true });
}
assert.equal(sha256(await readFile(exportedPath)), archiveBeforeImport, 'Imports never alter the source archive');
sql(copies[0], 'UPDATE m4_probe SET value=? WHERE id=?', ['independent copy', row.id]);
assert.equal(sql(id, 'SELECT value FROM m4_probe WHERE id=?', [row.id]).rows[0].value, row.value);
check('no-Node import and copy create independent new IDs, preserve business primary keys, files and user prose');

// Verify user-built provenance survives the same archive path independently of
// the prebuilt starter coverage above. The configured Node has been restored.
const builtProjectId = copies[1];
succeeded(waitTask(cli(['project', 'build', '--project', builtProjectId]), tasks));
const builtInfo = info(builtProjectId);
assert.equal(builtInfo.buildInfo.origin, 'projectBuild'); assert.equal(builtInfo.buildInfo.sourceState, 'matched');
const builtArchive = join(output, 'project-build.zip');
succeeded(operation('export', builtProjectId, { destinationPath: builtArchive }, tasks));
const builtImported = operation('import', undefined, { archivePath: builtArchive, mode: 'copy', name: 'M4 可丢弃 用户构建归档导入' }, tasks);
succeeded(builtImported); const builtImportedId = projectIdOf(builtImported); projects.push(builtImportedId);
const builtImportedInfo = info(builtImportedId);
assert.equal(builtImportedInfo.buildInfo.origin, 'projectBuild'); assert.equal(builtImportedInfo.buildInfo.sourceState, 'matched');
assert.deepEqual(sql(builtImportedId, 'SELECT id, uuid, value FROM m4_probe').rows, [row]);
await session(current, builtImportedId);
check('projectBuild export/import retains verified portable provenance and runnable webpage without rebuilding the imported project');

succeeded(begin(id, tasks)); let sourceMaintenance = maintenance(id);
const editedSource = (await readFile(join(root, 'app/src/App.tsx'), 'utf8')) + '\n// M4 pending maintenance source\n';
await writeFile(join(root, 'app/src/App.tsx'), editedSource);
const maintenanceCopy = operation('copy', id, { name: 'M4 可丢弃 维护副本', maintenanceId: sourceMaintenance.maintenanceId }, tasks);
succeeded(maintenanceCopy); const maintenanceCopyId = projectIdOf(maintenanceCopy); projects.push(maintenanceCopyId);
const copyState = maintenance(maintenanceCopyId);
assert.equal(copyState.mode, 'maintenance'); assert.equal(copyState.readyForChanges, true);
assert.notEqual(copyState.maintenanceId, sourceMaintenance.maintenanceId);
assert.notEqual(copyState.baselineBackupId, sourceMaintenance.baselineBackupId);
assert.equal(inspectArchive(archivePath(maintenanceCopyId, copyState.baselineBackupId)).manifest.projectId, maintenanceCopyId);
assert.ok(expectFailure(['maintenance', 'end', '--project', maintenanceCopyId, '--input', '-'], { maintenanceId: copyState.maintenanceId }, tasks));
sql(maintenanceCopyId, 'UPDATE m4_probe SET value=?', ['changed after new-ID baseline'], copyState.maintenanceId);
succeeded(restore(maintenanceCopyId, copyState.baselineBackupId, tasks, copyState.maintenanceId));
const restoredCopyState = maintenance(maintenanceCopyId);
assert.equal(sql(maintenanceCopyId, 'SELECT value FROM m4_probe', [], restoredCopyState.maintenanceId).rows[0].value, row.value);
assert.ok(expectFailure(['maintenance', 'end', '--project', maintenanceCopyId, '--input', '-'], { maintenanceId: restoredCopyState.maintenanceId }, tasks));
assert.equal(maintenance(id).maintenanceId, sourceMaintenance.maintenanceId);
check('maintenance copy receives its own usable new-ID baseline; restore cannot clear its pending build requirement');

const maintenanceExport = join(output, 'maintenance.zip');
succeeded(operation('export', id, { destinationPath: maintenanceExport, maintenanceId: sourceMaintenance.maintenanceId }, tasks));
assert.equal(inspectArchive(maintenanceExport).manifest.snapshotState, 'maintenance');
const maintenanceImported = operation('import', undefined, { archivePath: maintenanceExport, mode: 'copy', name: 'M4 可丢弃 维护归档导入' }, tasks);
succeeded(maintenanceImported); const maintenanceImportedId = projectIdOf(maintenanceImported); projects.push(maintenanceImportedId);
const importedState = maintenance(maintenanceImportedId);
assert.equal(importedState.mode, 'maintenance'); assert.equal(importedState.readyForChanges, true);
assert.notEqual(importedState.maintenanceId, sourceMaintenance.maintenanceId);
assert.equal(inspectArchive(archivePath(maintenanceImportedId, importedState.baselineBackupId)).manifest.projectId, maintenanceImportedId);
assert.ok(expectFailure(['maintenance', 'end', '--project', maintenanceImportedId, '--input', '-'], { maintenanceId: importedState.maintenanceId }, tasks));
check('maintenance export/import keeps an independent session and new-ID baseline without clearing pending inputs');

// Restore the source's pre-change baseline so it can finish maintenance cleanly.
succeeded(restore(id, sourceMaintenance.baselineBackupId, tasks, sourceMaintenance.maintenanceId));
sourceMaintenance = maintenance(id); succeeded(end(id, sourceMaintenance.maintenanceId, tasks));
const beforeDelete = await session(current, id);
const legacyTrashBefore = await legacyTrashEvidence();
const deleted = operation('delete', id, {}, tasks);
const firstSystemTrashPath = await verifySystemTrashed(deleted, source);
assert.deepEqual(await legacyTrashEvidence(), legacyTrashBefore, 'Existing platform trash records remain unchanged');
for (const [path, hash] of Object.entries(hashes)) assert.equal(sha256(await readFile(join(firstSystemTrashPath, path))), hash, path);
const deletedPage = await business(current, beforeDelete, 'db/execute', 'POST', { sql: 'SELECT 1' });
assert.ok(!deletedPage.ok, 'Deleted project cannot use its old session');
assert.equal(sha256(await readFile(archivePath(id, manualBackupId))), manualBackupHash);
assert.equal(sha256(await readFile(exportedPath)), archiveBeforeImport);
const originalImport = operation('import', undefined, { archivePath: exportedPath, mode: 'original' }, tasks);
succeeded(originalImport); assert.equal(projectIdOf(originalImport), id);
const reimported = await session(current, id); assert.notEqual(reimported.epoch, beforeDelete.epoch);
await expectBusinessError(await business(current, { ...reimported, epoch: beforeDelete.epoch }, 'db/execute', 'POST', { sql: 'SELECT 1' }), 'PROJECT_RELOAD_REQUIRED');
assert.deepEqual(sql(id, 'SELECT id, uuid, value FROM m4_probe').rows, [row]);
check('system Trash receives the complete project; existing platform trash, backups and external exports are retained; original-ID ZIP import requires a new document identity');

// An independent second OS entry must not overwrite the first one. Recovery in
// this probe uses the existing public ZIP import, never an in-app trash restore.
const reimportedProject = assertDisposable(info(id));
const secondDelete = operation('delete', id, {}, tasks);
const secondSystemTrashPath = await verifySystemTrashed(secondDelete, reimportedProject);
assert.notEqual(secondSystemTrashPath, firstSystemTrashPath);
assert.equal(sha256(await readFile(archivePath(id, manualBackupId))), manualBackupHash);
const secondImport = operation('import', undefined, { archivePath: exportedPath, mode: 'original' }, tasks);
succeeded(secondImport); assert.equal(projectIdOf(secondImport), id);
const afterSecondImport = await session(current, id); assert.notEqual(afterSecondImport.epoch, reimported.epoch);
await expectBusinessError(await business(current, { ...afterSecondImport, epoch: reimported.epoch }, 'db/execute', 'POST', { sql: 'SELECT 1' }), 'PROJECT_RELOAD_REQUIRED');
assert.deepEqual(await legacyTrashEvidence(), legacyTrashBefore);
check('repeated deletion produces distinct system Trash entries and never revives old documents or purges earlier evidence');

// This new fixture has never opened a business database worker. Backup opens and
// closes its own read-only handle before the bytes are intentionally damaged.
const damaged = createProject('坏库系统废纸篓验证'); projects.push(damaged.id);
const damagedBackup = backup(damaged.id, tasks); succeeded(damagedBackup); const damagedBackupId = backupIdOf(damagedBackup);
const healthyHistoryHash = sha256(await readFile(archivePath(damaged.id, damagedBackupId)));
const damagedBytes = Buffer.from('M4 deliberately damaged SQLite fixture; preserve me');
await writeFile(join(damaged.projectRoot, 'data/main.sqlite'), damagedBytes);
const damagedDelete = operation('delete', damaged.id, {}, tasks);
const damagedSystemTrashPath = await verifySystemTrashed(damagedDelete, damaged);
assert.equal(sha256(await readFile(join(damagedSystemTrashPath, 'data/main.sqlite'))), sha256(damagedBytes));
assert.equal(sha256(await readFile(archivePath(damaged.id, damagedBackupId))), healthyHistoryHash);
assert.deepEqual(await legacyTrashEvidence(), legacyTrashBefore);
check('damaged database moves byte-for-byte to system Trash without a healthy-current-database prerequisite; independent historical backup stays unchanged');

const reportName = `m4-system-trash-live-${id}.json`;
await saveReport(reportName, { ok: true, projectId: id, projectRoot: root, workspaceRoot: current.workspaceRoot, instanceId: current.instanceId, projects, maintenanceCopyId, maintenanceImportedId, checks, tasks, exportedPath, manualBackupId, stableHashes: hashes, systemTrashPaths: [firstSystemTrashPath, secondSystemTrashPath, damagedSystemTrashPath], deletionMode: 'system-trash', systemTrashRetainedAsEvidence: true });
console.log(`M4 system-trash live completed; source ${id}; report .test-artifacts/${reportName}`);
