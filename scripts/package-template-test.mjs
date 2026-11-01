import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtemp, mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { packageTemplate } from './package-template.mjs';

async function fixture(t, lockVersion = 3) {
  const directory = await mkdtemp(path.join(tmpdir(), 'panda-disposable-template-packaging-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  await mkdir(path.join(directory, 'app'));
  await writeFile(path.join(directory, 'app/package.json'), JSON.stringify({ scripts: { build: 'node author-build.cjs' } }));
  await writeFile(path.join(directory, 'app/package-lock.json'), JSON.stringify({ lockfileVersion: lockVersion, packages: { '': {} } }));
  await writeFile(path.join(directory, 'app/index.html'), '<main>template source</main>');
  await writeFile(path.join(directory, 'template.json'), JSON.stringify({ templateVersion: 'disposable', sdkApiVersion: 1, sdkScript: '/_platform/sdk.js' }));
  return directory;
}

test('packages source without running its script or creating prebuilt generations', async t => {
  const directory = await fixture(t);
  const script = "throw new Error('packaging must not execute the author script')";
  await writeFile(path.join(directory, 'app/author-build.cjs'), script);
  assert.deepEqual(await packageTemplate(directory), { templateVersion: 'disposable', sourceDirectory: '.', sdkApiVersion: 1, sdkScript: '/_platform/sdk.js' });
  assert.deepEqual((await readdir(directory)).sort(), ['app', 'template.json']);
  assert.equal(await readFile(path.join(directory, 'app/author-build.cjs'), 'utf8'), script);
});

test('rejects a template whose npm lockfile cannot be used by the runtime builder', async t => {
  const directory = await fixture(t, 1);
  await assert.rejects(packageTemplate(directory), /npm v2\/v3 lockfile/);
});
