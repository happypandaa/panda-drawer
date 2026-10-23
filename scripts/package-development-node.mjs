import { constants } from 'node:fs';
import { chmod, lstat, mkdir, mkdtemp, open, readFile, readdir, rename, rm, rmdir, unlink, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { execFile } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { gunzipSync } from 'node:zlib';
import { NODE_RELEASE, packageNodeRuntime } from './package-node-runtime.mjs';

const execute = promisify(execFile);
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
const utf8 = new TextDecoder('utf-8', { fatal: true });
const MAX_ARCHIVE_BYTES = 96 * 1024 * 1024;
const MAX_TREE_BYTES = 180 * 1024 * 1024;
const MAX_FILE_BYTES = 160 * 1024 * 1024;
const METADATA_FILE = 'development-node.json';
const NPM_ROOT = 'lib/node_modules/npm';
const releaseBytes = await readFile(new URL('./development-node-release.json', import.meta.url));
export const DEVELOPMENT_NODE_RELEASE = JSON.parse(releaseBytes);

// These ordinary shims resolve their sibling Node, never a global installation.
// npm itself invokes npm-cli.js using the explicitly selected Node in builds.
export function developmentLauncher(name) {
  if (!['npm', 'npx'].includes(name)) throw new Error('Unknown development launcher.');
  return Buffer.from(`#!/bin/sh\nset -eu\nbase=$(CDPATH= cd -- "$(/usr/bin/dirname -- "$0")" && pwd -P)\nexec "$base/node" "$base/../lib/node_modules/npm/bin/${name}-cli.js" "$@"\n`);
}

function validateRelease(release) {
  if (release.schemaVersion !== 1 || release.nodeVersion !== NODE_RELEASE.version
      || release.npmVersion !== '11.19.1' || release.platform !== 'darwin' || release.arch !== 'arm64'
      || release.archiveName !== NODE_RELEASE.archiveName || release.archiveSha256 !== NODE_RELEASE.source.sha256
      || release.tree?.rule !== 'sorted-utf8-path-nul-size-nul-sha256-lf-v1'
      || !Number.isSafeInteger(release.tree.fileCount) || release.tree.fileCount < 5 || release.tree.fileCount > 10_000
      || !Number.isSafeInteger(release.tree.totalBytes) || release.tree.totalBytes > MAX_TREE_BYTES
      || !/^[a-f0-9]{64}$/.test(release.tree.sha256) || !Array.isArray(release.executables)
      || !['bin/node', 'bin/npm', 'bin/npx'].every(name => release.executables.includes(name))) {
    throw new Error('Invalid fixed development Node release metadata.');
  }
  for (const relative of release.executables) validateRelative(relative);
}

function validateRelative(relative) {
  if (!relative || /[\\\u0000-\u001f\u007f]/.test(relative)
      || relative.split('/').some(part => !part || part === '.' || part === '..')) {
    throw new Error('Unsafe development toolchain relative path.');
  }
  return relative;
}

function absoluteInput(value, label) {
  if (typeof value !== 'string' || !path.isAbsolute(value) || /[\\\u0000-\u001f\u007f]/.test(value)
      || value.split('/').some(part => part === '.' || part === '..')) {
    throw new Error(`${label} must be an explicit absolute path without traversal or control characters.`);
  }
  return path.normalize(value);
}

const sameFile = (a, b) => a.dev === b.dev && a.ino === b.ino;
const contains = (parent, child) => child === parent || child.startsWith(`${parent}${path.sep}`);

async function directoryGuard(directory) {
  const chain = [];
  let cursor = path.parse(directory).root;
  for (const part of [null, ...directory.slice(cursor.length).split(path.sep).filter(Boolean)]) {
    if (part !== null) cursor = path.join(cursor, part);
    const info = await lstat(cursor);
    if (!info.isDirectory() || info.isSymbolicLink()) throw new Error(`Development toolchain path is not an ordinary directory: ${cursor}`);
    chain.push({ path: cursor, info });
  }
  return chain;
}

async function checkGuard(guard) {
  for (const saved of guard) {
    const current = await lstat(saved.path);
    if (!current.isDirectory() || current.isSymbolicLink() || !sameFile(current, saved.info)) {
      throw new Error('Development toolchain directory changed identity.');
    }
  }
}

async function requireAbsent(target) {
  try { await lstat(target); } catch (error) { if (error.code === 'ENOENT') return; throw error; }
  throw new Error('Development output already exists; choose a new directory.');
}

async function readOrdinary(file, maxBytes, expected) {
  const before = expected ?? await lstat(file);
  if (!before.isFile() || before.isSymbolicLink() || before.nlink !== 1 || before.size > maxBytes) {
    throw new Error(`Development toolchain requires a bounded ordinary file without links: ${file}`);
  }
  const handle = await open(file, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const current = await handle.stat();
    if (!current.isFile() || current.nlink !== 1 || !sameFile(before, current) || current.size !== before.size) {
      throw new Error('Development toolchain file changed while opening.');
    }
    const bytes = Buffer.alloc(before.size);
    for (let position = 0; position < bytes.length;) {
      const { bytesRead } = await handle.read(bytes, position, bytes.length - position, position);
      if (!bytesRead) throw new Error('Development toolchain file was truncated.');
      position += bytesRead;
    }
    const after = await handle.stat();
    if (after.size !== before.size || after.mtimeMs !== before.mtimeMs || after.ctimeMs !== before.ctimeMs || after.nlink !== 1) {
      throw new Error('Development toolchain file changed while reading.');
    }
    return bytes;
  } finally { await handle.close(); }
}

function tarString(bytes) {
  const end = bytes.indexOf(0);
  return utf8.decode(end < 0 ? bytes : bytes.subarray(0, end));
}
function tarNumber(bytes) {
  const value = tarString(bytes).trim();
  if (!/^[0-7]*$/.test(value)) throw new Error('Unsupported development tar numeric encoding.');
  const result = value ? Number.parseInt(value, 8) : 0;
  if (!Number.isSafeInteger(result)) throw new Error('Development tar size is out of range.');
  return result;
}
function paxValues(bytes) {
  const values = {};
  for (let position = 0; position < bytes.length;) {
    const space = bytes.indexOf(32, position);
    const digits = bytes.subarray(position, space).toString('ascii');
    if (space < position || !/^[1-9]\d*$/.test(digits)) throw new Error('Malformed development tar PAX record.');
    const size = Number(digits);
    if (!Number.isSafeInteger(size) || size < space - position + 4 || position + size > bytes.length || bytes[position + size - 1] !== 10) {
      throw new Error('Malformed development tar PAX size.');
    }
    const record = utf8.decode(bytes.subarray(space + 1, position + size - 1));
    const equals = record.indexOf('=');
    if (equals < 1 || record.startsWith('GNU.sparse.')) throw new Error('Unsupported development tar PAX field.');
    values[record.slice(0, equals)] = record.slice(equals + 1);
    position += size;
  }
  return values;
}

// Select only npm's complete subtree from already checksum-verified bytes.
// Nothing is extracted by tar, and links or special files inside npm are rejected.
export function extractDevelopmentNpm(tar) {
  const prefix = `${NODE_RELEASE.archiveRoot}/${NPM_ROOT}/`;
  const files = new Map();
  const seen = new Set();
  let pax = {}; let longPath; let count = 0; let total = 0;
  for (let offset = 0; offset + 512 <= tar.length;) {
    const header = tar.subarray(offset, offset + 512);
    if (header.every(byte => byte === 0)) {
      if (offset + 1024 > tar.length || !tar.subarray(offset).every(byte => byte === 0)) throw new Error('Invalid development tar end marker.');
      if (!files.has(`${NPM_ROOT}/package.json`) || !files.has(`${NPM_ROOT}/LICENSE`)
          || !files.has(`${NPM_ROOT}/bin/npm-cli.js`) || !files.has(`${NPM_ROOT}/bin/npx-cli.js`)) {
        throw new Error('The official archive is missing npm package files.');
      }
      const metadata = JSON.parse(files.get(`${NPM_ROOT}/package.json`).bytes);
      if (metadata.version !== '11.19.1') throw new Error('The official archive contains an unexpected npm version.');
      return files;
    }
    if (++count > 100_000) throw new Error('Development tar has too many entries.');
    const checksum = header.reduce((sum, byte, index) => sum + (index >= 148 && index < 156 ? 32 : byte), 0);
    if (checksum !== tarNumber(header.subarray(148, 156))) throw new Error('Invalid development tar header checksum.');
    const type = String.fromCharCode(header[156] || 48);
    const size = tarNumber(header.subarray(124, 136));
    const start = offset + 512; const end = start + size;
    offset = start + Math.ceil(size / 512) * 512;
    if (end > tar.length || offset > tar.length) throw new Error('Truncated development tar.');
    const bytes = tar.subarray(start, end);
    if (type === 'x' || type === 'g') {
      const values = paxValues(bytes);
      if (type === 'g' && ['path', 'linkpath', 'size'].some(key => key in values)) throw new Error('Unsupported global development tar override.');
      if (type === 'x') pax = { ...pax, ...values };
      continue;
    }
    if (type === 'L' || type === 'K') {
      if (type === 'L') longPath = tarString(bytes);
      continue;
    }
    let name = tarString(header.subarray(0, 100));
    if (header.subarray(257, 263).equals(Buffer.from('ustar\0'))) {
      const parent = tarString(header.subarray(345, 500));
      if (parent) name = `${parent}/${name}`;
    }
    if ('size' in pax && Number(pax.size) !== size) throw new Error('Unsupported development tar size override.');
    name = pax.path ?? longPath ?? name;
    pax = {}; longPath = undefined;
    validateRelative(name.endsWith('/') ? name.slice(0, -1) : name);
    if (name.split('/')[0] !== NODE_RELEASE.archiveRoot) throw new Error('Unexpected development tar root.');
    if (name.replace(/\/$/, '') === prefix.slice(0, -1)) {
      if (type !== '5' || size !== 0) throw new Error('npm root must be an ordinary directory.');
      continue;
    }
    if (!name.startsWith(prefix)) continue;
    const relative = name.slice(NODE_RELEASE.archiveRoot.length + 1).replace(/\/$/, '');
    if (seen.has(relative)) throw new Error('Duplicate npm archive member.');
    seen.add(relative);
    if (type === '5') { if (size !== 0) throw new Error('Invalid npm directory member.'); continue; }
    if (type !== '0') throw new Error('npm archive links or special members are not allowed.');
    total += size;
    if (size > 16 * 1024 * 1024 || total > 32 * 1024 * 1024 || files.size >= 10_000) throw new Error('npm archive exceeds its bounds.');
    files.set(relative, { bytes: Buffer.from(bytes), mode: (tarNumber(header.subarray(100, 108)) & 0o111) ? 0o755 : 0o644 });
  }
  throw new Error('Development tar has no complete end marker.');
}

export function developmentTreeDigest(entries) {
  return treeSummary([...entries].map(([relative, bytes]) => [relative, { size: bytes.length, sha256: sha256(bytes) }]));
}

function treeSummary(entries) {
  const sorted = [...entries].sort(([a], [b]) => Buffer.compare(Buffer.from(a), Buffer.from(b)));
  const hash = createHash('sha256'); let totalBytes = 0;
  for (const [relative, entry] of sorted) {
    validateRelative(relative);
    hash.update(`${relative}\0${entry.size}\0${entry.sha256}\n`);
    totalBytes += entry.size;
  }
  return { fileCount: sorted.length, totalBytes, sha256: hash.digest('hex') };
}

// This checks against the source-controlled trust anchor, never hashes supplied
// by the installation being inspected. It does not execute Node or npm.
export async function verifyDevelopmentNode(root) {
  validateRelease(DEVELOPMENT_NODE_RELEASE);
  root = absoluteInput(root, 'Development root');
  const guard = await directoryGuard(root);
  const entries = []; const expectedExecutables = new Set(DEVELOPMENT_NODE_RELEASE.executables);
  let metadata; let fileCount = 0; let totalBytes = 0;
  async function visit(directory, relative = '') {
    const before = await lstat(directory);
    if (!before.isDirectory() || before.isSymbolicLink()) throw new Error('Development tree contains a linked or non-directory path.');
    if (![0o555, 0o755].includes(before.mode & 0o7777)) throw new Error('Development directory permissions differ from 0555/0755.');
    const names = await readdir(directory);
    if (!names.length) throw new Error('Development tree contains an unexpected empty directory.');
    for (const name of names.sort()) {
      const child = relative ? `${relative}/${name}` : name;
      validateRelative(child);
      const file = path.join(directory, name); const info = await lstat(file);
      if (info.isSymbolicLink()) throw new Error('Development tree contains a symbolic link.');
      if (info.isDirectory()) { await visit(file, child); continue; }
      if (++fileCount > 10_000) throw new Error('Development tree contains too many files.');
      totalBytes += info.size;
      if (totalBytes > MAX_TREE_BYTES) throw new Error('Development tree exceeds its byte budget.');
      const bytes = await readOrdinary(file, MAX_FILE_BYTES, info);
      const allowedModes = expectedExecutables.has(child) ? [0o555, 0o755] : [0o444, 0o644];
      if (!allowedModes.includes(info.mode & 0o7777)) throw new Error(`Development file permissions differ: ${child}`);
      if (child === METADATA_FILE) metadata = bytes;
      else entries.push([child, { size: bytes.length, sha256: sha256(bytes) }]);
    }
    const after = await lstat(directory);
    if (!sameFile(before, after) || before.mtimeMs !== after.mtimeMs || before.ctimeMs !== after.ctimeMs) throw new Error('Development directory changed during verification.');
  }
  await visit(root);
  await checkGuard(guard);
  if (!metadata?.equals(releaseBytes)) throw new Error('Development metadata differs from the fixed release.');
  const actual = treeSummary(entries); const expected = DEVELOPMENT_NODE_RELEASE.tree;
  if (actual.fileCount !== expected.fileCount || actual.totalBytes !== expected.totalBytes || actual.sha256 !== expected.sha256) {
    throw new Error('Development Node/npm tree differs from the fixed release.');
  }
  return { root, nodePath: path.join(root, 'bin/node'), npmPath: path.join(root, NPM_ROOT, 'bin/npm-cli.js'),
    npxPath: path.join(root, NPM_ROOT, 'bin/npx-cli.js'), binPath: path.join(root, 'bin'),
    nodeVersion: DEVELOPMENT_NODE_RELEASE.nodeVersion, npmVersion: DEVELOPMENT_NODE_RELEASE.npmVersion, tree: actual };
}

async function probePrepared(root) {
  const bin = path.join(root, 'bin');
  const options = { cwd: root, env: { PATH: `${bin}:/usr/bin:/bin`, LC_ALL: 'C' }, timeout: 10_000, killSignal: 'SIGKILL', maxBuffer: 4096, encoding: 'utf8' };
  const node = JSON.parse((await execute(path.join(bin, 'node'), ['-p', 'JSON.stringify({version:process.versions.node,platform:process.platform,arch:process.arch})'], options)).stdout);
  if (node.version !== '24.22.0' || node.platform !== 'darwin' || node.arch !== 'arm64') throw new Error('Development Node execution identity differs.');
  for (const name of ['npm', 'npx']) {
    if ((await execute(path.join(bin, name), ['--version'], options)).stdout.trim() !== '11.19.1') throw new Error(`Development ${name} execution version differs.`);
  }
}

export async function prepareDevelopmentNode({ outputDir, cacheDir } = {}) {
  validateRelease(DEVELOPMENT_NODE_RELEASE);
  if (process.platform !== 'darwin' || process.arch !== 'arm64') throw new Error('Development Node preparation supports macOS arm64 only.');
  const output = absoluteInput(outputDir, 'Output');
  const cache = cacheDir === undefined ? undefined : absoluteInput(cacheDir, 'Cache');
  if (cache && (contains(cache, output) || contains(output, cache))) throw new Error('Development output and cache must not overlap.');
  const parentGuard = await directoryGuard(path.dirname(output));
  await requireAbsent(output);
  const stage = await mkdtemp(path.join(path.dirname(output), '.development-node-stage-'));
  const owned = await lstat(stage); let reservation;
  try {
    const payload = path.join(stage, 'payload');
    const archiveCache = cache ?? path.join(stage, 'archive-cache');
    // Its existing verifier only executes the pinned Node binary, before npm is materialized.
    await packageNodeRuntime({ output: payload, cache: archiveCache });
    const cacheGuard = await directoryGuard(archiveCache);
    const archive = await readOrdinary(path.join(archiveCache, NODE_RELEASE.archiveName), MAX_ARCHIVE_BYTES);
    await checkGuard(cacheGuard);
    if (sha256(archive) !== NODE_RELEASE.source.sha256) throw new Error('Development archive differs from the fixed official checksum.');
    const npm = extractDevelopmentNpm(gunzipSync(archive, { maxOutputLength: 512 * 1024 * 1024 }));
    for (const [relative, { bytes, mode }] of npm) {
      const destination = path.join(payload, relative);
      await mkdir(path.dirname(destination), { recursive: true, mode: 0o755 });
      await writeFile(destination, bytes, { flag: 'wx', mode });
      await chmod(destination, mode);
    }
    await unlink(path.join(payload, 'runtime.json')); // Only our newly prepared copy.
    for (const name of ['npm', 'npx']) {
      const launcher = path.join(payload, 'bin', name);
      await writeFile(launcher, developmentLauncher(name), { flag: 'wx', mode: 0o755 });
      await chmod(launcher, 0o755);
    }
    await writeFile(path.join(payload, METADATA_FILE), releaseBytes, { flag: 'wx', mode: 0o644 });
    await chmod(path.join(payload, METADATA_FILE), 0o644);
    await verifyDevelopmentNode(payload);
    await probePrepared(payload);
    await checkGuard(parentGuard);
    await mkdir(output, { mode: 0o700 });
    reservation = await lstat(output);
    await checkGuard(parentGuard);
    if (!sameFile(await lstat(output), reservation)) throw new Error('Development output reservation changed identity.');
    await rename(payload, output);
    reservation = undefined;
    return { outputDir: output, nodeVersion: '24.22.0', npmVersion: '11.19.1', tree: DEVELOPMENT_NODE_RELEASE.tree };
  } finally {
    if (reservation) {
      await checkGuard(parentGuard);
      const current = await lstat(output).catch(error => { if (error.code === 'ENOENT') return undefined; throw error; });
      if (current?.isDirectory() && !current.isSymbolicLink() && sameFile(current, reservation)) {
        await rmdir(output).catch(error => { if (!['ENOTEMPTY', 'EEXIST'].includes(error.code)) throw error; });
      }
    }
    await checkGuard(parentGuard);
    const current = await lstat(stage);
    if (!current.isDirectory() || current.isSymbolicLink() || !sameFile(current, owned)) throw new Error('Owned development staging changed identity; retained for inspection.');
    await rm(stage, { recursive: true });
  }
}

export function parseDevelopmentArguments(args) {
  const options = {};
  for (let index = 0; index < args.length; index++) {
    const key = args[index] === '--output' ? 'outputDir' : args[index] === '--cache' ? 'cacheDir' : undefined;
    if (!key || options[key] !== undefined || !args[index + 1] || args[index + 1].startsWith('--')) throw new Error('Usage: node scripts/package-development-node.mjs --output <new-absolute-directory> [--cache <absolute-directory>]');
    options[key] = args[++index];
  }
  if (!options.outputDir) throw new Error('An explicit new --output directory is required.');
  return options;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { console.log(JSON.stringify(await prepareDevelopmentNode(parseDevelopmentArguments(process.argv.slice(2))))); }
  catch (error) { console.error(`Development Node preparation failed: ${error.message}`); process.exitCode = 1; }
}
