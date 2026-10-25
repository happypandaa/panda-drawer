// Non-destructive M2 acceptance probe: creates ONE new project in the fixed development workspace.
import assert from 'node:assert/strict';
import {spawnSync} from 'node:child_process';
import {readFile,writeFile,rm,mkdir,symlink,rename} from 'node:fs/promises';
import {join,resolve} from 'node:path';
import {homedir} from 'node:os';
import {createHash} from 'node:crypto';
const cliPath=resolve('target/release/bundle/macos/熊猫小抽屉.app/Contents/MacOS/local-projects-dev');
function invoke(args,input,allowError=false){const result=spawnSync(cliPath,[...args,'--json'],{input:input===undefined?undefined:JSON.stringify(input),encoding:'utf8',maxBuffer:10_000_000});assert.ifError(result.error);const wire=JSON.parse(result.stdout);if(!allowError)assert.equal(wire.ok,true,JSON.stringify(wire));return wire;}
const cli=(args,input)=>invoke(args,input).data;
const runtime=cli(['service','status']).runtime;
assert.equal(runtime.identifier,'dev.happypandaa.pandadrawer');assert.equal(runtime.environment,'development');
assert.equal(runtime.workspaceRoot,join(homedir(),'Library/Application Support/dev.happypandaa.pandadrawer/workspace'));
const project=cli(['project','create','--input','-'],{name:'M2 构建边界验证',description:'自动创建的可丢弃开发测试项目；构建失败、来源检测与无 Node 验证。'});
const id=project.id, root=project.projectRoot, origin=runtime.baseUrl;
const hash=bytes=>createHash('sha256').update(bytes).digest('hex');
const read=relative=>readFile(join(root,relative));
const getInfo=()=>cli(['project','info','--project',id]);
const taskRecords=[];
function build(){const submitted=cli(['project','build','--project',id]);let task;do{task=cli(['task','wait','--task',submitted.taskId]);}while(!task.final);taskRecords.push(task);return task;}
const source=await read('app/src/App.tsx'), packageBytes=await read('app/package.json');
const initialSdk=hash(await read('app/vendor/local-projects-sdk-0.1.0.tgz')),initialLock=hash(await read('app/package-lock.json'));
assert.equal(getInfo().state,'ready');
assert.equal(getInfo().buildInfo.sourceState,'matched');
assert.equal(getInfo().buildInfo.origin,'bundledTemplate');
assert.equal(getInfo().buildInfo.provenance,undefined);
const handoff=cli(['project','handoff','--project',id]);
assert.equal(handoff.skillSource,'project');
assert.equal(handoff.skillPath,join(root,'.agents/skills/local-projects-app/SKILL.md'));
assert.match(await readFile(handoff.skillPath,'utf8'),/name: local-projects-app/);
for(const entry of ['README.md','AGENTS.md','CLAUDE.md'])assert.match((await read(entry)).toString(),/\.agents\/skills\/local-projects-app\/SKILL\.md/);
let first=build();assert.equal(first.status,'succeeded',JSON.stringify(first));assert.equal(getInfo().buildInfo.sourceState,'matched');
console.log('PASS template build',first.taskId);
const launch=cli(['project','url','--project',id]);const ticket=new URLSearchParams(new URL(launch.launchUrl).hash.slice(1)).get('ticket');
const exchange=await fetch(origin+'/_platform/session',{method:'POST',headers:{Origin:origin,'Content-Type':'application/json'},body:JSON.stringify({ticket})});assert.equal(exchange.status,200);const cookie=exchange.headers.get('set-cookie').split(';')[0];
async function page(url=launch.url){const response=await fetch(url,{headers:{Cookie:cookie}});assert.equal(response.status,200,await response.clone().text());return response.text();}
const firstHtml=await page();assert.match(firstHtml,/_builds\/task_/);const epoch=/name="platform-document-epoch" content="([^"]+)"/.exec(firstHtml)[1];
const api=async(route,method='GET',body)=>fetch(`${origin}/p/${id}/api/v1/${route}`,{method,headers:{Cookie:cookie,Origin:origin,'X-Platform-Document-Epoch':epoch,...(typeof body==='string'?{'Content-Type':'application/json'}:{})},body});
const sql=async(text,params=[])=>{const response=await api('db/execute','POST',JSON.stringify({sql:text,params}));const wire=await response.json();assert.equal(wire.ok,true,JSON.stringify(wire));return wire.data;};
await sql('CREATE TABLE probe (id INTEGER PRIMARY KEY, value TEXT NOT NULL)');await sql('INSERT INTO probe VALUES (?,?)',[1,'saved-before-builds']);
const fileResponse=await api('file?path=probe.txt','PUT',new TextEncoder().encode('saved attachment'));assert.equal(fileResponse.status,200);
const oldBase=new URL(/<base href="([^"]+)"/.exec(firstHtml)[1],origin);const oldScript=new URL(/<script[^>]+src="([^"]+)"/.exec(firstHtml)[1],oldBase);const oldAsset=await page(oldScript.href);
await writeFile(join(root,'README.md'),(await read('README.md'))+'\nMetadata-only verification.\n');const manifest=JSON.parse(await read('project.json'));manifest.description+=' metadata edit';await writeFile(join(root,'project.json'),JSON.stringify(manifest,null,2));assert.equal(getInfo().buildInfo.sourceState,'matched');
await writeFile(join(root,'app/src/App.tsx'),Buffer.concat([source,Buffer.from('\n// source probe\n')]));assert.equal(getInfo().buildInfo.sourceState,'modified');
const controller=new AbortController();const events=await fetch(`${origin}/p/${id}/api/v1/events?documentEpoch=${encodeURIComponent(epoch)}`,{headers:{Cookie:cookie},signal:controller.signal});const reader=events.body.getReader();const eventResult=(async()=>{let text='';try{while(!text.includes('event: project.updated')){const chunk=await reader.read();if(chunk.done)break;text+=new TextDecoder().decode(chunk.value);}return text;}finally{controller.abort();}})();
let second=build();assert.equal(second.status,'succeeded',JSON.stringify(second));
const timer=setTimeout(()=>controller.abort(),10_000);try{assert.match(await eventResult,/event: project.updated/);}finally{clearTimeout(timer);controller.abort();}
assert.equal(await page(oldScript.href),oldAsset,'old version assets remain available');
const published=hash(await read('dist/index.html'));console.log('PASS published update event and old assets',second.taskId);
await writeFile(join(root,'app/src/App.tsx'),Buffer.concat([source,Buffer.from('\nthis is deliberately invalid TypeScript!!!\n')]));const failed=build();assert.equal(failed.status,'failed');assert.equal(hash(await read('dist/index.html')),published);
await writeFile(join(root,'app/src/App.tsx'),Buffer.concat([source,Buffer.from('\n// source probe\n')]));
const pkg=JSON.parse(packageBytes);pkg.scripts.build='node build-probe.mjs';await writeFile(join(root,'app/package.json'),JSON.stringify(pkg,null,2));
await writeFile(join(root,'app/build-probe.mjs'),`import{mkdirSync,writeFileSync}from'node:fs';const output=process.argv[process.argv.indexOf('--outDir')+1];writeFileSync('src/mutated-during-build.txt','change');mkdirSync(output,{recursive:true});writeFileSync(output+'/index.html','<html><head></head><body>must not publish</body></html>');`);
const changed=build();assert.equal(changed.status,'failed');assert.equal(changed.error.code,'BUILD_INPUT_CHANGED');assert.equal(hash(await read('dist/index.html')),published);
await writeFile(join(root,'app/package.json'),packageBytes);await rm(join(root,'app/build-probe.mjs'));await rm(join(root,'app/src/mutated-during-build.txt'));assert.equal(getInfo().buildInfo.sourceState,'matched');
console.log('PASS failed / input-changed builds preserve published page');
const environment=cli(['env','doctor']).buildEnvironment;const testNode=join(root,'.platform','test-node');await symlink(environment.node.path,testNode);try{cli(['env','node','set','--path',testNode]);await rename(testNode,testNode+'.offline');assert.equal(cli(['env','doctor']).buildEnvironment.ready,false);const rejected=invoke(['project','build','--project',id],undefined,true);assert.equal(rejected.ok,false);assert.equal(rejected.error.code,'NODE_NOT_FOUND');await page();assert.equal((await sql('SELECT value FROM probe WHERE id=1')).rows[0].value,'saved-before-builds');await sql('INSERT INTO probe VALUES (?,?)',[2,'saved-without-node']);assert.equal(await (await api('file?path=probe.txt')).text(),'saved attachment');}finally{if(environment.configuredNodePath)cli(['env','node','set','--path',environment.configuredNodePath]);else cli(['env','node','clear']);await rm(testNode,{force:true});await rm(testNode+'.offline',{force:true});}
assert.equal(hash(await read('app/vendor/local-projects-sdk-0.1.0.tgz')),initialSdk);assert.equal(hash(await read('app/package-lock.json')),initialLock);
const report={projectId:id,projectRoot:root,taskRecords,sdkSha256:initialSdk,lockSha256:initialLock,checks:['template build','input sourceState','metadata and data excluded','project.updated event','old generation remains available','failed build preserves dist/data/files','input mutation rejected','missing Node cannot build but can read/write','SDK and lock unchanged']};await mkdir('.test-artifacts',{recursive:true});await writeFile('.test-artifacts/m2-live.json',JSON.stringify(report,null,2));console.log('PASS controlled missing Node runtime; report .test-artifacts/m2-live.json');
