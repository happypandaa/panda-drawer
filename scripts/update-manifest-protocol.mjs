// Offline metadata protocol only. No network, signing, runtime admission or installation.
import { DOWNLOAD_SOURCES, updateEndpoints, updateSource } from './download-sources.mjs';

export const UPDATE_TARGETS = Object.freeze({
  'darwin-aarch64': '.app.tar.gz',
  'windows-x86_64': '.exe',
});

export const UPDATE_POLICY = Object.freeze({
  schemaVersion: 1,
  profile: 'production',
  identifier: 'com.happypandaa.pandadrawer',
  target: 'darwin-aarch64',
  repository: DOWNLOAD_SOURCES.releaseRepository,
  endpoint: updateEndpoints()[0],
  firstVersion: '0.1.0',
  maxDocumentBytes: 64 * 1024,
  maxNotesBytes: 16 * 1024,
  maxSignatureBytes: 8 * 1024,
  maxArtifactBytes: 1024 * 1024 * 1024,
});

function requireValue(condition, message) { if (!condition) throw new Error(message); }
function exactKeys(value, keys, label) {
  requireValue(value !== null && typeof value === 'object' && !Array.isArray(value)
    && [Object.prototype, null].includes(Object.getPrototypeOf(value)), `${label} must be an object`);
  requireValue(Object.keys(value).length === keys.length && keys.every(key => Object.hasOwn(value, key)), `${label} has missing or unknown fields`);
}
function text(value, maxBytes, label) {
  requireValue(typeof value === 'string' && value.isWellFormed() && Buffer.byteLength(value) <= maxBytes, `${label} exceeds its text budget or is invalid`);
  return value;
}

export function parseVersion(value) {
  text(value, 128, 'version');
  const match = /^(?:v)?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$/.exec(value);
  requireValue(match, 'version must be valid SemVer');
  const core = match.slice(1, 4).map(part => BigInt(part));
  requireValue(core.every(part => part <= 18446744073709551615n), 'version component exceeds the Tauri SemVer range');
  const prerelease = match[4]?.split('.') ?? [];
  requireValue(prerelease.every(part => !/^\d+$/.test(part) || part === '0' || !part.startsWith('0')), 'numeric prerelease identifiers cannot have leading zeros');
  return { version: value.replace(/^v/, ''), core, prerelease, build: match[5] ?? '' };
}

export function compareVersions(left, right) {
  const a = parseVersion(left); const b = parseVersion(right);
  for (let i = 0; i < 3; i++) if (a.core[i] !== b.core[i]) return a.core[i] < b.core[i] ? -1 : 1;
  if (!a.prerelease.length || !b.prerelease.length) return a.prerelease.length === b.prerelease.length ? 0 : a.prerelease.length ? -1 : 1;
  for (let i = 0; i < Math.max(a.prerelease.length, b.prerelease.length); i++) {
    const x = a.prerelease[i]; const y = b.prerelease[i];
    if (x === undefined || y === undefined) return x === undefined ? -1 : 1;
    if (x === y) continue;
    const nx = /^\d+$/.test(x); const ny = /^\d+$/.test(y);
    if (nx && ny) return BigInt(x) < BigInt(y) ? -1 : 1;
    if (nx !== ny) return nx ? -1 : 1;
    return x < y ? -1 : 1;
  }
  return 0; // Build metadata has no SemVer precedence.
}

function publicationDate(value) {
  text(value, 64, 'pub_date');
  const m = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.\d{1,9})?(Z|[+-]\d{2}:\d{2})$/.exec(value);
  requireValue(m, 'pub_date must be RFC 3339 with an explicit timezone');
  const [year, month, day, hour, minute, second] = m.slice(1, 7).map(Number);
  const leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  requireValue(year >= 1970 && month >= 1 && month <= 12 && day >= 1 && day <= days[month - 1]
    && hour <= 23 && minute <= 59 && second <= 59
    && (m[7] === 'Z' || Number(m[7].slice(1, 3)) <= 23 && Number(m[7].slice(4)) <= 59)
    && Number.isFinite(Date.parse(value)), 'pub_date contains an invalid calendar date');
  return value;
}

function assetName(value, target = UPDATE_POLICY.target) {
  text(value, 255, 'artifact name');
  requireValue(Object.hasOwn(UPDATE_TARGETS, target), 'unsupported update platform');
  const suffix = UPDATE_TARGETS[target];
  const stem = value.slice(0, -suffix.length);
  requireValue(value.endsWith(suffix) && value.normalize('NFC') === value
    && /^[\p{L}\p{N}][\p{L}\p{N} ._-]*$/u.test(stem)
    && !/[\u0000-\u001f\u007f]/.test(value)
    && !stem.endsWith(' ') && !value.includes('..'), 'artifact must be a plain filename matching its update platform');
  return value;
}

export function releaseAssetUrl(version, name, { source, target = UPDATE_POLICY.target } = {}) {
  const parsed = parseVersion(version);
  return `${updateSource(source).artifactBaseUrl}/v${parsed.version}/${encodeURIComponent(assetName(name, target))}`;
}

function canonicalBase64(value, bytes, label) {
  requireValue(typeof value === 'string' && /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(value) && value.length > 0, `${label} must be canonical base64`);
  const decoded = Buffer.from(value, 'base64');
  requireValue(decoded.toString('base64') === value && (bytes === undefined || decoded.length === bytes), `${label} has an invalid decoded length`);
  return decoded;
}

// Structural checks are not authentication. Only the future updater's pinned
// public key can authenticate these bytes and their version/file comment.
export function validateSignatureFormat(value, { version, name, target = UPDATE_POLICY.target }) {
  text(value, UPDATE_POLICY.maxSignatureBytes, 'signature');
  const decoded = new TextDecoder('utf-8', { fatal: true }).decode(canonicalBase64(value, undefined, 'signature'));
  const lines = decoded.replace(/\n$/, '').split('\n');
  requireValue(lines.length === 4 && /^untrusted comment: [^\x00-\x1f\x7f]{1,200}$/.test(lines[0]), 'signature must contain the Tauri minisign text');
  const signature = canonicalBase64(lines[1], 74, 'minisign signature');
  requireValue(signature[0] === 0x45 && (signature[1] === 0x64 || signature[1] === 0x44), 'unsupported minisign signature algorithm');
  canonicalBase64(lines[3], 64, 'minisign global signature');
  const comment = /^trusted comment: timestamp:(0|[1-9]\d*)\tfile:([^\t\r\n]+)\tversion:([^\t\r\n]+)$/.exec(lines[2]);
  requireValue(comment && BigInt(comment[1]) <= 253402300799n, 'signature must carry one timestamp, file and signed version');
  requireValue(comment[2] === assetName(name, target) && comment[3] === parseVersion(version).version, 'signature file/version comment does not match the release');
  return value;
}

export function validateStaticManifest(value) {
  requireValue(Buffer.byteLength(JSON.stringify(value) ?? '') <= UPDATE_POLICY.maxDocumentBytes, 'manifest exceeds its document budget');
  exactKeys(value, ['version', 'notes', 'pub_date', 'platforms'], 'manifest');
  const version = parseVersion(value.version).version;
  text(value.notes, UPDATE_POLICY.maxNotesBytes, 'notes');
  publicationDate(value.pub_date);
  requireValue(value.platforms && typeof value.platforms === 'object' && !Array.isArray(value.platforms), 'platforms must be an object');
  const targets = Object.keys(value.platforms);
  requireValue(targets.length > 0 && targets.every(target => Object.hasOwn(UPDATE_TARGETS, target)), 'manifest contains an unsupported or empty platform set');
  const platforms = {};
  let manifestSource;
  for (const target of targets) {
    const entry = value.platforms[target];
    exactKeys(entry, ['url', 'signature'], 'platform entry');
    text(entry.url, 2048, 'asset URL');
    let url;
    try { url = new URL(entry.url); } catch { throw new Error('asset URL is invalid'); }
    requireValue(url.protocol === 'https:' && !url.username && !url.password && !url.search && !url.hash, 'asset URL must use fixed credential-free HTTPS');
    let name;
    try { name = decodeURIComponent(url.pathname.split('/').at(-1)); } catch { throw new Error('asset URL encoding is invalid'); }
    const source = Object.keys(DOWNLOAD_SOURCES.updates.sources).find(id => entry.url === releaseAssetUrl(version, name, { source: id, target }));
    requireValue(source !== undefined, 'asset URL must pin a registered source and exact version tag');
    requireValue(manifestSource === undefined || manifestSource === source, 'all platform artifacts in a manifest must use the same source');
    manifestSource = source;
    validateSignatureFormat(entry.signature, { version, name, target });
    platforms[target] = { url: entry.url, signature: entry.signature };
  }
  return { version, notes: value.notes, pub_date: value.pub_date, platforms };
}

export function generateStaticManifest(metadata, { previousVersion, source } = {}) {
  requireValue(Buffer.byteLength(JSON.stringify(metadata) ?? '') <= UPDATE_POLICY.maxDocumentBytes, 'metadata exceeds its document budget');
  updateSource(source);
  const entries = Array.isArray(metadata) ? metadata : [metadata];
  requireValue(entries.length > 0 && entries.length <= Object.keys(UPDATE_TARGETS).length, 'metadata must contain a supported nonempty platform set');
  const manifests = entries.map(entry => generatePlatformManifest(entry, { previousVersion, source }));
  const first = manifests[0];
  const platforms = {};
  for (const manifest of manifests) {
    requireValue(manifest.version === first.version && manifest.notes === first.notes && manifest.pub_date === first.pub_date, 'platform releases must have the same version, notes and publication date');
    for (const [target, entry] of Object.entries(manifest.platforms)) {
      requireValue(!Object.hasOwn(platforms, target), 'duplicate update platform');
      platforms[target] = entry;
    }
  }
  return validateStaticManifest({ ...first, platforms });
}

function generatePlatformManifest(metadata, { previousVersion, source }) {
  exactKeys(metadata, ['schemaVersion', 'profile', 'identifier', 'target', 'releaseRepository', 'version', 'tag', 'notes', 'pub_date', 'artifact'], 'signed artifact metadata');
  requireValue(metadata.schemaVersion === UPDATE_POLICY.schemaVersion && metadata.profile === UPDATE_POLICY.profile
    && metadata.identifier === UPDATE_POLICY.identifier && Object.hasOwn(UPDATE_TARGETS, metadata.target)
    && metadata.releaseRepository === UPDATE_POLICY.repository, 'signed artifact metadata has a different identity, profile, target or repository');
  const parsed = parseVersion(metadata.version);
  requireValue(parsed.version === metadata.version && parsed.prerelease.length === 0 && parsed.build === ''
    && compareVersions(parsed.version, UPDATE_POLICY.firstVersion) >= 0, 'stable release generation requires a canonical version at or above 0.1.0');
  requireValue(metadata.tag === `v${parsed.version}`, 'release tag does not match the version');
  if (previousVersion !== undefined) requireValue(compareVersions(parsed.version, previousVersion) > 0, 'release must be newer than the previous version');
  exactKeys(metadata.artifact, ['name', 'bytes', 'sha256', 'signature'], 'artifact');
  const artifact = metadata.artifact;
  requireValue(Number.isSafeInteger(artifact.bytes) && artifact.bytes > 0 && artifact.bytes <= UPDATE_POLICY.maxArtifactBytes, 'artifact size is outside the release budget');
  requireValue(typeof artifact.sha256 === 'string' && /^[0-9a-f]{64}$/.test(artifact.sha256), 'artifact SHA-256 is invalid');
  return validateStaticManifest({ version: parsed.version, notes: metadata.notes, pub_date: metadata.pub_date,
    platforms: { [metadata.target]: { url: releaseAssetUrl(parsed.version, artifact.name, { source, target: metadata.target }), signature: artifact.signature } } });
}

export function assessStableUpdate(manifest, currentVersion) {
  const checked = validateStaticManifest(manifest);
  const parsed = parseVersion(checked.version);
  const comparison = compareVersions(parsed.version, currentVersion);
  const state = parsed.prerelease.length ? 'prerelease-skipped' : comparison > 0 ? 'newer' : comparison < 0 ? 'older' : 'same';
  return { state, currentVersion: parseVersion(currentVersion).version, version: checked.version,
    cryptographicSignatureVerified: false, installAuthorized: false };
}

export function automaticCheckPolicy(buildInfo) {
  // No caller-supplied JSON can enable the not-yet-integrated runtime updater.
  const development = buildInfo?.profile !== UPDATE_POLICY.profile || buildInfo?.identifier !== UPDATE_POLICY.identifier;
  return { enabled: false, reason: development ? 'development-or-unknown-build' : 'runtime-updater-not-enabled' };
}

export function parseProtocolJson(bytes) {
  const raw = typeof bytes === 'string' ? text(bytes, UPDATE_POLICY.maxDocumentBytes, 'JSON document')
    : new TextDecoder('utf-8', { fatal: true }).decode(bytes);
  requireValue(Buffer.byteLength(raw) <= UPDATE_POLICY.maxDocumentBytes, 'JSON document exceeds its budget');
  // JSON.parse alone silently accepts duplicate keys. Reject them before any
  // identity or fixed-origin checks so different parsers cannot disagree.
  let cursor = 0;
  function whitespace() { while (/[\t\n\r ]/.test(raw[cursor] ?? '\0')) cursor++; }
  function string() {
    const start = cursor++;
    while (cursor < raw.length) {
      const c = raw[cursor++];
      if (c === '\\') cursor++;
      else if (c === '"') return JSON.parse(raw.slice(start, cursor));
    }
    throw new Error('JSON string is unterminated');
  }
  function visit(depth) {
    requireValue(depth <= 16, 'JSON nesting exceeds its budget'); whitespace();
    if (raw[cursor] === '{') {
      cursor++; whitespace(); const keys = new Set();
      if (raw[cursor] === '}') { cursor++; return; }
      while (cursor < raw.length) {
        whitespace(); requireValue(raw[cursor] === '"', 'JSON object key is invalid'); const key = string();
        requireValue(!keys.has(key), 'JSON contains duplicate object keys'); keys.add(key); whitespace();
        requireValue(raw[cursor++] === ':', 'JSON object separator is invalid'); visit(depth + 1); whitespace();
        const c = raw[cursor++]; if (c === '}') return; requireValue(c === ',', 'JSON object is invalid');
      }
    } else if (raw[cursor] === '[') {
      cursor++; whitespace(); if (raw[cursor] === ']') { cursor++; return; }
      while (cursor < raw.length) { visit(depth + 1); whitespace(); const c = raw[cursor++]; if (c === ']') return; requireValue(c === ',', 'JSON array is invalid'); }
    } else if (raw[cursor] === '"') string();
    else { const match = /^(?:true|false|null|-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?)/.exec(raw.slice(cursor)); requireValue(match, 'JSON value is invalid'); cursor += match[0].length; }
  }
  visit(0); whitespace(); requireValue(cursor === raw.length, 'JSON has trailing or incomplete content');
  return JSON.parse(raw);
}
