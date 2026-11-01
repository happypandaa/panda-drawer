import assert from 'node:assert/strict';
import test from 'node:test';
import { resolvePnpmCommand } from './pnpm-command.mjs';

function files(...names) { return filename => names.includes(filename); }
test('Windows global pnpm uses its JS entry instead of pnpm.cmd', async () => {
  const cli = 'C:\\Tools\\npm\\node_modules\\pnpm\\bin\\pnpm.cjs';
  const command = await resolvePnpmCommand({ platform: 'win32', searchPath: 'C:\\Tools\\npm', nodePath: 'C:\\Node\\node.exe', fileExists: files('C:\\Tools\\npm\\pnpm.cmd', cli) });
  assert.deepEqual(command, { executable: 'C:\\Node\\node.exe', prefix: [cli] });
});
test('Windows standalone pnpm exe runs directly', async () => {
  const executable = 'C:\\Tools\\pnpm\\pnpm.exe';
  assert.deepEqual(await resolvePnpmCommand({ platform: 'win32', searchPath: 'C:\\Tools\\pnpm', fileExists: files(executable) }), { executable, prefix: [] });
});
test('Unix pnpm symlink resolves to the installed Node CLI', async () => {
  const cli = '/opt/pnpm/bin/pnpm.cjs';
  assert.deepEqual(await resolvePnpmCommand({ platform: 'darwin', searchPath: '/usr/local/bin', nodePath: '/opt/node/bin/node', fileExists: files('/usr/local/bin/pnpm', cli), canonicalPath: async () => cli }), { executable: '/opt/node/bin/node', prefix: [cli] });
});
test('explicit JS CLI is supported and a .cmd alone is rejected', async () => {
  const cli = 'D:\\pnpm\\bin\\pnpm.cjs';
  assert.deepEqual(await resolvePnpmCommand({ platform: 'win32', searchPath: '', override: cli, nodePath: 'node.exe', fileExists: files(cli) }), { executable: 'node.exe', prefix: [cli] });
  await assert.rejects(resolvePnpmCommand({ platform: 'win32', searchPath: '', override: 'D:\\pnpm.cmd', fileExists: files('D:\\pnpm.cmd') }), /pnpm.cjs/);
});
