// Disposable DOM fixtures validate modal ownership only. They do not operate
// the native application or read or save any project configuration.
import assert from 'node:assert/strict';
import { after, afterEach, test } from 'node:test';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';
import { JSDOM } from 'jsdom';

const repo = fileURLToPath(new URL('../../../', import.meta.url));
await mkdir(path.join(repo, '.test-artifacts'), { recursive: true });
const output = await mkdtemp(path.join(repo, '.test-artifacts/modal-dom-'));
const sourcePath = path.join(repo, 'apps/desktop/src/Modal.tsx');
const compiledPath = path.join(output, 'Modal.mjs');
await writeFile(compiledPath, ts.transpileModule(await readFile(sourcePath, 'utf8'), {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX }, fileName: sourcePath,
}).outputText);
const dom = new JSDOM('<!doctype html><html><body></body></html>', { url: 'http://localhost/', pretendToBeVisual: true });
for (const name of ['window', 'document', 'Node', 'HTMLElement', 'HTMLInputElement', 'Event', 'MouseEvent', 'KeyboardEvent', 'FocusEvent', 'MutationObserver']) Object.defineProperty(globalThis, name, { value: name === 'window' ? dom.window : dom.window[name], configurable: true });
Object.defineProperty(globalThis, 'navigator', { value: dom.window.navigator, configurable: true });
globalThis.getComputedStyle = dom.window.getComputedStyle.bind(dom.window);
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
const { createElement: h, act, StrictMode, useState } = await import('react');
const { createRoot } = await import('react-dom/client');
const { Modal } = await import(pathToFileURL(compiledPath).href);
let root;
afterEach(async () => { if (root) await act(async () => root.unmount()); root = null; });
after(async () => { dom.window.close(); await rm(output, { recursive: true, force: true }); });

async function setup(tree) {
  document.body.innerHTML = '<button id="opener">打开配置</button><aside id="already-inert" inert></aside><div id="root"></div>';
  document.body.style.overflow = 'auto';
  document.getElementById('opener').focus();
  root = createRoot(document.getElementById('root'));
  await act(async () => root.render(tree));
}
function dialog(title) {
  return [...document.querySelectorAll('[role="dialog"]')].find(element => document.getElementById(element.getAttribute('aria-labelledby'))?.textContent === title);
}
function layer(title) { return dialog(title)?.closest('.modal-layer'); }
async function click(element) { assert.ok(element); await act(async () => { element.focus(); element.click(); }); }
async function key(key, options = {}) {
  const event = new window.KeyboardEvent('keydown', { key, bubbles: true, cancelable: true, ...options });
  await act(async () => document.activeElement.dispatchEvent(event));
  return event;
}
async function change(element, value) {
  await act(async () => {
    Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set.call(element, value);
    element.dispatchEvent(new window.Event('input', { bubbles: true }));
  });
}
function assertRestored() {
  assert.equal(document.body.style.overflow, 'auto');
  assert.equal(document.getElementById('root').hasAttribute('inert'), false);
  assert.equal(document.getElementById('opener').hasAttribute('inert'), false);
  assert.equal(document.getElementById('already-inert').hasAttribute('inert'), true);
  assert.equal(document.activeElement, document.getElementById('opener'));
}

function NestedFixture({ initialHelp = false, onParentClose = () => {} }) {
  const [open, setOpen] = useState(true);
  const [help, setHelp] = useState(initialHelp);
  const [more, setMore] = useState(false);
  const [draft, setDraft] = useState('');
  return open && h(Modal, { title: '应用配置', onClose() { onParentClose(); setOpen(false); } },
    h('input', { 'aria-label': '人工配置', value: draft, onChange: event => setDraft(event.currentTarget.value) }),
    h('button', { id: 'info', type: 'button', onClick: () => setHelp(true) }, '字段说明'),
    help && h(Modal, { title: '字段说明', onClose: () => setHelp(false) },
      h('p', null, '人工说明，不含密钥。'),
      h('button', { id: 'more', type: 'button', onClick: () => setMore(true) }, '更多说明'),
      more && h(Modal, { title: '更多说明', onClose: () => setMore(false) }, h('button', { id: 'last', type: 'button' }, '查看示例'))));
}

test('Nested help owns Escape and returns focus without closing or clearing the configuration draft', async () => {
  let parentClosed = 0;
  await setup(h(NestedFixture, { onParentClose: () => parentClosed++ }));
  const input = document.querySelector('[aria-label="人工配置"]');
  await change(input, 'disposable unsaved configuration');
  await click(document.getElementById('info'));
  assert.equal(layer('应用配置').hasAttribute('inert'), true);
  assert.equal(dialog('应用配置').getAttribute('aria-modal'), 'false');
  assert.equal(layer('字段说明').hasAttribute('inert'), false);
  assert.equal(dialog('字段说明').getAttribute('aria-modal'), 'true');
  assert.equal(document.activeElement, dialog('字段说明').querySelector('h2'));
  assert.equal((await key('Escape')).defaultPrevented, true);
  assert.equal(parentClosed, 0);
  assert.equal(dialog('字段说明'), undefined);
  assert.equal(document.activeElement, document.getElementById('info'));
  assert.equal(input.value, 'disposable unsaved configuration');
  assert.equal(layer('应用配置').hasAttribute('inert'), false);
  assert.equal(document.body.style.overflow, 'hidden');
  await key('Escape');
  assert.equal(parentClosed, 1);
  assertRestored();
});

test('Only the top modal traps Tab and focus, and background close buttons and backdrops are ignored', async () => {
  await setup(h(NestedFixture));
  await click(document.getElementById('info'));
  const help = dialog('字段说明');
  const first = help.querySelector('button');
  const last = document.getElementById('more');
  assert.equal((await key('Tab')).defaultPrevented, true);
  assert.equal(document.activeElement, first);
  await key('Tab', { shiftKey: true });
  assert.equal(document.activeElement, last);
  await key('Tab');
  assert.equal(document.activeElement, first);
  await act(async () => document.querySelector('[aria-label="人工配置"]').focus());
  assert.equal(document.activeElement, help.querySelector('h2'));
  await click(dialog('应用配置').querySelector('.modal-close'));
  await click(layer('应用配置'));
  assert.ok(dialog('应用配置'));
  assert.ok(dialog('字段说明'));
  await click(help.querySelector('.modal-close'));
  assert.ok(dialog('应用配置'));
  assert.equal(dialog('字段说明'), undefined);
});

test('Three nested dialogs close in order with the correct layer and return focus', async () => {
  await setup(h(NestedFixture));
  await click(document.getElementById('info'));
  await click(document.getElementById('more'));
  assert.ok(Number(layer('更多说明').style.zIndex) > Number(layer('字段说明').style.zIndex));
  assert.ok(Number(layer('字段说明').style.zIndex) > Number(layer('应用配置').style.zIndex));
  await key('Escape');
  assert.equal(dialog('更多说明'), undefined);
  assert.equal(document.activeElement, document.getElementById('more'));
  assert.equal(layer('字段说明').hasAttribute('inert'), false);
  assert.equal(layer('应用配置').hasAttribute('inert'), true);
  await key('Escape');
  assert.equal(document.activeElement, document.getElementById('info'));
  await key('Escape');
  assertRestored();
});

test('An initially mounted child stays above its parent despite child-first React effects', async () => {
  await setup(h(NestedFixture, { initialHelp: true }));
  assert.equal(document.activeElement, dialog('字段说明').querySelector('h2'));
  assert.equal(layer('应用配置').hasAttribute('inert'), true);
  assert.equal(layer('字段说明').hasAttribute('inert'), false);
  assert.ok(Number(layer('字段说明').style.zIndex) > Number(layer('应用配置').style.zIndex));
  await key('Escape');
  assert.ok(dialog('应用配置'));
  assert.equal(dialog('字段说明'), undefined);
  await key('Escape');
  assertRestored();
});

test('Removing a lower independent modal preserves the remaining top focus and page lock', async () => {
  function Tree({ parent = true, child = true }) {
    return h('div', null,
      parent && h(Modal, { title: '下层', onClose() {} }, h('button', null, '下层操作')),
      child && h(Modal, { title: '上层', onClose() {} }, h('button', { id: 'top-action' }, '上层操作')));
  }
  await setup(h(Tree));
  await act(async () => document.getElementById('top-action').focus());
  await act(async () => root.render(h(Tree, { parent: false })));
  assert.equal(document.activeElement, document.getElementById('top-action'));
  assert.equal(document.body.style.overflow, 'hidden');
  assert.equal(document.getElementById('root').hasAttribute('inert'), true);
  assert.equal(layer('上层').hasAttribute('inert'), false);
  await act(async () => root.render(h(Tree, { parent: false, child: false })));
  assertRestored();
});

test('Dynamically added background stays inert until the final modal closes', async () => {
  await setup(h(NestedFixture));
  const background = document.createElement('button');
  background.textContent = '人工背景按钮';
  const preserved = document.createElement('aside');
  preserved.setAttribute('inert', '');
  await act(async () => { document.body.append(background, preserved); });
  assert.equal(background.hasAttribute('inert'), true);
  await click(document.getElementById('info'));
  await key('Escape');
  assert.equal(background.hasAttribute('inert'), true);
  await key('Escape');
  assert.equal(background.hasAttribute('inert'), false);
  assert.equal(preserved.hasAttribute('inert'), true);
  assertRestored();
});

test('Only the top close guard runs and closeDisabled uses its latest value', async () => {
  let parentGuard = 0;
  let childGuard = 0;
  let childClosed = 0;
  function Tree({ disabled = true }) {
    return h(Modal, { title: '父级', beforeClose() { parentGuard++; return false; }, onClose() {} },
      h(Modal, { title: '子级', closeDisabled: disabled, beforeClose() { childGuard++; return false; }, onClose() { childClosed++; } }));
  }
  await setup(h(Tree));
  await key('Escape');
  assert.equal(parentGuard, 0);
  assert.equal(childGuard, 0);
  await act(async () => root.render(h(Tree, { disabled: false })));
  await key('Escape');
  assert.equal(parentGuard, 0);
  assert.equal(childGuard, 1);
  assert.equal(childClosed, 0);
});

test('StrictMode and complete nested unmount restore the original page without a leftover focus trap', async () => {
  await setup(h(StrictMode, null, h(NestedFixture, { initialHelp: true })));
  assert.equal(layer('字段说明').hasAttribute('inert'), false);
  await act(async () => root.unmount());
  root = null;
  assertRestored();
  const input = document.createElement('input');
  document.body.append(input);
  input.focus();
  assert.equal(document.activeElement, input);
  assert.equal((await key('Escape')).defaultPrevented, false);
});
