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

const { NodeApplyChangesPanel } = await component('NodeApplyChangesPanel.tsx');
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
const source = {sourceId:'src_'+'a'.repeat(32),name:'disposable build',status:'ready',contentDigest:'b'.repeat(64),inspection:{kind:'node-service-source',issues:[]}};
const task = (extra={}) => ({taskId:'task_disposable_node_build',type:'nodeDevelopmentBuild',projectId:null,status:'running',final:false,phase:'building-source',...extra});
const developmentProps = extra => ({appId:'11111111-1111-4111-8111-111111111111',instanceId:'instance_one',disabled:false,running:false,onPendingChange(){},onInstalled(){},...extra});

test('Apply modifications prevents duplicate submissions and exposes a verified candidate without installing', async () => {
  const submission=deferred(); const calls=[]; const pending=[]; let final=false;
  await mount(NodeApplyChangesPanel,developmentProps({onPendingChange:value=>pending.push(value)}),async(command,args)=>{
    calls.push([command,args]);
    if(command==='node_development_build') return submission.promise;
    if(command==='task_get') return final?task({status:'succeeded',final:true,result:{appId:developmentProps().appId,source}}):task();
    throw Error(command);
  });
  await click('应用修改'); await click('应用修改');
  assert.equal(calls.filter(([command])=>command==='node_development_build').length,1);
  await act(async()=>submission.resolve(task())); assert.equal(button('应用修改').disabled,true);
  final=true; await tick(); await tick();
  assert.match(document.querySelector('.node-install-panel').textContent,/确认应用修改/);
  assert.equal(pending.at(-1),false);
  assert.equal(calls.some(([command])=>/node_install_apply|node_app_start|node_install_list/.test(command)),false);
});

test('Cancel waits for the terminal build result before releasing the application gate', async () => {
  const calls=[]; const pending=[]; let cancelled=false;
  await mount(NodeApplyChangesPanel,developmentProps({onPendingChange:value=>pending.push(value)}),async(command,args)=>{
    calls.push([command,args]); if(command==='node_development_build')return task();
    if(command==='node_development_cancel'){cancelled=true;return task();}
    if(command==='task_get')return cancelled?task({status:'cancelled',final:true,error:{message:'人工取消'}}):task();
    throw Error(command);
  });
  await click('应用修改'); await click('取消应用修改');
  assert.equal(button('应用修改').disabled,true); assert.equal(pending.at(-1),true);
  await tick(); assert.equal(pending.at(-1),false);
  assert.equal(document.querySelector('.node-install-panel'),null);
  assert.deepEqual(calls.find(([command])=>command==='node_development_cancel'),['node_development_cancel',{taskId:task().taskId}]);
});

test('A build result for another application is rejected and retry remains available', async () => {
  await mount(NodeApplyChangesPanel,developmentProps(),async command=>{
    if(command==='node_development_build')return task();
    if(command==='task_get')return task({status:'succeeded',final:true,result:{appId:'other_app',source}});
    throw Error(command);
  });
  await click('应用修改'); assert.equal(document.querySelector('.node-install-panel'),null);
  assert.match(document.querySelector('[role=alert]').textContent,/应用修改结果不完整/);
  assert.equal(button('应用修改').disabled,false);
});

test('Unmounted apply workflows ignore late task submissions without publishing candidates', async () => {
  const delayed=deferred(); const seen=[];
  await mount(NodeApplyChangesPanel,developmentProps({onTask:value=>seen.push(value)}),async command=>{assert.equal(command,'node_development_build');return delayed.promise;});
  await click('应用修改'); await act(async()=>root.unmount());root=null;
  await act(async()=>delayed.resolve(task({final:true,status:'succeeded',result:{appId:developmentProps().appId,source}})));
  assert.deepEqual(seen,[]);
});

test('Running applications require closing before applying source modifications', async () => {
  await mount(NodeApplyChangesPanel,developmentProps({running:true}),async command=>{throw Error(command);});
  assert.equal(button('应用修改').disabled,true);
  assert.match(document.body.textContent,/先关闭应用/);
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
