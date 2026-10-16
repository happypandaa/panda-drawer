import { cp, mkdir, rm } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { root, run, rustEnvironment } from './process.mjs';
import { packageTemplate } from './package-template.mjs';

export async function prepareDesktop({ release = false } = {}) {
  if (process.platform !== 'darwin' || process.arch !== 'arm64') {
    throw new Error('M0 packaging currently supports macOS on Apple Silicon only.');
  }
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
