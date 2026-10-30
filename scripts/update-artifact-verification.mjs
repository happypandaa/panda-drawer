import { constants } from 'node:fs';
import { lstat, open } from 'node:fs/promises';
import { createHash, createPublicKey, timingSafeEqual, verify } from 'node:crypto';
import path from 'node:path';
import { hasTraversal } from './native-path.mjs';
import { UPDATE_POLICY, assessStableUpdate, validateSignatureFormat, validateStaticManifest } from './update-manifest-protocol.mjs';

export const ARTIFACT_VERIFICATION_BUDGET = Object.freeze({ publicKeyBytes: 8 * 1024, chunkBytes: 64 * 1024 });
const decoder = () => new TextDecoder('utf-8', { fatal: true });
function requireValue(condition, message) { if (!condition) throw new Error(message); }
function base64(value, length, label) {
  requireValue(typeof value === 'string' && value.length > 0 && /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(value), `${label} must be canonical base64.`);
  const bytes = Buffer.from(value, 'base64');
  requireValue(bytes.toString('base64') === value && (length === undefined || bytes.length === length), `${label} has an invalid length.`);
  return bytes;
}
function algorithm(bytes) {
  requireValue(bytes[0] === 0x45 && (bytes[1] === 0x64 || bytes[1] === 0x44), 'Unsupported minisign algorithm.');
  return bytes[1] === 0x44 ? 'ED' : 'Ed';
}
function publicKey(encoded) {
  const text = decoder().decode(base64(encoded, undefined, 'Public key'));
  const lines = text.replace(/\n$/, '').split('\n');
  requireValue(lines.length === 2 && /^untrusted comment: [^\x00-\x1f\x7f]{1,200}$/.test(lines[0]), 'Public key must contain exactly one Tauri minisign public-key box.');
  const packet = base64(lines[1], 42, 'Public-key packet');
  algorithm(packet);
  const key = createPublicKey({ key: Buffer.concat([Buffer.from('302a300506032b6570032100', 'hex'), packet.subarray(10)]), format: 'der', type: 'spki' });
  return { key, id: packet.subarray(2, 10), sha256: createHash('sha256').update(packet).digest('hex') };
}

async function checkedFile(value, maxBytes) {
  requireValue(typeof value === 'string' && path.isAbsolute(value) && !value.includes('\0') && !hasTraversal(value), 'Verification paths must be absolute without parent traversal.');
  const target = path.normalize(value);
  let current = path.parse(target).root;
  const parts = target.slice(current.length).split(path.sep).filter(Boolean);
  for (const part of parts.slice(0, -1)) {
    current = path.join(current, part);
    const info = await lstat(current);
    requireValue(info.isDirectory() && !info.isSymbolicLink(), 'Verification parent paths must be ordinary directories.');
  }
  const info = await lstat(target, { bigint: true });
  requireValue(info.isFile() && !info.isSymbolicLink() && info.nlink === 1n && info.size > 0n && info.size <= BigInt(maxBytes), 'Verification input must be one bounded, nonempty ordinary file.');
  const file = await open(target, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  try {
    const before = await file.stat({ bigint: true });
    requireValue(before.isFile() && before.nlink === 1n && before.size > 0n && before.size <= BigInt(maxBytes) && before.dev === info.dev && before.ino === info.ino, 'Verification input changed before opening.');
    return { file, before, target };
  } catch (error) { await file.close(); throw error; }
}
async function unchanged({ file, before, target }, length) {
  const after = await file.stat({ bigint: true });
  const named = await lstat(target, { bigint: true });
  requireValue(BigInt(length) === before.size && ['dev', 'ino', 'mode', 'nlink', 'uid', 'gid', 'size', 'mtimeNs', 'ctimeNs'].every(key => before[key] === after[key]) && named.isFile() && !named.isSymbolicLink() && named.dev === after.dev && named.ino === after.ino && named.nlink === 1n, 'Verification input changed during its bounded read.');
}
async function readPublicKey(value) {
  const opened = await checkedFile(value, ARTIFACT_VERIFICATION_BUDGET.publicKeyBytes);
  try {
    const buffer = Buffer.alloc(ARTIFACT_VERIFICATION_BUDGET.publicKeyBytes + 1);
    let length = 0;
    while (length < buffer.length) {
      const { bytesRead } = await opened.file.read(buffer, length, buffer.length - length, length);
      if (!bytesRead) break;
      length += bytesRead;
    }
    requireValue(length <= ARTIFACT_VERIFICATION_BUDGET.publicKeyBytes, 'Public key exceeds its read budget.');
    await unchanged(opened, length);
    return publicKey(decoder().decode(buffer.subarray(0, length)).replace(/\n$/, ''));
  } finally { await opened.file.close(); }
}

/** Offline operator-supplied public key only. Never an application trust root,
 * package-identity check, stable file permit, runtime gate or installer permit. */
export async function verifyUpdateArtifact({ manifest, artifact, publicKeyPath, currentVersion }) {
  const checked = validateStaticManifest(manifest);
  const entry = checked.platforms[UPDATE_POLICY.target];
  const name = decodeURIComponent(new URL(entry.url).pathname.split('/').at(-1));
  requireValue(typeof artifact === 'string' && path.basename(artifact) === name, 'Artifact basename must match the manifest and signed filename.');
  validateSignatureFormat(entry.signature, { version: checked.version, name });
  const lines = decoder().decode(base64(entry.signature, undefined, 'Signature')).replace(/\n$/, '').split('\n');
  const packet = base64(lines[1], 74, 'Signature packet');
  const mode = algorithm(packet);
  // The algorithm tag is not covered by the global signature. Accepting Ed as
  // well would let an ED signature authenticate the 64-byte hash as a new file.
  requireValue(mode === 'ED', 'Legacy Ed artifact signatures are not accepted; an ED prehashed signature is required.');
  const trustedComment = lines[2].slice('trusted comment: '.length);
  const key = await readPublicKey(publicKeyPath);
  requireValue(timingSafeEqual(key.id, packet.subarray(2, 10)), 'Minisign public key ID does not match the signature.');
  // The global signature authenticates the entire trusted-comment byte string,
  // including version and filename. Text-format matching alone proves neither.
  requireValue(verify(null, Buffer.concat([packet.subarray(10), Buffer.from(trustedComment, 'utf8')]), key.key, base64(lines[3], 64, 'Global signature')), 'Minisign trusted-comment signature did not verify.');
  const maxBytes = UPDATE_POLICY.maxArtifactBytes;
  const opened = await checkedFile(artifact, maxBytes);
  try {
    const digest = createHash('sha256');
    const prehash = createHash('blake2b512');
    const buffer = Buffer.alloc(ARTIFACT_VERIFICATION_BUDGET.chunkBytes);
    let length = 0;
    while (true) {
      const { bytesRead } = await opened.file.read(buffer, 0, Math.min(buffer.length, maxBytes + 1 - length), length);
      if (!bytesRead) break;
      length += bytesRead;
      requireValue(length <= maxBytes, `Artifact exceeds the ${mode} verification byte budget.`);
      const chunk = buffer.subarray(0, bytesRead);
      digest.update(chunk);
      prehash.update(chunk);
    }
    await unchanged(opened, length);
    requireValue(verify(null, prehash.digest(), key.key, packet.subarray(10)), 'Artifact byte signature did not verify.');
    return { valid: true, scope: 'offline-artifact-bytes-and-signed-comment', cryptographicSignatureVerified: true, artifactBytesVerified: true,
      productionTrustBound: false, packageIdentityVerified: false, runtimeAdmission: false, installAuthorized: false,
      version: checked.version, signedFilename: name, signatureAlgorithm: mode,
      publicKey: { keyId: key.id.toString('hex'), sha256: key.sha256, source: 'explicit-operator-input' },
      artifact: { path: opened.target, bytes: length, sha256: digest.digest('hex') },
      ...(currentVersion === undefined ? {} : { assessment: { ...assessStableUpdate(checked, currentVersion), cryptographicSignatureVerified: true } }) };
  } finally { await opened.file.close(); }
}
