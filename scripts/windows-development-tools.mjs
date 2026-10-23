// Developer-only preparation of fixed Windows development tools. This is not
// an application installer, a runtime safety boundary or a generic ZIP tool.
import { constants } from 'node:fs';
import { lstat, mkdir, mkdtemp, open, readFile, readdir, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import https from 'node:https';
import path from 'node:path';
import { inflateRawSync } from 'node:zlib';

export const TREE_RULE = 'sorted-utf8-path-nul-size-nul-sha256-lf-v1';
const MAX_ARCHIVE = 96 * 1024 * 1024;
const MAX_EXPANDED = 512 * 1024 * 1024;
const MAX_FILE = 160 * 1024 * 1024;
const MAX_TREE = 220 * 1024 * 1024;
const MAX_FILES = 10_000;
const utf8 = new TextDecoder('utf-8', { fatal: true });
export const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
const sameFile = (a, b) => a.dev === b.dev && a.ino === b.ino;
const order = (a, b) => Buffer.compare(Buffer.from(a), Buffer.from(b));

function component(value) {
  return value && value !== '.' && value !== '..' && !/[<>:"|?*\u0000-\u001f\u007f]/.test(value)
    && !/[. ]$/.test(value) && !/^(con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\.|$)/i.test(value);
}
export function relativePath(value) {
  if (typeof value !== 'string' || value.includes('\\') || !value.split('/').every(component)) throw new Error('Unsafe Windows payload path.');
  return value;
}
function inputPath(value, label) {
  if (typeof value !== 'string' || !path.isAbsolute(value) || /[\u0000-\u001f\u007f]/.test(value)
    || value.slice(path.parse(value).root.length).split(/[\\/]/).some(part => !component(part))) throw new Error(`${label} must be an absolute ordinary path without traversal or aliases.`);
  if (process.platform === 'win32' && !/^[a-z]:[\\/]$/i.test(path.parse(value).root)) throw new Error(`${label} must use a local drive path.`);
  return path.resolve(value);
}
async function directoryGuard(directory) {
  const guard = []; let cursor = path.parse(directory).root;
  for (const part of [null, ...directory.slice(cursor.length).split(path.sep).filter(Boolean)]) {
    if (part !== null) cursor = path.join(cursor, part);
    const info = await lstat(cursor);
    if (!info.isDirectory() || info.isSymbolicLink()) throw new Error('Development tool path contains a link or non-directory.');
    guard.push({ path: cursor, info });
  }
  return guard;
}
async function checkGuard(guard) {
  for (const saved of guard) {
    const now = await lstat(saved.path);
    if (!now.isDirectory() || now.isSymbolicLink() || !sameFile(now, saved.info)) throw new Error('A development tool directory changed identity.');
  }
}
async function absent(target) {
  try { await lstat(target); } catch (cause) { if (cause.code === 'ENOENT') return; throw cause; }
  throw new Error('Development output already exists; choose a new output directory.');
}
async function regular(file, limit) {
  const guards = await directoryGuard(path.dirname(file)); const before = await lstat(file);
  if (!before.isFile() || before.isSymbolicLink() || before.nlink !== 1 || before.size > limit) throw new Error('Development input must be a bounded ordinary file without links.');
  const handle = await open(file, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0));
  try {
    const opened = await handle.stat();
    if (!sameFile(before, opened) || !opened.isFile() || opened.nlink !== 1 || opened.size !== before.size) throw new Error('Development file changed while opening.');
    const bytes = Buffer.alloc(before.size);
    for (let offset = 0; offset < bytes.length;) {
      const { bytesRead } = await handle.read(bytes, offset, bytes.length - offset, offset);
      if (!bytesRead) throw new Error('Development file was truncated.');
      offset += bytesRead;
    }
    const after = await handle.stat(); const named = await lstat(file);
    if (!sameFile(after, named) || !sameFile(before, after) || after.size !== before.size || after.mtimeMs !== before.mtimeMs
      || after.ctimeMs !== before.ctimeMs || after.nlink !== 1 || named.isSymbolicLink()) throw new Error('Development file changed while reading.');
    await checkGuard(guards); return bytes;
  } finally { await handle.close(); }
}

export function peIdentity(bytes) {
  if (bytes.length < 2 || bytes.readUInt16LE(0) !== 0x5a4d) return null;
  const at = bytes.length >= 64 ? bytes.readUInt32LE(0x3c) : 0;
  if (bytes.length < 128 || at < 64 || at + 26 > bytes.length || bytes.readUInt32LE(at) !== 0x00004550) throw new Error('Malformed PE payload.');
  const machine = bytes.readUInt16LE(at + 4); const flags = bytes.readUInt16LE(at + 22); const magic = bytes.readUInt16LE(at + 24);
  if (!(flags & 2) || !((machine === 0x8664 && magic === 0x20b) || (machine === 0x14c && magic === 0x10b))) throw new Error('Unsupported PE payload architecture or executable flags.');
  return { machine, dll: Boolean(flags & 0x2000) };
}
export function fileRecord(name, bytes) {
  relativePath(name); const pe = peIdentity(bytes); const executable = /\.exe$/i.test(name);
  if ((executable && (!pe || pe.dll)) || (/\.dll$/i.test(name) && (!pe || !pe.dll))) throw new Error('Executable or DLL payload lacks matching PE flags.');
  return { path: name, size: bytes.length, sha256: sha256(bytes), executable, peMachine: pe?.machine ?? null };
}
export function treeRecord(files) {
  const hash = createHash('sha256'); let total = 0;
  for (const file of [...files].sort((a, b) => order(a.path, b.path))) {
    hash.update(`${file.path}\0${file.size}\0${file.sha256}\n`); total += file.size;
  }
  return { rule: TREE_RULE, fileCount: files.length, totalBytes: total, sha256: hash.digest('hex') };
}
export function validateRelease(release) {
  if (release?.schemaVersion !== 1 || !['development-node', 'development-git'].includes(release.kind) || release.platform !== 'win32' || release.arch !== 'x64'
    || !/^\d+\.\d+\.\d+(?:\.windows\.\d+)?$/.test(release.version) || !component(release.archiveName)
    || ![null, undefined].includes(release.archiveRoot) && !component(release.archiveRoot)
    || !/^https:\/\//.test(release.source?.url ?? '') || !/^[a-f0-9]{64}$/.test(release.source?.sha256 ?? '')
    || !Array.isArray(release.files) || !release.files.length || release.files.length > MAX_FILES || !Array.isArray(release.licenses) || !release.licenses.length) throw new Error('Invalid fixed Windows development release.');
  const names = new Set(); const directories = new Set(); let previous = '';
  for (const file of release.files) {
    relativePath(file.path); const key = file.path.normalize('NFC').toLowerCase();
    if (names.has(key) || (previous && order(previous, file.path) >= 0) || !Number.isSafeInteger(file.size) || file.size < 0 || file.size > MAX_FILE
      || !/^[a-f0-9]{64}$/.test(file.sha256) || typeof file.executable !== 'boolean' || file.executable !== /\.exe$/i.test(file.path)
      || ![null, 0x8664, 0x14c].includes(file.peMachine) || (/\.(exe|dll)$/i.test(file.path) && file.peMachine === null)) throw new Error('Invalid Windows development file manifest.');
    names.add(key); previous = file.path;
    const parts = key.split('/'); for (let end = 1; end < parts.length; end++) directories.add(parts.slice(0, end).join('/'));
  }
  for (const name of names) if (directories.has(name)) throw new Error('Development manifest has a file/directory collision.');
  const actual = treeRecord(release.files);
  if (actual.totalBytes > MAX_TREE || JSON.stringify(actual) !== JSON.stringify(release.tree)) throw new Error('Development tree declaration differs from its complete file manifest.');
  for (const name of [...Object.values(release.paths), ...release.licenses]) {
    relativePath(name); if (!names.has(name.normalize('NFC').toLowerCase()) && !directories.has(name.normalize('NFC').toLowerCase())) throw new Error('Required development path is missing from the manifest.');
  }
  const main = release.files.find(file => file.path === release.paths.executable);
  if (!main?.executable || main.peMachine !== 0x8664) throw new Error('The development entry point must be native Windows x64.');
}

const crcTable = Array.from({ length: 256 }, (_, index) => {
  let value = index; for (let bit = 0; bit < 8; bit++) value = (value >>> 1) ^ ((value & 1) ? 0xedb88320 : 0); return value >>> 0;
});
function crc32(bytes) { let value = 0xffffffff; for (const byte of bytes) value = (value >>> 8) ^ crcTable[(value ^ byte) & 255]; return (value ^ 0xffffffff) >>> 0; }
function extraFields(bytes) {
  for (let cursor = 0; cursor < bytes.length;) {
    if (cursor + 4 > bytes.length) throw new Error('Truncated ZIP extra field.');
    const kind = bytes.readUInt16LE(cursor); const size = bytes.readUInt16LE(cursor + 2); cursor += 4;
    if (cursor + size > bytes.length || ![0x000a, 0x5455].includes(kind)) throw new Error('Unsupported ZIP extra field.');
    cursor += size;
  }
}

// Both reviewed official ZIPs use single-disk non-ZIP64 records, no descriptors,
// ASCII names, ordinary files/directories and timestamp-only extra records.
export function extractDevelopmentZip(archive, { archiveRoot = null, select = () => true } = {}) {
  if (!Buffer.isBuffer(archive) || archive.length < 22 || archive.length > MAX_ARCHIVE) throw new Error('Invalid bounded development ZIP.');
  let eocd = -1;
  for (let cursor = archive.length - 22; cursor >= Math.max(0, archive.length - 22 - 65535); cursor--) {
    if (archive.readUInt32LE(cursor) === 0x06054b50 && cursor + 22 + archive.readUInt16LE(cursor + 20) === archive.length) { eocd = cursor; break; }
  }
  if (eocd < 0 || archive.readUInt16LE(eocd + 4) || archive.readUInt16LE(eocd + 6)) throw new Error('Missing or multi-disk ZIP end record.');
  const count = archive.readUInt16LE(eocd + 10); const start = archive.readUInt32LE(eocd + 16); const centralSize = archive.readUInt32LE(eocd + 12);
  if (!count || count > MAX_FILES || archive.readUInt16LE(eocd + 8) !== count || start + centralSize !== eocd) throw new Error('Invalid ZIP central directory bounds.');
  const selected = new Map(); const names = new Set(); const kinds = new Map(); const ranges = []; let cursor = start; let expanded = 0;
  for (let index = 0; index < count; index++) {
    if (cursor + 46 > eocd || archive.readUInt32LE(cursor) !== 0x02014b50) throw new Error('Truncated ZIP central record.');
    const flags = archive.readUInt16LE(cursor + 8); const method = archive.readUInt16LE(cursor + 10);
    const crc = archive.readUInt32LE(cursor + 16); const packed = archive.readUInt32LE(cursor + 20); const size = archive.readUInt32LE(cursor + 24);
    const nameLength = archive.readUInt16LE(cursor + 28); const extraLength = archive.readUInt16LE(cursor + 30); const commentLength = archive.readUInt16LE(cursor + 32);
    const attributes = archive.readUInt32LE(cursor + 38); const local = archive.readUInt32LE(cursor + 42); const end = cursor + 46 + nameLength + extraLength + commentLength;
    if (end > eocd || flags & ~0x800 || ![0, 8].includes(method) || archive.readUInt16LE(cursor + 34) || packed === 0xffffffff || size === 0xffffffff || local === 0xffffffff) throw new Error('Unsupported ZIP flags, compression or bounds.');
    const raw = archive.subarray(cursor + 46, cursor + 46 + nameLength);
    if (!(flags & 0x800) && raw.some(byte => byte > 127)) throw new Error('Ambiguous ZIP name encoding.');
    const text = utf8.decode(raw); const directory = text.endsWith('/'); const name = directory ? text.slice(0, -1) : text; relativePath(name);
    if (archiveRoot && name !== archiveRoot && !name.startsWith(archiveRoot + '/')) throw new Error('Unexpected ZIP archive root.');
    const key = name.normalize('NFC').toLowerCase(); if (names.has(key)) throw new Error('Duplicate or case-colliding ZIP member.'); names.add(key);
    for (let parent = key.lastIndexOf('/'); parent > 0; parent = key.lastIndexOf('/', parent - 1)) {
      const ancestor = key.slice(0, parent); if (kinds.get(ancestor) === 'file') throw new Error('ZIP member has a file ancestor.'); kinds.set(ancestor, 'directory');
    }
    if (!directory && kinds.has(key)) throw new Error('ZIP member collides with a directory.'); kinds.set(key, directory ? 'directory' : 'file');
    const type = (attributes >>> 16) & 0xf000;
    if ((type && type !== (directory ? 0x4000 : 0x8000)) || (attributes & 0xffff & ~0x37) || Boolean(attributes & 0x10) !== directory) throw new Error('ZIP links or special members are forbidden.');
    if ((directory && (size || packed || method)) || size > MAX_FILE || (expanded += size) > MAX_EXPANDED) throw new Error('ZIP expanded budget or directory metadata is invalid.');
    extraFields(archive.subarray(cursor + 46 + nameLength, cursor + 46 + nameLength + extraLength));
    if (local + 30 > start || archive.readUInt32LE(local) !== 0x04034b50) throw new Error('Invalid ZIP local record.');
    const localName = archive.readUInt16LE(local + 26); const localExtra = archive.readUInt16LE(local + 28); const dataStart = local + 30 + localName + localExtra; const dataEnd = dataStart + packed;
    if (dataEnd > start || localName !== nameLength || !archive.subarray(local + 30, local + 30 + localName).equals(raw)
      || archive.readUInt16LE(local + 6) !== flags || archive.readUInt16LE(local + 8) !== method || archive.readUInt32LE(local + 14) !== crc
      || archive.readUInt32LE(local + 18) !== packed || archive.readUInt32LE(local + 22) !== size) throw new Error('ZIP local and central records disagree.');
    extraFields(archive.subarray(local + 30 + localName, dataStart)); ranges.push([local, dataEnd]);
    const relative = archiveRoot ? name.slice(archiveRoot.length + 1) : name;
    if (!directory && select(relative)) {
      const compressed = archive.subarray(dataStart, dataEnd); let bytes;
      if (method === 0) bytes = Buffer.from(compressed);
      else { const inflated = inflateRawSync(compressed, { maxOutputLength: Math.max(1, size), info: true }); if (inflated.engine.bytesWritten !== packed) throw new Error('ZIP member has trailing compressed data.'); bytes = inflated.buffer; }
      if (bytes.length !== size || crc32(bytes) !== crc) throw new Error('ZIP member size or CRC is invalid.'); selected.set(relative, bytes);
    }
    cursor = end;
  }
  if (cursor !== eocd) throw new Error('ZIP contains unclaimed central bytes.'); let localEnd = 0;
  for (const [first, last] of ranges.sort((a, b) => a[0] - b[0])) { if (first !== localEnd) throw new Error('ZIP local records overlap or leave unclaimed bytes.'); localEnd = last; }
  if (localEnd !== start || !selected.size) throw new Error('ZIP has unclaimed local data or no selected payload.'); return selected;
}
export function selectNodePayload(name) {
  return ['node.exe', 'LICENSE', 'npm', 'npm.cmd', 'npm.ps1', 'npx', 'npx.cmd', 'npx.ps1'].includes(name) || name.startsWith('node_modules/npm/');
}
export function verifyPayload(files, release) {
  if (files.size !== release.files.length) throw new Error('Development payload file count differs from the fixed manifest.');
  const records = [];
  for (const expected of release.files) {
    const bytes = files.get(expected.path); if (!bytes) throw new Error('Development payload is missing a required file.'); const actual = fileRecord(expected.path, bytes);
    if (JSON.stringify(actual) !== JSON.stringify(expected)) throw new Error('Development file differs from the fixed manifest: ' + expected.path); records.push(actual);
  }
  if (JSON.stringify(treeRecord(records)) !== JSON.stringify(release.tree)) throw new Error('Development full tree differs from the fixed release.');
}
export function downloadRedirect(current, location) {
  const next = new URL(location, current); const from = new URL(current);
  if (from.hostname !== 'github.com' || next.protocol !== 'https:' || next.hostname !== 'release-assets.githubusercontent.com' || next.username || next.password || next.port || next.hash) throw new Error('Development download redirect is not an official GitHub release asset.');
  return next.href;
}
async function download(release, destination) {
  let url = release.source.url;
  for (let redirects = 0; redirects < 2; redirects++) {
    const response = await new Promise((resolve, reject) => {
      const request = https.get(url, { headers: { 'accept-encoding': 'identity' }, signal: AbortSignal.timeout(120_000) }, resolve);
      request.setTimeout(30_000, () => request.destroy(new Error('Development download timed out.'))); request.on('error', reject);
    });
    if ([301, 302, 303, 307, 308].includes(response.statusCode)) { response.destroy(); url = downloadRedirect(url, response.headers.location); continue; }
    const length = response.headers['content-length'];
    if (response.statusCode !== 200 || (response.headers['content-encoding'] && response.headers['content-encoding'] !== 'identity') || (length && (!/^\d+$/.test(length) || Number(length) > MAX_ARCHIVE))) { response.destroy(); throw new Error('Official development download rejected.'); }
    let handle;
    try {
      handle = await open(destination, 'wx'); let total = 0;
      for await (const bytes of response) { total += bytes.length; if (total > MAX_ARCHIVE) throw new Error('Development download exceeds its budget.'); await handle.writeFile(bytes); }
      await handle.sync(); return;
    } finally { response.destroy(); await handle?.close(); }
  }
  throw new Error('Too many development download redirects.');
}

function parents(files) {
  const result = new Set(); for (const file of files) { const parts = file.path.split('/'); for (let end = 1; end < parts.length; end++) result.add(parts.slice(0, end).join('/')); }
  return result;
}
export async function inspectDevelopmentTree(root, { release, metadataBytes, metadataFile }) {
  root = inputPath(root, 'Development root'); validateRelease(release); const guards = await directoryGuard(root); const wantedDirs = parents(release.files); const found = new Map(); const seen = new Set();
  async function visit(relative) {
    const directory = path.join(root, relative); const localGuard = await directoryGuard(directory);
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const name = relative ? `${relative}/${entry.name}` : entry.name; relativePath(name);
      const key = name.normalize('NFC').toLowerCase(); if (seen.has(key)) throw new Error('Case-colliding installed development paths.'); seen.add(key);
      if (entry.isDirectory()) { if (!wantedDirs.has(name)) throw new Error('Unexpected development directory: ' + name); await visit(name); }
      else if (name === metadataFile) { if (!(await regular(path.join(root, name), 8 * 1024 * 1024)).equals(metadataBytes)) throw new Error('Development metadata differs from source trust.'); }
      else { if (!release.files.some(file => file.path === name)) throw new Error('Unexpected development file: ' + name); found.set(name, await regular(path.join(root, name), MAX_FILE)); }
    }
    await checkGuard(localGuard);
  }
  await visit(''); if (!seen.has(metadataFile.toLowerCase())) throw new Error('Missing development metadata.'); verifyPayload(found, release); await checkGuard(guards);
  return { root, ...Object.fromEntries(Object.entries(release.paths).map(([name, value]) => [name, path.join(root, value)])), version: release.version, tree: release.tree };
}
async function writeTree(root, files) {
  const rootGuard = await directoryGuard(root); const created = new Set();
  for (const [name, bytes] of files) {
    const parts = name.split('/');
    for (let end = 1; end < parts.length; end++) {
      const parent = parts.slice(0, end).join('/'); if (created.has(parent)) continue;
      const guard = await directoryGuard(path.dirname(path.join(root, parent))); await checkGuard(guard); await mkdir(path.join(root, parent)); created.add(parent);
    }
    const guard = await directoryGuard(path.dirname(path.join(root, name))); await checkGuard(rootGuard); await checkGuard(guard); const handle = await open(path.join(root, name), 'wx');
    try { await handle.writeFile(bytes); await handle.sync(); } finally { await handle.close(); }
    await checkGuard(guard);
  }
  await checkGuard(rootGuard);
}
export async function prepareDevelopment(options, configuration, dependencies = {}) {
  if (Object.keys(options).some(key => !['outputDir', 'cacheDir', 'archive'].includes(key))) throw new Error('Unknown Windows development preparation option.');
  const host = dependencies.host ?? process; if (host.platform !== 'win32' || host.arch !== 'x64') throw new Error('Development tool preparation requires Windows x64.');
  const release = dependencies.release ?? configuration.release; validateRelease(release);
  const config = { ...configuration, release, metadataBytes: dependencies.release ? Buffer.from(`${JSON.stringify(release, null, 2)}\n`) : configuration.metadataBytes };
  const output = inputPath(options.outputDir, 'Output'); const archiveInput = options.archive === undefined ? null : inputPath(options.archive, 'Archive');
  const outputParent = await directoryGuard(path.dirname(output)); await absent(output); let cacheFile; let cacheGuard;
  if (options.cacheDir !== undefined) {
    const cache = inputPath(options.cacheDir, 'Cache'); const within = (a, b) => { const relative = path.relative(a, b); return !relative || (!relative.startsWith('..' + path.sep) && !path.isAbsolute(relative)); };
    if (within(cache, output) || within(output, cache)) throw new Error('Development cache and output must not overlap.');
    const parent = await directoryGuard(path.dirname(cache)); await checkGuard(parent); try { await mkdir(cache); } catch (cause) { if (cause.code !== 'EEXIST') throw cause; }
    cacheGuard = await directoryGuard(cache); cacheFile = path.join(cache, release.archiveName);
  }
  await checkGuard(outputParent); const stage = await mkdtemp(path.join(path.dirname(output), `.${release.kind}-stage-`)); const stageGuard = await directoryGuard(stage);
  let archive;
  if (archiveInput) archive = await regular(archiveInput, MAX_ARCHIVE);
  else if (cacheFile) { await checkGuard(cacheGuard); try { archive = await regular(cacheFile, MAX_ARCHIVE); } catch (cause) { if (cause.code !== 'ENOENT') throw cause; } }
  if (!archive) { const downloaded = path.join(stage, release.archiveName); await (dependencies.download ?? download)(release, downloaded); archive = await regular(downloaded, MAX_ARCHIVE); }
  if (sha256(archive) !== release.source.sha256) throw new Error('Development ZIP SHA-256 differs from the fixed official checksum.');
  const files = extractDevelopmentZip(archive, { archiveRoot: release.archiveRoot, select: configuration.select }); verifyPayload(files, release);
  if (cacheFile) {
    await checkGuard(cacheGuard); try { await writeFile(cacheFile, archive, { flag: 'wx' }); }
    catch (cause) { if (cause.code !== 'EEXIST') throw cause; if (sha256(await regular(cacheFile, MAX_ARCHIVE)) !== release.source.sha256) throw new Error('Existing development cache is invalid and is preserved.'); }
  }
  await checkGuard(stageGuard); const payload = path.join(stage, 'payload'); await mkdir(payload);
  await writeTree(payload, files); await writeFile(path.join(payload, config.metadataFile), config.metadataBytes, { flag: 'wx' });
  const checked = await inspectDevelopmentTree(payload, config); await (dependencies.probe ?? configuration.probe)(checked, release);
  await inspectDevelopmentTree(payload, config); // Execution cannot silently change the trusted tree.
  await checkGuard(outputParent); await checkGuard(stageGuard); await mkdir(output); // Exclusive; never rename over a competing output.
  const checkedFiles = new Map(); for (const file of release.files) checkedFiles.set(file.path, await regular(path.join(payload, file.path), MAX_FILE)); verifyPayload(checkedFiles, release);
  await writeTree(output, checkedFiles); await writeFile(path.join(output, config.metadataFile), config.metadataBytes, { flag: 'wx' });
  const result = await inspectDevelopmentTree(output, config); await checkGuard(outputParent);
  // Both successful and failed stages are named disposable developer evidence;
  // no recursive cleanup or caller-owned file removal is performed here.
  return { ...result, outputDir: output, evidenceDir: stage };
}
export function parseDevelopmentArguments(args, name) {
  const options = {};
  for (let index = 0; index < args.length; index++) {
    const key = { '--output': 'outputDir', '--cache': 'cacheDir', '--archive': 'archive' }[args[index]];
    if (!key || options[key] !== undefined || !args[index + 1] || args[index + 1].startsWith('--')) throw new Error(`Usage: node scripts/package-windows-development-${name}.mjs --output <new-directory> [--cache <directory>] [--archive <official-zip>]`);
    options[key] = args[++index];
  }
  if (!options.outputDir) throw new Error('An explicit new --output directory is required.'); return options;
}
export async function fixedRelease(url) {
  const bytes = await readFile(url); const release = JSON.parse(bytes); validateRelease(release);
  const freeze = value => { if (value && typeof value === 'object') { for (const child of Object.values(value)) freeze(child); Object.freeze(value); } };
  freeze(release); return { bytes, release };
}
