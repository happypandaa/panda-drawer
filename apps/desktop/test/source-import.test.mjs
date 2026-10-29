// Pure command orchestration fixtures; no native install or network is exercised.
import assert from 'node:assert/strict';
import { after, afterEach, test } from 'node:test';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';

const repo = fileURLToPath(new URL('../../../', import.meta.url));
await mkdir(path.join(repo, '.test-artifacts'), { recursive: true });
const output = await mkdtemp(path.join(repo, '.test-artifacts/disposable-source-import-'));
const compiled = new Map();
async function compile(file) {
  if (compiled.has(file)) return compiled.get(file);
  const target = path.join(output, `${path.basename(file).replace(/\.tsx?$/, '')}.mjs`);
  compiled.set(file, target);
  let source = ts.transpileModule(await readFile(file, 'utf8'), { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX }, fileName: file }).outputText;
  for (const match of [...source.matchAll(/from\s+(['"])(\.[^'"]+)\1/g)]) {
    const base = path.resolve(path.dirname(file), match[2]);
    let dependency;
    for (const candidate of [base, `${base}.tsx`, `${base}.ts`]) {
      try { await readFile(candidate); dependency = candidate; break; } catch { /* Next extension. */ }
    }
    assert.ok(dependency, `Missing dependency ${base}`);
    source = source.replaceAll(`${match[1]}${match[2]}${match[1]}`, JSON.stringify(pathToFileURL(await compile(dependency)).href));
  }
  await writeFile(target, source);
  return target;
}
after(async () => { await rm(output, { recursive: true, force: true }); });
const { SourceImportSession, SourceImportError } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/source-import.ts'))).href);
const { acceptProjectDeletion, checkedProjectDeletion, deletedSourceMatches, observeDeletionRuntime, projectWasDeleted, taskWasDeleted, resetProjectDeletionsForTests } = await import(pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/project-deletion.ts'))).href);
afterEach(resetProjectDeletionsForTests);

const appId = '11111111-1111-4111-8111-111111111111';
const task = (changes={}) => ({taskId:'task_import',projectId:null,type:'nodeGitImport',status:'succeeded',final:true,phase:'completed',result:null,error:null,...changes});
const snapshot = (changes={}) => ({task:task(),appId,sourcePath:'/disposable/project-sources/one',commitState:'committed',cleanupPending:false,remainingPaths:[],app:null,...changes});
const input={url:'https://github.com/disposable/one.git',branch:null};
function fixture(overrides={}) {
 const calls=[]; const defaults={node_git_import_start:()=>task(),node_git_import_status:()=>snapshot(),node_git_import_retry_cleanup:()=>snapshot(),node_git_import_cancel:()=>task()};
 const session=new SourceImportSession(async(command,args)=>{calls.push([command,args]);return (overrides[command]??defaults[command])(args);},{isCurrent:()=>true,onTask(){},onChange(){},pause:async()=>{}});return {session,calls};
}
test('One backend transaction receives one stable request and replaces prepare/apply/publish orchestration',async()=>{
 const {session,calls}=fixture(); assert.equal((await session.run(input)).commitState,'committed');
 assert.deepEqual(calls.map(([command])=>command),['node_git_import_start','node_git_import_status']);
 assert.equal(calls[0][1].input.importId,session.importId);
 await assert.rejects(session.run(input),/已经受理/);assert.equal(calls.length,2);
});
test('A lost submission reply reconciles the original importId without resubmitting',async()=>{
 const {session,calls}=fixture({node_git_import_start:()=>{throw Error('lost reply');}});await session.run(input);
 assert.equal(calls.filter(([command])=>command==='node_git_import_start').length,1);
 for(const [command,args]of calls)if(command==='node_git_import_status')assert.equal(args.importId,session.importId);
});
test('Unknown registration is retained for reconciliation and never reported as added',async()=>{
 let confirmed=false;const {session,calls}=fixture({node_git_import_status:()=>snapshot({commitState:confirmed?'committed':'unknown',cleanupPending:!confirmed})});
 await assert.rejects(session.run(input),error=>error instanceof SourceImportError&&error.recovery);assert.equal(session.snapshot.commitState,'unknown');
 confirmed=true;assert.equal((await session.reconcile()).commitState,'committed');assert.equal(calls.filter(([command])=>command==='node_git_import_start').length,1);
});
test('Cleanup retry preserves the original failure and uses only its task',async()=>{
 const failed=snapshot({commitState:'not_committed',cleanupPending:true,remainingPaths:['/disposable/partial'],task:task({status:'failed',error:{code:'NEEDS_MODIFICATION',message:'需要修改'}})});
 const {session,calls}=fixture({node_git_import_status:()=>failed,node_git_import_retry_cleanup:()=>({...failed,cleanupPending:false,remainingPaths:[]})});
 await assert.rejects(session.run(input),/需要修改/);const value=await session.retryCleanup();assert.equal(value.commitState,'not_committed');assert.equal(value.task.error.message,'需要修改');
 assert.deepEqual(calls.at(-1),['node_git_import_retry_cleanup',{taskId:'task_import'}]);
});
test('Mismatched task or instance cannot drive subsequent mutations',async()=>{
 const {session}=fixture({node_git_import_status:()=>snapshot({task:task({type:'nodeInstallApply'})})});await assert.rejects(session.run(input),/身份/);
});

test('A definite pre-admission rejection does not become an unknown accepted import',async()=>{
 const {session,calls}=fixture({node_git_import_start:()=>{throw {code:'NODE_GIT_IMPORT_BUSY',message:'另一个添加任务未结束',details:{operationOutcome:'not_started'}};}});
 await assert.rejects(session.run(input),error=>error instanceof SourceImportError&&!error.recovery);assert.deepEqual(calls.map(([command])=>command),['node_git_import_start']);
});
