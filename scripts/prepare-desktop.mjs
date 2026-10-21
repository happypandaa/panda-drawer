import { constants } from 'node:fs';
import { cp, lstat, mkdir, open, readdir, realpath, rm } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { root, run, rustEnvironment } from './process.mjs';
import { packageTemplate } from './package-template.mjs';
import { packageNodeRuntime } from './package-node-runtime.mjs';
import { prepareDevelopmentNode } from './package-development-node.mjs';

// tauri-build copies resources over target/<profile>/resources. Earlier trial
// copies used 0444/0555 modes, so replacing them fails before bundling. Repair
// only the known generated Node tree; never recursively chmod user projects.
export async function repairGeneratedNodeRuntimePermissions(directory) {
  let rootInfo;
  try { rootInfo = await lstat(directory); } catch (error) { if (error.code === 'ENOENT') return; throw error; }
  if (!path.isAbsolute(directory) || !rootInfo.isDirectory() || rootInfo.isSymbolicLink() || await realpath(directory) !== directory) {
    throw new Error('Generated Node runtime copy is not an ordinary canonical directory.');
  }
  const expected = new Map([
    ['bin', { directory: true, mode: 0o755 }],
    ['bin/node', { directory: false, mode: 0o755 }],
    ['LICENSE', { directory: false, mode: 0o644 }],
    ['runtime.json', { directory: false, mode: 0o644 }],
  ]);
  const entries = [{ file: directory, info: rootInfo, mode: 0o755 }];
  for (const name of await readdir(directory)) {
    if (!['bin', 'LICENSE', 'runtime.json'].includes(name)) throw new Error('Generated Node runtime copy contains unexpected entries.');
  }
  for (const [relative, spec] of expected) {
    const file = path.join(directory, relative);
    let info;
    try { info = await lstat(file); } catch (error) { if (error.code === 'ENOENT') continue; throw error; }
    if (info.isSymbolicLink() || (spec.directory ? !info.isDirectory() : !info.isFile() || info.nlink !== 1)) {
      throw new Error('Generated Node runtime copy contains a link or special file.');
    }
    if (spec.directory && (await readdir(file)).some((name) => name !== 'node')) throw new Error('Generated Node bin copy contains unexpected entries.');
    entries.push({ file, info, mode: spec.mode });
  }
  for (const entry of entries) {
    const handle = await open(entry.file, constants.O_RDONLY | constants.O_NOFOLLOW | (entry.info.isDirectory() ? constants.O_DIRECTORY : 0));
    try {
      const current = await handle.stat();
      if (current.dev !== entry.info.dev || current.ino !== entry.info.ino) throw new Error('Generated Node runtime copy changed identity.');
      await handle.chmod(entry.mode);
    } finally { await handle.close(); }
  }
}

export async function prepareDesktop({ release = false } = {}) {
  if (process.platform !== 'darwin' || process.arch !== 'arm64') {
    throw new Error('M0 packaging currently supports macOS on Apple Silicon only.');
  }
  await repairGeneratedNodeRuntimePermissions(path.join(root, 'target', release ? 'release' : 'debug', 'resources', 'node-runtime'));
  await run(process.execPath, ['scripts/package-sdk.mjs']);
  const target = execFileSync('rustc', ['--print', 'host-tuple'], { env: rustEnvironment(), encoding: 'utf8' }).trim();
  if (target !== 'aarch64-apple-darwin') throw new Error(`Unsupported target: ${target}`);
  await run('cargo', ['build', '--locked', '-p', 'platform-cli', ...(release ? ['--release'] : [])], {
    env: { ...rustEnvironment(), MACOSX_DEPLOYMENT_TARGET: '15.0' },
  });
  const tauriDir = path.join(root, 'apps/desktop/src-tauri');
  const binaries = path.join(tauriDir, 'binaries');
  await mkdir(binaries, { recursive: true });
  await cp(path.join(root, `target/${release ? 'release' : 'debug'}/local-projects-dev`), path.join(binaries, `local-projects-dev-${target}`));
  const resources = path.join(tauriDir, 'resources');
  // This generated directory contains distribution copies only, never user data.
  await rm(resources, { recursive: true, force: true });
  await mkdir(resources, { recursive: true });
  // A pinned private execution engine, separate from the developer's build Node.
  // The preparer verifies the official archive, executable and complete license.
  await packageNodeRuntime({
    output: path.join(resources, 'node-runtime'),
    cache: path.join(root, '.tools/node-runtime-cache'),
  });
  // Development is a separately verified Node/npm installation. Build selection
  // never changes the fixed execution engine used by registered applications.
  await prepareDevelopmentNode({
    outputDir: path.join(resources, 'development-node'),
    cacheDir: path.join(root, '.tools/node-runtime-cache'),
  });
  const resourceFilter = (source) => !source.split(path.sep).some(part => ['node_modules', 'dist', '.git', 'test', 'tests'].includes(part));
  await cp(path.join(root, 'packages/sdk'), path.join(resources, 'sdk'), { recursive: true, filter: resourceFilter });
  await cp(path.join(root, 'templates/react'), path.join(resources, 'template'), { recursive: true, filter: resourceFilter });
  await cp(path.join(root, 'skills/local-projects-app'), path.join(resources, 'skills/local-projects-app'), { recursive: true, filter: resourceFilter });
  // Built only by the platform developer. End users can create/open the starter without Node or a network connection.
  await packageTemplate(path.join(resources, 'template'));
  await cp(path.join(root, 'docs/development.md'), path.join(resources, 'README.md'));
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  try { await prepareDesktop(); } catch (error) { console.error(error.message); process.exitCode = 1; }
}
