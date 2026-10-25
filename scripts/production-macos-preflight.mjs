// Metadata-only production planning. No subprocess, network, credentials,
// runtime initialization, signing, bundling or publishing belongs in this entry.
import { constants } from 'node:fs';
import { lstat, mkdir, open, realpath } from 'node:fs/promises';
import { createHash, randomUUID } from 'node:crypto';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const repository = fileURLToPath(new URL('../', import.meta.url));
const profileFields = {
  schemaVersion: 1, profile: 'production', version: '0.1.0',
  identifier: 'com.happypandaa.pandadrawer', environment: 'production',
  displayName: '熊猫小抽屉', distribution: 'production-candidate',
  releaseRepository: 'https://github.com/happypandaa/panda-drawer-releases',
  target: 'aarch64-apple-darwin', minimumSystemVersion: '15.0', resourceAnchors: 'unconfigured',
};
const inputPaths = {
  profile: 'build-profiles/production.json',
  signingPolicy: 'build-profiles/macos-signing-policy.json',
  developmentConfiguration: 'apps/desktop/src-tauri/tauri.conf.json',
  nodeRuntime: 'scripts/node-runtime-release.json',
  developmentNode: 'scripts/development-node-release.json',
  developmentGit: 'scripts/development-git-release.json',
};
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const fail = message => { throw new Error(message); };
const object = value => value !== null && typeof value === 'object' && !Array.isArray(value);
function fields(value, expected, label) {
  if (!object(value) || Object.keys(value).sort().join('\0') !== [...expected].sort().join('\0')) fail(`${label} 字段不符合固定协议。`);
}

// JSON.parse validates grammar; this bounded second pass also rejects duplicate
// object keys (including differently escaped spellings) and excessive nesting.
export function strictJson(bytes) {
  if (bytes.length > 256 * 1024) fail('配置文件超过大小限制。');
  const text = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
  const parsed = JSON.parse(text);
  let offset = 0;
  const whitespace = () => { while (/\s/.test(text[offset] ?? '') && offset < text.length) offset++; };
  function string() {
    const start = offset++;
    while (offset < text.length) {
      if (text[offset] === '\\') offset += 2;
      else if (text[offset++] === '"') break;
    }
    return JSON.parse(text.slice(start, offset));
  }
  function value(depth) {
    if (depth > 64) fail('配置嵌套过深。');
    whitespace();
    const first = text[offset++];
    if (first === '{') {
      const seen = new Set();
      whitespace();
      if (text[offset] === '}') { offset++; return; }
      do {
        whitespace();
        const key = string();
        if (seen.has(key)) fail('配置含重复 JSON 字段。');
        seen.add(key);
        whitespace(); offset++; // colon, already checked by JSON.parse
        value(depth + 1); whitespace();
      } while (text[offset++] === ',');
    } else if (first === '[') {
      whitespace();
      if (text[offset] === ']') { offset++; return; }
      do { value(depth + 1); whitespace(); } while (text[offset++] === ',');
    } else if (first === '"') { offset--; string(); }
    else { while (offset < text.length && !/[\s,}\]]/.test(text[offset])) offset++; }
  }
  value(0);
  return parsed;
}

export function validateInputs(inputs) {
  const profile = strictJson(inputs.profile);
  fields(profile, Object.keys(profileFields), '生产配置');
  for (const [key, expected] of Object.entries(profileFields)) if (profile[key] !== expected) fail(`生产配置 ${key} 与已确认首版不符。`);
  const policy = strictJson(inputs.signingPolicy);
  fields(policy, ['schemaVersion', 'signingProfile', 'expectedTeamId', 'nodeEntitlementsSha256', 'gitEntitlementsSha256'], '签名策略');
  if (policy.schemaVersion !== 1 || policy.signingProfile !== 'macos-developer-id-hardened-runtime-v1') fail('签名策略版本不受支持。');
  if (policy.expectedTeamId !== null && (typeof policy.expectedTeamId !== 'string' || !/^[A-Z0-9]{10}$/.test(policy.expectedTeamId))) fail('签名策略 Team ID 格式无效。');
  for (const key of ['nodeEntitlementsSha256', 'gitEntitlementsSha256']) {
    if (policy[key] !== null && (typeof policy[key] !== 'string' || !/^[a-f0-9]{64}$/.test(policy[key]))) fail(`签名策略 ${key} 格式无效。`);
  }
  const base = strictJson(inputs.developmentConfiguration);
  fields(base, ['$schema', 'productName', 'version', 'identifier', 'build', 'app', 'bundle'], '开发 Tauri 配置');
  if (base.$schema !== 'https://schema.tauri.app/config/2' || base.productName !== profile.displayName || base.identifier !== 'dev.localprojects.platform.m0' || base.version !== '0.0.0') fail('开发 Tauri 固定身份或版本发生变化。');
  fields(base.build, ['beforeDevCommand', 'devUrl', 'beforeBuildCommand', 'frontendDist'], '开发构建配置');
  if (base.build.beforeDevCommand !== 'npm run dev' || base.build.beforeBuildCommand !== 'npm run build' || base.build.frontendDist !== '../dist' || base.build.devUrl !== 'http://127.0.0.1:1420') fail('开发构建入口发生变化，须先审查隔离规则。');
  fields(base.app, ['windows', 'security'], '开发 app 配置');
  fields(base.app.security, ['csp', 'devCsp'], '开发安全配置');
  if (typeof base.app.security.csp !== 'string' || typeof base.app.security.devCsp !== 'string') fail('开发安全配置无效。');
  if (!Array.isArray(base.app.windows) || base.app.windows.length !== 1 || base.app.windows[0].label !== 'manager' || base.app.windows[0].create !== false) fail('管理窗口配置发生变化。');
  fields(base.app.windows[0], ['label', 'create', 'title', 'width', 'height', 'minWidth', 'minHeight', 'backgroundColor'], '管理窗口');
  if (typeof base.app.windows[0].title !== 'string' || !/^#[a-fA-F0-9]{6}$/.test(base.app.windows[0].backgroundColor)) fail('管理窗口外观无效。');
  for (const key of ['width', 'height', 'minWidth', 'minHeight']) if (!Number.isSafeInteger(base.app.windows[0][key]) || base.app.windows[0][key] < 1 || base.app.windows[0][key] > 10000) fail('管理窗口尺寸无效。');
  fields(base.bundle, ['active', 'targets', 'category', 'shortDescription', 'externalBin', 'resources', 'icon', 'macOS'], '开发打包配置');
  if (base.bundle.active !== true || JSON.stringify(base.bundle.targets) !== '["app","dmg"]' || JSON.stringify(base.bundle.externalBin) !== '["binaries/local-projects-dev"]' || JSON.stringify(base.bundle.resources) !== '{"resources/":"resources/"}') fail('开发打包资源入口发生变化，须先审查隔离规则。');
  if (base.bundle.category !== 'DeveloperTool' || typeof base.bundle.shortDescription !== 'string') fail('开发打包分类或说明发生变化。');
  if (!Array.isArray(base.bundle.icon) || base.bundle.icon.length === 0 || base.bundle.icon.some(icon => typeof icon !== 'string' || !/^icons\/[A-Za-z0-9@.-]+$/.test(icon))) fail('图标路径不是固定品牌资源。');
  fields(base.bundle.macOS, ['minimumSystemVersion', 'signingIdentity', 'entitlements'], '开发 Mac 配置');
  if (base.bundle.macOS.minimumSystemVersion !== '15.0' || base.bundle.macOS.signingIdentity !== '-' || base.bundle.macOS.entitlements !== './Entitlements.plist') fail('开发 Mac 身份或权限入口发生变化。');
  const sources = {};
  for (const [key, versionKey] of [['nodeRuntime', 'version'], ['developmentNode', 'nodeVersion'], ['developmentGit', 'gitVersion']]) {
    const source = strictJson(inputs[key]);
    if (!object(source) || source.schemaVersion !== 1 || source.platform !== 'darwin' || source.arch !== 'arm64' || typeof source[versionKey] !== 'string' || !/^\d+\.\d+\.\d+$/.test(source[versionKey])) fail(`${key} 来源元数据与目标不符。`);
    sources[key] = { sha256: digest(inputs[key]), version: source[versionKey] };
  }
  return { profile, policy, base, sources };
}

function safePath(value) {
  if (!path.isAbsolute(value) || /[\x00-\x1f\x7f\\]/.test(value)) fail('预检路径必须是无控制字符的本机绝对路径。');
}
const same = (a, b) => ['dev', 'ino', 'size', 'mtimeNs', 'ctimeNs', 'mode', 'nlink'].every(key => a[key] === b[key]);
async function directoryChain(root, directory) {
  safePath(root); safePath(directory);
  if (directory !== root && !directory.startsWith(`${root}${path.sep}`)) fail('配置路径越出固定工程。');
  const found = [];
  let cursor = root;
  for (const part of ['', ...path.relative(root, directory).split(path.sep).filter(Boolean)]) {
    if (part) cursor = path.join(cursor, part);
    const info = await lstat(cursor, { bigint: true });
    if (!info.isDirectory() || info.isSymbolicLink()) fail('配置或输出祖先不是普通目录。');
    found.push([cursor, info]);
  }
  return found;
}
async function unchanged(entries) {
  for (const [file, before] of entries) if (!same(before, await lstat(file, { bigint: true }))) fail('预检期间配置路径发生变化。');
}
async function readInput(root, relative) {
  const file = path.join(root, relative);
  const parents = await directoryChain(root, path.dirname(file));
  const before = await lstat(file, { bigint: true });
  if (!before.isFile() || before.isSymbolicLink() || before.nlink !== 1n || before.size > 256n * 1024n) fail('配置必须是有界、无链接的普通文件。');
  const handle = await open(file, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    if (!same(before, await handle.stat({ bigint: true }))) fail('配置打开前发生变化。');
    const buffer = Buffer.alloc(256 * 1024 + 1);
    let length = 0;
    while (length < buffer.length) {
      const chunk = await handle.read(buffer, length, buffer.length - length);
      if (chunk.bytesRead === 0) break;
      length += chunk.bytesRead;
    }
    if (length > 256 * 1024) fail('配置读取超过大小限制。');
    const bytes = buffer.subarray(0, length);
    if (!same(before, await handle.stat({ bigint: true }))) fail('配置读取时发生变化。');
    await unchanged([...parents, [file, before]]);
    return { bytes, entries: [...parents, [file, before]] };
  } finally { await handle.close(); }
}
async function absent(file) {
  try { await lstat(file); } catch (error) { if (error.code === 'ENOENT') return; throw error; }
  fail('发现额外 Tauri 配置，须先审查合并顺序。');
}
function rejectOverrides(environment) {
  for (const key of ['TAURI_CONFIG', 'CARGO_TARGET_DIR', 'CARGO_BUILD_TARGET', 'PANDA_DRAWER_COMPILED_TOOL_MANIFEST', 'PANDA_DRAWER_COMPILED_TOOL_ROOT']) {
    if (Object.hasOwn(environment, key)) fail(`预检不接受 ${key} 覆盖；未读取其内容。`);
  }
  if (Object.hasOwn(environment, 'MACOSX_DEPLOYMENT_TARGET') && environment.MACOSX_DEPLOYMENT_TARGET !== '15.0') fail('预检不接受其他最低系统版本。');
}
function planFor(root, validated, inputs, mode) {
  const { profile, policy, base, sources } = validated;
  const output = path.join(root, 'target', 'production-macos', `preflight-${randomUUID()}`);
  const stage = path.join(output, 'stage');
  const layout = {
    output, cargoTarget: path.join(output, 'cargo-target'), stage,
    resources: path.join(stage, 'resources'), sourceTools: path.join(stage, 'source-tools'),
    frontend: path.join(stage, 'frontend'), sidecarDirectory: path.join(stage, 'binaries'),
    sidecarBase: path.join(stage, 'binaries', 'local-projects-dev'),
    sidecarTarget: path.join(stage, 'binaries', `local-projects-dev-${profile.target}`),
    signingDirectory: path.join(stage, 'signing'), receipts: path.join(output, 'receipts'),
    compiledToolManifest: path.join(stage, 'signed-tool-manifest.json'),
    tauriOverride: path.join(output, 'tauri.production-plan.json'), report: path.join(output, 'preflight.json'),
  };
  const tauriOverride = {
    productName: profile.displayName, version: profile.version, identifier: profile.identifier,
    build: { beforeDevCommand: null, beforeBuildCommand: null, beforeBundleCommand: null, devUrl: null, frontendDist: layout.frontend, features: ['production-resource-anchors'] },
    app: { windows: base.app.windows.map(window => ({ ...window, title: profile.displayName })) },
    bundle: {
      active: true, targets: ['app', 'dmg'], createUpdaterArtifacts: false,
      shortDescription: '熊猫小抽屉：集中收纳和打开本地应用',
      externalBin: [layout.sidecarBase],
      // RFC 7396 removes the inherited development mapping before adding stage.
      resources: { 'resources/': null, [`${layout.resources}/`]: 'resources/' },
      icon: base.bundle.icon.map(icon => path.join(root, 'apps/desktop/src-tauri', icon)),
      macOS: { minimumSystemVersion: profile.minimumSystemVersion, bundleVersion: profile.version, signingIdentity: null, hardenedRuntime: true, entitlements: path.join(layout.signingDirectory, 'main.entitlements.plist') },
    },
  };
  const missingRequirements = [];
  for (const [key, code, message] of [
    ['expectedTeamId', 'SIGNING_TEAM_UNCONFIGURED', '待账号持有人提供公开 Team ID；未检查本机证书。'],
    ['nodeEntitlementsSha256', 'NODE_ENTITLEMENTS_UNCONFIGURED', '待完成 Node 正式权限配置及摘要绑定。'],
    ['gitEntitlementsSha256', 'GIT_ENTITLEMENTS_UNCONFIGURED', '待完成 Git 正式权限配置及摘要绑定。'],
  ]) if (policy[key] === null) missingRequirements.push({ code, message });
  missingRequirements.push(
    { code: 'SIGNED_STAGE_TRANSACTION_UNIMPLEMENTED', message: '独占来源复制、真实签名转换、验签及清单生成事务尚未实现。' },
    { code: 'PRODUCTION_BUILD_TRANSACTION_UNIMPLEMENTED', message: '本入口不填充 stage、不编译 CLI/桌面，也不生成应用或 DMG。' },
    { code: 'PRODUCTION_RUNTIME_ADMISSION_UNIMPLEMENTED', message: '生产运行门禁保持拒绝，不初始化或收养任何工作区。' },
    { code: 'NOTARIZATION_AND_INSTALL_UNVERIFIED', message: '正式签名、公证、票据、最低系统及实际安装仍待验收。' },
    { code: 'UPDATER_TRUST_UNCONFIGURED', message: '更新公钥与签名产物尚未配置；不生成密钥，更新产物生成保持关闭。' },
    { code: 'UPDATER_HANDOFF_UNIMPLEMENTED', message: '安装 helper、管理窗最终销毁及旧主进程退出交接尚未实现；ReadyForNativeExit 不授予安装许可。' },
  );
  return {
    schemaVersion: 1, kind: 'production-macos-metadata-preflight', mode,
    metadataValidated: true, outputWritten: mode === 'prepare',
    productionBuildAvailable: false, runtimeAdmission: false, releaseReady: false,
    profile: { ...profile, sha256: digest(inputs.profile) },
    signingPolicy: { sha256: digest(inputs.signingPolicy), configured: ![policy.expectedTeamId, policy.nodeEntitlementsSha256, policy.gitEntitlementsSha256].includes(null), evidenceVerified: false },
    developmentConfigurationSha256: digest(inputs.developmentConfiguration), sources, layout,
    plannedBuildEnvironment: { CARGO_TARGET_DIR: layout.cargoTarget, MACOSX_DEPLOYMENT_TARGET: '15.0', PANDA_DRAWER_COMPILED_TOOL_ROOT: layout.resources, PANDA_DRAWER_COMPILED_TOOL_MANIFEST: layout.compiledToolManifest },
    tauriOverride, missingRequirements,
  };
}
async function ensureDirectory(root, directory, { privateDirectory = false } = {}) {
  await directoryChain(root, path.dirname(directory));
  try { await mkdir(directory, { mode: 0o700 }); } catch (error) { if (error.code !== 'EEXIST') throw error; }
  const info = await lstat(directory);
  if (!info.isDirectory() || info.isSymbolicLink() || (process.getuid && info.uid !== process.getuid()) || (privateDirectory && (info.mode & 0o077) !== 0)) fail('预检输出必须是本用户拥有的普通私有目录。');
}
async function writeExclusive(file, value) {
  const handle = await open(file, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600);
  try { await handle.writeFile(`${JSON.stringify(value, null, 2)}\n`); await handle.sync(); } finally { await handle.close(); }
}
async function prepare(root, report) {
  await ensureDirectory(root, path.join(root, 'target'));
  await ensureDirectory(root, path.join(root, 'target', 'production-macos'), { privateDirectory: true });
  await mkdir(report.layout.output, { mode: 0o700 }); // exclusive; never overwrite or remove an earlier slot
  for (const directory of [report.layout.cargoTarget, report.layout.stage, report.layout.resources, report.layout.sourceTools, report.layout.frontend, report.layout.sidecarDirectory, report.layout.signingDirectory, report.layout.receipts]) await mkdir(directory, { mode: 0o700 });
  // Failed writes retain this exclusive slot for inspection; there is no cleanup
  // scan, recursive deletion, overwrite, or resume of an existing transaction.
  await writeExclusive(report.layout.tauriOverride, report.tauriOverride);
  await writeExclusive(report.layout.report, report);
}

async function main() {
  const args = process.argv.slice(2);
  if (args.length === 1 && ['--help', '-h'].includes(args[0])) {
    console.log('用法：node scripts/production-macos-preflight.mjs [check|prepare]\ncheck 只读检查固定发行元数据（默认）；prepare 在独占目录生成计划和 Tauri 覆盖配置。不会构建、签名、公证、安装或发布。');
    return;
  }
  if (args.length > 1 || (args.length === 1 && !['check', 'prepare'].includes(args[0]))) fail('只接受 check 或 prepare；不接受身份、根目录、输出或配置覆盖。');
  const mode = args[0] ?? 'check';
  rejectOverrides(process.env);
  const root = await realpath(repository);
  safePath(root);
  for (const name of ['tauri.macos.conf.json', 'tauri.macos.conf.json5', 'Tauri.macos.toml', 'tauri.conf.json5', 'Tauri.toml']) await absent(path.join(root, 'apps/desktop/src-tauri', name));
  const inputs = {}, entries = [];
  for (const [key, relative] of Object.entries(inputPaths)) {
    const input = await readInput(root, relative);
    inputs[key] = input.bytes; entries.push(...input.entries);
  }
  const report = planFor(root, validateInputs(inputs), inputs, mode);
  await unchanged(entries);
  if (mode === 'prepare') await prepare(root, report);
  console.log(JSON.stringify(report, null, 2));
}

const invoked = process.argv[1] ? await realpath(process.argv[1]).catch(() => null) : null;
if (invoked === fileURLToPath(import.meta.url)) {
  main().catch(error => { console.error(`Mac 正式发行预检未完成：${error.message}`); process.exitCode = 1; });
}
