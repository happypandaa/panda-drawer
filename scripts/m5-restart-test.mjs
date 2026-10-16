// Run only after restarting the final packaged development process.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';import {join} from 'node:path';
import {runtime,readReport,cli,expectFailure,session,business,sql,sha256,saveReport} from './m4-test-utils.mjs';
const saved=await readReport('m5-live-state.json');const current=runtime();assert.notEqual(current.instanceId,saved.instanceId);assert.equal(current.workspaceRoot,join(saved.root,'../..'));
const old=await business(current,saved.oldDocument,'context');assert.equal(old.status,401);
const conflict=expectFailure(['project','metadata','set','--project',saved.projectId,'--input','-'],{expectedRevision:saved.oldRevision,name:'must not overwrite after restart'},[]);assert.equal(conflict.code,'METADATA_CHANGED');
const metadata=cli(['project','metadata','get','--project',saved.projectId]);assert.notEqual(metadata.metadataRevision,saved.oldRevision);assert.equal(metadata.name,'M5 可丢弃 已改名');assert.equal(metadata.iconStatus.state,'valid');
assert.equal(sql(saved.projectId,'SELECT value FROM m5_notes WHERE id=1').rows[0].value,'saved before metadata edits');
assert.equal(sha256(await readFile(join(saved.root,'dist/index.html'))),saved.entryHash);assert.equal(sha256(await readFile(join(saved.root,'app/vendor/local-projects-sdk-0.1.0.tgz'))),saved.sdkHash);
const fresh=await session(current,saved.projectId);assert.equal((await business(current,fresh,'context')).status,200);
const file=await business(current,fresh,'file?path=photo.png');assert.equal(file.status,200);assert.equal(file.headers.get('content-type'),'image/png');
await saveReport('m5-restart.json',{ok:true,checkedAt:new Date().toISOString(),projectId:saved.projectId,beforeInstanceId:saved.instanceId,afterInstanceId:current.instanceId,workspaceRoot:current.workspaceRoot,checks:['old browser session and metadata revision rejected','saved metadata/icon/database/attachments retained','new browser session reads saved content','fixed SDK and running artifact bytes unchanged']});
console.log(`PASS M5 process restart and saved content: ${saved.projectId}`);
