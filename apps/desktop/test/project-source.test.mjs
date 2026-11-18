import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import ts from 'typescript';

function moduleUrl(source) {
  const { outputText } = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 } });
  return `data:text/javascript;base64,${Buffer.from(outputText).toString('base64')}`;
}

const repoParser = moduleUrl(await readFile(new URL('../src/git-repository.ts', import.meta.url), 'utf8'));
const source = await readFile(new URL('../src/project-source.ts', import.meta.url), 'utf8');
const { projectSource } = await import(moduleUrl(source.replace("'./git-repository'", JSON.stringify(repoParser))));

const projectRoot = '/disposable/ownership-fixture/app';
const documentEpoch = '12345678-1234-4123-8123-123456789012';
const gitSource = { url: 'https://github.com/example-owner/example-app.git', branch: 'main' };

test('Project sources identify all four ownership and Git combinations', () => {
  const base = { documentEpoch, projectRoot };
  assert.deepEqual(projectSource({ ...base, ownership: 'platform', gitSource }), {
    ownership: 'platform', isLocal: false, kind: 'downloaded', host: 'GitHub', repository: 'example-owner/example-app',
  });
  assert.deepEqual(projectSource({ ...base, ownership: 'platform', gitSource: null }), {
    ownership: 'platform', isLocal: false, kind: 'created',
  });
  assert.deepEqual(projectSource({ ...base, ownership: 'user', gitSource }), {
    ownership: 'user', isLocal: true, kind: 'downloaded', path: projectRoot,
  });
  assert.deepEqual(projectSource({ ...base, ownership: 'user', gitSource: null }), {
    ownership: 'user', isLocal: true, kind: 'local', path: projectRoot,
  });
});

test('Unverified registration does not turn a placeholder owner or registration path into a local folder', () => {
  assert.deepEqual(projectSource({ documentEpoch: '', ownership: 'user', projectRoot, gitSource: null }), {
    ownership: null, isLocal: false, kind: 'unknown',
  });
});

test('Git source falls back to its original address when it cannot be parsed', () => {
  const url = 'https://git.example.com/owner/repo';
  assert.deepEqual(projectSource({ documentEpoch, ownership: 'platform', projectRoot, gitSource: { url } }), {
    ownership: 'platform', isLocal: false, kind: 'downloaded', url,
  });
});
