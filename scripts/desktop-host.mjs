// Keep host-specific executable dispatch outside the shared desktop workflow.
import path from 'node:path';

export function desktopInvocation({ root, args, configuration, targetDirectory, target, platform = process.platform, node = process.execPath }) {
  if (platform === 'win32') {
    const separator = args.indexOf('--');
    const at = separator === -1 ? args.length : separator;
    if (configuration && args.slice(at + 1).some(arg => /^(--target-dir|--target|--profile|--config)(=|$)/.test(arg))) {
      throw new Error('Windows 桌面准备使用本次独立 target；请移除转发给 Cargo 的 target-dir、target、profile 或 config 覆盖参数。');
    }
    const configuredArgs = configuration ? [...args.slice(0, at),
      '--config', JSON.stringify({ bundle: { resources: null, externalBin: [] } }),
      '--config', JSON.stringify(configuration), ...args.slice(at)] : args;
    return {
      command: node,
      args: [path.join(root, 'node_modules/@tauri-apps/cli/tauri.js'),
        ...configuredArgs],
      environment: targetDirectory ? { CARGO_TARGET_DIR: targetDirectory, CARGO_BUILD_TARGET: target } : {},
    };
  }
  return { command: path.join(root, 'node_modules/.bin/tauri'), args,
    environment: { MACOSX_DEPLOYMENT_TARGET: '15.0' } };
}
