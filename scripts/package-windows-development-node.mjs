import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { writeFile } from 'node:fs/promises';
import { fixedRelease, inspectDevelopmentTree, parseDevelopmentArguments, prepareDevelopment, selectNodePayload } from './windows-development-tools.mjs';

const execute = promisify(execFile);
const { bytes, release } = await fixedRelease(new URL('./windows-development-node-release.json', import.meta.url));
if (release.kind !== 'development-node' || release.version !== '24.22.1' || release.npmVersion !== '11.19.1'
  || release.paths.executable !== 'node.exe' || release.paths.npmCli !== 'node_modules/npm/bin/npm-cli.js' || release.paths.npxCli !== 'node_modules/npm/bin/npx-cli.js'
  || release.source.url !== 'https://nodejs.org/dist/v24.22.1/node-v24.22.1-win-x64.zip'
  || release.source.sha256 !== 'd2046d28c2c3dcd211ac5145c0bf30e2047ef9dfb306693c8c6e2eccd8b95fa5'
  || release.tree.sha256 !== '4cf206d1c7b48a996c6a7d9008cbe07bd70ecbce10fbb7dd0162f5ff4576b273') throw new Error('Windows development Node metadata differs from the reviewed official release.');
export const WINDOWS_DEVELOPMENT_NODE_RELEASE = release;

async function probe(checked, expected) {
  const systemRoot = process.env.SystemRoot ?? process.env.SYSTEMROOT;
  if (!systemRoot || !path.isAbsolute(systemRoot)) throw new Error('A Windows system root is required for the development Node probe.');
  const userConfig = path.join(path.dirname(checked.root), 'npm-user.npmrc');
  const globalConfig = path.join(path.dirname(checked.root), 'npm-global.npmrc');
  // npm rejects a single file loaded as both user and global configuration.
  // Use two explicit empty files in this disposable stage, never user settings.
  await writeFile(userConfig, '', { flag: 'wx' }); await writeFile(globalConfig, '', { flag: 'wx' });
  const options = { cwd: checked.root, env: { SystemRoot: systemRoot, PATH: `${checked.root};${path.join(systemRoot, 'System32')}`,
    npm_config_cache: path.join(path.dirname(checked.root), 'npm-probe-cache'), npm_config_userconfig: userConfig, npm_config_globalconfig: globalConfig,
    npm_config_update_notifier: 'false', npm_config_audit: 'false', npm_config_fund: 'false' },
  timeout: 15_000, maxBuffer: 4096, encoding: 'utf8', windowsHide: true, shell: false };
  const identity = JSON.parse((await execute(checked.executable, ['--input-type=commonjs', '--eval', 'process.stdout.write(JSON.stringify({version:process.versions.node,platform:process.platform,arch:process.arch}))'], options)).stdout);
  if (identity.version !== expected.version || identity.platform !== 'win32' || identity.arch !== 'x64') throw new Error('Development Node reported an incompatible execution identity.');
  for (const cli of [checked.npmCli, checked.npxCli]) {
    const actual = (await execute(checked.executable, [cli, '--version'], options)).stdout.trim();
    if (actual !== expected.npmVersion) throw new Error('Development npm/npx version differs from the fixed release.');
  }
}
const configuration = { release, metadataBytes: bytes, metadataFile: 'development-node.json', select: selectNodePayload, probe };
export const inspectWindowsDevelopmentNode = root => inspectDevelopmentTree(root, configuration);
export const prepareWindowsDevelopmentNode = (options = {}, dependencies = {}) => prepareDevelopment(options, configuration, dependencies);
export const parseWindowsDevelopmentNodeArguments = args => parseDevelopmentArguments(args, 'node');
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { const result = await prepareWindowsDevelopmentNode(parseWindowsDevelopmentNodeArguments(process.argv.slice(2))); console.log(JSON.stringify(result)); }
  catch (cause) { console.error('Windows development Node preparation failed: ' + cause.message); process.exitCode = 1; }
}
