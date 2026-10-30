import { statSync } from 'node:fs';
import path from 'node:path';

function ordinaryFile(filename) {
  try { return statSync(filename).isFile(); }
  catch (error) {
    if (['ENOENT', 'ENOTDIR'].includes(error.code)) return false;
    throw error;
  }
}

// npm.cmd needs a shell on Windows. Run npm's JavaScript entry point with the
// current Node instead, keeping each caller-supplied argument as a separate value.
export function npmCommand(args, {
  platform = process.platform,
  executable = process.execPath,
  environment = process.env,
  isFile = ordinaryFile,
} = {}) {
  if (platform !== 'win32') return { command: 'npm', args: [...args] };

  const paths = path.win32;
  const explicit = environment.npm_execpath;
  const candidates = [];
  if (typeof explicit === 'string' && paths.isAbsolute(explicit)
      && paths.basename(explicit).toLowerCase() === 'npm-cli.js') {
    candidates.push(explicit);
  }
  candidates.push(paths.join(paths.dirname(executable), 'node_modules', 'npm', 'bin', 'npm-cli.js'));
  const pathKey = Object.keys(environment).sort().find(key => key.toLowerCase() === 'path');
  for (const directory of (environment[pathKey] ?? '').split(paths.delimiter).filter(Boolean)) {
    if (paths.isAbsolute(directory)) candidates.push(paths.join(directory, 'node_modules', 'npm', 'bin', 'npm-cli.js'));
  }
  const cli = [...new Set(candidates)].find(candidate => isFile(candidate));
  if (!cli) {
    throw new Error('Windows npm CLI is unavailable. Install npm alongside Node or set npm_execpath to an existing npm-cli.js.');
  }
  return { command: executable, args: [cli, ...args] };
}
