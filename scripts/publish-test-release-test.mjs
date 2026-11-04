import assert from 'node:assert/strict';
import test from 'node:test';
import { chmod, mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { publishTestRelease, verifyPublicDownload } from './publish-test-release.mjs';

test('Test publication creates matching prereleases on both hosts, verifies anonymous bytes and resumes without duplicates', async t => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'disposable-panda-test-release-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const assetsDirectory = path.join(root, 'assets');
  await mkdir(assetsDirectory);
  const payload = Buffer.from('disposable public download probe');
  await writeFile(path.join(assetsDirectory, 'probe.txt'), payload);
  const credentialsPath = path.join(root, 'credentials.json');
  await writeFile(credentialsPath, JSON.stringify({ github: 'disposable-github-token', gitee: 'disposable-gitee-token' }), { mode: 0o600 });
  const options = { tag: 'download-routes-test-20261104-1', commit: 'a'.repeat(40), assetsDirectory, credentialsPath };
  const releases = new Map();
  const uploads = new Map();
  const mutations = [];
  const fetcher = async (url, request = {}) => {
    const address = new URL(url);
    const github = address.hostname.includes('github.com');
    const host = github ? 'github' : 'gitee';
    const method = request.method ?? 'GET';
    const json = value => Response.json(value);
    if (address.pathname.includes('/releases/download/')) {
      assert.equal(request.headers, undefined, 'Download verification must be anonymous.');
      return new Response(payload);
    }
    if (method === 'GET' && address.pathname.includes('/releases/tags/')) {
      return releases.has(host) ? json(releases.get(host)) : new Response('', { status: 404 });
    }
    if (method === 'POST' && address.pathname.endsWith('/releases')) {
      mutations.push(host + ':release');
      const data = github ? JSON.parse(request.body) : Object.fromEntries(request.body);
      assert.equal(data.tag_name, options.tag);
      assert.equal(data.target_commitish, options.commit);
      if (github) assert.equal(data.make_latest, 'false');
      else assert.equal(data.access_token, 'disposable-gitee-token');
      const release = { id: 123, tag_name: options.tag, prerelease: true, assets: [] };
      releases.set(host, release);
      return json(release);
    }
    if (method === 'GET' && address.pathname.endsWith('/attach_files')) return json(uploads.get(host) ?? []);
    if (method === 'POST' && address.pathname.endsWith('/assets')) {
      mutations.push(host + ':asset');
      assert.equal(request.headers.Authorization, 'Bearer disposable-github-token');
      assert.deepEqual(request.body, payload);
      releases.get(host).assets.push({ name: 'probe.txt' });
      return json({ id: 7, name: 'probe.txt' });
    }
    if (method === 'POST' && address.pathname.endsWith('/attach_files')) {
      mutations.push(host + ':asset');
      assert.equal(request.body.get('access_token'), 'disposable-gitee-token');
      assert.deepEqual(Buffer.from(await request.body.get('file').arrayBuffer()), payload);
      uploads.set(host, [{ name: 'probe.txt' }]);
      return json({ id: 8, name: 'probe.txt' });
    }
    throw new Error('Unexpected fixture API request');
  };
  const result = await publishTestRelease(options, { fetcher });
  assert.deepEqual(result.releases.map(release => release.source), ['github', 'gitee']);
  assert.ok(result.releases.every(release => release.assets[0].verified));
  assert.deepEqual(mutations, ['github:release', 'github:asset', 'gitee:release', 'gitee:asset']);
  await publishTestRelease(options, { fetcher });
  assert.equal(mutations.length, 4);
  await assert.rejects(publishTestRelease({ ...options, tag: 'v0.1.0' }, { fetcher }), /explicit/);
  if (process.platform !== 'win32') {
    await chmod(credentialsPath, 0o644);
    await assert.rejects(publishTestRelease(options, { fetcher }), /private ordinary file/);
  }
});

test('Public download verification rejects changed bytes and non-HTTPS redirects', async () => {
  await assert.rejects(verifyPublicDownload('https://example.invalid/probe', 3, '0'.repeat(64), async () => new Response('bad')), /SHA-256/);
  await assert.rejects(verifyPublicDownload('https://example.invalid/probe', 3, '0'.repeat(64),
    async () => new Response('', { status: 302, headers: { location: 'http://example.invalid/probe' } })), /HTTPS/);
});
