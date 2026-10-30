import assert from 'node:assert/strict';
import test from 'node:test';
import {mkdtemp,mkdir,readFile,readdir,writeFile,rename,rm,link,symlink} from 'node:fs/promises';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {root as repositoryRoot} from './process.mjs';
import os from 'node:os';
import path from 'node:path';
import {WINDOWS_DEVELOPMENT_GIT_RELEASE,prepareDevelopmentGitWindows,verifyDevelopmentGitWindows,parseWindowsGitArguments} from './package-development-git-windows.mjs';
const windows=process.platform==='win32'&&process.arch==='x64';
const dependencies=process.env.WINDOWS_NATIVE_PACKAGER_HELPER?{helper:process.env.WINDOWS_NATIVE_PACKAGER_HELPER}:{};
const execute=promisify(execFile);
async function disposable(t){const root=await mkdtemp(path.join(os.tmpdir(),'disposable-windows-mingit-pack-'));t.after(()=>rm(root,{recursive:true,force:true}));return root;}

test('Windows MinGit anchor keeps the official UCRT64 tree and Mac source release separate',async()=>{
 const release=WINDOWS_DEVELOPMENT_GIT_RELEASE;
 assert.equal(release.platform,'win32');assert.equal(release.arch,'x64');assert.equal(release.distribution,'MinGit');
 assert.equal(release.gitVersion,'2.56.0.windows.4');assert.equal(release.source.size,39846926);
 assert.equal(release.source.sha256,'e3bf78351fe777fba47ddee5950082f8a98bb0f02604fdc7ba3ee2cb68ffa877');
 assert.deepEqual(release.tree,{rule:'sorted-utf8-path-nul-size-nul-sha256-lf-v1',fileCount:375,totalBytes:96237338,sha256:'ed8f1fcdbfee9074b148cdf8a8f4d311ffb146ce11daeb8789a8f44fe3b3399b'});
 assert.equal(release.license.sha256,'454649ddc02b5cc098513cea28db6592b45ac0a906386287c4d48cf8dbde651c');
 assert.equal(release.layout.git,'cmd/git.exe');assert.equal(release.layout.execPath,'ucrt64/libexec/git-core');
 assert.equal(release.build,undefined);assert.equal(release.signature,undefined);assert.ok(Object.isFrozen(release.source));
 const mac=JSON.parse(await readFile(new URL('./development-git-release.json',import.meta.url),'utf8'));
 assert.equal(mac.platform,'darwin');assert.equal(mac.arch,'arm64');assert.equal(mac.gitVersion,'2.56.0');assert.ok(mac.executables.includes('bin/git'));
});
test('Windows Git CLI accepts only location/offline options',()=>{
 assert.deepEqual(parseWindowsGitArguments(['--output','D:\\new','--cache','D:\\cache','--offline']),{outputDir:'D:\\new',cacheDir:'D:\\cache',offline:true});
 for(const args of [[],['--source','x'],['--url','https://example.invalid'],['--metadata','x'],['--helper','x'],['--output','a','--output','b'],['--output','x','--offline','--offline']])assert.throws(()=>parseWindowsGitArguments(args));
});
test('corrupt MinGit archive is preserved without payload creation or execution',{skip:!windows},async t=>{
 const root=await disposable(t);const cache=path.join(root,'cache');await mkdir(cache);
 const archive=path.join(cache,WINDOWS_DEVELOPMENT_GIT_RELEASE.source.archiveName);await writeFile(archive,'caller archive bytes');
 await assert.rejects(prepareDevelopmentGitWindows({outputDir:path.join(root,'new'),cacheDir:cache,offline:true},dependencies),/SHA-256/);
 await assert.rejects(prepareDevelopmentGitWindows({outputDir:`${root}\\.\\normalized`,cacheDir:`${cache}\\.`,offline:true},dependencies),/SHA-256/);
 assert.equal(await readFile(archive,'utf8'),'caller archive bytes');assert.deepEqual(await readdir(root),['cache']);
});
test('native MinGit receiver removes its own rejected partial and preserves existing cache material',{skip:!windows},async t=>{
 const root=await disposable(t);const cache=path.join(root,'receiver-cache');await mkdir(cache);await writeFile(path.join(cache,'caller'),'keep');
 const result=await new Promise(resolve=>{
  const child=execFile(dependencies.helper??path.join(repositoryRoot,'.tools','native-packager-target','debug','native-packager.exe'),['git-receive',cache],{windowsHide:true,timeout:10000},(error,stdout,stderr)=>resolve({error,stdout,stderr}));
  child.stdin.end('bounded unverified bytes are not a ZIP');
 });
 assert.ok(result.error);assert.equal(JSON.parse(result.stderr).code,'WINDOWS_GIT_PACK_FAILED');assert.match(JSON.parse(result.stderr).message,/SHA-256/);
 assert.deepEqual(await readdir(cache),['caller']);assert.equal(await readFile(path.join(cache,'caller'),'utf8'),'keep');
});
test('MinGit rejects occupied outputs, junctions, hardlinks and trailing-separator overlap',{skip:!windows},async t=>{
 const root=await disposable(t);const cache=path.join(root,'cache');await mkdir(cache);
 const archive=path.join(cache,WINDOWS_DEVELOPMENT_GIT_RELEASE.source.archiveName);await writeFile(archive,'retain caller input');
 const occupied=path.join(root,'occupied');await mkdir(occupied);await writeFile(path.join(occupied,'caller'),'keep');
 await assert.rejects(prepareDevelopmentGitWindows({outputDir:occupied,cacheDir:cache,offline:true},dependencies),/already exists/);
 await assert.rejects(prepareDevelopmentGitWindows({outputDir:`${occupied}\\`,cacheDir:path.join(occupied,'cache')},{helper:path.join(root,'must-not-run.exe')}),/must not overlap/);
 assert.deepEqual(await readdir(occupied),['caller']);
 const hard=path.join(root,'caller-original');await link(archive,hard);
 await assert.rejects(prepareDevelopmentGitWindows({outputDir:path.join(root,'new'),cacheDir:cache,offline:true},dependencies));
 assert.equal(await readFile(hard,'utf8'),'retain caller input');
 const alias=path.join(root,'cache-alias');await symlink(cache,alias,'junction');
 await assert.rejects(prepareDevelopmentGitWindows({outputDir:path.join(root,'another'),cacheDir:alias,offline:true},dependencies));
 assert.equal(await readFile(archive,'utf8'),'retain caller input');
});
test('real verified MinGit relocates and creates, commits, branches and clones a disposable repository',{skip:!windows||!process.env.DEVELOPMENT_GIT_WINDOWS_TEST_CACHE},async t=>{
 const root=await disposable(t);const original=path.join(root,'Git 原始 中文 空格');
 await prepareDevelopmentGitWindows({outputDir:original,cacheDir:process.env.DEVELOPMENT_GIT_WINDOWS_TEST_CACHE,offline:true},dependencies);
 const moved=path.join(root,'Git 移动后 中文 空格');await rename(original,moved);
 const report=await verifyDevelopmentGitWindows(moved,dependencies);
 const home=path.join(root,'isolated-home');const temporary=path.join(root,'temp');const hooks=path.join(root,'empty-hooks');const repository=path.join(root,'临时 source repo');
 for(const folder of [home,temporary,hooks,repository])await mkdir(folder);
 const env={SystemRoot:process.env.SystemRoot,WINDIR:process.env.WINDIR,PATH:[path.join(moved,'cmd'),path.join(moved,'ucrt64','bin'),path.join(moved,'usr','bin'),path.join(process.env.SystemRoot,'System32')].join(';'),HOME:home,USERPROFILE:home,APPDATA:home,LOCALAPPDATA:home,XDG_CONFIG_HOME:home,TEMP:temporary,TMP:temporary,GIT_CONFIG_NOSYSTEM:'1',GIT_CONFIG_SYSTEM:'NUL',GIT_CONFIG_GLOBAL:'NUL',GIT_TERMINAL_PROMPT:'0',GCM_INTERACTIVE:'Never',GIT_CEILING_DIRECTORIES:root};
 const git=async(args,cwd=repository)=>(await execute(report.gitPath,['-c','core.longpaths=true','-c','core.autocrlf=false','-c','credential.helper=','-c',`core.hooksPath=${hooks}`,...args],{cwd,env,windowsHide:true,timeout:15000,maxBuffer:1024*1024})).stdout;
 assert.equal((await git(['--version'])).trim(),'git version 2.56.0.windows.4');
 assert.equal(path.resolve((await git(['--exec-path'])).trim()),path.resolve(report.execPath));
 const defaultRepository=path.join(root,'默认 template repo');await git(['init','--initial-branch=main',defaultRepository],root);
 for(const file of ['description','info/exclude','hooks/pre-commit.sample'])assert.deepEqual(await readFile(path.join(defaultRepository,'.git',file)),await readFile(path.join(report.templatesPath,file)));
 await git(['init','--initial-branch=main',`--template=${report.templatesPath}`]);
 const contents='disposable Windows Git bytes\n中文\n';await writeFile(path.join(repository,'中文 source.txt'),contents);
 await git(['add','--','中文 source.txt']);
 await git(['-c','user.name=Disposable Windows Git','-c','user.email=disposable@example.invalid','-c','commit.gpgsign=false','commit','-m','disposable fixed-payload validation']);
 assert.equal(await git(['status','--porcelain=v1','-z']),'');
 const head=(await git(['rev-parse','HEAD'])).trim();assert.match(head,/^[a-f0-9]{40}$/);
 await git(['switch','-c','disposable-branch']);await git(['switch','main']);
 const clone=path.join(root,'临时 cloned repo');await git(['clone','--no-local','--no-hardlinks',`--template=${report.templatesPath}`,repository,clone],root);
 assert.equal((await git(['rev-parse','HEAD'],clone)).trim(),head);assert.equal(await readFile(path.join(clone,'中文 source.txt'),'utf8'),contents);
 await verifyDevelopmentGitWindows(moved,dependencies);
 await writeFile(path.join(moved,'unexpected-member'),'keep');
 await assert.rejects(verifyDevelopmentGitWindows(moved,dependencies),/tree differs/);assert.equal(await readFile(path.join(moved,'unexpected-member'),'utf8'),'keep');
});
