import test from 'node:test';
import assert from 'node:assert/strict';
import { desktopPlan } from './desktop.mjs';

test('Windows prepares its embedded project resources before dev and build', () => {
  assert.deepEqual(desktopPlan(['dev'], 'win32', 'x64'), { prepareResources: false, prepareWindowsResources: true, args: ['dev'] });
  assert.deepEqual(desktopPlan(['build', '--debug'], 'win32', 'x64'), {
    prepareResources: false, prepareWindowsResources: true, args: ['build', '--debug', '--no-bundle'],
  });
  assert.deepEqual(desktopPlan(['build', '--no-bundle'], 'win32', 'x64').args, ['build', '--no-bundle']);
  assert.deepEqual(desktopPlan(['build', '--debug', '--', '--locked'], 'win32', 'x64').args,
    ['build', '--debug', '--no-bundle', '--', '--locked']);
});

test('Windows cannot accidentally enter the unimplemented installer path', () => {
  for (const args of [['build', '--bundles', 'app'], ['build', '--bundles=nsis']]) {
    assert.throws(() => desktopPlan(args, 'win32', 'x64'), /installers are not enabled/);
  }
});

test('Mac keeps its existing resource preparation and bundle arguments', () => {
  for (const args of [['dev'], ['build', '--bundles', 'app'], ['build']]) {
    assert.deepEqual(desktopPlan(args, 'darwin', 'arm64'), { prepareResources: true, prepareWindowsResources: false, args });
  }
});

test('unsupported native targets fail before resource preparation', () => {
  for (const [platform, arch] of [['win32', 'arm64'], ['win32', 'ia32'], ['darwin', 'x64'], ['linux', 'x64']]) {
    assert.throws(() => desktopPlan(['build'], platform, arch), /supports macOS arm64 and Windows x64 only/);
  }
  assert.deepEqual(desktopPlan(['--help'], 'linux', 'x64'), { prepareResources: false, prepareWindowsResources: false, args: ['--help'] });
});
