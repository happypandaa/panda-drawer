import { spawnSync } from 'node:child_process';
import { cp, mkdtemp, mkdir, readFile, readdir, rm, stat, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const ignored = new Set(['node_modules', 'dist', '.git', '.hg', '.svn', '.vite', '.cache']);

// Match platform-core's tree digest: sorted depth-first paths, byte lengths and content.
async function treeDigest(directory) {
  const digest = createHash('sha256');
  const length = n => { const value = Buffer.alloc(8); value.writeBigUInt64LE(BigInt(n)); return value; };
  async function visit(relative = '') {
    const entries = await readdir(path.join(directory, relative), { withFileTypes: true });
    entries.sort((a, b) => Buffer.compare(Buffer.from(a.name), Buffer.from(b.name)));
    for (const entry of entries) {
      const label = relative ? `${relative}/${entry.name}` : entry.name;
      if (entry.isDirectory()) await visit(label);
      else if (entry.isFile()) {
        const name = Buffer.from(label);
        const data = await readFile(path.join(directory, label));
        digest.update(length(name.length)).update(name).update(length(data.length)).update(data);
      } else throw new Error(`Template may contain only ordinary files/directories: ${label}`);
    }
  }
  await visit();
  return digest.digest('hex');
}

export async function packageTemplate(output) {
  const temporary = await mkdtemp(path.join(tmpdir(), 'local-projects-template-'));
  const source = path.join(temporary, 'app');
  const built = path.join(temporary, 'starter');
  try {
    // When assembling a desktop bundle, compile the exact source snapshot already
    // copied to its resources, even if the repository changes while packaging.
    const hasResourceSource = await stat(path.join(output, 'app')).then(info => info.isDirectory()).catch(error => {
      if (error.code === 'ENOENT') return false;
      throw error;
    });
    const templateRoot = hasResourceSource ? output : path.join(root, 'templates/react');
    const templateSource = path.join(templateRoot, 'app');
    const sourceFilter = file => !path.relative(templateSource, file).split(path.sep).some(part => ignored.has(part));
    await cp(templateSource, source, { recursive: true, filter: sourceFilter });
    const sourceDigest = await treeDigest(source);
    const npm = (args) => {
      const result = spawnSync('npm', args, { cwd: source, encoding: 'utf8', maxBuffer: 16 * 1024 * 1024 });
      if (result.status !== 0) throw new Error(result.stderr || result.stdout || 'Template npm command failed');
      return result.stdout.trim();
    };
    const npmVersion = npm(['--version']);
    npm(['ci', '--no-audit', '--no-fund']);
    npm(['run', 'build', '--', '--outDir', built]);
    // node_modules is the only generated source directory allowed by the template build.
    await rm(path.join(source, 'node_modules'), { recursive: true, force: true });
    if (await treeDigest(source) !== sourceDigest) throw new Error('Template build changed its source; refusing to package mismatched starter');
    const template = JSON.parse(await readFile(path.join(templateRoot, 'template.json'), 'utf8'));
    const record = {
      schemaVersion: 1,
      origin: 'bundledTemplate',
      templateVersion: template.templateVersion,
      sdkVersion: template.sdkVersion,
      sourceDigest,
      artifactDigest: await treeDigest(built),
      entryRelative: 'index.html',
      builtAt: Date.now(),
      nodeVersion: process.versions.node,
      npmVersion,
    };
    await readFile(path.join(built, record.entryRelative));
    await mkdir(output, { recursive: true });
    await rm(path.join(output, 'starter'), { recursive: true, force: true });
    await cp(built, path.join(output, 'starter'), { recursive: true });
    await writeFile(path.join(output, 'starter.json'), `${JSON.stringify(record, null, 2)}\n`);
    console.log(`Bundled starter ${record.templateVersion}: ${record.artifactDigest}`);
    return record;
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const outputIndex = process.argv.indexOf('--output');
  const output = outputIndex >= 0 && process.argv[outputIndex + 1];
  if (!output || !path.isAbsolute(output)) throw new Error('Pass --output ABSOLUTE_TEMPLATE_RESOURCE_DIRECTORY');
  await packageTemplate(output);
}
