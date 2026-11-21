// Windows vendor-pinned payloads. This manifest proves byte/path integrity,
// not our Authenticode publisher identity or successful installed GUI admission.
import { createHash } from 'node:crypto';
import { constants } from 'node:fs';
import { lstat, open, readdir, realpath } from 'node:fs/promises';
import path from 'node:path';
import { packageNodeRuntimeWindows, verifyWindowsNodePayload } from './package-node-runtime-windows.mjs';
import { prepareDevelopmentNodeWindows } from './package-development-node-windows.mjs';
import { prepareDevelopmentGitWindows, verifyDevelopmentGitWindows } from './package-development-git-windows.mjs';

const MAX_FILES = 10_001;
const MAX_BYTES = 256 * 1024 * 1024;
const MAX_ALL_BYTES = 640 * 1024 * 1024;
const MAX_FILE_BYTES = 160 * 1024 * 1024;
const specs = [
  { key: 'nodeRuntime', directory: 'node-runtime', source: 'node-runtime-release.windows-x64.json', version: 'version' },
  { key: 'developmentNode', directory: 'development-node', source: 'development-node-release.windows-x64.json', version: 'nodeVersion' },
  { key: 'developmentGit', directory: 'development-git', source: 'development-git-release.windows-x64.json', version: 'gitVersion' },
];
export const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
const requireThat = (condition, message) => { if (!condition) throw new Error(message); };
const sameFile = (left, right) => ['dev', 'ino', 'size', 'mtimeNs', 'ctimeNs', 'nlink'].every(key => left[key] === right[key]);

export function payloadDigest(entries) {
  const hash = createHash('sha256');
  for (const entry of entries) hash.update(`${entry.path}\0${entry.size}\0${entry.sha256}\n`);
  return hash.digest('hex');
}

export async function inventoryPayload(root) {
  requireThat(path.isAbsolute(root) && await realpath(root) === root, 'Tool root must be canonical and absolute.');
  const entries = [];
  let bytes = 0;
  async function walk(directory, prefix = '') {
    const beforeDirectory = await lstat(directory, { bigint: true });
    requireThat(beforeDirectory.isDirectory() && !beforeDirectory.isSymbolicLink(), 'Tool directories must not be links.');
    const names = await readdir(directory);
    for (const name of names) {
      requireThat(!/[\x00-\x1f\x7f\\/:*?"<>|]/.test(name) && !/[. ]$/.test(name), 'Unsupported Windows tool filename.');
      const file = path.join(directory, name);
      const relative = prefix ? `${prefix}/${name}` : name;
      const info = await lstat(file, { bigint: true });
      requireThat(!info.isSymbolicLink(), 'Tool trees must not contain links.');
      if (info.isDirectory()) {
        await walk(file, relative);
        continue;
      }
      requireThat(info.isFile() && info.nlink === 1n && info.size <= BigInt(MAX_FILE_BYTES), 'Tool payload requires bounded ordinary single-link files.');
      const handle = await open(file, constants.O_RDONLY | constants.O_NOFOLLOW);
      try {
        const opened = await handle.stat({ bigint: true });
        requireThat(sameFile(info, opened), 'Tool payload changed before opening.');
        const hash = createHash('sha256');
        const buffer = Buffer.alloc(64 * 1024);
        let size = 0;
        while (true) {
          const chunk = await handle.read(buffer, 0, buffer.length, size);
          if (!chunk.bytesRead) break;
          size += chunk.bytesRead;
          requireThat(size <= MAX_FILE_BYTES, 'Tool file exceeded its byte budget.');
          hash.update(buffer.subarray(0, chunk.bytesRead));
        }
        requireThat(sameFile(info, await handle.stat({ bigint: true })) && sameFile(info, await lstat(file, { bigint: true })) && BigInt(size) === info.size, 'Tool payload changed during hashing.');
        bytes += size;
        requireThat(entries.length < MAX_FILES && bytes <= MAX_BYTES, 'Tool inventory exceeded its budget.');
        entries.push({ path: relative, size, sha256: hash.digest('hex') });
      } finally { await handle.close(); }
    }
    const afterDirectory = await lstat(directory, { bigint: true });
    requireThat(beforeDirectory.dev === afterDirectory.dev && beforeDirectory.ino === afterDirectory.ino
      && beforeDirectory.mtimeNs === afterDirectory.mtimeNs && beforeDirectory.ctimeNs === afterDirectory.ctimeNs,
    'Tool directory changed during inventory.');
  }
  await walk(root);
  entries.sort((left, right) => Buffer.compare(Buffer.from(left.path), Buffer.from(right.path)));
  return { fileCount: entries.length, totalBytes: bytes, sha256: payloadDigest(entries), entries };
}

export async function prepareWindowsProductionTools({ repository, resources }) {
  const cache = path.join(repository, '.tools', 'node-runtime-cache-windows-x64');
  await packageNodeRuntimeWindows({ outputDir: path.join(resources, 'node-runtime'), cacheDir: cache });
  await prepareDevelopmentNodeWindows({ outputDir: path.join(resources, 'development-node'), cacheDir: cache });
  await prepareDevelopmentGitWindows({ outputDir: path.join(resources, 'development-git'),
    cacheDir: path.join(repository, '.tools', 'development-git-cache-windows-x64') });
  await verifyWindowsProductionTools(resources);
}

export async function verifyWindowsProductionTools(resources) {
  await verifyWindowsNodePayload('runtime', path.join(resources, 'node-runtime'));
  await verifyWindowsNodePayload('development', path.join(resources, 'development-node'));
  await verifyDevelopmentGitWindows(path.join(resources, 'development-git'));
}

export async function createWindowsToolManifest({ profileBytes, sourceBytes, resources }) {
  const profile = JSON.parse(profileBytes);
  const payloads = {}, sourceAnchors = {};
  let allBytes = 0;
  for (const spec of specs) {
    const bytes = sourceBytes[spec.key];
    requireThat(Buffer.isBuffer(bytes), `Missing committed source anchor: ${spec.key}`);
    const source = JSON.parse(bytes);
    const inventory = await inventoryPayload(path.join(resources, spec.directory));
    allBytes += inventory.totalBytes;
    requireThat(allBytes <= MAX_ALL_BYTES, 'Production tools exceed the combined byte budget.');
    sourceAnchors[spec.key] = sha256(bytes);
    payloads[spec.key] = { root: spec.directory, toolVersion: source[spec.version],
      npmVersion: spec.key === 'developmentNode' ? source.npmVersion : null, ...inventory };
  }
  return {
    schemaVersion: 1, digestRuleVersion: 1,
    profile: { sha256: sha256(profileBytes), version: profile.version, identifier: profile.identifier,
      environment: profile.environment, target: profile.target, minimumSystemVersion: profile.minimumSystemVersion },
    sourceAnchors, payloads,
  };
}
