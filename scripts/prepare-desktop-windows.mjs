// Windows distribution preparation always uses a new generated directory.
// It never replaces the Mac resources or adopts previous experimental outputs.
import { cp, lstat, mkdir, mkdtemp, realpath, writeFile } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import path from 'node:path';
import { root, run, rustEnvironment } from './process.mjs';
import { packageNodeRuntimeWindows } from './package-node-runtime-windows.mjs';
import { prepareDevelopmentNodeWindows } from './package-development-node-windows.mjs';
import { prepareDevelopmentGitWindows } from './package-development-git-windows.mjs';
import { packageTemplate } from './package-template.mjs';

export function windowsResourceConfiguration(preparationRoot) {
  return {
    bundle: {
      resources: { [`${path.join(preparationRoot, 'resources')}${path.sep}`]: 'resources/' },
      externalBin: [path.join(preparationRoot, 'binaries', 'local-projects-dev')],
    },
  };
}

export async function prepareDesktopWindows({ release = false, offline = false } = {}) {
  if (process.platform !== 'win32' || process.arch !== 'x64') {
    throw new Error('Windows desktop preparation requires Windows x64.');
  }
  const environment = rustEnvironment();
  const target = execFileSync('rustc', ['--print', 'host-tuple'], {
    env: environment, encoding: 'utf8', windowsHide: true,
  }).trim();
  if (target !== 'x86_64-pc-windows-msvc') throw new Error(`Unsupported Windows Rust host: ${target}`);
  const tools = path.join(root, '.tools');
  await mkdir(tools, { recursive: true });
  const info = await lstat(tools);
  if (!info.isDirectory() || info.isSymbolicLink()) throw new Error('Windows preparation requires an ordinary repository .tools directory.');
  const preparationRoot = await mkdtemp(path.join(await realpath(tools), 'desktop-windows-preparation-'));
  const targetDirectory = path.join(preparationRoot, 'target');
  environment.CARGO_TARGET_DIR = targetDirectory;
  environment.CARGO_BUILD_TARGET = target;
  // Preserve this new stage on failure for diagnosis. No old stage, resource
  // directory, installed application or package is recursively removed.
  const resources = path.join(preparationRoot, 'resources');
  const binaries = path.join(preparationRoot, 'binaries');
  await mkdir(resources);
  await mkdir(binaries);
  await run(process.execPath, ['scripts/package-sdk.mjs'], { windowsHide: true });
  const cacheDir = path.join(tools, 'node-runtime-cache-windows-x64');
  await packageNodeRuntimeWindows({ outputDir: path.join(resources, 'node-runtime'), cacheDir, offline });
  await prepareDevelopmentNodeWindows({ outputDir: path.join(resources, 'development-node'), cacheDir, offline });
  await prepareDevelopmentGitWindows({
    outputDir: path.join(resources, 'development-git'),
    cacheDir: path.join(tools, 'development-git-cache-windows-x64'), offline,
  });
  const filter = source => !source.split(path.sep).some(part => ['node_modules', 'dist', '.git', 'test', 'tests'].includes(part));
  for (const [source, destination] of [
    ['packages/sdk', 'sdk'], ['templates/react', 'template'],
    ['skills/local-projects-app', 'skills/local-projects-app'],
  ]) {
    await cp(path.join(root, source), path.join(resources, destination), { recursive: true, filter });
  }
  await packageTemplate(path.join(resources, 'template'));
  await cp(path.join(root, 'docs/development.md'), path.join(resources, 'README.md'));
  await run('cargo', ['build', '--locked', '-p', 'platform-cli', ...(release ? ['--release'] : [])], {
    env: environment, windowsHide: true,
  });
  await cp(path.join(targetDirectory, target, release ? 'release' : 'debug', 'local-projects-dev.exe'),
    path.join(binaries, `local-projects-dev-${target}.exe`), { errorOnExist: true, force: false });
  const configuration = windowsResourceConfiguration(preparationRoot);
  const configurationPath = path.join(preparationRoot, 'tauri-resources.json');
  await writeFile(configurationPath, `${JSON.stringify(configuration, null, 2)}\n`, { flag: 'wx' });
  console.log(`Windows resources prepared: ${configurationPath}`);
  return { preparationRoot, resources, configuration, configurationPath, target, targetDirectory };
}
