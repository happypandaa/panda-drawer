// Shared helpers for M3 probes. Only the fixed development service is accepted.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';

export const repository = resolve(fileURLToPath(new URL('..', import.meta.url)));
export const artifactRoot = join(repository, '.test-artifacts');
export const cliPath = process.env.M3_TEST_CLI || join(repository, 'target/release/bundle/macos/熊猫小抽屉.app/Contents/MacOS/local-projects-dev');
export const expectedWorkspace = join(homedir(), 'Library/Application Support/dev.happypandaa.pandadrawer/workspace');
export const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');

export function invoke(args, input, allowError = false) {
  const result = spawnSync(cliPath, [...args, '--json'], {
    input: input === undefined ? undefined : JSON.stringify(input),
    encoding: 'utf8', maxBuffer: 12_000_000,
  });
  assert.ifError(result.error);
  let wire;
  try { wire = JSON.parse(result.stdout); }
  catch { throw new Error(`CLI did not return JSON for ${args.join(' ')}: ${result.stderr || result.stdout}`); }
  if (!allowError) assert.equal(wire.ok, true, JSON.stringify(wire));
  return wire;
}
export const cli = (args, input) => invoke(args, input).data;
export function runtime() {
  const current = cli(['service', 'status']).runtime;
  assert.equal(current.identifier, 'dev.happypandaa.pandadrawer');
  assert.equal(current.environment, 'development');
  assert.equal(current.workspaceRoot, expectedWorkspace);
  assert.equal(new URL(current.baseUrl).hostname, '127.0.0.1');
  return current;
}
export function assertDisposable(project) {
  assert.match(project.name, /^M3 可丢弃/);
  assert.equal(dirname(project.projectRoot), join(expectedWorkspace, 'projects'));
  return project;
}
export function createProject(label) {
  return assertDisposable(cli(['project', 'create', '--input', '-'], {
    name: `M3 可丢弃 ${label} ${new Date().toISOString()}`,
    description: 'M3 自动验收专用；只写本次新建项目，保留历史开发数据。',
  }));
}
export function waitTask(submitted, taskRecords = []) {
  if (!submitted.taskId) return submitted;
  let task;
  do { task = cli(['task', 'wait', '--task', submitted.taskId, '--timeout', '30']); } while (!task.final);
  taskRecords.push(task);
  return task;
}
export function succeeded(task) {
  if (task.status) assert.equal(task.status, 'succeeded', JSON.stringify(task));
  return task.result ?? task;
}
export const maintenance = id => cli(['maintenance', 'status', '--project', id]);
export const info = id => cli(['project', 'info', '--project', id]);
export function sql(id, statement, params = [], maintenanceId) {
  return cli(['db', 'execute', '--project', id, '--input', '-'], { sql: statement, params, ...(maintenanceId ? { maintenanceId } : {}) });
}
export function begin(id, taskRecords, maintenanceId) {
  return waitTask(cli(['maintenance', 'begin', '--project', id, '--input', '-'], {
    reason: 'M3 可丢弃项目验收：完整基线、维护和恢复', ...(maintenanceId ? { maintenanceId } : {}),
  }), taskRecords);
}
export function end(id, maintenanceId, taskRecords) {
  return waitTask(cli(['maintenance', 'end', '--project', id, '--input', '-'], { maintenanceId }), taskRecords);
}
export function backup(id, taskRecords, maintenanceId) {
  return waitTask(cli(['backup', 'create', '--project', id, ...(maintenanceId ? ['--maintenance', maintenanceId] : [])]), taskRecords);
}
export function restore(id, backupId, taskRecords, maintenanceId) {
  return waitTask(cli(['backup', 'restore', '--project', id, '--input', '-'], {
    backupId, ...(maintenanceId ? { maintenanceId } : {}),
  }), taskRecords);
}
export function backupIdOf(task) {
  const data = task.result ?? task;
  const backupId = data.backupId ?? data.backup?.backupId;
  assert.equal(typeof backupId, 'string', JSON.stringify(task));
  return backupId;
}
export const archivePath = (id, backupId) => join(expectedWorkspace, 'backups', id, `${backupId}.zip`);

export async function session(current, id) {
  const launch = cli(['project', 'url', '--project', id]);
  const ticket = new URLSearchParams(new URL(launch.launchUrl).hash.slice(1)).get('ticket');
  assert.ok(ticket);
  const response = await fetch(`${current.baseUrl}/_platform/session`, {
    method: 'POST', headers: { Origin: current.baseUrl, 'Content-Type': 'application/json' }, body: JSON.stringify({ ticket }),
  });
  assert.equal(response.status, 200, await response.clone().text());
  const cookie = response.headers.get('set-cookie').split(';')[0];
  const htmlResponse = await fetch(launch.url, { headers: { Cookie: cookie } });
  assert.equal(htmlResponse.status, 200, await htmlResponse.clone().text());
  const html = await htmlResponse.text();
  const epoch = /name="platform-document-epoch" content="([^"]+)"/.exec(html)?.[1];
  assert.ok(epoch, 'Project HTML must carry its immutable document epoch');
  return { id, cookie, epoch, html, url: launch.url };
}
export function business(current, document, route, method = 'GET', body) {
  const json = body !== undefined && !(body instanceof Uint8Array);
  return fetch(`${current.baseUrl}/p/${document.id}/api/v1/${route}`, {
    method, headers: {
      Cookie: document.cookie, Origin: current.baseUrl, 'X-Platform-Document-Epoch': document.epoch,
      ...(json ? { 'Content-Type': 'application/json' } : {}),
    }, body: body === undefined ? undefined : json ? JSON.stringify(body) : body,
  });
}
export async function expectBusinessError(response, code) {
  const wire = await response.json();
  assert.equal(wire.ok, false, JSON.stringify(wire));
  assert.equal(wire.error.code, code, JSON.stringify(wire));
}
export async function saveReport(name, value, privateFile = false) {
  const path = join(artifactRoot, name);
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, JSON.stringify(value, null, 2), { mode: privateFile ? 0o600 : 0o644 });
  return path;
}
export async function readReport(name) { return JSON.parse(await readFile(join(artifactRoot, name), 'utf8')); }

// Python's standard library reads the ZIP without extracting into the project.
export function inspectArchive(path) {
  const code = `import hashlib,json,sys,zipfile\nwith zipfile.ZipFile(sys.argv[1]) as z:\n m=json.loads(z.read('backup.json'))\n print(json.dumps({'manifest':m,'entries':[{'path':i.filename,'bytes':i.file_size,'sha256':hashlib.sha256(z.read(i)).hexdigest()} for i in z.infolist() if not i.is_dir()]}))`;
  const result = spawnSync('/usr/bin/python3', ['-c', code, path], { encoding: 'utf8', maxBuffer: 12_000_000 });
  assert.ifError(result.error); assert.equal(result.status, 0, result.stderr);
  return JSON.parse(result.stdout);
}

export function corruptArchivedDatabase(path) {
  // Keep the ZIP and file checksums valid so the failure must reach SQLite validation.
  const code = `import hashlib,json,os,sys,zipfile\np=sys.argv[1]\nwith zipfile.ZipFile(p) as z:\n entries=[(i,z.read(i)) for i in z.infolist()]\nm=json.loads(next(b for i,b in entries if i.filename=='backup.json'))\nmanifest=json.loads(next(b for i,b in entries if i.filename=='project/project.json'))\ndb=manifest['database']\nbad=b'not a valid SQLite database'\nfor f in m['files']:\n if f['path']==db: f.update(bytes=len(bad),sha256=hashlib.sha256(bad).hexdigest())\nwith zipfile.ZipFile(p+'.m3-tmp','w') as out:\n for i,b in entries:\n  if i.filename=='backup.json': b=json.dumps(m).encode()\n  elif i.filename=='project/'+db: b=bad\n  out.writestr(i,b)\nos.replace(p+'.m3-tmp',p)`;
  const result = spawnSync('/usr/bin/python3', ['-c', code, path], { encoding: 'utf8', maxBuffer: 1_000_000 });
  assert.ifError(result.error); assert.equal(result.status, 0, result.stderr);
}
