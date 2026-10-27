// Artificial prompt/DOM fixtures only. No actual user source, external AI,
// clipboard integration, native folder picker or installed Codex is accessed.
import assert from 'node:assert/strict';
import { after, afterEach, test } from 'node:test';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';
import { JSDOM } from 'jsdom';

const repo = fileURLToPath(new URL('../../../', import.meta.url));
await mkdir(path.join(repo, '.test-artifacts'), { recursive: true });
const output = await mkdtemp(path.join(repo, '.test-artifacts/disposable-project-ai-help-dom-'));
const mock = path.join(output, 'tauri-mock.mjs');
await writeFile(mock, 'export const invoke = (...args) => globalThis.__PROJECT_AI_HELP_INVOKE__(...args);\n');
const compiled = new Map();
async function compile(file) {
  if (compiled.has(file)) return compiled.get(file);
  const target = path.join(output, `${path.basename(file).replace(/\.tsx?$/, '')}.mjs`);
  compiled.set(file, target);
  let source = ts.transpileModule(await readFile(file, 'utf8'), { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX }, fileName: file }).outputText;
  for (const match of [...source.matchAll(/from\s+(['"])(\.[^'"]+)\1/g)]) {
    const base = path.resolve(path.dirname(file), match[2]); let dependency;
    for (const candidate of [base, `${base}.tsx`, `${base}.ts`]) {
      try { await readFile(candidate); dependency = candidate; break; } catch { /* Next extension. */ }
    }
    assert.ok(dependency, `Missing dependency ${base}`);
    source = source.replaceAll(`${match[1]}${match[2]}${match[1]}`, JSON.stringify(pathToFileURL(await compile(dependency)).href));
  }
  source = source.replaceAll(/(['"])@tauri-apps\/api\/core\1/g, JSON.stringify(pathToFileURL(mock).href));
  await writeFile(target, source); return target;
}

const dom = new JSDOM('<!doctype html><html><body></body></html>', { url: 'http://localhost/', pretendToBeVisual: true });
for (const name of ['window', 'document', 'Node', 'HTMLElement', 'HTMLInputElement', 'HTMLSelectElement', 'Event', 'MouseEvent', 'KeyboardEvent', 'MutationObserver']) Object.defineProperty(globalThis, name, { value: name === 'window' ? dom.window : dom.window[name], configurable: true });
Object.defineProperty(globalThis, 'navigator', { value: dom.window.navigator, configurable: true });
globalThis.getComputedStyle = dom.window.getComputedStyle.bind(dom.window);
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
const { createElement: h, act } = await import('react');
const { createRoot } = await import('react-dom/client');
const { AiProjectHelp } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/AiProjectHelp.tsx'))).href);
const { buildProjectAiPrompt, checkedProjectReviewExcerpt, MAX_REVIEW_BYTES } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/project-ai-help.ts'))).href);
const { PROJECT_AI_REQUIREMENTS } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/project-ai-requirements.ts'))).href);
let root;
afterEach(async () => { if (root) await act(async () => root.unmount()); root = null; globalThis.__PROJECT_AI_HELP_INVOKE__ = undefined; });
after(async () => { dom.window.close(); await rm(output, { recursive: true, force: true }); });
const button = label => [...document.querySelectorAll('button')].find(value => value.textContent.trim() === label);
async function click(value) { assert.ok(value); await act(async () => value.click()); }
function deferred() { let resolve; const promise = new Promise(yes => { resolve = yes; }); return { promise, resolve }; }
async function setup(context = { mode: 'evaluation', sourceKind: 'local' }, overrides = {}) {
  const calls = []; const copies = [];
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async value => { copies.push(value); } } });
  globalThis.__PROJECT_AI_HELP_INVOKE__ = async (command, args) => {
    calls.push([command, args]);
    if (overrides[command]) return overrides[command](args);
    if (command === 'source_review_capabilities') return { codexAvailable: false };
    throw new Error(`Unexpected artificial native command: ${command}`);
  };
  document.body.innerHTML = '<div id="root"></div>'; root = createRoot(document.getElementById('root'));
  let props = { context };
  async function render(next = {}) { props = { ...props, ...next }; await act(async () => root.render(h(AiProjectHelp, props))); }
  await render();
  return { calls, copies, render };
}

test('The bundled platform requirements exactly match the maintained copyable document', async () => {
  const document = await readFile(path.join(repo, 'docs/project-compatibility-review-prompt.md'), 'utf8');
  const prompt = document.match(/^```text\n([\s\S]*?)\n```/m)[1];
  assert.equal(PROJECT_AI_REQUIREMENTS, prompt.slice(prompt.indexOf('【平台要求：')));
});

test('A Git failure prompt preserves source identity and delivery while removing known secret log formats', () => {
  const prompt = buildProjectAiPrompt({ mode: 'add-failed', sourceKind: 'git', url: 'https://github.com/disposable/artificial.git', branch: 'example', commit: 'a'.repeat(40), checkedAt: '2026-10-26T00:00:00Z', platformVersion: 'disposable-test', issues: ['缺少应用声明'], logs: ['API_KEY=synthetic-key-never-use', 'Authorization: Bearer synthetic-control-token', '{"password":"synthetic-json-password"}', 'Cookie: first=synthetic-cookie-one; next=synthetic-cookie-two', 'download failed'] });
  assert.match(prompt, /https:\/\/github.com\/disposable\/artificial\.git/);
  assert.match(prompt, /disposable-test/); assert.match(prompt, /缺少应用声明/);
  assert.match(prompt, /不假设失败下载文件仍存在/); assert.match(prompt, /添加本地项目/);
  assert.match(prompt, /删除或清空应用数据.*取得用户明确同意/);
  assert.doesNotMatch(prompt, /synthetic-key-never-use|synthetic-control-token|synthetic-json-password|synthetic-cookie-one|synthetic-cookie-two/);
});

test('Update and external-start requests use the matching next action without performing a Git rollback', () => {
  const update = buildProjectAiPrompt({ mode: 'update-failed', sourceKind: 'managed', sourcePath: '/disposable/author' });
  const external = buildProjectAiPrompt({ mode: 'start-failed', sourceKind: 'local', sourcePath: '/disposable/author' });
  assert.match(update, /不要直接编辑平台准备的运行程序/); assert.match(update, /构建并更新/);
  assert.match(external, /不得覆盖未保存改动/); assert.match(external, /提醒用户重新打开应用/);
});

test('Review excerpts reject secret/data paths, escaped paths, duplicates and oversized content', () => {
  const excerpt = files => ({ sourcePath: '/disposable/author', files, totalBytes: 100, truncated: false });
  for (const path of ['.env', '.env.production', 'data/records.json', 'credentials/key.txt', 'upload.sqlite', '../outside', '/outside', 'app\\outside']) {
    assert.throws(() => checkedProjectReviewExcerpt(excerpt([{ path, content: 'artificial' }]), '/disposable/author'));
  }
  assert.throws(() => checkedProjectReviewExcerpt(excerpt([{ path: 'README.md', content: '' }, { path: 'README.md', content: '' }]), '/disposable/author'));
  assert.throws(() => checkedProjectReviewExcerpt(excerpt([{ path: 'README.md', content: 'a'.repeat(MAX_REVIEW_BYTES) }]), '/disposable/author'));
  assert.throws(() => checkedProjectReviewExcerpt(excerpt([]), '/disposable/different'));
  assert.equal(checkedProjectReviewExcerpt(excerpt([{ path: '.env.example', content: 'API_KEY=example-only' }]), '/disposable/author').files.length, 1);
});

test('Opening a general assessment does not read source files or copy anything', async () => {
  const fixture = await setup({ mode: 'evaluation', sourceKind: 'local', sourcePath: '/disposable/author' });
  assert.deepEqual(fixture.calls, []);
  await click(button('不确定能不能用？先复制说明问问 AI'));
  assert.equal(document.querySelector('[role=dialog] h2').textContent, '先让 AI 看看能不能用');
  assert.deepEqual(fixture.calls.map(([name]) => name), ['source_review_capabilities']);
  assert.deepEqual(fixture.copies, []);
  await click(button('复制评估说明'));
  assert.equal(fixture.copies.length, 1); assert.match(fixture.copies[0], /本次先只读评估/);
});

test('A failure trigger copies the contextual explanation in one action and does not read the failed source', async () => {
  const fixture = await setup({ mode: 'add-failed', sourceKind: 'git', url: 'https://github.com/disposable/artificial.git', issues: ['人工缺失声明'] });
  await click(button('复制说明给 AI'));
  assert.equal(fixture.copies.length, 1); assert.match(fixture.copies[0], /人工缺失声明/);
  assert.match(document.body.textContent, /已复制/);
  assert.equal(fixture.calls.some(([name]) => name === 'source_review_context'), false);
  assert.equal(button('在 Codex 中打开草稿'), undefined);
});

test('Web mode reads bounded excerpts after selection and copies only user-selected files', async () => {
  const excerpt = { sourcePath: '/disposable/author', totalBytes: 90, truncated: true, files: [{ path: 'README.md', content: 'artificial-readme-fragment' }, { path: 'package.json', content: 'artificial-package-fragment' }] };
  const fixture = await setup({ mode: 'evaluation', sourceKind: 'local', sourcePath: excerpt.sourcePath }, { source_review_context: () => excerpt });
  await click(button('不确定能不能用？先复制说明问问 AI'));
  await click(document.querySelectorAll('input[type=radio]')[1]);
  assert.equal(fixture.calls.filter(([name]) => name === 'source_review_context').length, 1);
  assert.equal(document.querySelectorAll('input[type=checkbox]').length, 2);
  await click(document.querySelectorAll('input[type=checkbox]')[1]);
  await click(button('复制评估说明'));
  assert.match(fixture.copies[0], /artificial-readme-fragment/);
  assert.doesNotMatch(fixture.copies[0], /artificial-package-fragment/);
  assert.match(fixture.copies[0], /这是摘录，不是完整工程/);
});

test('An unsafe native excerpt remains an error and is never included in copied prompt', async () => {
  const fixture = await setup({ mode: 'evaluation', sourceKind: 'local', sourcePath: '/disposable/author' }, { source_review_context: () => ({ sourcePath: '/disposable/author', totalBytes: 20, truncated: false, files: [{ path: '.env', content: 'synthetic-secret-fragment' }] }) });
  await click(button('不确定能不能用？先复制说明问问 AI'));
  await click(document.querySelectorAll('input[type=radio]')[1]);
  assert.match(document.body.textContent, /材料没有读取完成/);
  await click(button('复制评估说明'));
  assert.doesNotMatch(fixture.copies[0], /synthetic-secret-fragment/);
});

test('A context change discards a late excerpt instead of exposing another project in the next dialog', async () => {
  const reading = deferred();
  const fixture = await setup({ mode: 'evaluation', sourceKind: 'local', sourcePath: '/disposable/first' }, { source_review_context: () => reading.promise });
  await click(button('不确定能不能用？先复制说明问问 AI'));
  await click(document.querySelectorAll('input[type=radio]')[1]);
  await fixture.render({ context: { mode: 'evaluation', sourceKind: 'local', sourcePath: '/disposable/second' } });
  await act(async () => reading.resolve({ sourcePath: '/disposable/first', totalBytes: 10, truncated: false, files: [{ path: 'README.md', content: 'late-first-project-fragment' }] }));
  await click(button('不确定能不能用？先复制说明问问 AI'));
  await click(button('复制评估说明'));
  assert.doesNotMatch(fixture.copies[0], /late-first-project-fragment|\/disposable\/first/);
  assert.match(fixture.copies[0], /\/disposable\/second/);
});

test('Only a verified installed Codex exposes the draft action and opening it never sends the request', async () => {
  const fixture = await setup({ mode: 'evaluation', sourceKind: 'local', sourcePath: '/disposable/author' }, { source_review_capabilities: () => ({ codexAvailable: true }), source_review_open_codex: () => ({ requested: true }) });
  await click(button('不确定能不能用？先复制说明问问 AI'));
  await click(button('在 Codex 中打开草稿'));
  assert.equal(fixture.copies.length, 1);
  assert.deepEqual(fixture.calls.find(([name]) => name === 'source_review_open_codex')[1], { sourcePath: '/disposable/author' });
  assert.match(document.body.textContent, /未发送草稿/);
  assert.equal(fixture.calls.some(([name]) => /send|message/.test(name)), false);
});

test('Clipboard failure retains a full selectable explanation without pretending it was copied', async () => {
  const fixture = await setup();
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async () => { throw new Error('人工剪贴板失败'); } } });
  await click(button('不确定能不能用？先复制说明问问 AI'));
  await click(button('复制评估说明'));
  assert.match(document.body.textContent, /人工剪贴板失败/);
  assert.ok(document.querySelector('textarea').value.includes('【平台要求：'));
  assert.deepEqual(fixture.copies, []);
});
