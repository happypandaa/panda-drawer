import assert from 'node:assert/strict';
import { link, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { createWindowsToolManifest, inventoryPayload, payloadDigest, sha256 } from './production-windows-resources.mjs';
import { prepareProductionAppResources } from './production-macos-resources.mjs';

const repository = fileURLToPath(new URL('../', import.meta.url));
async function fixture(t) {
  const root = await realpath(await mkdtemp(path.join(os.tmpdir(), 'panda-production-windows-tools-test-')));
  t.after(async () => {
    assert.equal(await realpath(root), root);
    assert.match(path.basename(root), /^panda-production-windows-tools-test-/);
    await rm(root, { recursive: true });
  });
  return root;
}

test('shared public-resource preparation accepts the canonical Windows Git root spelling', { skip: process.platform !== 'win32' }, async t => {
  const resources = await fixture(t);
  await mkdir(path.join(resources, 'schemas'));
  // Deliberately stop at the first exclusive output directory. This proves
  // the real Git slash spelling passes admission without copying resources,
  // running pnpm, or depending on Windows symlink privileges.
  await assert.rejects(prepareProductionAppResources({ root: await realpath(repository), resources }), { code: 'EEXIST' });
});

test('tool inventory binds each real file including metadata and deterministic UTF-8 path order', async t => {
  const root = await fixture(t);
  await mkdir(path.join(root, 'nested'));
  await writeFile(path.join(root, 'z.txt'), 'z');
  await writeFile(path.join(root, 'nested', 'empty'), '');
  await writeFile(path.join(root, 'runtime.json'), '{"fixed":true}\n');
  const inventory = await inventoryPayload(root);
  assert.deepEqual(inventory.entries.map(entry => entry.path), ['nested/empty', 'runtime.json', 'z.txt']);
  assert.equal(inventory.fileCount, 3);
  assert.equal(inventory.totalBytes, 16);
  assert.equal(inventory.entries[0].sha256, sha256(Buffer.alloc(0)));
  const canonical = inventory.entries.map(entry => `${entry.path}\0${entry.size}\0${entry.sha256}\n`).join('');
  assert.equal(inventory.sha256, createHash('sha256').update(canonical).digest('hex'));
  await writeFile(path.join(root, 'z.txt'), 'changed');
  assert.notEqual((await inventoryPayload(root)).sha256, inventory.sha256);
});

test('hard-linked payloads cannot be accepted as immutable candidate files', async t => {
  const root = await fixture(t);
  await writeFile(path.join(root, 'first'), 'original');
  await link(path.join(root, 'first'), path.join(root, 'second'));
  await assert.rejects(inventoryPayload(root), /single-link/);
});

test('manifest binds separate Windows source bytes, full payload digests, versions and metadata', async t => {
  const root = await fixture(t);
  const sourceBytes = {};
  for (const [key, folder, source] of [
    ['nodeRuntime', 'node-runtime', 'node-runtime-release.windows-x64.json'],
    ['developmentNode', 'development-node', 'development-node-release.windows-x64.json'],
    ['developmentGit', 'development-git', 'development-git-release.windows-x64.json'],
  ]) {
    await mkdir(path.join(root, folder));
    await writeFile(path.join(root, folder, 'tool.exe'), `artificial-${key}`);
    await writeFile(path.join(root, folder, 'metadata.json'), 'fixture only; not vendor verification');
    sourceBytes[key] = await readFile(path.join(repository, 'scripts', source));
  }
  const profileBytes = await readFile(path.join(repository, 'build-profiles/production.windows-x64.json'));
  const manifest = await createWindowsToolManifest({ profileBytes, sourceBytes, resources: root });
  assert.equal(manifest.schemaVersion, 1);
  assert.equal(manifest.digestRuleVersion, 1);
  assert.equal(manifest.profile.identifier, 'com.happypandaa.pandadrawer');
  assert.equal(manifest.profile.sha256, sha256(profileBytes));
  assert.equal(manifest.profile.target, 'x86_64-pc-windows-msvc');
  for (const key of Object.keys(sourceBytes)) {
    assert.equal(manifest.sourceAnchors[key], sha256(sourceBytes[key]));
    assert.equal(manifest.payloads[key].fileCount, 2);
    assert.equal(manifest.payloads[key].sha256, payloadDigest(manifest.payloads[key].entries));
    assert(manifest.payloads[key].entries.some(entry => entry.path === 'metadata.json'));
  }
  assert.equal(manifest.payloads.nodeRuntime.toolVersion, '24.22.0');
  assert.equal(manifest.payloads.developmentNode.npmVersion, '11.19.1');
  assert.equal(manifest.payloads.developmentGit.npmVersion, null);
  assert.equal(manifest.payloads.developmentGit.toolVersion, '2.56.0.windows.4');
});
