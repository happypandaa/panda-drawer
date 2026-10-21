// Developer-only preparation. Only the pinned executable and upstream license
// leave the archive; no extraction tool, npm install or system-Node fallback.
import { constants } from 'node:fs';
import { lstat, mkdir, mkdtemp, open, readFile, readdir, rmdir, unlink, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { execFile } from 'node:child_process';
import https from 'node:https';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { inflateRawSync } from 'node:zlib';

const execFileAsync = promisify(execFile);
const SOURCE_URL = 'https://nodejs.org/download/release/v24.22.0/node-v24.22.0-win-x64.zip';
const ARCHIVE_HASH = 'f9683a19877e12a9b167305c8db71040d812876359de21f0d2e97c3f321f80c3';
const BINARY_HASH = 'ba93f9a70bd86fce3ebbb39ec758501458453467aa92a34a0a3602d1e3fc2b9c';
const LICENSE_HASH = 'ed34dd8e3f0a78dbaf00d0444ce8e285b015b765379c2e17880455f70370f8e9';
const MAX_ARCHIVE = 96 * 1024 * 1024;
const MAX_EXPANDED = 512 * 1024 * 1024;
const MAX_BINARY = 160 * 1024 * 1024;
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const utf8 = new TextDecoder('utf-8', { fatal: true });
const sameFile = (a, b) => a.dev === b.dev && a.ino === b.ino;
export const WINDOWS_NODE_RELEASE = JSON.parse(await readFile(new URL('./node-runtime-release-windows.json', import.meta.url), 'utf8'));

function validateRelease(release, pinned) {
  if (release.schemaVersion !== 1 || release.version !== '24.22.0' || release.platform !== 'win32' || release.arch !== 'x64'
    || release.archiveName !== 'node-v24.22.0-win-x64.zip' || release.archiveRoot !== 'node-v24.22.0-win-x64'
    || release.source?.url !== SOURCE_URL || release.binary?.path !== 'node.exe' || release.license?.path !== 'LICENSE'
    || !Number.isSafeInteger(release.binary.size) || release.binary.size < 128 || release.binary.size > MAX_BINARY
    || ![release.source.sha256, release.binary.sha256, release.license.sha256].every(hash => /^[a-f0-9]{64}$/.test(hash))) throw new Error('Invalid fixed Windows Node release metadata.');
  if (pinned && (release.source.sha256 !== ARCHIVE_HASH || release.binary.sha256 !== BINARY_HASH || release.license.sha256 !== LICENSE_HASH || release.binary.size !== 93882184)) throw new Error('Windows Node hashes differ from the reviewed official release.');
}
validateRelease(WINDOWS_NODE_RELEASE, true);

function safeComponent(part) {
  return part && part !== '.' && part !== '..' && !/[<>:"|?*\u0000-\u001f\u007f]/.test(part)
    && !/[. ]$/.test(part) && !/^(con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\.|$)/i.test(part);
}
function inputPath(value, label) {
  if (typeof value !== 'string' || !path.isAbsolute(value) || /[\u0000-\u001f\u007f]/.test(value)
    || value.slice(path.parse(value).root.length).split(/[\\/]/).some(part => !safeComponent(part))) throw new Error(`${label} must be an absolute ordinary path without traversal or aliases.`);
  if (process.platform === 'win32' && !/^[a-z]:[\\/]$/i.test(path.parse(value).root)) throw new Error(`${label} must use a local drive path.`);
  return path.resolve(value);
}
async function directoryGuard(directory) {
  const guard = [];
  let cursor = path.parse(directory).root;
  for (const part of [null, ...directory.slice(cursor.length).split(path.sep).filter(Boolean)]) {
    if (part !== null) cursor = path.join(cursor, part);
    const info = await lstat(cursor);
    if (!info.isDirectory() || info.isSymbolicLink()) throw new Error('Runtime path contains a link or non-directory: ' + cursor);
    guard.push({ path: cursor, info });
  }
  return guard;
}
async function checkGuard(guard) {
  for (const saved of guard) {
    const info = await lstat(saved.path);
    if (!info.isDirectory() || info.isSymbolicLink() || !sameFile(info, saved.info)) throw new Error('A runtime directory changed identity.');
  }
}
async function requireAbsent(target) {
  try { await lstat(target); } catch (cause) { if (cause.code === 'ENOENT') return; throw cause; }
  throw new Error('Runtime output already exists; choose a new output directory.');
}
async function readRegularFile(file, maxBytes) {
  await directoryGuard(path.dirname(file));
  const before = await lstat(file);
  if (!before.isFile() || before.isSymbolicLink() || before.nlink !== 1 || before.size > maxBytes) throw new Error('Runtime input must be a bounded ordinary file without links.');
  const handle = await open(file, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0));
  try {
    const opened = await handle.stat();
    if (!sameFile(before, opened) || !opened.isFile() || opened.nlink !== 1) throw new Error('Runtime input changed while opening.');
    const bytes = Buffer.alloc(before.size);
    for (let offset = 0; offset < bytes.length;) {
      const { bytesRead } = await handle.read(bytes, offset, bytes.length - offset, offset);
      if (!bytesRead) throw new Error('Runtime input was truncated.');
      offset += bytesRead;
    }
    const after = await handle.stat();
    const named = await lstat(file);
    if (!sameFile(after, named) || after.size !== before.size || after.mtimeMs !== before.mtimeMs || after.nlink !== 1 || named.isSymbolicLink()) throw new Error('Runtime input changed while reading.');
    return bytes;
  } finally { await handle.close(); }
}
async function downloadOfficialArchive({ url, destination }) {
  if (url !== SOURCE_URL) throw new Error('Only the fixed official Windows Node URL is allowed.');
  const response = await new Promise((resolve, reject) => {
    const request = https.get(url, { headers: { 'accept-encoding': 'identity' }, signal: AbortSignal.timeout(120_000) }, resolve);
    request.setTimeout(30_000, () => request.destroy(new Error('Node download timed out.')));
    request.on('error', reject);
  });
  const length = response.headers['content-length'];
  if (response.statusCode !== 200 || (response.headers['content-encoding'] && response.headers['content-encoding'] !== 'identity')
    || (length && (!/^\d+$/.test(length) || Number(length) > MAX_ARCHIVE))) {
    response.destroy(); throw new Error('Official Node download rejected; redirects and oversized archives are not accepted.');
  }
  let handle;
  try {
    handle = await open(destination, 'wx');
    let total = 0;
    for await (const bytes of response) { total += bytes.length; if (total > MAX_ARCHIVE) throw new Error('Official archive exceeds its download budget.'); await handle.writeFile(bytes); }
    await handle.sync();
  } finally { response.destroy(); await handle?.close(); }
}

const crcTable = Array.from({ length: 256 }, (_, index) => {
  let value = index;
  for (let bit = 0; bit < 8; bit++) value = (value >>> 1) ^ ((value & 1) ? 0xedb88320 : 0);
  return value >>> 0;
});
function crc32(bytes) { let value = 0xffffffff; for (const byte of bytes) value = (value >>> 8) ^ crcTable[(value ^ byte) & 255]; return (value ^ 0xffffffff) >>> 0; }
function extraFields(bytes) {
  for (let cursor = 0; cursor < bytes.length;) {
    if (cursor + 4 > bytes.length) throw new Error('Truncated ZIP extra field.');
    const kind = bytes.readUInt16LE(cursor); const size = bytes.readUInt16LE(cursor + 2);
    cursor += 4;
    // Only timestamps are recognized. ZIP64, alternate path and Unix link
    // metadata cannot redirect the selected Windows payload.
    if (cursor + size > bytes.length || ![0x000a, 0x5455].includes(kind)) throw new Error('Unsupported ZIP extra field.');
    cursor += size;
  }
}
function archiveName(bytes, flags, root) {
  if (!(flags & 0x800) && bytes.some(byte => byte > 127)) throw new Error('Ambiguous ZIP filename encoding.');
  const name = utf8.decode(bytes);
  const directory = name.endsWith('/');
  const cleaned = directory ? name.slice(0, -1) : name;
  const parts = cleaned.split('/');
  if (name.includes('\\') || parts[0] !== root || !parts.every(safeComponent)) throw new Error('Unsafe or unexpected ZIP member path.');
  return { name: cleaned, directory };
}

// Strictly parse central and local records without extracting arbitrary files.
// The reviewed Node ZIP uses single-disk, non-ZIP64 records without descriptors.
export function extractWindowsRuntimeZip(archive, release = WINDOWS_NODE_RELEASE) {
  if (!Buffer.isBuffer(archive) || archive.length > MAX_ARCHIVE || archive.length < 22) throw new Error('Invalid bounded Node ZIP.');
  let eocd = -1;
  for (let cursor = archive.length - 22; cursor >= Math.max(0, archive.length - 22 - 65535); cursor--) {
    if (archive.readUInt32LE(cursor) === 0x06054b50 && cursor + 22 + archive.readUInt16LE(cursor + 20) === archive.length) { eocd = cursor; break; }
  }
  if (eocd < 0 || archive.readUInt16LE(eocd + 4) || archive.readUInt16LE(eocd + 6)) throw new Error('Missing or multi-disk ZIP end record.');
  const count = archive.readUInt16LE(eocd + 10);
  const centralSize = archive.readUInt32LE(eocd + 12); const centralStart = archive.readUInt32LE(eocd + 16);
  if (!count || count === 65535 || archive.readUInt16LE(eocd + 8) !== count || centralStart + centralSize !== eocd) throw new Error('Invalid ZIP central directory bounds.');
  const wanted = new Set([`${release.archiveRoot}/node.exe`, `${release.archiveRoot}/LICENSE`]);
  const selected = new Map(); const names = new Set(); const kinds = new Map(); const ranges = [];
  let cursor = centralStart; let expanded = 0;
  for (let index = 0; index < count; index++) {
    if (cursor + 46 > eocd || archive.readUInt32LE(cursor) !== 0x02014b50) throw new Error('Truncated ZIP central record.');
    const flags = archive.readUInt16LE(cursor + 8); const method = archive.readUInt16LE(cursor + 10);
    const crc = archive.readUInt32LE(cursor + 16); const packed = archive.readUInt32LE(cursor + 20); const size = archive.readUInt32LE(cursor + 24);
    const nameLength = archive.readUInt16LE(cursor + 28); const extraLength = archive.readUInt16LE(cursor + 30); const commentLength = archive.readUInt16LE(cursor + 32);
    const attributes = archive.readUInt32LE(cursor + 38); const local = archive.readUInt32LE(cursor + 42);
    const end = cursor + 46 + nameLength + extraLength + commentLength;
    if (end > eocd || flags & ~0x800 || ![0, 8].includes(method) || archive.readUInt16LE(cursor + 34) || packed === 0xffffffff || size === 0xffffffff || local === 0xffffffff) throw new Error('Unsupported ZIP flags, compression or record bounds.');
    const rawName = archive.subarray(cursor + 46, cursor + 46 + nameLength);
    const member = archiveName(rawName, flags, release.archiveRoot); const key = member.name.normalize('NFC').toLowerCase();
    if (names.has(key)) throw new Error('Duplicate or case-colliding ZIP member.');
    names.add(key);
    for (let parent = key.lastIndexOf('/'); parent > 0; parent = key.lastIndexOf('/', parent - 1)) {
      const ancestor = key.slice(0, parent);
      if (kinds.get(ancestor) === 'file') throw new Error('ZIP member has a file ancestor.');
      kinds.set(ancestor, 'directory');
    }
    if (!member.directory && kinds.has(key)) throw new Error('ZIP member collides with a directory.');
    kinds.set(key, member.directory ? 'directory' : 'file');
    const modeType = (attributes >>> 16) & 0xf000;
    if ((modeType && modeType !== (member.directory ? 0x4000 : 0x8000)) || (attributes & 0xffff & ~0x37) || Boolean(attributes & 0x10) !== member.directory) throw new Error('ZIP links or special members are forbidden.');
    if ((member.directory && (size || packed || method)) || (expanded += size) > MAX_EXPANDED) throw new Error('ZIP expanded budget or directory metadata is invalid.');
    extraFields(archive.subarray(cursor + 46 + nameLength, cursor + 46 + nameLength + extraLength));
    if (local + 30 > centralStart || archive.readUInt32LE(local) !== 0x04034b50) throw new Error('Invalid ZIP local record.');
    const localName = archive.readUInt16LE(local + 26); const localExtra = archive.readUInt16LE(local + 28);
    const dataStart = local + 30 + localName + localExtra; const dataEnd = dataStart + packed;
    if (dataEnd > centralStart || localName !== nameLength || !archive.subarray(local + 30, local + 30 + localName).equals(rawName)
      || archive.readUInt16LE(local + 6) !== flags || archive.readUInt16LE(local + 8) !== method || archive.readUInt32LE(local + 14) !== crc
      || archive.readUInt32LE(local + 18) !== packed || archive.readUInt32LE(local + 22) !== size) throw new Error('ZIP local and central records disagree.');
    extraFields(archive.subarray(local + 30 + localName, dataStart));
    ranges.push([local, dataEnd]);
    if (wanted.has(member.name)) {
      if (member.directory || size > (member.name.endsWith('/LICENSE') ? 2 * 1024 * 1024 : MAX_BINARY)) throw new Error('Selected Node member is not a bounded ordinary file.');
      const packedBytes = archive.subarray(dataStart, dataEnd);
      let bytes;
      if (method === 0) bytes = Buffer.from(packedBytes);
      else {
        const inflated = inflateRawSync(packedBytes, { maxOutputLength: Math.max(1, size), info: true });
        if (inflated.engine.bytesWritten !== packed) throw new Error('Selected Node member has trailing compressed data.');
        bytes = inflated.buffer;
      }
      if (bytes.length !== size || crc32(bytes) !== crc) throw new Error('Selected Node member size or CRC is invalid.');
      selected.set(member.name, bytes);
    }
    cursor = end;
  }
  if (cursor !== eocd || selected.size !== 2) throw new Error('ZIP lacks the exact Node executable and LICENSE.');
  let localEnd = 0;
  for (const [start, end] of ranges.sort((a, b) => a[0] - b[0])) { if (start !== localEnd) throw new Error('ZIP local records overlap or leave unclaimed bytes.'); localEnd = end; }
  if (localEnd !== centralStart) throw new Error('ZIP contains unclaimed local data.');
  return { binary: selected.get(`${release.archiveRoot}/node.exe`), license: selected.get(`${release.archiveRoot}/LICENSE`) };
}

export function verifyWindowsRuntimeBytes({ binary, license }, release = WINDOWS_NODE_RELEASE) {
  if (binary.length !== release.binary.size || digest(binary) !== release.binary.sha256 || digest(license) !== release.license.sha256) throw new Error('Node executable or LICENSE differs from the fixed release.');
  const pe = binary.length >= 64 ? binary.readUInt32LE(0x3c) : 0;
  if (binary.length < 128 || binary.readUInt16LE(0) !== 0x5a4d || pe < 64 || pe + 26 > binary.length || binary.readUInt32LE(pe) !== 0x00004550
    || binary.readUInt16LE(pe + 4) !== 0x8664 || !(binary.readUInt16LE(pe + 22) & 2) || binary.readUInt16LE(pe + 24) !== 0x20b) throw new Error('Private Node must be a native Windows x64 PE executable.');
}
async function probeBinary(binary) {
  const systemRoot = process.env.SystemRoot ?? process.env.SYSTEMROOT;
  if (!path.isAbsolute(binary) || !systemRoot || !path.isAbsolute(systemRoot)) throw new Error('An absolute private Node and Windows system root are required.');
  const options = { cwd: path.dirname(binary), env: { SystemRoot: systemRoot, PATH: path.join(systemRoot, 'System32') }, timeout: 10_000, maxBuffer: 4096, encoding: 'utf8', windowsHide: true, shell: false };
  const version = (await execFileAsync(binary, ['--version'], options)).stdout.trim();
  const identity = JSON.parse((await execFileAsync(binary, ['--input-type=commonjs', '--eval', 'process.stdout.write(JSON.stringify({version:process.versions.node,platform:process.platform,arch:process.arch}))'], options)).stdout);
  if (version !== `v${identity.version}`) throw new Error('Private Node identity probes disagree.');
  return identity;
}

// Tests may substitute in-memory releases/probes; the CLI exposes no such seam.
export async function packageWindowsNodeRuntime(options = {}, dependencies = {}) {
  if (Object.keys(options).some(key => !['output', 'cache', 'archive'].includes(key))) throw new Error('Unknown Windows Node preparation option.');
  const release = dependencies.release ?? WINDOWS_NODE_RELEASE;
  validateRelease(release, !dependencies.release);
  const host = dependencies.host ?? process;
  if (host.platform !== 'win32' || host.arch !== 'x64') throw new Error('Private Windows Node preparation requires Windows x64.');
  const output = inputPath(options.output, 'Output'); const archiveInput = options.archive === undefined ? null : inputPath(options.archive, 'Archive');
  const parent = await directoryGuard(path.dirname(output));
  await requireAbsent(output);
  let cacheFile; let cacheGuard;
  if (options.cache !== undefined) {
    const cache = inputPath(options.cache, 'Cache');
    const overlap = (a, b) => { const relative = path.relative(a, b); return !relative || (!relative.startsWith('..' + path.sep) && !path.isAbsolute(relative)); };
    if (overlap(cache, output) || overlap(output, cache)) throw new Error('Node cache and output must not overlap.');
    const cacheParent = await directoryGuard(path.dirname(cache));
    await checkGuard(cacheParent);
    try { await mkdir(cache); } catch (cause) { if (cause.code !== 'EEXIST') throw cause; }
    cacheGuard = await directoryGuard(cache); cacheFile = path.join(cache, release.archiveName);
  }
  await checkGuard(parent);
  // Stages are developer evidence. Failures retain their bytes instead of
  // recursively deleting caller paths or attempting to repair old outputs.
  const stage = await mkdtemp(path.join(path.dirname(output), '.windows-node-runtime-stage-'));
  const stageGuard = await directoryGuard(stage);
  const downloaded = path.join(stage, release.archiveName);
  let archive;
  if (archiveInput) archive = await readRegularFile(archiveInput, MAX_ARCHIVE);
  else if (cacheFile) { await checkGuard(cacheGuard); try { archive = await readRegularFile(cacheFile, MAX_ARCHIVE); } catch (cause) { if (cause.code !== 'ENOENT') throw cause; } }
  if (!archive) { await (dependencies.download ?? downloadOfficialArchive)({ url: release.source.url, destination: downloaded }); archive = await readRegularFile(downloaded, MAX_ARCHIVE); }
  if (digest(archive) !== release.source.sha256) throw new Error('Node ZIP SHA-256 does not match the fixed official checksum.');
  const bytes = extractWindowsRuntimeZip(archive, release); verifyWindowsRuntimeBytes(bytes, release);
  if (cacheFile) {
    await checkGuard(cacheGuard);
    try { await writeFile(cacheFile, archive, { flag: 'wx' }); }
    catch (cause) { if (cause.code !== 'EEXIST') throw cause; const cached = await readRegularFile(cacheFile, MAX_ARCHIVE); if (digest(cached) !== release.source.sha256) throw new Error('Existing Node cache is invalid and will not be overwritten.'); }
  }
  await checkGuard(stageGuard);
  const binary = path.join(stage, 'node.exe');
  await writeFile(binary, bytes.binary, { flag: 'wx' });
  await writeFile(path.join(stage, 'LICENSE'), bytes.license, { flag: 'wx' });
  await checkGuard(stageGuard);
  const identity = await (dependencies.probe ?? probeBinary)(binary);
  if (identity.version !== release.version || identity.platform !== 'win32' || identity.arch !== 'x64') throw new Error('The absolute private Node executable reported an incompatible identity.');
  const checked = { binary: await readRegularFile(binary, MAX_BINARY), license: await readRegularFile(path.join(stage, 'LICENSE'), 2 * 1024 * 1024) };
  verifyWindowsRuntimeBytes(checked, release);
  const runtime = { schemaVersion: 1, version: release.version, platform: 'win32', arch: 'x64', source: { ...release.source }, binary: { ...release.binary }, license: { ...release.license } };
  await checkGuard(parent); await checkGuard(stageGuard);
  await mkdir(output);
  const outputGuard = await directoryGuard(output);
  for (const [name, data] of [['node.exe', checked.binary], ['LICENSE', checked.license], ['runtime.json', `${JSON.stringify(runtime, null, 2)}\n`]]) {
    await checkGuard(outputGuard); await writeFile(path.join(output, name), data, { flag: 'wx' });
  }
  await checkGuard(outputGuard);
  // A successful stage has only the files created above. Remove those exact
  // ordinary files and the now-empty owned directory, never a recursive tree.
  await checkGuard(stageGuard);
  for (const name of await readdir(stage)) {
    if (![release.archiveName, 'node.exe', 'LICENSE'].includes(name)) throw new Error('Runtime stage contains unexpected evidence; leaving it untouched.');
    const file = path.join(stage, name); const info = await lstat(file);
    if (!info.isFile() || info.isSymbolicLink() || info.nlink !== 1) throw new Error('Runtime staging object changed; leaving it untouched.');
    await unlink(file);
  }
  await checkGuard(stageGuard); await rmdir(stage);
  return { outputDir: output, runtime };
}

export function parseWindowsRuntimeArguments(args) {
  const options = {};
  for (let index = 0; index < args.length; index++) {
    const key = { '--output': 'output', '--cache': 'cache', '--archive': 'archive' }[args[index]];
    if (!key || options[key] !== undefined || !args[index + 1] || args[index + 1].startsWith('--')) throw new Error('Usage: node scripts/package-windows-node-runtime.mjs --output <new-directory> [--cache <directory>] [--archive <verified-zip>]');
    options[key] = args[++index];
  }
  if (!options.output) throw new Error('An explicit new --output directory is required.');
  return options;
}
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { console.log(JSON.stringify(await packageWindowsNodeRuntime(parseWindowsRuntimeArguments(process.argv.slice(2))))); }
  catch (cause) { console.error('Private Windows Node preparation failed: ' + cause.message); process.exitCode = 1; }
}
