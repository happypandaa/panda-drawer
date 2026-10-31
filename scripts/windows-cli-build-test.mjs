import assert from 'node:assert/strict';
import test from 'node:test';
import { windowsCliBuildEnvironment } from './windows-cli-build.mjs';

const separator = '\x1f';
test('Windows CLI statically links its CRT without changing target or dev identity inputs', () => {
  const original = { CARGO_TARGET_DIR: 'fresh 中文 target', CARGO_BUILD_TARGET: 'x86_64-pc-windows-msvc', PATH: 'native tools' };
  const result = windowsCliBuildEnvironment(original);
  assert.deepEqual(result, { ...original, CARGO_ENCODED_RUSTFLAGS: ['-C', 'target-feature=+crt-static'].join(separator) });
  assert.equal(original.CARGO_ENCODED_RUSTFLAGS, undefined);
});

test('encoded Cargo flags retain literal Chinese, spaces and shell characters and take precedence', () => {
  const flags = ['-L', 'native=C:\\Compiler SDK 中文\\lib & extra', '', '-C', 'target-feature=-crt-static'];
  const result = windowsCliBuildEnvironment({ CARGO_ENCODED_RUSTFLAGS: flags.join(separator), RUSTFLAGS: '--cfg ignored_by_cargo' });
  assert.deepEqual(result.CARGO_ENCODED_RUSTFLAGS.split(separator), [...flags, '-C', 'target-feature=+crt-static']);
  assert.equal(result.RUSTFLAGS, '--cfg ignored_by_cargo');
});

test('space-separated flags keep Cargo token semantics and an explicit empty encoded list wins', () => {
  const result = windowsCliBuildEnvironment({ RUSTFLAGS: '-C debuginfo=1  --cfg disposable' });
  assert.deepEqual(result.CARGO_ENCODED_RUSTFLAGS.split(separator), ['-C', 'debuginfo=1', '--cfg', 'disposable', '-C', 'target-feature=+crt-static']);
  const empty = windowsCliBuildEnvironment({ CARGO_ENCODED_RUSTFLAGS: '', RUSTFLAGS: '--cfg ignored_by_cargo' });
  assert.deepEqual(empty.CARGO_ENCODED_RUSTFLAGS.split(separator), ['-C', 'target-feature=+crt-static']);
});
