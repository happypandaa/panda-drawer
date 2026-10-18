import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

export const root = fileURLToPath(new URL('../', import.meta.url));

export function rustEnvironment({ environment = process.env, platform = process.platform, repositoryRoot = root, fileExists = existsSync } = {}) {
  const result = { ...environment };
  const paths = platform === 'win32' ? path.win32 : path.posix;
  if (platform === 'darwin') result.MACOSX_DEPLOYMENT_TARGET = '15.0';
  const cargoHome = paths.join(repositoryRoot, '.tools', 'cargo');
  const rustupHome = paths.join(repositoryRoot, '.tools', 'rustup');
  const cargoBin = paths.join(cargoHome, 'bin');
  if (!fileExists(paths.join(cargoBin, platform === 'win32' ? 'cargo.exe' : 'cargo'))) return result;
  const pathKey = platform === 'win32'
    ? Object.keys(result).sort().find(key => key.toLowerCase() === 'path')
    : 'PATH';
  const inheritedPath = result[pathKey] ?? '';
  // Windows treats PATH keys case-insensitively. Node's child process API may
  // choose only one variant, so pass exactly one while retaining the full value.
  if (platform === 'win32') {
    for (const key of Object.keys(result)) if (key.toLowerCase() === 'path') delete result[key];
  }
  result.CARGO_HOME = cargoHome;
  result.RUSTUP_HOME = rustupHome;
  result.PATH = `${cargoBin}${paths.delimiter}${inheritedPath}`;
  return result;
}

export function run(command, args, options = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd: root, env: rustEnvironment(), stdio: 'inherit', ...options });
    const forward = (signal) => child.kill(signal);
    const onInt = () => forward('SIGINT');
    const onTerm = () => forward('SIGTERM');
    process.on('SIGINT', onInt);
    process.on('SIGTERM', onTerm);
    child.on('error', (error) => {
      process.off('SIGINT', onInt);
      process.off('SIGTERM', onTerm);
      reject(error);
    });
    child.on('exit', (code, signal) => {
      process.off('SIGINT', onInt);
      process.off('SIGTERM', onTerm);
      if (code === 0) resolve();
      else reject(new Error(`${command} exited with ${signal ?? code}`));
    });
  });
}
