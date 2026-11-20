// Transform verified upstream bytes only inside the preparer's private candidate slot.
import { createHash } from 'node:crypto';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { chmod, cp, lstat, mkdir, readFile, readdir, realpath, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { root } from './process.mjs';
import { packageNodeRuntime, NODE_RELEASE, verifyRuntimeBytes } from './package-node-runtime.mjs';
import {
  prepareDevelopmentNode,
  verifyDevelopmentNode,
  DEVELOPMENT_NODE_RELEASE,
} from './package-development-node.mjs';
import {
  prepareDevelopmentGit,
  verifyDevelopmentGit,
  DEVELOPMENT_GIT_RELEASE,
} from './package-development-git.mjs';

export const execute = promisify(execFile);
export const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');
const specifications = [
  {
    key: 'nodeRuntime',
    dir: 'node-runtime',
    anchor: 'node-runtime-release.json',
    toolVersion: NODE_RELEASE.version,
  },
  {
    key: 'developmentNode',
    dir: 'development-node',
    anchor: 'development-node-release.json',
    toolVersion: DEVELOPMENT_NODE_RELEASE.nodeVersion,
    npmVersion: DEVELOPMENT_NODE_RELEASE.npmVersion,
  },
  {
    key: 'developmentGit',
    dir: 'development-git',
    anchor: 'development-git-release.json',
    toolVersion: DEVELOPMENT_GIT_RELEASE.gitVersion,
  },
];

export async function verifyCode(file, identifier, teamId) {
  const requirement =
    'anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists' +
    ' and certificate leaf[subject.OU] = "' +
    teamId +
    '" and identifier "' +
    identifier +
    '"';
  await execute('/usr/bin/codesign', ['--verify', '--strict', '-R', '=' + requirement, file]);
  const { stderr } = await execute('/usr/bin/codesign', ['--display', '--verbose=4', file]);
  if (!/^CodeDirectory .*flags=0x[0-9a-f]+\([^)]*\bruntime\b/m.test(stderr) || !/^Timestamp=/m.test(stderr)) {
    throw new Error('Missing Hardened Runtime or secure timestamp on ' + path.basename(file));
  }
}

export async function signCode(file, identifier, identity, entitlements, teamId) {
  await execute(
    '/usr/bin/codesign',
    [
      '--force',
      '--options',
      'runtime',
      '--timestamp',
      '--sign',
      identity,
      '--identifier',
      identifier,
      ...(entitlements ? ['--entitlements', entitlements] : []),
      file,
    ],
    { timeout: 180_000 },
  );
  await verifyCode(file, identifier, teamId);
}

function equalPlist(a, b) {
  const sort = (value) =>
    value !== null && typeof value === 'object' && !Array.isArray(value)
      ? Object.fromEntries(Object.entries(value).sort(([l], [r]) => l.localeCompare(r)))
      : value;
  return JSON.stringify(a, (_, value) => sort(value)) === JSON.stringify(b, (_, value) => sort(value));
}

export async function verifyEntitlements(file, spec, scratch) {
  const { stdout } = await execute('/usr/bin/codesign', ['--display', '--entitlements', ':-', file], {
    encoding: 'buffer',
  });
  let actual = {};
  if (stdout.length) {
    await writeFile(scratch, stdout, { mode: 0o600 });
    actual = JSON.parse((await execute('/usr/bin/plutil', ['-convert', 'json', '-o', '-', scratch])).stdout);
  }
  if (!equalPlist(actual, spec)) throw new Error('Code entitlements differ from policy: ' + path.basename(file));
}

async function sourceCopy(spec, output) {
  // Existing generated inputs are merely a speed-up. Verify the private copy
  // against source-controlled hashes before any executable can be signed or run.
  const available = path.join(root, 'apps/desktop/src-tauri/resources', spec.dir);
  const exists = await lstat(available).then(
    () => true,
    (error) => {
      if (error.code !== 'ENOENT') throw error;
      return false;
    },
  );
  if (exists) {
    await cp(available, output, { recursive: true, force: false, errorOnExist: true, verbatimSymlinks: true });
  } else {
    const tools = path.join(root, '.tools');
    await mkdir(tools, { recursive: true });
    const cacheBase = await realpath(tools);
    const nodeCache = path.join(cacheBase, 'node-runtime-cache');
    if (spec.key === 'nodeRuntime') await packageNodeRuntime({ output, cache: nodeCache });
    else if (spec.key === 'developmentNode')
      await prepareDevelopmentNode({ outputDir: output, cacheDir: nodeCache });
    else
      await prepareDevelopmentGit({ outputDir: output, cacheDir: path.join(cacheBase, 'development-git-cache') });
  }
  if (spec.key === 'developmentGit') await verifyDevelopmentGit(output);
  else if (spec.key === 'developmentNode') await verifyDevelopmentNode(output);
  else {
    if (
      (await realpath(path.join(output, 'bin/node'))) !== path.join(output, 'bin/node') ||
      (await realpath(path.join(output, 'LICENSE'))) !== path.join(output, 'LICENSE')
    )
      throw new Error('Linked Node input.');
    verifyRuntimeBytes({
      binary: await readFile(path.join(output, 'bin/node')),
      license: await readFile(path.join(output, 'LICENSE')),
    });
  }
}

async function files(directory, relative = '') {
  const entries = [];
  for (const name of await readdir(path.join(directory, relative))) {
    const location = relative ? relative + '/' + name : name;
    const stat = await lstat(path.join(directory, location));
    if (stat.isDirectory()) {
      entries.push(
        { type: 'directory', path: location, mode: stat.mode & 0o7777 },
        ...(await files(directory, location)),
      );
    } else if (stat.isFile() && stat.nlink === 1) {
      entries.push({ type: 'file', path: location, mode: stat.mode & 0o7777 });
    } else throw new Error('Link or special tool input: ' + location);
  }
  return entries.sort((a, b) => Buffer.compare(Buffer.from(a.path), Buffer.from(b.path)));
}

async function transform(spec, layout, identity, policy, profile) {
  const source = path.join(layout.sourceTools, spec.dir),
    dest = path.join(layout.resources, spec.dir);
  await sourceCopy(spec, source);
  const entries = await files(source);
  if (
    spec.key === 'nodeRuntime' &&
    JSON.stringify(entries.map((entry) => entry.path)) !==
      JSON.stringify(['LICENSE', 'bin', 'bin/node', 'runtime.json'])
  ) {
    throw new Error('Unexpected runtime Node files.');
  }
  const nativePaths =
    spec.key === 'nodeRuntime'
      ? ['bin/node']
      : spec.key === 'developmentNode'
        ? DEVELOPMENT_NODE_RELEASE.executables
        : DEVELOPMENT_GIT_RELEASE.executables;
  await cp(source, dest, { recursive: true, force: false, errorOnExist: true });
  const role = spec.key === 'developmentGit' ? 'git' : 'node';
  const entitlements = path.join(root, 'build-profiles', 'macos-' + role + '.entitlements.plist');
  const entitlementsSha256 = sha256(await readFile(entitlements));
  if (policy[role + 'EntitlementsSha256'] !== entitlementsSha256)
    throw new Error('Entitlement policy digest mismatch.');
  const expected = JSON.parse(
    (await execute('/usr/bin/plutil', ['-convert', 'json', '-o', '-', entitlements])).stdout,
  );
  const rootMode = (await lstat(dest)).mode & 0o7777;
  const digest = createHash('sha256').update('signed-tools-tree-v1\0' + spec.dir + '\0' + rootMode + '\n');
  let totalBytes = 0,
    fileCount = 0;
  for (const entry of entries) {
    if (entry.type === 'file') {
      const file = path.join(dest, entry.path);
      let bytes = await readFile(file);
      const original = { size: bytes.length, sha256: sha256(bytes) };
      let signature = null,
        sourceBinding = null;
      // Only thin little-endian arm64 Mach-O is valid for these pinned sources.
      if (bytes.length >= 4 && bytes.readUInt32LE(0) === 0xfeedfacf) {
        if (
          !nativePaths.includes(entry.path) ||
          bytes.length < 8 ||
          bytes.readUInt32LE(4) !== 0x0100000c ||
          !bytes.equals(await readFile(path.join(source, entry.path)))
        )
          throw new Error('Private signing input changed.');
        const identifier = profile.identifier + '.tools.' + spec.dir + '.' + sha256(entry.path);
        await signCode(file, identifier, identity, entitlements, policy.expectedTeamId);
        await verifyEntitlements(file, expected, path.join(layout.receipts, 'entitlements.plist'));
        sourceBinding = original;
        signature = { teamId: policy.expectedTeamId, identifier, hardenedRuntime: true, entitlementsSha256 };
        bytes = await readFile(file);
      }
      Object.assign(entry, { size: bytes.length, sha256: sha256(bytes), signature, source: sourceBinding });
      fileCount++;
      totalBytes += bytes.length;
    }
    digest.update(JSON.stringify(entry) + '\n');
  }
  const git = spec.key === 'developmentGit';
  const program = git ? 'bin/git' : 'bin/node';
  const javascript = 'let total=0; for(let i=0;i<100000;i++) total+=i; process.versions.node+" "+total';
  const args = git ? ['--version'] : ['-p', javascript];
  const expectedOutput = git ? 'git version ' + spec.toolVersion : spec.toolVersion + ' 4999950000';
  const response = await execute(path.join(dest, program), args, {
    env: { PATH: '/usr/bin:/bin', LC_ALL: 'C' },
    cwd: dest,
    timeout: 15_000,
  });
  if (response.stdout.trim() !== expectedOutput) throw new Error('Signed tool execution check failed: ' + spec.dir);
  return {
    root: spec.dir,
    rootMode,
    toolVersion: spec.toolVersion,
    npmVersion: spec.npmVersion ?? null,
    entryCount: entries.length,
    fileCount,
    totalBytes,
    sha256: digest.digest('hex'),
    entries,
  };
}

export async function prepareSignedTools(plan, identity, policy) {
  const { profile, layout } = plan;
  const manifest = {
    schemaVersion: 1,
    digestRuleVersion: 1,
    profile: {
      sha256: profile.sha256,
      version: profile.version,
      identifier: profile.identifier,
      environment: profile.environment,
      target: profile.target,
      minimumSystemVersion: profile.minimumSystemVersion,
    },
    signingProfile: policy.signingProfile,
    signingPolicySha256: plan.signingPolicy.sha256,
    expectedTeamId: policy.expectedTeamId,
    sourceAnchors: {},
    payloads: {},
  };
  for (const spec of specifications) {
    manifest.sourceAnchors[spec.key] = sha256(await readFile(path.join(root, 'scripts', spec.anchor)));
    manifest.payloads[spec.key] = await transform(spec, layout, identity, policy, profile);
  }
  await writeFile(layout.compiledToolManifest, JSON.stringify(manifest) + '\n', { flag: 'wx', mode: 0o644 });
  await chmod(layout.compiledToolManifest, 0o644);
}
