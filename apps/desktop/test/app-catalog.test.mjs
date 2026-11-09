import assert from 'node:assert/strict';
import { after, test } from 'node:test';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import ts from 'typescript';

const root = fileURLToPath(new URL('../../../', import.meta.url));
const artifacts = path.join(root, '.test-artifacts');
await mkdir(artifacts, { recursive: true });
const output = await mkdtemp(path.join(artifacts, 'app-catalog-'));
after(() => rm(output, { recursive: true, force: true }));
for (const name of ['git-repository', 'app-catalog-model']) {
  const original = await readFile(path.join(root, 'apps/desktop/src', `${name}.ts`), 'utf8');
  const source = original.replace("from './git-repository'", "from './git-repository.mjs'")
    .replace("import { invoke } from '@tauri-apps/api/core';", 'const invoke = () => { throw new Error("Unexpected native call"); };');
  await writeFile(path.join(output, `${name}.mjs`), ts.transpileModule(source, {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext },
  }).outputText);
}
const { parseAppCatalog, catalogSourceUrl, availableCatalogApps } = await import(pathToFileURL(path.join(output, 'app-catalog-model.mjs')).href);
const github = 'https://github.com/disposable-fixture/example.git';
const gitee = 'https://gitee.com/disposable-fixture/example.git';
const entry = { id: 'example', name: 'Example', summary: 'Disposable catalog fixture', repo: { github, gitee }, needs: [] };
const catalog = repo => ({ version: 1, apps: [{ ...entry, repo }] });

test('Current and cached single-address catalogs normalize to provider-specific repositories', () => {
  assert.deepEqual(parseAppCatalog(catalog(github))[0].repo, { github });
  assert.deepEqual(parseAppCatalog(catalog(gitee))[0].repo, { gitee });
  assert.deepEqual(parseAppCatalog(catalog({ github, gitee }))[0].repo, { github, gitee });
  for (const repo of [{}, [], null, { github: gitee }, { gitee: github }, { github: 'http://github.com/a/b' }]) {
    assert.throws(() => parseAppCatalog(catalog(repo)));
  }
});

test('GitHub is preferred today and a registered Gitee address can be selected later', () => {
  const app = parseAppCatalog(catalog({ github, gitee }))[0];
  assert.equal(catalogSourceUrl(app), github);
  assert.equal(catalogSourceUrl(app, 'gitee'), gitee);
  assert.equal(catalogSourceUrl(parseAppCatalog(catalog({ github }))[0], 'gitee'), github);
  assert.equal(catalogSourceUrl(parseAppCatalog(catalog({ gitee }))[0]), gitee);
});

test('Agent recommendation markers are optional booleans and preserve an explicit false', () => {
  assert.equal(parseAppCatalog(catalog({ github }))[0].agent, undefined);
  for (const agent of [true, false]) {
    assert.equal(parseAppCatalog({ version: 1, apps: [{ ...entry, agent }] })[0].agent, agent);
  }
  for (const agent of [null, 'true', 1, {}, []]) {
    assert.throws(() => parseAppCatalog({ version: 1, apps: [{ ...entry, agent }] }));
  }
});

test('A saved catalog ID hides an application after its repository changes, and older sources match either provider', () => {
  const apps = parseAppCatalog(catalog({ github, gitee }));
  assert.deepEqual(availableCatalogApps(apps, [], ['example']), []);
  assert.deepEqual(availableCatalogApps(apps, [github.replace('.git', '')]), []);
  assert.deepEqual(availableCatalogApps(apps, [gitee]), []);
  assert.deepEqual(availableCatalogApps(apps, [], ['another-app']), apps);
});

test('Published recommendations contain the current-format applications, the agent app and provider-specific addresses', async () => {
  const apps = parseAppCatalog(JSON.parse(await readFile(path.join(root, 'catalog/apps.json'), 'utf8')));
  assert.deepEqual(apps.map(app => app.id), ['gleaner', 'clack', 'readio', 'radio-bob']);
  assert.equal(apps.find(app => app.id === 'gleaner').agent, true);
  assert.deepEqual(apps.filter(app => app.agent).map(app => app.id), ['gleaner']);
  assert.ok(apps.every(app => app.repo.github && app.repo.gitee));
  assert.deepEqual(apps.find(app => app.id === 'readio').needs, ['DeepSeek API Key', '豆包语音 API Key']);
});
