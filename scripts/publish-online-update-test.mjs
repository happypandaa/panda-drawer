import { createHash } from 'node:crypto';
import { constants } from 'node:fs';
import { lstat, open, readdir, realpath } from 'node:fs/promises';
import { userInfo } from 'node:os';
import path from 'node:path';
import { verifyPublicDownload } from './publish-test-release.mjs';
import { verifyUpdateArtifact } from './update-artifact-verification.mjs';
import { parseProtocolJson, releaseAssetUrl, UPDATE_POLICY } from './update-manifest-protocol.mjs';

const repository = 'happypandaa/panda-drawer';
const apiBase = `https://api.github.com/repos/${repository}`;
const webBase = `https://github.com/${repository}`;
const version = '0.1.1';
const target = 'darwin-aarch64';
const archiveName = `PandaDrawer_${version}_aarch64.app.tar.gz`;
const signatureName = `${archiveName}.sig`;
const manifestName = 'latest.json';
const githubTestTag = /^auto-update-(?:dual-)?test-[0-9]{8}-[0-9]{6}$/;
const assetLimits = new Map([
  [archiveName, UPDATE_POLICY.maxArtifactBytes],
  [signatureName, UPDATE_POLICY.maxSignatureBytes + 1],
  [manifestName, UPDATE_POLICY.maxDocumentBytes],
]);
const fileIdentityFields = ['dev', 'ino', 'mode', 'size', 'nlink', 'uid', 'gid', 'mtimeNs', 'ctimeNs'];

class PublicationError extends Error {}

function requireValue(condition, message) {
  if (!condition) throw new PublicationError(message);
}

function sameFile(a, b) {
  return b.isFile() && fileIdentityFields.every(key => a[key] === b[key]);
}

async function openFile(file, maxBytes, label) {
  const limit = BigInt(maxBytes);
  const named = await lstat(file, { bigint: true }).catch(() => null);
  requireValue(named?.isFile() && named.nlink === 1n && named.size > 0n && named.size <= limit,
    `${label} must be one bounded, nonempty ordinary file.`);
  const handle = await open(file, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK)
    .catch(() => { throw new PublicationError(`Could not open ${label}.`); });
  try {
    requireValue(sameFile(named, await handle.stat({ bigint: true })), `${label} changed while opening.`);
    return { path: file, handle, identity: named, size: Number(named.size), label };
  } catch (error) {
    await handle.close();
    throw error;
  }
}

async function assertUnchanged(file) {
  const current = await file.handle.stat({ bigint: true });
  const named = await lstat(file.path, { bigint: true }).catch(() => null);
  requireValue(named && sameFile(file.identity, current) && sameFile(file.identity, named),
    `${file.label} changed during publication.`);
}

async function readSmallFile(file) {
  const value = Buffer.alloc(file.size);
  let length = 0;
  while (length < value.length) {
    const { bytesRead } = await file.handle.read(value, length, value.length - length, length);
    if (!bytesRead) break;
    length += bytesRead;
  }
  requireValue(length === value.length, `${file.label} changed while reading.`);
  await assertUnchanged(file);
  return value;
}

async function readToken() {
  const user = userInfo();
  requireValue(user.uid >= 0, 'Publication requires a POSIX owner-only release credentials file.');
  const credentials = await openFile(path.join(user.homedir, '.config/panda-drawer/release-credentials.json'),
    16 * 1024, 'Release credentials');
  try {
    requireValue(credentials.identity.uid === BigInt(user.uid) && (credentials.identity.mode & 0o077n) === 0n,
      'Release credentials must be owned by the current user with no group or other access.');
    let config;
    try { config = JSON.parse(await readSmallFile(credentials)); }
    catch { throw new PublicationError('Release credentials could not be read.'); }
    const token = typeof config?.github === 'string' ? config.github.trim() : '';
    requireValue(token && /^[\x21-\x7e]+$/.test(token), 'The release credentials file lacks a valid GitHub token.');
    return token;
  } finally {
    await credentials.handle.close();
  }
}

function assetUrl(tag, name) {
  return `${webBase}/releases/download/${tag}/${name}`;
}

async function prepareAssets(directory, tag, opened) {
  requireValue(path.isAbsolute(directory), 'Use the absolute path of the online kit assets directory.');
  const canonical = await realpath(directory);
  const names = await readdir(canonical);
  requireValue(names.length === assetLimits.size && names.every(name => assetLimits.has(name)),
    'The online kit assets directory must contain exactly the update archive, its signature and latest.json.');

  for (const [name, limit] of assetLimits) {
    const input = await openFile(path.join(canonical, name), limit, name);
    opened.set(name, { ...input, name, url: assetUrl(tag, name) });
  }
  const archive = opened.get(archiveName);
  const signature = opened.get(signatureName);
  const manifest = opened.get(manifestName);
  const manifestBytes = await readSmallFile(manifest);
  const signatureBytes = await readSmallFile(signature);
  let data;
  try { data = parseProtocolJson(manifestBytes); }
  catch { throw new PublicationError('The online kit manifest is invalid.'); }
  const entry = data?.platforms?.[target];
  requireValue(data?.version === version && entry?.url === archive.url
    && typeof entry.signature === 'string' && entry.signature === signatureBytes.toString('utf8').trim(),
    'The online kit manifest must match the pinned test tag, version, archive and detached signature.');

  // The offline verifier expects the stable URL shape. Match the actual test
  // URL above, then adapt its single entry only for offline byte verification.
  const offlineManifest = { ...data, platforms: { ...data.platforms, [target]: {
    ...entry, url: releaseAssetUrl(version, archiveName, { source: 'github', target }),
  } } };
  requireValue(Object.keys(offlineManifest.platforms).length === 1, 'The online test kit must target only Apple Silicon macOS.');
  const kit = path.resolve(canonical, tag.startsWith('auto-update-dual-test-') ? '../..' : '..');
  let verified;
  try {
    verified = await verifyUpdateArtifact({ manifest: offlineManifest, artifact: archive.path,
      publicKeyPath: path.join(kit, 'update-test.pub'), target });
  } catch {
    throw new PublicationError('The update archive or its signed filename and version did not verify with the online kit public key.');
  }
  await assertUnchanged(archive);
  requireValue(verified.artifact.bytes === archive.size, 'The verified update archive size changed.');
  archive.sha256 = verified.artifact.sha256;
  signature.sha256 = createHash('sha256').update(signatureBytes).digest('hex');
  manifest.sha256 = createHash('sha256').update(manifestBytes).digest('hex');
  return [...opened.values()];
}

async function api(url, token, { method = 'GET', data, stream, size } = {}) {
  const headers = {
    Accept: 'application/vnd.github+json',
    'User-Agent': 'panda-drawer-online-update-test',
    'X-GitHub-Api-Version': '2022-11-28',
  };
  if (token) headers.Authorization = `Bearer ${token}`;
  const options = { method, headers, redirect: 'error', signal: AbortSignal.timeout(stream ? 1_200_000 : 30_000) };
  if (stream) {
    headers['Content-Type'] = 'application/octet-stream';
    headers['Content-Length'] = String(size);
    options.body = stream;
    options.duplex = 'half';
  } else if (data) {
    headers['Content-Type'] = 'application/json';
    options.body = JSON.stringify(data);
  }
  let response;
  try { response = await fetch(url, options); }
  catch { throw new PublicationError(`GitHub ${method} request failed before receiving a response.`); }
  if (response.status === 404 && method === 'GET') return null;
  requireValue(response.ok, `GitHub ${method} request returned HTTP ${response.status}.`);
  try { return await response.json(); }
  catch { throw new PublicationError('GitHub returned an invalid release API response.'); }
}

async function assertRemoteTag(token, tag, commit) {
  const reference = await api(`${apiBase}/git/ref/tags/${tag}`, token);
  requireValue(reference?.ref === `refs/tags/${tag}`, 'Push the explicit test tag to GitHub before publishing.');
  let object = reference.object;
  for (let depth = 0; depth < 4 && object?.type === 'tag' && /^[0-9a-f]{40}$/.test(object.sha); depth++) {
    object = (await api(`${apiBase}/git/tags/${object.sha}`, token))?.object;
  }
  requireValue(object?.type === 'commit' && object.sha === commit, 'The remote test tag does not resolve to the supplied commit.');
}

async function findRelease(token, tag) {
  let match;
  // The tag lookup only returns published releases. Authenticated listings
  // also find a draft left behind by an interrupted upload.
  for (let page = 1; ; page++) {
    const releases = await api(`${apiBase}/releases?per_page=100&page=${page}`, token);
    requireValue(Array.isArray(releases), 'GitHub did not return a release list.');
    for (const release of releases) {
      if (release.tag_name !== tag) continue;
      requireValue(!match, 'More than one GitHub release uses this test tag.');
      match = release;
    }
    if (releases.length < 100) return match;
  }
}

function assertTestRelease(release, tag, body) {
  requireValue(release?.tag_name === tag && release.prerelease === true && typeof release.draft === 'boolean'
    && Number.isSafeInteger(release.id) && release.id > 0 && release.body === body,
  'Existing release does not match the test tag, commit and exact asset set.');
}

function assertAsset(actual, expected) {
  requireValue(actual?.name === expected.name && actual.state === 'uploaded'
    && actual.size === expected.size && actual.digest === `sha256:${expected.sha256}`,
  `Existing GitHub asset ${expected.name} does not match the local bytes. Existing assets are preserved.`);
}

async function matchingAssets(token, release, expected) {
  const remote = await api(`${apiBase}/releases/${release.id}/assets?per_page=4`, token);
  requireValue(Array.isArray(remote) && remote.length <= expected.length, 'The test release has unexpected assets.');
  const names = new Set();
  for (const asset of remote) {
    const local = expected.find(item => item.name === asset.name);
    requireValue(local && !names.has(asset.name), 'The test release has unexpected or duplicate assets.');
    assertAsset(asset, local);
    names.add(asset.name);
  }
  return names;
}

async function upload(token, release, asset) {
  await assertUnchanged(asset);
  const stream = asset.handle.createReadStream({ start: 0, end: asset.size - 1, autoClose: false });
  try {
    const url = `https://uploads.github.com/repos/${repository}/releases/${release.id}/assets?name=${encodeURIComponent(asset.name)}`;
    const remote = await api(url, token, { method: 'POST', stream, size: asset.size });
    assertAsset(remote, asset);
    await assertUnchanged(asset);
  } finally {
    stream.destroy();
  }
}

function checkDownloadRedirect(sources) {
  return async (address, options) => {
    const url = new URL(address);
    const allowedAssetHost = ['release-assets.githubusercontent.com', 'objects.githubusercontent.com'].includes(url.hostname)
      && url.pathname.startsWith('/github-production-release-asset');
    requireValue(url.protocol === 'https:' && !url.port && !url.username && !url.password && !url.hash
      && (sources.includes(address) || allowedAssetHost), 'GitHub release redirected outside its trusted asset locations.');
    return fetch(address, options);
  };
}

async function verifyAnonymous(url, size, sha256, versionedUrl = url) {
  try { await verifyPublicDownload(url, size, sha256, checkDownloadRedirect([url, versionedUrl])); }
  catch { throw new PublicationError(`Anonymous GitHub download verification failed for ${new URL(url).pathname}.`); }
}

async function stableRelease() {
  const release = await api(`${apiBase}/releases/latest`);
  if (!release) return null;
  requireValue(!release.draft && !release.prerelease && Number.isSafeInteger(release.id) && release.id > 0
    && typeof release.tag_name === 'string' && Array.isArray(release.assets), 'GitHub returned invalid stable Release metadata.');
  const manifest = release.assets.find(asset => asset.name === manifestName);
  let manifestState = null;
  if (manifest) {
    requireValue(/^[\w.-]+$/.test(release.tag_name) && Number.isSafeInteger(manifest.id)
      && Number.isSafeInteger(manifest.size) && manifest.size > 0 && manifest.size <= UPDATE_POLICY.maxDocumentBytes
      && /^sha256:[0-9a-f]{64}$/.test(manifest.digest), 'The current stable manifest cannot be verified.');
    const sha256 = manifest.digest.slice('sha256:'.length);
    await verifyAnonymous(`${webBase}/releases/latest/download/${manifestName}`, manifest.size, sha256,
      assetUrl(encodeURIComponent(release.tag_name), manifestName));
    manifestState = { id: manifest.id, size: manifest.size, sha256 };
  }
  return { id: release.id, tag: release.tag_name, manifest: manifestState };
}

async function publish(tag, commit, directory) {
  requireValue(typeof tag === 'string' && githubTestTag.exec(tag)?.[0] === tag
    && /^[0-9a-f]{40}$/.exec(commit)?.[0] === commit,
  'Use an auto-update-test or auto-update-dual-test tag with YYYYMMDD-HHMMSS and a full commit SHA.');
  const opened = new Map();
  try {
    const assets = await prepareAssets(directory, tag, opened);
    const token = await readToken();
    await assertRemoteTag(token, tag, commit);
    const before = await stableRelease();
    const body = ['Isolated macOS online update test (0.1.0 to 0.1.1).', `Commit: ${commit}`,
      ...assets.map(asset => `${asset.name}: ${asset.size} bytes, SHA-256 ${asset.sha256}`)].join('\n');
    let release = await findRelease(token, tag);
    if (!release) {
      release = await api(`${apiBase}/releases`, token, { method: 'POST', data: {
        tag_name: tag, target_commitish: commit, name: `Online update test · ${tag}`, body,
        draft: true, prerelease: true, make_latest: 'false',
      } });
    }
    assertTestRelease(release, tag, body);
    const existing = await matchingAssets(token, release, assets);
    if (release.draft) {
      for (const asset of assets) {
        if (!existing.has(asset.name)) await upload(token, release, asset);
      }
      const complete = await matchingAssets(token, release, assets);
      requireValue(complete.size === assets.length, 'The complete test asset set is required before publication.');
      await assertRemoteTag(token, tag, commit);
      release = await api(`${apiBase}/releases/${release.id}`, token, { method: 'PATCH', data: {
        draft: false, prerelease: true, make_latest: 'false',
      } });
      assertTestRelease(release, tag, body);
    } else {
      requireValue(existing.size === assets.length, 'Published test release is missing assets.');
    }
    requireValue(!release.draft, 'GitHub did not publish the test prerelease.');
    for (const asset of assets) await verifyAnonymous(asset.url, asset.size, asset.sha256);
    await assertRemoteTag(token, tag, commit);
    const after = await stableRelease();
    requireValue(JSON.stringify(before) === JSON.stringify(after), 'GitHub stable latest changed during publication.');
    return { tag, commit, release: `${webBase}/releases/tag/${tag}`, stableLatest: after, stableLatestUnchanged: true,
      assets: assets.map(({ name, url, size, sha256 }) => ({ name, url, size, sha256, verified: true })) };
  } finally {
    await Promise.all([...opened.values()].map(asset => asset.handle.close()));
  }
}

try {
  const args = process.argv.slice(2);
  requireValue(args.length === 6 && args[0] === '--tag' && args[2] === '--commit' && args[4] === '--assets',
    'Usage: node scripts/publish-online-update-test.mjs --tag TEST_TAG --commit FULL_SHA --assets /absolute/kit/assets (or /absolute/kit/assets/github for dual tests)');
  console.log(JSON.stringify(await publish(args[1], args[3], args[5]), null, 2));
} catch (error) {
  console.error(error instanceof PublicationError ? error.message : 'Online update test publication failed while reading the kit or handling the release.');
  process.exitCode = 1;
}
