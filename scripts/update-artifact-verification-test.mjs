import assert from 'node:assert/strict';
import { after, test } from 'node:test';
import { constants } from 'node:fs';
import { createHash, generateKeyPairSync, randomBytes, sign } from 'node:crypto';
import promises, { mkdtemp, readFile, realpath, rm, writeFile, symlink, link, open, rename } from 'node:fs/promises';
import { syncBuiltinESMExports } from 'node:module';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { verifyUpdateArtifact, ARTIFACT_VERIFICATION_BUDGET } from './update-artifact-verification.mjs';
import { releaseAssetUrl } from './update-manifest-protocol.mjs';
import { parseManifestArguments, runManifestCommand } from './update-manifest.mjs';

// Test-only keys are generated in memory or in these private temporary folders.
// No production key, account, executable artifact or credential is accessed.
const root = await mkdtemp(path.join(await realpath(os.tmpdir()), 'disposable-panda-update-verify-'));
after(async () => rm(root, { recursive: true, force: true }));
const name = '人工验证.app.tar.gz';
const version = '0.1.1';
const pair = generateKeyPairSync('ed25519');
const id = randomBytes(8);
function publicBox(keyPair = pair, keyId = id, tag = 'Ed') {
  const raw = keyPair.publicKey.export({ format: 'der', type: 'spki' }).subarray(-32);
  const packet = Buffer.concat([Buffer.from(tag), keyId, raw]);
  return Buffer.from(`untrusted comment: disposable test public key\n${packet.toString('base64')}\n`).toString('base64');
}
function signature(data, { mode = 'ED', comment = `timestamp:1792886400\tfile:${name}\tversion:${version}`, keyPair = pair, keyId = id } = {}) {
  const signed = sign(null, mode === 'ED' ? createHash('blake2b512').update(data).digest() : data, keyPair.privateKey);
  const packet = Buffer.concat([Buffer.from(mode), keyId, signed]);
  const global = sign(null, Buffer.concat([signed, Buffer.from(comment)]), keyPair.privateKey);
  return Buffer.from(`untrusted comment: disposable test signature\n${packet.toString('base64')}\ntrusted comment: ${comment}\n${global.toString('base64')}\n`).toString('base64');
}
function manifest(sig, release = version) {
  return { version: release, notes: '人工验签样本', pub_date: '2026-10-25T00:00:00Z', platforms: { 'darwin-aarch64': { url: releaseAssetUrl(release, name), signature: sig } } };
}
async function fixture(data = Buffer.from('disposable actual artifact bytes'), options = {}) {
  const folder = await mkdtemp(path.join(root, 'case-'));
  const artifact = path.join(folder, name);
  const publicKeyPath = path.join(folder, 'operator.pub');
  const input = path.join(folder, 'latest.json');
  const value = manifest(signature(data, options));
  await writeFile(artifact, data); await writeFile(publicKeyPath, publicBox()); await writeFile(input, JSON.stringify(value));
  return { folder, artifact, publicKeyPath, input, manifest: value };
}

async function fileSymlinkFixture(t, target, destination) {
  try { await symlink(target, destination); return true; } catch (error) {
    if (process.platform !== 'win32' || !['EPERM', 'EACCES', 'ENOTSUP'].includes(error.code)) throw error;
    t.diagnostic(`Windows file symlink fixture unavailable (${error.code}); this case provides no file-symlink rejection proof.`);
    t.skip('Windows host does not permit this disposable file symlink fixture');
    return false;
  }
}
function editSignature(value, edit) {
  const lines = Buffer.from(value, 'base64').toString().replace(/\n$/, '').split('\n');
  edit(lines);
  return Buffer.from(`${lines.join('\n')}\n`).toString('base64');
}

test('Actual ED signatures authenticate artifact bytes and the complete version/file comment', async () => {
  const mode = 'ED';
  const f = await fixture(Buffer.from(`actual ${mode} bytes`), { mode });
  const result = await verifyUpdateArtifact({ ...f, currentVersion: '0.1.0' });
  assert.equal(result.cryptographicSignatureVerified, true); assert.equal(result.artifactBytesVerified, true);
  assert.equal(result.signatureAlgorithm, mode); assert.equal(result.signedFilename, name); assert.equal(result.version, version);
  assert.equal(result.artifact.sha256, createHash('sha256').update(`actual ${mode} bytes`).digest('hex'));
  assert.equal(result.productionTrustBound, false); assert.equal(result.packageIdentityVerified, false); assert.equal(result.installAuthorized, false);
  assert.equal(result.assessment.state, 'newer'); assert.equal(result.assessment.installAuthorized, false);
});

test('Legacy Ed signatures are explicitly refused without a format fallback', async () => {
  const f = await fixture(Buffer.from('legacy artificial bytes'), { mode: 'Ed' });
  await assert.rejects(verifyUpdateArtifact(f), /Legacy Ed.*not accepted/);
});

test('An ED signature relabelled Ed cannot authenticate its 64-byte digest as a substituted artifact', async () => {
  const data = Buffer.from('original artificial compressed artifact bytes');
  const f = await fixture(data);
  f.manifest.platforms['darwin-aarch64'].signature = editSignature(f.manifest.platforms['darwin-aarch64'].signature, lines => {
    const packet = Buffer.from(lines[1], 'base64'); packet[1] = 0x64; lines[1] = packet.toString('base64');
  });
  await writeFile(f.artifact, createHash('blake2b512').update(data).digest());
  await assert.rejects(verifyUpdateArtifact(f), /Legacy Ed.*not accepted/);
});

test('Byte corruption, wrong key, key ID mismatch and either signature corruption fail', async () => {
  const f = await fixture();
  await writeFile(f.artifact, 'corrupt artifact');
  await assert.rejects(verifyUpdateArtifact(f), /byte signature/);
  const good = await fixture();
  await writeFile(good.publicKeyPath, publicBox(generateKeyPairSync('ed25519')));
  await assert.rejects(verifyUpdateArtifact(good), /trusted-comment signature/);
  await writeFile(good.publicKeyPath, publicBox(pair, randomBytes(8)));
  await assert.rejects(verifyUpdateArtifact(good), /key ID/);
  for (const line of [1, 3]) {
    const value = structuredClone(f.manifest);
    value.platforms['darwin-aarch64'].signature = editSignature(value.platforms['darwin-aarch64'].signature, lines => {
      const bytes = Buffer.from(lines[line], 'base64'); bytes[bytes.length - 1] ^= 1; lines[line] = bytes.toString('base64');
    });
    await assert.rejects(verifyUpdateArtifact({ ...f, manifest: value }), /signature/);
  }
});

test('A tampered full trusted comment cannot be authenticated even when its version/file text still matches', async () => {
  const f = await fixture();
  const sig = f.manifest.platforms['darwin-aarch64'].signature;
  f.manifest.platforms['darwin-aarch64'].signature = editSignature(sig, lines => { lines[2] = lines[2].replace('1792886400', '1792886401'); });
  await assert.rejects(verifyUpdateArtifact(f), /trusted-comment signature/);
});

test('Signed-version requirement, filename binding and downgrade assessment remain strict', async () => {
  const data = Buffer.from('versioned bytes');
  const f = await fixture(data);
  for (const comment of [`timestamp:1\tfile:${name}`, `timestamp:1\tfile:${name}\tversion:9.9.9`, `timestamp:1\tfile:other.app.tar.gz\tversion:${version}`]) {
    await assert.rejects(verifyUpdateArtifact({ ...f, manifest: manifest(signature(data, { comment })) }), /signature|signed version|comment/);
  }
  const other = path.join(f.folder, 'other.app.tar.gz'); await writeFile(other, data);
  await assert.rejects(verifyUpdateArtifact({ ...f, artifact: other }), /basename/);
  for (const [currentVersion, state] of [[version, 'same'], ['9.0.0', 'older']]) {
    const result = await verifyUpdateArtifact({ ...f, currentVersion });
    assert.equal(result.assessment.state, state); assert.equal(result.installAuthorized, false);
  }
});

test('Public-key decoding rejects noncanonical base64, wrong packet length and high-bit algorithm aliases', async () => {
  const f = await fixture();
  for (const value of [publicBox() + '\n\n', 'not base64', Buffer.from('untrusted comment: test\nAAAA\n').toString('base64'), publicBox(pair, id, 'Xx')]) {
    await writeFile(f.publicKeyPath, value); await assert.rejects(verifyUpdateArtifact(f));
  }
  const inner = Buffer.from(publicBox(), 'base64').toString().split('\n');
  const packet = Buffer.from(inner[1], 'base64'); packet[0] |= 0x80; packet[1] |= 0x80; inner[1] = packet.toString('base64');
  await writeFile(f.publicKeyPath, Buffer.from(inner.join('\n')).toString('base64'));
  await assert.rejects(verifyUpdateArtifact(f), /algorithm/);
  for (const tag of ['Ed', 'ED']) {
    await writeFile(f.publicKeyPath, publicBox(pair, id, tag));
    assert.equal((await verifyUpdateArtifact(f)).cryptographicSignatureVerified, true);
  }
});

test('Symlink and hardlink artifacts or public keys are rejected without reading them', async t => {
  for (const field of ['artifact', 'publicKeyPath']) for (const kind of ['symlink', 'hardlink']) {
    await t.test(`${field}: ${kind}`, async t => {
      const f = await fixture(); const original = f[field]; const moved = original + '.original';
      const bytes = await readFile(original); await rm(original); await writeFile(moved, bytes);
      if (kind === 'symlink') {
        if (!await fileSymlinkFixture(t, moved, original)) return;
      } else await link(moved, original);
      await assert.rejects(verifyUpdateArtifact(f), /ordinary file/);
    });
  }
});

test('Oversized artifacts and public keys are rejected before verification', async () => {
  const large = await fixture(Buffer.from('small'));
  const file = await open(large.artifact, 'r+'); await file.truncate(1024 * 1024 * 1024 + 1); await file.close();
  await assert.rejects(verifyUpdateArtifact(large), /bounded/);
  const f = await fixture(); await writeFile(f.publicKeyPath, Buffer.alloc(ARTIFACT_VERIFICATION_BUDGET.publicKeyBytes + 1));
  await assert.rejects(verifyUpdateArtifact(f), /bounded/);
});

test('Relative and traversing inputs never become verification sources', async () => {
  const f = await fixture();
  for (const artifact of [name, `${f.folder}/../${path.basename(f.folder)}/${name}`]) await assert.rejects(verifyUpdateArtifact({ ...f, artifact }));
  for (const publicKeyPath of ['operator.pub', `${f.folder}/../${path.basename(f.folder)}/operator.pub`]) await assert.rejects(verifyUpdateArtifact({ ...f, publicKeyPath }));
});

test('Directory-link parents never become verification sources', async () => {
  const f = await fixture();
  const alias = path.join(root, 'parent-alias');
  if (process.platform === 'win32') await symlink(f.folder, alias, 'junction');
  else await symlink(f.folder, alias);
  await assert.rejects(verifyUpdateArtifact({ ...f, artifact: path.join(alias, name) }));
  await assert.rejects(verifyUpdateArtifact({ ...f, publicKeyPath: path.join(alias, 'operator.pub') }));
});

test('An in-flight artifact overwrite never verifies a different byte sequence', async () => {
  const f = await fixture(Buffer.alloc(8 * 1024 * 1024, 0x41));
  const operation = verifyUpdateArtifact(f);
  const rejected = assert.rejects(operation, /changed|signature/);
  await new Promise(resolve => setImmediate(resolve));
  const changed = await open(f.artifact, constants.O_WRONLY);
  await changed.write(Buffer.from('B'), 0, 1, 4 * 1024 * 1024); await changed.close();
  await rejected;
});

async function manifestInputSwap(t, phase) {
  const actualOpen = promises.open;
  const f = await fixture(); let swapped = false;
  const original = f.input + '.preserved';
  const replace = async () => {
    await rename(f.input, original);
    if (phase === 'fifo-before-open') {
      const result = spawnSync('/usr/bin/mkfifo', [f.input], { encoding: 'utf8' });
      assert.equal(result.status, 0, 'Disposable FIFO creation failed.');
    } else await writeFile(f.input, JSON.stringify(f.manifest));
  };
  t.mock.method(promises, 'open', async (target, flags, ...rest) => {
    if (target !== f.input || swapped) return actualOpen(target, flags, ...rest);
    swapped = true;
    if (phase === 'replacement-after-open') {
      const handle = await actualOpen(target, flags, ...rest); await replace(); return handle;
    }
    // Fail safely instead of hanging the regression if NONBLOCK disappears.
    if (process.platform !== 'win32') assert.notEqual(flags & constants.O_NONBLOCK, 0, 'Manifest open must remain bounded when its named file becomes a FIFO.');
    await replace();
    return actualOpen(target, flags, ...rest);
  });
  syncBuiltinESMExports();
  try {
    await assert.rejects(runManifestCommand({ command: 'verify', input: f.input, artifact: f.artifact, publicKeyPath: f.publicKeyPath }), /Manifest input (?:is invalid|changed)/);
    assert.equal(swapped, true);
    assert.deepEqual(JSON.parse(await readFile(original, 'utf8')), f.manifest);
  } finally { t.mock.restoreAll(); syncBuiltinESMExports(); }
}

test('Manifest input rejects a deterministic Unix FIFO around its actual open', {
  skip: process.platform === 'win32' && 'Unix FIFO/nonblocking-open fixture; real named-file replacement checks run on Windows',
}, async t => {
  await manifestInputSwap(t, 'fifo-before-open');
});

test('Manifest input rejects named-file swaps around its actual open', async t => {
  for (const phase of ['replacement-before-open', 'replacement-after-open']) {
    await t.test(phase, async t => manifestInputSwap(t, phase));
  }
});

test('The verify CLI composes existing strict manifest parsing with real cryptographic verification', async () => {
  const f = await fixture();
  const parsed = parseManifestArguments(['verify', '--input', f.input, '--artifact', f.artifact, '--public-key', f.publicKeyPath, '--current-version', '0.1.0']);
  const result = await runManifestCommand(parsed); assert.equal(result.cryptographicSignatureVerified, true); assert.equal(result.productionTrustBound, false);
  const cli = fileURLToPath(new URL('./update-manifest.mjs', import.meta.url));
  const args = [cli, 'verify', '--input', f.input, '--artifact', f.artifact, '--public-key', f.publicKeyPath];
  const accepted = spawnSync(process.execPath, args, { encoding: 'utf8' });
  assert.equal(accepted.status, 0, accepted.stderr);
  const actual = JSON.parse(accepted.stdout);
  assert.equal(actual.cryptographicSignatureVerified, true); assert.equal(actual.installAuthorized, false); assert.equal(actual.productionTrustBound, false);
  await writeFile(f.artifact, 'corruption');
  const refused = spawnSync(process.execPath, args, { encoding: 'utf8' });
  assert.equal(refused.status, 1); assert.equal(refused.stdout, ''); assert.match(refused.stderr, /signature/);
  await writeFile(f.input, '{"version":"0.1.1","version":"0.1.2"}');
  await assert.rejects(runManifestCommand(parsed), /duplicate/);
});

test('Locked Tauri CLI 2.12.3 signs an artificial temporary artifact that this verifier accepts', async () => {
  const f = await fixture();
  const cli = fileURLToPath(new URL('../node_modules/@tauri-apps/cli/tauri.js', import.meta.url));
  const packageInfo = JSON.parse(await readFile(new URL('../node_modules/@tauri-apps/cli/package.json', import.meta.url)));
  assert.equal(packageInfo.version, '2.12.3');
  const key = path.join(f.folder, 'disposable-test-key');
  // Capture and discard CLI output: key generation may print private material.
  // Never include stdout/stderr/args or key bytes in assertion messages/logs.
  const invoke = args => {
    const result = spawnSync(process.execPath, [cli, ...args], { env: { PATH: process.env.PATH, CI: 'true' }, encoding: 'utf8', timeout: 30_000, maxBuffer: 1024 * 1024 });
    assert.equal(result.status, 0, 'Artificial Tauri signer operation failed; captured key output is suppressed.');
  };
  invoke(['signer', 'generate', '--ci', '--password', '', '--write-keys', key]);
  invoke(['signer', 'sign', '--password', '', '--private-key-path', key, '--app-version', version, f.artifact]);
  const value = manifest((await readFile(`${f.artifact}.sig`, 'utf8')).trimEnd());
  const result = await verifyUpdateArtifact({ ...f, manifest: value, publicKeyPath: `${key}.pub` });
  assert.equal(result.signatureAlgorithm, 'ED'); assert.equal(result.cryptographicSignatureVerified, true);
  assert.equal(result.installAuthorized, false); assert.equal(result.productionTrustBound, false);
});
