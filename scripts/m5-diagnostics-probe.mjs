// Manual diagnostic probe. Only this NEW disposable fixture is damaged.
import assert from 'node:assert/strict';import {mkdir,readFile,writeFile,rm} from 'node:fs/promises';import {join} from 'node:path';
import {cli,runtime,waitTask,succeeded,expectFailure,sha256,saveReport} from './m4-test-utils.mjs';
const current=runtime();const project=cli(['project','create','--input','-'],{name:`M5 可丢弃 坏库与缺入口诊断 ${new Date().toISOString()}`});
assert.match(project.name,/^M5 可丢弃/);const id=project.id,root=project.projectRoot,database=join(root,'data/main.sqlite');
await mkdir(join(root,'data'),{recursive:true});await writeFile(database,'M5 intentionally damaged diagnostic fixture');const hash=sha256(await readFile(database));
assert.equal(cli(['project','info','--project',id]).name,project.name);
const task=waitTask(cli(['storage','scan','--project',id]));succeeded(task);const storage=cli(['storage','info','--project',id]);
assert.equal(Number(storage.currentProject.categories.database.bytes),(await readFile(database)).length);
assert.equal(sha256(await readFile(database)),hash,'Info/storage cannot repair or replace database');
await rm(join(root,'dist'),{recursive:true,force:true});const diagnostic=cli(['project','info','--project',id]);assert.equal(diagnostic.state,'needsBuild');assert.equal(diagnostic.name,project.name);
const denied=expectFailure(['db','execute','--project',id,'--input','-'],{sql:'SELECT 1'},[]);assert.ok(denied.code);assert.equal(sha256(await readFile(database)),hash,'Failed SQL cannot replace database');
await saveReport('m5-diagnostics.json',{ok:true,instanceId:current.instanceId,projectId:id,projectRoot:root,databaseSha256:hash,scanTask:task,sqlError:denied.code,checks:['read-only diagnostics preserve damaged database','missing declared entry remains needsBuild','failed SQL creates no empty replacement']});
console.log(`PASS damaged-database and missing-entry diagnostics: ${id}`);
