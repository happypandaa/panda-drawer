import assert from 'node:assert/strict';
import test from 'node:test';
import { lstat, mkdtemp, readFile, realpath, rm, symlink, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { UPDATE_POLICY, assessStableUpdate, automaticCheckPolicy, compareVersions, generateStaticManifest, parseProtocolJson, parseVersion, releaseAssetUrl, validateSignatureFormat, validateStaticManifest } from './update-manifest-protocol.mjs';
import { parseManifestArguments, runManifestCommand } from './update-manifest.mjs';
import { hasTraversal } from './native-path.mjs';

test('Native traversal checks keep Mac backslash names and reject Windows parent components with either separator', () => {
  for (const value of ['C:\\workspace\\..\\file.json', 'C:/workspace/../file.json', 'C:\\workspace/../file.json', 'C:/workspace\\..\\file.json']) {
    assert.equal(hasTraversal(value, { platform: 'win32' }), true);
  }
  for (const value of ['C:\\工作区\\.env', 'C:/workspace/..notes', 'C:\\workspace\\.\\file.json', 'C:/workspace/./file.json', 'C:\\workspace/.../file.json']) {
    assert.equal(hasTraversal(value, { platform: 'win32' }), false);
  }
  for (const value of ['/workspace/../file.json', '../file.json']) assert.equal(hasTraversal(value, { platform: 'darwin' }), true);
  for (const value of ['/workspace/..\\file.json', '/workspace\\..\\file.json', '/workspace/./file.json', '/workspace/.env']) {
    assert.equal(hasTraversal(value, { platform: 'darwin' }), false);
  }
});

test('verify arguments require explicit artifact and public-key inputs without adding identity or source overrides', () => {
  const args = ['verify', '--input', '/tmp/latest.json', '--artifact', '/tmp/app.app.tar.gz', '--public-key', '/tmp/operator.pub'];
  assert.deepEqual(parseManifestArguments(args), { command: 'verify', input: '/tmp/latest.json', artifact: '/tmp/app.app.tar.gz', publicKeyPath: '/tmp/operator.pub' });
  for (const invalid of [args.slice(0, -2), [...args, '--public-key', '/tmp/other.pub'], [...args, '--url', 'https://example.invalid'], [...args, '--identifier', 'other'], [...args, '--output', '/tmp/result.json']]) assert.throws(() => parseManifestArguments(invalid));
});

// Artificial format-only bytes, never a valid signature, executable or key.
// No keys are generated or loaded by any test.
function signature(version = '0.1.0', name = '熊猫小抽屉.app.tar.gz') {
  const packet = Buffer.alloc(74, 1); packet.write('ED', 0, 'ascii');
  return Buffer.from(`untrusted comment: artificial unsigned test fixture\n${packet.toString('base64')}\ntrusted comment: timestamp:1792886400\tfile:${name}\tversion:${version}\n${Buffer.alloc(64, 2).toString('base64')}\n`).toString('base64');
}
function metadata(overrides = {}) {
  return { schemaVersion: 1, profile: 'production', identifier: UPDATE_POLICY.identifier, target: UPDATE_POLICY.target,
    releaseRepository: UPDATE_POLICY.repository, version: '0.1.0', tag: 'v0.1.0', notes: '人工测试说明\n没有真实发行文件。', pub_date: '2026-10-25T00:00:00Z',
    artifact: { name: '熊猫小抽屉.app.tar.gz', bytes: 123, sha256: 'a'.repeat(64), signature: signature() }, ...overrides };
}
function manifest(version = '0.1.0') {
  const name = '熊猫小抽屉.app.tar.gz';
  return { version, notes: 'Artificial fixture only.', pub_date: '2026-10-25T08:00:00+08:00',
    platforms: { [UPDATE_POLICY.target]: { url: releaseAssetUrl(version, name), signature: signature(version.replace(/^v/, ''), name) } } };
}
async function disposable(t) {
  const root = await mkdtemp(path.join(await realpath(os.tmpdir()), 'disposable-panda-update-manifest-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  return root;
}

async function fileSymlinkFixture(t, target, destination) {
  try { await symlink(target, destination); return true; } catch (error) {
    if (process.platform !== 'win32' || !['EPERM', 'EACCES', 'ENOTSUP'].includes(error.code)) throw error;
    t.diagnostic(`Windows file symlink fixture unavailable (${error.code}); this case provides no file-symlink rejection proof.`);
    t.skip('Windows host does not permit this disposable file symlink fixture');
    return false;
  }
}

test('generation uses only the official static fields and the fixed tagged repository URL', () => {
  const value = generateStaticManifest(metadata());
  assert.deepEqual(Object.keys(value), ['version', 'notes', 'pub_date', 'platforms']);
  assert.deepEqual(Object.keys(value.platforms), ['darwin-aarch64']);
  assert.equal(value.platforms['darwin-aarch64'].url, `${UPDATE_POLICY.repository}/releases/download/v0.1.0/${encodeURIComponent('熊猫小抽屉.app.tar.gz')}`);
  assert.equal(value.platforms['darwin-aarch64'].signature, metadata().artifact.signature);
  assert.equal('bytes' in value.platforms['darwin-aarch64'], false);
});

test('SemVer comparison handles multi-digit numbers, metadata and the full prerelease ordering', () => {
  const order = ['1.0.0-alpha', '1.0.0-alpha.1', '1.0.0-alpha.beta', '1.0.0-beta', '1.0.0-beta.2', '1.0.0-beta.11', '1.0.0-rc.1', '1.0.0'];
  for (let i = 1; i < order.length; i++) assert.equal(compareVersions(order[i - 1], order[i]), -1);
  assert.equal(compareVersions('v0.10.0', '0.9.9'), 1);
  assert.equal(compareVersions('1.0.0+first', '1.0.0+second'), 0);
  assert.equal(compareVersions('18446744073709551615.0.0', '9007199254740992.0.0'), 1);
  for (const value of ['01.2.3', '1.2', '1.2.3.4', '1.0.0-01', '1.0.0-a..b', '1.0.0+', ' 1.0.0', 'vv1.0.0', '18446744073709551616.0.0']) assert.throws(() => parseVersion(value));
});

test('stable assessment refuses downgrade, equal precedence and prerelease offers without granting installation', () => {
  for (const [version, current, state] of [['0.2.0', '0.1.0', 'newer'], ['0.1.0', '0.2.0', 'older'], ['0.1.0+build', '0.1.0+other', 'same'], ['0.2.0-rc.1', '0.1.0', 'prerelease-skipped'], ['0.2.0', '0.2.0-rc.1', 'newer']]) {
    const result = assessStableUpdate(manifest(version), current);
    assert.equal(result.state, state); assert.equal(result.installAuthorized, false); assert.equal(result.cryptographicSignatureVerified, false);
  }
  assert.equal(validateStaticManifest(manifest('v0.1.0')).version, '0.1.0');
});

test('generator rejects prereleases, build aliases and equal or older release publication', () => {
  for (const version of ['v0.1.0', '0.1.0+build', '0.2.0-beta.1', '0.0.9']) assert.throws(() => generateStaticManifest(metadata({ version })));
  for (const previousVersion of ['0.1.0', '0.1.0+other', '0.2.0']) assert.throws(() => generateStaticManifest(metadata(), { previousVersion }), /newer/);
  assert.equal(generateStaticManifest(metadata(), { previousVersion: '0.1.0-rc.1' }).version, '0.1.0');
});

test('identity, profile, platform, repository, tag and path overrides are rejected', () => {
  for (const overrides of [{ identifier: 'dev.happypandaa.pandadrawer' }, { profile: 'development' }, { target: 'darwin-x86_64' }, { releaseRepository: 'https://github.com/someone/else' }, { tag: 'latest' }, { artifactPath: '/private/file' }, { schemaVersion: 2 }]) assert.throws(() => generateStaticManifest(metadata(overrides)));
  for (const name of ['../bad.app.tar.gz', '/bad.app.tar.gz', 'bad\\app.app.tar.gz', 'bad.dmg', 'bad.app.tar.gz.sig', 'bad%2f.app.tar.gz', 'bad?token.app.tar.gz', 'bad#frag.app.tar.gz', 'x'.repeat(256) + '.app.tar.gz']) {
    assert.throws(() => generateStaticManifest(metadata({ artifact: { ...metadata().artifact, name } })));
  }
});

test('manifest rejects unsupported platforms and incomplete or unknown official fields', () => {
  for (const modify of [m => { m.identifier = UPDATE_POLICY.identifier; }, m => { m.platforms['darwin-x86_64'] = m.platforms[UPDATE_POLICY.target]; }, m => { delete m.platforms; }, m => { m.platforms[UPDATE_POLICY.target].size = 123; }, m => { delete m.platforms[UPDATE_POLICY.target].signature; }]) {
    const m = manifest(); modify(m); assert.throws(() => validateStaticManifest(m));
  }
});

test('One release generates a complete Mac and Windows manifest without changing either signed artifact', () => {
  const name = 'Panda_0.1.0_x64-setup.exe';
  const windows = metadata({ target: 'windows-x86_64', artifact: { ...metadata().artifact, name, signature: signature('0.1.0', name) } });
  const combined = generateStaticManifest([metadata(), windows]);
  assert.deepEqual(Object.keys(combined.platforms), ['darwin-aarch64', 'windows-x86_64']);
  assert.equal(combined.platforms['windows-x86_64'].url, `${UPDATE_POLICY.repository}/releases/download/v0.1.0/${name}`);
  assert.equal(combined.platforms['windows-x86_64'].signature, windows.artifact.signature);
  assert.deepEqual(validateStaticManifest(combined), combined);
  for (const changes of [{ notes: 'another release' }, { pub_date: '2026-10-26T00:00:00Z' }, { version: '0.2.0' }]) {
    assert.throws(() => generateStaticManifest([metadata(), { ...windows, ...changes }]));
  }
  assert.throws(() => generateStaticManifest([metadata(), metadata()]), /duplicate/);
  assert.throws(() => generateStaticManifest([]), /nonempty/);
  assert.throws(() => generateStaticManifest(windows, { source: 'oss' }), /Unregistered/);
  assert.throws(() => generateStaticManifest(metadata({ artifact: windows.artifact })), /filename/);
  assert.throws(() => generateStaticManifest({ ...windows, artifact: metadata().artifact }), /filename/);
  assert.throws(() => releaseAssetUrl('0.1.0', 'Panda\n.app.tar.gz'), /filename/);
  assert.throws(() => releaseAssetUrl('0.1.0', 'Panda\n.exe', { target: 'windows-x86_64' }), /filename/);
});

test('Gitee manifests reuse the same signed artifacts while binding their URLs to the registered mirror', () => {
  const github = generateStaticManifest(metadata());
  const gitee = generateStaticManifest(metadata(), { source: 'gitee' });
  const entry = gitee.platforms['darwin-aarch64'];
  assert.equal(entry.signature, github.platforms['darwin-aarch64'].signature);
  assert.equal(entry.url, 'https://gitee.com/mycolorfuldays/panda-drawer/releases/download/v0.1.0/' + encodeURIComponent(metadata().artifact.name));
  assert.deepEqual(validateStaticManifest(gitee), gitee);
  entry.url = entry.url.replace('/mycolorfuldays/', '/another-owner/');
  assert.throws(() => validateStaticManifest(gitee), /registered source/);
});

test('asset URLs reject redirects, rolling tags, query tokens, credentials, traversal and alternate encodings', () => {
  const valid = manifest().platforms[UPDATE_POLICY.target].url;
  for (const url of [valid.replace('https:', 'http:'), valid.replace('github.com', 'github.com.evil.invalid'), valid.replace('github.com', 'user:pass@github.com'), valid.replace('/v0.1.0/', '/latest/'), valid.replace('/v0.1.0/', '/v0.2.0/'), valid.replace('panda-drawer', 'panda-drawer-releases'), valid + '?token=x', valid + '#fragment', valid.replace('github.com/', 'github.com:443/'), valid.replace('/download/', '/download/../download/'), valid.replace('/v0.1.0/', '/v0%2E1%2E0/'), 'file:///tmp/unsigned.app.tar.gz']) {
    const m = manifest(); m.platforms[UPDATE_POLICY.target].url = url; assert.throws(() => validateStaticManifest(m));
  }
});

test('calendar validation accepts explicit offsets and leap days but rejects normalized invalid dates', () => {
  for (const pub_date of ['2024-02-29T01:02:03.123456789Z', '2026-10-25T08:00:00+08:00']) assert.equal(generateStaticManifest(metadata({ pub_date })).pub_date, pub_date);
  for (const pub_date of ['2026-02-29T00:00:00Z', '2024-02-30T00:00:00Z', '2026-10-25', '2026-10-25T24:00:00Z', '2026-10-25T00:00:60Z', '2026-10-25T00:00:00+24:00', '2026-10-25T00:00:00', '2026-10-25T00:00:00Z trailing']) assert.throws(() => generateStaticManifest(metadata({ pub_date })));
});

test('signature validation rejects paths, plain base64, missing version and wrong file/version bindings', () => {
  for (const value of ['', '/tmp/update.sig', 'https://github.com/update.sig', Buffer.from('not minisign').toString('base64'), signature('0.0.9'), signature('0.1.0', 'other.app.tar.gz'), signature() + '\n']) assert.throws(() => validateSignatureFormat(value, { version: '0.1.0', name: metadata().artifact.name }));
  const text = Buffer.from(signature(), 'base64').toString();
  for (const changed of [text.replace('\tversion:0.1.0', ''), text.replace('\tversion:0.1.0', '\tversion:0.1.0\tversion:0.2.0'), text + '\nextra']) {
    assert.throws(() => validateSignatureFormat(Buffer.from(changed).toString('base64'), { version: '0.1.0', name: metadata().artifact.name }));
  }
  const lines = text.split('\n'); const packet = Buffer.from(lines[1], 'base64'); packet.write('XX', 0, 'ascii'); lines[1] = packet.toString('base64');
  assert.throws(() => validateSignatureFormat(Buffer.from(lines.join('\n')).toString('base64'), { version: '0.1.0', name: metadata().artifact.name }));
  // Structural acceptance deliberately gives no cryptographic proof.
  assert.equal(validateSignatureFormat(signature(), { version: '0.1.0', name: metadata().artifact.name }), signature());
});

test('minisign algorithm bytes reject high-bit ASCII aliases rather than masking their top bit', () => {
  for (const bytes of [[0xc5, 0xc4], [0xc5, 0xe4], [0x45, 0xc4], [0x45, 0xe4], [0xc5, 0x44], [0xc5, 0x64]]) {
    const lines = Buffer.from(signature(), 'base64').toString().split('\n');
    const packet = Buffer.from(lines[1], 'base64'); packet[0] = bytes[0]; packet[1] = bytes[1]; lines[1] = packet.toString('base64');
    assert.throws(() => validateSignatureFormat(Buffer.from(lines.join('\n')).toString('base64'), { version: '0.1.0', name: metadata().artifact.name }), /algorithm/);
  }
  for (const byte of [0x44, 0x64]) {
    const lines = Buffer.from(signature(), 'base64').toString().split('\n');
    const packet = Buffer.from(lines[1], 'base64'); packet[1] = byte; lines[1] = packet.toString('base64');
    assert.doesNotThrow(() => validateSignatureFormat(Buffer.from(lines.join('\n')).toString('base64'), { version: '0.1.0', name: metadata().artifact.name }));
  }
});

test('control characters at scalar boundaries never become versions, dates, filenames, hashes or signatures', () => {
  for (const control of ['\n', '\r', '\r\n', '\0', '\t', '\u007f']) {
    for (const wrap of [value => control + value, value => value + control]) {
      assert.throws(() => parseVersion(wrap('0.1.0')));
      assert.throws(() => assessStableUpdate(manifest(), wrap('0.1.0')));
      assert.throws(() => releaseAssetUrl('0.1.0', wrap('Panda.app.tar.gz')));
      assert.throws(() => generateStaticManifest(metadata({ pub_date: wrap('2026-10-25T00:00:00Z') })));
      assert.throws(() => generateStaticManifest(metadata({ artifact: { ...metadata().artifact, sha256: wrap('a'.repeat(64)) } })));
      assert.throws(() => validateSignatureFormat(wrap(signature()), { version: '0.1.0', name: metadata().artifact.name }));
    }
  }
});

test('all document, note, signature, declared artifact size and SHA budgets fail closed', () => {
  for (const bytes of [0, -1, 1.5, UPDATE_POLICY.maxArtifactBytes + 1, Number.MAX_SAFE_INTEGER + 1]) assert.throws(() => generateStaticManifest(metadata({ artifact: { ...metadata().artifact, bytes } })));
  for (const sha256 of ['a'.repeat(63), 'A'.repeat(64), 'g'.repeat(64), 'https://example.invalid']) assert.throws(() => generateStaticManifest(metadata({ artifact: { ...metadata().artifact, sha256 } })));
  assert.throws(() => generateStaticManifest(metadata({ notes: '中'.repeat(UPDATE_POLICY.maxNotesBytes) })));
  assert.throws(() => validateSignatureFormat('A'.repeat(UPDATE_POLICY.maxSignatureBytes + 4), { version: '0.1.0', name: metadata().artifact.name }));
  assert.throws(() => parseProtocolJson(Buffer.alloc(UPDATE_POLICY.maxDocumentBytes + 1, 32)));
  assert.equal(generateStaticManifest(metadata({ artifact: { ...metadata().artifact, bytes: UPDATE_POLICY.maxArtifactBytes }, notes: 'x'.repeat(UPDATE_POLICY.maxNotesBytes) })).notes.length, UPDATE_POLICY.maxNotesBytes);
});

test('bounded JSON rejects duplicates, invalid UTF-8, trailing text and deep unknown inputs', () => {
  assert.deepEqual(parseProtocolJson('{"a":1,"nested":[true,null,"x\\\"y"]}'), { a: 1, nested: [true, null, 'x"y'] });
  for (const raw of ['{"version":"0.1.0","version":"0.2.0"}', '{"x":{"version":1,"versio\\u006e":2}}', '{} {}', '{"a":}', '['.repeat(18) + '0' + ']'.repeat(18)]) assert.throws(() => parseProtocolJson(raw));
  assert.throws(() => parseProtocolJson(Buffer.from([0xff, 0xfe])));
});

test('development, unknown and caller-forged production metadata cannot enable automatic checking', () => {
  for (const info of [undefined, {}, { profile: 'development', identifier: 'dev.happypandaa.pandadrawer', releaseReady: true }, { profile: 'production', identifier: UPDATE_POLICY.identifier, releaseReady: true, updaterEnabled: true }]) assert.equal(automaticCheckPolicy(info).enabled, false);
});

test('CLI parsing rejects source/identity overrides, duplicate flags and wrong command options', () => {
  assert.deepEqual(parseManifestArguments(['generate', '--input', '/metadata.json', '--output', '/latest.json']), { command: 'generate', input: '/metadata.json', output: '/latest.json' });
  for (const args of [[], ['generate'], ['validate', '--input', '/m', '--output', '/o'], ['generate', '--input', '/a', '--output', '/b', '--url', 'https://example.invalid'], ['generate', '--input', '/a', '--output', '/b', '--identifier', 'other'], ['validate', '--input', '/a', '--input', '/b']]) assert.throws(() => parseManifestArguments(args));
});

test('actual CLI generates and revalidates a new local manifest without overwriting existing output', async t => {
  const root = await disposable(t); const input = path.join(root, 'metadata.json'); const output = path.join(root, 'latest.json');
  await writeFile(input, JSON.stringify(metadata()));
  const cli = fileURLToPath(new URL('./update-manifest.mjs', import.meta.url));
  const generated = spawnSync(process.execPath, [cli, 'generate', '--input', input, '--output', output], { encoding: 'utf8' });
  assert.equal(generated.status, 0, generated.stderr);
  const report = JSON.parse(generated.stdout); assert.equal(report.installAuthorized, false); assert.equal(report.artifactBytesVerified, false); assert.equal(report.cryptographicSignatureVerified, false);
  const checked = spawnSync(process.execPath, [cli, 'validate', '--input', output, '--current-version', '0.2.0'], { encoding: 'utf8' });
  assert.equal(checked.status, 0, checked.stderr); assert.equal(JSON.parse(checked.stdout).assessment.state, 'older');
  const before = await readFile(output);
  const refused = spawnSync(process.execPath, [cli, 'generate', '--input', input, '--output', output], { encoding: 'utf8' });
  assert.equal(refused.status, 1); assert.deepEqual(await readFile(output), before);
});

test('CLI refuses linked or oversized input and linked output without changing the original file', async t => {
  const root = await disposable(t); const input = path.join(root, 'metadata.json'); const linked = path.join(root, 'linked.json');
  await writeFile(input, JSON.stringify(metadata()));
  await t.test('file symlink input and output', async t => {
    if (!await fileSymlinkFixture(t, input, linked)) return;
    await assert.rejects(runManifestCommand({ command: 'validate', input: linked }));
    await assert.rejects(runManifestCommand({ command: 'generate', input, output: linked }));
    assert.deepEqual(JSON.parse(await readFile(input)), metadata());
  });
  await t.test('oversized and relative inputs', async () => {
    const oversized = path.join(root, 'oversized.json'); await writeFile(oversized, Buffer.alloc(UPDATE_POLICY.maxDocumentBytes + 1));
    await assert.rejects(runManifestCommand({ command: 'validate', input: oversized }));
    await assert.rejects(runManifestCommand({ command: 'validate', input: 'relative.json' }));
  });
  await t.test('directory-link input and output parents', async () => {
    const alias = path.join(root, 'alias');
    if (process.platform === 'win32') await symlink(root, alias, 'junction');
    else await symlink(root, alias);
    await assert.rejects(runManifestCommand({ command: 'validate', input: path.join(alias, 'metadata.json') }));
    await assert.rejects(runManifestCommand({ command: 'generate', input, output: path.join(alias, 'new.json') }));
    await assert.rejects(lstat(path.join(root, 'new.json')), { code: 'ENOENT' });
  });
});

test('invalid signed metadata fails before creating any requested output', async t => {
  const root = await disposable(t); const input = path.join(root, 'invalid.json'); const output = path.join(root, 'latest.json');
  await writeFile(input, JSON.stringify(metadata({ artifact: { ...metadata().artifact, signature: signature('0.0.9') } })));
  await assert.rejects(runManifestCommand({ command: 'generate', input, output }));
  await assert.rejects(lstat(output), { code: 'ENOENT' });
});
