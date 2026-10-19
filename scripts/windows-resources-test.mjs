import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdir, mkdtemp, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import path from 'node:path';
import { root } from './process.mjs';
import { templateNpmCommand, treeDigest } from './package-template.mjs';

test('Windows template packaging invokes npm-cli.js with Node and separate arguments', () => {
  const node = 'C:\\Program Files\\Node\\node.exe';
  const npm = 'C:\\custom & fixture\\npm-cli.js';
  assert.deepEqual(templateNpmCommand({
    platform: 'win32', executable: node, environment: { npm_execpath: npm },
    fileExists: candidate => candidate === npm,
  }), { command: node, prefix: [npm] });
});

test('Windows npm resolution falls back to the active Node installation and never uses a shell', () => {
  const node = 'C:\\Program Files\\Node\\node.exe';
  const npm = 'C:\\Program Files\\Node\\node_modules\\npm\\bin\\npm-cli.js';
  assert.deepEqual(templateNpmCommand({
    platform: 'win32', executable: node, environment: { npm_execpath: 'C:\\other\\yarn.js' },
    fileExists: candidate => candidate === npm,
  }), { command: node, prefix: [npm] });
  assert.throws(() => templateNpmCommand({
    platform: 'win32', executable: node, environment: {}, fileExists: () => false,
  }), /Cannot locate npm-cli.js/);
});

test('Mac keeps the existing npm executable invocation', () => {
  assert.deepEqual(templateNpmCommand({ platform: 'darwin' }), { command: 'npm', prefix: [] });
});

test('resource tree digest uses slash paths and UTF-8 depth-first ordering', async () => {
  const parent = path.join(root, '.test-artifacts');
  await mkdir(parent, { recursive: true });
  const fixture = await mkdtemp(path.join(parent, 'windows-resource-digest-'));
  const files = [
    ['a/z.txt', 'alpha'],
    ['a-.txt', 'beta'],
    ['a\uE000/x.txt', 'bmp'],
    ['a😀/z.txt', 'unicode'],
  ];
  const expected = createHash('sha256');
  const length = value => { const buffer = Buffer.alloc(8); buffer.writeBigUInt64LE(BigInt(value)); return buffer; };
  for (const [label, bytes] of files) {
    const target = path.join(fixture, label);
    await mkdir(path.dirname(target), { recursive: true });
    await writeFile(target, bytes);
    expected.update(length(Buffer.byteLength(label))).update(label).update(length(Buffer.byteLength(bytes))).update(bytes);
  }
  assert.equal(await treeDigest(fixture), expected.digest('hex'));
});
