import { lstat, readdir, readFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { DOWNLOAD_SOURCES } from './download-sources.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
const targets = [
  { id: 'github', repository: 'happypandaa/panda-drawer', web: DOWNLOAD_SOURCES.releaseRepository, api: 'https://api.github.com/repos/happypandaa/panda-drawer' },
  { id: 'gitee', repository: 'mycolorfuldays/panda-drawer', web: 'https://gitee.com/mycolorfuldays/panda-drawer', api: 'https://gitee.com/api/v5/repos/mycolorfuldays/panda-drawer' },
];
const digest = bytes => createHash('sha256').update(bytes).digest('hex');

async function credentials(file) {
  const info = await lstat(file);
  if (!info.isFile() || info.isSymbolicLink() || info.size > 16 * 1024
    || process.platform !== 'win32' && ((info.mode & 0o077) !== 0 || info.uid !== process.getuid())) {
    throw new Error('Release credentials must be a private ordinary file owned by the current user.');
  }
  let value;
  try { value = JSON.parse(await readFile(file, 'utf8')); }
  catch { throw new Error('Could not read the release credentials file.'); }
  if (!targets.every(target => typeof value[target.id] === 'string' && value[target.id].trim())) {
    throw new Error('Both GitHub and Gitee release credentials are required.');
  }
  return value;
}

async function api(target, token, endpoint, { method = 'GET', data, bytes, filename } = {}, fetcher = fetch) {
  const headers = { 'User-Agent': 'panda-drawer-test-release' };
  let body;
  if (target.id === 'github') {
    headers.Authorization = 'Bearer ' + token;
    headers.Accept = 'application/vnd.github+json';
    headers['X-GitHub-Api-Version'] = '2022-11-28';
    if (bytes) { headers['Content-Type'] = 'application/octet-stream'; body = bytes; }
    else if (data) { headers['Content-Type'] = 'application/json'; body = JSON.stringify(data); }
  } else if (bytes) {
    body = new FormData();
    body.set('access_token', token);
    body.set('file', new Blob([bytes]), filename);
  } else if (data) {
    body = new URLSearchParams({ ...data, access_token: token });
  }
  const url = endpoint.startsWith('https://uploads.github.com/') ? endpoint : target.api + endpoint;
  const response = await fetcher(url, { method, headers, body, redirect: 'error', signal: AbortSignal.timeout(120_000) });
  if (response.status === 404 && method === 'GET') return null;
  if (!response.ok) throw new Error(target.id + ' release API failed with HTTP ' + response.status + ' during ' + method + '.');
  return response.json();
}

export async function verifyPublicDownload(url, expectedBytes, expectedDigest, fetcher = fetch) {
  const controller = new AbortController();
  let timer;
  const progress = () => {
    clearTimeout(timer);
    timer = setTimeout(() => controller.abort(), 30_000);
  };
  progress();
  try {
    let response;
    for (let redirects = 0; redirects <= 5; redirects++) {
      if (new URL(url).protocol !== 'https:') throw new Error('Release downloads must use HTTPS.');
      response = await fetcher(url, { redirect: 'manual', signal: controller.signal });
      if (![301, 302, 303, 307, 308].includes(response.status)) break;
      const location = response.headers.get('location');
      if (!location || redirects === 5) throw new Error('Release download redirect limit exceeded.');
      await response.body?.cancel();
      url = new URL(location, url).href;
    }
    if (!response?.ok || !response.body) throw new Error('Public release download failed.');
    const hash = createHash('sha256');
    let total = 0;
    for await (const chunk of response.body) {
      progress();
      total += chunk.byteLength;
      if (total > expectedBytes) throw new Error('Public release download exceeded the expected size.');
      hash.update(chunk);
    }
    if (total !== expectedBytes || hash.digest('hex') !== expectedDigest) {
      throw new Error('Public release download did not match its size and SHA-256.');
    }
  } finally { clearTimeout(timer); }
}

export async function publishTestRelease({ tag, commit, assetsDirectory, credentialsPath }, { fetcher = fetch } = {}) {
  if (!/^download-routes-test-[0-9]{8}(?:-[A-Za-z0-9.-]+)?$/.test(tag) || !/^[0-9a-f]{40}$/.test(commit)) {
    throw new Error('Use an explicit download-routes-test-YYYYMMDD tag and full commit SHA.');
  }
  if (!path.isAbsolute(assetsDirectory) || !path.isAbsolute(credentialsPath)) throw new Error('Release inputs must use absolute paths.');
  const keys = await credentials(credentialsPath);
  const assets = [];
  for (const name of (await readdir(assetsDirectory)).sort()) {
    const file = path.join(assetsDirectory, name);
    const info = await lstat(file);
    if (!info.isFile() || info.isSymbolicLink() || !/^[\p{L}\p{N}][\p{L}\p{N}._ -]*$/u.test(name) || name.includes('..')) {
      throw new Error('Release assets must be ordinary files with plain filenames.');
    }
    if (info.size === 0 || info.size > 100_000_000) throw new Error('A test asset exceeds the Gitee 100 MB single-file limit.');
    const bytes = await readFile(file);
    assets.push({ name, bytes, sha256: digest(bytes) });
  }
  if (!assets.length) throw new Error('At least one test release asset is required.');
  const result = { tag, commit, releases: [] };
  for (const target of targets) {
    const token = keys[target.id];
    let release = await api(target, token, '/releases/tags/' + encodeURIComponent(tag), {}, fetcher);
    if (!release) {
      const data = {
        tag_name: tag, name: '下载线路测试 · ' + tag, target_commitish: commit,
        body: '测试开发版下载线路。GitHub 与 Gitee 使用相同提交和附件，文件按 SHA-256 验证。正式自动更新清单与安装验收另行记录。',
        prerelease: target.id === 'gitee' ? 'true' : true,
      };
      if (target.id === 'github') data.make_latest = 'false';
      release = await api(target, token, '/releases', { method: 'POST', data }, fetcher);
    }
    if (release.tag_name !== tag || ![true, 'true'].includes(release.prerelease) || !Number.isSafeInteger(release.id) || release.id <= 0) {
      throw new Error('An existing release is not the requested prerelease.');
    }
    const existing = target.id === 'github' ? release.assets ?? []
      : await api(target, token, '/releases/' + release.id + '/attach_files?per_page=100', {}, fetcher) ?? [];
    const record = { source: target.id, url: target.web + '/releases/tag/' + encodeURIComponent(tag), assets: [] };
    for (const asset of assets) {
      if (!existing.some(item => (item.name ?? item.filename) === asset.name)) {
        const endpoint = target.id === 'github'
          ? 'https://uploads.github.com/repos/' + target.repository + '/releases/' + release.id + '/assets?name=' + encodeURIComponent(asset.name)
          : '/releases/' + release.id + '/attach_files';
        await api(target, token, endpoint, { method: 'POST', bytes: asset.bytes, filename: asset.name }, fetcher);
      }
      const url = target.web + '/releases/download/' + encodeURIComponent(tag) + '/' + encodeURIComponent(asset.name);
      await verifyPublicDownload(url, asset.bytes.length, asset.sha256, fetcher);
      record.assets.push({ name: asset.name, url, bytes: asset.bytes.length, sha256: asset.sha256, verified: true });
    }
    result.releases.push(record);
  }
  return result;
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  try {
    const args = process.argv.slice(2);
    if (args.length !== 6 || args[0] !== '--tag' || args[2] !== '--assets' || args[4] !== '--credentials') {
      throw new Error('Usage: node scripts/publish-test-release.mjs --tag download-routes-test-YYYYMMDD-1 --assets /absolute/assets --credentials /absolute/private.json');
    }
    const tag = args[1];
    const commit = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim();
    const tagged = execFileSync('git', ['rev-parse', 'refs/tags/' + tag + '^{commit}'], { cwd: root, encoding: 'utf8' }).trim();
    if (tagged !== commit) throw new Error('The test tag must point to the current commit and be pushed to both repositories first.');
    console.log(JSON.stringify(await publishTestRelease({ tag, commit, assetsDirectory: args[3], credentialsPath: args[5] }), null, 2));
  } catch (error) {
    console.error(error instanceof Error ? error.message : 'Test release failed.');
    process.exitCode = 1;
  }
}
