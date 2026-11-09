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
await writeFile(mockPath, 'export const invoke = (...args) => globalThis.__SOURCE_IMPORT_INVOKE__(...args); export const isTauri = () => true;\n');
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
const { GitImportRecoveryNotice } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/GitImportRecoveryNotice.tsx'))).href);
const { ImportProjectCard } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/ImportProjectCard.tsx'))).href);
const { TaskCenter } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/TaskCenter.tsx'))).href);
const { canDismissGitImport } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/git-import-view-state.ts'))).href);
const { downloadRoutes, DOWNLOAD_ROUTE_KEY } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/download-routes.ts'))).href);
const { DownloadRouteSettings } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/DownloadRouteSettings.tsx'))).href);
const { FrontendPlatformProvider, frontendPlatformFromBootstrap } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/frontend-platform.tsx'))).href);
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
const app={id:appId,name:'Disposable app',sourcePath:'/disposable/projects/one',projectRoot:'/disposable/projects/one',hasBackend:true,state:'ready',fields:[],configuration:{revision:0,values:{},secretStatus:{},missingRequired:[],canStart:true}};
const snapshot=(changes={})=>({task:task(),appId,sourcePath:app.sourcePath,commitState:'committed',cleanupPending:false,remainingPaths:[],app,...changes});
async function setup(overrides={},extra={}){
 const calls=[];const entered=[];const webEntered=[];const states=[];let added=0;
 const defaults={node_git_import_start:()=>task(),node_git_import_status:()=>snapshot(),node_git_import_retry_cleanup:()=>snapshot(),source_review_capabilities:()=>({codexAvailable:false})};
 globalThis.__SOURCE_IMPORT_INVOKE__=async(command,args)=>{calls.push([command,args]);const fn=overrides[command]??defaults[command];assert.ok(fn,command);return fn(args);};
 document.body.innerHTML='<div id="root"></div>';root=createRoot(document.getElementById('root'));
 const { frontendPlatform, ...componentProps } = extra;
 const props={disabled:false,instanceId:'instance_one',projectDirectory:'/disposable/projects',onTask(){},onClose(){},onInstalled(){added++;},onEnter:value=>entered.push(value),onWebEnter:value=>webEntered.push(value),onStateChange:value=>states.push(value),...componentProps};
 const view=()=>frontendPlatform?h(FrontendPlatformProvider,{value:frontendPlatform},h(SourceAcquisitionPanel,props)):h(SourceAcquisitionPanel,props);
 await act(async()=>root.render(view()));
 return {calls,entered,webEntered,states,async render(patch){Object.assign(props,patch);await act(async()=>root.render(view()));},get added(){return added;}};
}
const button=text=>[...document.querySelectorAll('button')].find(item=>item.textContent===text);
async function click(item){assert.ok(item);await act(async()=>item.click());}
async function change(id,value){await act(async()=>{const item=document.getElementById(id);Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype,'value').set.call(item,value);item.dispatchEvent(new window.Event('input',{bubbles:true}));});}
async function begin(){await change('git-import-url',url);await click(button('开始添加'));}
async function flushTimeouts(){const timers=[...timeouts.values()];timeouts.clear();await act(async()=>{for(const timer of timers)timer();});}
const macPlatform=frontendPlatformFromBootstrap({build:{target:'aarch64-apple-darwin'}});
const agentRecommendation={url,name:'Disposable agent',catalogId:'agent-example',agent:true};

test('A committed download opens required configuration once after cleanup and keeps its success result on return', async () => {
 const incomplete={...app,state:'needsConfiguration'};
 const clean=snapshot({app:incomplete});
 const fixture=await setup({
  node_git_import_status:()=>({...clean,cleanupPending:true,remainingPaths:['/disposable/import-control']}),
  node_git_import_retry_cleanup:()=>clean,
  app_configuration:args=>{assert.equal(args.appId,appId);return {...incomplete,manifestDigest:'reviewed-manifest',fields:[],configuration:{revision:1,values:{},secretStatus:{},missingRequired:['API_KEY'],canStart:false}};},
 });
 await begin();
 assert.equal(fixture.calls.some(([command])=>command==='app_configuration'),false,'Cleanup must finish before configuration opens');
 await click(button('重试清理'));
 assert.ok(document.querySelector('.node-configuration-form'));
 assert.match(document.body.textContent,/已添加「Disposable app」/);
 assert.equal(document.querySelector('.git-import-source code').textContent,app.projectRoot);
 assert.deepEqual(fixture.entered,[]); assert.deepEqual(fixture.webEntered,[]);
 await click(button('返回添加结果'));
 await fixture.render({platformVersion:'same-session-refresh'});
 assert.equal(document.querySelector('.node-configuration-form'),null);
 assert.equal(fixture.calls.filter(([command])=>command==='app_configuration').length,1,'A later result render must not reopen the form');
 assert.match(document.body.textContent,/已添加「Disposable app」/);
 assert.equal(document.querySelector('.git-import-source code').textContent,app.projectRoot);
});

test('A recommended agent waits for a location decision and displays the actual default parent', async () => {
 const fixture=await setup({}, {frontendPlatform:macPlatform,initialSource:agentRecommendation,projectDirectory:'/disposable/current-workspace/project-sources'});
 await flushTimeouts();
 const placement=document.querySelector('[aria-label="放在哪里"]');
 assert.ok(placement);
 assert.equal(placement.querySelector('.agent-install-option code').textContent,'/disposable/current-workspace/project-sources');
 assert.match(placement.textContent,/你的 AI 会在这个文件夹里工作/);
 assert.match(placement.textContent,/删除应用时整个文件夹和 data\/ 一起删除/);
 assert.equal(fixture.calls.length,0,'Opening the location step must not download or open a picker');
 await click(button('开始添加'));
 const starts=fixture.calls.filter(([command])=>command==='node_git_import_start');
 assert.equal(starts.length,1);assert.equal(starts[0][1].input.selectedDirectory,undefined);
});

test('A recommended agent passes the checked empty folder itself as the installation target', async () => {
 const selected={path:'/disposable/user-picked/agent',revision:'checked-empty-target'};
 const fixture=await setup({choose_path:args=>{assert.deepEqual(args,{kind:'agent'});return selected.path;},source_import_directory_inspect:args=>{assert.deepEqual(args,{path:selected.path});return selected;},node_git_import_status:()=>snapshot({sourcePath:selected.path,app:{...app,sourcePath:selected.path}})}, {frontendPlatform:macPlatform,initialSource:agentRecommendation});
 await click(button('选择文件夹…'));
 assert.deepEqual(fixture.calls.map(([command])=>command),['choose_path','source_import_directory_inspect']);
 assert.match(document.querySelector('[aria-label="放在哪里"]').textContent,/文件夹、代码和 data\/ 会保留/);
 assert.equal(fixture.calls.some(([command])=>command==='node_git_import_start'),false);
 await click(button('开始添加'));
 const input=fixture.calls.find(([command])=>command==='node_git_import_start')[1].input;
 assert.deepEqual(input.selectedDirectory,selected);assert.equal(input.catalogId,'agent-example');
 assert.equal(document.querySelector('.git-import-source code').textContent,selected.path);
});

test('Canceling the agent folder picker keeps the location step without checking or downloading', async () => {
 const fixture=await setup({choose_path:()=>null}, {frontendPlatform:macPlatform,initialSource:agentRecommendation});
 await click(button('选择文件夹…'));await flushTimeouts();
 assert.ok(document.querySelector('[aria-label="放在哪里"]'));
 assert.equal(button('开始添加').disabled,true);
 assert.deepEqual(fixture.calls.map(([command])=>command),['choose_path']);
});

test('A nonempty agent folder stays on the location step and cannot start a download', async () => {
 const fixture=await setup({choose_path:()=>'/disposable/nonempty',source_import_directory_inspect:()=>{throw Error('请选择一个空文件夹，或新建一个');}}, {frontendPlatform:macPlatform,initialSource:agentRecommendation});
 await click(button('选择文件夹…'));await flushTimeouts();
 assert.ok(document.querySelector('[aria-label="放在哪里"]'));
 assert.match(document.querySelector('[role="alert"]').textContent,/请选择一个空文件夹，或新建一个/);
 assert.equal(button('开始添加').disabled,true);
 assert.deepEqual(fixture.calls.map(([command])=>command),['choose_path','source_import_directory_inspect']);
});

test('Windows agent recommendations keep the existing default download flow', async () => {
 const fixture=await setup({}, {frontendPlatform:frontendPlatformFromBootstrap({build:{target:'x86_64-pc-windows-msvc'}}),initialSource:agentRecommendation});
 assert.equal(document.querySelector('[aria-label="放在哪里"]')===null,true);
 await flushTimeouts();
 const requests=fixture.calls.filter(([command])=>command==='node_git_import_start');
 assert.equal(requests.length,1);assert.equal(requests[0][1].input.selectedDirectory,undefined);
 assert.equal(fixture.calls.some(([command])=>command==='choose_path'),false);
});

test('An ordinary recommendation on macOS skips agent location selection', async () => {
 const fixture=await setup({}, {frontendPlatform:macPlatform,initialSource:{url,catalogId:'ordinary-example',agent:false}});
 assert.equal(document.querySelector('[aria-label="放在哪里"]')===null,true);
 await flushTimeouts();
 assert.equal(fixture.calls.filter(([command])=>command==='node_git_import_start').length,1);
});

test('Pasted links on macOS keep the existing form and omit any selected folder', async () => {
 const fixture=await setup({}, {frontendPlatform:macPlatform});
 assert.equal(document.querySelector('[aria-label="放在哪里"]')===null,true);
 assert.ok(document.getElementById('git-import-url'));
 await begin();
 assert.equal(fixture.calls.find(([command])=>command==='node_git_import_start')[1].input.selectedDirectory,undefined);
});

test('Switching a failed agent recommendation to another route preserves its checked folder', async () => {
 downloadRoutes.setMode('global');
 const gitee='https://gitee.com/disposable/one.git';const selectedDirectory={path:'/disposable/user-agent',revision:'same-checked-folder'};let selectedUrl='';
 const fixture=await setup({choose_path:()=>selectedDirectory.path,source_import_directory_inspect:()=>selectedDirectory,node_git_import_start:({input})=>{selectedUrl=input.url;return task();},node_git_import_status:()=>selectedUrl===url?snapshot({app:null,commitState:'not_committed',task:task({status:'failed',error:{code:'SOURCE_GIT_FAILED',message:'failed',details:{downloadFailure:true}}})}):snapshot({sourcePath:selectedDirectory.path,downloadUrl:gitee})}, {frontendPlatform:macPlatform,initialSource:{...agentRecommendation,catalogRepositories:{github:url,gitee},downloadRoute:'global'}});
 await click(button('选择文件夹…'));await click(button('开始添加'));await click(button('换成国内线路并重试'));
 const requests=fixture.calls.filter(([command])=>command==='node_git_import_start').map(([,args])=>args.input);
 assert.deepEqual(requests.map(input=>input.url),[url,gitee]);
 assert.ok(requests.every(input=>input.selectedDirectory.path===selectedDirectory.path&&input.selectedDirectory.revision===selectedDirectory.revision));
 assert.equal(fixture.calls.filter(([command])=>command==='source_import_directory_inspect').length,1,'Retry must reuse the checked target rather than adopt a changed directory');
});

test('A recommended import passes one alternate to the backend and records the successful mirror', async () => {
  downloadRoutes.setMode('auto');
  downloadRoutes.recordSuccess('global');
  const gitee = 'https://gitee.com/disposable/one.git';
  const fixture = await setup({ node_git_import_status: () => snapshot({ downloadUrl: gitee }) }, {
    initialSource: { url, catalogId: 'example', catalogRepositories: { github: url, gitee }, downloadRoute: 'global' },
  });
  const timers = [...timeouts.values()]; timeouts.clear();
  await act(async () => { for (const timer of timers) timer(); });
  const requests = fixture.calls.filter(([command]) => command === 'node_git_import_start');
  assert.equal(requests.length, 1);
  assert.equal(requests[0][1].input.alternateUrl, gitee);
  assert.equal(requests[0][1].input.catalogId, 'example');
  assert.equal(downloadRoutes.getSnapshot().mode, 'auto');
  assert.equal(downloadRoutes.getSnapshot().detectedRoute, 'cn');
});

test('A manual download failure offers an explicit source change after cleanup and retries with the saved catalog ID', async () => {
  downloadRoutes.setMode('global');
  const gitee = 'https://gitee.com/disposable/one.git';
  let selected = '';
  const fixture = await setup({
    node_git_import_start: ({ input }) => { selected = input.url; return task(); },
    node_git_import_status: () => selected === url
      ? snapshot({ app: null, commitState: 'not_committed', task: task({ status: 'failed', error: { code: 'SOURCE_GIT_FAILED', message: 'failed', details: { downloadFailure: true } } }) })
      : snapshot({ downloadUrl: gitee }),
  }, { initialSource: { url, catalogId: 'example', catalogRepositories: { github: url, gitee }, downloadRoute: 'global' } });
  const timers = [...timeouts.values()]; timeouts.clear();
  await act(async () => { for (const timer of timers) timer(); });
  assert.match(document.body.textContent, /用国外线路下载失败/);
  await click(button('换成国内线路并重试'));
  const requests = fixture.calls.filter(([command]) => command === 'node_git_import_start').map(([, args]) => args.input);
  assert.deepEqual(requests.map(input => input.url), [url, gitee]);
  assert.ok(requests.every(input => input.catalogId === 'example' && input.alternateUrl === undefined));
  assert.notEqual(requests[0].importId, requests[1].importId);
  assert.equal(downloadRoutes.getSnapshot().mode, 'cn');
  assert.equal(fixture.added, 1);
});

test('The download settings preserve a manual choice and allow explicit automatic redetection', async () => {
  downloadRoutes.setMode('auto');
  document.body.innerHTML = '<div id="root"></div>';
  root = createRoot(document.getElementById('root'));
  await act(async () => root.render(h(DownloadRouteSettings)));
  const select = document.querySelector('select');
  await act(async () => {
    Object.getOwnPropertyDescriptor(window.HTMLSelectElement.prototype, 'value').set.call(select, 'cn');
    select.dispatchEvent(new window.Event('change', { bubbles: true }));
  });
  assert.equal(JSON.parse(dom.window.localStorage.getItem(DOWNLOAD_ROUTE_KEY)).mode, 'cn');
  assert.equal(button('重新检测'), undefined);
  globalThis.__SOURCE_IMPORT_INVOKE__ = async command => {
    assert.equal(command, 'app_catalog');
    return { version: 1, apps: [] };
  };
  await act(async () => {
    Object.getOwnPropertyDescriptor(window.HTMLSelectElement.prototype, 'value').set.call(select, 'auto');
    select.dispatchEvent(new window.Event('change', { bubbles: true }));
  });
  await click(button('重新检测'));
  assert.equal(downloadRoutes.getSnapshot().mode, 'auto');
  assert.match(document.body.textContent, /自动 · 当前使用/);
});
test('The form describes one folder, both runtimes and separate file/browser data',async()=>{
 await setup();assert.match(document.body.textContent,/直接使用这一份代码/);assert.doesNotMatch(document.body.textContent,/源码记录与恢复|运行平台准备好的版本|同意并继续/);assert.match(document.body.textContent,/文件数据.*data\//);assert.match(document.body.textContent,/浏览器数据/);assert.match(document.body.textContent,/需要运行应用自带的准备脚本时，先由你确认/);
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
 await setup({node_git_import_start:()=>{throw {code:'NODE_GIT_IMPORT_BUSY',message:'正在添加另一个项目',details:{operationOutcome:'not_started'}};}});await begin();assert.match(document.body.textContent,/尚未开始下载应用文件/);await click(button('返回填写'));assert.equal(document.getElementById('git-import-url').value,url);
});
test('Reconciliation of a running accepted import resumes progress until its final result',async()=>{
 let finish;const final=new Promise(resolve=>{finish=resolve;});let reads=0;
 const running=snapshot({task:task({status:'running',final:false,phase:'downloading'}),commitState:'not_committed',app:null});
 const fixture=await setup({node_git_import_start:()=>{throw Error('lost reply');},node_git_import_status:()=>{reads++;if(reads===1)throw Error('connection lost');if(reads===2)return running;return final;}});
 await begin();assert.match(document.body.textContent,/添加结果待核对/);await click(button('重新核对'));assert.doesNotMatch(document.body.textContent,/这个项目暂时不能添加|临时材料已清理/);assert.equal(button('取消添加').disabled,false);
 await act(async()=>finish(snapshot()));assert.match(document.body.textContent,/已添加「Disposable app」/);assert.equal(fixture.calls.filter(([command])=>command==='node_git_import_start').length,1);
});

test('Closing a running import backgrounds the same session and continues reporting card progress',async()=>{
 let backgrounds=0;let resolveStart;
 const start=new Promise(resolve=>{resolveStart=resolve;});
 const fixture=await setup({node_git_import_start:()=>start},{onBackground:()=>backgrounds++});
 await begin();assert.equal(fixture.states.at(-1).view,'running');assert.equal(fixture.states.at(-1).name,'one');
 await click(document.querySelector('.modal-close'));assert.equal(backgrounds,1);
 await fixture.render({open:false});assert.equal(document.querySelector('[role="dialog"]'),null);
 await act(async()=>resolveStart(task()));assert.equal(fixture.states.at(-1).view,'success');assert.equal(fixture.states.at(-1).snapshot.appId,appId);
 assert.equal(fixture.calls.filter(([command])=>command==='node_git_import_start').length,1);
 assert.equal(fixture.calls.filter(([command])=>command==='node_git_import_status').length,1);
});

test('A closed failed result retains its original session until its card is explicitly removed',async()=>{
 const closes=[];
 const failure=snapshot({app:null,commitState:'not_committed',task:task({status:'failed',error:{code:'FAIL',message:'需要修复'}})});
 const fixture=await setup({node_git_import_status:()=>failure},{onClose:value=>closes.push(value)});await begin();
 assert.equal(canDismissGitImport(fixture.states.at(-1)),true);await click(button('关闭'));assert.deepEqual(closes,[{keepSession:true}]);
 await fixture.render({open:false});await fixture.render({open:true});assert.match(document.body.textContent,/需要修复/);
 assert.equal(fixture.calls.filter(([command])=>command==='node_git_import_start').length,1);
});

test('Entering an installed application or switching to local addition preserves unfinished import cleanup',async()=>{
 const closes=[];let local=0;
 await setup({node_git_import_status:()=>snapshot({cleanupPending:true,remainingPaths:['/disposable/partial']})},{onClose:value=>closes.push(value)});await begin();await click(button('打开应用'));assert.deepEqual(closes,[{keepSession:true}]);
 await act(async()=>root.unmount());root=null;
 const failure=snapshot({app:null,commitState:'not_committed',cleanupPending:true,remainingPaths:['/disposable/partial'],task:task({status:'failed',error:{code:'FAIL',message:'需要修复'}})});
 await setup({node_git_import_status:()=>failure},{onClose:value=>closes.push(value),onOpenLocal:()=>local++});await begin();assert.equal(button('改用本地添加'),undefined);assert.ok(button('交给 AI 处理'));assert.equal(local,0);
});

test('Import cards show real phases and prohibit removing unknown or pending cleanup',async()=>{
 await setup();let views=0;let removes=0;
 const state={sessionId:'disposable',url,name:'临时应用',task:task({status:'running',final:false,phase:'dependencies'}),snapshot:null,view:'running',busy:true,error:'',warning:''};
 const render=async patch=>{Object.assign(state,patch);await act(async()=>root.render(h(ImportProjectCard,{state:{...state},onView:()=>views++,onRemove:()=>removes++})));};
 await render({});assert.match(document.body.textContent,/准备依赖中/);assert.doesNotMatch(document.body.textContent,/%/);assert.equal(button('移除'),undefined);
 await click(document.querySelector('.launcher-open'));assert.equal(views,1);
 await render({view:'unknown',busy:false,task:task(),snapshot:snapshot({commitState:'unknown'})});assert.equal(button('移除'),undefined);assert.equal(button('查看原因'),undefined);await click(document.querySelector('.import-project-card .launcher-open'));assert.equal(views,2);
 await render({view:'failed',task:task({status:'failed'}),snapshot:snapshot({commitState:'not_committed',cleanupPending:true})});assert.equal(button('移除'),undefined);
 await render({snapshot:snapshot({commitState:'not_committed'})});await click(button('移除'));assert.equal(removes,1);
});

test('Task trigger hides completed history and deduplicates active import attention by task ID',async()=>{
 await setup();let opened=false;
 const props={tasks:[task()],projectName:()=>null,open:false,onOpenChange:value=>{opened=value;}};
 const render=async patch=>{Object.assign(props,patch);await act(async()=>root.render(h(TaskCenter,props)));};
 await render({});assert.equal(document.querySelector('.radio-task-button'),null);
 const running=task({status:'running',final:false,phase:'downloading'});
 await render({tasks:[running,running],importFlow:{taskId:running.taskId,active:true,needsAttention:false,onOpen(){}}});
 assert.equal(document.querySelector('.radio-task-button').textContent,'正在下载 1 个应用');await click(document.querySelector('.radio-task-button'));assert.equal(opened,true);
 await render({open:true,tasks:[task({status:'failed'}),task({status:'failed'})],importFlow:{taskId:running.taskId,needsAttention:true,onOpen(){}}});
 assert.equal(document.querySelector('.radio-task-button').textContent,'1 项需要处理');assert.equal(document.querySelectorAll('[aria-label="需要处理的任务"] li').length,1);
});

test('A successful current import never masks a separate unresolved previous addition',async()=>{
 await setup();let recoveries=0;
 await act(async()=>root.render(h(TaskCenter,{tasks:[task()],projectName:()=>null,open:true,onOpenChange(){},importFlow:{taskId:'task_import',active:false,needsAttention:false,onOpen(){}},recoveryFlow:{needsAttention:true,onOpen:()=>recoveries++}})));
 assert.equal(document.querySelector('.radio-task-button').textContent,'1 项需要处理');assert.match(document.querySelector('.task-center-panel').textContent,/上次添加需要核对/);assert.ok(document.querySelector('[aria-label="最近完成的任务"]'));
 await click(button('查看上次添加'));assert.equal(recoveries,1);
});

test('An open task popover keeps its completion result and restores focus after the trigger disappears',async()=>{
 await setup();const settings=document.createElement('button');settings.className='radio-settings-button';document.body.append(settings);
 let opened=true;const props={tasks:[task({status:'running',final:false})],projectName:()=>null,open:true,onOpenChange:value=>{opened=value;}};
 await act(async()=>root.render(h(TaskCenter,props)));document.querySelector('.radio-task-button').focus();
 await act(async()=>root.render(h(TaskCenter,{...props,tasks:[task()]})));
 assert.equal(document.querySelector('.radio-task-button'),null);assert.ok(document.querySelector('[aria-label="最近完成的任务"]'));
 assert.equal(document.activeElement,document.querySelector('[aria-label="关闭任务"]'));await click(document.querySelector('[aria-label="关闭任务"]'));
 assert.equal(opened,false);assert.equal(document.activeElement,settings);settings.remove();
});

test('Task attention includes all items beyond the completed-history limit and final results beat stale active copies',async()=>{
 await setup();const values=Array.from({length:24},(_,i)=>task({taskId:`failure_${i}`,status:'failed'}));values.push(task({taskId:'resolved'}),task({taskId:'resolved',status:'running',final:false}));
 await act(async()=>root.render(h(TaskCenter,{tasks:values,projectName:()=>null,open:true,onOpenChange(){}})));
 assert.equal(document.querySelector('.radio-task-button').textContent,'24 项需要处理');assert.equal(document.querySelectorAll('[aria-label="需要处理的任务"] li').length,24);
 assert.equal(document.querySelector('[aria-label="进行中的任务"]'),null);
});

test('Recovered unknown or pending cleanup keeps a reopenable attention entry until reconciliation succeeds',async()=>{
 await setup();let reopen;const attention=[];let recovered=[{taskId:'disposable_recovered',status:'blocked',message:'需要核对原添加',cleanupPending:true,remainingPaths:['/disposable/partial']}];
 globalThis.__SOURCE_IMPORT_INVOKE__=async command=>{assert.equal(command,'source_import_recover');return recovered;};
 await act(async()=>root.render(h(GitImportRecoveryNotice,{instanceId:'instance_one',onOpenExisting(){},onOpenReady:value=>{reopen=value;},onAttentionChange:value=>attention.push(value)})));
 assert.equal(typeof reopen,'function');assert.equal(attention.at(-1),true);await click(button('关闭'));assert.equal(document.querySelector('[role="dialog"]'),null);assert.equal(attention.at(-1),true);
 await act(async()=>reopen());assert.match(document.body.textContent,/需要核对原添加/);recovered=[];await click(button('重新核对'));assert.equal(attention.at(-1),false);
 await act(async()=>root.unmount());root=null;assert.equal(reopen,null);
});

const webId='prj_22222222222222222222222222222222';
const webApp={id:webId,name:'Disposable webpage',description:'',runtime:'web',ownership:'platform',state:'ready',projectRoot:'/disposable/projects/web',documentEpoch:'epoch_web',issues:[],manifest:null};
const waiting=(revision='checked_one',changes={})=>snapshot({appId:webId,sourcePath:webApp.projectRoot,runtime:'web',webProjectId:webId,app:null,commitState:'not_committed',task:task({projectId:webId,status:'waitingConfirmation',final:false,phase:'awaiting-build-confirmation'}),buildConfirmation:{revision,sourcePath:webApp.projectRoot,sourceUrl:url,sourceDir:'.',script:'bundle:web'},...changes});
const webSuccess=()=>snapshot({appId:webId,sourcePath:webApp.projectRoot,runtime:'web',webProjectId:webId,app:webApp,task:task({projectId:webId})});
const queued=()=>task({status:'queued',final:false,phase:'queued'});

test('Selected agent build consent distinguishes the real preparation stage from the final folder', async () => {
 const selectedDirectory={path:'/disposable/user-agent',revision:'checked-user-agent'};
 const stage='/disposable/private-import/working';
 const fixture=await setup({choose_path:()=>selectedDirectory.path,source_import_directory_inspect:()=>selectedDirectory,node_git_import_start:queued,node_git_import_status:()=>waiting('checked-stage',{sourcePath:stage,buildConfirmation:{revision:'checked-stage',sourcePath:stage,sourceUrl:url,sourceDir:'.',script:'bundle:web'}})}, {frontendPlatform:macPlatform,initialSource:agentRecommendation});
 await click(button('选择文件夹…'));await click(button('开始添加'));
 assert.equal(document.querySelector('.agent-install-target code').textContent,selectedDirectory.path);
 const confirmation=document.querySelector('[aria-label="准备确认"]');
 assert.match(confirmation.textContent,/准备运行文件的位置：\/disposable\/private-import\/working/);
 assert.equal([...confirmation.querySelectorAll('code')].some(code=>code.textContent===selectedDirectory.path),false);
 assert.equal(fixture.calls.filter(([command])=>command==='node_git_import_confirm_build').length,0);
});

test('A webpage with committed output is added without any build consent and enters the web callback',async()=>{
 const fixture=await setup({node_git_import_status:webSuccess});await begin();assert.match(document.body.textContent,/已添加「Disposable webpage」/);assert.equal(button('准备并添加'),undefined);assert.equal(fixture.calls.filter(([command])=>command==='node_git_import_confirm_build').length,0);await click(button('打开应用'));assert.equal(fixture.entered[0].id,webId);assert.equal(fixture.webEntered.length,0);
});
test('Web build consent displays the exact risk and repository, survives backgrounding, and requires an explicit action',async()=>{
 let confirmed=false,backgrounds=0;const fixture=await setup({node_git_import_start:queued,node_git_import_status:()=>confirmed?webSuccess():waiting(),node_git_import_confirm_build:args=>{assert.equal(args.expectedRevision,'checked_one');confirmed=true;return task({projectId:webId,status:'running',final:false,phase:'building'});}},{onBackground:()=>backgrounds++});await begin();
 assert.match(document.body.textContent,/这个应用需要先准备运行文件。继续会运行应用自带的脚本（包括依赖安装脚本），可能需要几分钟。/);assert.match(document.body.textContent,/应用地址：https:\/\/github.com\/disposable\/one.git/);assert.match(document.body.textContent,/npm run bundle:web/);assert.ok(button('准备并添加'));assert.ok(button('取消'));assert.equal(fixture.states.at(-1).view,'waiting');assert.equal(canDismissGitImport(fixture.states.at(-1)),false);assert.equal(fixture.calls.filter(([command])=>command==='node_git_import_confirm_build').length,0);
 await click(document.querySelector('.modal-close'));assert.equal(backgrounds,1);await fixture.render({open:false});await fixture.render({open:true});assert.ok(button('准备并添加'));await click(button('准备并添加'));assert.match(document.body.textContent,/已添加「Disposable webpage」/);assert.equal(fixture.added,1);assert.equal(fixture.calls.filter(([command])=>command==='node_git_import_confirm_build').length,1);await click(button('打开应用'));assert.equal(fixture.entered[0].id,webId);
});
test('A changed build preview is shown again and the old consent is never sent',async()=>{
 let revision='checked_one',confirmed=false;const fixture=await setup({node_git_import_start:queued,node_git_import_status:()=>confirmed?webSuccess():waiting(revision),node_git_import_confirm_build:args=>{assert.equal(args.expectedRevision,'checked_two');confirmed=true;return task({projectId:webId,status:'running',final:false,phase:'building'});}});await begin();revision='checked_two';await click(button('准备并添加'));assert.match(document.body.textContent,/已变化，请核对新的确认内容/);assert.ok(button('准备并添加'));assert.equal(fixture.calls.filter(([command])=>command==='node_git_import_confirm_build').length,0);await click(button('准备并添加'));assert.equal(fixture.calls.filter(([command])=>command==='node_git_import_confirm_build').length,1);assert.equal(fixture.webEntered.length,0);
});
test('Canceling build confirmation waits for cleanup before the flow can be removed',async()=>{
 let cancelled=false,release;const cleanup=new Promise(resolve=>{release=resolve;});const fixture=await setup({node_git_import_start:queued,node_git_import_status:()=>cancelled?cleanup:waiting(),node_git_import_cancel:()=>{cancelled=true;return task({projectId:webId,status:'running',final:false,phase:'cleanup'});}});await begin();await click(button('取消'));assert.match(document.body.textContent,/正在停止并清理/);assert.equal(fixture.states.at(-1).busy,true);assert.equal(canDismissGitImport(fixture.states.at(-1)),false);assert.equal(fixture.calls.filter(([command])=>command==='node_git_import_confirm_build').length,0);
 await act(async()=>release(waiting('checked_one',{buildConfirmation:null,task:task({status:'failed',error:{code:'NODE_GIT_IMPORT_CANCELLED',message:'添加已取消'}})})));assert.match(document.body.textContent,/已取消添加/);assert.match(document.body.textContent,/本次下载及临时材料已清理/);assert.equal(canDismissGitImport(fixture.states.at(-1)),true);assert.equal(fixture.added,0);
});
test('A waiting import card presents confirmation attention and cannot be dismissed',async()=>{
 await setup();let views=0;await act(async()=>root.render(h(ImportProjectCard,{state:{sessionId:'web_waiting',url,name:'Waiting webpage',task:waiting().task,snapshot:waiting(),view:'waiting',busy:false,error:'',warning:''},onView:()=>views++,onRemove(){assert.fail('a pending confirmation is not dismissible');}})));assert.match(document.body.textContent,/需要确认/);assert.equal(button('移除'),undefined);assert.equal(button('查看确认'),undefined);await click(document.querySelector('.import-project-card .launcher-open'));assert.equal(views,1);
});
test('An already-added web repository uses the same duplicate choice and preserves its identity',async()=>{
 let opened;const fixture=await setup({}, {existingRepositories:[{url,branch:null,appId:webId,name:'Existing webpage',runtime:'web'}],onOpenExisting:id=>{opened=id;}});await begin();assert.match(document.body.textContent,/已经添加过：「Existing webpage」/);assert.equal(fixture.calls.length,0);await click(button('打开已添加的'));assert.equal(opened,webId);
});

test('A linked agent discovered after download asks for placement before registration', async () => {
 let placed = false; const chosen = {path: '/disposable/user-owned-linked-agent', revision: 'checked-linked-agent-folder'};
 const waitingPlacement = () => snapshot({app: null, commitState: 'not_committed', placementRequired: true, task: task({status: 'waitingConfirmation', final: false, phase: 'waitingPlacement'})});
 const fixture = await setup({
   node_git_import_start: () => task({status: 'running', final: false, phase: 'downloading'}),
   node_git_import_status: () => placed ? snapshot() : waitingPlacement(),
   choose_path: () => chosen.path,
   source_import_directory_inspect: () => chosen,
   source_import_confirm_placement: args => {assert.deepEqual(args.destination, chosen); placed = true; return task({status: 'running', final: false, phase: 'publishing'});},
 }, {frontendPlatform: macPlatform});
 await begin();
 assert.ok(document.querySelector('[aria-label="放在哪里"]'));
 assert.equal(fixture.added, 0); assert.equal(fixture.calls.filter(([command]) => command === 'source_import_confirm_placement').length, 0);
 await click(button('选择文件夹…')); await click(button('开始添加'));
 assert.equal(fixture.added, 1); assert.match(document.body.textContent, /已添加「Disposable app」/);
 assert.equal(fixture.calls.filter(([command]) => command === 'node_git_import_start').length, 1);
 assert.equal(fixture.calls.filter(([command]) => command === 'source_import_confirm_placement').length, 1);
});
