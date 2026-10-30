// Explicit Windows entry point. The existing Mac release and entry stay fixed.
import {readFile,lstat} from 'node:fs/promises';
import {execFile,spawn} from 'node:child_process';
import {promisify} from 'node:util';
import {pipeline} from 'node:stream/promises';
import https from 'node:https';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {root,rustEnvironment} from './process.mjs';

function freeze(value){if(value&&typeof value==='object'){for(const child of Object.values(value))freeze(child);Object.freeze(value);}return value;}
export const WINDOWS_DEVELOPMENT_GIT_RELEASE=freeze(JSON.parse(await readFile(new URL('./development-git-release.windows-x64.json',import.meta.url),'utf8')));
const source=WINDOWS_DEVELOPMENT_GIT_RELEASE.source;
if(source.url!=='https://github.com/git-for-windows/git/releases/download/v2.56.0.windows.4/MinGit-2.56.0.4-64-bit.zip'||source.sha256!=='e3bf78351fe777fba47ddee5950082f8a98bb0f02604fdc7ba3ee2cb68ffa877'||source.size!==39846926||source.archiveName!=='MinGit-2.56.0.4-64-bit.zip')throw new Error('Invalid fixed Windows MinGit release declaration.');
const execute=promisify(execFile);
const target=path.join(root,'.tools','native-packager-target');
let built;
async function nativeHelper(){
 built??=execute('cargo',['build','--locked','--offline','-p','native-packager','--target-dir',target],{cwd:root,env:rustEnvironment(),windowsHide:true,maxBuffer:1024*1024,timeout:120000});
 await built;return path.join(target,'debug','native-packager.exe');
}
function host(){if(process.platform!=='win32'||process.arch!=='x64')throw new Error('Windows MinGit preparation supports Windows x64 only.');}
function input(value,label){
 if(typeof value!=='string'||!value||!path.isAbsolute(value)||/[\u0000-\u001f\u007f]/.test(value)||value.split(/[\\/]/).includes('..'))throw new Error(`${label} must be an explicit absolute path without parent traversal or control characters.`);
 return path.normalize(value);
}
function contains(a,b){const relative=path.relative(a.toLowerCase(),b.toLowerCase());return relative===''||!path.isAbsolute(relative)&&relative!=='..'&&!relative.startsWith(`..${path.sep}`);}
async function call(helper,args){
 try{return JSON.parse((await execute(helper,args,{cwd:root,windowsHide:true,maxBuffer:1024*1024,timeout:120000})).stdout);}
 catch(error){let detail;try{detail=JSON.parse(error.stderr);}catch{throw error;}if(detail.code==='WINDOWS_GIT_PACK_FAILED'){const reported=new Error(detail.message,{cause:error});Object.assign(reported,detail);throw reported;}throw error;}
}
async function response(url=source.url,hops=0){
 const reply=await new Promise((resolve,reject)=>{const request=https.get(url,{headers:{'accept-encoding':'identity','user-agent':'panda-drawer-windows-port'},signal:AbortSignal.timeout(120000)},resolve);request.once('error',reject);});
 if(reply.statusCode>=300&&reply.statusCode<400){
  reply.resume();if(hops>=2||!reply.headers.location)throw new Error('Official MinGit redirect limit exceeded.');
  const next=new URL(reply.headers.location,url);
  if(next.protocol!=='https:'||next.username||next.password||!['github.com','release-assets.githubusercontent.com','objects.githubusercontent.com'].includes(next.hostname))throw new Error('Unexpected official MinGit redirect host.');
  return response(next,hops+1);
 }
 if(reply.statusCode!==200||reply.headers['content-encoding']&&reply.headers['content-encoding']!=='identity'){reply.destroy();throw new Error('Official fixed MinGit ZIP request rejected.');}
 if(reply.headers['content-length']&&(!/^\d+$/.test(reply.headers['content-length'])||Number(reply.headers['content-length'])!==source.size)){reply.destroy();throw new Error('Official MinGit ZIP size differs from the fixed release.');}
 return reply;
}
async function receive(helper,cache){
 const reply=await response();
 const child=spawn(helper,['git-receive',cache],{cwd:root,windowsHide:true,stdio:['pipe','pipe','pipe']});
 let stdout='',stderr='';child.stdout.setEncoding('utf8').on('data',value=>stdout+=value);child.stderr.setEncoding('utf8').on('data',value=>stderr+=value);
 const finished=new Promise((resolve,reject)=>{child.once('error',reject);child.once('close',code=>code===0?resolve():reject(new Error(stderr||`Native MinGit receiver exited ${code}`)));});
 const results=await Promise.allSettled([pipeline(reply,child.stdin),finished]);
 for(const result of results)if(result.status==='rejected')throw result.reason;
 return JSON.parse(stdout);
}

// The focused-test helper selector is never accepted by the command line.
export async function packageDevelopmentGitWindows({outputDir,output,cacheDir,cache,offline=false}={},dependencies={}){
 host();
 if(outputDir!==undefined&&output!==undefined||cacheDir!==undefined&&cache!==undefined)throw new Error('Duplicate output/cache aliases.');
 const destination=input(outputDir??output,'Output');
 const cacheRoot=input(cacheDir??cache??path.join(root,'.tools','development-git-cache-windows-x64'),'Cache');
 if(contains(destination,cacheRoot)||contains(cacheRoot,destination))throw new Error('MinGit output and cache must not overlap.');
 const helper=dependencies.helper??await nativeHelper();
 const guard=await call(helper,['check-output',destination]);
 const archive=path.join(cacheRoot,source.archiveName);
 let missing=false;try{await lstat(archive);}catch(error){if(error.code!=='ENOENT')throw error;missing=true;}
 if(missing){if(offline)throw new Error('The fixed official MinGit ZIP is absent from the offline cache.');await receive(helper,cacheRoot);}
 return call(helper,['git-prepare',archive,destination,JSON.stringify(guard)]);
}
export async function verifyDevelopmentGitWindows(directory,dependencies={}){
 host();return call(dependencies.helper??await nativeHelper(),['git-verify',input(directory,'Root')]);
}
export const prepareDevelopmentGitWindows=packageDevelopmentGitWindows;
export function parseWindowsGitArguments(args){
 const options={};
 for(let index=0;index<args.length;index++){
  if(args[index]==='--offline'){if(options.offline)throw new Error('Duplicate offline option.');options.offline=true;continue;}
  const key=args[index]==='--output'?'outputDir':args[index]==='--cache'?'cacheDir':undefined;
  if(!key||options[key]!==undefined||!args[index+1]||args[index+1].startsWith('--'))throw new Error('Usage: --output <new-absolute-directory> [--cache <absolute-directory>] [--offline]');
  options[key]=args[++index];
 }
 if(!options.outputDir)throw new Error('An explicit new --output directory is required.');return options;
}
if(process.argv[1]&&path.resolve(process.argv[1])===fileURLToPath(import.meta.url)){
 try{console.log(JSON.stringify(await packageDevelopmentGitWindows(parseWindowsGitArguments(process.argv.slice(2)))));}catch(error){console.error(`Windows MinGit preparation failed: ${error.message}`);process.exitCode=1;}
}
