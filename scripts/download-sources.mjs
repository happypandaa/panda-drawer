import { readFileSync } from 'node:fs';

// This registry ships with the application. Network responses, environment
// variables and CLI arguments cannot add a trusted download origin.
export const DOWNLOAD_SOURCES = JSON.parse(readFileSync(new URL('../distribution/download-sources.json', import.meta.url), 'utf8'));

function httpsUrl(value) {
  const url = new URL(value);
  if (url.protocol !== 'https:' || url.username || url.password || url.search || url.hash || url.href !== value) {
    throw new Error('Download sources must be canonical, credential-free HTTPS URLs.');
  }
  return value;
}

if (DOWNLOAD_SOURCES.schemaVersion !== 1) throw new Error('Unsupported download source registry.');
httpsUrl(DOWNLOAD_SOURCES.releaseRepository);
for (const group of ['catalog', 'updates']) {
  const configuration = DOWNLOAD_SOURCES[group];
  if (!configuration || !Object.hasOwn(configuration.sources ?? {}, configuration.primary)) {
    throw new Error('The primary download source must be registered.');
  }
  for (const [id, source] of Object.entries(configuration.sources)) {
    if (!/^[a-z][a-z0-9-]*$/.test(id)) throw new Error('Invalid download source ID.');
    if (group === 'catalog') httpsUrl(source);
    else {
      httpsUrl(source.manifestUrl);
      httpsUrl(source.artifactBaseUrl);
      if (source.artifactBaseUrl.endsWith('/')) throw new Error('Artifact bases must not end in a slash.');
    }
    if (source && typeof source === 'object') Object.freeze(source);
  }
  Object.freeze(configuration.sources);
  Object.freeze(configuration);
}
Object.freeze(DOWNLOAD_SOURCES);

export function updateSource(id = DOWNLOAD_SOURCES.updates.primary) {
  if (!Object.hasOwn(DOWNLOAD_SOURCES.updates.sources, id)) throw new Error('Unregistered update source.');
  return DOWNLOAD_SOURCES.updates.sources[id];
}

export function updateEndpoints(ids = [DOWNLOAD_SOURCES.updates.primary]) {
  if (!Array.isArray(ids) || ids.length === 0) throw new Error('At least one registered update source is required.');
  return [...new Set(ids)].map(id => updateSource(id).manifestUrl);
}
