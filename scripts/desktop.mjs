import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { root, run, rustEnvironment } from './process.mjs';
import { prepareDesktop } from './prepare-desktop.mjs';
import { prepareWindowsResources } from './prepare-windows-resources.mjs';

export function desktopPlan(args, platform = process.platform, architecture = process.arch) {
  const preparing = ['dev', 'build'].includes(args[0]);
  const windows = platform === 'win32';
  if (preparing && !((platform === 'darwin' && architecture === 'arm64') || (windows && architecture === 'x64'))) {
    throw new Error('Desktop development supports macOS arm64 and Windows x64 only.');
  }
  if (windows && args[0] === 'build' && args.some(arg => arg === '--bundles' || arg.startsWith('--bundles='))) {
    throw new Error('Windows internal builds produce the desktop executable only. Use npm run build; installers are not enabled yet.');
  }
  const invocation = [...args];
  if (windows && args[0] === 'build' && !args.includes('--no-bundle')) {
    const separator = invocation.indexOf('--');
    invocation.splice(separator === -1 ? invocation.length : separator, 0, '--no-bundle');
  }
  return {
    prepareResources: preparing && !windows,
    prepareWindowsResources: preparing && windows,
    args: invocation,
  };
}

async function main() {
  const plan = desktopPlan(process.argv.slice(2));
  if (plan.prepareResources) await prepareDesktop({ release: plan.args[0] === 'build' && !plan.args.includes('--debug') });
  if (plan.prepareWindowsResources) await prepareWindowsResources();
  // Invoke the package's JS entry with Node: .bin/tauri is not a Windows executable.
  await run(process.execPath, [path.join(root, 'node_modules/@tauri-apps/cli/tauri.js'), ...plan.args], {
    cwd: path.join(root, 'apps/desktop'),
    env: rustEnvironment(),
  });
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  main().catch(error => { console.error(error.message); process.exitCode = 1; });
}
