import test from 'node:test';
import assert from 'node:assert/strict';
import { rustEnvironment } from './process.mjs';

test('Windows selects repository cargo.exe, retains the system Path and leaves its caller unchanged', () => {
  const input = { Path: 'C:\\Program Files\\nodejs;C:\\Windows\\System32', TEMP: 'C:\\Temp' };
  const original = { ...input };
  const probes = [];
  const result = rustEnvironment({ environment: input, platform: 'win32', repositoryRoot: 'D:\\repo',
    fileExists: filename => { probes.push(filename); return filename === 'D:\\repo\\.tools\\cargo\\bin\\cargo.exe'; } });
  assert.deepEqual(probes, ['D:\\repo\\.tools\\cargo\\bin\\cargo.exe']);
  assert.equal(result.PATH, 'D:\\repo\\.tools\\cargo\\bin;C:\\Program Files\\nodejs;C:\\Windows\\System32');
  assert.equal(result.CARGO_HOME, 'D:\\repo\\.tools\\cargo');
  assert.equal(result.RUSTUP_HOME, 'D:\\repo\\.tools\\rustup');
  assert.equal(Object.keys(result).filter(key => key.toLowerCase() === 'path').length, 1);
  assert.deepEqual(input, original);
});

test('Windows removes ambiguous PATH variants using the same lexical precedence as Node', () => {
  const result = rustEnvironment({ environment: { Path: 'ignored', PATH: 'C:\\tools', path: 'also ignored' },
    platform: 'win32', repositoryRoot: 'D:\\repo', fileExists: () => true });
  assert.equal(result.PATH, 'D:\\repo\\.tools\\cargo\\bin;C:\\tools');
  assert.deepEqual(Object.keys(result).filter(key => key.toLowerCase() === 'path'), ['PATH']);
});

test('an absent local Windows toolchain preserves the system toolchain environment', () => {
  const input = { Path: 'C:\\tools', CARGO_HOME: 'C:\\user-cargo', RUSTUP_HOME: 'C:\\user-rustup' };
  const result = rustEnvironment({ environment: input, platform: 'win32', repositoryRoot: 'D:\\repo', fileExists: () => false });
  assert.deepEqual(result, input);
  assert.notEqual(result, input);
});

test('macOS retains its private Cargo paths and deployment target behavior', () => {
  const input = { PATH: '/usr/bin:/bin', MACOSX_DEPLOYMENT_TARGET: '14.0' };
  const probes = [];
  const result = rustEnvironment({ environment: input, platform: 'darwin', repositoryRoot: '/repo',
    fileExists: filename => { probes.push(filename); return filename === '/repo/.tools/cargo/bin/cargo'; } });
  assert.deepEqual(probes, ['/repo/.tools/cargo/bin/cargo']);
  assert.equal(result.PATH, '/repo/.tools/cargo/bin:/usr/bin:/bin');
  assert.equal(result.CARGO_HOME, '/repo/.tools/cargo');
  assert.equal(result.RUSTUP_HOME, '/repo/.tools/rustup');
  assert.equal(result.MACOSX_DEPLOYMENT_TARGET, '15.0');
  assert.equal(input.MACOSX_DEPLOYMENT_TARGET, '14.0');
});

test('macOS system Cargo fallback still receives the fixed deployment target', () => {
  const input = { PATH: '/usr/bin' };
  assert.deepEqual(rustEnvironment({ environment: input, platform: 'darwin', repositoryRoot: '/repo', fileExists: () => false }),
    { PATH: '/usr/bin', MACOSX_DEPLOYMENT_TARGET: '15.0' });
  assert.deepEqual(input, { PATH: '/usr/bin' });
});
