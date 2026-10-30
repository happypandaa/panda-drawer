import assert from 'node:assert/strict';
import { cp, link, lstat, mkdir, mkdtemp, readFile, readdir, realpath, rm, symlink, writeFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { strictJson, validateInputs } from './production-macos-preflight.mjs';

// Keep pure validation on both hosts; the filesystem planner prepares the
// macOS production layout and enforces its Unix ownership/permission contract.
const macPreflightFilesystem = {
  skip: process.platform !== 'darwin' && 'macOS production-preflight filesystem fixture; no Windows production proof',
};

const repository = fileURLToPath(new URL('../', import.meta.url));
const script = 'scripts/production-macos-preflight.mjs';
const files = {
  profile: 'build-profiles/production.json', signingPolicy: 'build-profiles/macos-signing-policy.json',
  developmentConfiguration: 'apps/desktop/src-tauri/tauri.conf.json',
  nodeRuntime: 'scripts/node-runtime-release.json', developmentNode: 'scripts/development-node-release.json', developmentGit: 'scripts/development-git-release.json',
};
const originals = Object.fromEntries(await Promise.all(Object.entries(files).map(async ([key, file]) => [key, await readFile(path.join(repository, file))])));
const json = value => Buffer.from(`${JSON.stringify(value, null, 2)}\n`);
const changed = (key, mutate) => { const value = JSON.parse(originals[key]); mutate(value); return { ...originals, [key]: json(value) }; };
function run(root, args = [], extraEnvironment = {}, cwd = os.tmpdir()) {
  const result = spawnSync(process.execPath, [path.join(root, script), ...args], { encoding: 'utf8', cwd, env: { PATH: '/usr/bin:/bin', ...extraEnvironment }, maxBuffer: 1024 * 1024 });
  assert.equal(result.error, undefined);
  return result;
}
async function fixture(t) {
  const root = await realpath(await mkdtemp(path.join(os.tmpdir(), 'panda-production-plan-test-')));
  t.after(() => rm(root, { recursive: true, force: true }));
  for (const relative of [...Object.values(files), script]) {
    await mkdir(path.dirname(path.join(root, relative)), { recursive: true });
    await cp(path.join(repository, relative), path.join(root, relative));
  }
  return root;
}
async function noTarget(root) { await assert.rejects(lstat(path.join(root, 'target')), { code: 'ENOENT' }); }
async function preservedInputs(root) {
  for (const [key, relative] of Object.entries(files)) assert.deepEqual(await readFile(path.join(root, relative)), originals[key]);
}
function mergePatch(target, patch) {
  if (patch === null || typeof patch !== 'object' || Array.isArray(patch)) return structuredClone(patch);
  const result = target !== null && typeof target === 'object' && !Array.isArray(target) ? structuredClone(target) : {};
  for (const [key, value] of Object.entries(patch)) {
    if (value === null) delete result[key];
    else result[key] = mergePatch(result[key], value);
  }
  return result;
}

test('strict JSON rejects duplicate/escaped duplicate keys, invalid UTF-8, nesting and oversized inputs', () => {
  for (const text of ['{"a":1,"a":2}', '{"a":1,"\\u0061":2}', '[{"nested":{"a":1,"a":2}}]']) assert.throws(() => strictJson(Buffer.from(text)), /重复/);
  assert.throws(() => strictJson(Buffer.from([0xff])), /encoded|encoding/i);
  assert.throws(() => strictJson(Buffer.from(`${'['.repeat(66)}0${']'.repeat(66)}`)), /嵌套/);
  assert.throws(() => strictJson(Buffer.alloc(256 * 1024 + 1)), /大小/);
  assert.deepEqual(strictJson(Buffer.from('{"a":[1,true,null,{"escaped":"a\\\"b"}],"b":{}}')), { a: [1, true, null, { escaped: 'a"b' }], b: {} });
});

test('fixed production fields reject unknown/missing fields and every identity/profile/target override', () => {
  const profile = JSON.parse(originals.profile);
  for (const key of Object.keys(profile)) {
    assert.throws(() => validateInputs(changed('profile', value => { value[key] = key === 'schemaVersion' ? 2 : 'changed'; })), /生产配置/);
    assert.throws(() => validateInputs(changed('profile', value => { delete value[key]; })), /字段/);
  }
  for (const key of ['dataRoot', 'releaseReady', 'updaterPubkey', 'other']) assert.throws(() => validateInputs(changed('profile', value => { value[key] = true; })), /字段/);
  assert.equal(validateInputs(originals).profile.identifier, 'com.happypandaa.pandadrawer');
});

test('signing policy accepts only explicit nulls or exact public binding formats without granting readiness', () => {
  for (const [key, values] of [
    ['schemaVersion', [2, null]], ['signingProfile', ['other', null]],
    ['expectedTeamId', ['', 1234567890, 'smallteam1', 'ABCDEFGHIJK']],
    ['nodeEntitlementsSha256', ['', 'A'.repeat(64), 123]], ['gitEntitlementsSha256', ['a'.repeat(63), {}, true]],
  ]) for (const value of values) assert.throws(() => validateInputs(changed('signingPolicy', policy => { policy[key] = value; })), /签名策略/);
  assert.throws(() => validateInputs(changed('signingPolicy', policy => { policy.ready = true; })), /字段/);
  const explicit = changed('signingPolicy', policy => { policy.expectedTeamId = 'ABCDEFGHI1'; policy.nodeEntitlementsSha256 = 'a'.repeat(64); policy.gitEntitlementsSha256 = 'b'.repeat(64); });
  assert.equal(validateInputs(explicit).policy.expectedTeamId, 'ABCDEFGHI1');
});

test('development layout or identity drift refuses planning instead of inheriting unknown inputs', () => {
  for (const mutate of [
    base => { base.identifier = 'other'; }, base => { base.version = '1.0.0'; },
    base => { base.build.frontendDist = '/other'; }, base => { base.build.features = ['other']; },
    base => { base.bundle.externalBin.push('other'); }, base => { base.bundle.resources.extra = '/other'; },
    base => { base.bundle.macOS.signingIdentity = 'some identity'; },
    base => { base.bundle.icon = ['../../other']; }, base => { base.app.windows[0].url = 'https://example.invalid'; },
    base => { base.plugins = { updater: {} }; },
  ]) assert.throws(() => validateInputs(changed('developmentConfiguration', mutate)));
  for (const key of ['nodeRuntime', 'developmentNode', 'developmentGit']) {
    assert.throws(() => validateInputs(changed(key, source => { source.arch = 'x64'; })), /来源/);
    assert.throws(() => validateInputs(changed(key, source => { source.platform = 'win32'; })), /来源/);
  }
});

test('default check works from an unrelated cwd, is read-only and reports all pending proof honestly', macPreflightFilesystem, async t => {
  const root = await fixture(t);
  const result = run(root);
  assert.equal(result.status, 0, result.stderr);
  const report = JSON.parse(result.stdout);
  assert.equal(report.mode, 'check'); assert.equal(report.outputWritten, false);
  assert.equal(report.metadataValidated, true);
  for (const key of ['productionBuildAvailable', 'runtimeAdmission', 'releaseReady']) assert.equal(report[key], false);
  assert.equal(report.signingPolicy.configured, false); assert.equal(report.signingPolicy.evidenceVerified, false);
  assert.equal(report.missingRequirements.length, 9);
  assert.ok(report.missingRequirements.some(item => item.code === 'UPDATER_HANDOFF_UNIMPLEMENTED' && item.message.includes('ReadyForNativeExit')));
  assert.match(report.layout.output, new RegExp(`^${root}/target/production-macos/preflight-[a-f0-9-]+$`));
  assert.equal(report.plannedBuildEnvironment.CARGO_TARGET_DIR, report.layout.cargoTarget);
  await noTarget(root); await preservedInputs(root);
});

test('prepare exclusively creates private metadata and empty future slots without touching development outputs', macPreflightFilesystem, async t => {
  const root = await fixture(t);
  const sentinel = path.join(root, 'target/release/bundle/macos/熊猫小抽屉.app/sentinel');
  const developmentResource = path.join(root, 'apps/desktop/src-tauri/resources/sentinel');
  for (const file of [sentinel, developmentResource]) { await mkdir(path.dirname(file), { recursive: true }); await writeFile(file, 'preserve development output'); }
  const result = run(root, ['prepare']);
  assert.equal(result.status, 0, result.stderr);
  const report = JSON.parse(result.stdout);
  assert.equal(report.outputWritten, true);
  assert.deepEqual(JSON.parse(await readFile(report.layout.report)), report);
  assert.deepEqual(JSON.parse(await readFile(report.layout.tauriOverride)), report.tauriOverride);
  for (const key of ['cargoTarget', 'resources', 'sourceTools', 'frontend', 'sidecarDirectory', 'signingDirectory', 'receipts']) {
    assert.deepEqual(await readdir(report.layout[key]), []);
    assert.equal((await lstat(report.layout[key])).mode & 0o777, 0o700);
  }
  for (const key of ['report', 'tauriOverride']) assert.equal((await lstat(report.layout[key])).mode & 0o777, 0o600);
  for (const file of [sentinel, developmentResource]) assert.equal(await readFile(file, 'utf8'), 'preserve development output');
  const again = run(root, ['prepare']); assert.equal(again.status, 0, again.stderr);
  assert.notEqual(JSON.parse(again.stdout).layout.output, report.layout.output);
  assert.deepEqual(JSON.parse(await readFile(report.layout.report)), report);
  await preservedInputs(root);
});

test('Tauri merge patch removes development resources/hooks and exclusively selects isolated production inputs', macPreflightFilesystem, async t => {
  const root = await fixture(t);
  const result = run(root, ['prepare']); assert.equal(result.status, 0, result.stderr);
  const report = JSON.parse(result.stdout);
  const base = JSON.parse(originals.developmentConfiguration);
  const effective = mergePatch(base, report.tauriOverride);
  assert.deepEqual(effective.bundle.resources, { [`${report.layout.resources}/`]: 'resources/' });
  assert.deepEqual(effective.bundle.externalBin, [report.layout.sidecarBase]);
  assert.equal(effective.identifier, 'com.happypandaa.pandadrawer'); assert.equal(effective.version, '0.1.0');
  assert.equal(effective.app.windows[0].title, '熊猫小抽屉');
  assert.equal(effective.build.frontendDist, report.layout.frontend);
  assert.deepEqual(effective.build.features, ['production-resource-anchors']);
  for (const key of ['beforeBuildCommand', 'beforeDevCommand', 'beforeBundleCommand', 'devUrl']) assert.equal(Object.hasOwn(effective.build, key), false);
  assert.equal(Object.hasOwn(effective.bundle.macOS, 'signingIdentity'), false);
  assert.equal(effective.bundle.macOS.hardenedRuntime, true);
  assert.equal(effective.bundle.macOS.minimumSystemVersion, '15.0');
  assert.equal(effective.bundle.createUpdaterArtifacts, false);
  assert.deepEqual(effective.app.security, base.app.security);
  assert.deepEqual(base, JSON.parse(originals.developmentConfiguration));
  await assert.rejects(lstat(report.layout.sidecarTarget), { code: 'ENOENT' });
  await assert.rejects(lstat(report.layout.compiledToolManifest), { code: 'ENOENT' });
});

test('CLI rejects identity/root/config/output options and environment overrides before creating output', async t => {
  const root = await fixture(t);
  for (const args of [['--root', '/elsewhere'], ['--config=other'], ['prepare', 'other'], ['--identifier=other'], ['build'], ['--output=/elsewhere']]) {
    const result = run(root, args); assert.equal(result.status, 1); assert.match(result.stderr, /只接受/);
  }
  for (const key of ['TAURI_CONFIG', 'CARGO_TARGET_DIR', 'CARGO_BUILD_TARGET', 'PANDA_DRAWER_COMPILED_TOOL_MANIFEST', 'PANDA_DRAWER_COMPILED_TOOL_ROOT']) {
    const result = run(root, ['prepare'], { [key]: 'SYNTHETIC_VALUE_NOT_TO_PRINT' });
    assert.equal(result.status, 1); assert.ok(result.stderr.includes(key)); assert.ok(!result.stderr.includes('SYNTHETIC_VALUE_NOT_TO_PRINT'));
  }
  await noTarget(root);
});

test('Mac preflight accepts only the fixed deployment target without creating output', macPreflightFilesystem, async t => {
  const root = await fixture(t);
  assert.equal(run(root, ['prepare'], { MACOSX_DEPLOYMENT_TARGET: '14.0' }).status, 1);
  assert.equal(run(root, ['check'], { MACOSX_DEPLOYMENT_TARGET: '15.0' }).status, 0);
  await noTarget(root);
});

test('signing environment values are not read or emitted and cannot enable any operation', macPreflightFilesystem, async t => {
  const root = await fixture(t);
  const result = run(root, ['prepare'], { APPLE_SIGNING_IDENTITY: 'SYNTHETIC_PRIVATE_MARKER', APPLE_PASSWORD: 'SYNTHETIC_PRIVATE_MARKER', TAURI_SIGNING_PRIVATE_KEY: 'SYNTHETIC_PRIVATE_MARKER' });
  assert.equal(result.status, 0, result.stderr);
  assert.ok(!`${result.stdout}${result.stderr}`.includes('SYNTHETIC_PRIVATE_MARKER'));
  const report = JSON.parse(result.stdout);
  assert.equal(report.productionBuildAvailable, false); assert.equal(report.releaseReady, false);
  assert.ok(!(await readFile(report.layout.report, 'utf8')).includes('SYNTHETIC_PRIVATE_MARKER'));
  const source = await readFile(path.join(root, script), 'utf8');
  for (const forbidden of ['node:child_process', 'fetch(', 'node:https', 'node:http', 'process.env.APPLE', 'process.env.TAURI_SIGNING']) assert.ok(!source.includes(forbidden));
});

test('configured public policy still leaves all actual signing/build/installation proofs pending', macPreflightFilesystem, async t => {
  const root = await fixture(t);
  await writeFile(path.join(root, files.signingPolicy), changed('signingPolicy', policy => { policy.expectedTeamId = 'ABCDEFGHI1'; policy.nodeEntitlementsSha256 = 'a'.repeat(64); policy.gitEntitlementsSha256 = 'b'.repeat(64); }).signingPolicy);
  const result = run(root); assert.equal(result.status, 0, result.stderr);
  const report = JSON.parse(result.stdout);
  assert.equal(report.signingPolicy.configured, true); assert.equal(report.signingPolicy.evidenceVerified, false);
  assert.equal(report.missingRequirements.length, 6); assert.equal(report.runtimeAdmission, false); assert.equal(report.releaseReady, false);
  await noTarget(root);
});

test('extra platform configuration, linked inputs and linked input ancestors refuse before mutation', macPreflightFilesystem, async t => {
  const root = await fixture(t);
  const extra = path.join(root, 'apps/desktop/src-tauri/tauri.macos.conf.json');
  await writeFile(extra, '{}');
  let result = run(root, ['prepare']); assert.equal(result.status, 1); assert.match(result.stderr, /额外 Tauri/);
  await rm(extra);
  const input = path.join(root, files.profile), kept = path.join(root, 'saved-profile.json');
  await cp(input, kept); await rm(input); await symlink(kept, input);
  result = run(root, ['prepare']); assert.equal(result.status, 1); assert.match(result.stderr, /无链接/);
  await rm(input); await link(kept, input);
  result = run(root, ['prepare']); assert.equal(result.status, 1); assert.match(result.stderr, /无链接/);
  await rm(input); await rm(kept);
  const originalDirectory = path.join(root, 'build-profiles'), relocated = path.join(root, 'saved-build-profiles');
  await cp(originalDirectory, relocated, { recursive: true }); await rm(originalDirectory, { recursive: true }); await symlink(relocated, originalDirectory);
  result = run(root, ['prepare']); assert.equal(result.status, 1); assert.match(result.stderr, /祖先/);
  await noTarget(root);
});

test('linked or permissive output parents refuse without following or deleting their contents', macPreflightFilesystem, async t => {
  const root = await fixture(t);
  const outside = path.join(root, 'outside'); await mkdir(outside); await writeFile(path.join(outside, 'sentinel'), 'retain');
  await symlink(outside, path.join(root, 'target'));
  let result = run(root, ['prepare']); assert.equal(result.status, 1); assert.equal(await readFile(path.join(outside, 'sentinel'), 'utf8'), 'retain');
  assert.deepEqual(await readdir(outside), ['sentinel']);
  await rm(path.join(root, 'target')); await mkdir(path.join(root, 'target'));
  await symlink(outside, path.join(root, 'target/production-macos'));
  result = run(root, ['prepare']); assert.equal(result.status, 1); assert.deepEqual(await readdir(outside), ['sentinel']);
  await rm(path.join(root, 'target/production-macos'));
  await mkdir(path.join(root, 'target/production-macos'), { mode: 0o755 });
  result = run(root, ['prepare']); assert.equal(result.status, 1); assert.match(result.stderr, /私有目录/);
  assert.deepEqual(await readdir(path.join(root, 'target/production-macos')), []);
});

test('missing or malformed source metadata does not leave a partial output', macPreflightFilesystem, async t => {
  const root = await fixture(t);
  await writeFile(path.join(root, files.nodeRuntime), '{"schemaVersion":1,"schemaVersion":1}');
  let result = run(root, ['prepare']); assert.equal(result.status, 1); assert.match(result.stderr, /重复/); await noTarget(root);
  await rm(path.join(root, files.nodeRuntime));
  result = run(root, ['prepare']); assert.equal(result.status, 1); await noTarget(root);
});

test('invoking through a repository symlink still uses the script repository and never the current directory', macPreflightFilesystem, async t => {
  const root = await fixture(t);
  const linkRoot = path.join(root, 'workspace-link'); await symlink(root, linkRoot);
  const result = run(linkRoot, ['check']); assert.equal(result.status, 0, result.stderr);
  assert.ok(JSON.parse(result.stdout).layout.output.startsWith(`${root}/target/production-macos/`));
  await noTarget(root);
});
