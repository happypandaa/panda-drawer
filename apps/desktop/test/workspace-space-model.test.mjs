// Pure accounting fixtures only; no filesystem scan or user data is accessed.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import ts from 'typescript';

const file = new URL('../src/workspace-space-model.ts', import.meta.url);
const source = ts.transpileModule(await readFile(file, 'utf8'), {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext },
  fileName: file.pathname,
}).outputText;
const { spaceOverview } = await import(`data:text/javascript;base64,${Buffer.from(source).toString('base64')}`);

const part = (totalBytes, extra = {}) => ({ totalBytes, complete: true, stale: false, ...extra });
const category = (bytes) => ({ bytes });
const snapshot = (extra = {}) => ({ platformProgram: part(0), workspace: part(0, { categories: {} }), otherLocalLocations: [], ...extra });
const group = (model, id) => model.groups.find((item) => item.id === id)?.part;
const bigint = (value) => typeof value === 'number' ? BigInt(value) : BigInt(value.$int64);

test('An absent snapshot has no fabricated totals or groups', () => {
  assert.deepEqual(spaceOverview(null), { total: null, complete: false, stale: false, groups: [], barReady: false });
});

test('All disjoint scopes are counted once while project references and disk capacity are ignored', () => {
  const model = spaceOverview(snapshot({
    platformProgram: part(100),
    workspace: part(500, { categories: { projects: category(400), control: category(100) }, projects: [part(400, { referenceOnly: true })] }),
    otherLocalLocations: [part(200, { scope: { kind: 'applicationData' } })],
    knownLocalTotal: part(800),
    disks: [{ availableBytes: 9_000_000, totalBytes: 10_000_000 }],
  }));
  assert.equal(model.total, 800);
  assert.equal(group(model, 'projects').totalBytes, 400);
  assert.equal(group(model, 'apps').totalBytes, 200);
  assert.equal(model.barReady, true);
  assert.equal(model.groups.reduce((sum, item) => sum + item.part.totalBytes, 0), 800);
});

test('Purpose groups preserve backup, operation, old deletion, retained root, and control bytes', () => {
  const model = spaceOverview(snapshot({
    workspace: part(36, { categories: { projects: category(1), backups: category(2), operations: category(3), trash: category(4), sharedCache: category(5), control: category(6), metadata: category(7), logs: category(8) } }),
    otherLocalLocations: [part(9, { scope: { kind: 'retainedWorkspace' } })],
  }));
  assert.equal(group(model, 'history').totalBytes, 18);
  assert.equal(group(model, 'cache').totalBytes, 26);
  assert.equal(model.total, 45);
  assert.equal(model.barReady, true);
});

test('Unrecognized workspace categories and external scopes remain visible as other files', () => {
  const model = spaceOverview(snapshot({
    workspace: part(10, { categories: { futureCategory: category(10) } }),
    otherLocalLocations: [part(20, { scope: { kind: 'futureScope' } }), part(30)],
  }));
  assert.equal(group(model, 'other').totalBytes, 60);
  assert.equal(model.total, 60);
  assert.equal(model.barReady, true);
});

test('Complete fresh zero groups disappear and zero totals never produce a percentage bar', () => {
  assert.deepEqual(spaceOverview(snapshot()), { total: 0, complete: true, stale: false, groups: [], barReady: false });
});

test('Partial and stale zero scopes remain visible instead of being presented as empty', () => {
  const model = spaceOverview(snapshot({
    workspace: part(0, { complete: false }),
    platformProgram: part(0, { stale: true }),
  }));
  assert.equal(group(model, 'workspace').totalBytes, 0);
  assert.equal(group(model, 'workspace').complete, false);
  assert.equal(group(model, 'program').totalBytes, 0);
  assert.equal(group(model, 'program').stale, true);
  assert.equal(model.complete, false);
  assert.equal(model.stale, true);
  assert.equal(model.barReady, false);
  const emptyCategories = spaceOverview(snapshot({ workspace: part(0, { categories: {}, complete: false }) }));
  assert.equal(group(emptyCategories, 'workspace').totalBytes, 0);
});

test('An unknown scope preserves the measured lower bound without claiming a complete total', () => {
  const model = spaceOverview(snapshot({
    workspace: part(40, { complete: false, categories: { projects: category(40) } }),
    otherLocalLocations: [part(null, { complete: false, scope: { kind: 'retainedWorkspace' } })],
    knownLocalTotal: part(40, { complete: false }),
  }));
  assert.equal(model.total, 40);
  assert.equal(group(model, 'history').totalBytes, null);
  assert.equal(group(model, 'projects').complete, false);
  assert.equal(model.complete, false);
  assert.equal(model.barReady, false);
});

test('A mixture of known and unknown historical roots keeps the observed history bytes', () => {
  const model = spaceOverview(snapshot({
    workspace: part(5, { categories: { backups: category(5) } }),
    otherLocalLocations: [part(10, { scope: { kind: 'retainedWorkspace' } }), part(null, { complete: false, scope: { kind: 'retainedWorkspace' } })],
  }));
  assert.equal(group(model, 'history').totalBytes, 15);
  assert.equal(group(model, 'history').complete, false);
  assert.equal(model.total, 15);
  assert.equal(model.barReady, false);
});

test('Scopes without measurements remain unknown even when an alternative legacy size is present', () => {
  const model = spaceOverview(snapshot({ workspace: part(null, { bytes: 100, complete: false }), platformProgram: part(null, { complete: false }) }));
  assert.equal(model.total, null);
  assert.equal(group(model, 'workspace').totalBytes, null);
  assert.equal(model.complete, false);
});

test('A backend zero accumulator cannot turn entirely unknown scopes into measured empty storage', () => {
  const data = snapshot({
    workspace: part(null, { complete: false, categories: { projects: category(null) } }),
    platformProgram: part(null, { complete: false }),
    otherLocalLocations: [part(null, { complete: false, scope: { kind: 'retainedWorkspace' } })],
    knownLocalTotal: part(0, { complete: false }),
  });
  const model = spaceOverview(data);
  assert.equal(model.total, null);
  assert.equal(model.complete, false);
  assert.equal(model.barReady, false);
  assert.deepEqual(model.groups.map((item) => item.id), ['workspace', 'program', 'history']);
  assert.ok(model.groups.every((item) => item.part.totalBytes === null));
  const measuredZero = spaceOverview({ ...data, platformProgram: part(0) });
  assert.equal(measuredZero.total, 0, 'A genuine zero measurement still counts as an observed lower bound');
  assert.equal(measuredZero.complete, false);
});

test('Missing or inconsistent category breakdowns use the independently measured workspace scope', () => {
  for (const categories of [undefined, { projects: category(5) }, { projects: category(null) }, { projects: null }, []]) {
    const model = spaceOverview(snapshot({ workspace: part(10, { categories }) }));
    assert.equal(group(model, 'workspace').totalBytes, 10);
    assert.equal(group(model, 'projects'), undefined);
    assert.equal(model.total, 10);
    assert.equal(model.barReady, true);
  }
});

test('Unknown categories in a partial scan never invent missing zero size buckets', () => {
  const model = spaceOverview(snapshot({ workspace: part(10, { complete: false, categories: { projects: category(10), backups: category(null) } }) }));
  assert.deepEqual(model.groups.map((item) => item.id), ['workspace']);
  assert.equal(model.barReady, false);
});

test('Large integer counters remain exact through aggregation and conservation checks', () => {
  const large = { $int64: '9007199254740993' };
  const model = spaceOverview(snapshot({
    workspace: part(large, { categories: { projects: category(large) } }),
    platformProgram: part(7),
    otherLocalLocations: [part({ $int64: '9007199254741001' }, { scope: { kind: 'applicationData' } })],
  }));
  assert.deepEqual(model.total, { $int64: '18014398509482001' });
  assert.equal(model.groups.reduce((sum, item) => sum + bigint(item.part.totalBytes), 0n), bigint(model.total));
  assert.equal(model.barReady, true);
});

test('Unsafe, negative, noninteger, or malformed values are unknown instead of rounded or subtracted', () => {
  for (const value of [Number.MAX_SAFE_INTEGER + 1, -1, 1.5, Infinity, { $int64: '-2' }, { $int64: '1.2' }, { $int64: 'bad' }]) {
    const model = spaceOverview(snapshot({ workspace: part(value), platformProgram: part(null, { complete: false }) }));
    assert.equal(model.total, null);
    assert.equal(model.complete, false);
    assert.equal(model.barReady, false);
  }
});

test('A reported total which disagrees with scope measurements cannot enable a misleading complete bar', () => {
  const model = spaceOverview(snapshot({ workspace: part(10), knownLocalTotal: part(11) }));
  assert.equal(model.total, 11);
  assert.equal(model.complete, false);
  assert.equal(model.barReady, false);
});

test('Stale data suppresses the distribution even when all measured numbers are complete', () => {
  const model = spaceOverview(snapshot({ workspace: part(10, { stale: true, categories: { projects: category(10) } }) }));
  assert.equal(model.complete, true);
  assert.equal(model.stale, true);
  assert.equal(group(model, 'projects').stale, true);
  assert.equal(model.barReady, false);
});

test('The most recent valid scope timestamp is retained without claiming a shared scan time', () => {
  const model = spaceOverview(snapshot({
    workspace: part(0, { completedAt: '2026-10-22T00:00:00Z' }),
    platformProgram: part(0, { completedAt: 'not-a-date' }),
    otherLocalLocations: [part(0, { completedAt: Date.parse('2026-10-22T12:00:00Z') }), part(0, { completedAt: '2026-10-23T00:00:00Z' })],
  }));
  assert.equal(model.updatedAt, '2026-10-23T00:00:00Z');
});
