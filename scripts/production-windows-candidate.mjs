// Local Windows production candidate only: no tags, Release or OSS writes.
import { execFile } from 'node:child_process';
import { constants, createReadStream } from 'node:fs';
import { copyFile, lstat, mkdir, readFile, realpath, rename, writeFile } from 'node:fs/promises';
import { createHash, randomUUID } from 'node:crypto';
import { promisify } from 'node:util';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { root as repository, run, rustEnvironment } from './process.mjs';
import { strictJson } from './production-macos-preflight.mjs';
import { prepareProductionAppResources } from './production-macos-resources.mjs';
import { parseUpdatePublicKey } from './update-minisign.mjs';
import { generateStaticManifest } from './update-manifest-protocol.mjs';
import { verifyUpdateArtifact } from './update-artifact-verification.mjs';
import { resolvePnpmCommand } from './pnpm-command.mjs';
import { windowsCliBuildEnvironment } from './windows-cli-build.mjs';
import { desktopInvocation } from './desktop-host.mjs';
import { createWindowsToolManifest, prepareWindowsProductionTools, verifyWindowsProductionTools, sha256 } from './production-windows-resources.mjs';

const execute = promisify(execFile);
const root = path.resolve(repository);
const desktop = path.join(root, 'apps/desktop');
const base = path.join(root, 'target', 'production-windows');
const requiredSource = '33b9ff8546f2546cb7a901ccd65c4599c3481d65';
const profileFields = Object.freeze({
  schemaVersion: 1, profile: 'production', version: '0.1.0', identifier: 'com.happypandaa.pandadrawer',
  environment: 'production', displayName: '熊猫小抽屉', distribution: 'production-candidate',
  releaseRepository: 'https://github.com/happypandaa/panda-drawer', target: 'x86_64-pc-windows-msvc',
  minimumSystemVersion: '10.0.19045', resourceAnchors: 'unconfigured',
});
const inputPaths = Object.freeze({
  profile: 'build-profiles/production.windows-x64.json', updaterPublicKey: 'build-profiles/production-updater.pub',
  sources: 'distribution/download-sources.json', packageManifest: 'package.json',
  baseConfiguration: 'apps/desktop/src-tauri/tauri.conf.json', windowsConfiguration: 'apps/desktop/src-tauri/tauri.windows.conf.json',
  nodeRuntime: 'scripts/node-runtime-release.windows-x64.json', developmentNode: 'scripts/development-node-release.windows-x64.json',
  developmentGit: 'scripts/development-git-release.windows-x64.json',
});
const requireThat = (value, message) => { if (!value) throw new Error(message); };
const json = async file => strictJson(await readFile(file));
const uniqueJson = (file, value) => writeFile(file, JSON.stringify(value, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
const same = (left, right) => ['dev', 'ino', 'size', 'mtimeNs', 'ctimeNs', 'nlink'].every(key => left[key] === right[key]);

export function validateWindowsProductionInputs(inputs) {
  const profile = strictJson(inputs.profile);
  requireThat(Object.keys(profile).length === Object.keys(profileFields).length
    && Object.entries(profileFields).every(([key, value]) => profile[key] === value), 'Windows production profile differs from the fixed 0.1.0 identity.');
  const sources = strictJson(inputs.sources);
  requireThat(sources.schemaVersion === 1 && sources.releaseRepository === profile.releaseRepository
    && sources.updates?.primary === 'github'
    && Object.keys(sources.updates.sources ?? {}).sort().join(',') === 'github,oss'
    && sources.updates.sources?.github?.manifestUrl === `${profile.releaseRepository}/releases/latest/download/latest.json`
    && sources.updates.sources.github.artifactBaseUrl === `${profile.releaseRepository}/releases/download`
    && sources.updates.sources?.oss?.manifestUrl === 'https://panda-updates.zhimibuhui.com/latest.json'
    && sources.updates.sources.oss.artifactBaseUrl === 'https://panda-updates.zhimibuhui.com'
    && sources.routes?.global?.updates === 'github' && sources.routes?.cn?.updates === 'oss',
  'Production GitHub/OSS update routes are not the fixed public channels.');
  const publicKey = new TextDecoder('utf-8', { fatal: true }).decode(inputs.updaterPublicKey).replace(/\r?\n$/, '');
  requireThat(publicKey.length <= 512, 'Production public key exceeds the client limit.');
  const parsedKey = parseUpdatePublicKey(publicKey);
  const baseConfiguration = strictJson(inputs.baseConfiguration);
  const windowsConfiguration = strictJson(inputs.windowsConfiguration);
  requireThat(baseConfiguration.productName === '熊猫小抽屉' && baseConfiguration.identifier === 'dev.happypandaa.pandadrawer'
    && baseConfiguration.version === '0.0.0' && baseConfiguration.app?.windows?.length === 1
    && baseConfiguration.app.windows[0].label === 'manager' && baseConfiguration.app.windows[0].create === false,
  'The isolated development baseline changed; review production overrides first.');
  requireThat(windowsConfiguration.bundle?.windows?.webviewInstallMode?.type === 'skip'
    && windowsConfiguration.bundle.windows.nsis?.installMode === 'currentUser'
    && windowsConfiguration.bundle.windows.nsis.installerHooks === 'windows-webview2.nsh',
  'The Windows WebView2 auto-setup hook must remain configured.');
  const anchors = {};
  for (const [key, version] of [['nodeRuntime', 'version'], ['developmentNode', 'nodeVersion'], ['developmentGit', 'gitVersion']]) {
    const source = strictJson(inputs[key]);
    requireThat(source.schemaVersion === 1 && source.platform === 'win32' && source.arch === 'x64'
      && typeof source[version] === 'string' && source[version].length > 0, `Invalid Windows ${key} anchor.`);
    anchors[key] = { sha256: sha256(inputs[key]), version: source[version] };
  }
  const scripts = strictJson(inputs.packageManifest).scripts;
  return { profile, baseConfiguration, windowsConfiguration, sources, anchors, publicKey,
    publicKeyPacketSha256: parsedKey.sha256,
    candidateRegistered: scripts?.['release:windows:candidate'] === 'node scripts/production-windows-candidate.mjs' };
}

export function rejectProductionOverrides(environment) {
  for (const key of ['TAURI_CONFIG', 'CARGO_TARGET_DIR', 'CARGO_BUILD_TARGET', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTFLAGS',
    'PANDA_DRAWER_COMPILED_TOOL_MANIFEST', 'PANDA_DRAWER_COMPILED_TOOL_ROOT', 'PANDA_DRAWER_COMPILED_CLI_SHA256',
    'PANDA_DRAWER_UPDATE_PUBLIC_KEY', 'PANDA_DRAWER_UPDATE_TEST_VERSION', 'PANDA_DRAWER_UPDATE_TEST_RELEASE_TAG']) {
    requireThat(!Object.hasOwn(environment, key), `Production candidate rejects external ${key}; its value was not inspected.`);
  }
}

export function createWindowsCandidatePlan(repositoryRoot, validated, id = randomUUID()) {
  requireThat(path.isAbsolute(repositoryRoot) && /^[\da-f]{8}(?:-[\da-f]{4}){3}-[\da-f]{12}$/i.test(id), 'Invalid candidate root or slot ID.');
  const slot = path.join(repositoryRoot, 'target', 'production-windows', `candidate-${id}`);
  const stage = path.join(slot, 'stage');
  const layout = {
    slot, stage, receipts: path.join(slot, 'receipts'), candidate: path.join(slot, 'candidate'),
    resources: path.join(stage, 'resources'), frontend: path.join(stage, 'frontend'), public: path.join(stage, 'frontend-public'),
    binaries: path.join(stage, 'binaries'), manifest: path.join(stage, 'windows-tool-manifest.json'),
    cargoTarget: path.join(repositoryRoot, 'target', 'production-windows', 'cargo-target'),
    configuration: path.join(stage, 'tauri.production-windows.json'),
    executable: path.join(slot, 'candidate', `PandaDrawer_${validated.profile.version}_x64-setup.exe`),
  };
  const configuration = {
    // An ASCII product name keeps the installation directory distinct from
    // the existing development application's Chinese product directory.
    productName: 'PandaDrawer', mainBinaryName: 'PandaDrawer', version: validated.profile.version,
    identifier: validated.profile.identifier,
    build: { beforeDevCommand: null, beforeBuildCommand: null, beforeBundleCommand: null,
      devUrl: null, frontendDist: layout.frontend, features: ['production-resource-anchors'] },
    app: { windows: validated.baseConfiguration.app.windows.map(window => ({ ...window, title: validated.profile.displayName })) },
    bundle: {
      active: true, targets: ['nsis'], createUpdaterArtifacts: false,
      shortDescription: '熊猫小抽屉：集中收纳和打开本地应用',
      resources: { 'resources/': null, [`${layout.resources}${path.sep}`]: 'resources/' },
      externalBin: [path.join(layout.binaries, 'local-projects-dev')],
      icon: [path.join(repositoryRoot, 'apps/desktop/src-tauri/icons/icon.ico')],
      windows: { webviewInstallMode: { type: 'skip' },
        nsis: { installMode: 'currentUser', installerHooks: path.join(repositoryRoot, 'apps/desktop/src-tauri/windows-webview2.nsh') } },
    },
  };
  return { schemaVersion: 1, kind: 'windows-production-plan', profile: validated.profile, layout, configuration };
}

async function readInputs() {
  const result = {};
  for (const [key, relative] of Object.entries(inputPaths)) {
    const file = path.join(root, relative);
    const info = await lstat(file);
    requireThat(info.isFile() && !info.isSymbolicLink() && info.nlink === 1 && info.size <= 256 * 1024,
      `Production input must be a bounded ordinary file: ${relative}`);
    result[key] = await readFile(file);
  }
  return result;
}
async function sourceStatus() {
  const options = { cwd: root, windowsHide: true, maxBuffer: 4 * 1024 * 1024 };
  const sourceCommit = (await execute('git', ['rev-parse', 'HEAD'], options)).stdout.trim();
  const dirty = (await execute('git', ['status', '--porcelain', '--untracked-files=normal'], options)).stdout.trim();
  await execute('git', ['merge-base', '--is-ancestor', requiredSource, sourceCommit], options);
  requireThat(/^[\da-f]{40}$/.test(sourceCommit), 'Git did not return a complete source SHA.');
  return { sourceCommit, clean: dirty.length === 0 };
}
async function committed() {
  const state = await sourceStatus();
  requireThat(state.clean, 'Production candidate requires a clean committed source tree.');
  return state.sourceCommit;
}
function host() {
  requireThat(process.platform === 'win32' && process.arch === 'x64', 'Production NSIS builds require Windows x64.');
}
function buildEnvironment() {
  const safe = {};
  for (const [key, value] of Object.entries(process.env)) {
    if (/^(path|systemroot|windir|temp|tmp|userprofile|appdata|localappdata|programfiles|programfiles\(x86\)|programdata|comspec|pathext|include|lib|libpath|vcinstalldir|vsinstalldir|vctoolsinstalldir|windowssdkdir|windowssdkversion|universalcrtsdkdir|ucrtversion|vscmd_arg_tgt_arch|lang|lc_all)$/i.test(key)) safe[key] = value;
  }
  return { ...rustEnvironment({ environment: safe }), CARGO_BUILD_JOBS: '2',
    CARGO_PROFILE_DEV_DEBUG: '0', CARGO_PROFILE_TEST_DEBUG: '0', CARGO_INCREMENTAL: '0' };
}
async function fileReceipt(file) {
  const before = await lstat(file, { bigint: true });
  requireThat(before.isFile() && !before.isSymbolicLink() && before.nlink === 1n
    && before.size > 0n && before.size <= 1024n * 1024n * 1024n, 'Artifact must be one bounded ordinary file.');
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(file)) hash.update(chunk);
  requireThat(same(before, await lstat(file, { bigint: true })), 'Artifact changed while hashing.');
  return { name: path.basename(file), bytes: Number(before.size), sha256: hash.digest('hex') };
}
async function frontendResources(layout, environment) {
  await mkdir(layout.public);
  const { stdout } = await execute('git', ['ls-tree', '-r', '-z', '--full-tree', 'HEAD', '--', 'apps/desktop/public'],
    { cwd: root, windowsHide: true, encoding: 'buffer', maxBuffer: 4 * 1024 * 1024 });
  for (const row of stdout.toString('utf8').split('\0').filter(Boolean)) {
    const separator = row.indexOf('\t');
    const [mode, kind, object] = row.slice(0, separator).split(' ');
    const relative = row.slice(separator + 1).slice('apps/desktop/public/'.length);
    requireThat(kind === 'blob' && ['100644', '100755'].includes(mode)
      && relative.split('/').every(part => part && part !== '.' && part !== '..' && !/[\x00-\x1f\x7f\\:*?"<>|]/.test(part)),
    'Frontend release resources must be ordinary committed files.');
    const target = path.join(layout.public, ...relative.split('/'));
    await mkdir(path.dirname(target), { recursive: true });
    const blob = await execute('git', ['cat-file', 'blob', object],
      { cwd: root, windowsHide: true, encoding: 'buffer', maxBuffer: 64 * 1024 * 1024 });
    await writeFile(target, blob.stdout, { flag: 'wx' });
  }
  const pnpm = await resolvePnpmCommand({});
  await run(pnpm.executable, [...pnpm.prefix, 'exec', 'tsc', '-b'], { cwd: desktop, env: environment, windowsHide: true });
  const { build } = await import(pathToFileURL(path.join(desktop, 'node_modules/vite/dist/node/index.js')).href);
  await build({ root: desktop, publicDir: layout.public, envDir: false, envPrefix: [],
    build: { outDir: layout.frontend, emptyOutDir: false } });
}
async function publisherSignature(file) {
  const shell = path.join(process.env.SystemRoot ?? process.env.WINDIR ?? 'C:\\Windows', 'System32/WindowsPowerShell/v1.0/powershell.exe');
  const command = '$s=Get-AuthenticodeSignature -LiteralPath $env:PANDA_PRODUCTION_AUTHENTICODE_FILE; $subject=$null; $thumb=$null; if($s.SignerCertificate){$subject=$s.SignerCertificate.Subject;$thumb=$s.SignerCertificate.Thumbprint}; [pscustomobject]@{status=[string]$s.Status;publisher=$subject;thumbprint=$thumb;timestamped=($null -ne $s.TimeStamperCertificate)}|ConvertTo-Json -Compress';
  try {
    const result = await execute(shell, ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command', command],
      { env: { ...buildEnvironment(), PANDA_PRODUCTION_AUTHENTICODE_FILE: file }, windowsHide: true, timeout: 30_000, maxBuffer: 32 * 1024 });
    return JSON.parse(result.stdout.replace(/^\uFEFF/, '').trim());
  } catch { return { status: 'NotInspected', publisher: null, thumbprint: null, timestamped: false }; }
}
export function productionSigningState(environment = process.env) {
  const material = ['TAURI_SIGNING_PRIVATE_KEY', 'TAURI_SIGNING_PRIVATE_KEY_PATH'].filter(key => Boolean(environment[key]));
  return { keySupplied: material.length === 1, ambiguous: material.length > 1,
    passwordSupplied: Boolean(environment.TAURI_SIGNING_PRIVATE_KEY_PASSWORD) };
}
async function updateSignature(plan, validated, artifact) {
  const state = productionSigningState();
  const missing = (code, message) => ({ configured: state.keySupplied, verified: false, signature: null,
    missingRequirements: [{ code, message }] });
  if (state.ambiguous) return missing('UPDATE_SIGNING_KEY_AMBIGUOUS', 'Select one local production signing-key environment input; values were not inspected.');
  if (!state.keySupplied) return missing('UPDATE_SIGNING_PRIVATE_KEY_MISSING', 'Supply the private key matching the committed production public key; no test key is substituted. An encrypted key also requires its local password.');
  const environment = buildEnvironment();
  for (const key of ['TAURI_SIGNING_PRIVATE_KEY', 'TAURI_SIGNING_PRIVATE_KEY_PATH', 'TAURI_SIGNING_PRIVATE_KEY_PASSWORD']) {
    if (process.env[key] !== undefined) environment[key] = process.env[key];
  }
  const signatureFile = plan.layout.executable + '.sig';
  try {
    // Signer diagnostics can contain private material. Never forward stdout,
    // stderr, exceptions or environment values into receipts/build logs.
    await execute(process.execPath, [path.join(root, 'node_modules/@tauri-apps/cli/tauri.js'), 'signer', 'sign',
      ...(state.passwordSupplied ? [] : ['--password', '']), '--app-version', validated.profile.version, plan.layout.executable],
    { env: environment, windowsHide: true, timeout: 90_000, maxBuffer: 1024 * 1024 });
    const signatureInfo = await lstat(signatureFile);
    requireThat(signatureInfo.isFile() && !signatureInfo.isSymbolicLink() && signatureInfo.nlink === 1
      && signatureInfo.size > 0 && signatureInfo.size <= 8192, 'Invalid updater signature file.');
    const signature = (await readFile(signatureFile, 'utf8')).trim();
    const metadata = { schemaVersion: 1, profile: 'production', identifier: validated.profile.identifier,
      target: 'windows-x86_64', releaseRepository: validated.profile.releaseRepository, version: validated.profile.version,
      tag: `v${validated.profile.version}`, notes: 'Local production candidate; not published.',
      pub_date: new Date().toISOString(), artifact: { ...artifact, signature } };
    for (const source of ['github', 'oss']) {
      const manifest = generateStaticManifest(metadata, { source });
      await verifyUpdateArtifact({ manifest, artifact: plan.layout.executable,
        publicKeyPath: path.join(root, inputPaths.updaterPublicKey), target: 'windows-x86_64' });
      await uniqueJson(path.join(plan.layout.receipts, `unpublished-${source}-manifest.json`), manifest);
    }
    return { configured: true, verified: true, signature: path.basename(signatureFile), missingRequirements: [],
      publicKeyPacketSha256: validated.publicKeyPacketSha256 };
  } catch {
    try { await rename(signatureFile, path.join(plan.layout.receipts, `rejected-signature-${randomUUID()}.txt`)); }
    catch (error) { if (error.code !== 'ENOENT') throw new Error('Cannot quarantine the rejected candidate signature; inspect the local candidate directory.'); }
    return missing('UPDATE_SIGNATURE_NOT_VERIFIED', 'Configured local signing input did not produce a signature verifiable by the committed production key; private diagnostics were suppressed. Check the matching key and, if encrypted, password locally.');
  }
}

async function build(validated, inputs) {
  host();
  requireThat(validated.candidateRegistered, 'Register the fixed release:windows:candidate entry before building.');
  const sourceCommit = await committed();
  const plan = createWindowsCandidatePlan(root, validated);
  const { layout } = plan;
  await mkdir(base, { recursive: true });
  await mkdir(layout.slot); // exclusive slot; never overwrite earlier evidence
  for (const directory of [layout.stage, layout.receipts, layout.candidate, layout.resources, layout.binaries]) await mkdir(directory);
  await uniqueJson(path.join(layout.receipts, 'build-start.json'), { sourceCommit, requiredSource });
  await prepareWindowsProductionTools({ repository: root, resources: layout.resources });
  await prepareProductionAppResources({ root, resources: layout.resources });
  const manifest = await createWindowsToolManifest({ profileBytes: inputs.profile, sourceBytes: inputs, resources: layout.resources });
  const manifestBytes = Buffer.from(JSON.stringify(manifest, null, 2) + '\n');
  requireThat(manifestBytes.length <= 8 * 1024 * 1024, 'Compiled Windows tool manifest exceeds its budget.');
  await writeFile(layout.manifest, manifestBytes, { flag: 'wx' });
  const environment = { ...buildEnvironment(), CARGO_TARGET_DIR: layout.cargoTarget, CARGO_BUILD_TARGET: validated.profile.target,
    PANDA_DRAWER_COMPILED_TOOL_ROOT: layout.resources, PANDA_DRAWER_COMPILED_TOOL_MANIFEST: layout.manifest };
  await frontendResources(layout, environment);
  await run('cargo', ['build', '--locked', '--release', '-p', 'platform-cli', '--features', 'production-resource-anchors', '--bin', 'local-projects-dev'],
    { cwd: root, env: windowsCliBuildEnvironment(environment), windowsHide: true });
  const compiled = path.join(layout.cargoTarget, validated.profile.target, 'release');
  const compiledCli = path.join(compiled, 'local-projects-dev.exe');
  const cli = await fileReceipt(compiledCli);
  const cliMetadata = JSON.parse((await execute(compiledCli, ['build-info', '--json'],
    { env: environment, windowsHide: true, timeout: 30_000, maxBuffer: 128 * 1024 })).stdout);
  requireThat(cliMetadata.ok && cliMetadata.data.profile === 'production' && cliMetadata.data.version === validated.profile.version
    && cliMetadata.data.identifier === validated.profile.identifier && cliMetadata.data.target === validated.profile.target
    && cliMetadata.data.resourceAnchors === 'compiled-windows-tools-v1'
    && cliMetadata.data.resourceManifestSha256 === sha256(manifestBytes), 'Compiled CLI production identity/resource anchors mismatch.');
  await copyFile(compiledCli, path.join(layout.binaries, `local-projects-dev-${validated.profile.target}.exe`), constants.COPYFILE_EXCL);
  await uniqueJson(layout.configuration, plan.configuration);
  const invocation = desktopInvocation({ root, configuration: plan.configuration, targetDirectory: layout.cargoTarget,
    target: validated.profile.target, args: ['build', '--bundles', 'nsis', '--ci', '--', '--locked'] });
  await run(invocation.command, invocation.args, { cwd: desktop,
    env: { ...environment, ...invocation.environment, PANDA_DRAWER_COMPILED_CLI_SHA256: cli.sha256 }, windowsHide: true });
  const generated = path.join(compiled, 'bundle/nsis', `PandaDrawer_${validated.profile.version}_x64-setup.exe`);
  await copyFile(generated, layout.executable, constants.COPYFILE_EXCL);
  const artifact = await fileReceipt(layout.executable);
  const updater = await updateSignature(plan, validated, artifact);
  const authenticode = await publisherSignature(layout.executable);
  requireThat(await committed() === sourceCommit, 'Source changed during the production build; candidate is retained for diagnosis.');
  const receipt = { schemaVersion: 1, kind: 'windows-production-candidate', sourceCommit,
    version: validated.profile.version, identifier: validated.profile.identifier, target: validated.profile.target,
    executable: path.relative(layout.slot, layout.executable), artifact, cliSha256: cli.sha256,
    resourceManifestSha256: sha256(manifestBytes), profileSha256: sha256(inputs.profile),
    updaterPublicKeyPacketSha256: validated.publicKeyPacketSha256,
    productionRoutes: validated.sources.updates, updater, authenticode,
    installationIntegrityVerified: false, guiAcceptanceVerified: false, smartScreen: { verified: false }, releaseReady: false };
  await uniqueJson(path.join(layout.receipts, 'candidate.json'), receipt);
  await uniqueJson(path.join(layout.receipts, 'plan.json'), plan);
  const pointer = path.join(base, `.latest-${randomUUID()}.json`);
  await uniqueJson(pointer, { slot: path.basename(layout.slot) });
  await rename(pointer, path.join(base, 'latest-candidate.json'));
  return { ...receipt, candidateDirectory: layout.slot, artifactPath: layout.executable };
}

async function allowedSlot(slot) {
  requireThat(typeof slot === 'string' && path.isAbsolute(slot)
    && path.dirname(path.normalize(slot)) === base
    && /^candidate-[\da-f]{8}(?:-[\da-f]{4}){3}-[\da-f]{12}$/i.test(path.basename(slot)), 'Invalid production candidate slot.');
  const normalized = path.normalize(slot);
  requireThat(await realpath(normalized) === normalized && !(await lstat(normalized)).isSymbolicLink(), 'Candidate slot must be an ordinary canonical directory.');
  return normalized;
}
async function verify(slot, validated, inputs) {
  host();
  slot = await allowedSlot(slot);
  const record = await json(path.join(slot, 'receipts/candidate.json'));
  const plan = await json(path.join(slot, 'receipts/plan.json'));
  requireThat(record.schemaVersion === 1 && record.kind === 'windows-production-candidate'
    && record.sourceCommit === await committed() && record.version === validated.profile.version
    && record.identifier === validated.profile.identifier && record.target === validated.profile.target
    && record.profileSha256 === sha256(inputs.profile)
    && record.updaterPublicKeyPacketSha256 === validated.publicKeyPacketSha256,
  'Candidate receipt does not match the current complete production source.');
  requireThat(plan.layout.slot === slot && plan.layout.resources === path.join(slot, 'stage/resources')
    && plan.layout.manifest === path.join(slot, 'stage/windows-tool-manifest.json')
    && record.executable === path.join('candidate', `PandaDrawer_${validated.profile.version}_x64-setup.exe`), 'Candidate layout mismatch.');
  await verifyWindowsProductionTools(plan.layout.resources);
  const manifest = await createWindowsToolManifest({ profileBytes: inputs.profile, sourceBytes: inputs, resources: plan.layout.resources });
  const manifestBytes = await readFile(plan.layout.manifest);
  requireThat(sha256(manifestBytes) === record.resourceManifestSha256
    && JSON.stringify(JSON.parse(manifestBytes)) === JSON.stringify(manifest), 'Candidate resource manifest changed.');
  const artifactPath = path.join(slot, record.executable);
  requireThat(JSON.stringify(await fileReceipt(artifactPath)) === JSON.stringify(record.artifact), 'Candidate installer bytes changed.');
  const stagedCli = path.join(slot, 'stage/binaries', `local-projects-dev-${validated.profile.target}.exe`);
  requireThat((await fileReceipt(stagedCli)).sha256 === record.cliSha256, 'Candidate CLI differs from the desktop compile anchor.');
  if (record.updater.verified) {
    const signature = (await readFile(artifactPath + '.sig', 'utf8')).trim();
    const metadata = { schemaVersion: 1, profile: 'production', identifier: validated.profile.identifier,
      target: 'windows-x86_64', releaseRepository: validated.profile.releaseRepository,
      version: validated.profile.version, tag: `v${validated.profile.version}`, notes: 'Local production candidate; not published.',
      pub_date: new Date().toISOString(), artifact: { ...record.artifact, signature } };
    for (const source of ['github', 'oss']) await verifyUpdateArtifact({
      manifest: generateStaticManifest(metadata, { source }), artifact: artifactPath,
      publicKeyPath: path.join(root, inputPaths.updaterPublicKey), target: 'windows-x86_64' });
  }
  return { verified: true, scope: 'local-candidate-installer-bytes-and-tool-integrity',
    updateSignatureVerified: record.updater.verified, sourceCommit: record.sourceCommit, version: record.version,
    artifactPath, artifact: record.artifact, updater: record.updater,
    authenticode: await publisherSignature(artifactPath), installationIntegrityVerified: false,
    guiAcceptanceVerified: false, releaseReady: false };
}

export async function runWindowsCandidate(args) {
  requireThat(args.length <= 2, 'Use check, build, verify <candidate-directory>, or verify-latest.');
  if (args.length === 1 && ['--help', '-h'].includes(args[0])) return {
    help: 'Local only: check; build (default); verify <absolute candidate directory>; verify-latest. No installation, tag, Release, OSS publication or credential creation. Missing production update-signing input is recorded, never replaced by a test key.' };
  const mode = args[0] ?? 'build';
  requireThat(['check', 'build', 'verify', 'verify-latest'].includes(mode)
    && (mode === 'verify' ? args.length === 2 : args.length <= 1), 'Unsupported candidate command or arguments.');
  rejectProductionOverrides(process.env);
  const inputs = await readInputs();
  const validated = validateWindowsProductionInputs(inputs);
  if (mode === 'check') return { schemaVersion: 1, kind: 'windows-production-preflight', metadataValidated: true,
    profile: validated.profile, source: await sourceStatus(), productionRoutes: validated.sources.updates,
    updaterPublicKey: { configured: true, packetSha256: validated.publicKeyPacketSha256 },
    candidateRegistered: validated.candidateRegistered, updaterSigning: productionSigningState(),
    buildAvailable: process.platform === 'win32' && process.arch === 'x64' && validated.candidateRegistered,
    releaseReady: false, privateMaterialInspected: false };
  if (mode === 'build') return build(validated, inputs);
  const slot = mode === 'verify-latest'
    ? path.join(base, (await json(path.join(base, 'latest-candidate.json'))).slot) : args[1];
  return verify(slot, validated, inputs);
}

const invoked = process.argv[1] ? path.resolve(process.argv[1]) : null;
if (invoked === fileURLToPath(import.meta.url)) {
  runWindowsCandidate(process.argv.slice(2)).then(result => console.log(JSON.stringify(result, null, 2)))
    .catch(error => { console.error(`Windows production candidate failed: ${error.message}`); process.exitCode = 1; });
}
