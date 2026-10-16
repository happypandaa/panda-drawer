// Run after an explicit quit + restart following test:m1:live.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFile, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
const repository = resolve(fileURLToPath(new URL('..', import.meta.url)));
const cliPath = process.env.M1_TEST_CLI || join(repository, 'target/debug/local-projects-dev');
function cli(args, input) {
  const result = spawnSync(cliPath, [...args, '--json'], { input: input ? JSON.stringify(input) : undefined, encoding: 'utf8', maxBuffer: 9_000_000 });
  if (result.error) throw result.error;
  const value = JSON.parse(result.stdout);
  assert.equal(result.status, 0, result.stdout);
  assert.equal(value.ok, true, result.stdout);
  return value.data;
}
const old = JSON.parse(await readFile(join(repository, 'artifacts/m1-old-sessions.json'), 'utf8'));
const runtime = cli(['service', 'status']).runtime;
assert.equal(runtime.identifier, 'dev.localprojects.platform.m0');
assert.equal(runtime.environment, 'development');
assert.notEqual(runtime.instanceId, old.instanceId, 'Quit and restart before this test');
for (const project of old.projects) {
  const stale = await fetch(`${runtime.baseUrl}/p/${project.id}/api/v1/context`, { headers: { Cookie: project.cookie, 'X-Platform-Document-Epoch': project.epoch } });
  assert.equal(stale.status, 401, 'Previous-instance cookie must fail');
  const data = cli(['db', 'execute', '--project', project.id, '--input', '-'], { sql: 'SELECT word FROM words ORDER BY word' });
  assert.ok(data.rows.some(row => row.word === project.word));
  assert.ok(!data.rows.some(row => row.word === (project.word === 'alpha' ? 'beta' : 'alpha')));
  const launch = cli(['project', 'url', '--project', project.id]);
  assert.equal(launch.documentEpoch, project.epoch, 'Ordinary restart preserves document identity');
  const ticket = new URLSearchParams(new URL(launch.launchUrl).hash.slice(1)).get('ticket');
  const session = await fetch(`${runtime.baseUrl}/_platform/session`, { method: 'POST', headers: { Origin: runtime.baseUrl, 'Content-Type': 'application/json' }, body: JSON.stringify({ ticket }) });
  assert.equal(session.status, 200);
  const cookie = session.headers.get('set-cookie').split(';')[0];
  const attachment = await fetch(`${runtime.baseUrl}/p/${project.id}/api/v1/file?path=protocol-note.txt`, { headers: { Cookie: cookie, 'X-Platform-Document-Epoch': project.epoch } });
  assert.equal(await attachment.text(), `独立项目 ${project.id} 的学习资料`);
}
const report = { ok: true, beforeInstanceId: old.instanceId, afterInstanceId: runtime.instanceId, projects: old.projects.map(p => p.id), checks: ['previous-instance sessions rejected', 'SQLite rows retained', 'project data still isolated', 'attachments retained', 'fresh launch sessions work'] };
await writeFile(join(repository, 'artifacts/m1-restart-test.json'), JSON.stringify(report, null, 2));
console.log(JSON.stringify(report, null, 2));
