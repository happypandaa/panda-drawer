// Prepare and sign local updater artifacts from verified production candidates.
import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { createHash } from 'node:crypto';
import { constants, createReadStream } from 'node:fs';
import { copyFile, link, lstat, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import path from 'node:path';
import { promisify } from 'node:util';
import { root } from './process.mjs';
import { verifyUpdateArtifact } from './update-artifact-verification.mjs';
import { generateStaticManifest, UPDATE_POLICY } from './update-manifest-protocol.mjs';

const execute = promisify(execFile);
const readJson = async file => JSON.parse(await readFile(file, 'utf8'));
const writeJson = (file, data) => writeFile(file, JSON.stringify(data, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
const publicKeyPath = path.join(root, 'build-profiles/production-updater.pub');
const platforms = {
  'aarch64-apple-darwin': { kind: 'macos-production-update-archive', profile: 'production.json', target: 'darwin-aarch64', suffix: '_aarch64.app.tar.gz' },
  'x86_64-pc-windows-msvc': { kind: 'windows-production-candidate', profile: 'production.windows-x64.json', target: 'windows-x86_64', suffix: '_x64-setup.exe' },
};

async function ordinaryFile(file, limit) {
  const info = await lstat(file);
  assert(info.isFile() && info.nlink === 1 && info.size > 0 && info.size <= limit, 'Expected a bounded ordinary file: ' + file);
  return info;
}

async function describeArtifact(file) {
  const { size } = await ordinaryFile(file, UPDATE_POLICY.maxArtifactBytes);
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(file)) hash.update(chunk);
  return { name: path.basename(file), bytes: size, sha256: hash.digest('hex') };
}

async function archive(directory) {
  const profile = await readJson(path.join(root, 'build-profiles/production.json'));
  const candidateTool = path.join(root, 'scripts/production-macos-candidate.mjs');
  const candidate = JSON.parse((await execute(process.execPath, [candidateTool, 'verify-latest'])).stdout);
  assert(candidate.verified && candidate.version === profile.version, 'Expected a verified candidate for the production version.');
  const name = path.basename(candidate.app);
  directory = path.join(await realpath(path.dirname(directory)), path.basename(directory));
  assert(!directory.startsWith(candidate.app + path.sep), 'Choose an output directory outside the signed app.');
  const file = path.join(directory, 'PandaDrawer_' + profile.version + platforms[profile.target].suffix);
  await mkdir(directory, { mode: 0o700 });
  try {
    // Keep the signed bundle at the archive root, with its executable modes and symlinks.
    await execute('/usr/bin/tar', ['--no-mac-metadata', '-czf', file, '-C', path.dirname(candidate.app), name]);
    const extracted = await mkdtemp(path.join(directory, '.verify-'));
    try {
      await execute('/usr/bin/tar', ['-xzf', file, '-C', extracted]);
      const app = path.join(extracted, name);
      await execute('/usr/bin/codesign', ['--verify', '--deep', '--strict', app]);
      await execute('/usr/bin/xcrun', ['stapler', 'validate', app]);
      const cdhash = async bundle => (await execute('/usr/bin/codesign', ['-dvvv', bundle])).stderr.match(/^CDHash=([a-f0-9]+)$/m)?.[1];
      const expected = await cdhash(candidate.app);
      assert(expected && await cdhash(app) === expected, 'Archived app differs from the verified candidate.');
    } finally {
      await rm(extracted, { recursive: true, force: true });
    }
    const receipt = path.join(directory, 'archive-receipt.json');
    await writeJson(receipt, {
      schemaVersion: 1, kind: platforms[profile.target].kind, target: profile.target, identifier: profile.identifier,
      version: profile.version, sourceCommit: candidate.sourceCommit, artifact: await describeArtifact(file),
    });
    return { file, receipt, sourceCommit: candidate.sourceCommit, dmg: candidate.dmg };
  } catch (error) {
    await rm(directory, { recursive: true, force: true });
    throw error;
  }
}

async function signLocally(file, version) {
  const env = { ...process.env };
  assert(!(env.TAURI_SIGNING_PRIVATE_KEY && env.TAURI_SIGNING_PRIVATE_KEY_PATH), 'Select only one Tauri signing key input.');
  if (!env.TAURI_SIGNING_PRIVATE_KEY && !env.TAURI_SIGNING_PRIVATE_KEY_PATH) {
    env.TAURI_SIGNING_PRIVATE_KEY_PATH = path.join(homedir(), '.tauri/panda-drawer.key');
    assert((await lstat(env.TAURI_SIGNING_PRIVATE_KEY_PATH)).isFile(), 'Configure the production signing key locally.');
  }
  if (env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD === undefined) {
    assert(process.platform === 'darwin', 'Supply the signing password in the local signer environment.');
    const dialog = 'set response to display dialog "请输入正式更新私钥的解锁密码。" with title "熊猫小抽屉 · 更新签名" default answer "" with hidden answer buttons {"取消", "签名"} default button "签名" cancel button "取消" giving up after 240\nif gave up of response then error number -128\nreturn text returned of response';
    try {
      const answer = await execute('/usr/bin/osascript', ['-e', dialog], { timeout: 250_000 });
      env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD = answer.stdout.replace(/\n$/, '');
    } catch {
      throw new Error('Local signing dialog cancelled or unavailable.');
    }
  }
  try {
    // Signer diagnostics may contain private input; only report a generic failure.
    const cli = path.join(root, 'node_modules/@tauri-apps/cli/tauri.js');
    const emptyPassword = env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD === '' ? ['--password', ''] : [];
    await execute(process.execPath, [cli, 'signer', 'sign', ...emptyPassword, '--app-version', version, file], { env, timeout: 90_000 });
  } catch {
    throw new Error('Tauri signing failed. Check the local production key and its password.');
  }
}

async function signOrVerify(mode, file, receiptFile) {
  await ordinaryFile(receiptFile, UPDATE_POLICY.maxDocumentBytes);
  const receipt = await readJson(receiptFile);
  const platform = platforms[receipt.target];
  assert(platform && receipt.kind === platform.kind && receipt.schemaVersion === 1, 'Unrecognized production candidate receipt.');
  const profile = await readJson(path.join(root, 'build-profiles', platform.profile));
  const head = (await execute('git', ['rev-parse', 'HEAD'], { cwd: root })).stdout.trim();
  assert(receipt.sourceCommit === head && receipt.version === profile.version && receipt.identifier === profile.identifier
    && receipt.target === profile.target && profile.identifier === UPDATE_POLICY.identifier, 'Receipt does not match this release checkout.');
  assert(path.basename(file) === 'PandaDrawer_' + profile.version + platform.suffix, 'Expected the final production artifact name.');
  const artifact = await describeArtifact(file);
  assert.deepEqual(artifact, receipt.artifact, 'Artifact differs from its production receipt.');
  const verify = async signatureFile => {
    await ordinaryFile(signatureFile, UPDATE_POLICY.maxSignatureBytes);
    const metadata = {
      schemaVersion: 1, profile: profile.profile, identifier: profile.identifier, releaseRepository: profile.releaseRepository,
      version: profile.version, target: platform.target, tag: 'v' + profile.version,
      notes: 'Local production update artifact verification.', pub_date: new Date().toISOString(),
      artifact: { ...artifact, signature: (await readFile(signatureFile, 'utf8')).trim() },
    };
    const proof = await verifyUpdateArtifact({ manifest: generateStaticManifest(metadata), artifact: file, publicKeyPath, target: platform.target });
    assert(proof.artifact.sha256 === artifact.sha256 && proof.artifact.bytes === artifact.bytes, 'Signed bytes differ from the verified candidate.');
    return { verified: true, sourceCommit: head, version: profile.version, target: platform.target, artifact, signature: file + '.sig' };
  };
  const signature = file + '.sig';
  if (mode === 'verify') return verify(signature);
  try {
    await lstat(signature);
    return verify(signature);
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
  }
  const staging = await mkdtemp(path.join(path.dirname(file), '.sign-'));
  try {
    const copy = path.join(staging, artifact.name);
    await copyFile(file, copy, constants.COPYFILE_FICLONE | constants.COPYFILE_EXCL);
    assert.deepEqual(await describeArtifact(copy), artifact, 'Signing copy differs from candidate receipt.');
    await signLocally(copy, profile.version);
    const proof = await verify(copy + '.sig');
    await link(copy + '.sig', signature);
    return proof;
  } finally {
    await rm(staging, { recursive: true, force: true });
  }
}

const [mode, ...args] = process.argv.slice(2);
try {
  let result;
  if (mode === 'archive' && args.length === 1) result = await archive(path.resolve(args[0]));
  else if (['sign', 'verify'].includes(mode) && args.length === 2) {
    const file = path.join(await realpath(path.dirname(path.resolve(args[0]))), path.basename(args[0]));
    result = await signOrVerify(mode, file, path.resolve(args[1]));
  }
  else throw new Error('Use archive NEW_DIRECTORY, sign ARTIFACT RECEIPT, or verify ARTIFACT RECEIPT.');
  console.log(JSON.stringify(result, null, 2));
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}
