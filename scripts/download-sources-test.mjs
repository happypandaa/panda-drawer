import assert from 'node:assert/strict';
import test from 'node:test';
import { cp, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { DOWNLOAD_SOURCES, routeUpdateEndpoints, updateEndpoints, updateSource } from './download-sources.mjs';
import { releaseAssetUrl } from './update-manifest-protocol.mjs';

test('Platform updates use OSS in China and the public GitHub release repository elsewhere', () => {
  assert.deepEqual(Object.keys(DOWNLOAD_SOURCES.updates.sources), ['github', 'oss']);
  assert.deepEqual(Object.keys(DOWNLOAD_SOURCES.catalog.sources), ['github', 'gitee']);
  assert.deepEqual(DOWNLOAD_SOURCES.routes, {
    cn: { catalog: 'gitee', updates: 'oss', repository: 'gitee' },
    global: { catalog: 'github', updates: 'github', repository: 'github' },
  });
  const domestic = 'https://panda-updates.zhimibuhui.com/latest.json';
  const global = 'https://github.com/happypandaa/panda-drawer-releases/releases/latest/download/latest.json';
  assert.deepEqual(updateEndpoints(), [global]);
  assert.deepEqual(updateEndpoints(['github', 'github']), updateEndpoints());
  for (const source of ['gitee', 'https://example.invalid', '__proto__', 'constructor']) {
    assert.throws(() => updateSource(source), /Unregistered/);
    assert.throws(() => releaseAssetUrl('0.1.0', 'Panda.app.tar.gz', { source }), /Unregistered/);
  }
  assert.throws(() => updateEndpoints([]));
  assert.throws(() => { DOWNLOAD_SOURCES.updates.sources.oss = {}; }, TypeError);
  assert.deepEqual(routeUpdateEndpoints('cn'), [domestic, global]);
  assert.deepEqual(routeUpdateEndpoints('global'), [global, domestic]);
  assert.deepEqual(routeUpdateEndpoints('cn', 'global'), [global]);
  assert.deepEqual(routeUpdateEndpoints('global', 'cn'), [domestic]);
  assert.throws(() => routeUpdateEndpoints('other'));
});

test('A compiled OSS registration controls both manifest and exact versioned artifact paths', async t => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'disposable-panda-download-sources-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await mkdir(path.join(root, 'scripts'));
  await mkdir(path.join(root, 'distribution'));
  for (const name of ['download-sources.mjs', 'update-manifest-protocol.mjs']) {
    await cp(fileURLToPath(new URL(name, import.meta.url)), path.join(root, 'scripts', name));
  }
  const configuration = JSON.parse(await readFile(new URL('../distribution/download-sources.json', import.meta.url), 'utf8'));
  configuration.updates.sources.oss = {
    manifestUrl: 'https://disposable-oss.invalid/panda/latest.json',
    artifactBaseUrl: 'https://disposable-oss.invalid/panda/releases',
  };
  await writeFile(path.join(root, 'distribution/download-sources.json'), JSON.stringify(configuration));
  const { releaseAssetUrl: futureAssetUrl } = await import(pathToFileURL(path.join(root, 'scripts/update-manifest-protocol.mjs')).href);
  const { updateEndpoints: futureEndpoints } = await import(pathToFileURL(path.join(root, 'scripts/download-sources.mjs')).href);
  assert.equal(futureAssetUrl('0.1.0', 'Panda.app.tar.gz', { source: 'oss' }), 'https://disposable-oss.invalid/panda/releases/v0.1.0/Panda.app.tar.gz');
  assert.deepEqual(futureEndpoints(['oss', 'github']), [configuration.updates.sources.oss.manifestUrl, ...updateEndpoints()]);
});
