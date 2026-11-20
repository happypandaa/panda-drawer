// Artificial prompt/DOM fixtures only. No actual user source, external AI,
// clipboard integration, native folder picker or installed AI tool is accessed.
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
await writeFile(mock, 'export const invoke = (...args) => globalThis.__PROJECT_AI_HELP_INVOKE__(...args); export const isTauri = () => globalThis.__PROJECT_AI_HELP_TAURI__ === true; export const listen = async () => () => {};\n');
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
  source = source.replaceAll(/(['"])@tauri-apps\/api\/(?:core|event)\1/g, JSON.stringify(pathToFileURL(mock).href));
  await writeFile(target, source); return target;
}

const dom = new JSDOM('<!doctype html><html><body></body></html>', { url: 'http://localhost/', pretendToBeVisual: true });
// These interaction fixtures assert Chinese copy; OS defaults are tested separately.
dom.window.localStorage.setItem('panda-drawer:language:v1', 'zh-CN');
for (const name of ['window', 'document', 'Node', 'HTMLElement', 'HTMLInputElement', 'HTMLSelectElement', 'Event', 'MouseEvent', 'KeyboardEvent', 'MutationObserver']) Object.defineProperty(globalThis, name, { value: name === 'window' ? dom.window : dom.window[name], configurable: true });
Object.defineProperty(globalThis, 'navigator', { value: dom.window.navigator, configurable: true });
globalThis.getComputedStyle = dom.window.getComputedStyle.bind(dom.window);
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
const { createElement: h, act, StrictMode } = await import('react');
const { createRoot } = await import('react-dom/client');
const { AiProjectHelp } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/AiProjectHelp.tsx'))).href);
const { AiToolsProvider } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/AiToolsProvider.tsx'))).href);
const { PlatformAiIntroButton } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/PlatformAiIntroButton.tsx'))).href);
const { FrontendPlatformProvider, PlatformSkillPathProvider, frontendPlatformFromBootstrap } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/frontend-platform.tsx'))).href);
const { buildProjectAiPrompt, checkedProjectReviewExcerpt, MAX_REVIEW_BYTES } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/project-ai-help.ts'))).href);
const { PLATFORM_AI_SUMMARY } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/project-ai-requirements.ts'))).href);
const { checkedProcessDiagnostics, MAX_PROCESS_DIAGNOSTIC_BYTES } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/process-diagnostics.ts'))).href);
const { localizePlatformPrompt } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/platform-prompt.ts'))).href);
const { setLocale } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/i18n.ts'))).href);
const { AI_PROMPT_RULES, DIRECTORY_PROMPT, PROMPT_TEMPLATES } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/project-ai-requirements.ts'))).href);
let root;
afterEach(async () => { if (root) await act(async () => root.unmount()); root = null; globalThis.__PROJECT_AI_HELP_INVOKE__ = undefined; globalThis.__PROJECT_AI_HELP_TAURI__ = false; window.localStorage.removeItem('panda-drawer:ai-tools:v1'); });
after(async () => { dom.window.close(); await rm(output, { recursive: true, force: true }); });
const button = label => [...document.querySelectorAll('button')].find(value => value.textContent.trim() === label);
async function click(value) { assert.ok(value); await act(async () => value.click()); }
function deferred() { let resolve; const promise = new Promise(yes => { resolve = yes; }); return { promise, resolve }; }
/** `ai`: render inside the Mac AI tools provider with this saved tool choice. */
async function setup(context = { mode: 'evaluation', sourceKind: 'local' }, overrides = {}, ai = null) {
  const calls = []; const copies = [];
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async value => { copies.push(value); } } });
  globalThis.__PROJECT_AI_HELP_INVOKE__ = async (command, args) => {
    calls.push([command, args]);
    if (overrides[command]) return overrides[command](args);
    if (ai && command === 'detect_ai_tools') return [{ id: 'codex', installed: true, oneClick: true, capability: 'full', detectedBy: ['protocol:codex'] }];
    if (ai && command === 'ai_handoff_show_manager') return undefined;
    throw new Error(`Unexpected artificial native command: ${command}`);
  };
  if (ai) {
    globalThis.__PROJECT_AI_HELP_TAURI__ = true;
    window.localStorage.setItem('panda-drawer:ai-tools:v1', JSON.stringify({ acknowledged: true, selectedToolId: ai, hiddenGuides: [] }));
  }
  document.body.innerHTML = '<div id="root"></div>'; root = createRoot(document.getElementById('root'));
  let props = { context };
  const mac = frontendPlatformFromBootstrap({ build: { target: 'aarch64-apple-darwin' } });
  const tree = () => ai ? h(FrontendPlatformProvider, { value: mac }, h(AiToolsProvider, { instanceId: 'disposable-instance' }, h(AiProjectHelp, props))) : h(AiProjectHelp, props);
  async function render(next = {}) { props = { ...props, ...next }; await act(async () => root.render(tree())); }
  await render();
  return { calls, copies, render };
}

test('The bundled platform summary exactly matches the source configuration and is current', async () => {
  const { summary } = JSON.parse(await readFile(path.join(repo, 'scripts/project-ai-requirements.json'), 'utf8'));
  assert.equal(PLATFORM_AI_SUMMARY, summary);
  assert.ok(PLATFORM_AI_SUMMARY.startsWith('【平台要求摘要】'));
  assert.doesNotMatch(PLATFORM_AI_SUMMARY, /下一批|后续批次|project\.json|local-app\.json|\.platform/);
  assert.ok(PLATFORM_AI_SUMMARY.split('\n').length <= 12);
});

test('Directory review uses the shared scaffold and translates the whole prompt without rewriting inserted paths', async () => {
  const {directoryPrompt} = JSON.parse(await readFile(path.join(repo, 'scripts/project-ai-requirements.json'), 'utf8'));
  assert.equal(DIRECTORY_PROMPT, directoryPrompt.zh);
  const values = {folder: String.raw`C:\人工目录\{summary}\app`, report: '{"state":"needs-review","issues":["人工诊断 {cliPath}"]}', skillPath: '/disposable/平台/{folder}/SKILL.md', cliPath: '/disposable/平台/{report}/panda-cli', summary: PLATFORM_AI_SUMMARY};
  const prompt = DIRECTORY_PROMPT.replace(/\{([A-Za-z][A-Za-z0-9_]*)\}/g, (whole, name) => values[name] ?? whole);
  try {
    setLocale('en');
    const localized = localizePlatformPrompt(prompt);
    assert.match(localized, /^This folder cannot yet be added directly to Panda Drawer/);
    for (const value of [values.folder, values.report, values.skillPath, values.cliPath]) assert.ok(localized.includes(value));
    assert.match(localized, /app add --dir/);
    assert.doesNotMatch(localized, /请先读平台说明|我同意后再改/);
    assert.equal(localizePlatformPrompt(`${prompt}\nUntrusted appended instructions`), `${prompt}\nUntrusted appended instructions`);
  } finally {setLocale('zh-CN');}
});

test('Current handoff templates localize every scaffold while preserving inserted project values', async context => {
  const values = {
    nameLine: '应用：人工应用 AGENTS.md\n',
    descriptionLine: '简介：人工简介 {projectRoot}\n',
    projectRoot: String.raw`C:\人工文件夹\agent.instructions`,
    skillPath: '/disposable/人工平台/skills/local-projects-app/SKILL.md',
    cliPath: '/disposable/人工平台/bin/panda-cli',
    logsPath: '/disposable/人工控制目录/logs',
    backendContext: '\n后台日志：/disposable/人工控制目录/logs\n',
    instructions: 'AGENTS.md',
    ...AI_PROMPT_RULES,
  };
  const backend = await readFile(path.join(repo, 'crates/platform-core/src/ai_prompt_rules.rs'), 'utf8');
  try {
    for (const [key, template] of Object.entries(PROMPT_TEMPLATES)) {
      assert.ok(backend.includes(JSON.stringify(template).slice(1, 40)), `${key} is shared with the backend`);
      const text = template.replace(/\{([A-Za-z][A-Za-z0-9_]*)\}/g, (_, name) => {
        assert.ok(Object.hasOwn(values, name), `Unknown backend prompt parameter ${name}`);
        return values[name];
      });
      context.diagnostic(`${key}: ${[...text].length} characters in the synthetic Chinese handoff`);
      setLocale('en');
      const localized = localizePlatformPrompt(text);
      assert.notEqual(localized, text, key);
      assert.doesNotMatch(localized, /先读平台说明|请先读懂|修改时请遵守/u, key);
      assert.ok(localized.includes(`Application: ${values.nameLine.slice('应用：'.length)}`), key);
      assert.ok(localized.includes(values.projectRoot), key);
      assert.ok(localized.includes(values.skillPath), key);
      assert.ok(localized.includes(values.cliPath), key);
      if (key.endsWith('Node')) assert.ok(localized.includes(values.logsPath), key);
      if (key === 'create') assert.ok(localized.includes('Description: 人工简介 {projectRoot}\n'), key);
      setLocale('zh-CN');
      assert.equal(localizePlatformPrompt(text), text);
    }
    setLocale('en');
    const arbitrary = '用户自写：请先读懂这个应用，暂时不要修改任何文件。';
    assert.equal(localizePlatformPrompt(arbitrary), arbitrary);
  } finally {
    setLocale('zh-CN');
  }
});

test('A Git failure prompt preserves source identity and delivery while removing known secret log formats', () => {
  const prompt = buildProjectAiPrompt({ mode: 'add-failed', sourceKind: 'git', url: 'https://github.com/disposable/artificial.git', branch: 'example', commit: 'a'.repeat(40), checkedAt: '2026-10-26T00:00:00Z', platformVersion: 'disposable-test', issues: ['缺少应用声明', 'API_KEY=synthetic-key-never-use', 'Authorization: Bearer synthetic-control-token', '{"password":"synthetic-json-password"}', 'Cookie: first=synthetic-cookie-one; next=synthetic-cookie-two', 'download failed'] });
  assert.match(prompt, /https:\/\/github.com\/disposable\/artificial\.git/);
  assert.match(prompt, /disposable-test/); assert.match(prompt, /缺少应用声明/);
  assert.match(prompt, /平台自带的 Git/); assert.match(prompt, /app reserve --json/); assert.match(prompt, /app add --reservation/); assert.match(prompt, /保留已下载文件/);
  assert.ok(prompt.includes(PLATFORM_AI_SUMMARY));
  assert.match(prompt, /删除或清空应用数据.*取得我明确同意/);
  assert.match(prompt, /通俗易懂的中文/); assert.doesNotMatch(prompt, /普通话/);
  assert.doesNotMatch(prompt, /synthetic-key-never-use|synthetic-control-token|synthetic-json-password|synthetic-cookie-one|synthetic-cookie-two/);
});

test('Process diagnostics retain useful errors beyond the issue limit and bound untrusted UTF-8 output', () => {
  const processDiagnostics = { command: 'npm ci', exitCode: 1, signal: null, nodeVersion: '24.22.0', npmVersion: '11.19.1', stdout: '界'.repeat(3000), stderr: `${'context '.repeat(250)}npm error EAI_AGAIN\nAuthorization: Bearer synthetic-process-token`, truncated: false };
  const checked = checkedProcessDiagnostics(processDiagnostics);
  assert.ok(checked);
  assert.ok(new TextEncoder().encode(checked.stdout).byteLength <= MAX_PROCESS_DIAGNOSTIC_BYTES);
  assert.equal(checked.stdout.includes('\ufffd'), false);
  assert.equal(checked.truncated, true);
  const prompt = buildProjectAiPrompt({ mode: 'add-failed', sourceKind: 'git', processDiagnostics });
  assert.match(prompt, /"exitCode": 1/);
  assert.match(prompt, /npm error EAI_AGAIN/);
  assert.match(prompt, /"truncated": true/);
  assert.doesNotMatch(prompt, /synthetic-process-token/);
  for (const change of [{ exitCode: NaN }, { signal: -1 }, { stdout: {} }, { truncated: 'yes' }]) {
    assert.equal(checkedProcessDiagnostics({ ...processDiagnostics, ...change }), null);
  }
  assert.equal(checkedProcessDiagnostics(undefined), null);
});

test('Start repair prompts point to the installed skill instead of carrying platform rules', () => {
  for (const runtime of ['node', 'web']) {
    const prompt = buildProjectAiPrompt({ mode: 'start-failed', hasBackend: runtime === 'node', sourceKind: 'local', sourcePath: '/disposable/author' }, { skillPath: '/disposable/install/skills/local-projects-app/SKILL.md' });
    assert.ok(prompt.includes('平台说明：/disposable/install/skills/local-projects-app/SKILL.md'));
    assert.match(prompt, /以平台说明为准/); assert.match(prompt, /等我同意再改/);
    assert.doesNotMatch(prompt, /app\.id|安装副本|应用修改|local-app\.json|【平台要求|A\. 一句话/);
    assert.ok(!prompt.includes(PLATFORM_AI_SUMMARY));
    assert.equal(prompt.includes('stdout 只能输出平台控制协议消息'), runtime === 'node');
  }
  assert.match(buildProjectAiPrompt({ mode: 'start-failed', sourceKind: 'local' }), /复制时未读取到路径/);
});

test('Runtime problem prompts include real log paths, Git recovery and the stdout protocol limit', () => {
  const prompt = buildProjectAiPrompt({ mode: 'start-failed', sourceKind: 'local', sourcePath: '/disposable/author', runtimeLogPaths: { current: '/disposable/control/logs/current.log', previous: '/disposable/control/logs/previous.log' } });
  for (const expected of ['/disposable/control/logs/current.log', '/disposable/control/logs/previous.log', '修改前先用 Git 保存当前版本', '我要求回退时，用 Git 恢复到之前的版本', 'stdout 只能输出平台控制协议消息', 'console.error']) assert.ok(prompt.includes(expected), expected);
  assert.doesNotMatch(prompt, /Codex|构建并更新/);
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
  await click(button('不确定能不能用？问问你的 AI 工具'));
  assert.equal(document.querySelector('[role=dialog] h2').textContent, '问问 AI 能不能用');
  assert.deepEqual(fixture.calls, []);
  assert.deepEqual(fixture.copies, []);
  await click(button('复制提示词'));
  assert.equal(fixture.copies.length, 1); assert.match(fixture.copies[0], /本次只读评估/);
});

test('A failure trigger copies the contextual explanation in one action and does not read the failed source', async () => {
  const fixture = await setup({ mode: 'add-failed', sourceKind: 'git', url: 'https://github.com/disposable/artificial.git', issues: ['人工缺失声明'] });
  await click(button('复制提示词'));
  assert.equal(fixture.copies.length, 1); assert.match(fixture.copies[0], /人工缺失声明/);
  assert.match(document.body.textContent, /已复制/);
  assert.equal(fixture.calls.some(([name]) => name === 'source_review_context'), false);
  assert.equal(button('在 Codex 中打开草稿'), undefined);
});

test('Web mode reads bounded excerpts after selection and copies only user-selected files', async () => {
  const excerpt = { sourcePath: '/disposable/author', totalBytes: 90, truncated: true, files: [{ path: 'README.md', content: 'artificial-readme-fragment' }, { path: 'package.json', content: 'artificial-package-fragment' }] };
  const fixture = await setup({ mode: 'evaluation', sourceKind: 'local', sourcePath: excerpt.sourcePath }, { source_review_context: () => excerpt });
  await click(button('不确定能不能用？问问你的 AI 工具'));
  await click(document.querySelectorAll('input[type=radio]')[1]);
  assert.equal(fixture.calls.filter(([name]) => name === 'source_review_context').length, 1);
  assert.equal(document.querySelectorAll('input[type=checkbox]').length, 2);
  await click(document.querySelectorAll('input[type=checkbox]')[1]);
  await click(button('复制提示词'));
  assert.match(fixture.copies[0], /artificial-readme-fragment/);
  assert.doesNotMatch(fixture.copies[0], /artificial-package-fragment/);
  assert.match(fixture.copies[0], /这是摘录，不是完整工程/);
});

for (const mode of ['start-failed']) {
  test(`${mode} keeps the selected public excerpts and web AI access limits`, async () => {
    const excerpt = { sourcePath: '/disposable/author', totalBytes: 90, truncated: true, files: [{ path: 'README.md', content: 'artificial-repair-readme' }, { path: 'package.json', content: 'artificial-repair-package' }] };
    const fixture = await setup({ mode, sourceKind: 'local', sourcePath: excerpt.sourcePath }, { source_review_context: () => excerpt });
    await click(button('复制提示词'));
    assert.equal(fixture.copies.length, 1);
    await click(document.querySelectorAll('input[type=radio]')[1]);
    await click(document.querySelectorAll('input[type=checkbox]')[1]);
    await click(button('复制完整提示词'));
    const prompt = fixture.copies.at(-1);
    assert.equal(fixture.copies.length, 2);
    assert.match(prompt, /artificial-repair-readme/);
    assert.doesNotMatch(prompt, /artificial-repair-package/);
    assert.match(prompt, /材料受大小限制，部分内容未包含/);
    assert.match(prompt, /网页版 AI，通常读不到本机文件/);
    assert.match(prompt, /不要索要整个项目、真实 \.env、密钥或数据库/);
  });
}

test('An unsafe native excerpt remains an error and is never included in copied prompt', async () => {
  const fixture = await setup({ mode: 'evaluation', sourceKind: 'local', sourcePath: '/disposable/author' }, { source_review_context: () => ({ sourcePath: '/disposable/author', totalBytes: 20, truncated: false, files: [{ path: '.env', content: 'synthetic-secret-fragment' }] }) });
  await click(button('不确定能不能用？问问你的 AI 工具'));
  await click(document.querySelectorAll('input[type=radio]')[1]);
  assert.match(document.body.textContent, /材料没有读取完成/);
  await click(button('复制提示词'));
  assert.doesNotMatch(fixture.copies[0], /synthetic-secret-fragment/);
});

test('A context change discards a late excerpt instead of exposing another project in the next dialog', async () => {
  const reading = deferred();
  const fixture = await setup({ mode: 'evaluation', sourceKind: 'local', sourcePath: '/disposable/first' }, { source_review_context: () => reading.promise });
  await click(button('不确定能不能用？问问你的 AI 工具'));
  await click(document.querySelectorAll('input[type=radio]')[1]);
  await fixture.render({ context: { mode: 'evaluation', sourceKind: 'local', sourcePath: '/disposable/second' } });
  await act(async () => reading.resolve({ sourcePath: '/disposable/first', totalBytes: 10, truncated: false, files: [{ path: 'README.md', content: 'late-first-project-fragment' }] }));
  await click(button('不确定能不能用？问问你的 AI 工具'));
  await click(button('复制提示词'));
  assert.doesNotMatch(fixture.copies[0], /late-first-project-fragment|\/disposable\/first/);
  assert.match(fixture.copies[0], /\/disposable\/second/);
});

const handedOver = args => ({ toolId: 'codex', toolName: 'ChatGPT（Codex）', oneClick: true, opened: true, capability: 'full', prefilled: true, text: args.text, projectRoot: args.sourcePath ?? '/disposable/app-root', instanceId: 'disposable-instance', appName: '人工应用' });
const handoffCall = fixture => fixture.calls.find(([name]) => name === 'open_ai_handoff')?.[1];

test('Without native AI handoff, failure help only copies and never opens a tool', async () => {
  const fixture = await setup({ mode: 'start-failed', sourceKind: 'managed', appId: 'app-disposable', sourcePath: '/disposable/author' });
  await click(button('复制提示词'));
  assert.equal(document.querySelector('.project-ai-copy-only'), null);
  assert.deepEqual(fixture.calls, []);
  assert.equal(fixture.copies.length, 1);
  assert.match(fixture.copies[0], /以平台说明为准/);
});

test('A start failure hands the registered app and its repair explanation to the chosen tool in one click', async () => {
  const fixture = await setup({ mode: 'start-failed', sourceKind: 'managed', appId: 'app-disposable', appName: '人工应用', sourcePath: '/disposable/author', issues: ['人工启动失败'] }, { open_ai_handoff: handedOver }, 'codex');
  await click(button('告诉 Codex 出了什么问题'));
  const args = handoffCall(fixture);
  assert.equal(args.toolId, 'codex'); assert.equal(args.appId, 'app-disposable'); assert.equal(args.sourcePath, null);
  assert.equal(args.purpose, 'repair'); assert.equal(args.entry, 'start-failed');
  assert.match(args.text, /打不开了/); assert.match(args.text, /人工启动失败/);
  assert.equal(document.querySelector('.project-ai-help-modal'), null, 'No copy dialog is needed');
  assert.deepEqual(fixture.copies, [args.text]);
  assert.match(document.body.textContent, /正在 Codex 中打开 人工应用/);
});

test('A failed link reserves the app folder once and hands that folder, never a parent download folder', async () => {
  let reserved = 0;
  const fixture = await setup({ mode: 'add-failed', sourceKind: 'git', url: 'https://github.com/disposable/artificial.git', issues: ['人工下载失败'] }, {
    app_reserve: args => { reserved++; assert.deepEqual(args, { name: 'artificial' }); return { reservation: 'reservation-disposable', path: '/disposable/workspace/project-sources/artificial' }; },
    open_ai_handoff: handedOver,
  }, 'codex');
  await click(button('告诉 Codex 出了什么问题'));
  const args = handoffCall(fixture);
  assert.equal(args.sourcePath, '/disposable/workspace/project-sources/artificial');
  assert.equal(args.entry, 'add-failed-link');
  assert.match(args.text, /app add --reservation reservation-disposable/);
  assert.doesNotMatch(args.text, /app reserve --json/);
  await click(button('告诉 Codex 出了什么问题'));
  assert.equal(reserved, 1);
  assert.equal(fixture.calls.filter(([name]) => name === 'open_ai_handoff').length, 2);
});

test('A late reservation from another link does not supply the current app folder', async () => {
  const first = deferred();
  const second = { reservation: 'reservation-second', path: '/disposable/workspace/project-sources/second' };
  const fixture = await setup({ mode: 'add-failed', sourceKind: 'git', url: 'https://github.com/disposable/first.git' }, {
    app_reserve: ({ name }) => name === 'first' ? first.promise : second,
    open_ai_handoff: handedOver,
  }, 'codex');
  await click(button('告诉 Codex 出了什么问题'));
  await fixture.render({ context: { mode: 'add-failed', sourceKind: 'git', url: 'https://github.com/disposable/second.git' } });
  await act(async () => first.resolve({ reservation: 'reservation-first', path: '/disposable/workspace/project-sources/first' }));
  assert.equal(handoffCall(fixture), undefined);
  await click(button('告诉 Codex 出了什么问题'));
  assert.equal(handoffCall(fixture).sourcePath, second.path);
  assert.match(handoffCall(fixture).text, /app add --reservation reservation-second/);
});

test('A link whose folder cannot be reserved falls back to the copy dialog', async () => {
  const fixture = await setup({ mode: 'add-failed', sourceKind: 'git', url: 'https://github.com/disposable/artificial.git' }, { app_reserve: () => { throw new Error('人工预留失败'); } }, 'codex');
  await click(button('告诉 Codex 出了什么问题'));
  assert.equal(handoffCall(fixture), undefined);
  assert.equal(document.querySelector('[role=dialog] h2').textContent, '告诉 Codex 出了什么问题');
  assert.match(document.body.textContent, /人工预留失败/);
});

test('A local folder that could not be added is handed over as that folder, and copying stays available', async () => {
  const fixture = await setup({ mode: 'add-failed', sourceKind: 'local', sourcePath: '/disposable/author', issues: ['人工检查问题'] }, { open_ai_handoff: handedOver }, 'codex');
  await click(button('告诉 Codex 怎么处理'));
  const args = handoffCall(fixture);
  assert.equal(args.sourcePath, '/disposable/author'); assert.equal(args.entry, 'add-failed-folder');
  assert.match(args.text, /人工检查问题/); assert.match(args.text, /app add --dir/);
  await click(button('复制提示词'));
  assert.ok([...document.querySelectorAll('[role=dialog] h2')].some(title => title.textContent === '告诉 Codex 出了什么问题'), 'the copy dialog opens beside the steps panel');
  assert.match(fixture.copies.at(-1), /人工检查问题/);
});

test('An assessment hands a selected folder to the tool, while a repository address stays copy-only', async () => {
  const fixture = await setup({ mode: 'evaluation', sourceKind: 'local', sourcePath: '/disposable/author' }, { open_ai_handoff: handedOver }, 'codex');
  await click(button('不确定能不能用？问问 Codex'));
  await click(button('问问 Codex 能不能用'));
  const args = handoffCall(fixture);
  assert.equal(args.sourcePath, '/disposable/author'); assert.equal(args.entry, 'evaluation');
  assert.match(args.text, /本次只读评估/);
  await root.unmount(); root = null;
  const remote = await setup({ mode: 'evaluation', sourceKind: 'git', url: 'https://github.com/disposable/artificial.git' }, {}, 'codex');
  await click(button('不确定能不能用？问问 Codex'));
  assert.equal(Boolean(button('问问 Codex 能不能用')), false);
  assert.match(document.body.textContent, /只填了仓库地址时还没有本机文件夹/);
  assert.equal(handoffCall(remote), undefined);
});

test('Clipboard failure retains a full selectable explanation without pretending it was copied', async () => {
  const fixture = await setup();
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async () => { throw new Error('人工剪贴板失败'); } } });
  await click(button('不确定能不能用？问问你的 AI 工具'));
  await click(button('复制提示词'));
  assert.match(document.body.textContent, /人工剪贴板失败/);
  assert.ok(document.querySelector('textarea').value.includes('【平台要求摘要】'));
  assert.deepEqual(fixture.copies, []);
});

test('The platform introduction explains open-source porting and can append a software URL in both languages', async () => {
  const { buildPlatformAiIntroPrompt } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/platform-ai-intro.ts'))).href);
  const skill = '/disposable/install/skills/local-projects-app/SKILL.md';
  const url = 'https://example.test/open-source/project';
  const prompt = buildPlatformAiIntroPrompt('disposable-version', undefined, skill);
  assert.match(prompt, /^下面是“熊猫小抽屉”平台的介绍/);
  assert.match(prompt, /当前版本：disposable-version/);
  assert.match(prompt, /给出方案并经我同意之前，不要修改文件/);
  assert.match(prompt, /能读写本机文件、运行命令/);
  assert.match(prompt, /app reserve --json.*app check --dir.*app add --reservation/);
  assert.match(prompt, /如果是开源软件.*原有数据要不要导入/);
  assert.match(prompt, /如果是新项目/); assert.match(prompt, /如果是已有项目/);
  assert.ok(prompt.includes(PLATFORM_AI_SUMMARY)); assert.match(prompt, /references\/porting\.md/);
  assert.ok(prompt.includes(`完整的平台说明：${skill}`));
  assert.doesNotMatch(prompt.replace(skill, ''), /普通话|继续开发|sourcePath|"request"|应用 ID|\/disposable\/|^软件地址：/m);
  assert.equal(buildPlatformAiIntroPrompt('disposable-version', undefined, skill, undefined, ` ${url} `), `${prompt}\n软件地址：${url}`);
  assert.ok(prompt.length < 2000, String(prompt.length));
  assert.match(buildPlatformAiIntroPrompt(), /当前版本：未读取/);
  assert.match(prompt, /macOS 上的本地小应用平台/);
  try {
    setLocale('en');
    const english = buildPlatformAiIntroPrompt('disposable-version', undefined, skill, undefined, url);
    assert.match(english, /^Here is an introduction to Panda Drawer/);
    assert.match(english, /Wait for my approval of the plan before editing files/);
    assert.match(english, /app reserve --json.*app check --dir.*app add --reservation/);
    assert.match(english, /data\/main\.sqlite by default \(web SDK or node:sqlite/);
    assert.match(english, /references\/porting\.md/);
    assert.ok(english.endsWith(`\nSoftware URL: ${url}`));
  } finally { setLocale('zh-CN'); }
});

test('The actual Windows introduction button copies the native target description', async () => {
  const copies = [];
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async text => copies.push(text) } });
  document.body.innerHTML = '<div id="root"></div>'; root = createRoot(document.getElementById('root'));
  const platform = frontendPlatformFromBootstrap({ build: { target: 'x86_64-pc-windows-msvc' } });
  await act(async () => root.render(h(FrontendPlatformProvider, { value: platform }, h(PlatformSkillPathProvider, { value: 'C:\\Disposable\\skills\\local-projects-app\\SKILL.md' }, h(PlatformAiIntroButton, { platformVersion: 'disposable-version' })))));
  await click(button('复制平台介绍'));
  assert.equal(copies.length, 1);
  assert.match(copies[0], /Windows x64 上的本地小应用平台（当前版本：disposable-version）/);
  assert.doesNotMatch(copies[0], /macOS 上的本地小应用平台/);
  assert.ok(copies[0].includes(PLATFORM_AI_SUMMARY));
  assert.ok(copies[0].includes('C:\\Disposable\\skills\\local-projects-app\\SKILL.md'));
});

async function setupStrictPlatformIntro(writeText) {
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
  document.body.innerHTML = '<div id="root"></div>'; root = createRoot(document.getElementById('root'));
  await act(async () => root.render(h(StrictMode, null, h(PlatformAiIntroButton, { platformVersion: 'disposable-version' }))));
}

test('The platform introduction copies and becomes reusable after StrictMode effect replay', async () => {
  const writing = deferred(); const copies = [];
  await setupStrictPlatformIntro(async text => { copies.push(text); await writing.promise; });
  await click(button('复制平台介绍'));
  assert.equal(button('正在复制…').disabled, true);
  await act(async () => writing.resolve());
  assert.match(document.querySelector('[role=status]').textContent, /已复制/);
  assert.equal(button('复制平台介绍').disabled, false);
  await click(button('复制平台介绍'));
  assert.equal(copies.length, 2);
  assert.equal(copies[0], copies[1]);
  assert.match(copies[0], /当前版本：disposable-version/);
});

test('The platform introduction exposes its full fallback and can retry copying in StrictMode', async () => {
  let attempts = 0;
  await setupStrictPlatformIntro(async () => { if (++attempts === 1) throw new Error('artificial clipboard failure'); });
  await click(button('复制平台介绍'));
  assert.match(document.querySelector('[role=dialog]').textContent, /无法自动复制/);
  const field = document.querySelector('textarea');
  assert.equal(field.readOnly, true);
  assert.match(field.value, /当前版本：disposable-version/);
  assert.ok(field.value.includes(PLATFORM_AI_SUMMARY));
  assert.equal(document.querySelector('[role=status]'), null);
  await click(document.querySelector('button[aria-label="关闭平台介绍"]'));
  assert.equal(button('复制平台介绍').disabled, false);
  await click(button('复制平台介绍'));
  assert.equal(attempts, 2);
  assert.match(document.querySelector('[role=status]').textContent, /已复制/);
});
