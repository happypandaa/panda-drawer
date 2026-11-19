// Publish only immutable trial objects under the compiled-in CDN test prefix.
// ossutil owns credential loading and request signing. Never print its raw diagnostics.
import { execFile } from 'node:child_process';
import path from 'node:path';
import { promisify } from 'node:util';
import { verifyPublicDownload } from './publish-test-release.mjs';

const execute = promisify(execFile);
class OssTrialError extends Error { name = 'OssTrialError'; }

async function ossApi(operation, args, cwd) {
  try {
    const { stdout } = await execute(process.platform === 'win32' ? 'ossutil.exe' : 'ossutil',
      ['api', operation, ...args, '--quiet', '--loglevel', 'off', '--retry-times', '0', '--output-format', 'raw'], {
        // ossutil enforces connection and read timeouts. A progressing upload can
        // take longer than twenty minutes on a cross-region connection.
        cwd, windowsHide: true, timeout: operation === 'put-object' ? 0 : 30_000,
        maxBuffer: 128 * 1024, encoding: 'utf8',
      });
    return stdout;
  } catch (error) {
    const diagnostic = `${error.stderr ?? ''}\n${error.stdout ?? ''}`;
    if (operation === 'head-object' && /\bNoSuchKey\b|\b404 Not Found\b|status[ _-]?code\s*[:=,]\s*404\b/i.test(diagnostic)) return null;
    if (operation === 'put-object' && /\bFileAlreadyExists\b/.test(diagnostic)) return null;
    throw new OssTrialError(error.code === 'ENOENT' ? 'Install or locate the official ossutil v2 executable.'
      : `OSS ${operation} failed; check bucket, ossutil profile, permissions and connectivity. An interrupted trial can be resumed with the original kit.`);
  }
}

async function checkPublic(asset) {
  try {
    await verifyPublicDownload(asset.url, asset.size, asset.sha256, (url, options) => {
      if (url !== asset.url) throw new Error('Unexpected CDN redirect.');
      return fetch(url, options);
    });
  } catch {
    throw new OssTrialError(`The public CDN bytes did not match the kit at ${asset.url}; existing objects are preserved.`);
  }
}

export async function publishOssTrial({ tag, assets, assertUnchanged },
  { api = ossApi, verify = checkPublic, bucket = process.env.PANDA_UPDATE_OSS_BUCKET } = {}) {
  if (!/^[a-z0-9][a-z0-9-]{1,61}[a-z0-9]$/.test(bucket ?? '')) {
    throw new OssTrialError('Set PANDA_UPDATE_OSS_BUCKET to the existing CDN origin bucket.');
  }
  if (!/^auto-update-(?:oss|dual)-test-\d{8}-\d{6}$/.test(tag) || assets.length !== 3) {
    throw new OssTrialError('Invalid OSS trial tag or asset set.');
  }
  const [archive, signature, manifest] = assets;
  if (!['PandaDrawer_0.1.1_aarch64.app.tar.gz', 'PandaDrawer_0.1.1_x64-setup.exe'].includes(archive.name)
    || signature.name !== `${archive.name}.sig` || manifest.name !== 'latest.json') {
    throw new OssTrialError('The OSS trial must contain the package, signature and manifest in that order.');
  }
  const directory = path.dirname(archive.path);
  const prefix = `_check/${tag}/`;
  const argsFor = asset => ['--bucket', bucket, '--key', prefix + asset.name];
  for (const asset of assets) {
    if (asset.url !== `https://panda-updates.zhimibuhui.com/${prefix}${asset.name}`
      || path.join(directory, asset.name) !== asset.path) throw new OssTrialError('Invalid trial asset location.');
  }

  // OSS documents that forbid-overwrite is ignored for both Enabled and
  // Suspended versioning. Only the default, empty versioning configuration
  // provides the required create-only semantics.
  const xml = await api('get-bucket-versioning', ['--bucket', bucket], directory);
  if (!/^\s*(?:<\?xml\s+[^?]*\?>\s*)?<VersioningConfiguration(?:\s+xmlns="[^"]*")?\s*(?:\/>|>\s*<\/VersioningConfiguration>)\s*$/.test(xml)) {
    throw new OssTrialError('OSS must confirm bucket versioning has never been enabled for create-only uploads.');
  }

  // Inspect every existing object before adding any. A completed older
  // manifest (including one for the other platform) cannot acquire new assets.
  const exists = new Set();
  for (const asset of [manifest, archive, signature]) {
    if (await api('head-object', argsFor(asset), directory) !== null) {
      await verify(asset);
      exists.add(asset.name);
    }
  }
  if (exists.has(manifest.name) && exists.size !== assets.length) {
    throw new OssTrialError('Published OSS trial has missing assets. Preserve it and use a new batch.');
  }
  for (const asset of assets) {
    if (exists.has(asset.name)) continue;
    await assertUnchanged(asset);
    await api('put-object', [...argsFor(asset), '--body', `file://${asset.name}`, '--forbid-overwrite=true'], directory);
    await assertUnchanged(asset);
    // Also resolves an identical concurrent upload. On conflict or delayed
    // CDN propagation leave all remote bytes untouched and allow a rerun.
    await verify(asset);
  }
}
