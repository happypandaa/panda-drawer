// Disposable DOM fixtures only. No workspace data, services, or native windows.
import assert from 'node:assert/strict';
import { after, afterEach, test } from 'node:test';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';
import { JSDOM } from 'jsdom';

const repo = fileURLToPath(new URL('../../../', import.meta.url));
await mkdir(path.join(repo, '.test-artifacts'), { recursive: true });
const output = await mkdtemp(path.join(repo, '.test-artifacts/launcher-dom-'));
const compiled = new Map();
async function compile(file) {
  if (compiled.has(file)) return compiled.get(file);
  const target = path.join(output, `${path.basename(file).replace(/\.tsx?$/, '')}.mjs`);
  compiled.set(file, target);
  let source = ts.transpileModule(await readFile(file, 'utf8'), { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX }, fileName: file }).outputText;
  for (const match of [...source.matchAll(/from\s+(['"])(\.[^'"]+)\1/g)]) {
    const base = path.resolve(path.dirname(file), match[2]);
    let dependency;
    for (const candidate of [base, `${base}.tsx`, `${base}.ts`]) {
      try { await readFile(candidate); dependency = candidate; break; } catch { /* Try the next extension. */ }
    }
    assert.ok(dependency);
    source = source.replaceAll(`${match[1]}${match[2]}${match[1]}`, JSON.stringify(pathToFileURL(await compile(dependency)).href));
  }
  await writeFile(target, source);
  return target;
}

const dom = new JSDOM('<!doctype html><html><body></body></html>', { url: 'http://localhost/', pretendToBeVisual: true });
for (const name of ['window', 'document', 'Node', 'HTMLElement', 'Event', 'MouseEvent', 'KeyboardEvent', 'FocusEvent']) Object.defineProperty(globalThis, name, { value: name === 'window' ? dom.window : dom.window[name], configurable: true });
Object.defineProperty(globalThis, 'navigator', { value: dom.window.navigator, configurable: true });
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
const { createElement: h, act } = await import('react');
const { createRoot } = await import('react-dom/client');
const { ProjectCard, NodeProjectCard, launcherMenuPosition } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/ProjectCard.tsx'))).href);
let root;
afterEach(async () => { if (root) await act(async () => root.unmount()); root = null; });
after(async () => { dom.window.close(); await rm(output, { recursive: true, force: true }); });

const fixture = { id: 'prj_disposable_launcher', name: 'My actual app', description: 'An artificial description', projectRoot: '/disposable/project', documentEpoch: 'fixture', state: 'ready', issues: [], manifest: null };
const nodeFixture = { id: 'node_disposable_launcher', name: 'My Node app', sourcePath: '/disposable/source', runtime: 'node-service', state: 'stopped', restartRequired: false, runningRevision: null, lastError: null, configuration: { revision: 1, values: {}, secretStatus: {}, missingRequired: [], canStart: true }, fields: [] };
const defaults = { blocked: false, opening: false, deletingDisabled: false, onEnter() {}, onDetails() {}, onDelete() {} };
async function render(Component = ProjectCard, props = {}) {
  if (!root) { document.body.innerHTML = '<div id="root"></div><button id="outside">Outside</button>'; root = createRoot(document.getElementById('root')); }
  await act(async () => root.render(h(Component, { ...defaults, ...(Component === ProjectCard ? { project: fixture } : { app: nodeFixture }), ...props })));
}
async function click(element) { assert.ok(element); await act(async () => element.click()); }
async function key(element, value, extra = {}) { await act(async () => element.dispatchEvent(new window.KeyboardEvent('keydown', { key: value, bubbles: true, cancelable: true, ...extra }))); }
const more = () => document.querySelector('.launcher-more');
const enter = () => document.querySelector('.launcher-open');
const menu = () => document.querySelector('[role="menu"]');

// Native button activation supplies Enter/Space behavior in the browser. These
// DOM tests verify callbacks and menu navigation; they do not emulate WebView.
test('Launcher preserves real names and custom project icons with a native entry button', async () => {
  let enters = 0;
  const iconDataUrl = 'data:image/png;base64,artificial';
  await render(ProjectCard, { project: { ...fixture, iconDataUrl }, onEnter: () => enters++ });
  assert.equal(enter().tagName, 'BUTTON');
  assert.equal(enter().getAttribute('aria-label'), '进入项目「My actual app」');
  assert.equal(document.querySelector('.launcher-name').textContent, fixture.name);
  assert.equal(document.querySelector('.launcher-icon img').getAttribute('src'), iconDataUrl);
  assert.equal(document.querySelector('.launcher-node-badge'), null);
  assert.match(document.getElementById(enter().getAttribute('aria-describedby')).textContent, /网页项目/);
  assert.equal(document.querySelector('.launcher-statuses').textContent, '');
  await click(enter());
  assert.equal(enters, 1);
});

test('Default icon and color stay tied to stable project identity across renames', async () => {
  await render();
  const color = document.querySelector('.launcher-icon').className;
  const icon = document.querySelector('.launcher-icon svg').outerHTML;
  await render(ProjectCard, { project: { ...fixture, name: 'A renamed app' } });
  assert.equal(document.querySelector('.launcher-icon').className, color);
  assert.equal(document.querySelector('.launcher-icon svg').outerHTML, icon);
  assert.equal(document.querySelector('.launcher-name').textContent, 'A renamed app');
});

test('Menu has a keyboard entry, arrow navigation, Escape closing and focus return', async () => {
  await render();
  const trigger = more();
  assert.equal(trigger.getAttribute('aria-label'), '项目「My actual app」的更多操作');
  assert.equal(trigger.getAttribute('aria-haspopup'), 'menu');
  await key(trigger, 'ArrowDown');
  assert.equal(trigger.getAttribute('aria-expanded'), 'true');
  assert.equal(menu().getAttribute('aria-label'), '项目「My actual app」操作');
  assert.match(menu().textContent, /网页项目/);
  assert.equal(document.activeElement.getAttribute('aria-label'), '查看项目「My actual app」的详情');
  await key(document.activeElement, 'ArrowDown');
  assert.equal(document.activeElement.getAttribute('aria-label'), '删除项目「My actual app」');
  await key(document.activeElement, 'ArrowDown');
  assert.equal(document.activeElement.getAttribute('aria-label'), '查看项目「My actual app」的详情');
  await key(document.activeElement, 'End');
  assert.equal(document.activeElement.getAttribute('aria-label'), '删除项目「My actual app」');
  await key(document.activeElement, 'Home');
  assert.equal(document.activeElement.getAttribute('aria-label'), '查看项目「My actual app」的详情');
  await key(document.activeElement, 'Escape');
  assert.equal(menu(), null);
  assert.equal(document.activeElement, trigger);
  assert.equal(trigger.getAttribute('aria-expanded'), 'false');
});

test('Menu focuses its first action only after positioning makes it visible', async () => {
  const originalFocus = window.HTMLElement.prototype.focus;
  const hiddenAttempts = [];
  // Browsers ignore focus requests on visibility:hidden elements; jsdom does
  // not model that restriction, so reproduce it for this timing regression.
  window.HTMLElement.prototype.focus = function (...args) {
    if (this.closest('.launcher-menu')?.style.visibility === 'hidden') {
      hiddenAttempts.push(this);
      return;
    }
    return originalFocus.apply(this, args);
  };
  try {
    await render();
    await act(async () => more().focus());
    await click(more());
    assert.equal(menu().style.visibility, 'visible');
    assert.equal(hiddenAttempts.length, 0);
    assert.equal(document.activeElement.getAttribute('aria-label'), '查看项目「My actual app」的详情');
  } finally {
    window.HTMLElement.prototype.focus = originalFocus;
  }
});

test('Context menu, outside pointer and outside focus all close without invoking an action', async () => {
  let actions = 0;
  await render(ProjectCard, { onDetails: () => actions++, onDelete: () => actions++ });
  await act(async () => document.querySelector('article').dispatchEvent(new window.MouseEvent('contextmenu', { bubbles: true, cancelable: true })));
  assert.ok(menu());
  await act(async () => document.getElementById('outside').dispatchEvent(new window.Event('pointerdown', { bubbles: true })));
  assert.equal(menu(), null);
  await key(enter(), 'F10', { shiftKey: true });
  assert.ok(menu());
  await act(async () => document.getElementById('outside').focus());
  assert.equal(menu(), null);
  assert.equal(actions, 0);
});

test('Menu delegates details and deletion only to existing parent callbacks', async () => {
  const calls = [];
  await render(ProjectCard, { onEnter: () => calls.push('enter'), onDetails: () => calls.push('details'), onDelete: () => calls.push('delete') });
  await click(more());
  await click(document.querySelector('[aria-label="查看项目「My actual app」的详情"]'));
  assert.equal(menu(), null);
  assert.equal(document.activeElement, more());
  assert.deepEqual(calls, ['details']);
  await click(more());
  await click(document.querySelector('[aria-label="删除项目「My actual app」"]'));
  assert.equal(menu(), null);
  assert.deepEqual(calls, ['details', 'delete']);
});

test('Maintenance and task statuses remain distinct, and each action retains its own gate', async () => {
  let enters = 0;
  await render(ProjectCard, { blocked: true, detailsDisabled: true, onEnter: () => enters++, status: { mode: 'maintenance', recoveryRequired: false }, task: { final: false } });
  assert.equal(enter().disabled, true);
  assert.match(document.querySelector('.launcher-statuses').textContent, /维护中/);
  assert.match(document.querySelector('.launcher-statuses').textContent, /任务进行中/);
  await click(enter());
  assert.equal(enters, 0);
  assert.equal(more().disabled, false);
  await click(more());
  assert.equal(document.querySelector('[aria-label="查看项目「My actual app」的详情"]').disabled, true);
  assert.equal(document.querySelector('[aria-label="删除项目「My actual app」"]').disabled, false);
  assert.equal(document.activeElement.getAttribute('aria-label'), '删除项目「My actual app」');
  await render(ProjectCard, { detailsDisabled: true, deletingDisabled: true });
  assert.equal(more().disabled, true);
  assert.equal(menu(), null);
  await key(enter(), 'ContextMenu');
  assert.equal(menu(), null);
});

test('Unbuilt and recovery projects keep their visible state and cannot enter', async () => {
  await render(ProjectCard, { project: { ...fixture, state: 'unbuilt' } });
  assert.equal(enter().disabled, true);
  assert.match(document.querySelector('.launcher-statuses').textContent, /等待构建/);
  await render(ProjectCard, { project: { ...fixture, state: 'error' }, status: { mode: 'maintenance', recoveryRequired: true } });
  assert.equal(enter().disabled, true);
  assert.match(document.querySelector('.launcher-statuses').textContent, /恢复待处理/);
});

test('Node hexagon stays visible when stopped or failed; missing configuration can still enter', async () => {
  let enters = 0;
  await render(NodeProjectCard, { onEnter: () => enters++ });
  assert.equal(document.querySelector('.launcher-node-badge').getAttribute('src'), '/ui/node-mark.png');
  assert.equal(document.querySelector('.launcher-node-badge').getAttribute('alt'), '');
  assert.match(document.getElementById(enter().getAttribute('aria-describedby')).textContent, /Node 服务项目/);
  assert.equal(document.querySelector('.launcher-statuses').textContent, '');
  await render(NodeProjectCard, { app: { ...nodeFixture, state: 'failed', lastError: 'Artificial service failure', configuration: { ...nodeFixture.configuration, canStart: false, missingRequired: ['KEY'] } }, onEnter: () => enters++ });
  assert.ok(document.querySelector('.launcher-node-badge'));
  assert.match(document.querySelector('.launcher-statuses').textContent, /需要检查/);
  assert.match(document.querySelector('.launcher-statuses').textContent, /需要配置/);
  assert.equal(enter().disabled, false);
  await click(enter());
  assert.equal(enters, 1);
  await click(more());
  assert.match(menu().textContent, /Node 服务项目/);
  assert.equal(menu().querySelectorAll('[role="menuitem"]').length, 1);
  assert.doesNotMatch(menu().textContent, /删除项目/);
});

test('Pending Node entry disables opening and details without hiding configuration status', async () => {
  await render(NodeProjectCard, { opening: true, app: { ...nodeFixture, configuration: { ...nodeFixture.configuration, canStart: false } } });
  assert.equal(enter().disabled, true);
  assert.equal(more().disabled, true);
  assert.match(document.querySelector('.launcher-statuses').textContent, /需要配置/);
  assert.match(document.querySelector('.launcher-statuses').textContent, /进入中/);
});

test('A bottom-row menu flips above its trigger and fits the compact viewport', () => {
  const result = launcherMenuPosition({ top: 448, bottom: 472, right: 632 }, { width: 190, height: 130 }, { width: 640, height: 480 });
  assert.equal(result.placement, 'top');
  assert.equal(result.left, 442);
  assert.equal(result.top, 312);
  assert.ok(result.top >= 8);
  assert.ok(result.top + 130 < 448);
  assert.ok(result.left + 190 <= 632);
});

test('Edge menus stay in view and long menus receive room to scroll', () => {
  const leftEdge = launcherMenuPosition({ top: 20, bottom: 44, right: 20 }, { width: 190, height: 130 }, { width: 640, height: 480 });
  assert.equal(leftEdge.placement, 'bottom');
  assert.equal(leftEdge.left, 8);
  assert.equal(leftEdge.top, 50);
  const long = launcherMenuPosition({ top: 220, bottom: 244, right: 740 }, { width: 800, height: 900 }, { width: 640, height: 480 });
  assert.equal(long.left, 8);
  assert.equal(long.maxWidth, 624);
  assert.equal(long.top, 250);
  assert.equal(long.maxHeight, 222);
  assert.ok(long.top + long.maxHeight <= 472);
});

test('Menus render outside the scrolling tile, retain inside interactions and close on viewport changes', async () => {
  await render();
  await click(more());
  assert.equal(menu().parentElement, document.body);
  assert.equal(menu().style.position, 'fixed');
  assert.equal(menu().style.right, 'auto');
  assert.equal(menu().style.overflowY, 'auto');
  assert.equal(menu().style.visibility, 'visible');
  await act(async () => menu().dispatchEvent(new window.Event('pointerdown', { bubbles: true })));
  assert.ok(menu());
  await act(async () => menu().dispatchEvent(new window.Event('scroll')));
  assert.ok(menu(), 'scrolling a long menu must not dismiss it');
  await act(async () => document.getElementById('root').dispatchEvent(new window.Event('scroll')));
  assert.equal(menu(), null);
  assert.equal(document.activeElement, more());
  await click(more());
  await act(async () => window.dispatchEvent(new window.Event('resize')));
  assert.equal(menu(), null);
  assert.equal(document.activeElement, more());
});
