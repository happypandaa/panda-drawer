import path from 'node:path';
import { readFile } from 'node:fs/promises';
import { root, run, rustEnvironment } from './process.mjs';
import { prepareDesktop } from './prepare-desktop.mjs';
import { desktopPackagePlan, withDesktopPackageOutput } from './desktop-package-output.mjs';

const args = process.argv.slice(2);
try {
  const configuration = JSON.parse(await readFile(path.join(root, 'apps/desktop/src-tauri/tauri.conf.json'), 'utf8'));
  const plan = desktopPackagePlan({ root, args, configuration });
  const execute = async () => {
    if (['dev', 'build'].includes(args[0])) await prepareDesktop({ release: args[0] === 'build' && !args.includes('--debug') });
    await run(path.join(root, 'node_modules/.bin/tauri'), args, {
      cwd: path.join(root, 'apps/desktop'),
      env: { ...rustEnvironment(), MACOSX_DEPLOYMENT_TARGET: '15.0' },
    });
  };
  if (plan) {
    const result = await withDesktopPackageOutput(plan, execute);
    console.log(`已核验最新应用包，清理 ${result.removedPrevious} 个旧生成包。`);
  } else await execute();
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}
