import {readFile} from 'node:fs/promises';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {packageWindowsNodePayload,parseWindowsNodeArguments,verifyWindowsNodePayload} from './package-node-runtime-windows.mjs';

export const WINDOWS_DEVELOPMENT_NODE_RELEASE=JSON.parse(await readFile(new URL('./development-node-release.windows-x64.json',import.meta.url),'utf8'));
export function prepareDevelopmentNodeWindows(options={},dependencies={}) {return packageWindowsNodePayload('development',options,dependencies);}
export function verifyDevelopmentNodeWindows(root,dependencies={}) {return verifyWindowsNodePayload('development',root,dependencies);}
if(process.argv[1]&&path.resolve(process.argv[1])===fileURLToPath(import.meta.url)){
  try{console.log(JSON.stringify(await prepareDevelopmentNodeWindows(parseWindowsNodeArguments(process.argv.slice(2)))));}catch(error){console.error(`Windows development Node preparation failed: ${error.message}`);process.exitCode=1;}
}
