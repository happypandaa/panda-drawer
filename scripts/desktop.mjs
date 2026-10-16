import path from 'node:path';
import { root, run, rustEnvironment } from './process.mjs';
import { prepareDesktop } from './prepare-desktop.mjs';

const args = process.argv.slice(2);
try {
  if (['dev', 'build'].includes(args[0])) await prepareDesktop({ release: args[0] === 'build' && !args.includes('--debug') });
  await run(path.join(root, 'node_modules/.bin/tauri'), args, {
    cwd: path.join(root, 'apps/desktop'),
    env: { ...rustEnvironment(), MACOSX_DEPLOYMENT_TARGET: '15.0' },
  });
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}
