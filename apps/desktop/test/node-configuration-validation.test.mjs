// Pure configuration validation with artificial declarations and credentials.
// No configuration files, native application or external service are accessed.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import ts from 'typescript';

const source = await readFile(new URL('../src/node-configuration-validation.ts', import.meta.url), 'utf8');
const compiled = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext } }).outputText;
const { validateNodeConfiguration: validate } = await import(`data:text/javascript;base64,${Buffer.from(compiled).toString('base64')}`);
const field = (name, type = 'string', extra = {}) => ({ name, type, label: `人工设置 ${name}`, required: false, secret: false, description: '人工说明', howToGet: ['人工步骤'], ...extra });
const app = (fields, configuration = {}) => ({ id: 'disposable-validation-fixture', name: '可丢弃配置校验样本', sourcePath: '/not-a-real-source', runtime: 'node', state: 'stopped', restartRequired: false, runningRevision: null, lastError: null, fields, configuration: { revision: 1, values: {}, secretStatus: {}, missingRequired: [], canStart: false, ...configuration } });
const draft = (ordinary = {}, secrets = {}) => ({ ordinary, secrets });
const key = { action: 'replace', value: 'artificial-key-only' };
const linkedFields = [field('URL', 'url', { required: true, default: 'https://example.invalid/api' }), field('KEY', 'string', { required: true, secret: true, endpointField: 'URL' })];

test('Missing required values are field issues without blocking incomplete saves', () => {
  const value = app([field('TEXT', 'string', { required: true }), field('CHOICE', 'select', { required: true, options: [{ value: 'one', label: '一' }] }), field('KEY', 'string', { required: true, secret: true })]);
  const result = validate(value, draft({ TEXT: '  ' }));
  assert.deepEqual(Object.keys(result), ['TEXT', 'CHOICE', 'KEY']);
  assert.ok(Object.values(result).every((issue) => issue.message && !issue.blocking));
  assert.deepEqual(validate(app([field('OPTIONAL')]), draft()), {});
});

test('Defaults satisfy required fields without materializing user overrides', () => {
  const value = app([field('TEXT', 'string', { required: true, default: 'default' }), field('ZERO', 'integer', { required: true, default: 0 }), field('OFF', 'boolean', { required: true, default: false }), field('CHOICE', 'select', { required: true, default: 'one', options: [{ value: 'one', label: '一' }] })]);
  const edit = draft({ TEXT: '  ' }); const original = structuredClone(edit);
  assert.deepEqual(validate(value, edit), {});
  assert.deepEqual(edit, original);
  assert.deepEqual(value.configuration.values, {});
});

test('Explicit zero and false are present, while cleared fields fall back to defaults', () => {
  const value = app([field('ZERO', 'integer', { required: true }), field('OFF', 'boolean', { required: true }), field('DEFAULT', 'integer', { required: true, default: 7 })]);
  assert.deepEqual(validate(value, draft({ ZERO: '0', OFF: 'false', DEFAULT: '' })), {});
});

test('Integers keep the existing exact JavaScript number range and reject other syntax', () => {
  const value = app([field('COUNT', 'integer')]);
  for (const input of ['0', '-0', '0001', '-9007199254740991', '9007199254740991']) assert.deepEqual(validate(value, draft({ COUNT: input })), {}, input);
  for (const input of ['9007199254740992', '-9007199254740992', '1.5', '1e3', '+1', ' 1', '1 ']) assert.equal(validate(value, draft({ COUNT: input })).COUNT.blocking, true, input);
});

test('Boolean and select values must match their declared choices', () => {
  const value = app([field('FLAG', 'boolean'), field('CHOICE', 'select', { options: [{ value: 'one', label: '一' }, { value: 'two', label: '二' }] })]);
  assert.deepEqual(validate(value, draft({ FLAG: 'true', CHOICE: 'two' })), {});
  const result = validate(value, draft({ FLAG: '0', CHOICE: 'unlisted' }));
  assert.equal(result.FLAG.blocking, true); assert.equal(result.CHOICE.blocking, true);
});

test('URL validation enforces HTTPS and rejects unsafe components including empty query or fragment', () => {
  const value = app([field('URL', 'url')]);
  for (const input of ['https://example.invalid', 'https://example.invalid/v1/', 'https://example.invalid/a%3Fb%23c', 'https://例子.测试/v1']) assert.deepEqual(validate(value, draft({ URL: input })), {}, input);
  for (const input of ['not a URL', 'http://127.0.0.1:9000', 'ftp://example.invalid', 'https://user:secret@example.invalid', 'https://user@example.invalid', 'https://example.invalid?token=secret', 'https://example.invalid?', 'https://example.invalid#', 'https://example.invalid#key', ' https://example.invalid', 'https://example.invalid ', 'https://example.invalid/\npath']) assert.equal(validate(value, draft({ URL: input })).URL.blocking, true, input);
});

test('Ordinary text follows UTF-8 byte limits and rejects C0 and C1 control characters', () => {
  const value = app([field('TEXT')]);
  for (const input of ['text', ' padded text ', '文'.repeat(1333), 'x'.repeat(4000)]) assert.deepEqual(validate(value, draft({ TEXT: input })), {});
  for (const input of ['文'.repeat(1334), 'x'.repeat(4001), 'x\u0000y', 'x\u007fy', 'x\u0085y', 'x\u009fy']) assert.equal(validate(value, draft({ TEXT: input })).TEXT.blocking, true);
});

test('Blank password keeps a saved key and absent required keys remain incomplete', () => {
  const value = app(linkedFields, { secretStatus: { KEY: 'set' } });
  assert.deepEqual(validate(value, draft()), {});
  assert.deepEqual(validate(value, draft({}, { KEY: { action: 'keep', value: '' } })), {});
  assert.equal(validate(app(linkedFields), draft()).KEY.blocking, false);
});

test('Explicit unbind can save a required key as missing and ignores obsolete endpoint confirmation', () => {
  const value = app(linkedFields, { secretStatus: { KEY: 'set' } });
  const edit = draft({ URL: 'https://other.invalid/api' }, { KEY: { action: 'unbind', value: '' } });
  assert.deepEqual(validate(value, edit), { KEY: { message: '请填写此项', blocking: false } });
  const optional = app(linkedFields.map((item) => ({ ...item, required: false })), { secretStatus: { KEY: 'endpoint-changed' } });
  assert.deepEqual(validate(optional, edit), {});
});

test('Secret replacement validates content and byte limits without exposing its value', () => {
  const value = app([field('KEY', 'string', { secret: true, required: true })]);
  for (const input of [' artificial key ', '密'.repeat(2730), 'x'.repeat(8192)]) assert.deepEqual(validate(value, draft({}, { KEY: { action: 'replace', value: input } })), {});
  for (const input of ['   ', '密'.repeat(2731), 'x'.repeat(8193), 'private\nvalue', 'private\u0085value']) {
    const result = validate(value, draft({}, { KEY: { action: 'replace', value: input } }));
    assert.equal(result.KEY.blocking, true);
    assert.equal(JSON.stringify(result).includes(input), false);
  }
});

test('Changing a saved credential target requires explicit rebind, replacement or removal', () => {
  const value = app(linkedFields, { secretStatus: { KEY: 'set' } });
  const ordinary = { URL: 'https://other.invalid/api' };
  assert.equal(validate(value, draft(ordinary)).KEY.blocking, true);
  assert.deepEqual(validate(value, draft(ordinary, { KEY: { action: 'rebind', value: '' } })), {});
  assert.deepEqual(validate(value, draft(ordinary, { KEY: key })), {});
  assert.deepEqual(validate(value, draft({ URL: 'https://example.invalid/api' })), {});
});

test('Saved explicit endpoint takes precedence over its author default', () => {
  const value = app(linkedFields, { values: { URL: 'https://saved.invalid/api' }, secretStatus: { KEY: 'set' } });
  assert.deepEqual(validate(value, draft({ URL: 'https://saved.invalid/api' })), {});
  assert.equal(validate(value, draft()).KEY.blocking, true, 'Clearing the override changes the target back to the author default');
});

test('Author endpoint changes and optional credentials retain the same confirmation guard', () => {
  const value = app(linkedFields.map((item) => ({ ...item, required: false })), { secretStatus: { KEY: 'endpoint-changed' } });
  assert.equal(validate(value, draft()).KEY.blocking, true);
  assert.deepEqual(validate(value, draft({}, { KEY: { action: 'rebind', value: '' } })), {});
});

test('Replacement or rebind requires a valid linked address even when it is optional', () => {
  const fields = [field('URL', 'url'), field('KEY', 'string', { secret: true, endpointField: 'URL' })];
  const value = app(fields, { secretStatus: { KEY: 'set' } });
  assert.equal(validate(value, draft({}, { KEY: key })).KEY.blocking, true);
  assert.equal(validate(value, draft({ URL: 'http://example.invalid' }, { KEY: { action: 'rebind', value: '' } })).KEY.blocking, true);
  assert.equal(validate(app(fields), draft({ URL: 'https://example.invalid' }, { KEY: { action: 'rebind', value: '' } })).KEY.blocking, true);
});

test('Validation reports no input values and does not infer backend launch authorization', () => {
  const value = app(linkedFields, { secretStatus: { KEY: 'set' }, canStart: false });
  assert.deepEqual(validate(value, draft()), {});
  assert.equal(value.configuration.canStart, false);
  const result = validate(value, draft({ URL: 'http://private.invalid?secret=artificial' }, { KEY: { action: 'replace', value: 'artificial\nsecret' } }));
  assert.doesNotMatch(JSON.stringify(result), /private\.invalid|artificial/);
});
