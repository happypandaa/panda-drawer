import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { npmCommand } from './npm-command.mjs';

test('Windows invokes the npm CLI beside Node without a command wrapper or shell quoting', () => {
  const executable = 'C:\\Program Files\\nodejs\\node.exe';
  const cli = 'C:\\Program Files\\nodejs\\node_modules\\npm\\bin\\npm-cli.js';
  const args = ['pack', '--pack-destination', 'D:\\中文 空格 & source', '$(never)', 'a"b', '%PATH%'];
  const saved = [...args];
  const result = npmCommand(args, { platform: 'win32', executable, environment: {}, isFile: file => file === cli });
  assert.deepEqual(result, { command: executable, args: [cli, ...saved] });
  assert.deepEqual(args, saved);
  result.args.push('later');
  assert.deepEqual(args, saved);
});

test('Windows honors an existing npm-cli.js from npm_execpath while preserving the selected Node', () => {
  const cli = 'D:\\工具 空格\\npm\\bin\\npm-cli.js';
  const probes = [];
  const result = npmCommand(['ci'], { platform: 'win32', executable: 'C:\\node\\node.exe',
    environment: { npm_execpath: cli }, isFile: file => { probes.push(file); return file === cli; } });
  assert.deepEqual(probes, [cli]);
  assert.deepEqual(result, { command: 'C:\\node\\node.exe', args: [cli, 'ci'] });
});

test('Windows ignores command wrappers and relative npm entry points, then checks absolute PATH installations', () => {
  const cli = 'D:\\npm tools\\node_modules\\npm\\bin\\npm-cli.js';
  for (const invalid of ['D:\\npm tools\\npm.cmd', 'D:\\npm tools\\npm.exe', 'npm-cli.js']) {
    const probes = [];
    const result = npmCommand(['--version'], { platform: 'win32', executable: 'C:\\node\\node.exe',
      environment: { npm_execpath: invalid, Path: 'relative;D:\\npm tools' },
      isFile: file => { probes.push(file); return file === cli; } });
    assert.ok(!probes.includes(invalid));
    assert.ok(probes.every(file => file.endsWith('npm-cli.js')));
    assert.deepEqual(result, { command: 'C:\\node\\node.exe', args: [cli, '--version'] });
  }
});

test('Windows reports a missing npm CLI instead of falling back to npm.cmd', () => {
  assert.throws(() => npmCommand(['ci'], { platform: 'win32', executable: 'C:\\node\\node.exe',
    environment: { npm_execpath: 'C:\\node\\npm.cmd' }, isFile: () => false }), /Windows npm CLI is unavailable/);
});

test('macOS keeps the existing npm executable and exact argument array without inspecting Windows candidates', () => {
  const args = ['run', 'build', '--', '--outDir', '/tmp/中文 & 空格;$(never)'];
  assert.deepEqual(npmCommand(args, { platform: 'darwin', environment: { npm_execpath: '/missing/npm-cli.js' },
    isFile: () => { throw new Error('macOS must keep the existing npm lookup'); } }), { command: 'npm', args });
});

test('the selected CLI receives literal Unicode, spaces and shell characters in a real Node child', async () => {
  const temporaryRoot = path.resolve(tmpdir());
  const temporary = await mkdtemp(path.join(temporaryRoot, 'panda-npm-中文 space-'));
  try {
    const directory = path.join(temporary, 'npm & tools');
    await mkdir(directory);
    const cli = path.join(directory, 'npm-cli.js');
    await writeFile(cli, 'process.stdout.write(JSON.stringify(process.argv.slice(2)));\n', { flag: 'wx' });
    const args = ['中文 空格', '& echo never', '; $(never)', '%PATH%', 'a"b', "a'b", '`never`'];
    const invocation = npmCommand(args, { platform: 'win32', environment: { npm_execpath: cli } });
    const result = spawnSync(invocation.command, invocation.args, { encoding: 'utf8', windowsHide: true });
    assert.equal(result.error, undefined);
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(JSON.parse(result.stdout), args);
    // A directory with the right name is not a usable JavaScript CLI.
    const directoryCli = path.join(temporary, 'directory', 'npm-cli.js');
    await mkdir(directoryCli, { recursive: true });
    assert.throws(() => npmCommand([], { platform: 'win32', executable: path.join(temporary, 'missing-node.exe'),
      environment: { npm_execpath: directoryCli } }), /Windows npm CLI is unavailable/);
  } finally {
    assert.equal(path.dirname(path.resolve(temporary)), temporaryRoot);
    assert.ok(path.basename(temporary).startsWith('panda-npm-中文 space-'));
    await rm(temporary, { recursive: true, force: true });
  }
});
