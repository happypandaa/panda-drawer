import assert from 'node:assert/strict';
import test from 'node:test';
import { readFile } from 'node:fs/promises';
import ts from 'typescript';

const source = await readFile(new URL('../src/download-route-policy.ts', import.meta.url), 'utf8');
const compiled = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext } }).outputText;
const { DownloadRouteController, DOWNLOAD_ROUTE_KEY, RouteDownloadError, guessDownloadRoute } = await import('data:text/javascript;base64,' + Buffer.from(compiled).toString('base64'));
const day = 24 * 60 * 60 * 1000;
function fixture(saved, probe = async () => {}) {
  const values = new Map(saved ? [[DOWNLOAD_ROUTE_KEY, JSON.stringify(saved)]] : []);
  const storage = { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value) };
  const calls = [];
  const controller = new DownloadRouteController({
    storage, probe: route => { calls.push(route); return probe(route); }, now: () => 10 * day,
    systemRoute: () => 'cn',
  });
  return { controller, calls, values };
}
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

test('Automatic fallback combines the system language with the mainland time zone', () => {
  assert.equal(guessDownloadRoute('zh-CN', 'Asia/Shanghai'), 'cn');
  assert.equal(guessDownloadRoute('zh-Hans', 'China Standard Time'), 'cn');
  for (const input of [['zh-TW', 'Asia/Shanghai'], ['zh-CN', 'America/New_York'], ['en', 'Asia/Shanghai']]) {
    assert.equal(guessDownloadRoute(...input), 'global');
  }
});

test('The first valid probe wins and a late probe cannot overwrite its saved result', async () => {
  const cn = deferred(), global = deferred();
  const f = fixture(null, route => route === 'cn' ? cn.promise : global.promise);
  const selected = f.controller.getRoute();
  assert.deepEqual(f.calls, ['cn', 'global']);
  global.resolve();
  assert.equal(await selected, 'global');
  cn.resolve();
  await Promise.resolve();
  assert.equal(f.controller.getSnapshot().detectedRoute, 'global');
  assert.equal(JSON.parse(f.values.get(DOWNLOAD_ROUTE_KEY)).detectedAt, 10 * day);
});

test('Failed probes preserve a previous result and otherwise use the system fallback', async () => {
  const probe = async () => { throw new Error('unavailable or invalid JSON'); };
  const previous = fixture({ mode: 'auto', detectedRoute: 'global', detectedAt: day }, probe);
  assert.equal(await previous.controller.detect(), 'global');
  assert.equal(previous.controller.getSnapshot().detectedAt, day);
  assert.equal(await fixture(null, probe).controller.getRoute(), 'cn');
});

test('An expired result is returned immediately while detection runs in the background', async () => {
  const cn = deferred(), global = deferred();
  const f = fixture({ mode: 'auto', detectedRoute: 'global', detectedAt: day }, route => route === 'cn' ? cn.promise : global.promise);
  assert.equal(await f.controller.getRoute(), 'global');
  assert.equal(f.controller.getSnapshot().detecting, true);
  cn.resolve();
  assert.equal(await f.controller.detect(), 'cn');
  global.reject(new Error('late failure'));
});

test('Manual selection disables detection and revokes a previous automatic result', async () => {
  const waiting = deferred();
  const f = fixture(null, () => waiting.promise);
  const pending = f.controller.getRoute();
  f.controller.setMode('global');
  waiting.resolve();
  assert.equal(await pending, 'global');
  assert.equal(f.controller.getSnapshot().detectedRoute, null);
  assert.equal(await f.controller.detect(), 'global');
  assert.equal(f.calls.length, 2);
});

test('Automatic failure retries the other source once and manual failure retains a switchable route', async () => {
  const f = fixture({ mode: 'auto', detectedRoute: 'global', detectedAt: 10 * day });
  const attempts = [];
  assert.equal(await f.controller.run(async route => {
    attempts.push(route);
    if (route === 'global') throw new Error('download error');
    return 'cached catalog';
  }), 'cached catalog');
  assert.deepEqual(attempts, ['global', 'cn']);
  assert.equal(f.controller.getSnapshot().detectedRoute, 'cn');
  f.controller.setMode('global');
  attempts.length = 0;
  await assert.rejects(f.controller.run(async route => { attempts.push(route); throw new Error('failed'); }),
    error => error instanceof RouteDownloadError && error.route === 'global' && error.mode === 'global');
  assert.deepEqual(attempts, ['global']);
});
