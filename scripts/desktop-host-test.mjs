import assert from 'node:assert/strict';
import test from 'node:test';
import path from 'node:path';
import { desktopInvocation } from './desktop-host.mjs';

const fixture = {
  root: path.resolve('disposable 中文 source'), platform: 'win32', node: 'node.exe',
  configuration: { bundle: { resources: { 'fresh/resources/': 'resources/' }, externalBin: ['fresh/cli'] } },
  targetDirectory: 'fresh/target', target: 'x86_64-pc-windows-msvc',
};

test('Windows dispatch appends reset and exact resources after user config and before application arguments', () => {
  const invocation = desktopInvocation({ ...fixture, args: ['dev', '--config', '{"bundle":{"resources":{"old/":"resources/"}}}', '--', '--example'] });
  assert.equal(invocation.command, 'node.exe');
  assert.equal(invocation.args[0], path.join(fixture.root, 'node_modules/@tauri-apps/cli/tauri.js'));
  assert.deepEqual(invocation.args.slice(1, 4), ['dev', '--config', '{"bundle":{"resources":{"old/":"resources/"}}}']);
  assert.equal(invocation.args[4], '--config');
  assert.deepEqual(JSON.parse(invocation.args[5]), { bundle: { resources: null, externalBin: [] } });
  assert.equal(invocation.args[6], '--config');
  assert.deepEqual(JSON.parse(invocation.args[7]), fixture.configuration);
  assert.deepEqual(invocation.args.slice(8), ['--', '--example']);
  assert.deepEqual(invocation.environment, { CARGO_TARGET_DIR: 'fresh/target', CARGO_BUILD_TARGET: fixture.target });
});

test('Windows controlled build refuses forwarded Cargo output and target overrides', () => {
  for (const option of ['--target-dir', '--target', '--profile', '--config']) {
    for (const forwarded of [[option, 'old-output'], [`${option}=old-output`]]) {
      assert.throws(() => desktopInvocation({ ...fixture, args: ['build', '--debug', '--no-bundle', '--', ...forwarded] }), /独立 target/);
    }
  }
  const invocation = desktopInvocation({ ...fixture, args: ['build', '--debug', '--no-bundle', '--', '--locked'] });
  assert.deepEqual(invocation.args.slice(-2), ['--', '--locked']);
});

test('Mac dispatch preserves its executable, original args and deployment environment', () => {
  const args = ['build', '--no-bundle', '--', '--target-dir', 'original-output'];
  const invocation = desktopInvocation({ ...fixture, platform: 'darwin', args });
  assert.equal(invocation.command, path.join(fixture.root, 'node_modules/.bin/tauri'));
  assert.strictEqual(invocation.args, args);
  assert.deepEqual(invocation.environment, { MACOSX_DEPLOYMENT_TARGET: '15.0' });
});
