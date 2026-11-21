import assert from 'node:assert/strict';
import { link, lstat, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import {
  createWindowsCandidatePlan, productionSigningState, rejectProductionOverrides,
  runWindowsCandidate, stageWindowsProductionCli, validateWindowsProductionInputs,
} from './production-windows-candidate.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
const paths = {
  profile: 'build-profiles/production.windows-x64.json', updaterPublicKey: 'build-profiles/production-updater.pub',
  sources: 'distribution/download-sources.json', packageManifest: 'package.json',
  baseConfiguration: 'apps/desktop/src-tauri/tauri.conf.json', windowsConfiguration: 'apps/desktop/src-tauri/tauri.windows.conf.json',
  nodeRuntime: 'scripts/node-runtime-release.windows-x64.json', developmentNode: 'scripts/development-node-release.windows-x64.json',
  developmentGit: 'scripts/development-git-release.windows-x64.json',
};
const inputs = Object.fromEntries(await Promise.all(Object.entries(paths).map(async ([key, file]) => [key, await readFile(path.join(root, file))])));
const changed = (key, transform) => {
  const value = JSON.parse(inputs[key]);
  transform(value);
  return { ...inputs, [key]: Buffer.from(JSON.stringify(value)) };
};

test('Cargo hard-linked output becomes an independent byte-identical sidecar without changing cache files', async t => {
  const temporary = await realpath(await mkdtemp(path.join(os.tmpdir(), 'panda-production-cli-stage-test-')));
  t.after(async () => {
    assert.equal(await realpath(temporary), temporary);
    assert.match(path.basename(temporary), /^panda-production-cli-stage-test-/);
    await rm(temporary, { recursive: true });
  });
  const release = path.join(temporary, 'release');
  const deps = path.join(release, 'deps');
  const sidecars = path.join(temporary, 'sidecars');
  await mkdir(deps, { recursive: true });
  await mkdir(sidecars);
  const compiled = path.join(release, 'local-projects-dev.exe');
  const dependency = path.join(deps, 'local_projects_dev.exe');
  const staged = path.join(sidecars, 'local-projects-dev-x86_64-pc-windows-msvc.exe');
  const bytes = Buffer.from('fixture represents Cargo executable bytes; no process is launched\n');
  await writeFile(dependency, bytes);
  await link(dependency, compiled);
  const cachedBefore = await lstat(compiled, { bigint: true });
  assert.equal(cachedBefore.nlink, 2n);
  const receipt = await stageWindowsProductionCli({ compiled, staged });
  const stagedInfo = await lstat(staged, { bigint: true });
  const cachedAfter = await lstat(compiled, { bigint: true });
  assert.equal(stagedInfo.nlink, 1n);
  assert(stagedInfo.dev !== cachedAfter.dev || stagedInfo.ino !== cachedAfter.ino);
  for (const key of ['dev', 'ino', 'nlink', 'size', 'mtimeNs', 'ctimeNs']) {
    assert.equal(cachedAfter[key], cachedBefore[key], `Cargo cache ${key} changed`);
  }
  assert.deepEqual(await readFile(staged), bytes);
  assert.deepEqual(await readFile(compiled), bytes);
  assert.deepEqual(await readFile(dependency), bytes);
  assert.deepEqual(receipt, { name: path.basename(staged), bytes: bytes.length,
    sha256: createHash('sha256').update(bytes).digest('hex') });

  // A pre-existing hard-linked stage is rejected rather than reused or
  // detached in place; the delivery receipt still requires exactly one link.
  const occupied = path.join(sidecars, 'occupied.exe');
  await link(dependency, occupied);
  await assert.rejects(stageWindowsProductionCli({ compiled, staged: occupied }), { code: 'EEXIST' });
  assert.equal((await lstat(occupied, { bigint: true })).nlink, 3n);
  assert.deepEqual(await readFile(dependency), bytes);
});

test('Windows production profile fixes identity/version/target without changing the Mac profile', () => {
  const value = validateWindowsProductionInputs(inputs);
  assert.equal(value.profile.identifier, 'com.happypandaa.pandadrawer');
  assert.equal(value.profile.version, '0.1.0');
  assert.equal(value.profile.target, 'x86_64-pc-windows-msvc');
  assert.equal(value.publicKeyPacketSha256.length, 64);
  for (const key of Object.keys(value.profile)) {
    assert.throws(() => validateWindowsProductionInputs(changed('profile', profile => { delete profile[key]; })), /fixed/);
    assert.throws(() => validateWindowsProductionInputs(changed('profile', profile => { profile[key] = 'unreviewed'; })), /fixed/);
  }
  assert.throws(() => validateWindowsProductionInputs(changed('profile', profile => { profile.dataRoot = 'elsewhere'; })), /fixed/);
});

test('production channel validation rejects test repositories, local routes and swapped countries', () => {
  for (const change of [
    sources => { sources.releaseRepository += '-test'; },
    sources => { sources.updates.sources.github.manifestUrl = 'http://127.0.0.1/latest.json'; },
    sources => { sources.updates.sources.github.artifactBaseUrl += '-test'; },
    sources => { sources.updates.sources.oss.manifestUrl = 'https://panda-updates.zhimibuhui.com/_check/latest.json'; },
    sources => { sources.updates.sources.oss.artifactBaseUrl += '/_check'; },
    sources => { sources.updates.sources.extra = { manifestUrl: 'https://other.invalid/latest.json', artifactBaseUrl: 'https://other.invalid' }; },
    sources => { sources.routes.cn.updates = 'github'; },
    sources => { sources.routes.global.updates = 'oss'; },
  ]) assert.throws(() => validateWindowsProductionInputs(changed('sources', change)), /routes/);
  assert.throws(() => validateWindowsProductionInputs({ ...inputs, updaterPublicKey: Buffer.from('not a production public key') }), /base64|key/i);
});

test('candidate NSIS isolates installed program identity and uses only the reviewed production feature', () => {
  const validated = validateWindowsProductionInputs(inputs);
  const plan = createWindowsCandidatePlan(path.resolve(root), validated, '00000000-0000-4000-8000-000000000001');
  assert.equal(plan.configuration.productName, 'PandaDrawer');
  assert.equal(plan.configuration.mainBinaryName, 'PandaDrawer');
  assert.equal(plan.configuration.identifier, validated.profile.identifier);
  assert.equal(plan.configuration.app.windows[0].title, '熊猫小抽屉');
  assert.deepEqual(plan.configuration.build.features, ['production-resource-anchors']);
  assert.equal(plan.configuration.build.beforeBuildCommand, null);
  assert.equal(plan.configuration.build.devUrl, null);
  assert.equal(plan.configuration.bundle.createUpdaterArtifacts, false, 'Final canonical filename must be signed separately.');
  assert.equal(plan.configuration.bundle.windows.nsis.installMode, 'currentUser');
  assert.equal(plan.configuration.bundle.windows.webviewInstallMode.type, 'skip');
  assert.match(plan.configuration.bundle.windows.nsis.installerHooks, /windows-webview2\.nsh$/);
  assert.equal(path.basename(plan.layout.executable), 'PandaDrawer_0.1.0_x64-setup.exe');
  assert.equal(plan.layout.cargoTarget, path.join(path.resolve(root), 'target/production-windows/cargo-target'));
  assert.throws(() => createWindowsCandidatePlan(root, validated, '../unsafe'), /slot/);
});

test('runtime source anchors remain Windows x64 and setup hook cannot silently disappear', () => {
  for (const key of ['nodeRuntime', 'developmentNode', 'developmentGit']) {
    assert.throws(() => validateWindowsProductionInputs(changed(key, source => { source.platform = 'darwin'; })), /anchor/);
    assert.throws(() => validateWindowsProductionInputs(changed(key, source => { source.arch = 'arm64'; })), /anchor/);
  }
  assert.throws(() => validateWindowsProductionInputs(changed('windowsConfiguration', config => {
    delete config.bundle.windows.nsis.installerHooks;
  })), /hook/);
});

test('environment cannot replace identity/resources/updater key and errors do not expose supplied values', () => {
  for (const key of ['TAURI_CONFIG', 'CARGO_TARGET_DIR', 'CARGO_BUILD_TARGET', 'RUSTFLAGS',
    'PANDA_DRAWER_COMPILED_TOOL_MANIFEST', 'PANDA_DRAWER_COMPILED_TOOL_ROOT', 'PANDA_DRAWER_COMPILED_CLI_SHA256',
    'PANDA_DRAWER_UPDATE_PUBLIC_KEY', 'PANDA_DRAWER_UPDATE_TEST_VERSION']) {
    assert.throws(() => rejectProductionOverrides({ [key]: 'artificial-sensitive-sentinel' }), error => {
      assert(error.message.includes(key));
      assert(!error.message.includes('artificial-sensitive-sentinel'));
      return true;
    });
  }
  rejectProductionOverrides({ CARGO_BUILD_JOBS: '2', TAURI_SIGNING_PRIVATE_KEY_PATH: 'operator-local-input' });
});

test('signing availability reports booleans only and never creates replacement keys', () => {
  assert.deepEqual(productionSigningState({}), { keySupplied: false, ambiguous: false, passwordSupplied: false });
  const state = productionSigningState({ TAURI_SIGNING_PRIVATE_KEY: 'artificial-key-secret', TAURI_SIGNING_PRIVATE_KEY_PASSWORD: 'artificial-password-secret' });
  assert.deepEqual(state, { keySupplied: true, ambiguous: false, passwordSupplied: true });
  assert(!JSON.stringify(state).includes('secret'));
  assert.equal(productionSigningState({ TAURI_SIGNING_PRIVATE_KEY: 'a', TAURI_SIGNING_PRIVATE_KEY_PATH: 'b' }).ambiguous, true);
});

test('candidate interface contains no publication or arbitrary production override command', async () => {
  for (const args of [['publish'], ['tag'], ['release'], ['build', '--identifier', 'other'], ['verify']]) {
    await assert.rejects(runWindowsCandidate(args), /Unsupported|Use/);
  }
  assert.match((await runWindowsCandidate(['--help'])).help, /No installation, tag, Release, OSS publication/);
});
