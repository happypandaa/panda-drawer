// DOM interaction fixtures only; these do not establish native Git/install behavior.
import assert from 'node:assert/strict';
import { after, afterEach, test } from 'node:test';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';
import { JSDOM } from 'jsdom';

const repo = fileURLToPath(new URL('../../../', import.meta.url));
await mkdir(path.join(repo, '.test-artifacts'), { recursive: true });
const output = await mkdtemp(path.join(repo, '.test-artifacts/disposable-source-import-dom-'));
const mockPath = path.join(output, 'tauri-mock.mjs');
await writeFile(mockPath, 'export const invoke = (...args) => globalThis.__SOURCE_IMPORT_INVOKE__(...args);\n');
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
  source = source.replaceAll(/(['"])@tauri-apps\/api\/core\1/g, JSON.stringify(pathToFileURL(mockPath).href));
  await writeFile(target, source); return target;
}
const dom = new JSDOM('<!doctype html><html><body></body></html>', { url: 'http://localhost/', pretendToBeVisual: true });
for (const name of ['window', 'document', 'Node', 'HTMLElement', 'HTMLInputElement', 'HTMLSelectElement', 'Event', 'MouseEvent', 'KeyboardEvent', 'MutationObserver']) Object.defineProperty(globalThis, name, { value: name === 'window' ? dom.window : dom.window[name], configurable: true });
Object.defineProperty(globalThis, 'navigator', { value: dom.window.navigator, configurable: true });
globalThis.getComputedStyle = dom.window.getComputedStyle.bind(dom.window);
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
dom.window.HTMLElement.prototype.scrollIntoView = function () {};
const { createElement: h, act } = await import('react');
const { createRoot } = await import('react-dom/client');
const { SourceAcquisitionPanel } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/SourceAcquisitionPanel.tsx'))).href);
let root; let nextTimer = 0;
const timeouts = new Map(); const intervals = new Map();
dom.window.setTimeout = fn => { const id = ++nextTimer; timeouts.set(id, fn); return id; };
dom.window.clearTimeout = id => timeouts.delete(id);
dom.window.setInterval = fn => { const id = ++nextTimer; intervals.set(id, fn); return id; };
dom.window.clearInterval = id => intervals.delete(id);
const { acceptProjectDeletion, observeDeletionRuntime, resetProjectDeletionsForTests } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/project-deletion.ts'))).href);
afterEach(async () => { if (root) await act(async () => root.unmount()); root = null; resetProjectDeletionsForTests(); timeouts.clear(); intervals.clear(); dom.window.localStorage.clear(); });
after(async () => { dom.window.close(); await rm(output, { recursive: true, force: true }); });

const appId='11111111-1111-4111-8111-111111111111';
const url='https://github.com/disposable/one.git';
const task=(changes={})=>({taskId:'task_import',projectId:null,type:'nodeGitImport',status:'succeeded',final:true,phase:'completed',result:null,error:null,...changes});
const app={id:appId,name:'Disposable app',sourcePath:'/disposable/projects/one',runtime:'node-service',state:'stopped',fields:[],configuration:{revision:0,values:{},secretStatus:{},missingRequired:[],canStart:true}};
const snapshot=(changes={})=>({task:task(),appId,sourcePath:app.sourcePath,commitState:'committed',cleanupPending:false,remainingPaths:[],app,...changes});
async function setup(overrides={},extra={}){
 const calls=[];const entered=[];let added=0;
 const defaults={node_git_import_start:()=>task(),node_git_import_status:()=>snapshot(),node_git_import_retry_cleanup:()=>snapshot(),source_review_capabilities:()=>({codexAvailable:false})};
 globalThis.__SOURCE_IMPORT_INVOKE__=async(command,args)=>{calls.push([command,args]);const fn=overrides[command]??defaults[command];assert.ok(fn,command);return fn(args);};
 document.body.innerHTML='<div id="root"></div>';root=createRoot(document.getElementById('root'));
 await act(async()=>root.render(h(SourceAcquisitionPanel,{disabled:false,instanceId:'instance_one',projectDirectory:'/disposable/projects',onTask(){},onClose(){},onInstalled(){added++;},onEnter:value=>entered.push(value),...extra})));
 return {calls,entered,get added(){return added;}};
}
const button=text=>[...document.querySelectorAll('button')].find(item=>item.textContent===text);
async function click(item){assert.ok(item);await act(async()=>item.click());}
async function change(id,value){await act(async()=>{const item=document.getElementById(id);Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype,'value').set.call(item,value);item.dispatchEvent(new window.Event('input',{bubbles:true}));});}
async function begin(){await change('git-import-url',url);await click(button('开始添加'));}
test('The form describes one directly running folder without a source library or build approval',async()=>{
 await setup();assert.match(document.body.textContent,/直接使用这一份代码/);assert.match(document.body.textContent,/不执行仓库脚本/);assert.doesNotMatch(document.body.textContent,/源码记录与恢复|运行平台准备好的版本|同意并继续/);
});
test('Successful addition uses one task and opens only after an explicit action',async()=>{
 const fixture=await setup();await begin();assert.match(document.body.textContent,/已添加「Disposable app」/);assert.equal(fixture.added,1);assert.equal(fixture.entered.length,0);
 assert.deepEqual(fixture.calls.map(([command])=>command),['node_git_import_start','node_git_import_status']);await click(button('打开应用'));assert.equal(fixture.entered[0].id,appId);
});
test('Duplicate detection uses repository URL and branch from direct registration without sourceId',async()=>{
 const fixture=await setup({}, {existingRepositories:[{url,branch:'feature/one',appId,name:'Existing direct'}]});
 await change('git-import-url',url.replace('.git','/'));await change('git-import-branch','feature/one');await click(button('开始添加'));
 assert.match(document.body.textContent,/已经添加过：「Existing direct」/);assert.equal(fixture.calls.length,0);await click(button('仍然另外添加一份'));assert.equal(fixture.calls.filter(([command])=>command==='node_git_import_start').length,1);
});
test('A different branch is a separate addition',async()=>{
 const fixture=await setup({}, {existingRepositories:[{url,branch:'feature/other',appId,name:'Other branch'}]});await begin();assert.equal(fixture.added,1);assert.equal(fixture.calls[0][1].input.branch,null);
});
test('Unknown registration is not labelled added and rechecking never starts a second transaction',async()=>{
 let confirmed=false;const fixture=await setup({node_git_import_status:()=>snapshot({commitState:confirmed?'committed':'unknown',cleanupPending:!confirmed})});await begin();assert.match(document.body.textContent,/添加结果待核对/);assert.doesNotMatch(document.body.textContent,/已添加「/);confirmed=true;await click(button('重新核对'));assert.equal(fixture.added,1);assert.equal(fixture.calls.filter(([command])=>command==='node_git_import_start').length,1);
});
test('Failed dependencies show their original error and exact remaining cleanup location',async()=>{
 const failure=snapshot({app:null,commitState:'not_committed',cleanupPending:true,remainingPaths:['/disposable/partial'],task:task({status:'failed',error:{code:'NEEDS_MODIFICATION',message:'需要修改：缺少锁文件'}})});
 const fixture=await setup({node_git_import_status:()=>failure,node_git_import_retry_cleanup:()=>({...failure,cleanupPending:false,remainingPaths:[]})});await begin();assert.match(document.body.textContent,/需要修改：缺少锁文件/);assert.match(document.body.textContent,/disposable\/partial/);await click(button('重试清理'));assert.match(document.body.textContent,/需要修改：缺少锁文件/);assert.doesNotMatch(document.body.textContent,/已添加「/);assert.equal(fixture.added,0);
});

test('Definite pre-admission failure returns to the preserved form without claiming cleanup',async()=>{
 await setup({node_git_import_start:()=>{throw {code:'NODE_GIT_IMPORT_BUSY',message:'正在添加另一个项目',details:{operationOutcome:'not_started'}};}});await begin();assert.match(document.body.textContent,/尚未开始下载项目文件/);await click(button('返回填写'));assert.equal(document.getElementById('git-import-url').value,url);
});
test('Reconciliation of a running accepted import resumes progress until its final result',async()=>{
 let finish;const final=new Promise(resolve=>{finish=resolve;});let reads=0;
 const running=snapshot({task:task({status:'running',final:false,phase:'downloading'}),commitState:'not_committed',app:null});
 const fixture=await setup({node_git_import_start:()=>{throw Error('lost reply');},node_git_import_status:()=>{reads++;if(reads===1)throw Error('connection lost');if(reads===2)return running;return final;}});
 await begin();assert.match(document.body.textContent,/添加结果待核对/);await click(button('重新核对'));assert.doesNotMatch(document.body.textContent,/这个项目暂时不能添加|临时材料已清理/);assert.equal(button('取消添加').disabled,false);
 await act(async()=>finish(snapshot()));assert.match(document.body.textContent,/已添加「Disposable app」/);assert.equal(fixture.calls.filter(([command])=>command==='node_git_import_start').length,1);
});
