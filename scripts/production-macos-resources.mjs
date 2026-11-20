// Assemble public application resources in a new candidate stage. Source files
// come from immutable Git blobs; installed runtime-tool trees have their own
// independent packaging and signature validation.
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { lstat, mkdir, readFile, realpath, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import path from 'node:path';
import { packageTemplate } from './package-template.mjs';

const run = promisify(execFile);
const directories = [
  ['packages/sdk', 'sdk'],
  ['templates/react', 'template'],
  ['skills/local-projects-app', 'skills/local-projects-app'],
];
const files = new Map([
  ['README.md', 'README.md'],
  ['LICENSE', 'LICENSE'],
  ['schemas/panda.schema.json', 'schemas/panda.schema.json'],
  ['distribution/recommended-asset-notices.txt', 'licenses/recommended-asset-notices.txt'],
  ['apps/desktop/public/ui/SOURCES.md', 'licenses/ui/SOURCES.md'],
  ['apps/desktop/public/fonts/doto-LICENSE.txt', 'licenses/fonts/doto-LICENSE.txt'],
  ['apps/desktop/public/fonts/space-mono-LICENSE.txt', 'licenses/fonts/space-mono-LICENSE.txt'],
  ['crates/platform-core/src/project_directory_name_data/LICENSE.transliteration.txt', 'licenses/transliteration/LICENSE'],
]);
const omitted = new Set(['node_modules', 'dist', 'test', 'tests', 'releases', 'vendor']);
const frontendPackages = [
  ['react', ['LICENSE']],
  ['react-dom', ['LICENSE']],
  ['scheduler', ['LICENSE']],
  ['lucide-react', ['LICENSE']],
  ['@tauri-apps/api', ['LICENSE-APACHE-2.0', 'LICENSE-MIT', 'LICENSE.spdx']],
];

function destination(source) {
  if (files.has(source)) return files.get(source);
  for (const [prefix, output] of directories) {
    if (!source.startsWith(prefix + '/')) continue;
    const relative = source.slice(prefix.length + 1);
    if (relative.split('/').some(part => omitted.has(part) || (part.startsWith('.') && part !== '.gitkeep'))) return;
    if (/[\x00-\x1f\x7f\\]/.test(relative)) throw new Error(`Unsupported release resource name: ${source}`);
    return path.join(output, relative);
  }
}

async function git(root, args) {
  const result = await run('git', ['-C', root, ...args], { encoding: 'buffer', maxBuffer: 4 * 1024 * 1024 });
  return result.stdout;
}

async function copyCommittedResources(root, resources, revision) {
  const sources = [...files.keys(), ...directories.map(([source]) => source)];
  const output = await git(root, ['ls-tree', '-r', '-z', '--full-tree', revision, '--', ...sources]);
  const seen = new Set();
  for (const row of output.toString('utf8').split('\0').filter(Boolean)) {
    const separator = row.indexOf('\t');
    const [mode, kind, id] = row.slice(0, separator).split(' ');
    const source = row.slice(separator + 1);
    const relative = destination(source);
    if (!relative) continue;
    if (kind !== 'blob' || !['100644', '100755'].includes(mode)) throw new Error(`Release resource must be an ordinary file: ${source}`);
    const target = path.join(resources, relative);
    await mkdir(path.dirname(target), { recursive: true });
    await writeFile(target, await git(root, ['cat-file', 'blob', id]), { flag: 'wx', mode: mode === '100755' ? 0o755 : 0o644 });
    seen.add(source);
  }
  for (const required of [...files.keys(), 'packages/sdk/sdk.js', 'packages/sdk/package.json', 'templates/react/app/package.json', 'skills/local-projects-app/SKILL.md']) {
    if (!seen.has(required)) throw new Error(`Missing committed release resource: ${required}`);
  }
  await packageTemplate(path.join(resources, 'template'));
}

async function copyFrontendNotices(root, resources) {
  const desktop = createRequire(path.join(root, 'apps/desktop/package.json'));
  const reactDom = createRequire(desktop.resolve('react-dom'));
  const origins = [];
  for (const [name, notices] of frontendPackages) {
    // Node resolves the package used by the frontend importer, including
    // scheduler resolved from react-dom's own dependency under pnpm.
    const importer = name === 'scheduler' ? reactDom : desktop;
    let directory = path.dirname(await realpath(importer.resolve(name)));
    let manifest;
    while (directory !== path.dirname(directory)) {
      const candidate = await readFile(path.join(directory, 'package.json'), 'utf8').catch(error => {
        if (error.code !== 'ENOENT') throw error;
      });
      if (candidate && (manifest = JSON.parse(candidate)).name === name) break;
      directory = path.dirname(directory);
    }
    if (manifest?.name !== name || !manifest.version) throw new Error(`Cannot identify installed package ${name}`);
    const out = path.join(resources, 'licenses', 'frontend', name);
    await mkdir(out, { recursive: true });
    for (const notice of notices) {
      const file = path.join(directory, notice);
      const stat = await lstat(file);
      if (!stat.isFile() || stat.size < 1 || stat.size > 1024 * 1024) throw new Error(`Invalid ${name}/${notice}`);
      await writeFile(path.join(out, notice), await readFile(file), { flag: 'wx', mode: 0o644 });
    }
    origins.push(`${name}@${manifest.version}  https://www.npmjs.com/package/${name}/v/${manifest.version}`);
  }
  await writeFile(path.join(resources, 'licenses/frontend/SOURCES.txt'), origins.join('\n') + '\n', { flag: 'wx', mode: 0o644 });
}

export async function prepareProductionAppResources({ root, resources }) {
  for (const dir of [root, resources]) {
    if (!path.isAbsolute(dir) || await realpath(dir) !== dir || !(await lstat(dir)).isDirectory()) throw new Error('Candidate resources require canonical source and staging directories.');
  }
  if ((await git(root, ['rev-parse', '--show-toplevel'])).toString('utf8').trimEnd() !== root) throw new Error('Candidate source must be the platform checkout root.');
  const revision = (await git(root, ['rev-parse', '--verify', 'HEAD^{commit}'])).toString('utf8').trim();
  // Exclusive top-level destinations also rule out stale stages and links.
  for (const directory of ['schemas', 'sdk', 'template', 'skills', 'licenses']) {
    await mkdir(path.join(resources, directory), { mode: 0o755 });
  }
  await copyCommittedResources(root, resources, revision);
  await copyFrontendNotices(root, resources);
}
