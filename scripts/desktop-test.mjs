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
  for (const args of [['build', '--bundles', 'app'], ['build', '--bundles=nsis'], ['build', '-b', 'nsis'], ['build', '-bnsis']]) {
    assert.throws(() => desktopPlan(args, 'win32', 'x64'), /installers are not enabled/);
  }
});

test('Windows dev/build can snapshot a developer template without forwarding its option to Tauri', () => {
  const templateDirectory = 'D:\\developer samples & fixtures\\template';
  assert.deepEqual(desktopPlan(['dev', '--windows-template', templateDirectory], 'win32', 'x64'), {
    prepareResources: false, prepareWindowsResources: true, args: ['dev'], templateDirectory,
  });
  assert.deepEqual(desktopPlan(['build', '--windows-template', templateDirectory, '--debug', '--', '--locked'], 'win32', 'x64'), {
    prepareResources: false, prepareWindowsResources: true,
    args: ['build', '--debug', '--no-bundle', '--', '--locked'], templateDirectory,
  });
  assert.equal(desktopPlan(['build', '--windows-template=\\\\server\\share\\template'], 'win32', 'x64').templateDirectory,
    '\\\\server\\share\\template');
});

test('developer template options reject unsupported commands, missing or repeated values and relative paths', () => {
  const templateDirectory = 'D:\\developer-template';
  for (const [args, platform] of [
    [['dev', '--windows-template', templateDirectory], 'darwin'],
    [['build', '--windows-template', templateDirectory], 'linux'],
    [['--help', '--windows-template', templateDirectory], 'win32'],
    [['info', '--windows-template', templateDirectory], 'win32'],
  ]) assert.throws(() => desktopPlan(args, platform, platform === 'darwin' ? 'arm64' : 'x64'), /only available for Windows dev\/build/);
  for (const args of [
    ['build', '--windows-template'], ['build', '--windows-template', '--debug'], ['build', '--windows-template='],
  ]) assert.throws(() => desktopPlan(args, 'win32', 'x64'), /ABSOLUTE_TEMPLATE_DIRECTORY/);
  for (const value of ['template', '.\\template', 'D:template', '\\template', '/template']) {
    assert.throws(() => desktopPlan(['build', '--windows-template', value], 'win32', 'x64'), /absolute Windows template directory/);
  }
  assert.throws(() => desktopPlan(['build', '--windows-template', templateDirectory, '--windows-template', templateDirectory], 'win32', 'x64'), /only be specified once/);
  assert.throws(() => desktopPlan(['build', '--windows-template=' + templateDirectory, '--windows-template=' + templateDirectory], 'win32', 'x64'), /only be specified once/);
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
