// Usage: node scripts/m3-restart-test.mjs prepare
// Explicitly quit and restart the development app, then run with verify.
// It only ever mutates the one project created by prepare.
import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import {
  assertDisposable, begin, business, cli, createProject, end, expectBusinessError,
  info, maintenance, readReport, runtime, saveReport, session, sha256, sql, succeeded,
} from './m3-test-utils.mjs';

const phase = process.argv[2];
assert.ok(['prepare', 'verify'].includes(phase), 'Choose prepare or verify; restart the app between phases');
const current = runtime();
const taskRecords = [];
if (phase === 'prepare') {
  const project = createProject('维护重启验证');
  const old = await session(current, project.id);
  sql(project.id, 'CREATE TABLE m3_restart (id INTEGER PRIMARY KEY, value TEXT NOT NULL)');
  sql(project.id, 'INSERT INTO m3_restart VALUES (?, ?)', [1, 'committed-before-maintenance']);
  const attachment = join(project.projectRoot, 'data/attachments/restart.txt');
  await writeFile(attachment, 'M3 restart attachment');
  succeeded(begin(project.id, taskRecords));
  const status = maintenance(project.id);
  assert.equal(status.mode, 'maintenance');
  assert.equal(status.readyForChanges, true);
  assert.ok(status.baselineBackupId);
  sql(project.id, 'INSERT INTO m3_restart VALUES (?, ?)', [2, 'committed-during-maintenance'], status.maintenanceId);
  await expectBusinessError(await business(current, old, 'db/execute', 'POST', { sql: 'SELECT 1' }), 'PROJECT_MAINTENANCE');
  await saveReport('m3-restart-state.json', {
    project, instanceId: current.instanceId, old, status, taskRecords,
    attachmentSha256: sha256(await readFile(attachment)),
  }, true);
  console.log(`PREPARED ${project.id}; explicitly quit/restart the development app, then run verify.`);
} else {
  const saved = await readReport('m3-restart-state.json');
  const project = assertDisposable(info(saved.project.id));
  assert.notEqual(current.instanceId, saved.instanceId, 'App must be completely quit and restarted before verify');
  const status = maintenance(project.id);
  assert.equal(status.mode, 'maintenance');
  assert.equal(status.maintenanceId, saved.status.maintenanceId);
  assert.equal(status.baselineBackupId, saved.status.baselineBackupId);
  assert.equal(status.readyForChanges, true);
  const stale = await business(current, saved.old, 'context');
  assert.equal(stale.status, 401, 'Previous-instance cookie must not be accepted');
  const rows = sql(project.id, 'SELECT value FROM m3_restart ORDER BY id', [], status.maintenanceId).rows;
  assert.deepEqual(rows.map(row => row.value), ['committed-before-maintenance', 'committed-during-maintenance']);
  assert.equal(sha256(await readFile(join(project.projectRoot, 'data/attachments/restart.txt'))), saved.attachmentSha256);
  succeeded(end(project.id, status.maintenanceId, taskRecords));
  assert.equal(maintenance(project.id).mode, 'ready');
  const fresh = await session(current, project.id);
  assert.notEqual(fresh.epoch, saved.old.epoch);
  await expectBusinessError(await business(current, { ...fresh, epoch: saved.old.epoch }, 'db/execute', 'POST', {
    sql: 'INSERT INTO m3_restart VALUES (?, ?)', params: [3, 'must-not-be-written'],
  }), 'PROJECT_RELOAD_REQUIRED');
  assert.equal(sql(project.id, 'SELECT COUNT(*) AS count FROM m3_restart').rows[0].count, 2);
  const report = {
    ok: true, projectId: project.id, beforeInstanceId: saved.instanceId, afterInstanceId: current.instanceId, taskRecords,
    checks: ['maintenance ID and baseline survive restart', 'no automatic unlock', 'rows and attachment retained', 'old sessions rejected', 'explicit end advances document epoch', 'fresh cookie does not authorize old document writes'],
  };
  console.log(JSON.stringify(report, null, 2));
  await saveReport('m3-restart.json', report);
}
