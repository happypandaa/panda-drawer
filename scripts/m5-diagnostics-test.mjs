// Diagnostic reads and metadata edits must not open or replace a damaged database.
// Only this newly created M5 disposable fixture is altered.
import assert from 'node:assert/strict';import {readFile,writeFile,rm} from 'node:fs/promises';import {join} from 'node:path';
import {cli,runtime,waitTask,succeeded,expectFailure,sha256,saveReport,readReport} from './m4-test-utils.mjs';
const current=runtime();const project=process.argv.includes('--resume')?(await readReport('m5-diagnostics-state.json')).project:cli(['project','create','--input','-'],{name:`M5 可丢弃 坏库与未构建概览 ${new Date().toISOString()}`});assert.match(project.name,/^M5 可丢弃/);await saveReport('m5-diagnostics-state.json',{project});
const id=project.id;const root=project.projectRoot;const database=join(root,'data/main.sqlite');
await writeFile(database,'M5 intentionally damaged diagnostic fixture');const hash=sha256(await readFile(database));
const metadata=cli(['project','metadata','get','--project',id]);assert.ok(metadata.metadataRevision);
const renamed=cli(['project','metadata','set','--project',id,'--input','-'],{expectedRevision:metadata.metadataRevision,description:'仅本轮可丢弃坏库样本；未自动修复或重建数据库'});assert.equal(renamed.name,project.name);
const task=waitTask(cli(['storage','scan','--project',id]));succeeded(task);const storage=cli(['storage','info','--project',id]);assert.equal(storage.currentProject.categories.database.bytes,(await readFile(database)).length);
assert.equal(sha256(await readFile(database)),hash,'metadata and storage cannot repair or replace the database');
await rm(join(root,'dist'),{recursive:true,force:true});const diagnostic=cli(['project','info','--project',id]);assert.equal(diagnostic.state,'error');assert.ok(diagnostic.issues.some(issue=>/artifact|webpage|产物|missing/i.test(issue))); assert.equal(diagnostic.name,project.name);assert.ok(diagnostic.metadataRevision);
const denied=expectFailure(['db','execute','--project',id,'--input','-'],{sql:'SELECT 1'},[]);assert.ok(denied.code);assert.equal(sha256(await readFile(database)),hash,'failed SQL cannot create an empty replacement');
await saveReport('m5-diagnostics.json',{ok:true,checkedAt:new Date().toISOString(),projectId:id,projectRoot:root,databaseSha256:hash,databaseBytes:(await readFile(database)).length,scanTask:task,sqlError:denied.code,checks:['damaged database metadata read/edit and storage scan leave original bytes untouched','missing registered artifact remains an explicit error while manager card and metadata remain available','failed SQL creates no replacement empty database']});
console.log(`PASS M5 damaged-database and missing-artifact diagnostics: ${id}`);
