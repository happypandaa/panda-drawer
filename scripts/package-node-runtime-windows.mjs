// Windows-only entry point. The existing Mac entry and exported release stay unchanged.
import {readFile, lstat} from 'node:fs/promises';
import {execFile, spawn} from 'node:child_process';
import {promisify} from 'node:util';
import {pipeline} from 'node:stream/promises';
import https from 'node:https';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {root, rustEnvironment} from './process.mjs';

function freeze(value){if(value&&typeof value==='object'){for(const child of Object.values(value))freeze(child);Object.freeze(value);}return value;}
export const WINDOWS_NODE_RELEASE = freeze(JSON.parse(await readFile(new URL('./node-runtime-release.windows-x64.json',import.meta.url),'utf8')));
if(WINDOWS_NODE_RELEASE.source.url!=='https://nodejs.org/download/release/v24.22.0/node-v24.22.0-win-x64.zip'||WINDOWS_NODE_RELEASE.source.sha256!=='f9683a19877e12a9b167305c8db71040d812876359de21f0d2e97c3f321f80c3'||WINDOWS_NODE_RELEASE.archiveName!=='node-v24.22.0-win-x64.zip')throw new Error('Invalid fixed Windows Node release declaration.');
const execute = promisify(execFile);
const target = path.join(root,'.tools','native-packager-target');
let built;
async function nativeHelper() {
  built ??= execute('cargo',['build','--locked','--offline','-p','native-packager','--target-dir',target], {cwd:root,env:rustEnvironment(),windowsHide:true,maxBuffer:1024*1024,timeout:120000});
  await built;
  return path.join(target,'debug','native-packager.exe');
}
function host() {
  if(process.platform!=='win32'||process.arch!=='x64') throw new Error('Windows Node preparation supports Windows x64 only.');
}
function input(value,label,absoluteOnly=false) {
  if(typeof value!=='string'||!value||absoluteOnly&&!path.isAbsolute(value)||/[\u0000-\u001f\u007f]/.test(value)||value.split(/[\\/]/).includes('..')) throw new Error(`${label} must be an explicit ${absoluteOnly?'absolute ':''}path without parent traversal or control characters.`);
  return path.resolve(value);
}
function contains(a,b) {const relative=path.relative(a.toLowerCase(),b.toLowerCase());return relative===''||!path.isAbsolute(relative)&&relative!=='..'&&!relative.startsWith(`..${path.sep}`);}
async function call(helper,args) {
  try { return JSON.parse((await execute(helper,args,{cwd:root,windowsHide:true,maxBuffer:1024*1024,timeout:120000})).stdout); }
  catch(error) {
    try {const detail=JSON.parse(error.stderr);const reported=new Error(detail.message,{cause:error});Object.assign(reported,detail);throw reported;}
    catch(parse) {if(parse.code==='WINDOWS_NODE_PACK_FAILED')throw parse;throw error;}
  }
}
async function receive(helper,cache) {
  const response=await new Promise((resolve,reject)=>{const request=https.get(WINDOWS_NODE_RELEASE.source.url,{headers:{'accept-encoding':'identity'},signal:AbortSignal.timeout(120000)},resolve);request.on('error',reject);});
  if(response.statusCode!==200||(response.headers['content-encoding']&&response.headers['content-encoding']!=='identity')) {response.destroy();throw new Error('Fixed official Node ZIP request rejected; redirects are not accepted.');}
  if(response.headers['content-length']&&(!/^\d+$/.test(response.headers['content-length'])||Number(response.headers['content-length'])>96*1024*1024)){response.destroy();throw new Error('Official ZIP exceeds its download budget.');}
  const child=spawn(helper,['receive',cache],{cwd:root,stdio:['pipe','pipe','pipe'],windowsHide:true});
  let stdout='',stderr='';child.stdout.setEncoding('utf8').on('data',value=>stdout+=value);child.stderr.setEncoding('utf8').on('data',value=>stderr+=value);
  const finished=new Promise((resolve,reject)=>{child.once('error',reject);child.once('close',code=>code===0?resolve():reject(new Error(stderr||`Native ZIP receiver exited ${code}`)));});
  const outcomes=await Promise.allSettled([pipeline(response,child.stdin),finished]);
  for(const outcome of outcomes)if(outcome.status==='rejected')throw outcome.reason;
  return JSON.parse(stdout);
}

// The optional second argument only selects a source-built helper for focused tests.
// The command line never accepts executable/release/URL/version substitutions.
export async function packageWindowsNodePayload(kind,{outputDir,output,cacheDir,cache,offline=false}={},dependencies={}) {
  host();
  if(!['runtime','development'].includes(kind))throw new Error('Unknown Windows Node payload kind.');
  if(outputDir!==undefined&&output!==undefined||cacheDir!==undefined&&cache!==undefined)throw new Error('Duplicate output/cache aliases.');
  const destination=input(outputDir??output,'Output',kind==='development');
  const cacheRoot=input(cacheDir??cache??path.join(root,'.tools','node-runtime-cache-windows-x64'),'Cache',kind==='development');
  if(contains(destination,cacheRoot)||contains(cacheRoot,destination))throw new Error('Runtime output and cache must not overlap.');
  const helper=dependencies.helper??await nativeHelper();
  // Native negative checks precede cache creation/download. Recheck every
  // actual ancestor ID before preparation, then hold its native scope through
  // publication; a path-only observation never supplies deletion authority.
  const outputGuard=await call(helper,['check-output',destination]);
  const archive=path.join(cacheRoot,WINDOWS_NODE_RELEASE.archiveName);
  let missing=false;
  try {await lstat(archive);}catch(error){if(error.code!=='ENOENT')throw error;missing=true;}
  if(missing){if(offline)throw new Error('The fixed official Node ZIP is absent from the offline cache.');await receive(helper,cacheRoot);}
  // The helper independently verifies the complete fixed archive and compiled
  // release before writing or executing. It probes inside private staging,
  // waits for its real Job/pipes to close, then publishes without replacement.
  return call(helper,['prepare',kind,archive,destination,JSON.stringify(outputGuard)]);
}
export function packageNodeRuntimeWindows(options={},dependencies={}) {return packageWindowsNodePayload('runtime',options,dependencies);}
export async function verifyWindowsNodePayload(kind,directory,dependencies={}) {
  host();return call(dependencies.helper??await nativeHelper(),['verify',kind,input(directory,'Root',true)]);
}
export function parseWindowsNodeArguments(args) {
  const options={};
  for(let i=0;i<args.length;i++){
    if(args[i]==='--offline'){if(options.offline)throw new Error('Duplicate offline option.');options.offline=true;continue;}
    const key=args[i]==='--output'?'outputDir':args[i]==='--cache'?'cacheDir':undefined;
    if(!key||options[key]!==undefined||!args[i+1]||args[i+1].startsWith('--'))throw new Error('Usage: --output <new-absolute-directory> [--cache <absolute-directory>] [--offline]');
    options[key]=args[++i];
  }
  if(!options.outputDir)throw new Error('An explicit new --output directory is required.');return options;
}
if(process.argv[1]&&path.resolve(process.argv[1])===fileURLToPath(import.meta.url)){
  try{console.log(JSON.stringify(await packageNodeRuntimeWindows(parseWindowsNodeArguments(process.argv.slice(2)))));}catch(error){console.error(`Windows Node preparation failed: ${error.message}`);process.exitCode=1;}
}
