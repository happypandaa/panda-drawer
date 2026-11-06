import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const script = fileURLToPath(new URL('./generate-app-catalog.mjs', import.meta.url));
test('Catalog generation derives requirements, keeps provider addresses and skips applications awaiting adaptation', async t => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'disposable-panda-catalog-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const manifest = path.join(root, 'panda.json');
  await writeFile(manifest, JSON.stringify({ app: { name: 'Disposable application' }, configuration: { fields: [
    { label: 'Required service key', required: true }, { label: 'Optional setting', required: false },
  ] } }));
  const app = { id: 'example', repo: { github: 'https://github.com/disposable/example.git', gitee: 'https://gitee.com/disposable/example.git' }, summary: 'Disposable fixture', manifest };
  const input = path.join(root, 'input.json');
  const output = path.join(root, 'apps.json');
  const run = async apps => {
    await writeFile(input, JSON.stringify({ version: 1, apps }));
    return spawnSync(process.execPath, [script, '--input', input, '--output', output], { encoding: 'utf8' });
  };
  const generated = await run([app, { ...app, id: 'pending', enabled: false, manifest: path.join(root, 'missing.json') }]);
  assert.equal(generated.status, 0, generated.stderr);
  const value = JSON.parse(await readFile(output, 'utf8'));
  assert.equal(value.apps.length, 1);
  assert.deepEqual(value.apps[0].repo, app.repo);
  assert.deepEqual(value.apps[0].needs, ['Required service key']);
  assert.equal(value.apps[0].name, 'Disposable application');
  assert.equal(value.apps[0].agent, undefined);
  for (const apps of [[app, app], [{ ...app, repo: {} }], [{ ...app, repo: { github: app.repo.gitee } }]]) {
    assert.notEqual((await run(apps)).status, 0);
    assert.deepEqual(JSON.parse(await readFile(output, 'utf8')), value);
  }
});

test('Agent recommendations are derived from a valid manifest declaration rather than catalog input', async t => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'disposable-panda-agent-catalog-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const manifest = path.join(root, 'panda.json');
  const input = path.join(root, 'input.json');
  const output = path.join(root, 'apps.json');
  const run = () => spawnSync(process.execPath, [script, '--input', input, '--output', output], { encoding: 'utf8' });
  await writeFile(input, JSON.stringify({ version: 1, apps: [{ id: 'agent-example', repo: 'https://github.com/disposable/agent-example.git', summary: 'Disposable agent fixture', agent: false, manifest }] }));
  await writeFile(manifest, JSON.stringify({ app: { name: 'Disposable agent' }, agent: { instructions: 'AGENTS.md' } }));
  const generated = run();
  assert.equal(generated.status, 0, generated.stderr);
  const value = JSON.parse(await readFile(output, 'utf8'));
  assert.equal(value.apps[0].agent, true);
  for (const agent of [null, {}, { instructions: 'AGENTS.md', schedule: 'daily' }, { instructions: '.rules/AGENTS.md' }, { instructions: '../AGENTS.md' }, { instructions: '/AGENTS.md' }, { instructions: 'rules\\AGENTS.md' }]) {
    await writeFile(manifest, JSON.stringify({ app: { name: 'Disposable agent' }, agent }));
    assert.notEqual(run().status, 0);
    assert.deepEqual(JSON.parse(await readFile(output, 'utf8')), value, 'Invalid declarations must not replace the published output');
  }
});
