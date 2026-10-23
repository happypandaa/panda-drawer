// Explicit, non-destructive M1 integration test. Creates two NEW disposable projects.
// Requires the desktop development app to be running. No production/custom roots.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdir, cp, writeFile, readFile, lstat } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { homedir } from 'node:os';

const repository = resolve(fileURLToPath(new URL('..', import.meta.url)));
const cliPath = process.env.M1_TEST_CLI || join(repository, 'target/debug/local-projects-dev');
function cli(args, input) {
  const result = spawnSync(cliPath, [...args, '--json'], {
    input: input ? JSON.stringify(input) : undefined, encoding: 'utf8', maxBuffer: 9_000_000,
  });
  if (result.error) throw result.error;
  const envelope = JSON.parse(result.stdout);
  if (result.status !== 0 || !envelope.ok) throw new Error(JSON.stringify(envelope));
  return envelope.data;
}
const runtime = cli(['service', 'status']).runtime;
assert.equal(runtime.identifier, 'dev.localprojects.platform.m0');
assert.equal(runtime.environment, 'development');
const expectedRoot = join(homedir(), 'Library/Application Support/dev.localprojects.platform.m0/workspace');
assert.equal(runtime.workspaceRoot, expectedRoot);
assert.match(runtime.baseUrl, /^http:\/\/127\.0\.0\.1:\d+$/);
const origin = runtime.baseUrl;
const run = new Date().toISOString().replace(/[:.]/g, '-');
const projects = [];

for (const [index, word] of ['alpha', 'beta'].entries()) {
  const project = cli(['project', 'create', '--input', '-'], { name: `M1 验证 ${index + 1} · ${run}`, description: '自动创建的可丢弃测试资料；用于本地协议和项目隔离验证。' });
  const id = project.id ?? project.projectId;
  assert.match(id, /^prj_[a-z0-9_-]+$/);
  const root = project.projectRoot;
  assert.equal(dirname(root), join(expectedRoot, 'projects'));
  assert.equal((await lstat(root)).isSymbolicLink(), false);
  const manifest = JSON.parse(await readFile(join(root, 'project.json'), 'utf8'));
  assert.equal(manifest.id, id);
  assert.equal(manifest.outputDir, 'dist');
  cli(['db', 'transaction', '--project', id, '--input', '-'], {
    statements: [
      { sql: 'CREATE TABLE words (id TEXT PRIMARY KEY, word TEXT NOT NULL, meaning TEXT NOT NULL, created_at TEXT NOT NULL)' },
      { sql: 'INSERT INTO words (id, word, meaning, created_at) VALUES (?, ?, ?, ?)', params: ['seed', word, `仅项目 ${index + 1} 的初始记录`, run] },
    ],
  });
  await cp(join(repository, 'examples/m1-runtime'), join(root, 'app/m1-runtime-fixture'), { recursive: true, errorOnExist: true, force: false });
  if (project.state === 'ready') {
    // Current runtimes ship a starter. Publish the fixture through the normal build boundary.
    const packagePath = join(root, 'app/package.json');
    const pkg = JSON.parse(await readFile(packagePath, 'utf8'));
    pkg.scripts.build = 'node build-m1-fixture.mjs';
    await writeFile(packagePath, JSON.stringify(pkg, null, 2));
    await writeFile(join(root, 'app/build-m1-fixture.mjs'), "import {cpSync,mkdirSync} from 'node:fs';\nconst output=process.argv[process.argv.indexOf('--outDir')+1];\nif(!output)throw new Error('Missing --outDir');\nmkdirSync(output,{recursive:true});\nfor(const file of ['index.html','client.js','styles.css'])cpSync('m1-runtime-fixture/'+file,output+'/'+file);\n");
    const submitted = cli(['project', 'build', '--project', id]);
    let task;
    do { task = cli(['task', 'wait', '--task', submitted.taskId]); } while (!task.final);
    assert.equal(task.status, 'succeeded', JSON.stringify(task));
  } else {
    // Historical M1 runtime: no starter or build manager exists yet.
    await mkdir(join(root, 'dist'));
    for (const file of ['index.html', 'client.js', 'styles.css']) {
      await cp(join(repository, 'examples/m1-runtime', file), join(root, 'dist', file), { errorOnExist: true, force: false });
    }
  }
  await writeFile(join(root, 'docs/database.md'), `# M1 测试数据库\n\n仅用于验证，可以丢弃。\n\n## words\n每行一个测试单词。id：文本 UUID 主键（seed 是初始验证记录）；word：英文单词；meaning：中文释义；created_at：ISO 8601 UTC 时间。没有外键、软删除或隐式级联。学习笔记文件独立存放于 data/attachments，不与记录形成一个事务。\n\n源码：app/m1-runtime-fixture；受控产物：dist；由平台仓库 scripts/m1-live-test.mjs 安装；当前运行时经平台构建发布，历史 M1 运行时直接复制。\n`);
  const launch = cli(['project', 'url', '--project', id]);
  const ticket = new URLSearchParams(new URL(launch.launchUrl).hash.slice(1)).get('ticket');
  const sessionResponse = await fetch(`${origin}/_platform/session`, { method: 'POST', headers: { Origin: origin, 'Content-Type': 'application/json' }, body: JSON.stringify({ ticket }) });
  assert.equal(sessionResponse.status, 200, await sessionResponse.clone().text());
  const setCookie = sessionResponse.headers.get('set-cookie');
  assert.match(setCookie, /HttpOnly/i);
  assert.match(setCookie, /SameSite=Strict/i);
  const cookie = setCookie.split(';')[0];
  const entry = await fetch(launch.url, { headers: { Cookie: cookie } });
  const html = await entry.text();
  assert.equal(entry.status, 200, html);
  const epoch = /name="platform-document-epoch" content="([^"]+)"/.exec(html)?.[1];
  assert.ok(epoch, 'Entry must embed documentEpoch');
  const p = { id, root, cookie, epoch, word };
  projects.push(p);
  const replay = await fetch(`${origin}/_platform/session`, { method: 'POST', headers: { Origin: origin, 'Content-Type': 'application/json' }, body: JSON.stringify({ ticket }) });
  assert.equal(replay.status, 401, 'Launch ticket is single-use');
}
async function api(project, route, { method = 'GET', body, cookie = project.cookie, epoch = project.epoch, requestOrigin = origin, headers = {} } = {}) {
  return fetch(`${origin}/p/${project.id}/api/v1${route}`, {
    method, headers: { Cookie: cookie, 'X-Platform-Document-Epoch': epoch, Origin: requestOrigin, ...headers }, body,
  });
}
async function query(project, sql, params = []) {
  const response = await api(project, '/db/execute', { method: 'POST', body: JSON.stringify({ sql, params }), headers: { 'Content-Type': 'application/json' } });
  const result = await response.json();
  assert.equal(result.ok, true, JSON.stringify(result));
  return result.data;
}
for (const project of projects) {
  const rows = await query(project, 'SELECT word FROM words');
  assert.deepEqual(rows.rows, [{ word: project.word }]);
  const text = `独立项目 ${project.id} 的学习资料`;
  const upload = await api(project, '/file?path=protocol-note.txt', { method: 'PUT', body: text });
  assert.equal(upload.status, 200, await upload.clone().text());
  const download = await api(project, '/file?path=protocol-note.txt');
  assert.equal(await download.text(), text);
  const collision = await api(project, '/file?path=protocol-note.txt', { method: 'PUT', body: 'must not overwrite' });
  assert.equal(collision.status, 409);
  assert.equal(await (await api(project, '/file?path=protocol-note.txt')).text(), text);
  const oldEpoch = await api(project, '/db/execute', { method: 'POST', epoch: 'old-document', body: JSON.stringify({ sql: 'DELETE FROM words' }), headers: { 'Content-Type': 'application/json' } });
  assert.equal(oldEpoch.status, 409);
  const wrongOrigin = await api(project, '/db/execute', { method: 'POST', requestOrigin: 'https://outside.example', body: JSON.stringify({ sql: 'DELETE FROM words' }), headers: { 'Content-Type': 'application/json' } });
  assert.equal(wrongOrigin.status, 403);
  const traversal = await api(project, '/file?path=..%2Fmain.sqlite');
  assert.equal(traversal.status >= 400, true);
  const rollback = await api(project, '/db/transaction', { method: 'POST', body: JSON.stringify({ statements: [{ sql: 'DELETE FROM words' }, { sql: 'INSERT INTO absent_table VALUES (1)' }] }), headers: { 'Content-Type': 'application/json' } });
  assert.equal((await rollback.json()).ok, false);
  assert.deepEqual((await query(project, 'SELECT word FROM words')).rows, [{ word: project.word }]);
  const tagged = await query(project, 'SELECT ? AS huge, ? AS binary', [{ $int64: '9223372036854775807' }, { $bytes: 'AAEC/w==' }]);
  assert.deepEqual(tagged.rows[0], { huge: { $int64: '9223372036854775807' }, binary: { $bytes: 'AAEC/w==' } });
  const noBearer = await fetch(`${origin}/api/v1/projects`);
  assert.equal(noBearer.status, 401);
}
const cross = await api(projects[1], '/db/execute', { method: 'POST', cookie: projects[0].cookie, body: JSON.stringify({ sql: 'DELETE FROM words' }), headers: { 'Content-Type': 'application/json' } });
assert.equal(cross.status, 401);
const records = projects.map(({ id, root, word }) => ({ id, root, initialWord: word }));
await mkdir(join(repository, 'artifacts'), { recursive: true });
await writeFile(join(repository, 'artifacts/m1-live-test.json'), JSON.stringify({ run, instanceId: runtime.instanceId, projects: records, result: 'passed', checks: ['two isolated SQLite databases', 'CLI create/schema/SQL', 'single-use session', 'HttpOnly cookies', 'document epoch', 'Origin validation', 'cross-project session rejection', 'atomic attachment conflict', 'path traversal rejection', 'transaction rollback', 'lossless integer and blob'] }, null, 2));
// Cookies for a later explicit restart assertion are local, private, ignored test artifacts.
await writeFile(join(repository, 'artifacts/m1-old-sessions.json'), JSON.stringify({ origin, instanceId: runtime.instanceId, projects }), { mode: 0o600 });
console.log(JSON.stringify({ ok: true, projects: records, verification: join(repository, 'artifacts/m1-live-test.json') }, null, 2));
