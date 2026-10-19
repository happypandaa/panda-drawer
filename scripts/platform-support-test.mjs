// Compile the actual, dependency-free production policy, without importing or
// starting the platform. This is not a platform-core/Tauri compilation check.
import { lstat, mkdir, mkdtemp, realpath, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { root, run, rustEnvironment } from './process.mjs';

async function main() {
  if (process.argv.length !== 2) throw new Error('Usage: node scripts/platform-support-test.mjs');
  const parent = path.join(root, '.test-artifacts');
  await mkdir(parent, { recursive: false }).catch((error) => {
    if (error.code !== 'EEXIST') throw error;
  });
  const info = await lstat(parent);
  if (!info.isDirectory() || info.isSymbolicLink()
    || path.relative(await realpath(root), await realpath(parent)).toLowerCase() !== '.test-artifacts') {
    throw new Error('Tests require an ordinary repository .test-artifacts directory.');
  }
  const directory = await mkdtemp(path.join(parent, 'w1-platform-support-'));
  console.log(`Policy module evidence: ${directory}`);
  const executable = path.join(directory, process.platform === 'win32' ? 'policy-tests.exe' : 'policy-tests');
  const environment = rustEnvironment();
  // Do not let inherited linker options redirect output outside this new tree.
  for (const key of Object.keys(environment)) {
    if (/^(CL|_CL_|LINK|_LINK_|TEMP|TMP|TMPDIR)$/i.test(key)) delete environment[key];
  }
  Object.assign(environment, { TEMP: directory, TMP: directory, TMPDIR: directory,
    RUSTUP_AUTO_INSTALL: '0', RUSTUP_NO_UPDATE_CHECK: '1' });
  const options = { cwd: directory, env: environment };
  const args = ['--edition=2021', '--test', '-D', 'warnings',
    path.join(root, 'crates', 'platform-host', 'src', 'platform_support.rs'), '-o', executable];
  if (process.platform === 'win32') args.push('--target', 'x86_64-pc-windows-msvc');
  await run('rustc', args, options);
  await run(executable, ['--nocapture'], options);
  await writeFile(path.join(directory, 'result.json'), `${JSON.stringify({
    result: 'passed', host: process.platform, architecture: process.arch,
    source: 'crates/platform-host/src/platform_support.rs',
    scope: 'Production policy module only; not platform-core, Tauri, CLI, filesystem safety or host startup validation.'
  }, null, 2)}\n`, { flag: 'wx' });
  console.log('Policy module tests passed.');
}

try { await main(); }
catch (error) { console.error(error.message); process.exitCode = 1; }
