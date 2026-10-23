// Developer-only resource assembly. This never starts the app or a project.
import { cp, lstat, mkdir, readFile, readdir, rename, stat, writeFile } from 'node:fs/promises';
import { randomUUID } from 'node:crypto';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { root } from './process.mjs';
import { packageTemplate } from './package-template.mjs';
import { packageWindowsNodeRuntime } from './package-windows-node-runtime.mjs';
import { prepareWindowsDevelopmentNode } from './package-windows-development-node.mjs';
import { prepareWindowsDevelopmentGit } from './package-windows-development-git.mjs';

const generatedParent = path.join(root, 'apps/desktop/src-tauri/resources');
const destination = path.join(generatedParent, 'windows');
const excluded = new Set(['node_modules', 'dist', '.git', '.hg', '.svn', '.vite', '.cache', 'test', 'tests']);

// This is a developer build input, never a runtime project-import entry point.
// Compile only the copied snapshot, retaining its complete template metadata.
export async function snapshotWindowsTemplate(templateDirectory, output) {
  if (!path.isAbsolute(templateDirectory)) throw new Error('The Windows template directory must be absolute.');
  const relativeOutput = path.relative(templateDirectory, output);
  if (!relativeOutput || (!relativeOutput.startsWith('..' + path.sep) && !path.isAbsolute(relativeOutput))) {
    throw new Error('The template snapshot output must be outside its source directory.');
  }
  for (const [relative, directory] of [['', true], ['app', true], ['template.json', false], ['README.md', false]]) {
    const info = await lstat(path.join(templateDirectory, relative));
    if (directory ? !info.isDirectory() : !info.isFile()) {
      throw new Error('Windows templates require ordinary app/, template.json and README.md entries.');
    }
  }
  await cp(templateDirectory, output, {
    recursive: true, errorOnExist: true, force: false,
    filter: async source => {
      if (path.relative(templateDirectory, source).split(path.sep).some(part => excluded.has(part))) return false;
      const info = await lstat(source);
      if (!info.isDirectory() && !info.isFile()) throw new Error('Windows templates accept ordinary files and directories only.');
      return true;
    },
  });
  // SDK 0.1.0 is immutable. Check the exact copied vendor bytes, not a second
  // read of the developer's source after its snapshot has already been taken.
  const sdkRelease = await readFile(path.join(root, 'packages/sdk/releases/local-projects-sdk-0.1.0.tgz'));
  const templateSdk = await readFile(path.join(output, 'app/vendor/local-projects-sdk-0.1.0.tgz'));
  if (!sdkRelease.equals(templateSdk)) throw new Error('The template SDK differs from the immutable SDK 0.1.0 release.');
}

export async function embeddedResourceFiles(directory) {
  const files = [];
  async function visit(relative = '') {
    const entries = await readdir(path.join(directory, relative), { withFileTypes: true });
    entries.sort((left, right) => Buffer.compare(Buffer.from(left.name), Buffer.from(right.name)));
    for (const entry of entries) {
      const label = relative ? relative + '/' + entry.name : entry.name;
      if (['embedded-project-resources.json', 'node-runtime', 'development-node', 'development-git'].includes(label)
        || (!relative && /^\.development-(node|git)-stage-/.test(entry.name))) continue;
      if (entry.isDirectory()) await visit(label);
      else if (entry.isFile()) files.push([label, [...await readFile(path.join(directory, label))]]);
      else throw new Error('Embedded resources accept ordinary files and directories only: ' + label);
    }
  }
  await visit();
  return files;
}

export async function prepareWindowsResources({ templateDirectory = path.join(root, 'templates/react'), nodeRuntimeArchive, developmentNodeArchive, developmentGitArchive } = {}) {
  if (process.platform !== 'win32' || process.arch !== 'x64') {
    throw new Error('Windows resource assembly currently requires Windows x64.');
  }
  if (!path.win32.isAbsolute(templateDirectory) || path.win32.parse(templateDirectory).root.length <= 1) {
    throw new Error('The Windows template directory must be absolute.');
  }
  await mkdir(generatedParent, { recursive: true });
  const stage = path.join(generatedParent, '.windows-preparing-' + randomUUID());
  await mkdir(stage);
  const filter = source => !path.relative(root, source).split(path.sep).some(part => excluded.has(part));
  // Both a failed stage and earlier resource assemblies remain in this ignored,
  // generated-only directory. No existing projects or source are removed.
  await cp(path.join(root, 'packages/sdk'), path.join(stage, 'sdk'), { recursive: true, filter });
  await snapshotWindowsTemplate(templateDirectory, path.join(stage, 'template'));
  await cp(path.join(root, 'skills/local-projects-app'), path.join(stage, 'skills/local-projects-app'), { recursive: true, filter });
  const starter = await packageTemplate(path.join(stage, 'template'));
  const privateNode = await packageWindowsNodeRuntime({
    output: path.join(stage, 'node-runtime'),
    cache: path.join(root, '.tools/windows-node-runtime-cache'),
    ...(nodeRuntimeArchive ? { archive: nodeRuntimeArchive } : {}),
  });
  const developmentNode = await prepareWindowsDevelopmentNode({
    outputDir: path.join(stage, 'development-node'), cacheDir: path.join(root, '.tools/windows-development-cache'),
    ...(developmentNodeArchive ? { archive: developmentNodeArchive } : {}),
  });
  const developmentGit = await prepareWindowsDevelopmentGit({
    outputDir: path.join(stage, 'development-git'), cacheDir: path.join(root, '.tools/windows-development-cache'),
    ...(developmentGitArchive ? { archive: developmentGitArchive } : {}),
  });
  await cp(path.join(root, 'docs/development.md'), path.join(stage, 'README.md'));
  await writeFile(path.join(stage, 'windows-resources.json'), JSON.stringify({
    schemaVersion: 1,
    platform: 'windows',
    architecture: 'x64',
    sdkVersion: '0.1.0',
    templateVersion: starter.templateVersion,
    cliIncluded: false,
    privateNodeVersion: privateNode.runtime.version,
    developmentNodeVersion: developmentNode.version,
    developmentGitVersion: developmentGit.version,
    preparedAt: Date.now(),
  }, null, 2) + '\n');
  // All SDK, source, starter, skill and documentation bytes are compiled into
  // the executable. The separate private Node payload is mapped by Tauri and
  // deliberately excluded from this JSON; webpage creation remains embedded.
  await writeFile(path.join(stage, 'embedded-project-resources.json'), JSON.stringify({
    schemaVersion: 1,
    files: await embeddedResourceFiles(stage),
  }) + '\n');
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
