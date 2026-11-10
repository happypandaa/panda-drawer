import assert from 'node:assert/strict';
import test from 'node:test';
import { readFile } from 'node:fs/promises';
import ts from 'typescript';

const source = await readFile(new URL('../src/app-localization.ts', import.meta.url), 'utf8');
const compiled = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext } }).outputText;
const { localizedApp, localizedFields, localizedProject, localizedRequirements } = await import('data:text/javascript;base64,' + Buffer.from(compiled).toString('base64'));

test('Display translations fall back per field and leave original metadata and configuration identifiers intact', () => {
  const fields = [{ name: 'MODE', label: '模式', description: '原说明', placeholder: '原提示', howToGet: ['原步骤'], type: 'select', required: true, secret: false, default: 'one', options: [{ value: 'one', label: '一' }, { value: 'two', label: '二' }] }];
  const app = { name: '示例', description: '原简介', version: '1.0' };
  const localization = { en: { app: { name: 'Example' }, configuration: { MODE: { label: 'Mode', howToGet: ['First step', 'Second step'], options: { one: 'One' } } } } };
  const original = structuredClone({ fields, app, localization });
  assert.deepEqual(localizedApp(app, localization, 'en'), { ...app, name: 'Example' });
  const translated = localizedFields(fields, localization, 'en')[0];
  assert.equal(translated.label, 'Mode');
  assert.equal(translated.description, '原说明');
  assert.equal(translated.placeholder, '原提示');
  assert.deepEqual(translated.howToGet, ['First step', 'Second step']);
  assert.deepEqual(translated.options, [{ value: 'one', label: 'One' }, { value: 'two', label: '二' }]);
  assert.equal(translated.default, 'one');
  assert.equal(translated.required, true);
  assert.equal(translated.name, 'MODE');
  assert.deepEqual(localizedFields(fields, localization, 'zh-CN'), fields);
  assert.deepEqual({ fields, app, localization }, original);
});

test('Network translations match group and origin while retaining addresses and device declarations', () => {
  const requirements = { network: { connect: [{ origin: 'https://example.invalid', purpose: '连接用途', dataSent: '原发送内容' }], images: [{ origin: 'https://example.invalid', purpose: '图片用途', dataSent: '图片请求' }] }, devices: { camera: { purpose: '拍照' } } };
  const localization = { en: { runtimeRequirements: { network: { connect: { 'https://example.invalid': { purpose: 'Service connection' } } }, devices: { camera: { purpose: 'Take a photo' } } } } };
  const original = structuredClone(requirements);
  const translated = localizedRequirements(requirements, localization, 'en');
  assert.deepEqual(translated.network.connect[0], { ...requirements.network.connect[0], purpose: 'Service connection' });
  assert.deepEqual(translated.network.images, requirements.network.images);
  assert.deepEqual(translated.devices, { camera: { purpose: 'Take a photo' } });
  assert.deepEqual(requirements, original);
  assert.equal(localizedRequirements(undefined, localization, 'en'), undefined);
});

test('Switching the current language projects the retained original metadata without retaining a previous translation', () => {
  const raw = { id: 'disposable', name: '原名', description: '', manifest: { app: { name: '原名' }, localization: { en: { app: { name: 'Translated', description: 'English description' } } } } };
  const english = localizedProject(raw, 'en');
  assert.equal(english.name, 'Translated');
  const chinese = localizedProject(raw, 'zh-CN');
  assert.equal(chinese.name, '原名');
  assert.equal(chinese.description, '');
  assert.equal(english.manifest, raw.manifest);
  assert.equal(raw.name, '原名');
});

test('An untranslated project retains its exposed metadata even when manifest app metadata differs', () => {
  const project = { id: 'disposable', name: 'Exposed project name', description: 'Exposed project description', manifest: { app: { name: 'Other manifest name', description: 'Other manifest description' }, localization: { en: { app: { description: 'Translated description' } } } } };
  assert.equal(localizedProject(project, 'zh-CN'), project);
  assert.deepEqual(localizedProject(project, 'en'), { ...project, description: 'Translated description' });
  assert.equal(project.name, 'Exposed project name');
});
