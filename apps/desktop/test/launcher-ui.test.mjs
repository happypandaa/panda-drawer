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
const { ProjectCard, launcherMenuPosition } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/ProjectCard.tsx'))).href);
let root;
afterEach(async () => { if (root) await act(async () => root.unmount()); root = null; });
after(async () => { dom.window.close(); await rm(output, { recursive: true, force: true }); });

const fixture = { id: 'prj_disposable_launcher', name: 'My actual app', description: 'An artificial description', projectRoot: '/disposable/project', documentEpoch: 'fixture', state: 'ready', issues: [], manifest: null };
const nodeFixture = { id: 'node_disposable_launcher', name: 'My Node app', sourcePath: '/disposable/source', runtime: 'node', state: 'stopped', restartRequired: false, runningRevision: null, lastError: null, configuration: { revision: 1, values: {}, secretStatus: {}, missingRequired: [], canStart: true }, fields: [] };
const defaults = { blocked: false, opening: false, deletingDisabled: false, onEnter() {}, onDetails() {}, onDelete() {} };
async function render(Component = ProjectCard, props = {}) {
  if (!root) { document.body.innerHTML = '<div id="root"></div><button id="outside">Outside</button>'; root = createRoot(document.getElementById('root')); }
  await act(async () => root.render(h(Component, { ...defaults, ...(Component === ProjectCard ? { project: fixture } : { app: nodeFixture }), ...props })));
}
async function click(element) { assert.ok(element); await act(async () => element.click()); }
async function key(element, value, extra = {}) { await act(async () => element.dispatchEvent(new window.KeyboardEvent('keydown', { key: value, bubbles: true, cancelable: true, ...extra }))); }
const tile = () => document.querySelector('.launcher-tile');
const description = () => document.getElementById(enter().getAttribute('aria-describedby')).textContent;
const openMenu = () => key(enter().disabled ? tile() : enter(), 'ContextMenu');
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
  assert.match(document.getElementById(enter().getAttribute('aria-describedby')).textContent, /应用/);
  assert.equal(document.querySelector('.launcher-statuses'), null);
  assert.equal(document.querySelector('.launcher-more'), null);
  await click(enter());
  assert.equal(enters, 1);
});

test('Default monogram follows the name while its color stays tied to stable project identity', async () => {
  await render();
  const color = document.querySelector('.launcher-icon').className;
  assert.equal(document.querySelector('.launcher-monogram').textContent, 'M');
  await render(ProjectCard, { project: { ...fixture, name: 'a renamed app' } });
  assert.equal(document.querySelector('.launcher-icon').className, color);
  assert.equal(document.querySelector('.launcher-monogram').textContent, 'A');
  assert.equal(document.querySelector('.launcher-name').textContent, 'a renamed app');
  await render(ProjectCard, { project: { ...fixture, name: '  打僵尸' } });
  assert.equal(document.querySelector('.launcher-monogram').textContent, '打');
  await render(ProjectCard, { project: { ...fixture, name: '12 号应用' } });
  assert.equal(document.querySelector('.launcher-monogram').textContent, '12');
});

test('Menu has a keyboard entry, arrow navigation, Escape closing and focus return', async () => {
  await render();
  const trigger = enter();
  await act(async () => trigger.focus());
  assert.equal(tile().getAttribute('aria-haspopup'), 'menu');
  await key(trigger, 'F10', { shiftKey: true });
  assert.equal(tile().getAttribute('aria-expanded'), 'true');
  assert.equal(menu().getAttribute('aria-label'), '项目「My actual app」操作');
  assert.match(menu().textContent, /应用/);
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
  assert.equal(tile().getAttribute('aria-expanded'), 'false');
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
    await act(async () => enter().focus());
    await openMenu();
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
  await openMenu();
  await act(async () => enter().dispatchEvent(new window.Event('pointerdown', { bubbles: true })));
  assert.equal(menu(), null, 'pressing the same app icon dismisses its context menu');
  await openMenu();
  await act(async () => enter().focus());
  assert.equal(menu(), null, 'returning focus to the same app also dismisses its menu');
  assert.equal(actions, 0);
});

test('Menu delegates details and deletion only to existing parent callbacks', async () => {
  const calls = [];
  await render(ProjectCard, { onEnter: () => calls.push('enter'), onDetails: () => calls.push('details'), onDelete: () => calls.push('delete') });
  await openMenu();
  await click(document.querySelector('[aria-label="查看项目「My actual app」的详情"]'));
  assert.equal(menu(), null);
  assert.equal(document.activeElement, enter());
  assert.deepEqual(calls, ['details']);
  await openMenu();
  await click(document.querySelector('[aria-label="删除项目「My actual app」"]'));
  assert.equal(menu(), null);
  assert.deepEqual(calls, ['details', 'delete']);
});

test('Maintenance and task statuses remain distinct, and each action retains its own gate', async () => {
  let enters = 0;
  await render(ProjectCard, { blocked: true, detailsDisabled: true, onEnter: () => enters++, status: { mode: 'maintenance', recoveryRequired: false }, task: { type: 'build', status: 'running', final: false } });
  assert.equal(enter().disabled, true);
  assert.match(description(), /维护中/);
  assert.match(description(), /任务进行中/);
  assert.equal(document.querySelector('.launcher-statuses'), null);
  assert.match(tile().title, /维护中.*任务进行中/);
  await click(enter());
  assert.equal(enters, 0);
  assert.equal(tile().tabIndex, 0);
  await openMenu();
  assert.equal(document.querySelector('[aria-label="查看项目「My actual app」的详情"]').disabled, true);
  assert.equal(document.querySelector('[aria-label="删除项目「My actual app」"]').disabled, false);
  assert.equal(document.activeElement.getAttribute('aria-label'), '删除项目「My actual app」');
  await render(ProjectCard, { detailsDisabled: true, deletingDisabled: true });
  assert.equal(tile().tabIndex, -1);
  assert.equal(menu(), null);
  await key(enter(), 'ContextMenu');
  assert.equal(menu(), null);
});

test('A background space scan has consistent accessible and visible status without blocking entry, and clears when final', async () => {
  let enters = 0;
  const task = { taskId: 'disposable_space_scan', projectId: fixture.id, type: 'storageScan', status: 'running', final: false, phase: 'scanning', result: null, error: null };
  await render(ProjectCard, { task, onEnter: () => enters++ });
  assert.equal(enter().disabled, false);
  assert.equal(enter().getAttribute('aria-busy'), 'false');
  for (const text of [description(), tile().title, enter().title]) {
    assert.match(text, /正在统计空间/);
    assert.doesNotMatch(text, /任务进行中/);
  }
  await click(enter());
  assert.equal(enters, 1);
  await openMenu();
  assert.equal(menu().querySelector('.launcher-menu-status').textContent, '正在统计空间');
  for (const status of ['succeeded', 'failed', 'interrupted']) {
    await render(ProjectCard, { task: { ...task, status, final: true } });
    assert.doesNotMatch(description(), /正在统计空间|任务进行中/);
    assert.doesNotMatch(tile().title, /正在统计空间|任务进行中/);
    assert.equal(menu().querySelector('.launcher-menu-status'), null);
  }
});

test('Space scanning preserves maintenance and recovery warnings and their entry gates', async () => {
  for (const [status, warning] of [[{ mode: 'maintenance', recoveryRequired: false }, '维护中'], [{ mode: 'normal', recoveryRequired: true }, '恢复待处理']]) {
    await render(ProjectCard, { blocked: true, status, task: { type: 'storageScan', status: 'queued', final: false } });
    assert.equal(enter().disabled, true);
    for (const text of [description(), tile().title, enter().title]) {
      assert.ok(text.includes(warning));
      assert.match(text, /正在统计空间/);
      assert.doesNotMatch(text, /任务进行中/);
    }
    await openMenu();
    assert.equal(menu().querySelector('.launcher-menu-status').textContent, `${warning} · 正在统计空间`);
    assert.equal(document.querySelector('[aria-label="查看项目「My actual app」的详情"]').disabled, false);
  }
});

test('Unbuilt and recovery projects keep accessible status, keyboard details and cannot enter', async () => {
  await render(ProjectCard, { project: { ...fixture, state: 'needsBuild' } });
  assert.equal(enter().disabled, true);
  assert.match(description(), /需要构建/);
  assert.equal(tile().tabIndex, 0);
  await act(async () => tile().focus());
  await key(tile(), 'F10', { shiftKey: true });
  assert.ok(menu());
  assert.match(menu().textContent, /需要构建/);
  await key(document.activeElement, 'Escape');
  assert.equal(document.activeElement, tile());
  await render(ProjectCard, { project: { ...fixture, state: 'error' }, status: { mode: 'maintenance', recoveryRequired: true } });
  assert.equal(enter().disabled, true);
  assert.match(description(), /恢复待处理/);
});

test('Apps with a backend use the same launcher and configuration entry as apps without one', async () => {
 let enters = 0;
 await render(ProjectCard, {project: {...fixture, hasBackend: true, state: 'needsConfiguration'}, onEnter: () => enters++});
 assert.equal(enter().disabled, false); assert.match(description(), /需要配置/); await click(enter()); assert.equal(enters,1);
 await openMenu(); assert.equal(menu().querySelectorAll('[role="menuitem"]').length,2); assert.doesNotMatch(menu().textContent,/Node 服务项目|网页项目/);
});
test('A pending unified app entry disables actions while retaining accessible configuration status', async () => {
 await render(ProjectCard, {opening: true, detailsDisabled: true, project: {...fixture,hasBackend: true,state:'needsConfiguration'}});
 assert.equal(enter().disabled,true); assert.match(description(),/需要配置/); assert.match(description(),/进入中/);
});
