import { constants } from 'node:fs';
import { chmod, copyFile, link, lstat, mkdir, mkdtemp, open, readFile, readdir, rename, rm, rmdir, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { execFile } from 'node:child_process';
import https from 'node:https';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { gunzipSync } from 'node:zlib';

const execFileAsync = promisify(execFile);
const ARCHIVE_SHA256 = '6a9724d9b50a5f22b4816cded441b5a8d9ddae546191a5e5b7efa5a5e2ee26d5';
const SOURCE_URL = 'https://nodejs.org/download/release/v24.22.0/node-v24.22.0-darwin-arm64.tar.gz';
const MAX_ARCHIVE_BYTES = 96 * 1024 * 1024;
const MAX_TAR_BYTES = 512 * 1024 * 1024;
const utf8 = new TextDecoder('utf-8', { fatal: true });
const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');

// This source-controlled record is the only production metadata input. The CLI
// accepts neither a metadata path nor a source URL or version override.
export const NODE_RELEASE = JSON.parse(await readFile(new URL('./node-runtime-release.json', import.meta.url), 'utf8'));
validateRelease(NODE_RELEASE, true);

function validateRelease(release, pinned = false) {
  if (release.schemaVersion !== 1 || release.version !== '24.22.0' || release.platform !== 'darwin' || release.arch !== 'arm64'
      || release.archiveName !== 'node-v24.22.0-darwin-arm64.tar.gz' || release.archiveRoot !== 'node-v24.22.0-darwin-arm64'
      || release.source?.url !== SOURCE_URL || release.binary?.path !== 'bin/node' || release.license?.path !== 'LICENSE'
      || !Number.isSafeInteger(release.binary.size) || release.binary.size < 8 || release.binary.size > 160 * 1024 * 1024
      || ![release.source.sha256, release.binary.sha256, release.license.sha256].every((value) => /^[a-f0-9]{64}$/.test(value))) {
    throw new Error('Invalid fixed Node runtime release metadata.');
  }
  if (pinned && release.source.sha256 !== ARCHIVE_SHA256) throw new Error('Node archive checksum differs from the reviewed official release.');
}

function inputPath(value, label) {
  if (typeof value !== 'string' || !value || /[\\\u0000-\u001f\u007f]/.test(value) || value.split('/').includes('..')) {
    throw new Error(`${label} must be an explicit path without parent traversal or control characters.`);
  }
  return path.resolve(value);
}

const sameFile = (a, b) => a.dev === b.dev && a.ino === b.ino;
const contains = (parent, child) => child === parent || child.startsWith(`${parent}${path.sep}`);

async function directoryGuard(directory) {
  const chain = [];
  let cursor = path.parse(directory).root;
  for (const part of [null, ...directory.slice(cursor.length).split(path.sep).filter(Boolean)]) {
    if (part !== null) cursor = path.join(cursor, part);
    const info = await lstat(cursor);
    if (info.isSymbolicLink() || !info.isDirectory()) throw new Error(`Runtime path is not an ordinary directory: ${cursor}`);
    chain.push({ path: cursor, info });
  }
  return chain;
}

async function checkGuard(guard) {
  for (const saved of guard) {
    const info = await lstat(saved.path);
    if (!info.isDirectory() || info.isSymbolicLink() || !sameFile(info, saved.info)) {
      throw new Error('A runtime directory changed identity during preparation.');
    }
  }
}

async function requireAbsent(target) {
  try { await lstat(target); } catch (error) {
    if (error.code === 'ENOENT') return;
    throw error;
  }
  throw new Error('Runtime output already exists; choose a new output directory.');
}

async function readRegularFile(file, maxBytes) {
  const before = await lstat(file);
  if (!before.isFile() || before.isSymbolicLink() || before.nlink !== 1 || before.size > maxBytes) {
    throw new Error('Runtime archive must be a bounded ordinary file without links.');
  }
  const handle = await open(file, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const opened = await handle.stat();
    if (!sameFile(before, opened) || opened.nlink !== 1 || !opened.isFile()) throw new Error('Runtime file changed while opening.');
    const bytes = Buffer.alloc(before.size);
    for (let position = 0; position < bytes.length;) {
      const { bytesRead } = await handle.read(bytes, position, bytes.length - position, position);
      if (!bytesRead) throw new Error('Runtime file was truncated while reading.');
      position += bytesRead;
    }
    const after = await handle.stat();
    if (bytes.length !== before.size || after.size !== before.size || after.mtimeMs !== before.mtimeMs || after.nlink !== 1) {
      throw new Error('Runtime file changed while reading.');
    }
    return bytes;
  } finally { await handle.close(); }
}

async function downloadOfficialArchive({ url, destination }) {
  if (url !== SOURCE_URL) throw new Error('Only the fixed official Node archive may be downloaded.');
  const response = await new Promise((resolve, reject) => {
    const request = https.get(url, { headers: { 'accept-encoding': 'identity' }, signal: AbortSignal.timeout(120_000) }, resolve);
    request.setTimeout(30_000, () => request.destroy(new Error('Node archive download timed out.')));
    request.on('error', reject);
  });
  if (response.statusCode !== 200 || (response.headers['content-encoding'] && response.headers['content-encoding'] !== 'identity')) {
    response.destroy();
    throw new Error(`Official Node archive download rejected (HTTP ${response.statusCode}); redirects are not accepted.`);
  }
  const declaredLength = response.headers['content-length'];
  if (declaredLength && (!/^\d+$/.test(declaredLength) || Number(declaredLength) > MAX_ARCHIVE_BYTES)) {
    response.destroy();
    throw new Error('Official Node archive exceeds the download budget.');
  }
  let handle;
  try {
    handle = await open(destination, 'wx', 0o600);
    let total = 0;
    for await (const bytes of response) {
      total += bytes.length;
      if (total > MAX_ARCHIVE_BYTES) throw new Error('Official Node archive exceeds the download budget.');
      await handle.writeFile(bytes);
    }
    await handle.sync();
  } finally {
    response.destroy();
    await handle?.close();
  }
}

function tarString(bytes) {
  const end = bytes.indexOf(0);
  return utf8.decode(end < 0 ? bytes : bytes.subarray(0, end));
}

function tarNumber(bytes) {
  const value = tarString(bytes).trim();
  if (!/^[0-7]*$/.test(value)) throw new Error('Unsupported tar numeric encoding.');
  const parsed = value ? Number.parseInt(value, 8) : 0;
  if (!Number.isSafeInteger(parsed)) throw new Error('Tar size is out of range.');
  return parsed;
}

function paxValues(bytes) {
  const fields = {};
  let position = 0;
  while (position < bytes.length) {
    const space = bytes.indexOf(32, position);
    const digits = bytes.subarray(position, space).toString('ascii');
    if (space < position || !/^[1-9]\d*$/.test(digits)) throw new Error('Malformed tar PAX record.');
    const length = Number(digits);
    if (!Number.isSafeInteger(length) || position + length > bytes.length || bytes[position + length - 1] !== 10) throw new Error('Malformed tar PAX length.');
    const record = utf8.decode(bytes.subarray(space + 1, position + length - 1));
    const equals = record.indexOf('=');
    if (equals < 1) throw new Error('Malformed tar PAX field.');
    const key = record.slice(0, equals);
    if (key.startsWith('GNU.sparse.')) throw new Error('Sparse runtime archives are unsupported.');
    fields[key] = record.slice(equals + 1);
    position += length;
  }
  return fields;
}

function archivePath(value, root) {
  const cleaned = value.endsWith('/') ? value.slice(0, -1) : value;
  const parts = cleaned.split('/');
  if (!cleaned || /[\\\u0000-\u001f\u007f]/.test(cleaned) || parts.some((part) => !part || part === '.' || part === '..') || parts[0] !== root) {
    throw new Error('Unsafe or unexpected path in the Node archive.');
  }
  return cleaned;
}

// Parse bytes, never invoke tar or extract arbitrary archive members. Only the
// verified binary and the full upstream LICENSE (including dependency notices)
// are materialized; npm, links, headers and other executables stay out.
export function extractRuntimeTar(tar, release = NODE_RELEASE) {
  const selected = new Map();
  const wanted = new Set([`${release.archiveRoot}/bin/node`, `${release.archiveRoot}/LICENSE`]);
  let localPax = {};
  let longPath;
  let entries = 0;
  for (let offset = 0; offset + 512 <= tar.length;) {
    const header = tar.subarray(offset, offset + 512);
    if (header.every((byte) => byte === 0)) {
      if (offset + 1024 > tar.length || !tar.subarray(offset).every((byte) => byte === 0)) throw new Error('Invalid tar end marker.');
      if (selected.size !== 2) throw new Error('The Node archive is missing its binary or LICENSE.');
      return { binary: selected.get(`${release.archiveRoot}/bin/node`), license: selected.get(`${release.archiveRoot}/LICENSE`) };
    }
    if (++entries > 100_000) throw new Error('Node archive has too many entries.');
    const checksum = header.reduce((total, byte, index) => total + (index >= 148 && index < 156 ? 32 : byte), 0);
    if (checksum !== tarNumber(header.subarray(148, 156))) throw new Error('Invalid tar header checksum.');
    const type = String.fromCharCode(header[156] || 48);
    const size = tarNumber(header.subarray(124, 136));
    const start = offset + 512;
    const end = start + size;
    const next = start + Math.ceil(size / 512) * 512;
    if (end > tar.length || next > tar.length) throw new Error('Truncated Node archive.');
    const bytes = tar.subarray(start, end);
    offset = next;
    if (type === 'x' || type === 'g') {
      const values = paxValues(bytes);
      if (type === 'g' && ['path', 'linkpath', 'size'].some((key) => key in values)) throw new Error('Unsupported global tar path override.');
      if (type === 'x') localPax = { ...localPax, ...values };
      continue;
    }
    if (type === 'L' || type === 'K') {
      if (type === 'L') longPath = tarString(bytes);
      continue;
    }
    let name = tarString(header.subarray(0, 100));
    if (header.subarray(257, 263).equals(Buffer.from('ustar\0'))) {
      const prefix = tarString(header.subarray(345, 500));
      if (prefix) name = `${prefix}/${name}`;
    }
    if ('size' in localPax && Number(localPax.size) !== size) throw new Error('Unsupported tar size override.');
    name = archivePath(localPax.path ?? longPath ?? name, release.archiveRoot);
    localPax = {};
    longPath = undefined;
    if (!wanted.has(name)) continue;
    if (type !== '0' || selected.has(name)) throw new Error('Runtime archive contains a linked or duplicate required file.');
    if ((name.endsWith('/LICENSE') && size > 2 * 1024 * 1024) || size > 160 * 1024 * 1024) throw new Error('Required runtime member exceeds its budget.');
    selected.set(name, Buffer.from(bytes));
  }
  throw new Error('Node archive has no complete tar end marker.');
}

export function verifyRuntimeBytes({ binary, license }, release = NODE_RELEASE) {
  if (binary.length !== release.binary.size || sha256(binary) !== release.binary.sha256 || sha256(license) !== release.license.sha256) {
    throw new Error('Extracted Node binary or LICENSE differs from the fixed release.');
  }
  if (binary.length < 8 || binary.readUInt32LE(0) !== 0xfeedfacf || binary.readUInt32LE(4) !== 0x0100000c) {
    throw new Error('The private Node binary is not a native arm64 Mach-O executable.');
  }
}

async function probeBinary(binary) {
  if (!path.isAbsolute(binary)) throw new Error('Private Node must be executed by absolute path.');
  const options = { cwd: path.dirname(binary), env: { PATH: '/usr/bin:/bin', LC_ALL: 'C' }, timeout: 10_000, killSignal: 'SIGKILL', maxBuffer: 4096, encoding: 'utf8' };
  const version = (await execFileAsync(binary, ['--version'], options)).stdout.trim();
  const identity = JSON.parse((await execFileAsync(binary, ['--input-type=commonjs', '--eval', 'process.stdout.write(JSON.stringify({version:process.versions.node,platform:process.platform,arch:process.arch}))'], options)).stdout);
  if (version !== `v${identity.version}`) throw new Error('Private Node version probes disagree.');
  return identity;
}

async function publishCache(archive, cacheFile, guard) {
  await checkGuard(guard);
  const temporaryRoot = await mkdtemp(path.join(path.dirname(cacheFile), '.node-runtime-cache-'));
  const temporaryInfo = await lstat(temporaryRoot);
  const temporary = path.join(temporaryRoot, 'archive');
  try {
    await copyFile(archive, temporary, constants.COPYFILE_EXCL);
    await chmod(temporary, 0o444);
    await checkGuard(guard);
    try { await link(temporary, cacheFile); } catch (error) {
      if (error.code !== 'EEXIST') throw error;
      // Never replace an existing caller-owned cache entry, even if corrupt.
    }
  } finally {
    await checkGuard(guard);
    if (sameFile(await lstat(temporaryRoot), temporaryInfo)) await rm(temporaryRoot, { recursive: true, force: true });
  }
}

async function cleanupOwned(directory, identity, parentGuard) {
  await checkGuard(parentGuard);
  const current = await lstat(directory);
  if (!current.isDirectory() || current.isSymbolicLink() || !sameFile(current, identity)) throw new Error('Owned runtime staging directory changed identity; leaving it untouched.');
  // Only our private staging tree is removed. Caller output/cache directories
  // and preexisting files are never recursively deleted.
  async function writableTree(target) {
    const info = await lstat(target);
    if (!info.isDirectory() || info.isSymbolicLink()) return;
    await chmod(target, 0o700);
    for (const name of await readdir(target)) await writableTree(path.join(target, name));
  }
  await writableTree(directory);
  await rm(directory, { recursive: true, force: true });
}

// The second argument is a module-only testing seam for in-memory fixtures.
// The CLI never accepts release/probe/downloader substitutions or metadata paths.
export async function packageNodeRuntime(options = {}, testDependencies = {}) {
  const outputDir = options.output ?? options.outputDir;
  const cacheDir = options.cache ?? options.cacheDir;
  if ((options.output !== undefined && options.outputDir !== undefined) || (options.cache !== undefined && options.cacheDir !== undefined)) {
    throw new Error('Pass one output and one cache option, not duplicate aliases.');
  }
  const release = testDependencies.release ?? NODE_RELEASE;
  validateRelease(release, !testDependencies.release);
  const host = testDependencies.host ?? process;
  if (host.platform !== 'darwin' || host.arch !== 'arm64') throw new Error('Private Node preparation currently supports macOS arm64 only; Windows and Linux are not enabled.');
  const output = inputPath(outputDir, 'Output');
  const cache = cacheDir === undefined ? undefined : inputPath(cacheDir, 'Cache');
  if (cache && (contains(cache, output) || contains(output, cache))) throw new Error('Runtime output and cache must not overlap.');
  const parentGuard = await directoryGuard(path.dirname(output));
  await requireAbsent(output);
  let cacheGuard;
  if (cache) {
    const cacheParentGuard = await directoryGuard(path.dirname(cache));
    await checkGuard(cacheParentGuard);
    try { await mkdir(cache, { mode: 0o700 }); } catch (error) { if (error.code !== 'EEXIST') throw error; }
    cacheGuard = await directoryGuard(cache);
  }
  await checkGuard(parentGuard);
  const stage = await mkdtemp(path.join(path.dirname(output), '.node-runtime-stage-'));
  const stageInfo = await lstat(stage);
  let reservation;
  try {
    const archiveFile = path.join(stage, release.archiveName);
    const cacheFile = cache && path.join(cache, release.archiveName);
    let archive;
    if (cacheFile) {
      await checkGuard(cacheGuard);
      try { archive = await readRegularFile(cacheFile, MAX_ARCHIVE_BYTES); } catch (error) { if (error.code !== 'ENOENT') throw error; }
    }
    if (!archive) {
      await (testDependencies.download ?? downloadOfficialArchive)({ url: release.source.url, destination: archiveFile });
      archive = await readRegularFile(archiveFile, MAX_ARCHIVE_BYTES);
    }
    if (sha256(archive) !== release.source.sha256) throw new Error('Node archive SHA-256 does not match the fixed official checksum.');
    if (cacheFile && await lstat(archiveFile).then(() => true, (error) => { if (error.code === 'ENOENT') return false; throw error; })) {
      await publishCache(archiveFile, cacheFile, cacheGuard);
    }
    const bytes = extractRuntimeTar(gunzipSync(archive, { maxOutputLength: MAX_TAR_BYTES }), release);
    verifyRuntimeBytes(bytes, release);
    const payload = path.join(stage, 'payload');
    await mkdir(payload, { mode: 0o700 });
    await mkdir(path.join(payload, 'bin'), { mode: 0o700 });
    const binary = path.join(payload, 'bin/node');
    await writeFile(binary, bytes.binary, { flag: 'wx', mode: 0o700 });
    await writeFile(path.join(payload, 'LICENSE'), bytes.license, { flag: 'wx', mode: 0o600 });
    const identity = await (testDependencies.probe ?? probeBinary)(binary);
    if (identity.version !== release.version || identity.platform !== release.platform || identity.arch !== release.arch) throw new Error('The absolute private Node executable reported an incompatible identity.');
    const runtime = {
      schemaVersion: 1, version: release.version, platform: release.platform, arch: release.arch,
      source: { url: release.source.url, sha256: release.source.sha256 },
      binary: { path: 'bin/node', sha256: release.binary.sha256, size: bytes.binary.length },
      license: { path: 'LICENSE', sha256: release.license.sha256 },
    };
    await writeFile(path.join(payload, 'runtime.json'), `${JSON.stringify(runtime, null, 2)}\n`, { flag: 'wx', mode: 0o600 });
    // macOS packaging must modify xattrs and executable signatures. Generated
    // resources remain owner-writable; runtime immutability is enforced by the
    // execution contract and reviewed hashes, not filesystem write protection.
    await chmod(binary, 0o755);
    await chmod(path.join(payload, 'LICENSE'), 0o644);
    await chmod(path.join(payload, 'runtime.json'), 0o644);
    await chmod(path.join(payload, 'bin'), 0o755);
    await chmod(payload, 0o755);
    await checkGuard(parentGuard);
    // An exclusive empty reservation prevents rename from replacing a directory
    // created by another preparer. Only that owned inode may be replaced.
    await mkdir(output, { mode: 0o700 });
    reservation = await lstat(output);
    await checkGuard(parentGuard);
    if (!sameFile(await lstat(output), reservation)) throw new Error('Runtime output reservation changed identity.');
    await rename(payload, output);
    reservation = undefined;
    return { outputDir: output, runtime };
  } finally {
    if (reservation) {
      await checkGuard(parentGuard);
      const current = await lstat(output).catch((error) => { if (error.code === 'ENOENT') return undefined; throw error; });
      if (current && current.isDirectory() && !current.isSymbolicLink() && sameFile(current, reservation)) {
        await rmdir(output).catch((error) => { if (!['ENOTEMPTY', 'EEXIST'].includes(error.code)) throw error; });
      }
    }
    await cleanupOwned(stage, stageInfo, parentGuard);
  }
}

export function parseRuntimeArguments(args) {
  const options = {};
  for (let index = 0; index < args.length; index++) {
    const key = args[index] === '--output' ? 'outputDir' : args[index] === '--cache' ? 'cacheDir' : undefined;
    if (!key || options[key] !== undefined || !args[index + 1] || args[index + 1].startsWith('--')) throw new Error('Usage: node scripts/package-node-runtime.mjs --output <new-directory> [--cache <directory>]');
    options[key] = args[++index];
  }
  if (!options.outputDir) throw new Error('An explicit new --output directory is required.');
  return options;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const result = await packageNodeRuntime(parseRuntimeArguments(process.argv.slice(2)));
    console.log(JSON.stringify(result));
  } catch (error) {
    console.error(`Private Node preparation failed: ${error.message}`);
    process.exitCode = 1;
  }
}
