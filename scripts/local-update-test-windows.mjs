import { execFile } from 'node:child_process';
import { constants } from 'node:fs';
import { copyFile, mkdir, readFile, realpath, rm } from 'node:fs/promises';
import path from 'node:path';
import { promisify } from 'node:util';
import { desktopInvocation } from './desktop-host.mjs';
import { prepareDesktop } from './prepare-desktop.mjs';
import { stageWindowsCli } from './prepare-desktop-windows.mjs';
import { resolvePnpmCommand } from './pnpm-command.mjs';
import { root, run, rustEnvironment } from './process.mjs';

const execute = promisify(execFile);
export async function bundleWindowsTrial({ versions, publicKey, profile, outputForVersion }) {
  if (process.platform !== 'win32' || process.arch !== 'x64') {
    throw new Error('Windows 更新试验需要 Windows x64。');
  }
  // Apply the same limits to preparer subprocesses and the two desktop builds.
  Object.assign(process.env, {
    CARGO_BUILD_JOBS: '2', CARGO_PROFILE_DEV_DEBUG: '0', CARGO_PROFILE_TEST_DEBUG: '0', CARGO_INCREMENTAL: '0',
  });
  const [initial] = versions;
  const desktop = path.join(root, 'apps/desktop');
  const pnpm = await resolvePnpmCommand({});
  await run(pnpm.executable, [...pnpm.prefix, 'run', 'build'], { cwd: desktop, windowsHide: true });
  const localUpdateTestReleaseTag = profile.releaseTag;
  const prepared = await prepareDesktop({ localUpdateTestVersion: initial, localUpdateTestReleaseTag });
  const base = JSON.parse(await readFile(path.join(root, 'apps/desktop/src-tauri/tauri.conf.json'), 'utf8'));
  const environment = rustEnvironment();
  for (const key of Object.keys(environment)) {
    if (/^TAURI_(CONFIG|SIGNING_)/.test(key)) delete environment[key];
  }
  delete environment.PANDA_DRAWER_UPDATE_TEST_RELEASE_TAG;
  if (localUpdateTestReleaseTag) environment.PANDA_DRAWER_UPDATE_TEST_RELEASE_TAG = localUpdateTestReleaseTag;
  const mainBinaryName = `panda-drawer-${profile.identifier.split('.').at(-1)}-trial`;
  const output = path.join(prepared.targetDirectory, prepared.target, 'debug', 'bundle', 'nsis');
  await mkdir(output, { recursive: true });
  for (const version of versions) {
    const resources = version === initial ? prepared.configuration : await stageWindowsCli(prepared, { localUpdateTestVersion: version, localUpdateTestReleaseTag });
    const stagedCli = `${resources.bundle.externalBin[0]}-${prepared.target}.exe`;
    const { stdout } = await execute(stagedCli, ['build-info', '--json'], { windowsHide: true, timeout: 30_000 });
    const metadata = JSON.parse(stdout);
    if (!metadata.ok || metadata.data.identifier !== profile.identifier || metadata.data.profile !== 'local-update-test'
      || metadata.data.version !== version || metadata.data.updateTestReleaseTag !== localUpdateTestReleaseTag || metadata.data.target !== prepared.target) {
      throw new Error('Windows 测试包内置 CLI 的身份或版本与安装包不一致。');
    }
    const configuration = {
      productName: profile.name, mainBinaryName, identifier: profile.identifier, version,
      build: { beforeBuildCommand: '' },
      app: { windows: base.app.windows.map(window => ({ ...window, title: profile.name })) },
      bundle: { ...resources.bundle, targets: ['nsis'], createUpdaterArtifacts: false },
    };
    const invocation = desktopInvocation({
      root, configuration, targetDirectory: prepared.targetDirectory, target: prepared.target,
      args: ['build', '--debug', '--target', prepared.target, '--features', 'local-update-test', '--bundles', 'nsis', '--ci', '--', '--locked'],
    });
    await run(invocation.command, invocation.args, {
      cwd: desktop,
      env: { ...environment, ...invocation.environment, PANDA_DRAWER_UPDATE_TEST_VERSION: version, PANDA_DRAWER_UPDATE_PUBLIC_KEY: publicKey },
      windowsHide: true,
    });
    const generated = path.join(output, `${profile.name}_${version}_x64-setup.exe`);
    await copyFile(generated, outputForVersion(version), constants.COPYFILE_EXCL);
    await rm(generated);
  }
  // This stage was created by this invocation and both verified CLIs are now
  // packaged. Keep the normal repository build cache for subsequent checks.
  const stage = await realpath(prepared.preparationRoot);
  if (path.dirname(stage) !== await realpath(path.join(root, '.tools'))
      || !path.basename(stage).startsWith('desktop-windows-preparation-')) throw new Error('Unexpected Windows preparation stage.');
  await rm(stage, { recursive: true });
}
