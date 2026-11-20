import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { cp, mkdir, mkdtemp, readFile, realpath, rm, symlink, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { prepareProductionAppResources } from './production-macos-resources.mjs';

const repository = fileURLToPath(new URL('../', import.meta.url));
const git = (cwd, ...args) => execFileSync('git', ['-C', cwd, '-c', 'core.hooksPath=/dev/null', ...args], { encoding: 'utf8' });

test('candidate resources use committed source, retain tool payloads, and ship distribution notices', { skip: process.platform !== 'darwin' }, async t => {
  const temp = await realpath(await mkdtemp(path.join(os.tmpdir(), 'panda-public-resources-')));
  t.after(() => rm(temp, { recursive: true, force: true }));
  const root = path.join(temp, 'source');
  const resources = path.join(temp, 'resources');
  await mkdir(root);
  await mkdir(resources);
  const inputs = git(repository, 'ls-files', '-z', '--', 'packages/sdk', 'templates/react', 'skills/local-projects-app',
    'schemas/panda.schema.json', 'README.md', 'LICENSE', 'apps/desktop/public/fonts/*LICENSE*', 'apps/desktop/public/ui/SOURCES.md',
    'crates/platform-core/src/project_directory_name_data/LICENSE.transliteration.txt').split('\0').filter(Boolean);
  inputs.push('distribution/recommended-asset-notices.txt');
  for (const name of inputs) {
    await mkdir(path.dirname(path.join(root, name)), { recursive: true });
    await cp(path.join(repository, name), path.join(root, name));
  }
  git(root, 'init', '--quiet');
  git(root, 'add', '-f', '--', ...inputs);
  git(root, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', '-c', 'commit.gpgsign=false', 'commit', '-qm', 'fixture');
  await symlink(path.join(repository, 'apps/desktop/node_modules'), path.join(root, 'apps/desktop/node_modules'));
  await writeFile(path.join(root, 'packages/sdk/.DS_Store'), 'ignored');
  await writeFile(path.join(root, 'packages/sdk/.env'), 'ignored');
  await writeFile(path.join(root, 'packages/sdk/sdk.js'), 'working tree change');
  await mkdir(path.join(resources, 'node-runtime'));
  await writeFile(path.join(resources, 'node-runtime/LICENSE'), 'existing tool license');

  await prepareProductionAppResources({ root, resources });
  assert.deepEqual(await readFile(path.join(resources, 'sdk/sdk.js')), await readFile(path.join(repository, 'packages/sdk/sdk.js')));
  for (const name of ['sdk/.DS_Store', 'sdk/.env', 'sdk/test/sdk.test.mjs']) {
    await assert.rejects(readFile(path.join(resources, name)), { code: 'ENOENT' });
  }
  assert.equal(await readFile(path.join(resources, 'node-runtime/LICENSE'), 'utf8'), 'existing tool license');
  for (const name of ['react', 'react-dom', 'lucide-react', '@tauri-apps/api', 'scheduler']) {
    assert.ok((await readFile(path.join(resources, 'licenses/frontend/SOURCES.txt'), 'utf8')).includes(name + '@'));
  }
  assert.deepEqual(await readFile(path.join(resources, 'licenses/frontend/lucide-react/LICENSE')),
    await readFile(path.join(repository, 'apps/desktop/node_modules/lucide-react/LICENSE')));
  assert.deepEqual(await readFile(path.join(resources, 'LICENSE')), await readFile(path.join(repository, 'LICENSE')));
  assert.match(await readFile(path.join(resources, 'licenses/recommended-asset-notices.txt'), 'utf8'), /Gleaner/);

  await symlink(path.join(root, 'README.md'), path.join(root, 'packages/sdk/unsafe.js'));
  git(root, 'add', 'packages/sdk/unsafe.js');
  git(root, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', '-c', 'commit.gpgsign=false', 'commit', '-qm', 'unsupported link');
  const other = path.join(temp, 'other');
  await mkdir(other);
  await assert.rejects(prepareProductionAppResources({ root, resources: other }), /ordinary file/);
});
