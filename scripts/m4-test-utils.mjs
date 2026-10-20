// M4 probes bind to the fixed development data identity, while allowing the
// workspace selected by the public migration workflow to move.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import { dirname, isAbsolute, join, resolve } from 'node:path';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
export { inspectArchive, corruptArchivedDatabase } from './m3-test-utils.mjs';
export const repository = resolve(fileURLToPath(new URL('..', import.meta.url)));
export const artifactRoot = join(repository, '.test-artifacts');
export const dataRoot = join(homedir(), 'Library/Application Support/dev.localprojects.platform.m0');
export const defaultWorkspace = join(dataRoot, 'workspace');
export const cliPath = process.env.M4_TEST_CLI || join(repository, 'target/release/bundle/macos/熊猫应用小抽屉.app/Contents/MacOS/local-projects-dev');
export const sha256 = value => createHash('sha256').update(value).digest('hex');
export function settings() {
  const value = JSON.parse(readFileSync(join(dataRoot, 'settings.json'), 'utf8'));
  assert.equal(value.identifier, 'dev.localprojects.platform.m0');
  assert.equal(value.environment, 'development');
  assert.ok(isAbsolute(value.workspaceRoot));
  return value;
}
export function invoke(args, input, allowError = false) {
  const result = spawnSync(cliPath, [...args, '--json'], { input: input === undefined ? undefined : JSON.stringify(input), encoding: 'utf8', maxBuffer: 16_000_000 });
  assert.ifError(result.error);
  let wire;
  try { wire = JSON.parse(result.stdout); } catch { throw new Error(`CLI did not return JSON for ${args.join(' ')}: ${result.stderr || result.stdout}`); }
  if (!allowError) assert.equal(wire.ok, true, JSON.stringify(wire));
  return wire;
}
export const cli = (args, input) => invoke(args, input).data;
export function runtime() {
  const current = cli(['service', 'status']).runtime;
  const selected = settings();
  const discovery = JSON.parse(readFileSync(join(dataRoot, 'runtime/connection.json'), 'utf8'));
  for (const value of [current, discovery]) {
    assert.equal(value.identifier, 'dev.localprojects.platform.m0');
    assert.equal(value.environment, 'development');
    assert.equal(value.workspaceRoot, selected.workspaceRoot);
    assert.equal(new URL(value.baseUrl).hostname, '127.0.0.1');
  }
  assert.equal(current.instanceId, discovery.instanceId);
  return current;
}
export function assertDisposable(project) {
  assert.match(project.name, /^M4 可丢弃/);
  assert.equal(project.projectRoot, join(settings().workspaceRoot, 'projects', project.id));
  return project;
}
export function createProject(label) {
  return assertDisposable(cli(['project', 'create', '--input', '-'], {
    name: `M4 可丢弃 ${label} ${new Date().toISOString()}`,
    description: 'M4 自动验收专用；仅修改本次新建项目，保留历史开发数据。',
  }));
}
export function waitTask(submitted, records = []) {
  if (!submitted.taskId) return submitted;
  let task;
  do { task = cli(['task', 'wait', '--task', submitted.taskId, '--timeout', '30']); } while (!task.final);
  records.push(task); return task;
}
export function succeeded(task) { if (task.status) assert.equal(task.status, 'succeeded', JSON.stringify(task)); return task.result ?? task; }
export const info = id => cli(['project', 'info', '--project', id]);
export const maintenance = id => cli(['maintenance', 'status', '--project', id]);
export function sql(id, statement, params = [], maintenanceId) { return cli(['db', 'execute', '--project', id, '--input', '-'], { sql: statement, params, ...(maintenanceId ? { maintenanceId } : {}) }); }
export function operation(command, id, input, records) { return waitTask(cli(['project', command, ...(id ? ['--project', id] : []), '--input', '-'], input ?? {}), records); }
export function begin(id, records) { return waitTask(cli(['maintenance', 'begin', '--project', id, '--input', '-'], { reason: 'M4 可丢弃项目生命周期验收' }), records); }
export function end(id, maintenanceId, records) { return waitTask(cli(['maintenance', 'end', '--project', id, '--input', '-'], { maintenanceId }), records); }
export function restore(id, backupId, records, maintenanceId) { return waitTask(cli(['backup', 'restore', '--project', id, '--input', '-'], { backupId, ...(maintenanceId ? { maintenanceId } : {}) }), records); }
export function backup(id, records, maintenanceId) { return waitTask(cli(['backup', 'create', '--project', id, ...(maintenanceId ? ['--maintenance', maintenanceId] : [])]), records); }
export function trash(command, trashId, records) { return waitTask(cli(['trash', command, '--trash', trashId]), records); }
export function expectFailure(args, input, records) {
  const wire = invoke(args, input, true);
  if (!wire.ok) return wire.error;
  const task = waitTask(wire.data, records); assert.equal(task.status, 'failed', JSON.stringify(task)); return task.error;
}
export const backupIdOf = task => { const value = (task.result ?? task).backupId; assert.equal(typeof value, 'string'); return value; };
export const projectIdOf = task => { const data = task.result ?? task; const value = data.projectId ?? data.project?.id ?? data.id; assert.equal(typeof value, 'string', JSON.stringify(task)); return value; };
export const trashIdOf = task => { const data = task.result ?? task; const value = data.trashId ?? data.trash?.trashId; assert.equal(typeof value, 'string', JSON.stringify(task)); return value; };
export const archivePath = (id, backupId) => join(settings().workspaceRoot, 'backups', id, `${backupId}.zip`);
export async function session(current, id) {
  const launch = cli(['project', 'url', '--project', id]);
  const ticket = new URLSearchParams(new URL(launch.launchUrl).hash.slice(1)).get('ticket'); assert.ok(ticket);
  const response = await fetch(`${current.baseUrl}/_platform/session`, { method: 'POST', headers: { Origin: current.baseUrl, 'Content-Type': 'application/json' }, body: JSON.stringify({ ticket }) });
  assert.equal(response.status, 200, await response.clone().text());
  const cookie = response.headers.get('set-cookie').split(';')[0];
  const page = await fetch(launch.url, { headers: { Cookie: cookie } }); assert.equal(page.status, 200, await page.clone().text());
  const html = await page.text(); const epoch = /name="platform-document-epoch" content="([^"]+)"/.exec(html)?.[1]; assert.ok(epoch);
  return { id, cookie, epoch, html, url: launch.url };
}
export function business(current, document, route, method = 'GET', body) {
  const json = body !== undefined && !(body instanceof Uint8Array);
  return fetch(`${current.baseUrl}/p/${document.id}/api/v1/${route}`, { method, headers: { Cookie: document.cookie, Origin: current.baseUrl, 'X-Platform-Document-Epoch': document.epoch, ...(json ? { 'Content-Type': 'application/json' } : {}) }, body: body === undefined ? undefined : json ? JSON.stringify(body) : body });
}
export async function expectBusinessError(response, code) { const wire = await response.json(); assert.equal(wire.ok, false, JSON.stringify(wire)); assert.equal(wire.error.code, code, JSON.stringify(wire)); }
export async function saveReport(name, value, privateFile = false) { const path = join(artifactRoot, name); await mkdir(dirname(path), { recursive: true }); await writeFile(path, JSON.stringify(value, null, 2), { mode: privateFile ? 0o600 : 0o644 }); return path; }
export const readReport = async name => JSON.parse(await readFile(join(artifactRoot, name), 'utf8'));
