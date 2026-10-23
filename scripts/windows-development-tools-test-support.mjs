import assert from 'node:assert/strict';
import { mkdir, mkdtemp, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { deflateRawSync } from 'node:zlib';
import { root } from './process.mjs';
import { fileRecord, sha256, treeRecord } from './windows-development-tools.mjs';
import { WINDOWS_DEVELOPMENT_NODE_RELEASE } from './package-windows-development-node.mjs';
import { WINDOWS_DEVELOPMENT_GIT_RELEASE } from './package-windows-development-git.mjs';

export const host = { platform: 'win32', arch: 'x64' };
export function pe({ dll = false, x86 = false } = {}) {
  const bytes = Buffer.alloc(256); bytes.writeUInt16LE(0x5a4d); bytes.writeUInt32LE(64, 0x3c); bytes.writeUInt32LE(0x4550, 64);
  bytes.writeUInt16LE(x86 ? 0x14c : 0x8664, 68); bytes.writeUInt16LE(2 | (dll ? 0x2000 : 0), 86); bytes.writeUInt16LE(x86 ? 0x10b : 0x20b, 88); return bytes;
}
function crc32(bytes) {
  let value = 0xffffffff;
  for (const byte of bytes) { value ^= byte; for (let bit = 0; bit < 8; bit++) value = (value >>> 1) ^ ((value & 1) ? 0xedb88320 : 0); }
  return (value ^ 0xffffffff) >>> 0;
}
export function zip(entries) {
  const locals = []; const central = []; let offset = 0;
  for (const entry of entries) {
    const name = Buffer.from(entry.name); const bytes = entry.bytes ?? Buffer.alloc(0); const method = entry.method ?? 0; const packed = method === 8 ? deflateRawSync(bytes) : bytes;
    const local = Buffer.alloc(30); local.writeUInt32LE(0x04034b50); local.writeUInt16LE(20, 4); local.writeUInt16LE(entry.flags ?? 0, 6); local.writeUInt16LE(method, 8);
    local.writeUInt32LE(crc32(bytes), 14); local.writeUInt32LE(packed.length, 18); local.writeUInt32LE(bytes.length, 22); local.writeUInt16LE(name.length, 26);
    const record = Buffer.alloc(46); record.writeUInt32LE(0x02014b50); record.writeUInt16LE(63, 4); record.writeUInt16LE(20, 6); local.copy(record, 8, 6, 26);
    record.writeUInt16LE(name.length, 28); record.writeUInt32LE(entry.attributes ?? (entry.name.endsWith('/') ? 0x10 : 0x20), 38); record.writeUInt32LE(offset, 42);
    locals.push(local, name, packed); central.push(record, name); offset += local.length + name.length + packed.length;
  }
  const directory = Buffer.concat(central); const end = Buffer.alloc(22); end.writeUInt32LE(0x06054b50); end.writeUInt16LE(entries.length, 8); end.writeUInt16LE(entries.length, 10); end.writeUInt32LE(directory.length, 12); end.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, directory, end]);
}
export function fixture(kind) {
  const node = kind === 'node'; const base = structuredClone(node ? WINDOWS_DEVELOPMENT_NODE_RELEASE : WINDOWS_DEVELOPMENT_GIT_RELEASE);
  const files = new Map(node ? [
    ['node.exe', pe()], ['LICENSE', Buffer.from('Synthetic complete Node notices.\n')],
    ...['npm', 'npm.cmd', 'npm.ps1', 'npx', 'npx.cmd', 'npx.ps1'].map(name => [name, Buffer.from('Synthetic launcher ' + name)]),
    ['node_modules/npm/bin/npm-cli.js', Buffer.from('synthetic npm CLI')], ['node_modules/npm/bin/npx-cli.js', Buffer.from('synthetic npx CLI')],
    ['node_modules/npm/package.json', Buffer.from('{"version":"11.19.1"}')], ['node_modules/npm/LICENSE', Buffer.from('Synthetic npm notices')],
    ['node_modules/npm/node_modules/nested/LICENSE', Buffer.from('Synthetic nested dependency notices')],
  ] : [
    ['cmd/git.exe', pe()], ['LICENSE.txt', Buffer.from('Synthetic full Git notices.\n')],
    ['ucrt64/bin/git-remote-https.exe', pe()], ['ucrt64/bin/support.dll', pe({ dll: true, x86: true })],
    ['ucrt64/doc/git-credential-manager/LICENSE', Buffer.from('Synthetic GCM notices')],
    ['ucrt64/etc/ssl/certs/ca-bundle.crt', Buffer.from('Synthetic certificate material, not a trust store')],
    ['ucrt64/share/git-core/templates/info/exclude', Buffer.from('synthetic template')],
    ['ucrt64/libexec/git-core/git-helper', Buffer.from('synthetic helper, never executed')],
  ]);
  const entries = [...files].map(([name, bytes]) => ({ name: (base.archiveRoot ? base.archiveRoot + '/' : '') + name, bytes, method: 8 }));
  if (node) entries.push({ name: base.archiveRoot + '/install_tools.bat', bytes: Buffer.from('must never leave the archive') });
  const archive = zip(entries); base.source.sha256 = sha256(archive);
  base.files = [...files].map(([name, bytes]) => fileRecord(name, bytes)).sort((a, b) => Buffer.compare(Buffer.from(a.path), Buffer.from(b.path))); base.tree = treeRecord(base.files);
  return { release: base, files, archive, entries, metadataFile: `development-${kind}.json`, metadataBytes: Buffer.from(`${JSON.stringify(base, null, 2)}\n`) };
}
export async function ownedRoot(label = 'tools') {
  const parent = path.join(root, '.test-artifacts'); await mkdir(parent, { recursive: true }); return mkdtemp(path.join(parent, `windows-development-${label}-fixture-`));
}
export function dependencies(value, extra = {}) {
  return { host, release: value.release,
    download: async (release, destination) => { assert.equal(release.source.url, value.release.source.url); await writeFile(destination, value.archive, { flag: 'wx' }); },
    probe: async checked => { assert.ok(path.isAbsolute(checked.executable)); }, ...extra };
}
