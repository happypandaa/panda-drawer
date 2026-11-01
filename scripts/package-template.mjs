import { cp, mkdir, readFile, stat } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const ignored = new Set(['node_modules', 'dist', '.git', '.hg', '.svn', '.vite', '.cache']);

// Ship author source only. A new project builds once in its own folder using
// the same managed npm runner as subsequent user-triggered builds.
export async function packageTemplate(output) {
  const source = path.join(output, 'app');
  const present = await stat(source).then(info => info.isDirectory()).catch(error => {
    if (error.code === 'ENOENT') return false;
    throw error;
  });
  if (!present) {
    const template = path.join(root, 'templates/react');
    await mkdir(output, { recursive: true });
    await cp(template, output, {
      recursive: true,
      filter: file => !path.relative(template, file).split(path.sep).some(part => ignored.has(part)),
    });
  }
  const packageJson = JSON.parse(await readFile(path.join(source, 'package.json'), 'utf8'));
  const lock = JSON.parse(await readFile(path.join(source, 'package-lock.json'), 'utf8'));
  if (![2, 3].includes(lock.lockfileVersion) || typeof packageJson.scripts?.build !== 'string') {
    throw new Error('The web template requires an npm v2/v3 lockfile and its declared build script.');
  }
  await readFile(path.join(source, 'index.html'));
  const template = JSON.parse(await readFile(path.join(output, 'template.json'), 'utf8'));
  return { templateVersion: template.templateVersion, sourceDirectory: '.', sdkApiVersion: template.sdkApiVersion, sdkScript: template.sdkScript };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const outputIndex = process.argv.indexOf('--output');
  const output = outputIndex >= 0 && process.argv[outputIndex + 1];
  if (!output || !path.isAbsolute(output)) throw new Error('Pass --output ABSOLUTE_TEMPLATE_RESOURCE_DIRECTORY');
  await packageTemplate(output);
}
