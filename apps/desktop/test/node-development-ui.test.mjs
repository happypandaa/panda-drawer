// Management form regressions with artificial credentials only. These tests do
// not open a native app, system credential store, browser, or external service.
import assert from 'node:assert/strict';
import { after, afterEach, test } from 'node:test';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';
import { JSDOM } from 'jsdom';

const repo = fileURLToPath(new URL('../../../', import.meta.url));
await mkdir(path.join(repo, '.test-artifacts'), { recursive: true });
const output = await mkdtemp(path.join(repo, '.test-artifacts/node-form-dom-'));
const mockPath = path.join(output, 'tauri-mock.mjs');
await writeFile(mockPath, 'export const invoke = (...args) => globalThis.__NODE_FORM_INVOKE__(...args); export const isTauri = () => true;\n');
const compiled = new Map();
async function component(name) {
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
    source = source.replaceAll(/(['"])@tauri-apps\/api\/core\1/g, JSON.stringify(pathToFileURL(mockPath).href));
    await writeFile(target, source);
    return target;
  }
  return import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src', name))).href);
}

const dom = new JSDOM('<!doctype html><html><body></body></html>', { url: 'http://localhost/', pretendToBeVisual: true });
for (const name of ['window', 'document', 'Node', 'HTMLElement', 'HTMLInputElement', 'HTMLSelectElement', 'Event', 'MouseEvent', 'KeyboardEvent', 'FocusEvent', 'CustomEvent', 'MutationObserver']) Object.defineProperty(globalThis, name, { value: name === 'window' ? dom.window : dom.window[name], configurable: true });
Object.defineProperty(globalThis, 'navigator', { value: dom.window.navigator, configurable: true });
globalThis.getComputedStyle = dom.window.getComputedStyle.bind(dom.window);
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
globalThis.__NODE_FORM_INVOKE__ = async (command) => { throw new Error(`Unexpected native request: ${command}`); };
const { createElement: h, act } = await import('react');
const { createRoot } = await import('react-dom/client');

const { NodeDevelopmentPanel } = await component('NodeDevelopmentPanel.tsx');
const { NodeGitPanel } = await component('NodeGitPanel.tsx');
const { SourceLibraryPanel: SourceAcquisitionPanel } = await component('SourceLibraryPanel.tsx');
let root;
const timers = new Map(); let timerId = 0;
window.setInterval = callback => { const id = ++timerId; timers.set(id, callback); return id; };
window.clearInterval = id => timers.delete(id);
afterEach(async () => { if (root) await act(async () => root.unmount()); root = null; timers.clear(); });
after(async () => { dom.window.close(); await rm(output, { recursive: true, force: true }); });
function deferred() { let resolve, reject; const promise = new Promise((yes, no) => { resolve=yes; reject=no; }); return {promise,resolve,reject}; }
function button(text) { const value = [...document.querySelectorAll('button')].find(value => value.textContent === text); assert.ok(value, text); return value; }
async function click(text) { await act(async () => button(text).click()); }
async function change(element, value) { assert.ok(element); await act(async () => { Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set.call(element, value); element.dispatchEvent(new window.Event('input', {bubbles:true})); }); }
async function tick() { await act(async () => { for (const callback of [...timers.values()]) callback(); }); }
async function mount(Component, props, mock) { document.body.innerHTML='<div id="root"></div>'; root=createRoot(document.getElementById('root')); globalThis.__NODE_FORM_INVOKE__=mock; await act(async () => root.render(h(Component,props))); }
const info = (extra={}) => ({sourceRoot:'/disposable/one-authoritative-repository',runtimeIsSource:false,instanceId:'instance_one',...extra});
const source = {sourceId:'src_'+'a'.repeat(32),name:'disposable build',status:'ready',contentDigest:'b'.repeat(64),inspection:{kind:'node-service-source',issues:[]}};
const task = (extra={}) => ({taskId:'task_disposable_node_build',type:'nodeDevelopmentBuild',projectId:null,status:'running',final:false,phase:'building-source',...extra});
const status = (extra={}) => ({repository:{state:'project-root',root:'/disposable/source',markers:[],head:{oid:'a'.repeat(40),branch:'refs/heads/main'},indexState:'clean',git:null},scope:{policy:'node-source-v1',includedRoots:[],includedPaths:[],excluded:[],unclassifiedPaths:[],protectedTrackedPaths:[],complete:true},changes:[{path:'server/main.mjs',kind:'modify',staged:false}],snapshotDigest:'c'.repeat(64),canInitialize:false,canSave:true,canListVersions:true,canCreateBranch:true,branchCreationIssue:null,database:{state:'not-supported',reason:''},issues:[],...extra});
const developmentProps = extra => ({appId:'app_one',instanceId:'instance_one',disabled:false,running:false,onPendingChange(){},onPrepared(){},onInfo(){},...extra});
const gitProps = extra => ({appId:'app_one',instanceId:'instance_one',disabled:false,runtimeIsSource:false,onDraftStateChange(){},...extra});
function gitInput(label) { const element = [...document.querySelectorAll('label')].find(value => value.textContent === label); assert.ok(element,label); return element.querySelector('input') ?? document.getElementById(element.htmlFor); }
async function fillGit() { await change(gitInput('版本说明'),'人工源码说明'); await change(gitInput('提交人'),'Disposable Author'); await change(gitInput('邮箱'),'disposable@example.invalid'); }

test('Node development ignores late source information after connection changes', async () => {
  const old=deferred(); const seen=[]; let reads=0;
  const props=developmentProps({onInfo:value=>seen.push(value)});
  await mount(NodeDevelopmentPanel,props,async command=>{assert.equal(command,'node_development_info'); return ++reads===1?old.promise:info({sourceRoot:'/fresh/source',instanceId:'instance_two'});});
  await act(async()=>root.render(h(NodeDevelopmentPanel,{...props,instanceId:'instance_two'})));
  await act(async()=>old.resolve(info({sourceRoot:'/stale/source'})));
  assert.deepEqual(seen.map(value=>value.sourceRoot),['/fresh/source']);
  assert.match(document.body.textContent,/fresh\/source/); assert.doesNotMatch(document.body.textContent,/stale\/source/);
});

test('A source choice supersedes an outstanding initial info read', async () => {
  const old=deferred(); const seen=[]; let reads=0;
  await mount(NodeDevelopmentPanel,developmentProps({onInfo:value=>seen.push(value)}),async command=>{
    if(command==='node_development_info') return ++reads===1?old.promise:info({sourceRoot:'/chosen/source'});
    if(command==='choose_path') return '/chosen/source'; if(command==='node_development_bind') return {};
    throw Error(command);
  });
  await click('选择源码目录'); await act(async()=>old.resolve(info({sourceRoot:'/obsolete/source'})));
  assert.deepEqual(seen.map(value=>value.sourceRoot),['/chosen/source']); assert.doesNotMatch(document.body.textContent,/obsolete/);
});

test('A stale AI handoff neither copies instructions nor opens Codex', async () => {
  const handoff=deferred(); const copied=[]; const calls=[];
  Object.defineProperty(navigator,'clipboard',{configurable:true,value:{writeText:async value=>copied.push(value)}});
  const props=developmentProps();
  await mount(NodeDevelopmentPanel,props,async command=>{calls.push(command); if(command==='node_development_info')return info(); if(command==='node_development_handoff')return handoff.promise; throw Error(command);});
  await click('在 Codex 打开');
  await act(async()=>root.render(h(NodeDevelopmentPanel,{...props,instanceId:'instance_two'})));
  await act(async()=>handoff.resolve({projectRoot:'/old/source',instanceId:'instance_one',text:'obsolete instructions'}));
  assert.deepEqual(copied,[]); assert.equal(calls.includes('node_development_open_codex'),false); assert.doesNotMatch(document.body.textContent,/obsolete instructions/);
});

test('Node build prevents duplicate submissions and exposes one verified candidate without installing', async () => {
  const submission=deferred(); const calls=[]; const prepared=[]; const pending=[]; let final=false;
  await mount(NodeDevelopmentPanel,developmentProps({onPendingChange:value=>pending.push(value),onPrepared:value=>prepared.push(value)}),async(command,args)=>{
    calls.push([command,args]); if(command==='node_development_info') return info();
    if(command==='node_development_build') return submission.promise;
    if(command==='task_get') return final?task({status:'succeeded',final:true,result:{appId:'app_one',source}}):task();
    throw Error(command);
  });
  await click('构建并更新'); await click('构建并更新');
  assert.equal(calls.filter(([command])=>command==='node_development_build').length,1);
  await act(async()=>submission.resolve(task())); assert.equal(button('打开源码').disabled,true);
  final=true; await tick(); await tick();
  assert.deepEqual(prepared,[source]); assert.equal(pending.at(-1),false);
  assert.equal(calls.some(([command])=>/node_install_apply|node_app_start/.test(command)),false);
});

test('Cancel only requests cancellation and waits for the terminal build result', async () => {
  const calls=[]; const prepared=[]; const pending=[]; let cancelled=false;
  await mount(NodeDevelopmentPanel,developmentProps({onPrepared:value=>prepared.push(value),onPendingChange:value=>pending.push(value)}),async(command,args)=>{
    calls.push([command,args]); if(command==='node_development_info')return info(); if(command==='node_development_build')return task();
    if(command==='node_development_cancel'){cancelled=true;return task();}
    if(command==='task_get')return cancelled?task({status:'cancelled',final:true,error:{message:'人工取消'}}):task();
    throw Error(command);
  });
  await click('构建并更新'); await click('取消构建');
  assert.equal(button('构建并更新').disabled,true); assert.equal(pending.at(-1),true);
  await tick(); assert.equal(pending.at(-1),false); assert.deepEqual(prepared,[]);
  assert.deepEqual(calls.find(([command])=>command==='node_development_cancel'),['node_development_cancel',{taskId:task().taskId}]);
});

test('A build result for another application never becomes an update candidate', async () => {
  const prepared=[];
  await mount(NodeDevelopmentPanel,developmentProps({onPrepared:value=>prepared.push(value)}),async command=>{
    if(command==='node_development_info')return info(); if(command==='node_development_build')return task();
    if(command==='task_get')return task({status:'succeeded',final:true,result:{appId:'other_app',source}});
    throw Error(command);
  });
  await click('构建并更新'); assert.deepEqual(prepared,[]); assert.match(document.querySelector('[role=alert]').textContent,/构建结果不完整/);
});

test('Node Git rejects an old status read but retains drafts on connection changes', async () => {
  const old=deferred(); const drafts=[]; let reads=0;
  const props=gitProps({onDraftStateChange:value=>drafts.push(value)});
  await mount(NodeGitPanel,props,async()=>++reads===1?old.promise:status({repository:{...status().repository,head:{oid:'b'.repeat(40),branch:'refs/heads/fresh'}}}));
  await act(async()=>root.render(h(NodeGitPanel,{...props,instanceId:'instance_two'})));
  await fillGit();
  await act(async()=>old.resolve(status({repository:{...status().repository,head:{oid:'a'.repeat(40),branch:'refs/heads/obsolete'}}})));
  assert.doesNotMatch(document.body.textContent,/obsolete/); assert.equal(gitInput('版本说明').value,'人工源码说明'); assert.equal(drafts.at(-1).dirty,true);
  await act(async()=>root.render(h(NodeGitPanel,{...props,instanceId:'instance_three'})));
  assert.equal(gitInput('版本说明').value,'人工源码说明'); assert.equal(gitInput('提交人').value,'Disposable Author'); assert.equal(drafts.at(-1).dirty,true);
});

test('Author-only edits protect the Git draft until a successful save establishes its baseline', async () => {
  const drafts=[];
  await mount(NodeGitPanel,gitProps({onDraftStateChange:value=>drafts.push(value)}),async(command,args)=>args.operation==='save'?{operationOutcome:'committed'}:status());
  await change(gitInput('提交人'),'Disposable Author'); assert.equal(drafts.at(-1).dirty,true);
  await change(gitInput('邮箱'),'disposable@example.invalid'); await change(gitInput('版本说明'),'保存人工变更'); await click('保存源码版本');
  assert.equal(gitInput('版本说明').value,''); assert.equal(drafts.at(-1).dirty,false); assert.equal(drafts.at(-1).pending,false);
});

test('Node Git save sends the reviewed snapshot and cannot retry a stale write without rereading', async () => {
  const saved=deferred(); const calls=[];
  await mount(NodeGitPanel,gitProps(),async(command,args)=>{calls.push(args); if(args.operation==='status')return status(); if(args.operation==='save')return saved.promise; throw Error(args.operation);});
  await fillGit(); await click('保存源码版本'); await click('保存源码版本');
  const saves=calls.filter(value=>value.operation==='save'); assert.equal(saves.length,1);
  assert.equal(saves[0].input.expectedSnapshot,status().snapshotDigest); assert.equal(saves[0].input.expectedHead,status().repository.head.oid);
  await act(async()=>saved.reject({message:'人工快照已过期'}));
  assert.match(document.body.textContent,/人工快照已过期/); assert.equal(document.querySelector('.node-git-save'),null);
  await click('刷新状态'); assert.equal(gitInput('版本说明').value,'人工源码说明'); assert.equal(gitInput('提交人').value,'Disposable Author');
});

test('A Git save response from an old connection cannot clear the current draft', async () => {
  const saved=deferred(); const drafts=[];
  const props=gitProps({onDraftStateChange:value=>drafts.push(value)});
  await mount(NodeGitPanel,props,async(command,args)=>args.operation==='save'?saved.promise:status());
  await fillGit(); await click('保存源码版本');
  await act(async()=>root.render(h(NodeGitPanel,{...props,instanceId:'instance_two'})));
  await act(async()=>saved.resolve({operationOutcome:'committed'}));
  assert.equal(gitInput('版本说明').value,'人工源码说明'); assert.equal(drafts.at(-1).dirty,true); assert.equal(drafts.at(-1).pending,false);
  assert.doesNotMatch(document.body.textContent,/已保存到本机 Git/);
});

test('Rebinding the source repository clears its stale status and history while keeping the message', async () => {
  const seen=[]; let reads=0;
  const props=gitProps({sourceRoot:'/old/source'});
  await mount(NodeGitPanel,props,async(command,args)=>{seen.push(args); if(args.operation==='status')return status({snapshotDigest:String(++reads).repeat(64)}); if(args.operation==='versions')return {versions:[{oid:'a'.repeat(40),subject:'old source history',author:{name:'old'},authoredAt:'today'}]}; if(args.operation==='save')return {operationOutcome:'committed'}; throw Error(args.operation);});
  await fillGit(); await click('查看版本记录'); assert.match(document.body.textContent,/old source history/);
  await act(async()=>root.render(h(NodeGitPanel,{...props,sourceRoot:'/new/source'})));
  assert.doesNotMatch(document.body.textContent,/old source history/); assert.equal(gitInput('版本说明').value,'人工源码说明');
  await click('保存源码版本'); assert.equal(seen.find(value=>value.operation==='save').input.expectedSnapshot,'2'.repeat(64));
});

test('Git drafts belong to one application and are cleared when its identity changes', async () => {
  const props=gitProps();
  await mount(NodeGitPanel,props,async()=>status());
  await fillGit();
  await act(async()=>root.render(h(NodeGitPanel,{...props,appId:'other_app'})));
  assert.equal(gitInput('版本说明').value,''); assert.equal(gitInput('提交人').value,'');
});

test('Publishing the first managed generation refreshes source separation and enables AI handoff', async () => {
  const seen=[]; let reads=0;
  const props=developmentProps({onInfo:value=>seen.push(value)});
  await mount(NodeDevelopmentPanel,props,async command=>{ assert.equal(command,'node_development_info'); return info({runtimeIsSource:++reads===1}); });
  assert.equal(button('在 Codex 打开').disabled,true);
  await act(async()=>root.render(h(NodeDevelopmentPanel,{...props,installedGeneration:'gen_'+'a'.repeat(32)})));
  assert.equal(reads,2); assert.equal(seen.at(-1).runtimeIsSource,false); assert.equal(button('在 Codex 打开').disabled,false);
});

test('Built program sources report preparation instead of claiming code was never executed', async () => {
  await mount(SourceAcquisitionPanel,{disabled:false,instanceId:'instance_one',onClose(){},onTask(){}},async command=>{ assert.equal(command,'source_list'); return [{...source,provider:'local',localSourcePath:'/disposable/program',evidencePath:'/disposable/evidence',inspection:{...source.inspection,executionState:'build-prepared-program-not-started'}}]; });
  const candidate = document.querySelector('article');
  assert.match(candidate.textContent,/构建已准备，应用未启动/); assert.doesNotMatch(candidate.textContent,/未执行代码/);
});

test('Source acquisition preserves an explicit branch draft across connection changes and defaults only when empty', async () => {
  const old=deferred(); const calls=[];
  const props={disabled:false,instanceId:'instance_one',onClose(){},onTask(){}};
  await mount(SourceAcquisitionPanel,props,async(command,args)=>{
    if(command==='source_list')return [];
    assert.equal(command,'source_acquire'); calls.push(args);
    return calls.length===1?old.promise:{taskId:'task_source',type:'sourceAcquire',projectId:null,status:'failed',final:true,error:{code:'DISPOSABLE_FAILURE',message:'disposable source rejected'}};
  });
  await change(document.getElementById('source-url'),' https://github.com/disposable/source.git ');
  await change(document.getElementById('source-branch'),' codex/tts-providers ');
  await click('获取源码'); assert.equal(document.getElementById('source-branch').readOnly,true);
  await act(async()=>root.render(h(SourceAcquisitionPanel,{...props,instanceId:'instance_two'})));
  assert.equal(document.getElementById('source-branch').value,' codex/tts-providers ');
  await act(async()=>old.resolve({taskId:'old_source',type:'sourceAcquire',projectId:null,status:'failed',final:true,error:{message:'obsolete branch result'}}));
  assert.doesNotMatch(document.body.textContent,/obsolete branch result/);
  await click('获取源码'); assert.deepEqual(calls[1],{input:{url:'https://github.com/disposable/source.git',branch:'codex/tts-providers'}});
  await change(document.getElementById('source-branch'),' '); await click('获取源码');
  assert.equal(calls[2].input.branch,null);
});

const { NodeSourceRestorePanel } = await component('NodeSourceRestorePanel.tsx');
const restoreProps = extra => ({ appId: 'app_one', instanceId: 'instance_one', sourceRoot: '/disposable/source', disabled: false, runtimeIsSource: false, running: false, onDraftStateChange() {}, onChanged() {}, ...extra });
const restorePreview = extra => ({ targetCommit: 'a'.repeat(40), head: 'b'.repeat(40), branch: 'refs/heads/main', snapshotDigest: 'c'.repeat(64), planDigest: 'd'.repeat(64), canApply: true, changes: [{ path: 'src/example.mjs', kind: 'modify' }], ...extra });
const clearRestore = () => ({ state: 'clear', operations: [], issues: [] });
const restoreOperation = extra => ({ operationId: 'restore_11111111-1111-4111-8111-111111111111', phase: 'applying', targetCommit: 'a'.repeat(40), planDigest: 'd'.repeat(64), journalDigest: 'e'.repeat(64), canContinue: true, canUndo: true, changes: [{ path: 'src/example.mjs', state: 'after' }], ...extra });
async function previewRestore() { await change(document.querySelector('.node-source-restore-panel input'), 'a'.repeat(40)); await click('预览源码取回'); }

test('Node source restore previews before acknowledgement, submits once and never saves or builds automatically', async () => {
  const calls = []; const applied = deferred(); let changed = 0;
  await mount(NodeSourceRestorePanel, restoreProps({ onChanged: () => changed++ }), async (command, args) => {
    calls.push([command, args]); assert.equal(command, 'node_development_git');
    if (args.operation === 'restore-status') return clearRestore();
    if (args.operation === 'restore-preview') return restorePreview();
    if (args.operation === 'restore-apply') return applied.promise;
    throw Error(args.operation);
  });
  assert.deepEqual(calls.map(([, args]) => args.operation), ['restore-status']);
  await previewRestore(); assert.equal(button('确认取回源码').disabled, true);
  await act(async () => document.querySelector('input[type=checkbox]').click());
  await click('确认取回源码'); await click('确认取回源码');
  assert.equal(calls.filter(([, args]) => args.operation === 'restore-apply').length, 1);
  assert.deepEqual(calls.find(([, args]) => args.operation === 'restore-apply')[1].input, { targetCommit: 'a'.repeat(40), expectedSnapshot: 'c'.repeat(64), expectedPlanDigest: 'd'.repeat(64) });
  await act(async () => applied.resolve({ operationOutcome: 'committed', state: 'completed' }));
  assert.equal(changed, 1); assert.match(document.body.textContent, /源码已取回为未保存修改/);
  assert.deepEqual(calls.map(([, args]) => args.operation), ['restore-status', 'restore-preview', 'restore-apply', 'restore-status']);
});

test('A Node source-restore preview from an older connection never becomes actionable', async () => {
  const pending = deferred(); const props = restoreProps();
  await mount(NodeSourceRestorePanel, props, async (_command, args) => args.operation === 'restore-status' ? clearRestore() : pending.promise);
  await previewRestore();
  await act(async () => root.render(h(NodeSourceRestorePanel, { ...props, instanceId: 'instance_two' })));
  await act(async () => pending.resolve(restorePreview()));
  assert.equal(document.querySelector('[aria-label="Node 源码取回预览"]'), null);
});

test('Interrupted Node source restoration exposes fresh recovery tokens and requires an explicit continue action', async () => {
  const calls = []; let failed = false;
  await mount(NodeSourceRestorePanel, restoreProps(), async (_command, args) => {
    calls.push(args);
    if (args.operation === 'restore-status') return failed ? { state: 'needs-attention', operations: [restoreOperation()], issues: [] } : clearRestore();
    if (args.operation === 'restore-preview') return restorePreview();
    if (args.operation === 'restore-apply') { failed = true; throw Error('artificial interrupted file operation'); }
    if (args.operation === 'restore-recover') { failed = false; return { operationOutcome: 'committed', state: 'completed' }; }
    throw Error(args.operation);
  });
  await previewRestore(); await act(async () => document.querySelector('input[type=checkbox]').click()); await click('确认取回源码');
  assert.match(document.body.textContent, /有未完成的源码取回/);
  assert.equal(calls.filter(args => args.operation === 'restore-recover').length, 0);
  await click('继续本次取回');
  assert.deepEqual(calls.find(args => args.operation === 'restore-recover').input, { operationId: restoreOperation().operationId, expectedPlanDigest: 'd'.repeat(64), expectedJournalDigest: 'e'.repeat(64), action: 'continue' });
});

test('Unknown Node restore evidence disables recovery and runtime/source prerequisites block every write', async () => {
  const calls = [];
  await mount(NodeSourceRestorePanel, restoreProps(), async (_command, args) => { calls.push(args); return { state: 'needs-attention', operations: [restoreOperation({ canContinue: false, canUndo: false, changes: [] })], issues: [{ message: 'unknown third party file' }] }; });
  assert.equal(button('继续本次取回').disabled, true); assert.equal(button('撤回本次取回').disabled, true); assert.equal(button('预览源码取回').disabled, true);
  await click('继续本次取回'); assert.deepEqual(calls.map(args => args.operation), ['restore-status']);
  await act(async () => root.render(h(NodeSourceRestorePanel, restoreProps({ running: true }))));
  assert.equal(button('核对取回状态').disabled, true);
  await act(async () => root.render(h(NodeSourceRestorePanel, restoreProps({ runtimeIsSource: true }))));
  assert.equal(button('预览源码取回').disabled, true); assert.match(document.body.textContent, /将运行副本与作者源码分开/);
});

const { nodeGitFailureMessage } = await component('node-git-error.ts');
const stoppingReceipt = (result, details = {}) => ({ code: 'SERVICE_STOPPING', message: '人工服务正在退出', details: { operationOutcome: 'committed', result, ...details } });
const savedReceipt = { operation: 'save', operationOutcome: 'committed', repositoryRoot: '/disposable/source', commit: 'a'.repeat(40), parent: 'b'.repeat(40), branch: 'refs/heads/main', message: '人工源码说明', author: { name: 'Disposable Author', email: 'disposable@example.invalid' } };
const restoredReceipt = { operationId: restoreOperation().operationId, state: 'completed', operationOutcome: 'committed', sourceOnly: true, controlUpdateWarning: false, evidencePath: '/disposable/source/.git/local-projects-node-restores/retained', database: { state: 'not-supported' } };
const committedWords = /源码版本已保存|已创建并切换分支|Git 已启用|源码操作已提交/;

test('Native error receipt matrix recognizes only actual matching business commits and keeps physical cleanup separate', () => {
  const accepted = [
    [{ operation: 'save' }, savedReceipt, '源码版本已保存'],
    [{ operation: 'branch' }, { operation: 'create-branch', operationOutcome: 'committed', branch: 'refs/heads/custom', head: 'a'.repeat(40) }, '已创建并切换分支'],
    [{ operation: 'initialize' }, { operation: 'initialize', operationOutcome: 'committed', branch: 'main', head: null }, 'Git 已启用'],
    [{ operation: 'restore-apply' }, restoredReceipt, '源码操作已提交'],
    [{ operation: 'restore-apply' }, { ...restoredReceipt, state: 'undone' }, '源码操作已提交'],
    [{ operation: 'restore-recover', operationId: restoredReceipt.operationId }, restoredReceipt, '源码操作已提交'],
    [{ operation: 'restore-recover', operationId: restoredReceipt.operationId }, { ...restoredReceipt, state: 'undone' }, '源码操作已提交'],
  ];
  for (const [request, receipt, phrase] of accepted) {
    const message = nodeGitFailureMessage(stoppingReceipt(receipt), request);
    assert.ok(message.startsWith(phrase)); assert.match(message, /人工服务正在退出.*SERVICE_STOPPING/);
    assert.match(message, /最后状态尚待.*请勿重复提交/);
    if (request.operation.startsWith('restore-')) assert.doesNotMatch(message, /已取回|已撤回|目标提交/);
  }
  const pending = nodeGitFailureMessage(stoppingReceipt(savedReceipt, { processResourceClosed: false }), { operation: 'save' });
  assert.match(pending, /源码版本已保存/); assert.match(pending, /相关任务仍待收尾/); assert.doesNotMatch(pending, /资源已关闭|收尾完成/);
  const unknown = nodeGitFailureMessage(stoppingReceipt(savedReceipt, { operationOutcome: 'unknown', processResourceClosed: false }), { operation: 'save' });
  assert.doesNotMatch(unknown, committedWords); assert.match(unknown, /待核对/);
});

test('Missing, contradictory, unrelated and read-only receipts never claim a committed Node operation', () => {
  const rejected = [
    [{ operation: 'save' }, null],
    [{ operation: 'save' }, new Error('人工服务正在退出')],
    [{ operation: 'save' }, JSON.stringify(stoppingReceipt(savedReceipt))],
    [{ operation: 'save' }, { message: '人工服务正在退出', details: [] }],
    [{ operation: 'save' }, { message: '人工服务正在退出', details: { result: savedReceipt } }],
    [{ operation: 'save' }, stoppingReceipt(undefined)],
    [{ operation: 'save' }, stoppingReceipt([])],
    [{ operation: 'save' }, stoppingReceipt({ ...savedReceipt, operation: 'create-branch' })],
    [{ operation: 'branch' }, stoppingReceipt(savedReceipt)],
    [{ operation: 'save' }, stoppingReceipt({ ...savedReceipt, operationOutcome: 'unknown' })],
    [{ operation: 'save' }, stoppingReceipt(savedReceipt, { operationOutcome: true })],
    [{ operation: 'restore-apply' }, stoppingReceipt({ ...restoredReceipt, sourceOnly: undefined })],
    [{ operation: 'restore-apply' }, stoppingReceipt({ ...restoredReceipt, sourceOnly: 'true' })],
    [{ operation: 'restore-apply' }, stoppingReceipt({ ...restoredReceipt, state: undefined })],
    [{ operation: 'restore-apply' }, stoppingReceipt({ ...restoredReceipt, state: ['completed'] })],
    [{ operation: 'restore-apply' }, stoppingReceipt({ ...restoredReceipt, state: 'applying' })],
    [{ operation: 'restore-apply' }, stoppingReceipt({ ...restoredReceipt, operationId: 'restore_invalid' })],
    [{ operation: 'restore-recover' }, stoppingReceipt(restoredReceipt)],
    [{ operation: 'restore-recover', operationId: 'restore_22222222-2222-4222-8222-222222222222' }, stoppingReceipt(restoredReceipt)],
    ...['status', 'versions', 'restore-preview', 'restore-status'].flatMap(operation => [
      [{ operation }, stoppingReceipt(savedReceipt)], [{ operation }, stoppingReceipt(restoredReceipt)],
    ]),
  ];
  for (const [request, error] of rejected) {
    const message = nodeGitFailureMessage(error, request);
    assert.doesNotMatch(message, committedWords, JSON.stringify(request));
    if (['save', 'branch', 'restore-apply', 'restore-recover'].includes(request.operation)) assert.match(message, /待核对/);
  }
});

test('A committed save error remains an alert, retains its draft and waits for an explicit status refresh', async () => {
  const calls = []; const drafts = [];
  await mount(NodeGitPanel, gitProps({ onDraftStateChange: value => drafts.push(value) }), async (_command, args) => {
    calls.push(args); if (args.operation === 'status') return status();
    if (args.operation === 'save') throw stoppingReceipt(savedReceipt, { processResourceClosed: false });
    throw Error(args.operation);
  });
  await fillGit(); await click('保存源码版本');
  assert.match(document.querySelector('[role=alert]').textContent, /源码版本已保存.*请勿重复提交.*仍待收尾/);
  assert.equal(document.querySelector('[role=status]'), null); assert.equal(document.querySelector('.node-git-save'), null);
  assert.equal(drafts.at(-1).dirty, true); assert.equal(drafts.at(-1).pending, false);
  assert.deepEqual(calls.map(value => value.operation), ['status', 'save']);
  await click('刷新状态'); assert.equal(gitInput('版本说明').value, '人工源码说明'); assert.equal(gitInput('提交人').value, 'Disposable Author');
  assert.equal(calls.filter(value => value.operation === 'save').length, 1);
});

test('Matching branch and initialize error receipts disclose publication without treating the operation as success', async () => {
  const calls = [];
  await mount(NodeGitPanel, gitProps(), async (_command, args) => {
    calls.push(args); if (args.operation === 'status') return status();
    throw stoppingReceipt({ operation: 'create-branch', operationOutcome: 'committed', branch: 'refs/heads/custom' });
  });
  await change(gitInput('分支名称'), 'custom'); await click('创建并切换');
  assert.match(document.querySelector('[role=alert]').textContent, /已创建并切换分支.*请勿重复提交/);
  assert.deepEqual(calls.map(value => value.operation), ['status', 'branch']);
  await click('刷新状态'); assert.equal(gitInput('分支名称').value, 'custom');
  await act(async () => root.unmount()); root = null;
  await mount(NodeGitPanel, gitProps(), async (_command, args) => {
    if (args.operation === 'status') return status({ canInitialize: true, canSave: false });
    assert.equal(args.operation, 'initialize'); throw stoppingReceipt({ operation: 'initialize', operationOutcome: 'committed', branch: 'main', head: null });
  });
  await click('启用 Git'); assert.match(document.querySelector('[role=alert]').textContent, /Git 已启用.*最后状态尚待刷新/);
  assert.equal(document.querySelector('[role=status]'), null);
});

test('A committed-looking version read or old-generation error never becomes a save confirmation', async () => {
  await mount(NodeGitPanel, gitProps(), async (_command, args) => {
    if (args.operation === 'status') return status(); throw stoppingReceipt(savedReceipt);
  });
  await click('查看版本记录'); assert.match(document.querySelector('[role=alert]').textContent, /人工服务正在退出/); assert.doesNotMatch(document.body.textContent, committedWords);
  await act(async () => root.unmount()); root = null;
  const saved = deferred(); const props = gitProps();
  await mount(NodeGitPanel, props, async (_command, args) => args.operation === 'save' ? saved.promise : status());
  await fillGit(); await click('保存源码版本');
  await act(async () => root.render(h(NodeGitPanel, { ...props, instanceId: 'instance_two' })));
  await act(async () => saved.reject(stoppingReceipt(savedReceipt)));
  assert.equal(document.querySelector('[role=alert]'), null); assert.equal(gitInput('版本说明').value, '人工源码说明');
  assert.doesNotMatch(document.body.textContent, committedWords);
});

test('A source-only terminal error receipt uses generic commit wording, retains error state and never applies again', async () => {
  const calls = []; let changed = 0;
  await mount(NodeSourceRestorePanel, restoreProps({ onChanged: () => changed++ }), async (_command, args) => {
    calls.push(args); if (args.operation === 'restore-status') return clearRestore();
    if (args.operation === 'restore-preview') return restorePreview();
    if (args.operation === 'restore-apply') throw stoppingReceipt(restoredReceipt, { processResourceClosed: false });
    throw Error(args.operation);
  });
  await previewRestore(); await act(async () => document.querySelector('input[type=checkbox]').click()); await click('确认取回源码');
  assert.match(document.querySelector('[role=alert]').textContent, /源码操作已提交.*最后状态尚待核对.*请勿重复提交.*仍待收尾/);
  assert.doesNotMatch(document.querySelector('[role=alert]').textContent, /已取回|已撤回/);
  assert.equal(document.querySelector('[role=status]'), null); assert.equal(document.querySelector('[aria-label="Node 源码取回预览"]'), null);
  assert.equal(document.querySelector('.node-source-restore-panel input').value, 'a'.repeat(40)); assert.equal(changed, 0);
  assert.deepEqual(calls.map(value => value.operation), ['restore-status', 'restore-preview', 'restore-apply', 'restore-status']);
});

test('Recovery error receipts must match the exact record and never infer a direction', async () => {
  for (const same of [false, true]) {
    if (root) { await act(async () => root.unmount()); root = null; }
    const calls = [];
    await mount(NodeSourceRestorePanel, restoreProps(), async (_command, args) => {
      calls.push(args); if (args.operation === 'restore-status') return { state: 'needs-attention', operations: [restoreOperation()], issues: [] };
      if (args.operation === 'restore-recover') throw stoppingReceipt({ ...restoredReceipt, state: 'undone', operationId: same ? restoredReceipt.operationId : 'restore_22222222-2222-4222-8222-222222222222' });
      throw Error(args.operation);
    });
    await click('撤回本次取回'); const text = document.querySelector('[role=alert]').textContent;
    if (same) assert.match(text, /源码操作已提交/); else assert.doesNotMatch(text, committedWords);
    assert.doesNotMatch(text, /已取回|已撤回/); assert.match(text, /待核对/);
    assert.equal(calls.filter(value => value.operation === 'restore-recover').length, 1);
    assert.equal(calls.at(-1).operation, 'restore-status');
  }
});

test('A stale restore commit error and a preview error cannot publish a confirmation into the current generation', async () => {
  const applied = deferred(); const props = restoreProps(); let changed = 0;
  await mount(NodeSourceRestorePanel, { ...props, onChanged: () => changed++ }, async (_command, args) => {
    if (args.operation === 'restore-status') return clearRestore();
    if (args.operation === 'restore-preview') return restorePreview(); return applied.promise;
  });
  await previewRestore(); await act(async () => document.querySelector('input[type=checkbox]').click()); await click('确认取回源码');
  await act(async () => root.render(h(NodeSourceRestorePanel, { ...props, instanceId: 'instance_two', onChanged: () => changed++ })));
  await act(async () => applied.reject(stoppingReceipt(restoredReceipt)));
  assert.equal(document.querySelector('[role=alert]'), null); assert.equal(changed, 0); assert.doesNotMatch(document.body.textContent, committedWords);
  await act(async () => root.unmount()); root = null;
  await mount(NodeSourceRestorePanel, restoreProps(), async (_command, args) => { if (args.operation === 'restore-status') return clearRestore(); throw stoppingReceipt(restoredReceipt); });
  await previewRestore(); assert.match(document.querySelector('[role=alert]').textContent, /人工服务正在退出/); assert.doesNotMatch(document.body.textContent, committedWords);
});

test('A failed status read after a successful write uses read context instead of trusting an unrelated error receipt', async () => {
  let reads = 0;
  await mount(NodeGitPanel, gitProps(), async (_command, args) => {
    if (args.operation === 'status') { if (++reads === 1) return status(); throw stoppingReceipt(savedReceipt); }
    assert.equal(args.operation, 'save'); return { operationOutcome: 'committed' };
  });
  await fillGit(); await click('保存源码版本');
  assert.match(document.querySelector('[role=alert]').textContent, /人工服务正在退出/);
  assert.doesNotMatch(document.querySelector('[role=alert]').textContent, committedWords);
  await act(async () => root.unmount()); root = null;
  reads = 0;
  await mount(NodeSourceRestorePanel, restoreProps(), async (_command, args) => {
    if (args.operation === 'restore-status') { if (++reads === 1) return clearRestore(); throw stoppingReceipt(restoredReceipt); }
    if (args.operation === 'restore-preview') return restorePreview(); return { operationOutcome: 'committed', state: 'completed' };
  });
  await previewRestore(); await act(async () => document.querySelector('input[type=checkbox]').click()); await click('确认取回源码');
  assert.match(document.querySelector('[role=alert]').textContent, /人工服务正在退出/);
  assert.doesNotMatch(document.querySelector('[role=alert]').textContent, committedWords);
});
