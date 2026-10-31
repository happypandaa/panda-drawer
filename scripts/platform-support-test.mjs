// Exercise the actual compiled core policy, including its real build profile.
// Unit tests do not initialize paths, launch the desktop or start a service.
import { lstat, mkdir, mkdtemp, realpath, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { root, run } from './process.mjs';

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
  // A standalone rustc build would omit the real build_profile and compiled
  // production anchors. Do not replace those dependencies with test stubs.
  await run(process.execPath, ['scripts/rust.mjs', 'test', '--locked', '-p',
    'platform-core', '--lib', 'platform_support::tests', '--', '--nocapture']);
  await writeFile(path.join(directory, 'result.json'), `${JSON.stringify({
    result: 'passed', host: process.platform, architecture: process.arch,
    source: 'crates/platform-core/src/platform_support.rs',
    scope: 'Compiled platform-core policy unit tests only; not Tauri, CLI, filesystem safety or host startup validation.'
  }, null, 2)}\n`, { flag: 'wx' });
  console.log('Policy module tests passed.');
}

try { await main(); }
catch (error) { console.error(error.message); process.exitCode = 1; }
