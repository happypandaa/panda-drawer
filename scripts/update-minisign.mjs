import { createHash, createPublicKey } from 'node:crypto';

export function decodeBase64(value, length, label) {
  if (typeof value !== 'string' || !value.length || !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(value)) {
    throw new Error(`${label} must be canonical base64.`);
  }
  const bytes = Buffer.from(value, 'base64');
  if (bytes.toString('base64') !== value || (length !== undefined && bytes.length !== length)) {
    throw new Error(`${label} has an invalid length.`);
  }
  return bytes;
}

export function minisignAlgorithm(bytes) {
  if (bytes[0] !== 0x45 || (bytes[1] !== 0x64 && bytes[1] !== 0x44)) throw new Error('Unsupported minisign algorithm.');
  return bytes[1] === 0x44 ? 'ED' : 'Ed';
}

export function parseUpdatePublicKey(encoded) {
  const text = new TextDecoder('utf-8', { fatal: true }).decode(decodeBase64(encoded, undefined, 'Public key'));
  const lines = text.replace(/\n$/, '').split('\n');
  if (lines.length !== 2 || !/^untrusted comment: [^\x00-\x1f\x7f]{1,200}$/.test(lines[0])) {
    throw new Error('Public key must contain exactly one Tauri minisign public-key box.');
  }
  const packet = decodeBase64(lines[1], 42, 'Public-key packet');
  minisignAlgorithm(packet);
  const key = createPublicKey({ key: Buffer.concat([Buffer.from('302a300506032b6570032100', 'hex'), packet.subarray(10)]), format: 'der', type: 'spki' });
  return { key, id: packet.subarray(2, 10), sha256: createHash('sha256').update(packet).digest('hex') };
}
