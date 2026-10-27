// Program rollback DOM fixtures only; no native program or data is changed.
import assert from 'node:assert/strict';
import { after, afterEach, test } from 'node:test';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';
import { JSDOM } from 'jsdom';

const repo = fileURLToPath(new URL('../../../', import.meta.url));
await mkdir(path.join(repo, '.test-artifacts'), { recursive: true });
const output = await mkdtemp(path.join(repo, '.test-artifacts/disposable-program-rollback-dom-'));
const mockPath = path.join(output, 'tauri-mock.mjs');
await writeFile(mockPath, 'export const invoke = (...args) => globalThis.__NODE_ROLLBACK_INVOKE__(...args);\n');
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
const { NodeRollbackButton } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/NodeRollbackButton.tsx'))).href);
let root; let nextTimer = 0;
const timeouts = new Map(); const intervals = new Map();
dom.window.setTimeout = fn => { const id = ++nextTimer; timeouts.set(id, fn); return id; };
dom.window.clearTimeout = id => timeouts.delete(id);
dom.window.setInterval = fn => { const id = ++nextTimer; intervals.set(id, fn); return id; };
dom.window.clearInterval = id => intervals.delete(id);
afterEach(async () => { if (root) await act(async () => root.unmount()); root = null; timeouts.clear(); intervals.clear(); dom.window.localStorage.clear(); });
after(async () => { dom.window.close(); await rm(output, { recursive: true, force: true }); });

const appId='11111111-1111-4111-8111-111111111111';
const currentGeneration='gen_'+'a'.repeat(32);
const previousGeneration='gen_'+'b'.repeat(32);
const preview={appId,canRollback:true,name:'Disposable app',currentVersion:'2.0.0',previousVersion:'1.0.0',currentGeneration,previousGeneration,expectedRevision:'c'.repeat(64)};
const applied={appId,operationOutcome:'committed',generation:previousGeneration,dataChanged:false,configurationChanged:false,started:false};
function deferred(){let resolve,reject;const promise=new Promise((yes,no)=>{resolve=yes;reject=no;});return {promise,resolve,reject};}
async function setup(overrides={},initialProps={}){
  const calls=[];let count=0;
  globalThis.__NODE_ROLLBACK_INVOKE__=async(command,args)=>{
    calls.push([command,args]);if(overrides[command])return overrides[command](args);
    if(command==='node_program_rollback_preview')return preview;
    if(command==='node_program_rollback_apply')return applied;
    throw Error(`Unexpected rollback command ${command}`);
  };
  document.body.innerHTML='<button id="trigger">Startup failure</button><div id="root"></div>';
  root=createRoot(document.getElementById('root'));
  let props={appId,instanceId:'instance_one',disabled:false,onApplied(){count++;},...initialProps};
  const render=async(next={})=>{props={...props,...next};await act(async()=>root.render(h(NodeRollbackButton,props)));};
  await render();return {calls,render,get count(){return count;}};
}
const byText=text=>[...document.querySelectorAll('button')].find(button=>button.textContent===text);
async function click(element){assert.ok(element);await act(async()=>element.click());}

test('Rollback is offered only after a valid old-program preview and does not mutate on inspection or cancellation',async()=>{
  const fixture=await setup();
  assert.deepEqual(fixture.calls,[['node_program_rollback_preview',{appId}]]);
  await click(byText('退回上一个版本'));
  assert.match(document.querySelector('[role=dialog]').textContent,/不会把记录、附件或设置恢复到旧时点/);
  assert.match(document.querySelector('[role=dialog]').textContent,/不会自动打开应用/);
  await click(byText('取消'));
  assert.equal(document.querySelector('[role=dialog]'),null);
  assert.equal(fixture.calls.length,1);
});

test('Missing or mismatched old-program previews never offer a rollback action',async()=>{
  for(const value of [{appId,canRollback:false},{...preview,appId:'foreign'}, {...preview,expectedRevision:'bad'}, {...preview,previousGeneration:currentGeneration}]){
    await setup({node_program_rollback_preview:()=>value});
    assert.equal(byText('退回上一个版本'),undefined);
    await act(async()=>root.unmount());root=null;
  }
});

test('A confirmed program rollback sends only the reviewed revision once and never starts an app',async()=>{
  const write=deferred();const fixture=await setup({node_program_rollback_apply:()=>write.promise});
  await click(byText('退回上一个版本'));await click(byText('确认退回程序'));
  assert.equal(document.querySelector('.modal-close').disabled,true);
  await click(byText('取消'));
  assert.ok(document.querySelector('[role=dialog]'));
  assert.equal(fixture.calls.filter(([command])=>command==='node_program_rollback_apply').length,1);
  await act(async()=>write.resolve(applied));
  assert.equal(fixture.count,1);
  assert.equal(document.querySelector('[role=dialog]'),null);
  assert.deepEqual(fixture.calls[1],['node_program_rollback_apply',{appId,input:{expectedRevision:preview.expectedRevision}}]);
  assert.ok(!fixture.calls.some(([command])=>/start|open|configuration_save/.test(command)));
});

test('A lost response requires an explicit fresh preview before retrying and never silently repeats the switch',async()=>{
  let reads=0;let writes=0;
  const fixture=await setup({
    node_program_rollback_preview:()=>++reads===1?preview:{...preview,expectedRevision:'d'.repeat(64)},
    node_program_rollback_apply:()=>{if(++writes===1)throw Error('Lost native response');return applied;},
  });
  await click(byText('退回上一个版本'));await click(byText('确认退回程序'));
  assert.equal(fixture.count,0);assert.equal(byText('确认退回程序'),undefined);
  assert.ok(byText('重新核对'));
  assert.equal(writes,1);
  await click(byText('重新核对'));await click(byText('确认退回程序'));
  assert.equal(writes,2);assert.equal(fixture.count,1);
  assert.equal(fixture.calls.filter(([command])=>command==='node_program_rollback_apply')[1][1].input.expectedRevision,'d'.repeat(64));
});

test('A mismatched switch result stays uncertain and never reports the old program as applied',async()=>{
  const fixture=await setup({node_program_rollback_apply:()=>({...applied,generation:currentGeneration})});
  await click(byText('退回上一个版本'));await click(byText('确认退回程序'));
  assert.equal(fixture.count,0);assert.ok(byText('重新核对'));
  assert.equal(fixture.calls.filter(([command])=>command==='node_program_rollback_apply').length,1);
});

test('Late previews and switch responses from a previous service instance cannot affect the current application',async()=>{
  const write=deferred();const fixture=await setup({node_program_rollback_apply:()=>write.promise});
  await click(byText('退回上一个版本'));await click(byText('确认退回程序'));
  await fixture.render({instanceId:'instance_two'});
  await act(async()=>write.resolve(applied));
  assert.equal(fixture.count,0);
  assert.equal(document.querySelector('[role=dialog]'),null);
  assert.ok(byText('退回上一个版本'));
});
