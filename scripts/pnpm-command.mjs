// Resolve an already-installed pnpm without invoking a Windows shell wrapper.
import { existsSync } from 'node:fs';
import { realpath } from 'node:fs/promises';
import path from 'node:path';

export async function resolvePnpmCommand({
  platform = process.platform,
  searchPath = process.env.PATH ?? process.env.Path ?? '',
  override = process.env.PANDA_CI_PNPM_CLI,
  nodePath = process.execPath,
  fileExists = existsSync,
  canonicalPath = realpath,
} = {}) {
  const paths = platform === 'win32' ? path.win32 : path.posix;
  const candidates = [override];
  for (const directory of searchPath.split(paths.delimiter)) {
    if (!directory) continue;
    candidates.push(
      paths.join(directory, 'pnpm.cjs'),
      paths.join(directory, 'node_modules/pnpm/bin/pnpm.cjs'),
      paths.join(directory, '../lib/node_modules/pnpm/bin/pnpm.cjs'),
      paths.join(directory, 'pnpm.exe'),
    );
    if (platform !== 'win32' && fileExists(paths.join(directory, 'pnpm'))) candidates.push(await canonicalPath(paths.join(directory, 'pnpm')));
  }
  const selected = candidates.find(candidate => candidate && fileExists(candidate) && !/\.(?:cmd|bat)$/i.test(candidate));
  if (!selected) throw new Error('Install the packageManager pnpm version on this runner, or set PANDA_CI_PNPM_CLI to its pnpm.cjs or executable.');
  return /\.[cm]?js$/i.test(selected)
    ? { executable: nodePath, prefix: [selected] }
    : { executable: selected, prefix: [] };
}
