import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { fixedRelease, inspectDevelopmentTree, parseDevelopmentArguments, prepareDevelopment } from './windows-development-tools.mjs';

const execute = promisify(execFile);
const { bytes, release } = await fixedRelease(new URL('./windows-development-git-release.json', import.meta.url));
if (release.kind !== 'development-git' || release.version !== '2.56.0.windows.2'
  || release.paths.executable !== 'cmd/git.exe' || release.paths.execPath !== 'ucrt64/libexec/git-core' || release.paths.binPath !== 'ucrt64/bin'
  || release.source.url !== 'https://github.com/git-for-windows/git/releases/download/v2.56.0.windows.2/MinGit-2.56.0.2-64-bit.zip'
  || release.source.sha256 !== 'da35e72aa21c005a5a0d298cfbae110bc1609a815730ea0dde84b01a1b3cd3be'
  || release.tree.sha256 !== '50985a661335ffaed08e05ab966c0af1ae62b957edc375cd7af1ad62f315ebfa') throw new Error('Windows development Git metadata differs from the reviewed official release.');
export const WINDOWS_DEVELOPMENT_GIT_RELEASE = release;

async function probe(checked, expected) {
  const systemRoot = process.env.SystemRoot ?? process.env.SYSTEMROOT;
  if (!systemRoot || !path.isAbsolute(systemRoot)) throw new Error('A Windows system root is required for the development Git probe.');
  const options = { cwd: checked.root, env: { SystemRoot: systemRoot,
    PATH: [path.dirname(checked.executable), checked.binPath, path.join(checked.root, 'usr/bin'), path.join(systemRoot, 'System32')].join(';'),
    HOME: path.dirname(checked.root), USERPROFILE: path.dirname(checked.root), XDG_CONFIG_HOME: path.dirname(checked.root),
    GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_SYSTEM: 'NUL', GIT_CONFIG_GLOBAL: 'NUL', GIT_CONFIG_COUNT: '0', GIT_TERMINAL_PROMPT: '0',
    GIT_CEILING_DIRECTORIES: path.dirname(checked.root), GIT_DISCOVERY_ACROSS_FILESYSTEM: '0',
    GIT_EXEC_PATH: checked.execPath, GIT_TEMPLATE_DIR: checked.templatePath, GIT_SSL_CAINFO: checked.certificateBundle,
    GIT_OPTIONAL_LOCKS: '0', GIT_ATTR_NOSYSTEM: '1' }, timeout: 15_000, maxBuffer: 8192, encoding: 'utf8', windowsHide: true, shell: false };
  if ((await execute(checked.executable, ['--version'], options)).stdout.trim() !== `git version ${expected.version}`) throw new Error('Private MinGit reported an incompatible version.');
  const execPath = (await execute(checked.executable, ['--exec-path'], options)).stdout.trim();
  if (path.resolve(execPath).toLowerCase() !== path.resolve(checked.execPath).toLowerCase()) throw new Error('Private MinGit resolved helpers outside its fixed payload.');
  const config = (await execute(checked.executable, ['config', '--list', '--show-origin'], options)).stdout.trim();
  if (config) throw new Error('Private MinGit unexpectedly loaded configuration during the isolated probe.');
}
const configuration = { release, metadataBytes: bytes, metadataFile: 'development-git.json', select: () => true, probe };
export const inspectWindowsDevelopmentGit = root => inspectDevelopmentTree(root, configuration);
export const prepareWindowsDevelopmentGit = (options = {}, dependencies = {}) => prepareDevelopment(options, configuration, dependencies);
export const parseWindowsDevelopmentGitArguments = args => parseDevelopmentArguments(args, 'git');
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { const result = await prepareWindowsDevelopmentGit(parseWindowsDevelopmentGitArguments(process.argv.slice(2))); console.log(JSON.stringify(result)); }
  catch (cause) { console.error('Windows development Git preparation failed: ' + cause.message); process.exitCode = 1; }
}
