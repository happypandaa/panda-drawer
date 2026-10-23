import { constants } from 'node:fs';
import { chmod, lstat, mkdir, mkdtemp, open, readFile, readdir, readlink, realpath, rename, rm, rmdir, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { execFile, spawn } from 'node:child_process';
import { get } from 'node:https';
import path from 'node:path';
import os from 'node:os';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';

const execute = promisify(execFile);
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
const MAX_FILE_BYTES = 16 * 1024 * 1024;
const MAX_TREE_BYTES = 128 * 1024 * 1024;
const METADATA_FILE = 'development-git.json';
const releaseBytes = await readFile(new URL('./development-git-release.json', import.meta.url));
function immutable(value) {
  if (value && typeof value === 'object') { for (const child of Object.values(value)) immutable(child); Object.freeze(value); }
  return value;
}
export const DEVELOPMENT_GIT_RELEASE = immutable(JSON.parse(releaseBytes));

function validateRelease(release) {
  if (release.schemaVersion !== 1 || release.gitVersion !== '2.56.0' || release.platform !== 'darwin' || release.arch !== 'arm64'
      || release.source?.archiveName !== 'git-2.56.0.tar.xz' || release.source.archiveRoot !== 'git-2.56.0'
      || release.source.url !== 'https://www.kernel.org/pub/software/scm/git/git-2.56.0.tar.xz'
      || release.source.size !== 8489656 || release.source.sha256 !== '26c56c296b38c0695b26fa95f475f1d01704d2d38e73465ca30b0b2f5dc789d3'
      || release.tree?.rule !== 'sorted-utf8-path-nul-size-nul-sha256-lf-v1'
      || !Number.isSafeInteger(release.tree.fileCount) || release.tree.fileCount < 10 || release.tree.fileCount > 1000
      || !Number.isSafeInteger(release.tree.totalBytes) || release.tree.totalBytes < 1 || release.tree.totalBytes > MAX_TREE_BYTES
      || !/^[a-f0-9]{64}$/.test(release.tree.sha256) || !Array.isArray(release.executables)
      || !release.executables.includes('bin/git') || new Set(release.executables).size !== release.executables.length) {
    throw new Error('Invalid fixed development Git release metadata.');
  }
  for (const relative of release.executables) validateRelative(relative);
}

function validateRelative(relative) {
  if (!relative || /[\\\u0000-\u001f\u007f]/.test(relative)
      || relative.split('/').some(part => !part || part === '.' || part === '..')) {
    throw new Error('Unsafe development toolchain relative path.');
  }
  return relative;
}

function absoluteInput(value, label) {
  if (typeof value !== 'string' || !path.isAbsolute(value) || /[\\\u0000-\u001f\u007f]/.test(value)
      || value.split('/').some(part => part === '.' || part === '..')) {
    throw new Error(`${label} must be an explicit absolute path without traversal or control characters.`);
  }
  return path.normalize(value);
}

const sameFile = (a, b) => a.dev === b.dev && a.ino === b.ino;
const contains = (parent, child) => child === parent || child.startsWith(`${parent}${path.sep}`);

async function directoryGuard(directory) {
  const chain = [];
  let cursor = path.parse(directory).root;
  for (const part of [null, ...directory.slice(cursor.length).split(path.sep).filter(Boolean)]) {
    if (part !== null) cursor = path.join(cursor, part);
    const info = await lstat(cursor);
    if (!info.isDirectory() || info.isSymbolicLink()) throw new Error(`Development Git path is not an ordinary directory: ${cursor}`);
    chain.push({ path: cursor, info });
  }
  return chain;
}

async function checkGuard(guard) {
  for (const saved of guard) {
    const current = await lstat(saved.path);
    if (!current.isDirectory() || current.isSymbolicLink() || !sameFile(current, saved.info)) {
      throw new Error('Development Git directory changed identity.');
    }
  }
}

async function requireAbsent(target) {
  try { await lstat(target); } catch (error) { if (error.code === 'ENOENT') return; throw error; }
  throw new Error('Development Git output already exists; choose a new directory.');
}

async function readOrdinary(file, maxBytes, expected) {
  const before = expected ?? await lstat(file);
  if (!before.isFile() || before.isSymbolicLink() || before.nlink !== 1 || before.size > maxBytes) {
    throw new Error(`Development Git requires a bounded ordinary file without links: ${file}`);
  }
  const handle = await open(file, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  try {
    const current = await handle.stat();
    if (!current.isFile() || current.nlink !== 1 || !sameFile(before, current) || current.size !== before.size) {
      throw new Error('Development Git file changed while opening.');
    }
    const bytes = Buffer.alloc(before.size);
    for (let position = 0; position < bytes.length;) {
      const { bytesRead } = await handle.read(bytes, position, bytes.length - position, position);
      if (!bytesRead) throw new Error('Development Git file was truncated.');
      position += bytesRead;
    }
    const after = await handle.stat();
    if (after.mode !== before.mode || after.size !== before.size || after.mtimeMs !== before.mtimeMs || after.ctimeMs !== before.ctimeMs || after.nlink !== 1) {
      throw new Error('Development Git file changed while reading.');
    }
    return bytes;
  } finally { await handle.close(); }
}

function treeSummary(entries) {
  const sorted = [...entries].sort(([a], [b]) => Buffer.compare(Buffer.from(a), Buffer.from(b)));
  const hash = createHash('sha256'); let totalBytes = 0; const seen = new Set();
  for (const [relative, entry] of sorted) {
    validateRelative(relative);
    if (seen.has(relative)) throw new Error('Duplicate development Git tree entry.');
    seen.add(relative);
    hash.update(`${relative}\0${entry.size}\0${entry.sha256}\n`);
    totalBytes += entry.size;
  }
  return { fileCount: sorted.length, totalBytes, sha256: hash.digest('hex') };
}

export function gitTreeDigest(entries) {
  return treeSummary([...entries].map(([relative, bytes]) => [relative, { size: bytes.length, sha256: sha256(bytes) }]));
}

// Read-only: verification never runs an executable from the installation.
export async function verifyDevelopmentGit(root) {
  validateRelease(DEVELOPMENT_GIT_RELEASE);
  root = absoluteInput(root, 'Development Git root');
  const guard = await directoryGuard(root);
  const entries = []; const executables = new Set(DEVELOPMENT_GIT_RELEASE.executables);
  let metadata; let fileCount = 0; let entryCount = 0; let totalBytes = 0;
  async function visit(directory, relative = '', depth = 0) {
    if (depth > 64) throw new Error('Development Git directory is too deep.');
    const before = await lstat(directory);
    if (!before.isDirectory() || before.isSymbolicLink() || ![0o555, 0o755].includes(before.mode & 0o7777)) {
      throw new Error('Development Git requires ordinary directories with 0555/0755 permissions.');
    }
    const names = await readdir(directory);
    if (!names.length) throw new Error('Development Git contains an unexpected empty directory.');
    for (const name of names) {
      if (++entryCount > 2000) throw new Error('Development Git contains too many entries.');
      const child = relative ? `${relative}/${name}` : name;
      validateRelative(child);
      const file = path.join(directory, name); const info = await lstat(file);
      if (info.isSymbolicLink()) throw new Error('Development Git contains a symbolic link.');
      if (info.isDirectory()) { await visit(file, child, depth + 1); continue; }
      if (++fileCount > 1000 || (totalBytes += info.size) > MAX_TREE_BYTES) throw new Error('Development Git exceeds its tree budget.');
      const allowed = executables.has(child) ? [0o555, 0o755] : [0o444, 0o644];
      if (!allowed.includes(info.mode & 0o7777)) throw new Error(`Development Git file permissions differ: ${child}`);
      const bytes = await readOrdinary(file, child === METADATA_FILE ? 64 * 1024 : MAX_FILE_BYTES, info);
      if (child === METADATA_FILE) metadata = bytes;
      else entries.push([child, { size: bytes.length, sha256: sha256(bytes) }]);
    }
    const after = await lstat(directory);
    if (!sameFile(before, after) || before.mode !== after.mode || before.mtimeMs !== after.mtimeMs || before.ctimeMs !== after.ctimeMs) {
      throw new Error('Development Git directory changed during verification.');
    }
  }
  await visit(root);
  await checkGuard(guard);
  if (!metadata?.equals(releaseBytes)) throw new Error('Development Git metadata differs from the fixed release.');
  const actual = treeSummary(entries); const expected = DEVELOPMENT_GIT_RELEASE.tree;
  if (actual.fileCount !== expected.fileCount || actual.totalBytes !== expected.totalBytes || actual.sha256 !== expected.sha256) {
    throw new Error('Development Git tree differs from the fixed release.');
  }
  return { root, gitPath: path.join(root, 'bin/git'), execPath: path.join(root, 'libexec/git-core'),
    templatesPath: path.join(root, 'share/git-core/templates'), gitVersion: DEVELOPMENT_GIT_RELEASE.gitVersion, tree: actual };
}

// This exact recipe ships with the complete, original upstream source archive.
// It is a developer build tool. Installed Git itself has no Xcode dependency.
export function developmentGitBuildRecipe() {
  return Buffer.from(`#!/bin/sh
set -eu
if [ "$#" -ne 2 ]; then echo "Usage: build-git.sh <extracted-source> <new-build-output>" >&2; exit 2; fi
source_dir=$(CDPATH= cd -- "$1" && pwd -P)
output=$2
case "$output" in /*) ;; *) echo "Output must be absolute" >&2; exit 2;; esac
if [ -e "$output" ] || [ -L "$output" ]; then echo "Build output already exists" >&2; exit 2; fi
/bin/mkdir -m 700 "$output"
/bin/mkdir -m 700 "$output/home" "$output/tmp"
build() {
  /usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin HOME="$output/home" TMPDIR="$output/tmp" LC_ALL=C \\
    GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_SYSTEM=/dev/null GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0 \\
    GIT_CEILING_DIRECTORIES="$output" MACOSX_DEPLOYMENT_TARGET=15.0 \\
    /usr/bin/make -C "$source_dir" -j4 prefix=/private-development-git RUNTIME_PREFIX=YesPlease \\
    NO_GETTEXT=YesPlease NO_TCLTK=YesPlease NO_PERL=YesPlease NO_PYTHON=YesPlease NO_GITWEB=YesPlease \\
    NO_HOMEBREW=YesPlease NO_DARWIN_PORTS=YesPlease NO_FINK=YesPlease USE_HOMEBREW_LIBICONV= \\
    NO_INSTALL_HARDLINKS=YesPlease SKIP_DASHED_BUILT_INS=YesPlease CC=/usr/bin/clang \\
    "CFLAGS=-O2 -mmacosx-version-min=15.0 -ffile-prefix-map=$source_dir=." \\
    LDFLAGS=-mmacosx-version-min=15.0 NO_RUST=YesPlease "$@"
}
build all
build "DESTDIR=$output/install" install
`);
}

export function developmentGitSourceReadme() {
  return Buffer.from(`Git 2.56.0 for macOS arm64, minimum deployment target 15.0.

This directory contains the complete, unmodified upstream source archive,
its detached release signature, the publisher's public key and the exact
executable build recipe. COPYING contains GPLv2. The licenses directory
preserves SHA1DC and reftable notices. Other embedded notices remain in
the complete source archive, including xdiff and compat/regex.

Extract git-2.56.0.tar.xz in a new disposable directory, then run:
  ./build-git.sh /absolute/path/git-2.56.0 /absolute/path/new-build-output
Use a source/build location without spaces for upstream make. The installed
payload is relocatable and supports spaces and Unicode in its final path.
Build requirements: Apple clang 21.0.0 (clang-2100.3.34.2), SDK 27.0,
Apple linker 27037.1, GNU make 3.81; target macOS 15.0, arm64.
The compiler/SDK are developer prerequisites, never application prerequisites.
The source tree is not patched. Build/installation commands are in build-git.sh.

To form the shipped tree, copy install/private-development-git with regular
files. Materialize exactly these upstream aliases as independent file copies:
  bin/git-receive-pack -> git
  bin/git-upload-archive -> git
  bin/git-upload-pack -> git
  libexec/git-core/git-remote-ftp -> git-remote-http
  libexec/git-core/git-remote-ftps -> git-remote-http
  libexec/git-core/git-remote-https -> git-remote-http
Add COPYING, licenses/sha1dc.txt, licenses/reftable.txt and this sources directory.
The source-controlled release manifest fixes all shipped bytes and modes;
development-git.json is an exact copy of that manifest and is verified separately.
Native binaries retain their linker-produced ad-hoc signatures.

Perl, Python, Tcl/Tk and gettext support are disabled. Upstream's unavailable
command stubs and merge-tool adapters do not provide their external programs.
No credential helper or SSH keys are supplied. HTTPS uses macOS libcurl and
system trust; SSH authentication integration is a separate application feature.

Release source: https://www.kernel.org/pub/software/scm/git/git-2.56.0.tar.xz
Publisher primary fingerprint: 96E07AF25771955980DAD10020D04E5A713660A7
Identity anchor: https://www.kernel.org/doc/wot/junio.html
Public key: official Git refs/tags/junio-gpg-pub
The detached signature covers the decompressed tar. It was verified before
pinning this release with openpgp 6.3.2; preparation needs no OpenPGP dependency.
`);
}

function isolatedEnvironment(home, temporary, bin) {
  return { PATH: `${bin ? `${bin}:` : ''}/usr/bin:/bin:/usr/sbin:/sbin`, HOME: home, XDG_CONFIG_HOME: path.join(home, '.config'),
    TMPDIR: temporary, LC_ALL: 'C', GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_SYSTEM: '/dev/null', GIT_CONFIG_GLOBAL: '/dev/null',
    GIT_TERMINAL_PROMPT: '0', GIT_ASKPASS: '/usr/bin/false', SSH_ASKPASS: '/usr/bin/false', GIT_SSH_COMMAND: '/usr/bin/false',
    GIT_CEILING_DIRECTORIES: path.dirname(home) };
}

function downloadSource(url, redirects = 0) {
  if (url !== DEVELOPMENT_GIT_RELEASE.source.url || redirects > 2) throw new Error('Unexpected development Git download source.');
  return new Promise((resolve, reject) => {
    let deadline;
    const request = get(url, response => {
      if (response.statusCode >= 300 && response.statusCode < 400) {
        response.resume();
        try { resolve(downloadSource(new URL(response.headers.location, url).href, redirects + 1)); } catch (error) { reject(error); }
        return;
      }
      if (response.statusCode !== 200) { response.resume(); reject(new Error(`Development Git download returned HTTP ${response.statusCode}.`)); return; }
      const chunks = []; let length = 0;
      response.on('data', chunk => {
        length += chunk.length;
        if (length > DEVELOPMENT_GIT_RELEASE.source.size) response.destroy(new Error('Development Git download exceeds its fixed size.'));
        else chunks.push(chunk);
      });
      response.on('end', () => resolve(Buffer.concat(chunks)));
      response.on('error', reject);
    });
    deadline = setTimeout(() => request.destroy(new Error('Development Git download exceeded its total time limit.')), 60_000);
    request.setTimeout(60_000, () => request.destroy(new Error('Development Git download timed out.')));
    request.on('close', () => clearTimeout(deadline));
    request.on('error', reject);
  });
}

async function sourceArchive(cache) {
  const parentGuard = await directoryGuard(path.dirname(cache));
  try { await mkdir(cache, { mode: 0o700 }); } catch (error) { if (error.code !== 'EEXIST') throw error; }
  await checkGuard(parentGuard);
  const guard = await directoryGuard(cache);
  const name = DEVELOPMENT_GIT_RELEASE.source.archiveName;
  if ((await readdir(cache)).some(entry => entry !== name)) throw new Error('Development Git cache contains unexpected entries.');
  const file = path.join(cache, name);
  let bytes;
  try { bytes = await readOrdinary(file, MAX_FILE_BYTES); }
  catch (error) {
    if (error.code !== 'ENOENT') throw error;
    bytes = await downloadSource(DEVELOPMENT_GIT_RELEASE.source.url);
    if (bytes.length !== DEVELOPMENT_GIT_RELEASE.source.size || sha256(bytes) !== DEVELOPMENT_GIT_RELEASE.source.sha256) {
      throw new Error('Development Git source differs from the pinned official checksum.');
    }
    await checkGuard(guard);
    try { await writeFile(file, bytes, { flag: 'wx', mode: 0o600 }); }
    catch (error) { if (error.code !== 'EEXIST') throw error; bytes = await readOrdinary(file, MAX_FILE_BYTES); }
  }
  await checkGuard(guard);
  if ((await readdir(cache)).some(entry => entry !== name)) throw new Error('Development Git cache contains unexpected entries.');
  if (bytes.length !== DEVELOPMENT_GIT_RELEASE.source.size || sha256(bytes) !== DEVELOPMENT_GIT_RELEASE.source.sha256) {
    throw new Error('Development Git source differs from the pinned official checksum.');
  }
  return bytes;
}

// Build tools run in their own process group; a failed/expired build cannot leave
// compilers writing into a staging directory that is being cleaned up.
async function runBuild(recipe, source, output, env) {
  await new Promise((resolve, reject) => {
    const child = spawn('/bin/sh', [recipe, source, output], { env, cwd: path.dirname(source), detached: true, stdio: ['ignore', 'pipe', 'pipe'] });
    let log = ''; let failed; let killTimer;
    const stop = error => {
      if (failed) return;
      failed = error;
      try { process.kill(-child.pid, 'SIGTERM'); } catch (failure) { if (failure.code !== 'ESRCH') failed = failure; }
      killTimer = setTimeout(() => { try { process.kill(-child.pid, 'SIGKILL'); } catch {} }, 1000);
    };
    const timer = setTimeout(() => stop(new Error('Development Git build timed out.')), 180_000);
    const collect = bytes => {
      if (log.length < 1024 * 1024) log += bytes.toString();
      else stop(new Error('Development Git build output exceeded its limit.'));
    };
    child.stdout.on('data', collect); child.stderr.on('data', collect);
    child.on('error', error => { clearTimeout(timer); clearTimeout(killTimer); reject(error); });
    child.on('close', code => {
      clearTimeout(timer); clearTimeout(killTimer);
      if (failed || code !== 0) reject(new Error(`${failed?.message ?? `Development Git build failed (${code}).`}\n${log.slice(-6000)}`));
      else resolve();
    });
  });
}

const aliases = new Map([
  ['bin/git-receive-pack', 'git'], ['bin/git-upload-archive', 'git'], ['bin/git-upload-pack', 'git'],
  ['libexec/git-core/git-remote-ftp', 'git-remote-http'], ['libexec/git-core/git-remote-ftps', 'git-remote-http'], ['libexec/git-core/git-remote-https', 'git-remote-http'],
]);

async function writePayloadFile(file, bytes, mode = 0o644) {
  await mkdir(path.dirname(file), { recursive: true, mode: 0o755 });
  await writeFile(file, bytes, { flag: 'wx', mode });
  await chmod(file, mode);
}

async function copyInstallation(source, destination, relative = '') {
  await mkdir(destination, { mode: 0o755 });
  const directory = await lstat(source);
  if (!directory.isDirectory() || directory.isSymbolicLink()) throw new Error('Git build output directory is not ordinary.');
  for (const name of await readdir(source)) {
    const child = relative ? `${relative}/${name}` : name; validateRelative(child);
    let file = path.join(source, name); let info = await lstat(file);
    if (info.isSymbolicLink()) {
      const target = await readlink(file);
      if (aliases.get(child) !== target) throw new Error('Git build output contains an unknown link.');
      file = path.join(source, target); info = await lstat(file);
    }
    if (info.isDirectory()) await copyInstallation(file, path.join(destination, name), child);
    else await writePayloadFile(path.join(destination, name), await readOrdinary(file, MAX_FILE_BYTES, info), (info.mode & 0o111) ? 0o755 : 0o644);
  }
}

async function inspectNativePrograms(root) {
  for (const relative of DEVELOPMENT_GIT_RELEASE.executables) {
    const file = path.join(root, relative); const bytes = await readOrdinary(file, MAX_FILE_BYTES);
    if (!bytes.subarray(0, 4).equals(Buffer.from([0xcf, 0xfa, 0xed, 0xfe]))) continue;
    const options = { timeout: 10_000, maxBuffer: 256 * 1024, env: { PATH: '/usr/bin:/bin', LC_ALL: 'C' } };
    if ((await execute('/usr/bin/lipo', ['-archs', file], options)).stdout.trim() !== 'arm64') throw new Error('Development Git binary architecture differs.');
    const linked = (await execute('/usr/bin/otool', ['-L', file], options)).stdout.trim().split('\n').slice(1);
    if (!linked.length || linked.some(line => !/^\s+(\/usr\/lib\/|\/System\/Library\/)/.test(line))) {
      throw new Error('Development Git binary depends on a non-system library.');
    }
    await execute('/usr/bin/codesign', ['--verify', '--strict', file], options);
  }
}

async function probePrepared(root, home, temporary) {
  const options = { cwd: home, env: isolatedEnvironment(home, temporary, path.join(root, 'bin')), timeout: 10_000, maxBuffer: 16 * 1024 };
  const git = path.join(root, 'bin/git');
  if ((await execute(git, ['--version'], options)).stdout.trim() !== `git version ${DEVELOPMENT_GIT_RELEASE.gitVersion}`) throw new Error('Development Git version differs.');
  if ((await execute(git, ['--exec-path'], options)).stdout.trim() !== path.join(root, 'libexec/git-core')) throw new Error('Development Git helpers are not relocatable.');
  const repository = path.join(home, 'disposable-template-probe');
  await execute(git, ['-c', 'credential.helper=', 'init', '--quiet', '--initial-branch=main', repository], options);
  const copied = await readOrdinary(path.join(repository, '.git/hooks/pre-commit.sample'), MAX_FILE_BYTES);
  if (!copied.equals(await readOrdinary(path.join(root, 'share/git-core/templates/hooks/pre-commit.sample'), MAX_FILE_BYTES))) {
    throw new Error('Development Git templates are not relocatable.');
  }
}

export async function prepareDevelopmentGit({ outputDir, cacheDir } = {}) {
  validateRelease(DEVELOPMENT_GIT_RELEASE);
  if (process.platform !== 'darwin' || process.arch !== 'arm64') throw new Error('Development Git preparation supports macOS arm64 only.');
  const output = absoluteInput(outputDir, 'Output');
  const cache = absoluteInput(cacheDir ?? fileURLToPath(new URL('../.tools/development-git-cache', import.meta.url)), 'Cache');
  if (contains(cache, output) || contains(output, cache)) throw new Error('Development Git output and cache must not overlap.');
  const parentGuard = await directoryGuard(path.dirname(output));
  await requireAbsent(output);
  const archive = await sourceArchive(cache);
  const stage = await mkdtemp(path.join(path.dirname(output), '.development-git-stage-'));
  const owned = await lstat(stage); let reservation; let buildStage; let buildOwned; let buildParentGuard;
  try {
    // Upstream make has path-space limitations. Build independently of the
    // installation destination, which may contain Unicode and spaces.
    const temporary = await realpath(os.tmpdir());
    if (/[\s#$:]/.test(temporary)) throw new Error('Git build needs a developer temporary directory without make-special characters.');
    buildParentGuard = await directoryGuard(temporary);
    buildStage = await mkdtemp(path.join(temporary, 'disposable-development-git-build-'));
    buildOwned = await lstat(buildStage);
    await mkdir(path.join(buildStage, 'home'), { mode: 0o700 });
    const env = isolatedEnvironment(path.join(buildStage, 'home'), buildStage);
    const buildArchive = path.join(buildStage, DEVELOPMENT_GIT_RELEASE.source.archiveName);
    await writeFile(buildArchive, archive, { flag: 'wx', mode: 0o600 });
    // The fixed verified upstream archive contains three known internal source
    // aliases. They stay inside this owned build tree; no links enter the payload.
    await execute('/usr/bin/tar', ['-xJf', buildArchive, '-C', buildStage], { env, timeout: 30_000, maxBuffer: 64 * 1024 });
    const recipe = path.join(buildStage, 'build-git.sh');
    await writeFile(recipe, developmentGitBuildRecipe(), { flag: 'wx', mode: 0o700 });
    const source = path.join(buildStage, DEVELOPMENT_GIT_RELEASE.source.archiveRoot);
    const buildOutput = path.join(buildStage, 'build-output');
    await runBuild(recipe, source, buildOutput, env);
    const payload = path.join(stage, 'payload');
    await copyInstallation(path.join(buildOutput, 'install/private-development-git'), payload);
    for (const [from, to] of [['COPYING', 'COPYING'], ['sha1dc/LICENSE.txt', 'licenses/sha1dc.txt'], ['reftable/LICENSE', 'licenses/reftable.txt']]) {
      await writePayloadFile(path.join(payload, to), await readOrdinary(path.join(source, from), MAX_FILE_BYTES));
    }
    await writePayloadFile(path.join(payload, 'sources', DEVELOPMENT_GIT_RELEASE.source.archiveName), archive);
    await writePayloadFile(path.join(payload, 'sources/build-git.sh'), developmentGitBuildRecipe(), 0o755);
    await writePayloadFile(path.join(payload, 'sources/README.txt'), developmentGitSourceReadme());
    const signature = Buffer.from(DEVELOPMENT_GIT_RELEASE.signature.detachedBase64, 'base64');
    const publicKey = Buffer.from(DEVELOPMENT_GIT_RELEASE.signature.publicKeyArmored);
    if (sha256(signature) !== DEVELOPMENT_GIT_RELEASE.signature.detachedSha256 || sha256(publicKey) !== DEVELOPMENT_GIT_RELEASE.signature.publicKeySha256) throw new Error('Fixed Git signature materials differ.');
    await writePayloadFile(path.join(payload, 'sources/git-2.56.0.tar.sign'), signature);
    await writePayloadFile(path.join(payload, 'sources/junio-gpg-pub.asc'), publicKey);
    await writePayloadFile(path.join(payload, METADATA_FILE), releaseBytes);
    await verifyDevelopmentGit(payload);
    await inspectNativePrograms(payload);
    await probePrepared(payload, path.join(buildStage, 'home'), buildStage);
    await checkGuard(parentGuard);
    await mkdir(output, { mode: 0o700 });
    reservation = await lstat(output);
    await checkGuard(parentGuard);
    if (!sameFile(await lstat(output), reservation)) throw new Error('Development Git output reservation changed identity.');
    await rename(payload, output); reservation = undefined;
    return { outputDir: output, gitVersion: DEVELOPMENT_GIT_RELEASE.gitVersion, tree: DEVELOPMENT_GIT_RELEASE.tree };
  } finally {
    if (reservation) {
      await checkGuard(parentGuard);
      const current = await lstat(output).catch(error => { if (error.code === 'ENOENT') return undefined; throw error; });
      if (current?.isDirectory() && !current.isSymbolicLink() && sameFile(current, reservation)) {
        await rmdir(output).catch(error => { if (!['ENOTEMPTY', 'EEXIST'].includes(error.code)) throw error; });
      }
    }
    for (const [directory, original, guard] of [[buildStage, buildOwned, buildParentGuard], [stage, owned, parentGuard]]) {
      if (!directory) continue;
      await checkGuard(guard);
      const current = await lstat(directory);
      if (!current.isDirectory() || current.isSymbolicLink() || !sameFile(current, original)) throw new Error('Owned Git staging changed identity; retained for inspection.');
      await rm(directory, { recursive: true });
    }
  }
}

export function parseDevelopmentGitArguments(args) {
  const options = {};
  for (let index = 0; index < args.length; index++) {
    const key = args[index] === '--output' ? 'outputDir' : args[index] === '--cache' ? 'cacheDir' : undefined;
    if (!key || options[key] !== undefined || !args[index + 1] || args[index + 1].startsWith('--')) throw new Error('Usage: node scripts/package-development-git.mjs --output <new-absolute-directory> [--cache <absolute-directory>]');
    options[key] = args[++index];
  }
  if (!options.outputDir) throw new Error('An explicit new --output directory is required.');
  return options;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { console.log(JSON.stringify(await prepareDevelopmentGit(parseDevelopmentGitArguments(process.argv.slice(2))))); }
  catch (error) { console.error(`Development Git preparation failed: ${error.message}`); process.exitCode = 1; }
}
