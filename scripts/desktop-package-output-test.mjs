import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, readFile, readdir, realpath, rm, symlink, lstat } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { desktopPackagePlan, withDesktopPackageOutput } from './desktop-package-output.mjs';

const configuration = {
  productName: '熊猫应用小抽屉', version: '0.0.0', identifier: 'dev.localprojects.platform.m0', bundle: { targets: ['app', 'dmg'] },
};
const planFor = (root, args = ['build', '--bundles', 'app']) => desktopPackagePlan({ root, args, configuration, environment: {}, platform: 'darwin', arch: 'arm64' });
const appFile = app => path.join(app, 'Contents', 'identity.json');
async function fixture(t, args) {
  const root = await realpath(await mkdtemp(path.join(os.tmpdir(), 'desktop-package-test-')));
  t.after(() => rm(root, { recursive: true, force: true }));
  const plan = planFor(root, args);
  const bundle = path.join(root, 'target', 'release', 'bundle');
  const app = path.join(bundle, 'macos', '熊猫应用小抽屉.app');
  const dmg = path.join(bundle, 'dmg', '熊猫应用小抽屉_0.0.0_aarch64.dmg');
  const oldDmg = path.join(bundle, 'dmg', '熊猫应用小抽屉_0.0.0-old_aarch64.dmg');
  const legacyApp = path.join(bundle, 'macos', 'Local Projects Dev.app');
  const legacyDmg = path.join(bundle, 'dmg', 'Local Projects Dev_0.0.0_aarch64.dmg');
  const writeNamedApp = async (file, name, value = 'old') => {
    await mkdir(path.join(file, 'Contents'), { recursive: true });
    await writeFile(appFile(file), JSON.stringify({ identifier: plan.identifier, bundleName: name, executable: 'local-projects-desktop-dev', value }));
  };
  const writeApp = (value = 'old') => writeNamedApp(app, plan.productName, value);
  const writeLegacyApp = (value = 'legacy') => writeNamedApp(legacyApp, 'Local Projects Dev', value);
  await writeApp();
  await mkdir(path.dirname(dmg), { recursive: true });
  await writeFile(dmg, 'old-dmg');
  const verifiers = {
    processes: async () => [],
    verifyApp: async (file, candidatePlan) => {
      assert.equal((await lstat(file)).isSymbolicLink(), false);
      const identity = JSON.parse(await readFile(appFile(file), 'utf8'));
      assert.equal(identity.identifier, plan.identifier);
      assert.equal(identity.bundleName, candidatePlan.productName);
      assert.equal(identity.executable, 'local-projects-desktop-dev');
    },
    verifyDmg: async (file, candidatePlan) => {
      const info = await lstat(file);
      assert.ok(info.isFile() && !info.isSymbolicLink() && info.nlink === 1);
      assert.ok(path.basename(file).startsWith(`${candidatePlan.productName}_`));
      assert.match(await readFile(file, 'utf8'), /^(old|new)-dmg$/);
    },
  };
  return { root, plan, bundle, app, dmg, oldDmg, legacyApp, legacyDmg, writeApp, writeLegacyApp, writeNamedApp, verifiers };
}
async function noTransaction(f) {
  assert.deepEqual((await readdir(path.join(f.root, 'target'))).sort(), ['release']);
  assert.deepEqual((await readdir(path.join(f.root, 'target', 'release'))).sort(), ['bundle']);
}

test('standard plans recognize app, dmg and debug without changing application identity', () => {
  assert.equal(planFor('/some/repository').expectedDmg, false);
  assert.equal(planFor('/some/repository', ['build']).expectedDmg, true);
  assert.equal(planFor('/some/repository', ['build', '--bundles=dmg', '--debug']).profile, 'debug');
  assert.equal(planFor('/some/repository', ['dev']), undefined);
  assert.equal(planFor('/some/repository', ['build', '--help']), undefined);
  assert.equal(planFor('/some/repository', ['build', '--no-bundle']), undefined);
  assert.equal(desktopPackagePlan({ root: '/x', args: ['build'], configuration, platform: 'win32' }), undefined);
});

test('rejects custom output or identity overrides and unknown bundle types before mutation', () => {
  for (const args of [['build', '--config', 'other'], ['build', '-cother'], ['build', '--target=x'], ['build', '-tx'], ['build', '--profile', 'other'], ['build', '--bundles', 'deb'], ['build', '--bundles']]) {
    assert.throws(() => planFor('/x', args));
  }
  for (const key of ['CARGO_TARGET_DIR', 'CARGO_BUILD_TARGET', 'TAURI_CONFIG']) {
    assert.throws(() => desktopPackagePlan({ root: '/x', args: ['build'], configuration, platform: 'darwin', arch: 'arm64', environment: { [key]: 'override' } }));
  }
  assert.throws(() => desktopPackagePlan({ root: '/x', args: ['build'], configuration: { ...configuration, identifier: 'other' }, platform: 'darwin', arch: 'arm64', environment: {} }));
  for (const productName of ['Local Projects Dev', '熊猫应用小抽屉 Copy', '../熊猫应用小抽屉']) {
    assert.throws(() => desktopPackagePlan({ root: '/x', args: ['build'], configuration: { ...configuration, productName }, platform: 'darwin', arch: 'arm64', environment: {} }));
  }
});

test('brand migration removes only both exact verified generated product names', async t => {
  const f = await fixture(t);
  await f.writeLegacyApp();
  await writeFile(f.legacyDmg, 'old-dmg');
  const unrelatedApp = path.join(path.dirname(f.app), 'Local Projects Developer.app');
  const unrelatedDmg = path.join(path.dirname(f.dmg), 'Local Projects Dev Copy_0.0.0_aarch64.dmg');
  await f.writeNamedApp(unrelatedApp, 'Local Projects Developer', 'preserved');
  await writeFile(unrelatedDmg, 'preserved');
  const checkedApps = [];
  const checkedDmgs = [];
  const report = await withDesktopPackageOutput(f.plan, async () => f.writeApp('new'), {
    ...f.verifiers,
    verifyApp: async (file, plan) => { checkedApps.push([file, plan.productName]); await f.verifiers.verifyApp(file, plan); },
    verifyDmg: async (file, plan) => { checkedDmgs.push([file, plan.productName]); await f.verifiers.verifyDmg(file, plan); },
  });
  assert.equal(report.removedPrevious, 4);
  assert.deepEqual(checkedApps, [[f.app, '熊猫应用小抽屉'], [f.legacyApp, 'Local Projects Dev'], [f.app, '熊猫应用小抽屉']]);
  assert.deepEqual(new Map(checkedDmgs), new Map([[f.dmg, '熊猫应用小抽屉'], [f.legacyDmg, 'Local Projects Dev']]));
  assert.equal(JSON.parse(await readFile(appFile(f.app))).value, 'new');
  for (const file of [f.legacyApp, f.dmg, f.legacyDmg]) await assert.rejects(lstat(file), { code: 'ENOENT' });
  assert.equal(JSON.parse(await readFile(appFile(unrelatedApp))).value, 'preserved');
  assert.equal(await readFile(unrelatedDmg, 'utf8'), 'preserved');
  await noTransaction(f);
});

test('failed renamed build restores both original brands at their exact previous paths', async t => {
  const f = await fixture(t, ['build']);
  await f.writeLegacyApp();
  await writeFile(f.legacyDmg, 'old-dmg');
  await assert.rejects(withDesktopPackageOutput(f.plan, async () => {
    await f.writeApp('partial');
    await writeFile(f.dmg, 'partial-dmg');
    throw new Error('intentional renamed build failure');
  }, f.verifiers), /intentional renamed build failure/);
  assert.equal(JSON.parse(await readFile(appFile(f.app))).value, 'old');
  assert.equal(JSON.parse(await readFile(appFile(f.legacyApp))).value, 'legacy');
  assert.equal(await readFile(f.dmg, 'utf8'), 'old-dmg');
  assert.equal(await readFile(f.legacyDmg, 'utf8'), 'old-dmg');
  await noTransaction(f);
});

test('first renamed build failure restores legacy packages without retaining the partial new brand', async t => {
  const f = await fixture(t, ['build']);
  await rm(f.app, { recursive: true });
  await rm(f.dmg);
  await f.writeLegacyApp();
  await writeFile(f.legacyDmg, 'old-dmg');
  await assert.rejects(withDesktopPackageOutput(f.plan, async () => {
    await f.writeApp('partial');
    await writeFile(f.dmg, 'partial-dmg');
    throw new Error('intentional first renamed build failure');
  }, f.verifiers), /intentional first renamed build failure/);
  assert.equal(JSON.parse(await readFile(appFile(f.legacyApp))).value, 'legacy');
  assert.equal(await readFile(f.legacyDmg, 'utf8'), 'old-dmg');
  for (const file of [f.app, f.dmg]) await assert.rejects(lstat(file), { code: 'ENOENT' });
  await noTransaction(f);
});

test('legacy app running under its historical path blocks a renamed build before mutation', async t => {
  const f = await fixture(t);
  await f.writeLegacyApp();
  await writeFile(f.legacyDmg, 'old-dmg');
  await assert.rejects(withDesktopPackageOutput(f.plan, async () => assert.fail('must not build'), {
    ...f.verifiers,
    processes: async () => [`${f.legacyApp}/Contents/MacOS/local-projects-desktop-dev`],
  }), /请先从菜单退出正在运行的 Local Projects Dev/);
  assert.equal(JSON.parse(await readFile(appFile(f.legacyApp))).value, 'legacy');
  assert.equal(await readFile(f.legacyDmg, 'utf8'), 'old-dmg');
  assert.equal(JSON.parse(await readFile(appFile(f.app))).value, 'old');
  await noTransaction(f);
});

test('a legacy filename with a different bundle name or identity is preserved and blocks cleanup', async t => {
  for (const identity of [
    { identifier: 'dev.localprojects.platform.m0', bundleName: '熊猫应用小抽屉' },
    { identifier: 'foreign', bundleName: 'Local Projects Dev' },
  ]) {
    const f = await fixture(t);
    await f.writeLegacyApp();
    const contents = { ...identity, executable: 'local-projects-desktop-dev', value: 'preserved' };
    await writeFile(appFile(f.legacyApp), JSON.stringify(contents));
    await assert.rejects(withDesktopPackageOutput(f.plan, async () => assert.fail('must not build'), f.verifiers));
    assert.deepEqual(JSON.parse(await readFile(appFile(f.legacyApp))), contents);
    assert.equal(JSON.parse(await readFile(appFile(f.app))).value, 'old');
    assert.equal(await readFile(f.dmg, 'utf8'), 'old-dmg');
    await noTransaction(f);
  }
});

test('app-only success retains new app and removes only verified old app/DMGs', async t => {
  const f = await fixture(t);
  await writeFile(f.oldDmg, 'old-dmg');
  const unrelated = path.join(path.dirname(f.dmg), 'unrelated.dmg');
  await writeFile(unrelated, 'unrelated');
  const report = await withDesktopPackageOutput(f.plan, async () => f.writeApp('new'), f.verifiers);
  assert.equal(report.removedPrevious, 3);
  assert.equal(report.dmg, undefined);
  assert.equal(JSON.parse(await readFile(appFile(f.app))).value, 'new');
  assert.equal(await readFile(unrelated, 'utf8'), 'unrelated');
  await assert.rejects(lstat(f.dmg), { code: 'ENOENT' });
  await assert.rejects(lstat(f.oldDmg), { code: 'ENOENT' });
  await noTransaction(f);
});

test('full build verifies and retains latest app and DMG', async t => {
  const f = await fixture(t, ['build']);
  const report = await withDesktopPackageOutput(f.plan, async () => {
    await f.writeApp('new');
    await writeFile(f.dmg, 'new-dmg');
  }, f.verifiers);
  assert.equal(report.dmg, f.dmg);
  assert.equal(await readFile(f.dmg, 'utf8'), 'new-dmg');
  await noTransaction(f);
});

test('upgrade accepts signed older outputs without development tools and requires tools in new app and DMG', async t => {
  const f = await fixture(t, ['build']);
  await f.writeLegacyApp();
  await writeFile(f.legacyDmg, 'old-dmg');
  let built = false;
  const checks = [];
  const verification = kind => async (file, plan, options) => {
    await f.verifiers[kind](file, plan);
    assert.deepEqual(options, { requireDevelopmentNode: built });
    checks.push([kind, built]);
  };
  const report = await withDesktopPackageOutput(f.plan, async () => {
    built = true;
    await f.writeApp('new');
    await writeFile(f.dmg, 'new-dmg');
  }, { ...f.verifiers, verifyApp: verification('verifyApp'), verifyDmg: verification('verifyDmg') });
  assert.equal(report.removedPrevious, 4);
  assert.deepEqual(checks, [
    ['verifyApp', false], ['verifyApp', false], ['verifyDmg', false], ['verifyDmg', false],
    ['verifyApp', true], ['verifyDmg', true],
  ]);
  await noTransaction(f);
});

test('missing development tools in either new artifact restores the verified older outputs', async t => {
  for (const rejectedKind of ['verifyApp', 'verifyDmg']) {
    const f = await fixture(t, ['build']);
    let built = false;
    await assert.rejects(withDesktopPackageOutput(f.plan, async () => {
      built = true;
      await f.writeApp('new');
      await writeFile(f.dmg, 'new-dmg');
    }, {
      ...f.verifiers,
      [rejectedKind]: async (file, plan, options) => {
        await f.verifiers[rejectedKind](file, plan);
        assert.deepEqual(options, { requireDevelopmentNode: built });
        if (options.requireDevelopmentNode) throw new Error('development toolchain missing or invalid');
      },
    }), /development toolchain missing or invalid/);
    assert.equal(built, true);
    assert.equal(JSON.parse(await readFile(appFile(f.app))).value, 'old');
    assert.equal(await readFile(f.dmg, 'utf8'), 'old-dmg');
    await noTransaction(f);
  }
});

test('failed build restores previous packages and removes partial generated outputs', async t => {
  const f = await fixture(t);
  await writeFile(f.oldDmg, 'old-dmg');
  await assert.rejects(withDesktopPackageOutput(f.plan, async () => {
    await f.writeApp('partial');
    throw new Error('intentional build failure');
  }, f.verifiers), /intentional build failure/);
  assert.equal(JSON.parse(await readFile(appFile(f.app))).value, 'old');
  assert.equal(await readFile(f.dmg, 'utf8'), 'old-dmg');
  assert.equal(await readFile(f.oldDmg, 'utf8'), 'old-dmg');
  await noTransaction(f);
});

test('failed new-package validation restores old app and DMG', async t => {
  const f = await fixture(t, ['build']);
  await assert.rejects(withDesktopPackageOutput(f.plan, async () => f.writeApp('new'), f.verifiers), { code: 'ENOENT' });
  assert.equal(JSON.parse(await readFile(appFile(f.app))).value, 'old');
  assert.equal(await readFile(f.dmg, 'utf8'), 'old-dmg');
  await noTransaction(f);
});

test('running current app blocks preparation and leaves all packages unchanged', async t => {
  const f = await fixture(t);
  let builds = 0;
  await assert.rejects(withDesktopPackageOutput(f.plan, async () => { builds += 1; }, {
    ...f.verifiers, processes: async () => [`${f.app}/Contents/MacOS/local-projects-desktop-dev`],
  }), /请先从菜单退出/);
  assert.equal(builds, 0);
  assert.equal(await readFile(f.dmg, 'utf8'), 'old-dmg');
  await noTransaction(f);
});

test('foreign app identity fails before replacing any generated file', async t => {
  const f = await fixture(t);
  await writeFile(appFile(f.app), JSON.stringify({ identifier: 'foreign', value: 'keep' }));
  await assert.rejects(withDesktopPackageOutput(f.plan, async () => assert.fail('must not build'), f.verifiers));
  assert.equal(JSON.parse(await readFile(appFile(f.app))).value, 'keep');
  assert.equal(await readFile(f.dmg, 'utf8'), 'old-dmg');
  await noTransaction(f);
});

test('symbolic-link output parents are rejected without following or deleting them', async t => {
  const f = await fixture(t);
  const external = path.join(f.root, 'preserved');
  await mkdir(external);
  await writeFile(path.join(external, 'evidence'), 'keep');
  await rm(path.dirname(f.dmg), { recursive: true });
  await symlink(external, path.dirname(f.dmg));
  await assert.rejects(withDesktopPackageOutput(f.plan, async () => assert.fail('must not build'), f.verifiers), /真实目录/);
  assert.equal(await readFile(path.join(external, 'evidence'), 'utf8'), 'keep');
  assert.ok((await lstat(path.dirname(f.dmg))).isSymbolicLink());
  await noTransaction(f);
});

test('symbolic-link package candidates are rejected and target remains untouched', async t => {
  const f = await fixture(t);
  const external = path.join(f.root, 'preserved-dmg');
  await writeFile(external, 'keep');
  await rm(f.dmg);
  await symlink(external, f.dmg);
  await assert.rejects(withDesktopPackageOutput(f.plan, async () => assert.fail('must not build'), f.verifiers));
  assert.equal(await readFile(external, 'utf8'), 'keep');
  assert.ok((await lstat(f.dmg)).isSymbolicLink());
  await noTransaction(f);
});

test('an existing lock prevents concurrent build or ambiguous crash recovery', async t => {
  const f = await fixture(t);
  const lock = path.join(f.root, 'target', '.desktop-package-build.lock');
  await mkdir(lock);
  await writeFile(path.join(lock, 'owner.json'), 'preserved evidence');
  await assert.rejects(withDesktopPackageOutput(f.plan, async () => assert.fail('must not build'), f.verifiers), /未完成的恢复现场/);
  assert.equal(await readFile(path.join(lock, 'owner.json'), 'utf8'), 'preserved evidence');
  assert.equal(await readFile(f.dmg, 'utf8'), 'old-dmg');
});

test('a recovery failure preserves old packages and lock instead of guessing a cleanup', async t => {
  const f = await fixture(t);
  const external = path.join(f.root, 'preserved');
  await mkdir(external);
  await writeFile(path.join(external, 'evidence'), 'keep');
  await assert.rejects(withDesktopPackageOutput(f.plan, async () => {
    await rm(path.dirname(f.app), { recursive: true });
    await symlink(external, path.dirname(f.app));
    throw new Error('intentional failure');
  }, f.verifiers), /真实目录/);
  const oldRoot = path.join(f.root, 'target', 'release');
  const backup = (await readdir(oldRoot)).find(name => name.startsWith('.desktop-package-previous-'));
  assert.ok(backup);
  assert.ok((await readdir(path.join(oldRoot, backup))).includes('artifact-0'));
  assert.ok((await lstat(path.join(f.root, 'target', '.desktop-package-build.lock'))).isDirectory());
  assert.equal(await readFile(path.join(external, 'evidence'), 'utf8'), 'keep');
});


test('an old app launched from staging is retained with recovery record instead of deleted', async t => {
  const f = await fixture(t);
  let samples = 0;
  await assert.rejects(withDesktopPackageOutput(f.plan, async () => f.writeApp('new'), {
    ...f.verifiers,
    processes: async () => {
      if (++samples === 1) return [];
      const profile = path.join(f.root, 'target', 'release');
      const staging = (await readdir(profile)).find(name => name.startsWith('.desktop-package-previous-'));
      return [`${path.join(profile, staging, 'artifact-0')}/Contents/MacOS/local-projects-desktop-dev`];
    },
  }), /旧生成包正在运行/);
  assert.equal(JSON.parse(await readFile(appFile(f.app))).value, 'new');
  const profile = path.join(f.root, 'target', 'release');
  const staging = (await readdir(profile)).find(name => name.startsWith('.desktop-package-previous-'));
  assert.equal(JSON.parse(await readFile(appFile(path.join(profile, staging, 'artifact-0')))).value, 'old');
  assert.ok((await lstat(path.join(f.root, 'target', '.desktop-package-build.lock'))).isDirectory());
});
