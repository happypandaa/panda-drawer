import assert from 'node:assert/strict';
import { test } from 'node:test';
import path from 'node:path';
import { publishOssTrial } from './publish-oss-update-test.mjs';

const tag = 'auto-update-dual-test-20261119-100000';
const prefix = `_check/${tag}/`;
const archive = 'PandaDrawer_0.1.1_x64-setup.exe';
const names = [archive, archive + '.sig', 'latest.json'];
const assets = names.map(name => ({ name, url: `https://panda-updates.zhimibuhui.com/${prefix}${name}`,
  path: path.resolve('fixture', name) }));
const setup = { tag, assets, assertUnchanged() {} };

function origin(xml = '<VersioningConfiguration/>') {
  const objects = new Map();
  const writes = [];
  async function api(operation, args) {
    if (operation === 'get-bucket-versioning') return xml;
    const key = args[args.indexOf('--key') + 1];
    assert.ok(key.startsWith(prefix));
    const name = key.slice(prefix.length);
    assert.ok(names.includes(name));
    if (operation === 'head-object') return objects.has(name) ? 'present' : null;
    assert.equal(operation, 'put-object');
    assert.ok(args.includes('--forbid-overwrite=true'));
    assert.equal(args[args.indexOf('--body') + 1], 'file://' + name);
    writes.push(name);
    if (!objects.has(name)) objects.set(name, 'verified');
  }
  async function verify({ name }) {
    assert.equal(objects.get(name), 'verified');
  }
  return { objects, writes, api, verify, bucket: 'disposable-trial' };
}

test('OSS test manifest is written after public package verification and resumes without replacing objects', async () => {
  const state = origin();
  await assert.rejects(publishOssTrial(setup, { ...state, verify: async ({ name }) => {
    if (name === names[1]) throw new Error('CDN delayed');
  } }), /CDN delayed/);
  assert.deepEqual(state.writes, names.slice(0, 2));
  await publishOssTrial(setup, state);
  await publishOssTrial(setup, state);
  assert.deepEqual(state.writes, names);
});

test('existing foreign manifest, incomplete published batch and unsafe versioning all stop before a write', async () => {
  const state = origin();
  state.objects.set('latest.json', 'older macOS trial');
  await assert.rejects(publishOssTrial(setup, state), assert.AssertionError);
  state.objects.set('latest.json', 'verified');
  await assert.rejects(publishOssTrial(setup, state), /has missing assets/);
  assert.deepEqual(state.writes, []);
  for (const xml of ['<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>',
    '<VersioningConfiguration><Status>Suspended</Status></VersioningConfiguration>', 'AccessDenied']) {
    const unsafeBucket = origin(xml);
    await assert.rejects(publishOssTrial(setup, unsafeBucket), /versioning/);
    assert.deepEqual(unsafeBucket.writes, []);
  }
});
