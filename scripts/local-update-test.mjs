import { execFile, spawnSync } from 'node:child_process';
import { createReadStream } from 'node:fs';
import { lstat, mkdir, mkdtemp, readFile, realpath, rename, rm, rmdir, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { promisify } from 'node:util';
import { prepareDesktop, validUpdateTestReleaseTag } from './prepare-desktop.mjs';
import { root, run, rustEnvironment } from './process.mjs';

const execute = promisify(execFile);
const host = '127.0.0.1';
const port = 18427;
const local = {
  identifier: 'test.happypandaa.pandadrawer.update',
  name: '熊猫小抽屉 · 更新测试版',
  kitPrefix: 'panda-local-update-',
};
const versions = ['0.1.0', '0.1.1'];
const next = versions[1];
const asset = `PandaDrawer_${next}_aarch64.app.tar.gz`;
const assetRoute = `/v${next}/${asset}`;
const targetDirectory = path.join(root, 'target');
const kitParent = path.join(root, '.test-artifacts');
const tauri = path.join(root, 'node_modules', '@tauri-apps', 'cli', 'tauri.js');

function signer(args) {
  // Even temporary key generation may print private material. Keep its
  // console output out of build logs and out of error messages.
  const result = spawnSync(process.execPath, [tauri, 'signer', ...args], {
    env: { PATH: process.env.PATH, CI: 'true' },
    timeout: 90_000,
    maxBuffer: 1_000_000,
  });
  if (result.status !== 0) throw new Error('本机测试密钥生成或更新包签名失败。');
}

async function verifyApp(app, version, profile) {
  const { stdout } = await execute('/usr/bin/plutil', ['-convert', 'json', '-o', '-', path.join(app, 'Contents', 'Info.plist')]);
  const info = JSON.parse(stdout);
  if (info.CFBundleIdentifier !== profile.identifier || info.CFBundleShortVersionString !== version) {
    throw new Error('测试应用的标识或版本与构建不一致。');
  }
  const program = path.join(app, 'Contents', 'MacOS', 'local-projects-dev');
  const sidecar = JSON.parse((await execute(program, ['build-info', '--json'])).stdout);
  if (!sidecar.ok || sidecar.data.profile !== 'local-update-test'
    || sidecar.data.identifier !== profile.identifier || sidecar.data.version !== version
    || sidecar.data.updateTestReleaseTag !== profile.releaseTag) {
    throw new Error('测试应用内置 CLI 与试验版身份不一致。');
  }
  await execute('/usr/bin/codesign', ['--verify', '--deep', '--strict', app], { timeout: 60_000 });
}

async function bundle(version, publicKey, profile, generated) {
  await prepareDesktop({ localUpdateTestVersion: version, localUpdateTestReleaseTag: profile.releaseTag });
  const config = {
    productName: profile.name,
    version,
    identifier: profile.identifier,
    build: { beforeBuildCommand: 'pnpm run build' },
    app: { windows: [{
      label: 'manager', create: false, title: profile.name,
      width: 960, height: 640, minWidth: 640, minHeight: 480, backgroundColor: '#F7F7F5',
    }] },
    bundle: {
      targets: ['app'],
      createUpdaterArtifacts: false,
      externalBin: ['binaries/local-update-test/local-projects-dev'],
      macOS: { signingIdentity: '-' },
    },
  };
  const environment = rustEnvironment();
  for (const key of Object.keys(environment)) {
    if (/^(TAURI_(CONFIG|SIGNING_)|APPLE_|CARGO_BUILD_TARGET$)/.test(key)) delete environment[key];
  }
  delete environment.PANDA_DRAWER_UPDATE_TEST_RELEASE_TAG;
  if (profile.releaseTag) environment.PANDA_DRAWER_UPDATE_TEST_RELEASE_TAG = profile.releaseTag;
  await run(process.execPath, [
    tauri, 'build', '--debug', '--features', 'local-update-test', '--bundles', 'app',
    '--config', JSON.stringify(config), '--ci', '--', '--locked',
  ], {
    cwd: path.join(root, 'apps', 'desktop'),
    env: {
      ...environment,
      CARGO_TARGET_DIR: targetDirectory,
      CARGO_BUILD_JOBS: '2',
      CARGO_PROFILE_DEV_DEBUG: '0',
      CARGO_PROFILE_TEST_DEBUG: '0',
      CARGO_INCREMENTAL: '0',
      PANDA_DRAWER_UPDATE_TEST_VERSION: version,
      PANDA_DRAWER_UPDATE_PUBLIC_KEY: publicKey,
    },
  });
  await verifyApp(generated, version, profile);
}

async function build(profile) {
  if (process.platform !== 'darwin' || process.arch !== 'arm64') {
    throw new Error('本机更新测试只支持 Apple Silicon Mac。');
  }
  await mkdir(kitParent, { recursive: true });
  if (await realpath(kitParent) !== kitParent) throw new Error('测试包输出目录不能通过符号链接定位。');
  const lock = path.join(kitParent, 'local-update-test-build.lock');
  await mkdir(lock);
  const generated = path.join(targetDirectory, 'debug', 'bundle', 'macos', `${profile.name}.app`);
  let secret, kit, started;
  try {
    const existing = await lstat(generated).catch(error => {
      if (error.code !== 'ENOENT') throw error;
    });
    if (existing) throw new Error(`已有未整理的本机测试应用包：${generated}`);
    secret = await mkdtemp(path.join(await realpath(os.tmpdir()), 'panda-update-test-key-'));
    kit = await mkdtemp(path.join(kitParent, profile.kitPrefix));
    const privateKey = path.join(secret, 'fixture.key');
    signer(['generate', '--ci', '--password', '', '--write-keys', privateKey]);
    const publicKey = (await readFile(`${privateKey}.pub`, 'utf8')).trim();

    started = true;
    await bundle(versions[0], publicKey, profile, generated);
    const app = path.join(kit, `${profile.name}.app`);
    await rename(generated, app);
    await bundle(next, publicKey, profile, generated);

    const assets = profile.releaseTag ? path.join(kit, 'assets') : kit;
    if (profile.releaseTag) await mkdir(assets);
    const archive = path.join(assets, asset);
    await run('/usr/bin/tar', ['-czf', archive, '-C', path.dirname(generated), path.basename(generated)], {
      env: { ...process.env, COPYFILE_DISABLE: '1' },
    });
    signer(['sign', '--password', '', '--private-key-path', privateKey, '--app-version', next, archive]);
    const signature = (await readFile(`${archive}.sig`, 'utf8')).trim();
    const manifest = {
      version: next,
      notes: `${profile.releaseTag ? 'GitHub 线上' : '本机'}更新测试 · 验证下载确认、后台进度和安装重启。`,
      pub_date: new Date().toISOString(),
      platforms: { 'darwin-aarch64': {
        url: profile.assetBase ? `${profile.assetBase}/${asset}` : `http://${host}:${port}${assetRoute}`,
        signature,
      } },
    };
    await writeFile(path.join(assets, 'latest.json'), JSON.stringify(manifest, null, 2) + '\n', { flag: 'wx' });
    const result = { kit, app, archive, versions };
    if (profile.releaseTag) {
      const publicKeyPath = path.join(kit, 'update-test.pub');
      await writeFile(publicKeyPath, publicKey + '\n', { flag: 'wx' });
      Object.assign(result, {
        assets,
        manifestUrl: `${profile.assetBase}/latest.json`,
        publicKey: publicKeyPath,
        releaseTag: profile.releaseTag,
      });
    }
    console.log(JSON.stringify(result, null, 2));
  } catch (error) {
    if (kit) await rm(kit, { recursive: true, force: true });
    throw error;
  } finally {
    if (secret) await rm(secret, { recursive: true, force: true });
    if (started) {
      await rm(generated, { recursive: true, force: true });
      await rm(path.join(root, 'apps/desktop/src-tauri/binaries/local-update-test'), { recursive: true, force: true });
    }
    await rmdir(lock);
  }
}

async function serve(input) {
  const kit = await realpath(input);
  if (path.dirname(kit) !== kitParent || !path.basename(kit).startsWith(local.kitPrefix)) {
    throw new Error('请使用本机构建命令创建的测试包目录。');
  }
  const archive = path.join(kit, asset);
  const manifest = await readFile(path.join(kit, 'latest.json'));
  const info = await lstat(archive);
  if (!info.isFile() || info.isSymbolicLink()) throw new Error('更新归档必须是普通文件。');
  const server = createServer((request, response) => {
    if (request.method === 'GET' && request.url === '/latest.json') {
      response.writeHead(200, { 'content-type': 'application/json', 'content-length': manifest.length, 'cache-control': 'no-store' });
      response.end(manifest);
    } else if (request.method === 'GET' && request.url === assetRoute) {
      response.writeHead(200, { 'content-type': 'application/octet-stream', 'content-length': info.size });
      const stream = createReadStream(archive);
      stream.once('error', error => response.destroy(error));
      response.once('close', () => stream.destroy());
      stream.pipe(response);
    } else {
      response.writeHead(404).end();
    }
    console.log(`${request.method} ${request.url} ${response.statusCode}`);
  });
  server.once('error', error => { console.error(error.message); process.exitCode = 1; });
  server.listen(port, host, () => console.log(`本机更新源：http://${host}:${port}/latest.json`));
}

try {
  const [mode, value, ...extra] = process.argv.slice(2);
  if (mode === 'build' && value === undefined) {
    await build(local);
  } else if (mode === 'build-online' && !extra.length && validUpdateTestReleaseTag(value)) {
    await build({
      identifier: 'test.happypandaa.pandadrawer.online-update',
      name: '熊猫小抽屉 · 线上更新测试版',
      kitPrefix: 'panda-online-update-',
      releaseTag: value,
      assetBase: `https://github.com/happypandaa/panda-drawer/releases/download/${value}`,
    });
  } else if (mode === 'serve' && value && !extra.length) {
    await serve(value);
  } else {
    throw new Error('用法：node scripts/local-update-test.mjs build、build-online <auto-update-test-YYYYMMDD-HHMMSS> 或 serve <本机测试包目录>');
  }
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}
