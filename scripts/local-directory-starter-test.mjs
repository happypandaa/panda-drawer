// Exercise the optional backend control fixture with a real Node process and disposable data.
// The product starter is the shared SDK-enabled web project, verified through ProjectStore.
// This does not launch the desktop app or operate on registered user projects.
import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { randomBytes, randomUUID } from 'node:crypto';
import { mkdtemp, mkdir, copyFile, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { createInterface } from 'node:readline';
import { fileURLToPath } from 'node:url';

function bounded(promise, label) {
  let timer;
  return Promise.race([
    promise,
    new Promise((_, reject) => { timer = setTimeout(() => reject(new Error(`${label} timed out`)), 5000); }),
  ]).finally(() => clearTimeout(timer));
}

test('optional backend fixture follows the actual control and authenticated HTTP protocol', async t => {
  const root = await mkdtemp(path.join(tmpdir(), 'panda-disposable-local-starter-'));
  const backend = path.join(root, 'backend');
  const data = path.join(root, 'data');
  const cache = path.join(root, 'cache');
  const temporary = path.join(root, 'tmp');
  for (const folder of [backend, data, cache, temporary]) await mkdir(folder);
  const entry = path.join(backend, 'server.mjs');
  await copyFile(fileURLToPath(new URL('./fixtures/backend-control-server.mjs', import.meta.url)), entry);
  await writeFile(path.join(data, 'preserved-business-data.txt'), 'disposable existing content');
  const instanceId = randomUUID();
  const token = randomBytes(32).toString('hex');
  const child = spawn(process.execPath, [entry], {
    cwd: backend,
    env: { APP_HOST: '127.0.0.1', APP_PORT: '0', APP_INSTANCE_ID: instanceId, APP_DATA_DIR: data, APP_CACHE_DIR: cache, APP_TEMP_DIR: temporary, LANG: 'en_US.UTF-8' },
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  const exited = new Promise((resolve, reject) => { child.once('exit', (code, signal) => resolve({ code, signal })); child.once('error', reject); });
  let stderr = '';
  child.stderr.setEncoding('utf8');
  child.stderr.on('data', value => { stderr += value; });
  const reader = createInterface({ input: child.stdout });
  const messages = reader[Symbol.asyncIterator]();
  t.after(async () => {
    if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
    try { await bounded(exited, 'process cleanup'); }
    finally { reader.close(); await rm(root, { recursive: true, force: true }); }
  });
  async function receive(type) {
    const line = await bounded(messages.next(), type);
    assert.equal(line.done, false, `${type}: process exited before the expected control message`);
    const value = JSON.parse(line.value);
    assert.equal(value.protocolVersion, 1);
    assert.equal(value.type, type);
    assert.equal(value.instanceId, instanceId);
    return value;
  }
  const send = value => child.stdin.write(`${JSON.stringify({ protocolVersion: 1, instanceId, ...value })}\n`);
  send({ type: 'initialize', token });
  const ready = await receive('ready');
  assert.equal(ready.host, '127.0.0.1');
  assert.ok(Number.isInteger(ready.port) && ready.port > 0 && ready.port <= 65535);
  const origin = `http://127.0.0.1:${ready.port}`;
  const request = (route, bearer = token) => fetch(`${origin}${route}`, { headers: bearer === null ? {} : { Authorization: `Bearer ${bearer}` }, signal: AbortSignal.timeout(3000) });
  assert.equal((await request('/_platform/health', null)).status, 401);
  assert.equal((await request('/api/hello', 'disposable-wrong-token')).status, 401);
  const health = await request('/_platform/health');
  assert.equal(health.status, 200);
  assert.deepEqual(await health.json(), { protocolVersion: 1, instanceId, status: 'ok' });
  const hello = await request('/api/hello');
  assert.equal(hello.status, 200);
  assert.ok((await hello.json()).message);
  assert.equal((await request('/index.html')).status, 404, 'static pages belong to the platform gateway');
  assert.equal((await request('/api/not-defined')).status, 404);
  send({ type: 'ping', requestId: 'disposable-health-check' });
  assert.equal((await receive('pong')).requestId, 'disposable-health-check');
  send({ type: 'shutdown' });
  await receive('stopping');
  assert.deepEqual(await bounded(exited, 'graceful shutdown'), { code: 0, signal: null });
  assert.equal(stderr.includes(token), false);
  assert.equal(await readFile(path.join(data, 'preserved-business-data.txt'), 'utf8'), 'disposable existing content');
  await assert.rejects(fetch(`${origin}/_platform/health`, { signal: AbortSignal.timeout(1000) }));
});
