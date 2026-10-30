import assert from 'node:assert/strict';
import test from 'node:test';
import {mkdtemp,mkdir,readFile,readdir,writeFile,rename,rm,link,symlink} from 'node:fs/promises';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import os from 'node:os';
import {randomUUID} from 'node:crypto';
import path from 'node:path';
import {WINDOWS_NODE_RELEASE,packageNodeRuntimeWindows,parseWindowsNodeArguments,verifyWindowsNodePayload} from './package-node-runtime-windows.mjs';
import {NODE_RELEASE} from './package-node-runtime.mjs';
const windows=process.platform==='win32'&&process.arch==='x64';
const dependencies=process.env.WINDOWS_NATIVE_PACKAGER_HELPER?{helper:process.env.WINDOWS_NATIVE_PACKAGER_HELPER}:{};
const execute=promisify(execFile);
async function disposable(t){const root=await mkdtemp(path.join(os.tmpdir(),'disposable-windows-node-pack-'));t.after(()=>rm(root,{recursive:true,force:true}));return root;}

test('Windows release is explicit and does not replace the existing Mac anchor',()=>{
 assert.equal(WINDOWS_NODE_RELEASE.platform,'win32');assert.equal(WINDOWS_NODE_RELEASE.arch,'x64');
 assert.equal(WINDOWS_NODE_RELEASE.source.sha256,'f9683a19877e12a9b167305c8db71040d812876359de21f0d2e97c3f321f80c3');
 assert.equal(WINDOWS_NODE_RELEASE.binary.sha256,'ba93f9a70bd86fce3ebbb39ec758501458453467aa92a34a0a3602d1e3fc2b9c');
 assert.equal(WINDOWS_NODE_RELEASE.binary.size,93882184);
 assert.equal(WINDOWS_NODE_RELEASE.license.sha256,'ed34dd8e3f0a78dbaf00d0444ce8e285b015b765379c2e17880455f70370f8e9');
 assert.equal(NODE_RELEASE.platform,'darwin');assert.equal(NODE_RELEASE.binary.path,'bin/node');
});
test('Windows CLI accepts locations and offline mode without trust substitutions',()=>{
 assert.deepEqual(parseWindowsNodeArguments(['--output','C:\\new','--cache','D:\\cache','--offline']),{outputDir:'C:\\new',cacheDir:'D:\\cache',offline:true});
 for(const args of [[],['--metadata','x'],['--url','https://example.invalid'],['--version','26'],['--helper','x'],['--output','a','--output','b']])assert.throws(()=>parseWindowsNodeArguments(args));
});
test('corrupt offline archive is retained and nothing is published or executed',{skip:!windows},async t=>{
 const root=await disposable(t);const cache=path.join(root,'cache');await mkdir(cache);
 const archive=path.join(cache,WINDOWS_NODE_RELEASE.archiveName);await writeFile(archive,'caller corrupt archive');
 await assert.rejects(packageNodeRuntimeWindows({outputDir:path.join(root,'new'),cacheDir:cache,offline:true},dependencies),/SHA-256/);
 assert.equal(await readFile(archive,'utf8'),'caller corrupt archive');assert.deepEqual(await readdir(root),['cache']);
 await assert.rejects(packageNodeRuntimeWindows({outputDir:`${root}\\.\\normalized-output`,cacheDir:`${cache}\\.`,offline:true},dependencies),/SHA-256/);
 await assert.rejects(packageNodeRuntimeWindows({outputDir:`.test-artifacts/disposable-relative-node-${randomUUID()}`,cacheDir:cache,offline:true},dependencies),/SHA-256/);
});
test('Windows pack refuses empty occupied outputs and linked cache names without changing them',{skip:!windows},async t=>{
 const root=await disposable(t);const cache=path.join(root,'cache');await mkdir(cache);const archive=path.join(cache,WINDOWS_NODE_RELEASE.archiveName);await writeFile(archive,'unexecuted caller bytes');
 const occupied=path.join(root,'occupied');await mkdir(occupied);
 await assert.rejects(packageNodeRuntimeWindows({outputDir:occupied,cacheDir:cache,offline:true},dependencies),/already exists/);assert.deepEqual(await readdir(occupied),[]);
 const retained=path.join(root,'retained.zip');await link(archive,retained);
 await assert.rejects(packageNodeRuntimeWindows({outputDir:path.join(root,'new'),cacheDir:cache,offline:true},dependencies));assert.equal(await readFile(retained,'utf8'),'unexecuted caller bytes');
 const alias=path.join(root,'cache-junction');await symlink(cache,alias,'junction');
 await assert.rejects(packageNodeRuntimeWindows({outputDir:path.join(root,'another'),cacheDir:alias,offline:true},dependencies));assert.equal(await readFile(archive,'utf8'),'unexecuted caller bytes');
});
test('trailing output separator cannot hide an overlapping cache or start a receiver',{skip:!windows},async t=>{
 const root=await disposable(t);const occupied=path.join(root,'occupied');await mkdir(occupied);await writeFile(path.join(occupied,'caller'),'keep');
 await assert.rejects(packageNodeRuntimeWindows({outputDir:`${occupied}\\`,cacheDir:path.join(occupied,'cache')},{helper:path.join(root,'must-not-execute.exe')}),/must not overlap/);
 assert.deepEqual(await readdir(occupied),['caller']);assert.equal(await readFile(path.join(occupied,'caller'),'utf8'),'keep');
});
test('verified official Windows runtime relocates and does not replace an existing output',{skip:!windows||!process.env.NODE_RUNTIME_WINDOWS_TEST_CACHE},async t=>{
 const root=await disposable(t);const original=path.join(root,'运行时 中文 空格');
 const options={outputDir:original,cacheDir:process.env.NODE_RUNTIME_WINDOWS_TEST_CACHE,offline:true};
 await packageNodeRuntimeWindows(options,dependencies);
 assert.deepEqual((await readdir(original)).sort(),['LICENSE','node.exe','runtime.json']);
 await assert.rejects(packageNodeRuntimeWindows(options,dependencies),/already exists/);
 const moved=path.join(root,'移动后 中文 空格');await rename(original,moved);
 const report=await verifyWindowsNodePayload('runtime',moved,dependencies);
 assert.equal((await execute(report.nodePath,['--version'],{windowsHide:true,timeout:10000,env:{SystemRoot:process.env.SystemRoot,NODE_DISABLE_COMPILE_CACHE:'1',TEMP:root,TMP:root,USERPROFILE:root,HOME:root}})).stdout.trim(),'v24.22.0');
 await writeFile(path.join(moved,'unknown'),'keep');await assert.rejects(verifyWindowsNodePayload('runtime',moved,dependencies),/member set/);
 assert.equal(await readFile(path.join(moved,'unknown'),'utf8'),'keep');
});
