// Read-only inventory of projects that existed before the M3 upgrade.
// Database/WAL bytes are omitted because opening SQLite may checkpoint them.
import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { cli, info, readReport, runtime, saveReport, sha256 } from './m3-test-utils.mjs';

const phase = process.argv[2];
assert.ok(['prepare', 'verify'].includes(phase), 'Choose prepare or verify');
const current = runtime();
async function inventory(root, manifest, path = '') {
  const result = {};
  for (const entry of await readdir(join(root, path), { withFileTypes: true })) {
    const relative = path ? `${path}/${entry.name}` : entry.name;
    if (relative === '.platform' || relative === '.git' || relative === `${manifest?.sourceDir ?? 'app'}/node_modules`) continue;
    const database = manifest?.database ?? 'data/main.sqlite';
    if ([database, `${database}-wal`, `${database}-shm`].includes(relative)) continue;
    if (entry.isDirectory()) Object.assign(result, await inventory(root, manifest, relative));
    else if (entry.isFile()) result[relative] = sha256(await readFile(join(root, relative)));
    else result[relative] = 'non-regular-entry';
  }
  return result;
}
if (phase === 'prepare') {
  const projects = cli(['project', 'list']);
  const original = [];
  for (const project of projects) {
    assert.equal(dirname(project.projectRoot), join(current.workspaceRoot, 'projects'));
    original.push({ id: project.id, name: project.name, projectRoot: project.projectRoot, files: await inventory(project.projectRoot, project.manifest) });
  }
  await saveReport('m3-original-projects.json', { instanceId: current.instanceId, capturedAt: new Date().toISOString(), projects: original });
  console.log(`Recorded ${original.length} existing project inventories without writing to those projects.`);
} else {
  const original = await readReport('m3-original-projects.json');
  for (const project of original.projects) {
    const present = info(project.id);
    assert.equal(present.projectRoot, project.projectRoot);
    assert.deepEqual(await inventory(present.projectRoot, present.manifest), project.files, `Existing project changed: ${project.id}`);
  }
  const report = { ok: true, beforeInstanceId: original.instanceId, afterInstanceId: current.instanceId, projects: original.projects.map(project => ({ id: project.id, name: project.name, fileCount: Object.keys(project.files).length })), checks: ['all original projects remain registered', 'persistent source, SDK, docs, skill, metadata, current webpage and attachments unchanged'], exclusions: ['platform control records', 'project VCS internals', 'known dependency root', 'SQLite and WAL/SHM file bytes'] };
  await saveReport('m3-original-projects-verification.json', report);
  console.log(`Verified ${original.projects.length} existing projects unchanged.`);
}
