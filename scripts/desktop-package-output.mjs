// Only owns the standard generated macOS app/DMG in target/<profile>/bundle.
// It never scans installed applications, projects, backups or test evidence.
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { lstat, mkdir, mkdtemp, readdir, realpath, rename, rm, rmdir, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { randomUUID } from 'node:crypto';
import { verifyDevelopmentNode } from './package-development-node.mjs';

const execute = promisify(execFile);
const productName = '熊猫应用小抽屉';
// The only historical output owned by this build workflow keeps its own name.
const generatedProductNames = [productName, 'Local Projects Dev'];
const missing = error => error.code === 'ENOENT';
async function statIfPresent(file) {
  try { return await lstat(file); } catch (error) { if (missing(error)) return undefined; throw error; }
}

async function normalDirectory(directory, { create = false } = {}) {
  const absolute = path.resolve(directory);
  const parts = absolute.split(path.sep).filter(Boolean);
  let cursor = path.parse(absolute).root;
  for (const part of parts) {
    cursor = path.join(cursor, part);
    let info = await statIfPresent(cursor);
    if (!info && create) {
      try { await mkdir(cursor); } catch (error) { if (error.code !== 'EEXIST') throw error; }
      info = await lstat(cursor);
    }
    if (!info || !info.isDirectory() || info.isSymbolicLink()) throw new Error(`打包输出目录必须是真实目录：${cursor}`);
  }
}

export function desktopPackagePlan({ root, args, configuration, environment = process.env, platform = process.platform, arch = process.arch }) {
  if (platform !== 'darwin' || args[0] !== 'build' || (args.includes('--no-bundle') || args.includes('--help') || args.includes('-h'))) return undefined;
  if (args.some(arg => /^(--config|--target|--target-dir|--profile)(=|$)/.test(arg) || /^-[ct](?:.|$)/.test(arg)) || environment.CARGO_TARGET_DIR || environment.CARGO_BUILD_TARGET || environment.TAURI_CONFIG) {
    throw new Error('自动清理只支持标准打包输出；请移除自定义 config、target、profile 或覆盖打包配置与输出的环境变量。');
  }
  if (arch !== 'arm64') throw new Error('标准 Mac 内部包当前只支持 arm64。');
  const { productName: configuredProductName, version, identifier } = configuration;
  if (configuredProductName !== productName || identifier !== 'dev.localprojects.platform.m0' || !/^[0-9A-Za-z][0-9A-Za-z.+-]*$/.test(version)) {
    throw new Error('打包清理拒绝未知应用身份或版本文件名。');
  }
  let bundles = configuration.bundle?.targets;
  let selected = false;
  for (let index = 1; index < args.length; index += 1) {
    if (args[index] === '--bundles' || args[index] === '-b' || args[index].startsWith('--bundles=')) {
      if (selected) throw new Error('打包类型不能重复声明。');
      selected = true;
      const value = args[index].startsWith('--bundles=') ? args[index].slice('--bundles='.length) : args[++index];
      if (!value) throw new Error('打包类型不能为空。');
      bundles = value.split(',');
    }
  }
  if (!Array.isArray(bundles) || !bundles.length || bundles.some(bundle => !['app', 'dmg'].includes(bundle))) throw new Error('自动清理只支持 app、dmg 类型。');
  // Tauri builds the .app as part of a DMG as well.
  return {
    root: path.resolve(root),
    profile: args.includes('--debug') ? 'debug' : 'release',
    productName, version, identifier,
    expectedDmg: bundles.includes('dmg'),
    arch,
  };
}

async function plistIdentity(app, plan) {
  const { stdout } = await execute('/usr/bin/plutil', ['-convert', 'json', '-o', '-', path.join(app, 'Contents/Info.plist')]);
  const info = JSON.parse(stdout);
  if (info.CFBundleIdentifier !== plan.identifier || info.CFBundleName !== plan.productName || info.CFBundleExecutable !== 'local-projects-desktop-dev') throw new Error(`拒绝处理未知应用：${app}`);
}

export async function verifyDesktopApp(app, plan, { requireDevelopmentNode = true } = {}) {
  const info = await lstat(app);
  if (!info.isDirectory() || info.isSymbolicLink()) throw new Error(`应用输出必须是真实目录：${app}`);
  await plistIdentity(app, plan);
  await execute('/usr/bin/codesign', ['--verify', '--deep', '--strict', app], { timeout: 60_000 });
  if (requireDevelopmentNode) await verifyDevelopmentNode(path.join(app, 'Contents/Resources/resources/development-node'));
}

export async function verifyDesktopDmg(dmg, plan, { requireDevelopmentNode = true } = {}) {
  const info = await lstat(dmg);
  if (!info.isFile() || info.isSymbolicLink() || info.nlink !== 1) throw new Error(`镜像输出必须是独立普通文件：${dmg}`);
  await execute('/usr/bin/hdiutil', ['verify', dmg], { maxBuffer: 4 * 1024 * 1024, timeout: 60_000 });
  // macOS commonly exposes its temporary directory through /var -> /private/var.
  // Resolve only the system temporary parent; payload validation still rejects links.
  const mount = await mkdtemp(path.join(await realpath(os.tmpdir()), 'local-projects-package-inspection-'));
  let attached = false;
  try {
    await execute('/usr/bin/hdiutil', ['attach', '-readonly', '-nobrowse', '-noautoopen', '-mountpoint', mount, dmg], { maxBuffer: 4 * 1024 * 1024, timeout: 60_000 });
    attached = true;
    await verifyDesktopApp(path.join(mount, `${plan.productName}.app`), plan, { requireDevelopmentNode });
  } finally {
    if (attached) await execute('/usr/bin/hdiutil', ['detach', mount], { timeout: 60_000 });
    await rmdir(mount);
  }
}

async function runningAppPaths() {
  const { stdout } = await execute('/bin/ps', ['-axo', 'comm=']);
  return stdout.split('\n').map(line => line.trim()).filter(Boolean);
}

export async function withDesktopPackageOutput(plan, build, {
  verifyApp = verifyDesktopApp, verifyDmg = verifyDesktopDmg,
  processes = runningAppPaths,
} = {}) {
  const target = path.join(plan.root, 'target');
  await normalDirectory(plan.root);
  await normalDirectory(target, { create: true });
  const lock = path.join(target, '.desktop-package-build.lock');
  try { await mkdir(lock); } catch (error) {
    if (error.code === 'EEXIST') throw new Error(`已有打包事务或未完成的恢复现场：${lock}；请先检查，不会覆盖旧包。`);
    throw error;
  }
  let backup;
  let transactionStarted = false;
  let completed = false;
  const moved = [];
  const outputs = [];
  const profileRoot = path.join(target, plan.profile);
  const bundleRoot = path.join(profileRoot, 'bundle');
  const app = path.join(bundleRoot, 'macos', `${plan.productName}.app`);
  const currentDmg = path.join(bundleRoot, 'dmg', `${plan.productName}_${plan.version}_${plan.arch === 'arm64' ? 'aarch64' : plan.arch}.dmg`);
  const generatedApps = generatedProductNames.map(name => ({
    app: path.join(bundleRoot, 'macos', `${name}.app`),
    plan: { ...plan, productName: name },
  }));
  try {
    await writeFile(path.join(lock, 'owner.json'), `${JSON.stringify({ pid: process.pid, startedAt: new Date().toISOString(), profile: plan.profile })}\n`, { flag: 'wx' });
    await normalDirectory(profileRoot, { create: true });
    await normalDirectory(bundleRoot, { create: true });
    await normalDirectory(path.dirname(app), { create: true });
    await normalDirectory(path.dirname(currentDmg), { create: true });
    const active = await processes();
    for (const candidate of generatedApps) {
      if (active.some(command => command.startsWith(`${candidate.app}/Contents/MacOS/`))) throw new Error(`请先从菜单退出正在运行的 ${candidate.plan.productName}，再打包；当前程序：${candidate.app}`);
    }
    // Previous releases need a valid identity/signature, but may predate the
    // development toolchain. New outputs below must carry the complete payload.
    // Only fixed generated product names qualify. Unknown files are untouched.
    for (const candidate of generatedApps) {
      if (await statIfPresent(candidate.app)) { await verifyApp(candidate.app, candidate.plan, { requireDevelopmentNode: false }); outputs.push(candidate.app); }
    }
    for (const name of await readdir(path.dirname(currentDmg))) {
      const candidateName = generatedProductNames.find(product => name.startsWith(`${product}_`) && /^[0-9A-Za-z][0-9A-Za-z.+-]*_aarch64\.dmg$/.test(name.slice(product.length + 1)));
      if (!candidateName) continue;
      const file = path.join(path.dirname(currentDmg), name);
      await verifyDmg(file, { ...plan, productName: candidateName }, { requireDevelopmentNode: false });
      outputs.push(file);
    }
    const candidate = path.join(profileRoot, `.desktop-package-previous-${randomUUID()}`);
    await mkdir(candidate);
    backup = candidate;
    await writeFile(path.join(backup, 'transaction.json'), `${JSON.stringify({ schemaVersion: 1, identifier: plan.identifier, outputs }, null, 2)}\n`, { flag: 'wx' });
    for (let index = 0; index < outputs.length; index += 1) {
      const source = outputs[index];
      const saved = path.join(backup, `artifact-${index}`);
      await rename(source, saved);
      moved.push({ source, saved });
    }
    transactionStarted = true;
    await build();
    await normalDirectory(bundleRoot);
    await normalDirectory(path.dirname(app));
    await normalDirectory(path.dirname(currentDmg));
    await verifyApp(app, plan, { requireDevelopmentNode: true });
    if (plan.expectedDmg) await verifyDmg(currentDmg, plan, { requireDevelopmentNode: true });
    completed = true;
    const beforeCleanup = await processes();
    if (moved.some(({ saved }) => beforeCleanup.some(command => command.startsWith(`${saved}/Contents/MacOS/`)))) throw new Error(`旧生成包正在运行，保留它及打包恢复记录：${backup}`);
    // The staging directory contains only the entries moved above and our marker.
    for (const { saved } of moved) await rm(saved, { recursive: true });
    await rm(path.join(backup, 'transaction.json'), { force: true });
    await rmdir(backup);
    return { app, dmg: plan.expectedDmg ? currentDmg : undefined, removedPrevious: moved.length };
  } catch (error) {
    if (backup && !completed) {
      // New partial outputs exist only after build starts. No scan or broad delete.
      if (transactionStarted) {
        await normalDirectory(bundleRoot);
        for (const output of [app, currentDmg]) {
          await normalDirectory(path.dirname(output));
          await rm(output, { recursive: true, force: true });
        }
      }
      for (const { source, saved } of [...moved].reverse()) {
        if (await statIfPresent(source)) throw new Error(`打包失败且恢复位置被占用；旧包保留于 ${backup}。原错误：${error.message}`);
        await rename(saved, source);
      }
      await rm(path.join(backup, 'transaction.json'), { force: true });
      await rmdir(backup);
    }
    throw error;
  } finally {
    // A failed cleanup/rollback keeps its backup for manual inspection. The lock
    // prevents a later build from making that recovery ambiguous.
    if (!backup || !await statIfPresent(backup)) {
      await rm(path.join(lock, 'owner.json'), { force: true });
      await rmdir(lock);
    }
  }
}
