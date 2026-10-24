// Read-only capability wording regressions. These do not verify a native app,
// a Git repository, or the safety of any filesystem operation.
import assert from 'node:assert/strict';
import { after, test } from 'node:test';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';
import { JSDOM } from 'jsdom';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

const repo = fileURLToPath(new URL('../../../', import.meta.url));
await mkdir(path.join(repo, '.test-artifacts'), { recursive: true });
const output = await mkdtemp(path.join(repo, '.test-artifacts/object-layout-dom-'));
const sourceFile = path.join(repo, 'apps/desktop/src/ObjectLayoutPanel.tsx');
const compiledFile = path.join(output, 'ObjectLayoutPanel.mjs');
await writeFile(compiledFile, ts.transpileModule(await readFile(sourceFile, 'utf8'), {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX },
  fileName: sourceFile,
}).outputText);
const { ObjectLayoutPanel } = await import(pathToFileURL(compiledFile).href);
after(() => rm(output, { recursive: true, force: true }));

function fixture(extra = {}) {
  return {
    kind: 'project-v1',
    source: { state: 'available', path: '/disposable/project/app', reason: '已读取项目清单中的源码目录。' },
    runtime: { webRoot: '/disposable/project/app/dist', backendRoot: null, external: false },
    data: { root: '/disposable/project/data', database: '/disposable/project/data/app.sqlite', attachments: '/disposable/project/data/attachments' },
    configurationPath: null,
    controlPath: '/disposable/control/project',
    build: { state: 'platform-build', reason: '构建使用平台已有入口；可重建性仍需实际验证。' },
    versioning: { state: 'not-integrated', reason: '没有执行 Git 命令。', markers: [], inspectionIssues: [], database: { state: 'not-supported', reason: '当前活库不直接参与版本切换。' } },
    ...extra,
  };
}

function render(layout) {
  return JSDOM.fragment(renderToStaticMarkup(createElement(ObjectLayoutPanel, { layout })));
}

test('Absent and failed layout observations remain unknown instead of fabricating capabilities', () => {
  for (const value of [undefined, null]) {
    const panel = render(value);
    assert.equal(panel.querySelector('details').hasAttribute('open'), false);
    assert.match(panel.textContent, /尚未读取或未能核验/);
    assert.equal(panel.querySelector('code'), null);
    assert.doesNotMatch(panel.textContent, /可使用平台构建入口|源码目录存在|没有仓库/);
  }
});

test('A runnable project without source exposes runtime and data without claiming rebuild support', () => {
  const panel = render(fixture({ source: { state: 'missing', path: null, reason: '成品没有提供源码。' }, build: { state: 'unavailable', reason: '缺少 package.json 与 package-lock.json。' } }));
  assert.match(panel.textContent, /源码缺失。成品没有提供源码/);
  assert.match(panel.textContent, /平台构建不可用。缺少 package.json 与 package-lock.json/);
  assert.match(panel.textContent, /声明的网页产物目录/);
  assert.match(panel.textContent, /平台管理实际发布代次；已打开窗口可能仍使用原代次/);
  assert.doesNotMatch(panel.textContent, /当前网页运行目录|当前后端运行目录/);
  assert.ok([...panel.querySelectorAll('code')].some(value => value.textContent === '/disposable/project/app/dist'));
  assert.ok([...panel.querySelectorAll('code')].some(value => value.textContent === '/disposable/project/data/app.sqlite'));
  assert.doesNotMatch(panel.textContent, /可使用平台构建入口|源码目录存在/);
});

test('Git files, ancestors and unsafe markers are observations without verified history or database coverage', () => {
  const base = fixture();
  const panel = render(fixture({ versioning: { ...base.versioning, markers: [
    { path: '/disposable/project/app/.git', kind: 'file', scope: 'source' },
    { path: '/disposable/.git', kind: 'directory', scope: 'ancestor' },
    { path: '/disposable/project/.git', kind: 'unsafe', scope: 'project' },
  ], inspectionIssues: ['标记读取失败，保留原文件。'] } }));
  assert.match(panel.textContent, /仅为检测到的 .git 标记，尚未验证仓库、版本历史或工作区状态/);
  assert.match(panel.textContent, /源码目录 · 文件标记/);
  assert.match(panel.textContent, /上级目录 · 目录标记/);
  assert.match(panel.textContent, /无法安全核实的标记/);
  assert.match(panel.textContent, /标记读取失败，保留原文件/);
  assert.match(panel.textContent, /当前活库不直接参与版本切换/);
  assert.match(panel.textContent, /不另建独立备份体系。旧备份与未完成现场继续保留/);
  assert.doesNotMatch(panel.textContent, /仓库可用|已有版本|工作区干净/);
});

test('A registered Node directory is an external runtime location with unverified source and build', () => {
  const panel = render(fixture({
    kind: 'local-node-source',
    source: { state: 'unverified', path: '/disposable/external-node', reason: '登记目录不证明包含可编辑源码。' },
    runtime: { webRoot: '/disposable/external-node/public', backendRoot: '/disposable/external-node/server', external: true },
    build: { state: 'unavailable', reason: '未核实构建方式。' },
  }));
  assert.match(panel.textContent, /源码目录.*登记目录不证明包含可编辑源码/);
  assert.match(panel.textContent, /当前网页运行目录/);
  assert.match(panel.textContent, /当前后端运行目录/);
  assert.doesNotMatch(panel.textContent, /声明的网页产物目录|平台管理实际发布代次/);
  assert.match(panel.textContent, /源码未核实/);
  assert.match(panel.textContent, /仍依赖该目录；不代表已安装自包含成品包/);
  assert.match(panel.textContent, /平台构建不可用。未核实构建方式/);
  assert.doesNotMatch(panel.textContent, /可使用平台构建入口|源码目录存在/);
});

test('Unverified role paths are not inferred from neighboring paths', () => {
  const panel = render(fixture({ data: { root: null, database: null, attachments: null }, controlPath: null }));
  const rows = [...panel.querySelectorAll('dl > div')];
  for (const label of ['业务数据目录', '当前数据库', '附件目录', '平台控制目录']) {
    const row = rows.find(value => value.querySelector('dt').textContent === label);
    assert.equal(row.querySelector('code'), null);
    assert.match(row.querySelector('dd').textContent, /路径未能核验/);
  }
  assert.match(panel.textContent, /未发现可报告的 .git 标记；这不证明其他位置没有仓库或版本历史/);
});
