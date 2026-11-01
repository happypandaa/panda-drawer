// Manual probe: explicitly restart the development service after test:m5:live.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';import {join} from 'node:path';
import {runtime,readReport,session,business,sql,sha256,saveReport,info} from './m4-test-utils.mjs';
const saved=await readReport('m5-live-state.json');const current=runtime();assert.notEqual(current.instanceId,saved.instanceId);
const fresh=await session(current,saved.projectId);
const old=await business(current,saved.oldDocument,'context');assert.equal(old.status,401);
const project=info(saved.projectId);assert.equal(project.projectRoot,saved.root);assert.equal(project.name,saved.name);assert.match(project.iconDataUrl,/^data:image\/png;base64,/);
assert.equal(sql(saved.projectId,'SELECT value FROM m5_notes WHERE id=1').rows[0].value,'saved before author manifest edits');
assert.equal(sha256(await readFile(join(saved.root,'dist/index.html'))),saved.entryHash);
assert.equal(fresh.origin,saved.oldDocument.origin,'Fixed origin survives restart');
const sdk=await fetch(new URL('/_platform/sdk.js',fresh.url),{headers:{Cookie:fresh.cookie}});assert.equal(sdk.status,200);assert.equal(sha256(new Uint8Array(await sdk.arrayBuffer())),saved.sdkHash);
assert.equal((await business(current,fresh,'context')).status,200);
const file=await business(current,fresh,'file?path=photo.png');assert.equal(file.status,200);assert.equal(file.headers.get('content-type'),'image/png');
await saveReport('m5-restart.json',{ok:true,checkedAt:new Date().toISOString(),projectId:saved.projectId,beforeInstanceId:saved.instanceId,afterInstanceId:current.instanceId,checks:['old browser session rejected','author name/icon/database/attachments retained','new session reads saved content','SDK and webpage bytes unchanged']});
console.log(`PASS process restart and saved content: ${saved.projectId}`);
