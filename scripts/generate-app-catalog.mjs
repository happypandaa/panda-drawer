// Read-only author manifests + hand-written descriptions -> publishable catalog.
// pnpm generate:app-catalog --manifest-root /path/to/apps --output /path/to/apps.json
// Each input app may instead set `manifest` to a local panda.json path (relative
// to the input file) or an HTTPS URL. No application source is changed or cloned.
import { readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const usage = 'Usage: pnpm generate:app-catalog [--input file] [--manifest-root directory] [--output file]';
const options = {};
for (let i = 2; i < process.argv.length; i++) {
  const argument = process.argv[i];
  if (argument === '--help') { console.log(usage); process.exit(0); }
  if (!['--input', '--manifest-root', '--output'].includes(argument) || options[argument] !== undefined
    || !process.argv[i + 1] || process.argv[i + 1].startsWith('--')) throw new Error(usage);
  options[argument] = process.argv[++i];
}

function text(value, label) {
  if (typeof value !== 'string' || !value.trim()) throw new Error(`${label} must be a non-empty string.`);
  return value.trim();
}

function agentDeclaration(value, id) {
  if (value === undefined) return false;
  const instructions = value?.instructions;
  if (!value || typeof value !== 'object' || Array.isArray(value)
    || Object.keys(value).length !== 1 || typeof instructions !== 'string'
    || !instructions.trim() || Buffer.byteLength(instructions) > 1024
    || instructions.includes('\\') || instructions.includes(':') || instructions.includes('\0')
    || instructions.split('/').some(part => !part || part.startsWith('.'))) {
    throw new Error(`${id}: agent must contain only a valid project-relative instructions file path.`);
  }
  // The downloaded application's manifest inspection checks file existence
  // and resolved boundaries; catalog generation never clones application code.
  return true;
}

const manifestLimit = 64 * 1024;
async function readManifest(location) {
  let bytes;
  if (/^[a-z][a-z\d+.-]*:/i.test(location) && !path.isAbsolute(location)) {
    const url = new URL(location);
    if (url.protocol !== 'https:' || url.username || url.password) throw new Error('Manifest URLs must use HTTPS without credentials.');
    const response = await fetch(url, { signal: AbortSignal.timeout(10_000), redirect: 'error' });
    if (!response.ok) throw new Error(`Manifest request failed: HTTP ${response.status}`);
    const chunks = []; let length = 0;
    for await (const chunk of response.body ?? []) {
      length += chunk.byteLength;
      if (length > manifestLimit) throw new Error('Manifest exceeds 64 KiB.');
      chunks.push(chunk);
    }
    bytes = Buffer.concat(chunks);
  } else {
    bytes = await readFile(location);
  }
  if (bytes.byteLength > manifestLimit) throw new Error('Manifest exceeds 64 KiB.');
  return JSON.parse(bytes.toString('utf8'));
}

const inputPath = path.resolve(options['--input'] ?? fileURLToPath(new URL('./app-catalog-input.json', import.meta.url)));
const input = JSON.parse(await readFile(inputPath, 'utf8'));
if (input.version !== 1 || !Array.isArray(input.apps)) throw new Error('Catalog input must have version: 1 and an apps array.');
const manifestRoot = options['--manifest-root'] ? path.resolve(options['--manifest-root']) : null;
const ids = new Set();
const apps = [];
for (const entry of input.apps) {
  const id = text(entry.id, 'id');
  if (ids.has(id)) throw new Error(`Duplicate app id: ${id}`);
  ids.add(id);
  const repo = {};
  const addresses = typeof entry.repo === 'string'
    ? { [entry.repo.startsWith('https://gitee.com/') ? 'gitee' : 'github']: entry.repo }
    : entry.repo;
  if (!addresses || typeof addresses !== 'object' || Array.isArray(addresses)) throw new Error(`${id}.repo must contain application addresses.`);
  let repository;
  for (const provider of ['github', 'gitee']) {
    if (addresses[provider] === undefined) continue;
    const url = text(addresses[provider], `${id}.repo.${provider}`);
    const match = /^https:\/\/(github\.com|gitee\.com)\/[A-Za-z0-9][A-Za-z0-9._-]*\/([A-Za-z0-9][A-Za-z0-9._-]*?)\/?$/.exec(url);
    if (!match || match[1] !== `${provider}.com`) throw new Error(`${id}.repo.${provider} must use its own HTTPS provider.`);
    repo[provider] = url;
    repository ??= match[2].replace(/\.git$/, '');
  }
  if (!repository) throw new Error(`${id}.repo must contain at least one application address.`);
  if (entry.enabled !== undefined && typeof entry.enabled !== 'boolean') throw new Error(`${id}.enabled must be a boolean.`);
  if (entry.enabled === false) continue;
  let location;
  if (manifestRoot) {
    location = path.join(manifestRoot, repository, 'panda.json');
  } else if (entry.manifest !== undefined) {
    const source = text(entry.manifest, `${id}.manifest`);
    location = /^[a-z][a-z\d+.-]*:\/\//i.test(source) ? source : path.resolve(path.dirname(inputPath), source);

  } else {
    throw new Error(`Provide --manifest-root or set ${id}.manifest to a panda.json path or HTTPS URL.`);
  }
  const manifest = await readManifest(location);
  const agent = agentDeclaration(manifest.agent, id);
  const fields = manifest.configuration?.fields ?? [];
  if (!Array.isArray(fields)) throw new Error(`${id}: configuration.fields must be an array.`);
  const needs = [...new Set(fields.filter(field => field?.required === true).map(field => text(field.label, `${id}: required field label`)))];
  const app = {
    id,
    name: text(entry.name ?? manifest.app?.name, `${id}.name`),
    summary: text(entry.summary, `${id}.summary`),
    repo,
    ...(entry.branch === undefined ? {} : { branch: text(entry.branch, `${id}.branch`) }),
    needs,
    ...(agent ? { agent: true } : {}),
    ...(entry.note === undefined ? {} : { note: text(entry.note, `${id}.note`) }),
  };
  apps.push(app);
}
const output = `${JSON.stringify({ version: 1, apps }, null, 2)}\n`;
if (options['--output']) await writeFile(path.resolve(options['--output']), output);
else process.stdout.write(output);
