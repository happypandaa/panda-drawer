/** Panda browser SDK, API 1. Load as a normal script from /_platform/sdk.js. */
(function () {
'use strict';
const API_VERSION = 1;
const INT64_MIN = -(1n << 63n);
const INT64_MAX = (1n << 63n) - 1n;

class PlatformError extends Error {
  constructor(code, message, details = {}, retryable = false) {
    super(message);
    this.name = 'PlatformError';
    this.code = code;
    this.details = details;
    this.retryable = retryable;
  }
}

function invalid(message) {
  return new PlatformError('INVALID_ARGUMENT', message, { operationOutcome: 'not_started' });
}

function encodeValue(value) {
  if (value === null || typeof value === 'string') return value;
  if (typeof value === 'boolean') return value ? 1 : 0;
  if (typeof value === 'number') {
    if (!Number.isFinite(value) || (Number.isInteger(value) && !Number.isSafeInteger(value))) {
      throw invalid('SQL numbers must be finite and integers must be safe; use bigint for large integers');
    }
    return value;
  }
  if (typeof value === 'bigint') {
    if (value < INT64_MIN || value > INT64_MAX) throw invalid('SQL bigint is outside signed 64-bit range');
    return { $int64: value.toString() };
  }
  if (value instanceof Uint8Array) {
    let binary = '';
    for (let offset = 0; offset < value.length; offset += 8192) {
      binary += String.fromCharCode(...value.subarray(offset, offset + 8192));
    }
    return { $bytes: btoa(binary) };
  }
  throw invalid('Unsupported SQL value; use null, string, number, bigint, boolean or Uint8Array');
}

function protocol(message) {
  return new PlatformError('INVALID_RESPONSE', message);
}

function decodeValue(value) {
  if (value === null || typeof value === 'string') return value;
  if (typeof value === 'number' && Number.isFinite(value)) {
    if (Number.isInteger(value) && !Number.isSafeInteger(value)) throw protocol('Unencoded unsafe SQL integer');
    return value;
  }
  if (value && typeof value === 'object' && Object.keys(value).length === 1) {
    if (typeof value.$int64 === 'string' && /^-?(0|[1-9][0-9]*)$/.test(value.$int64)) {
      const integer = BigInt(value.$int64);
      if (integer >= INT64_MIN && integer <= INT64_MAX) {
        return integer >= BigInt(Number.MIN_SAFE_INTEGER) && integer <= BigInt(Number.MAX_SAFE_INTEGER)
          ? Number(integer) : integer;
      }
    }
    if (typeof value.$bytes === 'string') {
      try {
        const binary = atob(value.$bytes);
        if (btoa(binary) !== value.$bytes) throw new Error('Noncanonical base64');
        return Uint8Array.from(binary, char => char.charCodeAt(0));
      } catch { throw protocol('Invalid SQL bytes encoding'); }
    }
  }
  throw protocol('Invalid SQL value encoding');
}

function decodeResult(result) {
  if (!result || !Array.isArray(result.columns) || !Array.isArray(result.rows)) {
    throw protocol('Invalid SQL result');
  }
  return {
    ...result,
    rows: result.rows.map(row => Object.fromEntries(Object.entries(row).map(([key, value]) => [key, decodeValue(value)]))),
    ...(result.lastInsertRowid === undefined ? {} : { lastInsertRowid: decodeValue(result.lastInsertRowid) }),
  };
}

function statement(input) {
  if (!input || typeof input.sql !== 'string' || !input.sql.trim()) throw invalid('SQL must be a nonempty string');
  const params = input.params ?? [];
  if (!Array.isArray(params)) throw invalid('SQL params must be an array');
  return { sql: input.sql, params: params.map(encodeValue) };
}

function options(value) {
  if (value === undefined) return {};
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw invalid('SQL options must be an object');
  for (const [key, item] of Object.entries(value)) {
    if (!['maxRows', 'timeoutMs'].includes(key) || !Number.isSafeInteger(item) || item <= 0) {
      throw invalid('SQL options accept positive safe integer maxRows and timeoutMs only');
    }
  }
  return { options: value };
}

function logicalPath(path, allowEmpty = false) {
  if (typeof path !== 'string' || (!path && !allowEmpty) || path.includes('\\') || /[\u0000-\u001f]/.test(path) || path.startsWith('/')) {
    throw invalid('Attachment path must be a relative logical path');
  }
  const segments = allowEmpty && path.endsWith('/') ? path.slice(0, -1).split('/') : path.split('/');
  if (segments.some(part => part === '.' || part === '..' || (!part && path))) {
    throw invalid('Attachment path must not contain empty, . or .. segments');
  }
  return path;
}

/** One client implementation; application code uses the global window.panda. */
function createPlatform(environment = {}) {
  const doc = environment.document ?? globalThis.document;
  const loc = environment.location ?? globalThis.location;
  const win = environment.window ?? globalThis.window;
  const fetcher = environment.fetch ?? globalThis.fetch?.bind(globalThis);
  const Events = environment.EventSource ?? globalThis.EventSource;
  // The authenticated context fixes this document's identity. URL paths and HTML
  // metadata never supply an instance ID or override an established epoch.
  let epoch;
  let projectId;
  let instanceId;
  const origin = loc?.origin;
  let cachedContext;
  let contextPending;

  function endpoint(tail, query = {}) {
    if (!origin || !/^http:\/\/127\.0\.0\.1:\d+$/.test(origin)) {
      throw new PlatformError('PLATFORM_CONTEXT_MISSING', '请从平台打开此项目，或使用平台生成的浏览器入口。');
    }
    const url = new URL(`/_platform/api/v1/${tail}`, origin);
    for (const [key, value] of Object.entries(query)) url.searchParams.set(key, String(value));
    return url;
  }

  async function request(tail, { method = 'GET', body, query, binary = false } = {}) {
    const url = endpoint(tail, query);
    const headers = epoch ? { 'X-Platform-Document-Epoch': epoch } : {};
    if (body !== undefined && !binary) headers['Content-Type'] = 'application/json';
    const mutating = method !== 'GET';
    let response;
    try {
      response = await fetcher(url.href, {
        method, headers, credentials: 'same-origin', cache: 'no-store', redirect: 'error',
        ...(body === undefined ? {} : { body: binary ? body : JSON.stringify(body) }),
      });
    } catch {
      throw new PlatformError('PLATFORM_UNAVAILABLE', '无法连接本地平台，请检查平台是否正在运行。', {
        operationOutcome: mutating ? 'unknown' : 'no_change',
      }, true);
    }
    if (binary && method === 'GET' && response.ok) {
      try { return await response.blob(); }
      catch { throw new PlatformError('PLATFORM_UNAVAILABLE', '附件传输中断，请重新读取。', { operationOutcome: 'no_change' }, true); }
    }
    let envelope;
    try { envelope = await response.json(); }
    catch { throw new PlatformError('INVALID_RESPONSE', '平台返回了无效响应。', { operationOutcome: mutating ? 'unknown' : 'no_change' }); }
    if (envelope?.apiVersion !== API_VERSION) {
      throw new PlatformError('API_VERSION_UNSUPPORTED', '此 SDK 与平台接口版本不兼容。');
    }
    if (tail === 'context' && envelope.ok === true && envelope.projectId != null && envelope.data?.projectId !== envelope.projectId) throw protocol('Context envelope belongs to a different project');
    if (projectId && envelope.projectId != null && envelope.projectId !== projectId) throw protocol('Response belongs to a different project');
    if (!response.ok || envelope.ok !== true) {
      const error = envelope.error;
      if (!error || typeof error.code !== 'string' || typeof error.message !== 'string') throw protocol('Missing error details');
      const failure = new PlatformError(error.code, error.message, error.details ?? {}, error.retryable === true);
      failure.requestId = envelope.requestId;
      throw failure;
    }
    return envelope.data;
  }

  async function getContext() {
    const context = await request('context');
    if (context?.sdkApiVersion !== API_VERSION) throw new PlatformError('API_VERSION_UNSUPPORTED', '此项目需要平台 API v1。');
    if (typeof context.projectId !== 'string' || !context.projectId ||
        typeof context.documentEpoch !== 'string' || !context.documentEpoch ||
        typeof context.instanceId !== 'string' || !context.instanceId || !Array.isArray(context.capabilities)) {
      throw protocol('Invalid project context');
    }
    if (epoch && (context.documentEpoch !== epoch || context.projectId !== projectId || context.instanceId !== instanceId)) {
      throw new PlatformError('PROJECT_RELOAD_REQUIRED', '应用会话已变化，请保存未提交内容后重新打开。');
    }
    epoch ??= context.documentEpoch;
    projectId ??= context.projectId;
    instanceId ??= context.instanceId;
    if (context.state?.reloadRequired) throw new PlatformError('PROJECT_RELOAD_REQUIRED', '项目已更新，请保存未提交内容后重新打开。');
    if (context.state?.mode && context.state.mode !== 'normal') throw new PlatformError('PROJECT_MAINTENANCE', '项目正在维护，请稍后重试。', { state: context.state }, true);
    cachedContext = context;
    // Return a copy: application mutations must not change later capability checks.
    return structuredClone(context);
  }

  async function getConfiguration() {
    // Configuration works for ordinary pages with the SDK disabled. It must not
    // request a context or create a database as a prerequisite.
    const url = new URL('/_platform/config.json', endpoint('context').origin);
    let response;
    try {
      response = await fetcher(url.href, { method: 'GET', credentials: 'same-origin', cache: 'no-store', redirect: 'error' });
    } catch {
      throw new PlatformError('PLATFORM_UNAVAILABLE', '无法读取应用配置，请检查平台是否正在运行。', { operationOutcome: 'no_change' }, true);
    }
    let values;
    try { values = await response.json(); }
    catch { throw protocol('Invalid configuration response'); }
    if (!response.ok) {
      const error = values?.error;
      if (!error || typeof error.code !== 'string' || typeof error.message !== 'string') throw protocol('Missing configuration error details');
      const failure = new PlatformError(error.code, error.message, error.details ?? {}, error.retryable === true);
      failure.requestId = values.requestId;
      throw failure;
    }
    if (!values || typeof values !== 'object' || Array.isArray(values)) throw protocol('Configuration must be an object');
    return values;
  }

  async function capability(name) {
    if (!cachedContext) {
      contextPending ??= getContext().finally(() => { contextPending = undefined; });
      await contextPending;
    }
    if (!cachedContext.capabilities.includes(name)) {
      throw new PlatformError('CAPABILITY_UNSUPPORTED', `平台不支持 ${name}。`, { capability: name });
    }
  }

  function subscribe(listener) {
    if (typeof listener !== 'function') throw invalid('Subscription listener must be a function');
    let closed = false;
    let events;
    let checking = false;
    const emit = event => { if (!closed) listener(event); };
    const terminal = new Set(['PROJECT_RELOAD_REQUIRED', 'PROJECT_MAINTENANCE', 'UNAUTHORIZED', 'INVALID_SESSION', 'SESSION_EXPIRED', 'PROJECT_NOT_FOUND', 'API_VERSION_UNSUPPORTED', 'CAPABILITY_UNSUPPORTED', 'SDK_NOT_ENABLED']);
    const failure = error => {
      emit({ type: 'error', error });
      if (terminal.has(error.code)) { events?.close(); events = undefined; }
    };
    const revalidate = async source => {
      if (closed || checking) return;
      checking = true;
      try {
        await getContext();
        await capability('events');
        emit({ type: 'data.changed', projectId, source });
      } catch (error) { failure(error); }
      finally { checking = false; }
    };
    const focus = () => { void revalidate('focus'); };
    const visibility = () => { if (doc?.visibilityState === 'visible') void revalidate('visible'); };
    win?.addEventListener('focus', focus);
    doc?.addEventListener('visibilitychange', visibility);
    void (async () => {
      try {
        await capability('events');
        if (closed) return;
        if (!Events) throw new PlatformError('CAPABILITY_UNSUPPORTED', '此浏览器不支持 EventSource。');
        events = new Events(endpoint('events', { documentEpoch: epoch }).href, { withCredentials: true });
        events.addEventListener('open', () => { void revalidate('reconnect'); });
        events.addEventListener('error', () => { void revalidate('connection'); });
        events.addEventListener('data.changed', message => {
          try {
            const data = JSON.parse(message.data);
            if (data.projectId !== projectId) throw protocol('Event belongs to a different project');
            emit({ ...data, type: 'data.changed' });
          } catch (error) { failure(error instanceof PlatformError ? error : protocol('Malformed event')); }
        });
        events.addEventListener('project.state.changed', () => { void revalidate('state'); });
        events.addEventListener('project.updated', message => {
          try {
            const data = JSON.parse(message.data);
            if (data.projectId !== projectId) throw protocol('Event belongs to a different project');
            emit({ ...data, type: 'project.updated' });
          } catch (error) { failure(error instanceof PlatformError ? error : protocol('Malformed event')); }
        });
      } catch (error) { failure(error); }
    })();
    return () => {
      closed = true;
      events?.close();
      win?.removeEventListener('focus', focus);
      doc?.removeEventListener('visibilitychange', visibility);
    };
  }

  return Object.freeze({
    PlatformError,
    config: getConfiguration,
    context: Object.freeze({ get: getContext }),
    db: Object.freeze({
      async execute(input) {
        const body = { ...statement(input), ...options(input.options) };
        await capability('db.execute');
        return decodeResult(await request('db/execute', { method: 'POST', body }));
      },
      async transaction(input) {
        if (!input || !Array.isArray(input.statements) || input.statements.length === 0) throw invalid('Transaction needs statements');
        const body = { statements: input.statements.map(statement), ...options(input.options) };
        await capability('db.transaction');
        const results = await request('db/transaction', { method: 'POST', body });
        if (!Array.isArray(results)) throw protocol('Invalid transaction result');
        return results.map(decodeResult);
      },
      async schema() { await capability('db.schema'); return request('db/schema'); },
      subscribe,
    }),
    files: Object.freeze({
      async put({ path, content, overwrite = false }) {
        logicalPath(path);
        if (!(content instanceof Blob) && !(content instanceof Uint8Array)) throw invalid('File content must be Blob or Uint8Array');
        if (typeof overwrite !== 'boolean') throw invalid('overwrite must be boolean');
        await capability('files');
        const size = content instanceof Blob ? content.size : content.byteLength;
        const limit = cachedContext.limits?.fileUploadMaxBytes;
        if (!Number.isSafeInteger(limit) || limit <= 0) throw protocol('Missing attachment upload limit');
        if (size > limit) throw new PlatformError('FILE_TOO_LARGE', '附件超过当前平台限制。', { limit, operationOutcome: 'not_started' });
        return request('file', { method: 'PUT', binary: true, body: content, query: { path, overwrite } });
      },
      async get(path) { logicalPath(path); await capability('files'); return request('file', { binary: true, query: { path } }); },
      async list(prefix = '') { logicalPath(prefix, true); await capability('files'); return request('files', { query: { prefix } }); },
      async remove(path) { logicalPath(path); await capability('files'); return request('file', { method: 'DELETE', query: { path } }); },
      async url(path) { logicalPath(path); await capability('files'); return endpoint('file', { path, documentEpoch: epoch }).href; },
    }),
  });
}

if (!globalThis.panda) {
  Object.defineProperty(globalThis, 'panda', { value: createPlatform(), enumerable: true });
}
})();
