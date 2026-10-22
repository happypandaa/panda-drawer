// Dependency-free regression checks for the platform-injected status notice.
// This simulates DOM/events and does not replace real desktop/browser validation.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

// Resolve from this script, so callers do not need a particular working directory.
const sourceUrl = new URL('../crates/platform-core/src/runtime_notice.rs', import.meta.url);
const source = fs.readFileSync(sourceUrl, 'utf8');
const embeddedScript = source.match(/r#"([\s\S]*)"#\s*\}/)?.[1];
assert.ok(embeddedScript, 'runtime_notice.rs must expose its embedded JavaScript');

class Element {
  constructor(tag) {
    this.tag = tag;
    this.children = [];
    this.listeners = {};
    this.hidden = false;
    this.textContent = '';
  }
  append(...children) { this.children.push(...children); }
  attachShadow() {
    this.shadow = new Element('shadow');
    return this.shadow;
  }
  setAttribute(name, value) { this[name] = value; }
  addEventListener(name, handler) { this.listeners[name] = handler; }
  focus() { this.focused = true; }
}

function descendants(element) {
  return [element, ...element.children.flatMap(descendants)];
}

const requests = [];
const streams = [];
const documentEvents = {};
const windowEvents = {};
const platformEvents = [];
let runtimePolicyFingerprint = "initial-policy";
const body = new Element('body');
const draft = new Element('textarea');
draft.value = 'Unsaved project draft: keep this content.';
body.append(draft);
let state = { mode: 'normal', busy: false, reloadRequired: false };
let responseError = null;
let epoch = 'original-document';
let reloads = 0;
let nativeConfirmCalls = 0;

function forbiddenNativeConfirm() {
  nativeConfirmCalls += 1;
  throw new Error('Native confirmation dialogs are unavailable in this environment');
}

const context = {
  URL, AbortController, setTimeout, clearTimeout, Date, Error, JSON,
  setInterval:()=>1, clearInterval:()=>{},
  CustomEvent: class { constructor(type,options){this.type=type;this.detail=options.detail;} },
  document: {
    querySelector: selector => ({content:selector.includes("platform-runtime-policy")?"initial-policy":epoch}),
    currentScript: { nonce: 'test-nonce' },
    body,
    readyState: 'complete',
    visibilityState: 'visible',
    createElement: (tag) => new Element(tag),
    addEventListener: (name, handler) => { documentEvents[name] = handler; },
  },
  window: {
    dispatchEvent: event => platformEvents.push(event),
    addEventListener: (name, handler) => { windowEvents[name] = handler; },
    confirm: forbiddenNativeConfirm,
  },
  location: {
    pathname: '/p/project_test/',
    origin: 'http://127.0.0.1:12345',
    reload: () => { reloads += 1; },
  },
  confirm: forbiddenNativeConfirm,
  fetch: async (url, options) => {
    requests.push({ url: String(url), options });
    return {
      ok: !responseError,
      status: responseError === 'INVALID_SESSION' ? 401 : responseError ? 409 : 200,
      json: async () => responseError ? ({ ok: false, error: { code: responseError } }) : ({
        ok: true, apiVersion: 1,
        data: { projectId: 'project_test', state: { ...state }, runtimePolicyFingerprint },
      }),
    };
  },
  EventSource: class {
    constructor(url) {
      this.url = String(url);
      this.events = {};
      streams.push(this);
    }
    addEventListener(name, handler) { this.events[name] = handler; }
    close() { this.closed = true; }
  },
};

vm.runInNewContext(embeddedScript, context, { filename: 'runtime_notice.js', timeout: 1000 });
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
await flush();
const host = body.children.find((element) => element.id === 'local-projects-platform-status');
assert.ok(host?.shadow, 'the platform notice must appear in its own shadow DOM');
const elements = descendants(host.shadow);
const find = (predicate, description) => {
  const element = elements.find(predicate);
  assert.ok(element, `missing ${description}`);
  return element;
};
const style = find((element) => element.tag === 'style', 'notice styles');
const heading = find((element) => element.className === 'heading', 'status heading');
const detail = find((element) => element.className === 'detail', 'status explanation');
const confirmation = find((element) => element.role === 'group', 'inline confirmation');
const button = (label) => find((element) => element.tag === 'button' && element.textContent === label, label);
const retry = button('刷新状态');
const reload = button('处理草稿后重新加载');
const cancelReload = button('取消，保留当前页面');
const confirmReload = button('已处理草稿，确认重新加载');
const warning = find((element) => element.tag === 'p' && element.textContent.includes('丢弃当前页面未保存的内容'), 'draft warning');

function assertDraftPreserved() {
  assert.ok(body.children.includes(draft), 'the existing project DOM must remain attached');
  assert.equal(draft.value, 'Unsaved project draft: keep this content.');
}
async function changeState(next) {
  state = next;
  streams.at(-1).events['project.state.changed']({
    data: JSON.stringify({ projectId: 'project_test', ...state }),
  });
  await flush();
}

assert.equal(host.hidden, true);
assert.equal(style.nonce, 'test-nonce');
await changeState({ mode: 'normal', busy: true, reloadRequired: false });
assert.match(heading.textContent, /正在备份/);
assert.match(detail.textContent, /不会自动重试/);
assertDraftPreserved();
await changeState({ mode: 'maintenance', busy: false, reloadRequired: false });
assert.match(heading.textContent, /正在维护/);
assert.equal(reload.hidden, true);
assertDraftPreserved();

// A changed global meta value must never update this document's captured epoch.
epoch = 'new-global-epoch';
await changeState({ mode: 'normal', busy: false, reloadRequired: true });
assert.match(heading.textContent, /需要重新加载/);
assert.equal(reload.hidden, false);
confirmReload.listeners.click();
assert.equal(reloads, 0, 'the final action cannot bypass the first step');
reload.listeners.click();
assert.equal(reloads, 0, 'opening confirmation must not reload');
assert.equal(confirmation.hidden, false);
assert.equal(cancelReload.focused, true);
assert.match(warning.textContent, /丢弃当前页面未保存的内容/);
cancelReload.listeners.click();
assert.equal(reloads, 0);
assert.equal(confirmation.hidden, true);
assert.equal(reload.hidden, false);
assert.equal(reload.focused, true);
assertDraftPreserved();

reload.listeners.click();
confirmation.listeners.keydown({ key: 'Escape', preventDefault() {}, stopPropagation() {} });
assert.equal(confirmation.hidden, true);
assert.equal(reloads, 0);
assertDraftPreserved();

// Re-entering maintenance invalidates an already open reload confirmation.
reload.listeners.click();
await changeState({ mode: 'maintenance', busy: false, reloadRequired: true });
assert.equal(confirmation.hidden, true);
confirmReload.listeners.click();
assert.equal(reloads, 0);
assertDraftPreserved();
await changeState({ mode: 'normal', busy: false, reloadRequired: true });
reload.listeners.click();
assert.equal(reloads, 0);
confirmReload.listeners.click();
assert.equal(reloads, 1, 'only the explicit final confirmation may reload');
assert.equal(nativeConfirmCalls, 0, 'the notice must not depend on native confirm dialogs');

state = { mode: 'normal', busy: false, reloadRequired: false };
retry.listeners.click();
await flush();
assert.match(heading.textContent, /需要重新加载/, 'reload requirement stays latched for this document');
// Lifecycle notices have priority over a stale reload requirement and never
// reload or replay a write, even when a confirmation was already open.
reload.listeners.click();
await changeState({ mode: 'normal', reloadRequired: true, workspaceMigrating: true });
assert.match(heading.textContent, /工作区正在迁移/);
assert.equal(confirmation.hidden, true);
confirmReload.listeners.click();
assert.equal(reloads, 1);
assertDraftPreserved();
await changeState({ mode: 'normal', reloadRequired: true, trashed: true });
assert.match(heading.textContent, /项目已删除/);
assert.equal(reload.hidden, true);
assertDraftPreserved();
await changeState({ mode: 'normal', reloadRequired: true, instanceChanged: true });
assert.match(heading.textContent, /服务已切换/);
assert.equal(reload.hidden, true);
assertDraftPreserved();
for (const [code, titlePattern] of [
  ['WORKSPACE_MIGRATING', /工作区正在迁移/],
  ['PROJECT_TRASHED', /项目已删除/],
  ['SERVICE_INSTANCE_CHANGED', /服务已切换/],
  ['INVALID_SESSION', /连接已失效/],
]) {
  responseError = null;
  await changeState({ mode: 'normal', busy: false, reloadRequired: true });
  responseError = code;
  retry.listeners.click();
  await flush();
  assert.match(heading.textContent, titlePattern, `error envelope ${code} must be visible`);
  assert.equal(reload.hidden, true);
  reload.listeners.click();
  confirmReload.listeners.click();
  assert.equal(reloads, 1, 'old instances, trash, and migration cannot reload the old URL');
  assertDraftPreserved();
}
responseError = null;
await changeState({ mode: 'normal', trashed: true, reloadRequired: true });
responseError = 'INVALID_SESSION';
retry.listeners.click();
await flush();
assert.match(heading.textContent, /连接已失效/, 'an expired session cannot keep claiming an old trash state');
assertDraftPreserved();
responseError = null;
state = { mode: 'normal', reloadRequired: false };
runtimePolicyFingerprint = 'changed-policy';
retry.listeners.click();
await flush();
assert.match(heading.textContent,/运行需求已变化/);
assert.equal(reloads,1,'new network/device policy must never silently reload drafts');
assert.ok(platformEvents.some(event=>event.type==='platform:state'&&event.detail.workspaceMigrating));
assert.ok(platformEvents.some(event=>event.type==='platform:state'&&event.detail.expired));
assert.equal(platformEvents.at(-1).detail.reloadRequired,true,'media helpers receive a stop signal on policy changes');
assertDraftPreserved();
windowEvents.pagehide();
assert.equal(streams[0].closed, true);
windowEvents.pageshow({ persisted: true });
await flush();
assert.equal(streams.length, 2, 'back/forward cache restoration must reconnect state events');
for (const { url, options } of requests) {
  assert.equal(new URL(url).pathname, '/p/project_test/api/v1/context');
  assert.equal(options.method, 'GET');
  assert.equal(options.headers['X-Platform-Document-Epoch'], 'original-document');
  assert.equal(options.body, undefined, 'notice checks must never replay business writes');
}
for (const stream of streams) {
  assert.equal(new URL(stream.url).searchParams.get('documentEpoch'), 'original-document');
}
assertDraftPreserved();
console.log(JSON.stringify({
  passed: true,
  checks: [
    'backup and maintenance status',
    'inline confirmation, cancellation and Escape preserve drafts',
    'only final confirmation reloads without native dialogs',
    'new maintenance cancels pending reload',
    'trash, migration pause and old instance notices via events and errors',
    'immutable document epoch',
    'read-only requests',
    'back/forward cache reconnection',
    'runtime policy changes preserve drafts and signal media shutdown',
  ],
  requests: requests.length,
}));
