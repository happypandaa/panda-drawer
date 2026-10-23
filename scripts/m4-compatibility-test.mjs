// A read-only persistent-file baseline, usable before the service is launched.
// Verify follows the selected development workspace after public migration.
import assert from 'node:assert/strict';
import { readFile, readdir, writeFile } from 'node:fs/promises';
import { basename, join } from 'node:path';
import { artifactRoot, cli, dataRoot, runtime, saveReport, settings, sha256 } from './m4-test-utils.mjs';

const phase = process.argv[2]; assert.ok(['prepare', 'verify'].includes(phase), 'Choose prepare or verify');
const selected = settings();
async function inventory(root, manifest, path = '') {
  const result = {};
  for (const entry of await readdir(join(root, path), { withFileTypes: true })) {
    const relative = path ? `${path}/${entry.name}` : entry.name;
    if (relative === '.platform' || relative === '.git' || relative === `${manifest.sourceDir}/node_modules`) continue;
    if ([manifest.database, `${manifest.database}-wal`, `${manifest.database}-shm`, `${manifest.database}-journal`].includes(relative)) continue;
    if (entry.isDirectory()) Object.assign(result, await inventory(root, manifest, relative));
    else if (entry.isFile()) result[relative] = sha256(await readFile(join(root, relative)));
    else result[relative] = 'non-regular-entry';
  }
  return result;
}
if (phase === 'prepare') {
  const projects = [];
  for (const entry of await readdir(join(selected.workspaceRoot, 'projects'), { withFileTypes: true })) {
    assert.ok(entry.isDirectory(), 'Unexpected project-root entry');
    const root = join(selected.workspaceRoot, 'projects', entry.name);
    const manifest = JSON.parse(await readFile(join(root, 'project.json'), 'utf8'));
    assert.match(manifest.id, /^[a-z][a-z0-9_-]{0,63}$/);
    if (manifest.id !== entry.name) {
      const registration = JSON.parse(await readFile(join(selected.workspaceRoot, '.platform/project-state', `${manifest.id}.json`), 'utf8'));
      assert.equal(registration.id, manifest.id);
      assert.equal(registration.projectRoot, root, 'A named directory must have an explicit platform binding');
    }
    const db = {};
    for (const suffix of ['', '-wal', '-shm', '-journal']) {
      try { const bytes = await readFile(join(root, manifest.database + suffix)); db[suffix || 'main'] = { bytes: bytes.length, sha256: sha256(bytes) }; }
      catch (error) { if (error.code !== 'ENOENT') throw error; }
    }
    projects.push({ id: manifest.id, name: manifest.name, projectRoot: root, files: await inventory(root, manifest), databaseFileEvidence: db });
  }
  projects.sort((a,b) => a.id.localeCompare(b.id));
  const report = { capturedAt: new Date().toISOString(), dataRoot, workspaceRoot: selected.workspaceRoot, mode: 'offline read-only inventory', projects };
  await writeFile(join(artifactRoot, 'm4-original-projects.json'), JSON.stringify(report, null, 2), { flag: 'wx', mode: 0o600 });
  console.log(`Recorded ${projects.length} existing development projects without writing to them.`);
} else {
  const original = JSON.parse(await readFile(join(artifactRoot, 'm4-original-projects.json'), 'utf8'));
  const current = runtime(); const active = cli(['project', 'list']);
  for (const project of original.projects) {
    const present = active.find(value => value.id === project.id); assert.ok(present, `Missing original project ${project.id}`);
    assert.equal(present.projectRoot, join(current.workspaceRoot, 'projects', basename(project.projectRoot)));
    assert.deepEqual(await inventory(present.projectRoot, present.manifest), project.files, `Persistent files changed for ${project.id}`);
  }
  await saveReport('m4-original-projects-verification.json', { ok: true, beforeWorkspace: original.workspaceRoot, afterWorkspace: current.workspaceRoot, projects: original.projects.map(p => ({ id: p.id, name: p.name, fileCount: Object.keys(p.files).length })), checks: ['all original development projects remain active', 'source, SDK, documentation, metadata, webpage and attachments unchanged across upgrade/migration'], exclusions: ['project control records', 'known dependency roots', 'VCS internals', 'SQLite/WAL/SHM bytes may change during checkpoint; initial raw hashes retained separately'] });
  console.log(`Verified ${original.projects.length} original projects unchanged at ${current.workspaceRoot}.`);
}
