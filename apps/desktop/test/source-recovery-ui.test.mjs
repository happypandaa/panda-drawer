// Interrupted-add recovery DOM fixtures only; no real source or installation is changed.
import assert from 'node:assert/strict';
import { after, afterEach, test } from 'node:test';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';
import { JSDOM } from 'jsdom';

const repo = fileURLToPath(new URL('../../../', import.meta.url));
await mkdir(path.join(repo, '.test-artifacts'), { recursive: true });
const output = await mkdtemp(path.join(repo, '.test-artifacts/disposable-source-recovery-dom-'));
const mockPath = path.join(output, 'tauri-mock.mjs');
await writeFile(mockPath, 'export const invoke = (...args) => globalThis.__SOURCE_RECOVERY_INVOKE__(...args);\n');
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
const { GitImportRecoveryNotice } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/GitImportRecoveryNotice.tsx'))).href);
let root; let nextTimer = 0;
const timeouts = new Map(); const intervals = new Map();
dom.window.setTimeout = fn => { const id = ++nextTimer; timeouts.set(id, fn); return id; };
dom.window.clearTimeout = id => timeouts.delete(id);
dom.window.setInterval = fn => { const id = ++nextTimer; intervals.set(id, fn); return id; };
dom.window.clearInterval = id => intervals.delete(id);
afterEach(async () => { if (root) await act(async () => root.unmount()); root = null; timeouts.clear(); intervals.clear(); dom.window.localStorage.clear(); });
after(async () => { dom.window.close(); await rm(output, { recursive: true, force: true }); });

function deferred(){let resolve,reject;const promise=new Promise((yes,no)=>{resolve=yes;reject=no;});return {promise,resolve,reject};}
const byText=text=>[...document.querySelectorAll('button')].find(button=>button.textContent===text);
async function click(element){assert.ok(element);await act(async()=>element.click());}
async function setup(recover,initialProps={}){
  const calls=[];const opened=[];let found=0;
  globalThis.__SOURCE_RECOVERY_INVOKE__=async(command,args)=>{calls.push([command,args]);assert.equal(command,'source_import_recover');return recover(args);};
  document.body.innerHTML='<div id="root"></div>';root=createRoot(document.getElementById('root'));
  let props={instanceId:'instance_one',onFound(){found++;},onOpenExisting:id=>opened.push(id),...initialProps};
  const render=async(next={})=>{props={...props,...next};await act(async()=>root.render(h(GitImportRecoveryNotice,props)));};
  await render();return {calls,opened,render,get found(){return found;}};
}

test('Interrupted additions display verified cleanup and existing installation results without reinstalling or opening automatically',async()=>{
  const fixture=await setup(()=>[
    {taskId:'task_cleaned',status:'cleaned',message:'Only this stopped attempt was cleaned',url:'https://github.com/disposable/one.git'},
    {taskId:'task_installed',status:'installed',message:'Existing installation was found',appId:'app_existing'},
  ]);
  assert.equal(fixture.found,1);assert.equal(fixture.opened.length,0);
  assert.match(document.body.textContent,/临时文件已清理/);
  assert.match(document.body.textContent,/应用已经添加/);
  assert.deepEqual(fixture.calls,[['source_import_recover',undefined]]);
  await click(byText('查看已添加的应用'));
  assert.deepEqual(fixture.opened,['app_existing']);
  assert.equal(document.querySelector('[role=dialog]'),null);
});

test('Unknown or running additions permit only explicit rechecking and never repeat downloading or installation',async()=>{
  const next=deferred();let reads=0;
  const attempts=[{taskId:'task_unknown',status:'blocked',message:'The original commit still needs verification'}, {taskId:'task_running',status:'running',message:'The owned task is still running'}];
  const fixture=await setup(()=>++reads===1?attempts:next.promise);
  assert.match(document.body.textContent,/添加结果仍需核对/);
  assert.equal(byText('继续添加'),undefined);
  await click(byText('重新核对'));
  assert.equal(byText('正在核对…').disabled,true);
  await click(byText('正在核对…'));
  assert.equal(reads,2);
  await act(async()=>next.resolve(attempts));
  assert.deepEqual(fixture.calls.map(([command])=>command),['source_import_recover','source_import_recover']);
  assert.deepEqual(fixture.opened,[]);
});

test('A service-instance change performs a fresh recovery read and discards the previous late response',async()=>{
  const old=deferred();let reads=0;
  const fixture=await setup(()=>++reads===1?old.promise:[{taskId:'task_fresh',status:'cleaned',message:'Fresh instance cleanup result'}]);
  await fixture.render({instanceId:'instance_two'});
  assert.equal(reads,2);
  await act(async()=>old.resolve([{taskId:'task_old',status:'installed',message:'Obsolete installation result',appId:'obsolete_app'}]));
  assert.match(document.body.textContent,/Fresh instance cleanup result/);
  assert.doesNotMatch(document.body.textContent,/Obsolete installation result/);
  assert.equal(fixture.found,1);assert.deepEqual(fixture.opened,[]);
});

test('Enabling error reporting reveals the retained recovery error without rerunning recovery or clearing results',async()=>{
  const fixture=await setup(()=>{throw Error('Artificial recovery read unavailable');});
  assert.equal(document.querySelector('[role=dialog]'),null);
  await fixture.render({reportErrors:true});
  assert.match(document.querySelector('[role=dialog]').textContent,/Artificial recovery read unavailable/);
  assert.equal(fixture.calls.length,1);
  assert.equal(fixture.found,0);
});

test('Recovered registered webpages open their web instance ID without a Node-only lookup',async()=>{
 const id='prj_22222222222222222222222222222222';const fixture=await setup(()=>[{taskId:'task_web_done',status:'installed',message:'The web project is registered',runtime:'web',appId:id}]);await click(byText('查看已添加的应用'));assert.deepEqual(fixture.opened,[id]);assert.deepEqual(fixture.calls.map(([command])=>command),['source_import_recover']);
});
