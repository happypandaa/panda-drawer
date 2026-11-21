import assert from 'node:assert/strict';
import test from 'node:test';
import path from 'node:path';
import { desktopInvocation } from './desktop-host.mjs';
import { windowsPreparationLayout, windowsResourceConfiguration } from './prepare-desktop-windows.mjs';

const fixture = {
  root: path.resolve('disposable 中文 source'), platform: 'win32', node: 'node.exe',
  configuration: { bundle: { resources: { 'fresh/resources/': 'resources/' }, externalBin: ['fresh/cli'] } },
  targetDirectory: 'fresh/target', target: 'x86_64-pc-windows-msvc',
};

test('ordinary Windows preparations share a persistent compiler cache and retain independent resource/CLI stages', () => {
  const firstStage = path.join(fixture.root, '.tools/desktop-windows-preparation-first');
  const secondStage = path.join(fixture.root, '.tools/desktop-windows-preparation-second');
  const first = windowsPreparationLayout(fixture.root, firstStage);
  const second = windowsPreparationLayout(fixture.root, secondStage);
  assert.equal(first.targetDirectory, path.join(fixture.root, 'target/desktop-windows'));
  assert.equal(second.targetDirectory, first.targetDirectory);
  assert.equal(first.resources, path.join(firstStage, 'resources'));
  assert.equal(second.resources, path.join(secondStage, 'resources'));
  assert.notEqual(first.resources, second.resources);
  const firstConfiguration = windowsResourceConfiguration(firstStage);
  const secondConfiguration = windowsResourceConfiguration(secondStage);
  assert.deepEqual(Object.keys(firstConfiguration.bundle.resources), [first.resources + path.sep]);
  assert.deepEqual(Object.keys(secondConfiguration.bundle.resources), [second.resources + path.sep]);
  assert.notEqual(firstConfiguration.bundle.externalBin[0], secondConfiguration.bundle.externalBin[0]);
  assert.equal(firstConfiguration.bundle.externalBin[0], path.join(firstStage, 'binaries/local-projects-dev'));
  for (const localUpdateTestVersion of ['0.1.0', '0.1.1']) {
    const trial = windowsPreparationLayout(fixture.root, firstStage, { localUpdateTestVersion });
    assert.equal(trial.targetDirectory, path.join(fixture.root, 'target'));
    assert.equal(trial.resources, first.resources);
  }
});

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
