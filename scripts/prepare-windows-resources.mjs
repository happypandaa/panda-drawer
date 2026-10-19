// Developer-only resource assembly. This never starts the app or a project.
import { cp, mkdir, readFile, rename, stat, writeFile } from 'node:fs/promises';
import { randomUUID } from 'node:crypto';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { root } from './process.mjs';
import { packageTemplate } from './package-template.mjs';

const generatedParent = path.join(root, 'apps/desktop/src-tauri/resources');
const destination = path.join(generatedParent, 'windows');

export async function prepareWindowsResources() {
  if (process.platform !== 'win32' || process.arch !== 'x64') {
    throw new Error('Windows resource assembly currently requires Windows x64.');
  }
  // SDK 0.1.0 is immutable. Use its retained original release bytes; do not
  // regenerate the package on another OS under the same version.
  const sdkRelease = await readFile(path.join(root, 'packages/sdk/releases/local-projects-sdk-0.1.0.tgz'));
  const templateSdk = await readFile(path.join(root, 'templates/react/app/vendor/local-projects-sdk-0.1.0.tgz'));
  if (!sdkRelease.equals(templateSdk)) throw new Error('The template SDK differs from the immutable SDK 0.1.0 release.');
  await mkdir(generatedParent, { recursive: true });
  const stage = path.join(generatedParent, '.windows-preparing-' + randomUUID());
  await mkdir(stage);
  const excluded = new Set(['node_modules', 'dist', '.git', '.hg', '.svn', '.vite', '.cache', 'test', 'tests']);
  const filter = source => !path.relative(root, source).split(path.sep).some(part => excluded.has(part));
  // Both a failed stage and earlier resource assemblies remain in this ignored,
  // generated-only directory. No existing projects or source are removed.
  await cp(path.join(root, 'packages/sdk'), path.join(stage, 'sdk'), { recursive: true, filter });
  await cp(path.join(root, 'templates/react'), path.join(stage, 'template'), { recursive: true, filter });
  await cp(path.join(root, 'skills/local-projects-app'), path.join(stage, 'skills/local-projects-app'), { recursive: true, filter });
  const starter = await packageTemplate(path.join(stage, 'template'));
  await cp(path.join(root, 'docs/development.md'), path.join(stage, 'README.md'));
  await writeFile(path.join(stage, 'windows-resources.json'), JSON.stringify({
    schemaVersion: 1,
    platform: 'windows',
    architecture: 'x64',
    sdkVersion: '0.1.0',
    templateVersion: starter.templateVersion,
    cliIncluded: false,
    preparedAt: Date.now(),
  }, null, 2) + '\n');
  const previous = await stat(destination).catch(error => {
    if (error.code === 'ENOENT') return null;
    throw error;
  });
  if (previous) {
    if (!previous.isDirectory()) throw new Error('The generated Windows resource destination is not a directory.');
    await rename(destination, path.join(generatedParent, 'windows-previous-' + randomUUID()));
  }
  await rename(stage, destination);
  console.log('Windows resources prepared: ' + destination);
  return destination;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  prepareWindowsResources().catch(error => { console.error(error.message); process.exitCode = 1; });
}
