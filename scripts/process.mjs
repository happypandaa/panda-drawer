import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

export const root = fileURLToPath(new URL('../', import.meta.url));

export function rustEnvironment() {
  const environment = { ...process.env };
  if (process.platform === 'darwin') environment.MACOSX_DEPLOYMENT_TARGET = '15.0';
  const cargoHome = path.join(root, '.tools/cargo');
  const rustupHome = path.join(root, '.tools/rustup');
  const localCargo = path.join(cargoHome, 'bin/cargo');
  if (!existsSync(localCargo)) return environment;
  return {
    ...environment,
    CARGO_HOME: cargoHome,
    RUSTUP_HOME: rustupHome,
    PATH: `${path.join(cargoHome, 'bin')}${path.delimiter}${environment.PATH ?? ''}`,
  };
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
