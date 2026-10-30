// CI runs the same source tests on both hosts. It never launches the desktop app
// or the live-test scripts that use a developer's registered applications.
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { appendFile, mkdir, mkdtemp, readFile, readdir, realpath } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';

const source = await realpath(path.resolve(process.argv[2] ?? '.'));
const metadata = JSON.parse(await readFile(path.join(source, 'package.json'), 'utf8'));
if (metadata.name !== 'panda-drawer') throw new Error('CI source must be the Panda Drawer repository.');
const artifacts = path.join(source, '.test-artifacts');
await mkdir(artifacts, { recursive: true });
const sandbox = await mkdtemp(path.join(artifacts, 'self-hosted-ci-'));
const environment = { ...process.env };
// Preserve toolchain locations before isolating the user's home. These contain
// compiler installations, not the platform's data or any registered projects.
environment.CARGO_HOME ??= path.join(os.homedir(), '.cargo');
environment.RUSTUP_HOME ??= path.join(os.homedir(), '.rustup');
const cargoBin = process.env.PANDA_CI_RUST_BIN ?? path.join(environment.CARGO_HOME, 'bin');
const pathKey = Object.keys(environment).find(key => key.toLowerCase() === 'path');
const inheritedPath = environment[pathKey] ?? '';
for (const key of Object.keys(environment)) if (key.toLowerCase() === 'path') delete environment[key];
environment.PATH = `${cargoBin}${path.delimiter}${inheritedPath}`;
const isolated = {
  HOME: 'home', USERPROFILE: 'home', LOCALAPPDATA: 'local', APPDATA: 'roaming',
  XDG_DATA_HOME: 'data', XDG_CONFIG_HOME: 'config', XDG_CACHE_HOME: 'cache',
  TMP: 'tmp', TEMP: 'tmp', TMPDIR: 'tmp', npm_config_cache: 'npm-cache',
};
// Windows KnownFolder APIs do not follow these home/data environment values.
// Isolation of platform data also depends on the explicit unit-test list below:
// Rust fixtures use private temporary paths and no selected test launches the
// normal desktop/runtime CLI. Never broaden this to live/restart scripts.
for (const [name, relative] of Object.entries(isolated)) {
  environment[name] = path.join(sandbox, relative);
  await mkdir(environment[name], { recursive: true });
}
environment.CI = 'true';
environment.GIT_TERMINAL_PROMPT = '0';
environment.MACOSX_DEPLOYMENT_TARGET = '15.0';

// npm.cmd is a shell wrapper, not an executable accepted by spawn on Windows.
// Invoke the installed npm CLI through node on both platforms.
const nodeDirectory = path.dirname(await realpath(process.execPath));
const npmCandidates = [
  process.env.npm_execpath,
  path.join(nodeDirectory, 'node_modules/npm/bin/npm-cli.js'),
  path.join(nodeDirectory, '../lib/node_modules/npm/bin/npm-cli.js'),
];
for (const directory of inheritedPath.split(path.delimiter)) {
  if (process.platform !== 'win32' && existsSync(path.join(directory, 'npm'))) {
    npmCandidates.push(await realpath(path.join(directory, 'npm')));
  }
}
const npm = npmCandidates.find(candidate => candidate && existsSync(candidate));
if (!npm) throw new Error('Node 24 and its npm CLI must be installed on the runner.');
const failures = [];
async function run(name, executable, args) {
  console.log(`\n::group::${name}`);
  const status = await new Promise(resolve => {
    const child = spawn(executable, args, { cwd: source, env: environment, stdio: 'inherit', windowsHide: true });
    child.once('error', error => { console.error(error.message); resolve(1); });
    child.once('exit', (code, signal) => resolve(code ?? signal ?? 1));
  });
  console.log('::endgroup::');
  if (status !== 0) failures.push({ name, status });
  if (process.env.GITHUB_STEP_SUMMARY) {
    await appendFile(process.env.GITHUB_STEP_SUMMARY, `- ${status === 0 ? 'PASS' : 'FAIL'}: ${name}\n`);
  }
}
const npmRun = (name, args) => run(name, process.execPath, [npm, ...args]);
await run('Node version', process.execPath, ['--version']);
await npmRun('Install locked frontend dependencies', ['ci', '--no-audit', '--no-fund']);
await npmRun('Generated project directory names', ['run', 'test:project-directory-names']);
if (metadata.scripts['test:ai-prompts']) await npmRun('Generated AI prompts', ['run', 'test:ai-prompts']);
await npmRun('TypeScript and Vite', ['run', 'build', '--workspace', '@local-projects/desktop']);
await npmRun('All UI regressions', ['run', 'test:ui']);
await npmRun('SDK regressions', ['run', 'test:sdk']);
// Explicit unit-test entry points: *live-test and *restart-test intentionally do
// not belong here. They launch a real platform and need a separate acceptance run.
const scriptTests = (await readdir(path.join(source, 'scripts')))
  .filter(name => /^(process|package-.+|local-directory-starter|desktop-(host|package-output)|production-macos-preflight|update-.+|m5-(browser-tools|media-panel))-test\.mjs$/.test(name))
  .sort().map(name => `scripts/${name}`);
await run('Script unit tests', process.execPath, ['--test', ...scriptTests]);
await npmRun('Platform support checks', ['run', 'test:platform-support']);
await npmRun('Prepare desktop resources', ['run', 'prepare:desktop']);
await run('Rust compile, all workspace targets', process.execPath, ['scripts/rust.mjs', 'check', '--locked', '--workspace', '--all-targets']);
await run('Rust tests, complete workspace', process.execPath, ['scripts/rust.mjs', 'test', '--locked', '--workspace']);
console.log(JSON.stringify({ source, sandbox, failures }, null, 2));
process.exitCode = failures.length ? 1 : 0;
