import { constants } from 'node:fs';
import { lstat, open } from 'node:fs/promises';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { UPDATE_POLICY, assessStableUpdate, generateStaticManifest, parseProtocolJson, validateStaticManifest } from './update-manifest-protocol.mjs';

export function parseManifestArguments(args) {
  const [command, ...flags] = args;
  if (command === '--help' && flags.length === 0) return { command: 'help' };
  if (!['generate', 'validate'].includes(command)) throw new Error('Use generate or validate; --help shows the offline interface.');
  const values = {};
  const allowed = command === 'generate' ? ['--input', '--output', '--previous-version'] : ['--input', '--current-version'];
  for (let i = 0; i < flags.length; i += 2) {
    const flag = flags[i]; const value = flags[i + 1];
    if (!allowed.includes(flag) || values[flag] !== undefined || !value || value.startsWith('--')) throw new Error('Missing, duplicate or unsupported manifest option.');
    values[flag] = value;
  }
  if (!values['--input'] || command === 'generate' && !values['--output']) throw new Error('An explicit input and, for generate, a new output are required.');
  return { command, input: values['--input'], ...(values['--output'] ? { output: values['--output'] } : {}),
    ...(values['--previous-version'] ? { previousVersion: values['--previous-version'] } : {}),
    ...(values['--current-version'] ? { currentVersion: values['--current-version'] } : {}) };
}

async function checkedPath(value, output = false) {
  if (!path.isAbsolute(value) || value.split(path.sep).some(part => part === '..') || value.includes('\0')) throw new Error('Manifest paths must be absolute and contain no parent traversal.');
  const target = path.normalize(value);
  const parent = path.dirname(target);
  let current = path.parse(parent).root;
  for (const part of parent.slice(current.length).split(path.sep).filter(Boolean)) {
    current = path.join(current, part);
    const info = await lstat(current);
    if (!info.isDirectory() || info.isSymbolicLink()) throw new Error('Manifest parent paths must be existing ordinary directories.');
  }
  if (!output) {
    const info = await lstat(target);
    if (!info.isFile() || info.isSymbolicLink() || info.nlink !== 1 || info.size > UPDATE_POLICY.maxDocumentBytes) throw new Error('Manifest input must be one bounded ordinary JSON file.');
  }
  return target;
}

async function readDocument(input) {
  const target = await checkedPath(input);
  const file = await open(target, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const before = await file.stat();
    if (!before.isFile() || before.nlink !== 1 || before.size > UPDATE_POLICY.maxDocumentBytes) throw new Error('Manifest input is invalid or too large.');
    const buffer = Buffer.alloc(UPDATE_POLICY.maxDocumentBytes + 1);
    let length = 0;
    while (length < buffer.length) {
      const { bytesRead } = await file.read(buffer, length, buffer.length - length, length);
      if (!bytesRead) break;
      length += bytesRead;
    }
    const after = await file.stat();
    if (length > UPDATE_POLICY.maxDocumentBytes || length !== before.size || ['dev', 'ino', 'mode', 'size', 'mtimeMs', 'ctimeMs', 'nlink'].some(key => before[key] !== after[key])) throw new Error('Manifest input changed during its bounded read.');
    return parseProtocolJson(buffer.subarray(0, length));
  } finally { await file.close(); }
}

export async function runManifestCommand(options) {
  if (options.command === 'help') return { help: [
    'Offline only: no signing, network, publishing, runtime admission or installation.',
    'generate --input /absolute/signed-artifact-metadata.json --output /new/latest.json [--previous-version 0.1.0]',
    'validate --input /absolute/latest.json [--current-version 0.1.0]',
    'Validation checks signature format only; it does not authenticate the artifact or its metadata.',
  ].join('\n') };
  const document = await readDocument(options.input);
  const manifest = options.command === 'generate'
    ? generateStaticManifest(document, { previousVersion: options.previousVersion }) : validateStaticManifest(document);
  let output;
  if (options.command === 'generate') {
    output = await checkedPath(options.output, true);
    const file = await open(output, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600);
    try { await file.writeFile(`${JSON.stringify(manifest, null, 2)}\n`); await file.sync(); }
    finally { await file.close(); }
  }
  return { valid: true, scope: 'metadata-format-and-fixed-origin', cryptographicSignatureVerified: false,
    artifactBytesVerified: false, installAuthorized: false, ...(output ? { output } : {}), manifest,
    ...(options.currentVersion ? { assessment: assessStableUpdate(manifest, options.currentVersion) } : {}) };
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  try { const result = await runManifestCommand(parseManifestArguments(process.argv.slice(2))); console.log(result.help ?? JSON.stringify(result, null, 2)); }
  catch (error) { console.error(error.message); process.exitCode = 1; }
}
