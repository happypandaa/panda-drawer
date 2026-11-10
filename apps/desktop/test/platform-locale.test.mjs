// Preference/default tests only; no real platform data or OS settings are changed.
import assert from 'node:assert/strict';
import { after, test } from 'node:test';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';

const repo = fileURLToPath(new URL('../../../', import.meta.url));
await mkdir(path.join(repo, '.test-artifacts'), { recursive: true });
const output = await mkdtemp(path.join(repo, '.test-artifacts/disposable-platform-locale-'));
const mock = path.join(output, 'tauri.mjs');
await writeFile(mock, 'export const isTauri = () => globalThis.__nativeLocale; export const invoke = async () => globalThis.__savedNativeLocale;');
const compiled = new Map();
async function compile(file) {
  if (compiled.has(file)) return compiled.get(file);
  const target = path.join(output, path.basename(file).replace(/\.ts$/, '.mjs'));
  compiled.set(file, target);
  let source = ts.transpileModule(await readFile(file, 'utf8'), { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext } }).outputText;
  for (const match of [...source.matchAll(/from\s+(['"])(\.[^'"]+)\1/g)]) {
    const dependency = path.resolve(path.dirname(file), `${match[2]}.ts`);
    source = source.replaceAll(`${match[1]}${match[2]}${match[1]}`, JSON.stringify(pathToFileURL(await compile(dependency)).href));
  }
  source = source.replaceAll(/(['"])@tauri-apps\/api\/core\1/g, JSON.stringify(pathToFileURL(mock).href));
  await writeFile(target, source);
  return target;
}
const moduleUrl = pathToFileURL(await compile(path.join(repo, 'apps/desktop/src/i18n.ts'))).href;
let sequence = 0;
async function load(language, saved, native = false) {
  const storage = new Map(saved ? [['panda-drawer:language:v1', saved]] : []);
  Object.defineProperty(globalThis, 'navigator', { configurable: true, value: { language } });
  globalThis.window = { localStorage: { getItem: key => storage.get(key) ?? null, setItem: (key, value) => storage.set(key, value) } };
  globalThis.document = { documentElement: {}, title: '' };
  globalThis.__nativeLocale = native;
  return { module: await import(`${moduleUrl}?case=${++sequence}`), storage };
}
after(async () => { await rm(output, { recursive: true, force: true }); });

test('unsaved browser language maps all Chinese variants to zh-CN and others to en', async () => {
  for (const [language, expected] of [['zh-CN', 'zh-CN'], ['zh-Hant-TW', 'zh-CN'], ['en-US', 'en'], ['ja-JP', 'en']]) {
    const { module } = await load(language);
    await module.initializeLocale();
    assert.equal(module.getLocale(), expected);
    assert.equal(document.documentElement.lang, expected);
  }
});
test('saved manual choice wins over browser language and survives a system change', async () => {
  const { module, storage } = await load('en-US');
  module.setLocale('zh-CN');
  const next = await load('fr-FR', storage.get('panda-drawer:language:v1'));
  await next.module.initializeLocale();
  assert.equal(next.module.getLocale(), 'zh-CN');
  assert.equal(document.title, '熊猫小抽屉 · 开发版');
});
test('native startup locale is authoritative over browser defaults and browser storage', async () => {
  globalThis.__savedNativeLocale = 'en';
  const { module } = await load('zh-CN', 'zh-CN', true);
  await module.initializeLocale();
  assert.equal(module.getLocale(), 'en');
  assert.equal(document.title, 'Panda Drawer · Development');
});
test('native saved Chinese choice wins even when the browser reports English', async () => {
  globalThis.__savedNativeLocale = 'zh-CN';
  const { module } = await load('en-US', undefined, true);
  await module.initializeLocale();
  assert.equal(module.getLocale(), 'zh-CN');
});
