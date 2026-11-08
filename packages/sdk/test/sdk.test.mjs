import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { Script } from 'node:vm';
const script = new Script(await readFile(new URL('../sdk.js', import.meta.url), 'utf8'));
function loadSdk(environment) {
  const globals = { ...environment, URL, Uint8Array, Blob, btoa, atob, structuredClone };
  script.runInNewContext(globals);
  return globals.panda;
}

const caps = ['db.execute', 'db.transaction', 'db.schema', 'files', 'events'];
const baseContext = { projectId: 'prj_test', projectRoot: '/disposable/apps/current', name: 'Test', sdkApiVersion: 1, instanceId: 'instance', documentEpoch: 'original-epoch', capabilities: caps,
  limits: { fileUploadMaxBytes: 256 }, state: { mode: 'normal', busy: false, reloadRequired: false } };
const result = { columns: [{ name: 'id' }], rows: [{ id: 1 }], changes: 0, changed: false };
function envelope(data, extra = {}, status = 200) {
  return new Response(JSON.stringify({ apiVersion: 1, ok: true, requestId: 'request', projectId: 'prj_test', data, ...extra }), { status });
}
function harness(handler = () => envelope(result), context = baseContext) {
  const calls = [];
  const meta = { content: 'original-epoch' };
  const document = new EventTarget();
  document.visibilityState = 'visible';
  document.querySelector = () => meta;
  const window = new EventTarget();
  const streams = [];
  class Events extends EventTarget {
    constructor(url, options) { super(); this.url = url; this.options = options; this.closed = false; streams.push(this); }
    close() { this.closed = true; }
    emit(type, data) { const event = new Event(type); event.data = JSON.stringify(data); this.dispatchEvent(event); }
  }
  const environment = { document, window, location: { pathname: '/tools/nested/page.html', origin: 'http://127.0.0.1:1234' }, EventSource: Events,
    fetch: async (url, init) => {
      const call = { url: new URL(url), ...init }; calls.push(call);
      if (call.url.pathname.endsWith('/context')) return envelope(typeof context === 'function' ? context() : context);
      return handler(call);
    } };
  return { platform: loadSdk(environment), calls, meta, streams, document, window, environment };
}
const settle = () => new Promise(resolve => setImmediate(resolve));

test('SQL values are lossless and tags are decoded only at SQL value positions', async () => {
  const h = harness(call => {
    assert.deepEqual(JSON.parse(call.body).params, [null, 'hello', 3.5, 1, 0, { $int64: '9223372036854775807' }, { $bytes: 'AAH/' }]);
    return envelope({ ...result, rows: [{ big: { $int64: '-9223372036854775808' }, blob: { $bytes: 'AAH/' }, ordinary: '{"$int64":"9007199254740993"}' }], lastInsertRowid: { $int64: '9007199254740993' } });
  });
  const value = await h.platform.db.execute({ sql: 'SELECT ?', params: [null, 'hello', 3.5, true, false, 9223372036854775807n, Uint8Array.of(0, 1, 255)] });
  assert.equal(value.rows[0].big, -9223372036854775808n);
  assert.deepEqual(value.rows[0].blob, Uint8Array.of(0, 1, 255));
  assert.equal(value.rows[0].ordinary, '{"$int64":"9007199254740993"}');
  assert.equal(value.lastInsertRowid, 9007199254740993n);
  assert.equal(h.calls[1].credentials, 'same-origin');
  assert.equal(h.calls[1].redirect, 'error');
});

test('invalid numbers, integer ranges, objects and options fail before any HTTP request', async () => {
  const h = harness();
  for (const value of [NaN, Infinity, Number.MAX_SAFE_INTEGER + 1, 9223372036854775808n, -9223372036854775809n, undefined, { $int64: '3' }]) {
    await assert.rejects(h.platform.db.execute({ sql: 'SELECT ?', params: [value] }), { code: 'INVALID_ARGUMENT' });
  }
  await assert.rejects(h.platform.db.execute({ sql: 'SELECT 1', options: { timeoutMs: -1 } }), { code: 'INVALID_ARGUMENT' });
  assert.equal(h.calls.length, 0);
});

test('transaction sends one ordered request and decodes each result', async () => {
  const h = harness(call => {
    assert.deepEqual(JSON.parse(call.body), { statements: [{ sql: 'INSERT ...', params: [{ $int64: '9' }] }, { sql: 'SELECT 1', params: [] }], options: { maxRows: 20 } });
    return envelope([result, { ...result, rows: [{ id: { $int64: '9' } }] }]);
  });
  const response = await h.platform.db.transaction({ statements: [{ sql: 'INSERT ...', params: [9n] }, { sql: 'SELECT 1' }], options: { maxRows: 20 } });
  assert.equal(response[1].rows[0].id, 9);
  assert.equal(h.calls.filter(call => call.method === 'POST').length, 1);
});

test('context and error details are not recursively decoded and context cannot be mutated by consumers', async () => {
  const h = harness(() => envelope(null, { ok: false, error: { code: 'SQL_ERROR', message: 'bad query', details: { marker: { $int64: '9007199254740993' }, operationOutcome: 'rolled_back' }, retryable: false } }, 400), { ...baseContext, name: 'name', marker: { $bytes: 'AQ==' } });
  const context = await h.platform.context.get();
  assert.deepEqual(context.marker, { $bytes: 'AQ==' });
  context.capabilities.length = 0;
  await assert.rejects(h.platform.db.execute({ sql: 'SELECT x' }), error => {
    assert.ok(error instanceof h.platform.PlatformError);
    assert.equal(error.code, 'SQL_ERROR');
    assert.deepEqual(error.details.marker, { $int64: '9007199254740993' });
    assert.equal(error.requestId, 'request');
    return true;
  });
});

test('context fixes identity without parsing the URL or requiring injected metadata', async () => {
  let context = { ...baseContext };
  const h = harness(undefined, () => context);
  h.document.querySelector = () => null;
  await h.platform.context.get();
  await h.platform.db.execute({ sql: 'SELECT 1' });
  const url = await h.platform.files.url('中文/with ? & #.txt');
  assert.equal(new URL(url).pathname, '/_platform/api/v1/file');
  assert.equal(new URL(url).searchParams.get('documentEpoch'), 'original-epoch');
  assert.equal(new URL(url).searchParams.get('path'), '中文/with ? & #.txt');
  assert.equal(h.calls[0].headers['X-Platform-Document-Epoch'], undefined);
  assert.equal(h.calls[1].headers['X-Platform-Document-Epoch'], 'original-epoch');
  context = { ...baseContext, documentEpoch: 'replacement-epoch' };
  await assert.rejects(h.platform.context.get(), { code: 'PROJECT_RELOAD_REQUIRED' });
});

test('context preserves the current application root in its native path format', async () => {
  for (const projectRoot of ['/Users/disposable/应用 folder', String.raw`C:\Users\disposable\应用 folder`]) {
    const h = harness(undefined, { ...baseContext, projectRoot });
    const context = await h.platform.context.get();
    assert.equal(context.projectRoot, projectRoot);
    assert.equal(context.workspaceRoot, undefined);
    context.projectRoot = '/consumer/changed';
    assert.equal((await h.platform.context.get()).projectRoot, projectRoot);
  }
});

test('missing capabilities or unsupported API versions never issue the requested operation', async () => {
  const h = harness(undefined, { ...baseContext, capabilities: ['files'] });
  await assert.rejects(h.platform.db.execute({ sql: 'SELECT 1' }), { code: 'CAPABILITY_UNSUPPORTED' });
  assert.equal(h.calls.length, 1);
  const old = harness(undefined, { ...baseContext, sdkApiVersion: 2 });
  await assert.rejects(old.platform.context.get(), { code: 'API_VERSION_UNSUPPORTED' });
  const wrong = harness(() => envelope(result, { projectId: 'prj_other' }));
  await assert.rejects(wrong.platform.db.execute({ sql: 'SELECT 1' }), { code: 'INVALID_RESPONSE' });
});

test('file reads, atomic upload request, paths and limits use documented forms', async () => {
  const h = harness(call => {
    if (call.method === 'GET') return new Response('hello', { headers: { 'content-type': 'text/plain' } });
    return envelope({ path: 'notes/a.txt', size: 5, mediaType: 'text/plain', modifiedAt: 123 });
  });
  const file = await h.platform.files.put({ path: 'notes/a.txt', content: new Blob(['hello']) });
  assert.equal(file.size, 5);
  assert.equal(h.calls[1].url.searchParams.get('overwrite'), 'false');
  assert.ok(h.calls[1].body instanceof Blob);
  assert.equal(await (await h.platform.files.get('notes/a.txt')).text(), 'hello');
  await assert.rejects(h.platform.files.put({ path: 'huge.txt', content: new Uint8Array(257) }), { code: 'FILE_TOO_LARGE' });
  for (const path of ['/tmp/a', '../a', 'a/../b', 'a//b', 'a\\b', '\u0000']) {
    await assert.rejects(h.platform.files.url(path), { code: 'INVALID_ARGUMENT' });
  }
  const listing = harness(() => envelope([]));
  await listing.platform.files.list('notes/');
  assert.equal(listing.calls[1].url.searchParams.get('prefix'), 'notes/');
});

test('network errors do not replay a possibly committed write', async () => {
  const h = harness(() => { throw new TypeError('disconnected'); });
  await assert.rejects(h.platform.db.execute({ sql: 'INSERT ...' }), error => {
    assert.equal(error.code, 'PLATFORM_UNAVAILABLE');
    assert.equal(error.details.operationOutcome, 'unknown');
    return true;
  });
  assert.equal(h.calls.filter(call => call.method === 'POST').length, 1);
});

test('subscription verifies context before reconnect/focus refresh, forwards updates and cleans up', async () => {
  const h = harness();
  const events = [];
  const stop = h.platform.db.subscribe(event => events.push(event));
  await settle();
  assert.equal(new URL(h.streams[0].url).searchParams.get('documentEpoch'), 'original-epoch');
  h.streams[0].emit('open');
  await settle();
  assert.equal(events[0].source, 'reconnect');
  h.streams[0].emit('data.changed', { projectId: 'prj_test', source: 'sql' });
  h.streams[0].emit('project.updated', { projectId: 'prj_test', source: 'build', taskId: 'task-1' });
  assert.equal(events.at(-1).type, 'project.updated');
  h.window.dispatchEvent(new Event('focus'));
  await settle();
  assert.equal(events.at(-1).source, 'focus');
  const count = h.calls.length;
  stop();
  assert.ok(h.streams[0].closed);
  h.window.dispatchEvent(new Event('focus'));
  await settle();
  assert.equal(h.calls.length, count);
});

test('stale sessions stop SSE retry and signal error without reloading or replaying requests', async () => {
  let stale = false;
  const h = harness(undefined, () => ({ ...baseContext, state: { ...baseContext.state, reloadRequired: stale } }));
  const values = [];
  const stop = h.platform.db.subscribe(event => values.push(event));
  await settle();
  stale = true;
  h.streams[0].emit('error');
  await settle();
  assert.ok(h.streams[0].closed);
  assert.equal(values.at(-1).error.code, 'PROJECT_RELOAD_REQUIRED');
  assert.equal(h.calls.filter(call => call.method !== 'GET').length, 0);
  stop();
});

test('missing local platform origin fails before touching fetch', async () => {
  const h = harness();
  const sdk = loadSdk({ ...h.environment, location: { origin: 'https://outside.example', pathname: '/' } });
  await assert.rejects(sdk.context.get(), { code: 'PLATFORM_CONTEXT_MISSING' });
  assert.equal(h.calls.length, 0);
});

test('typed public configuration works without context or enabled data SDK', async () => {
  const calls = [];
  const sdk = loadSdk({ location: { origin: 'http://127.0.0.1:1234', pathname: '/' }, fetch: async (url, init) => {
    calls.push({ url, init });
    return new Response(JSON.stringify({ theme: 'dark', count: 4, enabled: false }));
  } });
  assert.deepEqual(await sdk.config(), { theme: 'dark', count: 4, enabled: false });
  assert.equal(calls.length, 1);
  assert.equal(calls[0].url, 'http://127.0.0.1:1234/_platform/config.json');
  assert.equal(calls[0].init.credentials, 'same-origin');
});

test('disabled SDK is an explicit service error and never requests SQL', async () => {
  const calls = [];
  const sdk = loadSdk({ location: { origin: 'http://127.0.0.1:1234', pathname: '/' }, fetch: async url => {
    calls.push(url);
    return envelope(null, { ok: false, error: { code: 'SDK_NOT_ENABLED', message: '这个应用没有启用平台 SDK' } }, 403);
  } });
  await assert.rejects(sdk.db.execute({ sql: 'SELECT 1' }), { code: 'SDK_NOT_ENABLED' });
  assert.deepEqual(calls, ['http://127.0.0.1:1234/_platform/api/v1/context']);
});

test('malformed SQL wire tags are rejected instead of rounded or guessed', async () => {
  for (const value of [{ $int64: '9223372036854775808' }, { $bytes: '!' }, { $int64: '3', extra: true }, 9007199254740992]) {
    const h = harness(() => envelope({ ...result, rows: [{ value }] }));
    await assert.rejects(h.platform.db.execute({ sql: 'SELECT value' }), { code: 'INVALID_RESPONSE' });
  }
});

test('ai.open posts the text once and refuses calls outside a user click', async () => {
  const { platform, calls } = harness(() => envelope({ requested: true }), { ...baseContext, capabilities: [...caps, 'ai.open'] });
  assert.deepEqual(await platform.ai.open('请在这里工作'), { requested: true });
  const post = calls.find(call => call.url.pathname.endsWith('/ai/open'));
  assert.equal(post.method, 'POST');
  assert.deepEqual(JSON.parse(post.body), { text: '请在这里工作' });
  await assert.rejects(platform.ai.open(''), { code: 'INVALID_ARGUMENT' });
  await assert.rejects(platform.ai.open('字'.repeat(2001)), { code: 'INVALID_ARGUMENT' });

  const inactive = harness(() => envelope({ requested: true }), { ...baseContext, capabilities: [...caps, 'ai.open'] });
  const script = new Script(await readFile(new URL('../sdk.js', import.meta.url), 'utf8'));
  const globals = { ...inactive.environment, navigator: { userActivation: { isActive: false } }, URL, Uint8Array, Blob, btoa, atob, structuredClone };
  script.runInNewContext(globals);
  await assert.rejects(globals.panda.ai.open('请在这里工作'), { code: 'USER_ACTIVATION_REQUIRED' });
  assert.equal(inactive.calls.length, 0);

  const old = harness(() => envelope({ requested: true }));
  await assert.rejects(old.platform.ai.open('请在这里工作'), { code: 'CAPABILITY_UNSUPPORTED' });
  assert.ok(!old.calls.some(call => call.url.pathname.endsWith('/ai/open')));
});
