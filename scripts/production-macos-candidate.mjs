// Build locally, notarize with Apple, and verify the exact distribution bytes.
// The candidate has no publication or production-channel write operation.
import { createHash, randomUUID } from 'node:crypto';
import { createReadStream } from 'node:fs';
import {
  chmod,
  copyFile,
  cp,
  lstat,
  mkdir,
  mkdtemp,
  readFile,
  realpath,
  rename,
  rm,
  rmdir,
  symlink,
  writeFile,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { root as repository, run, rustEnvironment } from './process.mjs';
import {
  execute,
  prepareSignedTools,
  signCode,
  signWithTimestamp,
  verifyCode,
  verifyEntitlements,
} from './production-macos-tools.mjs';
import { prepareProductionAppResources } from './production-macos-resources.mjs';

const root = path.resolve(repository);
const profiles = path.join(root, 'build-profiles');
const desktop = path.join(root, 'apps/desktop');
const base = path.join(root, 'target/production-macos');
const cliName = 'local-projects-dev';
const mainName = 'local-projects-desktop-dev';
const uuid = /^[\da-f]{8}-[\da-f]{4}-[\da-f]{4}-[\da-f]{4}-[\da-f]{12}$/i;
const json = async (file) => JSON.parse(await readFile(file, 'utf8'));
const exclusive = (file, data) =>
  writeFile(file, JSON.stringify(data, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
async function digestFile(file) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(file)) hash.update(chunk);
  return hash.digest('hex');
}
function requireThat(value, message) {
  if (!value) throw new Error(message);
}
async function commit() {
  const options = { cwd: root };
  requireThat(
    !(await execute('/usr/bin/git', ['status', '--porcelain', '--untracked-files=normal'], options)).stdout.trim(),
    'Candidate requires a clean committed source tree.',
  );
  return (await execute('/usr/bin/git', ['rev-parse', 'HEAD'], options)).stdout.trim();
}
async function allowedSlot(slot) {
  requireThat(
    typeof slot === 'string' &&
      path.isAbsolute(slot) &&
      path.dirname(slot) === base &&
      /^preflight-[\da-f-]{36}$/.test(path.basename(slot)) &&
      uuid.test(path.basename(slot).slice(10)) &&
      (await realpath(slot)) === slot &&
      ((await lstat(slot)).mode & 0o777) === 0o700,
    'Invalid candidate slot.',
  );
  return slot;
}
function productPaths(slot, profile) {
  const output = path.join(slot, 'candidate');
  return {
    output,
    app: path.join(output, profile.displayName + '.app'),
    dmg: path.join(output, 'PandaDrawer_' + profile.version + '_aarch64.dmg'),
    receipts: path.join(slot, 'receipts'),
    manifest: path.join(slot, 'stage/signed-tool-manifest.json'),
  };
}
function cleanEnv() {
  const env = Object.fromEntries(
    ['HOME', 'USER', 'LOGNAME', 'PATH', 'TMPDIR', 'LANG', 'LC_ALL', 'DEVELOPER_DIR', 'SDKROOT']
      .filter((key) => process.env[key])
      .map((key) => [key, process.env[key]]),
  );
  return { ...rustEnvironment({ environment: env }), CARGO_BUILD_JOBS: '2', MACOSX_DEPLOYMENT_TARGET: '15.0' };
}
async function signingIdentity(team) {
  const listing = (await execute('/usr/bin/security', ['find-identity', '-v', '-p', 'codesigning'])).stdout;
  const permitted = [
    ...listing.matchAll(/^\s*\d+\) ([0-9A-F]{40}) "(Developer ID Application: [^"\n]+)"\s*$/gm),
  ].filter(
    ([, hash, name]) =>
      name.endsWith(' (' + team + ')') &&
      (!process.env.APPLE_SIGNING_IDENTITY || [hash, name].includes(process.env.APPLE_SIGNING_IDENTITY)),
  );
  requireThat(
    permitted.length === 1,
    'Select one valid Developer ID Application identity for the configured team.',
  );
  return permitted[0][1];
}
async function signAppBundle(plan, policy, identity, sourceCommit) {
  const { profile, layout } = plan;
  const { output, app } = productPaths(layout.output, profile);
  const contents = path.join(app, 'Contents'),
    macos = path.join(contents, 'MacOS'),
    resources = path.join(contents, 'Resources');
  await mkdir(output);
  await mkdir(macos, { recursive: true });
  await mkdir(resources);
  await copyFile(path.join(layout.cargoTarget, 'release', mainName), path.join(macos, mainName));
  await copyFile(layout.sidecarTarget, path.join(macos, cliName));
  await copyFile(path.join(desktop, 'src-tauri/icons/icon.icns'), path.join(resources, 'icon.icns'));
  await cp(layout.resources, path.join(resources, 'resources'), {
    recursive: true,
    errorOnExist: true,
    force: false,
  });
  // The staging root is private. Installed resources must be readable by each Mac user.
  await chmod(path.join(resources, 'resources'), 0o755);
  const info = JSON.parse(
    (await execute('/usr/bin/plutil', ['-convert', 'json', '-o', '-', path.join(desktop, 'src-tauri/Info.plist')]))
      .stdout,
  );
  Object.assign(info, {
    CFBundleDevelopmentRegion: 'English',
    CFBundleDisplayName: profile.displayName,
    CFBundleName: profile.displayName,
    CFBundleExecutable: mainName,
    CFBundleIconFile: 'icon.icns',
    CFBundleIdentifier: profile.identifier,
    CFBundleInfoDictionaryVersion: '6.0',
    CFBundlePackageType: 'APPL',
    CFBundleShortVersionString: profile.version,
    CFBundleVersion: profile.version,
    PandaDrawerSourceCommit: sourceCommit,
    LSApplicationCategoryType: 'public.app-category.developer-tools',
    LSMinimumSystemVersion: profile.minimumSystemVersion,
    NSHighResolutionCapable: true,
  });
  const plist = path.join(contents, 'Info.plist');
  await writeFile(plist, JSON.stringify(info), { flag: 'wx', mode: 0o644 });
  await execute('/usr/bin/plutil', ['-convert', 'xml1', plist]);
  // Sign the sidecar before the outer seal. Never recursively re-sign payloads.
  await signCode(path.join(macos, cliName), profile.identifier + '.cli', identity, null, policy.expectedTeamId);
  const entitlements = path.join(desktop, 'src-tauri/Entitlements.plist');
  await signCode(app, profile.identifier, identity, entitlements, policy.expectedTeamId);
  await verifyEntitlements(
    app,
    JSON.parse((await execute('/usr/bin/plutil', ['-convert', 'json', '-o', '-', entitlements])).stdout),
    path.join(layout.receipts, 'main-entitlements.plist'),
  );
  await execute('/usr/bin/codesign', ['--verify', '--deep', '--strict', app]);
  return app;
}

async function build(profile, policy) {
  const sourceCommit = await commit();
  const preflight = path.join(root, 'scripts/production-macos-preflight.mjs');
  const environment = cleanEnv();
  const { stdout } = await execute(process.execPath, [preflight, 'prepare'], { cwd: root, env: environment });
  const plan = JSON.parse(stdout),
    { layout } = plan;
  await allowedSlot(layout.output);
  console.error('Candidate slot: ' + layout.output);
  requireThat(
    plan.profile.identifier === profile.identifier && plan.signingPolicy.configured,
    'Production signing policy is incomplete.',
  );
  const identity = await signingIdentity(policy.expectedTeamId);
  await exclusive(path.join(layout.receipts, 'build-start.json'), { sourceCommit });
  await prepareSignedTools(plan, identity, policy);
  await prepareProductionAppResources({ root, resources: layout.resources });
  const publicDir = path.join(layout.stage, 'frontend-public');
  await mkdir(publicDir);
  const publicFiles = (
    await execute('/usr/bin/git', ['ls-files', '-z', '--', 'apps/desktop/public'], { cwd: root })
  ).stdout
    .split('\0')
    .filter(Boolean);
  for (const item of publicFiles) {
    const target = path.join(publicDir, path.relative(path.join(desktop, 'public'), path.join(root, item)));
    const file = path.join(root, item);
    requireThat(
      (await realpath(file)) === file && (await lstat(file)).isFile(),
      'Unexpected nonregular frontend resource.',
    );
    await mkdir(path.dirname(target), { recursive: true });
    await copyFile(file, target);
  }
  const env = { ...environment, ...plan.plannedBuildEnvironment };
  await run('pnpm', ['exec', 'tsc', '-b'], { cwd: desktop, env });
  const { build: buildFrontend } = await import(
    pathToFileURL(path.join(desktop, 'node_modules/vite/dist/node/index.js')).href
  );
  await buildFrontend({
    root: desktop,
    publicDir,
    envDir: false,
    envPrefix: [],
    build: { outDir: layout.frontend, emptyOutDir: false },
  });
  await run(
    'cargo',
    [
      'build',
      '--locked',
      '--release',
      '-p',
      'platform-cli',
      '--features',
      'production-resource-anchors',
      '--bin',
      cliName,
    ],
    { cwd: root, env },
  );
  await copyFile(path.join(layout.cargoTarget, 'release', cliName), layout.sidecarTarget);
  await copyFile(
    path.join(desktop, 'src-tauri/Entitlements.plist'),
    path.join(layout.signingDirectory, 'main.entitlements.plist'),
  );
  await run(
    process.execPath,
    [
      path.join(root, 'node_modules/@tauri-apps/cli/tauri.js'),
      'build',
      '--no-bundle',
      '--ci',
      '--config',
      layout.tauriOverride,
      '--',
      '--locked',
    ],
    { cwd: desktop, env },
  );
  await signAppBundle(plan, policy, identity, sourceCommit);
  requireThat((await commit()) === sourceCommit, 'Source changed during candidate build.');
  return layout.output;
}

async function notarize(file, receipts, kind) {
  const args = ['--keychain-profile', 'panda-drawer-notary', '--output-format', 'json'];
  const record = path.join(receipts, kind + '-submission.json');
  let id;
  try {
    id = (await json(record)).id;
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
    const { stdout } = await execute('/usr/bin/xcrun', ['notarytool', 'submit', file, ...args], {
      timeout: 900_000,
    });
    id = JSON.parse(stdout).id;
    requireThat(uuid.test(id), 'Apple did not return a notarization submission ID.');
    await exclusive(record, { id });
  }
  requireThat(uuid.test(id), 'Invalid notarization submission ID.');
  console.error('Waiting for Apple ' + kind + ' notarization: ' + id);
  const { stdout } = await execute('/usr/bin/xcrun', ['notarytool', 'wait', id, ...args], { timeout: 3_600_000 });
  requireThat(
    JSON.parse(stdout).status === 'Accepted',
    'Apple did not accept ' + kind + ' notarization (' + id + ').',
  );
  return id;
}
async function checkApp(app, profile, policy, manifestSha, sourceCommit) {
  const plist = path.join(app, 'Contents/Info.plist');
  const info = JSON.parse((await execute('/usr/bin/plutil', ['-convert', 'json', '-o', '-', plist])).stdout);
  requireThat(
    info.CFBundleIdentifier === profile.identifier &&
      info.CFBundleShortVersionString === profile.version &&
      info.CFBundleVersion === profile.version &&
      info.PandaDrawerSourceCommit === sourceCommit &&
      info.CFBundleExecutable === mainName &&
      info.LSMinimumSystemVersion === profile.minimumSystemVersion,
    'Candidate app metadata mismatch.',
  );
  const cli = path.join(app, 'Contents/MacOS', cliName);
  await verifyCode(app, profile.identifier, policy.expectedTeamId);
  await verifyCode(cli, profile.identifier + '.cli', policy.expectedTeamId);
  await execute('/usr/bin/codesign', ['--verify', '--deep', '--strict', app]);
  const buildInfo = JSON.parse((await execute(cli, ['build-info', '--json'])).stdout).data;
  requireThat(
    buildInfo.profile === 'production' &&
      buildInfo.version === profile.version &&
      buildInfo.identifier === profile.identifier &&
      buildInfo.resourceManifestSha256 === manifestSha,
    'Compiled production resource anchors mismatch.',
  );
  await execute('/usr/bin/xcrun', ['stapler', 'validate', app]);
  let admission;
  try {
    admission = JSON.parse((await execute(cli, ['verify-installation', '--json'], { timeout: 120_000 })).stdout);
  } catch (error) {
    const reason = error.stdout && JSON.parse(error.stdout).error;
    if (reason?.code && reason?.message) throw new Error(reason.code + ' ' + reason.message);
    throw error;
  }
  requireThat(
    admission.ok &&
      admission.data?.runtimeAdmission === true &&
      admission.data?.signatureEvidenceVerified === true,
    'Bundle runtime admission did not verify.',
  );
  await execute('/usr/sbin/spctl', ['--assess', '--type', 'execute', app], { timeout: 120_000 });
}
async function verifyArtifacts(slot, profile, policy, record) {
  const { app, dmg, manifest } = productPaths(slot, profile);
  requireThat(
    record.schemaVersion === 1 &&
      record.kind === 'macos-production-candidate' &&
      record.version === profile.version &&
      record.identifier === profile.identifier &&
      record.teamId === policy.expectedTeamId &&
      record.sourceCommit === (await commit()) &&
      record.app === path.relative(slot, app) &&
      record.dmg === path.relative(slot, dmg) &&
      uuid.test(record.notarization?.appSubmissionId) &&
      uuid.test(record.notarization?.dmgSubmissionId),
    'Candidate receipt or source commit mismatch.',
  );
  for (const file of [app, dmg, manifest])
    requireThat((await realpath(file)) === file, 'Candidate output contains a path alias.');
  requireThat(
    (await digestFile(manifest)) === record.manifestSha256 && (await digestFile(dmg)) === record.dmgSha256,
    'Candidate disk image or manifest digest mismatch.',
  );
  await checkApp(app, profile, policy, record.manifestSha256, record.sourceCommit);
  await execute('/usr/bin/codesign', [
    '--verify',
    '--strict',
    '-R',
    '=anchor apple generic and certificate leaf[subject.OU] = "' +
      policy.expectedTeamId +
      '" and identifier "' +
      profile.identifier +
      '.dmg"',
    dmg,
  ]);
  await execute('/usr/bin/xcrun', ['stapler', 'validate', dmg]);
  await execute('/usr/sbin/spctl', ['--assess', '--type', 'open', '--context', 'context:primary-signature', dmg], {
    timeout: 120_000,
  });
  const mount = await mkdtemp(path.join(await realpath(tmpdir()), 'panda-drawer-verify-'));
  try {
    await execute(
      '/usr/bin/hdiutil',
      ['attach', dmg, '-readonly', '-nobrowse', '-noautoopen', '-mountpoint', mount],
      { timeout: 120_000 },
    );
    try {
      const imageApp = path.join(mount, path.basename(app));
      // A matching CodeDirectory seals the code and the resource envelope.
      const cdhash = async (value) =>
        (await execute('/usr/bin/codesign', ['-dvvv', value])).stderr.match(/^CDHash=([\da-f]+)$/m)?.[1];
      const expected = await cdhash(app);
      requireThat(expected && (await cdhash(imageApp)) === expected, 'DMG contains a different app.');
      await execute('/usr/bin/codesign', ['--verify', '--deep', '--strict', imageApp]);
      await execute('/usr/bin/xcrun', ['stapler', 'validate', imageApp]);
    } finally {
      await execute('/usr/bin/hdiutil', ['detach', mount]);
    }
  } finally {
    await rmdir(mount);
  }
  return {
    verified: true,
    version: record.version,
    sourceCommit: record.sourceCommit,
    dmgSha256: record.dmgSha256,
    app,
    dmg,
  };
}
async function finish(slot, profile, policy) {
  const { app, dmg, receipts, manifest } = productPaths(slot, profile);
  const { sourceCommit } = await json(path.join(receipts, 'build-start.json'));
  requireThat(sourceCommit === (await commit()), 'Source no longer matches the candidate.');
  const identity = await signingIdentity(policy.expectedTeamId);
  const verified = path.join(receipts, 'candidate.json');
  if (await exists(verified)) return verifyArtifacts(slot, profile, policy, await json(verified));
  const archive = path.join(slot, 'notarization.zip');
  await verifyCode(app, profile.identifier, policy.expectedTeamId);
  if (!(await exists(archive))) {
    const pending = path.join(slot, 'pending-' + randomUUID() + '.zip');
    try {
      await execute('/usr/bin/ditto', ['-c', '-k', '--keepParent', app, pending], { timeout: 300_000 });
      await rename(pending, archive);
    } finally {
      await rm(pending, { force: true });
    }
  }
  const appSubmissionId = await notarize(archive, receipts, 'app');
  await execute('/usr/bin/xcrun', ['stapler', 'staple', app]);
  const manifestSha256 = await digestFile(manifest);
  await checkApp(app, profile, policy, manifestSha256, sourceCommit);
  if (!(await exists(dmg))) {
    const payload = await mkdtemp(path.join(slot, 'image-layout-'));
    try {
      await execute('/usr/bin/ditto', [app, path.join(payload, path.basename(app))]);
      await symlink('/Applications', path.join(payload, 'Applications'));
      const image = path.join(slot, 'pending-' + randomUUID() + '.dmg');
      await execute(
        '/usr/bin/hdiutil',
        [
          'create',
          '-fs',
          'HFS+',
          '-format',
          'UDZO',
          '-volname',
          profile.displayName,
          '-srcfolder',
          payload,
          image,
        ],
        { timeout: 600_000 },
      );
      await signWithTimestamp(image, identity, ['--identifier', profile.identifier + '.dmg']);
      await rename(image, dmg);
    } finally {
      await rm(payload, { recursive: true, force: true });
    }
  }
  const dmgSubmissionId = await notarize(dmg, receipts, 'dmg');
  await execute('/usr/bin/xcrun', ['stapler', 'staple', dmg]);
  const receipt = {
    schemaVersion: 1,
    kind: 'macos-production-candidate',
    sourceCommit,
    version: profile.version,
    identifier: profile.identifier,
    teamId: policy.expectedTeamId,
    app: path.relative(slot, app),
    dmg: path.relative(slot, dmg),
    dmgSha256: await digestFile(dmg),
    manifestSha256,
    notarization: { appSubmissionId, dmgSubmissionId },
  };
  const result = await verifyArtifacts(slot, profile, policy, receipt);
  const unique = path.join(receipts, 'verified-' + randomUUID() + '.json');
  await exclusive(unique, receipt);
  await rename(unique, path.join(receipts, 'candidate.json'));
  const pointer = path.join(base, '.latest-' + randomUUID());
  await exclusive(pointer, { slot: path.basename(slot) });
  await rename(pointer, path.join(base, 'latest-candidate.json'));
  // Only the exact, freshly allocated candidate paths participate in cleanup.
  // Keep the sealed app, DMG, manifest, preflight and notarization receipts.
  for (const relative of [
    'cargo-target',
    'notarization.zip',
    'stage/binaries',
    'stage/resources',
    'stage/source-tools',
    'stage/frontend-public',
    'stage/frontend',
  ]) {
    await rm(path.join(slot, relative), { recursive: true, force: true });
  }
  return result;
}
async function exists(file) {
  return lstat(file).then(
    () => true,
    (error) => {
      if (error.code === 'ENOENT') return false;
      throw error;
    },
  );
}
async function main(args) {
  requireThat(
    process.platform === 'darwin' && process.arch === 'arm64',
    'Candidate builds require an Apple Silicon Mac.',
  );
  const profile = await json(path.join(profiles, 'production.json')),
    policy = await json(path.join(profiles, 'macos-signing-policy.json'));
  if (!args.length) return finish(await build(profile, policy), profile, policy);
  let [mode, slot] = args;
  if (mode === 'verify-latest' && args.length === 1) {
    const pointer = path.join(base, 'latest-candidate.json');
    requireThat(await exists(pointer), 'No verified macOS candidate exists for this checkout.');
    requireThat((await realpath(pointer)) === pointer, 'Invalid candidate pointer.');
    slot = path.join(base, (await json(pointer)).slot);
  } else
    requireThat(
      ['verify', 'finish'].includes(mode) && args.length === 2,
      'Use no arguments, verify-latest, verify SLOT or finish SLOT.',
    );
  await allowedSlot(slot);
  if (mode === 'finish') return finish(slot, profile, policy);
  const file = path.join(slot, 'receipts/candidate.json');
  requireThat((await realpath(file)) === file, 'Invalid candidate receipt location.');
  return verifyArtifacts(slot, profile, policy, await json(file));
}

if (fileURLToPath(import.meta.url) === process.argv[1]) {
  main(process.argv.slice(2))
    .then((result) => console.log(JSON.stringify(result)))
    .catch((error) => {
      console.error(error.stderr?.trim() || error.message);
      process.exitCode = 1;
    });
}
