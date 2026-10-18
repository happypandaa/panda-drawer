// Independent W1.1 compiler probe. Never imports or starts the platform.
import { execFile } from 'node:child_process';
import { lstat, mkdir, mkdtemp, open, realpath, stat, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { promisify } from 'node:util';
import { root, rustEnvironment } from './process.mjs';

const execute = promisify(execFile);
const expectedOutput = 'LOCAL_PROJECTS_WINDOWS_TOOLCHAIN_SMOKE_OK';
const steps = [];
let evidenceDirectory = null;
let toolchain = null;

class ProbeFailure extends Error {
  constructor(status, code, message) {
    super(message);
    this.status = status;
    this.code = code;
  }
}

function classify(error) {
  if (error instanceof ProbeFailure) return { status: error.status, code: error.code, message: error.message };
  return { status: ['EACCES', 'EPERM'].includes(error.code) ? 'inaccessible' : 'probeError',
    code: String(error.code ?? error.name ?? 'UNKNOWN'), message: String(error.message ?? error).slice(0, 500) };
}

function environmentValue(environment, name) {
  const key = Object.keys(environment).find((candidate) => candidate.toLowerCase() === name.toLowerCase());
  return environment[key];
}

function normalizePath(environment) {
  const result = { ...environment };
  const selected = Object.hasOwn(result, 'PATH') ? result.PATH : environmentValue(result, 'PATH');
  for (const key of Object.keys(result)) if (key.toLowerCase() === 'path') delete result[key];
  if (selected !== undefined) result.PATH = selected;
  return result;
}

async function requireFile(filename, code) {
  try {
    if (!(await stat(filename)).isFile()) throw new ProbeFailure('probeError', code, `Not a regular file: ${filename}`);
    return filename;
  } catch (error) {
    if (['ENOENT', 'ENOTDIR'].includes(error.code)) throw new ProbeFailure('missing', code, `Required tool or SDK file is absent: ${filename}`);
    throw error;
  }
}

// Environment capture is deliberately special: its stdout/stderr never enters a
// report, log, or thrown error, including when cmd/VsDevCmd fails.
async function command(id, executable, args, environment, { captureEnvironment = false, log = false } = {}) {
  let stdout = ''; let stderr = ''; let exitCode = null; let problem = null;
  try {
    const output = await execute(executable, args, { cwd: evidenceDirectory ?? root, env: normalizePath(environment),
      // cmd uses its own quoting rules. Only its fixed environment-capture
      // command uses verbatim arguments; all compiler arguments use Node quoting.
      windowsHide: true, windowsVerbatimArguments: captureEnvironment, encoding: captureEnvironment ? 'utf16le' : 'utf8',
      timeout: 120_000, maxBuffer: 2 * 1024 * 1024 });
    stdout = output.stdout; stderr = output.stderr; exitCode = 0;
  } catch (error) {
    stdout = String(error.stdout ?? ''); stderr = String(error.stderr ?? '');
    exitCode = typeof error.code === 'number' ? error.code : null;
    problem = { status: ['EACCES', 'EPERM'].includes(error.code) ? 'inaccessible' : 'probeError',
      code: String(error.code ?? (error.killed ? 'TIMEOUT' : 'EXECUTION_FAILED')), killed: Boolean(error.killed) };
  }
  const step = { id, executable, exitCode, status: problem?.status ?? 'available', ...(problem ? { error: problem } : {}) };
  if (!captureEnvironment) step.arguments = args;
  if (log && evidenceDirectory && !captureEnvironment) {
    step.log = path.join(evidenceDirectory, `${id}.log`);
    await writeFile(step.log, `stdout:\n${stdout}\nstderr:\n${stderr}`, { flag: 'wx' });
  }
  steps.push(step);
  if (problem) {
    throw new ProbeFailure(problem.status, `${id.toUpperCase()}_FAILED`, captureEnvironment
      ? 'The Visual Studio environment probe failed; its environment output was not saved or printed.'
      : `Command ${id} failed (exit ${exitCode ?? problem.code}); see the evidence log when present.`);
  }
  return { stdout, stderr };
}

async function discoverVisualStudio() {
  // ProgramFiles(x86) is used only to locate the documented vswhere executable;
  // no installation directories, packages, or disks are scanned.
  const programFiles = environmentValue(process.env, 'ProgramFiles(x86)');
  if (!programFiles) throw new ProbeFailure('probeError', 'PROGRAM_FILES_UNKNOWN', 'ProgramFiles(x86) is unavailable in this process environment.');
  const vswhere = await requireFile(path.join(programFiles, 'Microsoft Visual Studio', 'Installer', 'vswhere.exe'), 'VSWHERE_MISSING');
  const result = await command('vswhere', vswhere, ['-latest', '-products', '*', '-requires', 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64', '-format', 'json', '-utf8'], process.env);
  let instances;
  try { instances = JSON.parse(result.stdout.replace(/^\uFEFF/, '')); }
  catch { throw new ProbeFailure('probeError', 'VSWHERE_INVALID_JSON', 'vswhere did not return valid JSON.'); }
  if (!Array.isArray(instances)) throw new ProbeFailure('probeError', 'VSWHERE_INVALID_JSON', 'vswhere did not return an installation array.');
  const instance = instances.find((candidate) => candidate.isComplete === true && candidate.isLaunchable === true && typeof candidate.installationPath === 'string');
  if (!instance) throw new ProbeFailure('missing', 'COMPLETE_X64_MSVC_INSTANCE_MISSING', 'No complete, launchable Visual Studio instance with x64 VC tools was found.');
  const vsDevCmd = await requireFile(path.join(instance.installationPath, 'Common7', 'Tools', 'VsDevCmd.bat'), 'VSDEVCMD_MISSING');
  return { installationPath: instance.installationPath, installationVersion: instance.installationVersion, vsDevCmd };
}

async function developerEnvironment(visualStudio) {
  const windows = environmentValue(process.env, 'SystemRoot');
  if (!windows) throw new ProbeFailure('probeError', 'SYSTEM_ROOT_UNKNOWN', 'SystemRoot is unavailable.');
  const cmd = await requireFile(path.join(windows, 'System32', 'cmd.exe'), 'CMD_MISSING');
  const captureInput = { ...process.env };
  // VsDevCmd writes complete pre/post environment snapshots when DEBUG is set,
  // even to "0". Never inherit those diagnostics into a compiler probe.
  for (const key of Object.keys(captureInput)) if (/^VSCMD_(DEBUG|TEST)/i.test(key)) delete captureInput[key];
  captureInput.LOCAL_PROJECTS_VSDEV_CMD = visualStudio.vsDevCmd;
  captureInput.VSCMD_SKIP_SENDTELEMETRY = '1';
  const result = await command('vsdevcmd', cmd,
    ['/u', '/d', '/s', '/c', 'call "%LOCAL_PROJECTS_VSDEV_CMD%" -no_logo -arch=x64 -host_arch=x64 >nul && set'],
    captureInput, { captureEnvironment: true });
  const environment = {};
  for (const line of result.stdout.split(/\r?\n/)) {
    const separator = line.indexOf('=');
    // cmd's hidden drive-current-directory entries begin with '=' and are not
    // ordinary environment variables accepted by Node's child process API.
    if (separator > 0) environment[line.slice(0, separator)] = line.slice(separator + 1);
  }
  delete environment.LOCAL_PROJECTS_VSDEV_CMD;
  if (environmentValue(environment, 'VSCMD_ARG_TGT_ARCH') !== 'x64' || environmentValue(environment, 'VSCMD_ARG_HOST_ARCH') !== 'x64') {
    throw new ProbeFailure('probeError', 'VSDEVCMD_ARCHITECTURE_MISMATCH', 'VsDevCmd did not establish an x64 host and x64 target.');
  }
  return normalizePath(rustEnvironment({ environment }));
}

async function preflight(environment) {
  // These variables prepend/append native command options and can override
  // explicit output paths. Keep SDK search paths, but use only our own flags.
  for (const key of Object.keys(environment)) if (/^(CL|_CL_|LINK|_LINK_)$/i.test(key)) delete environment[key];
  environment.RUSTUP_AUTO_INSTALL = '0';
  environment.RUSTUP_NO_UPDATE_CHECK = '1';
  const toolsRoot = environmentValue(environment, 'VCToolsInstallDir');
  const sdkRoot = environmentValue(environment, 'WindowsSdkDir');
  const sdkVersion = environmentValue(environment, 'WindowsSDKVersion')?.replace(/[\\/]+$/, '');
  if (!toolsRoot || !sdkRoot || !sdkVersion) throw new ProbeFailure('missing', 'VC_OR_SDK_ENVIRONMENT_MISSING', 'VsDevCmd did not provide VC tools and a Windows SDK.');
  const bin = path.join(toolsRoot, 'bin', 'Hostx64', 'x64');
  const [compiler, librarian, linker] = await Promise.all(['cl.exe', 'lib.exe', 'link.exe'].map((name) => requireFile(path.join(bin, name), 'MSVC_TOOL_MISSING')));
  await Promise.all([
    path.join(sdkRoot, 'Include', sdkVersion, 'um', 'Windows.h'),
    path.join(sdkRoot, 'Include', sdkVersion, 'ucrt', 'stdio.h'),
    path.join(sdkRoot, 'Lib', sdkVersion, 'um', 'x64', 'kernel32.lib'),
    path.join(sdkRoot, 'Lib', sdkVersion, 'ucrt', 'x64', 'ucrt.lib'),
    path.join(sdkRoot, 'bin', sdkVersion, 'x64', 'rc.exe'),
  ].map((filename) => requireFile(filename, 'WINDOWS_SDK_FILE_MISSING')));
  const rustc = await findRustc(environment);
  const version = await command('rustc-version', rustc, ['-vV'], environment);
  const host = version.stdout.match(/^host: (.+)$/m)?.[1];
  if (host !== 'x86_64-pc-windows-msvc') throw new ProbeFailure('probeError', 'RUST_HOST_MISMATCH', 'The project-selected rustc is not an x86_64-pc-windows-msvc toolchain.');
  return { compiler, librarian, linker, rustc, sdkRoot, sdkVersion,
    vcToolsVersion: environmentValue(environment, 'VCToolsVersion') ?? null,
    rustVersion: version.stdout.match(/^rustc (.+)$/m)?.[1] ?? null, rustHost: host };
}

async function findRustc(environment) {
  const directories = (environmentValue(environment, 'PATH') ?? '').split(path.delimiter).filter(Boolean);
  for (const directory of directories) {
    const candidate = path.join(directory.replace(/^"|"$/g, ''), 'rustc.exe');
    try { if ((await stat(candidate)).isFile()) return candidate; }
    catch (error) { if (!['ENOENT', 'ENOTDIR'].includes(error.code)) throw error; }
  }
  throw new ProbeFailure('missing', 'RUSTC_MISSING', 'No rustc.exe was found in the project-selected environment.');
}

async function createEvidenceDirectory() {
  const parent = path.join(root, '.test-artifacts');
  try { await mkdir(parent); }
  catch (error) { if (error.code !== 'EEXIST') throw error; }
  const information = await lstat(parent);
  if (!information.isDirectory() || information.isSymbolicLink()
    || path.relative(await realpath(root), await realpath(parent)).toLowerCase() !== '.test-artifacts') {
    throw new ProbeFailure('probeError', 'EVIDENCE_DIRECTORY_UNSAFE', 'The evidence directory must be an ordinary repository .test-artifacts directory.');
  }
  return mkdtemp(path.join(parent, `windows-w1-toolchain-smoke-${new Date().toISOString().replace(/[:.]/g, '-')}-`));
}

async function inspectPe(filename) {
  const file = await open(filename, 'r');
  try {
    const dos = Buffer.alloc(64);
    if ((await file.read(dos, 0, dos.length, 0)).bytesRead !== dos.length || dos.toString('ascii', 0, 2) !== 'MZ') {
      throw new ProbeFailure('probeError', 'INVALID_PE', 'The generated executable has no DOS header.');
    }
    const offset = dos.readUInt32LE(0x3c);
    const header = Buffer.alloc(26);
    if ((await file.read(header, 0, header.length, offset)).bytesRead !== header.length || header.readUInt32LE(0) !== 0x00004550) {
      throw new ProbeFailure('probeError', 'INVALID_PE', 'The generated executable has no PE signature.');
    }
    const machine = header.readUInt16LE(4);
    const optionalHeaderMagic = header.readUInt16LE(24);
    if (machine !== 0x8664 || optionalHeaderMagic !== 0x20b) throw new ProbeFailure('probeError', 'PE_ARCHITECTURE_MISMATCH', 'The generated executable is not an AMD64 PE32+ image.');
    return { status: 'available', machine: '0x8664', optionalHeaderMagic: '0x020b', architecture: 'x64' };
  } finally { await file.close(); }
}

async function smoke() {
  const visualStudio = await discoverVisualStudio();
  const environment = await developerEnvironment(visualStudio);
  const tools = await preflight(environment);
  toolchain = { visualStudio: { installationPath: visualStudio.installationPath, version: visualStudio.installationVersion },
    versions: { vcTools: tools.vcToolsVersion, windowsSdk: tools.sdkVersion, rust: tools.rustVersion, rustHost: tools.rustHost } };
  evidenceDirectory = await createEvidenceDirectory();
  // Native compiler/linker temporary files also stay in this new evidence tree.
  for (const name of ['TEMP', 'TMP', 'TMPDIR']) {
    for (const key of Object.keys(environment)) if (key.toLowerCase() === name.toLowerCase()) delete environment[key];
    environment[name] = evidenceDirectory;
  }
  const files = Object.fromEntries(['probe.c', 'probe.rs', 'probe.obj', 'windows_toolchain_smoke.lib', 'probe.exe'].map((name) => [name, path.join(evidenceDirectory, name)]));
  await writeFile(files['probe.c'], '#include <windows.h>\nint local_projects_windows_probe(void) { return GetCurrentProcessId() != 0 ? 42 : 0; }\n', { flag: 'wx' });
  await writeFile(files['probe.rs'], `unsafe extern "C" { fn local_projects_windows_probe() -> i32; }\nfn main() {\n    assert_eq!(unsafe { local_projects_windows_probe() }, 42);\n    println!("${expectedOutput}");\n}\n`, { flag: 'wx' });
  await command('cl', tools.compiler, ['/nologo', '/c', '/W4', '/WX', '/MD', '/TC', `/Fo${files['probe.obj']}`, files['probe.c']], environment, { log: true });
  await command('lib', tools.librarian, ['/nologo', '/MACHINE:X64', `/OUT:${files['windows_toolchain_smoke.lib']}`, files['probe.obj']], environment, { log: true });
  await command('rustc', tools.rustc, ['--edition=2021', '--target', 'x86_64-pc-windows-msvc', files['probe.rs'],
    '-L', `native=${evidenceDirectory}`, '-l', 'static=windows_toolchain_smoke', '-l', 'dylib=kernel32',
    '-C', `linker=${tools.linker}`, '-o', files['probe.exe']], environment, { log: true });
  const pe = await inspectPe(files['probe.exe']);
  const result = await command('run', files['probe.exe'], [], environment, { log: true });
  if (result.stdout.trim() !== expectedOutput || result.stderr.trim()) throw new ProbeFailure('probeError', 'UNEXPECTED_PROBE_OUTPUT', 'The linked executable did not produce the exact expected success output.');
  return { status: 'available', result: 'passed', pe, output: expectedOutput, cReturnValue: 42 };
}

async function main() {
  let result;
  if (process.argv.slice(2).some((argument) => argument !== '--json')) {
    result = { status: 'probeError', code: 'INVALID_ARGUMENT', message: 'Usage: node scripts/windows-toolchain-smoke.mjs [--json]' };
  } else if (process.platform !== 'win32') {
    result = { status: 'probeError', code: 'WINDOWS_REQUIRED', message: 'This native x64 Windows compiler probe requires Windows.' };
  } else {
    try { result = await smoke(); }
    catch (error) { result = classify(error); }
  }
  const report = { schemaVersion: 1, generatedAt: new Date().toISOString(), ...result, toolchain, evidenceDirectory, steps,
    scope: 'Independent C static-library + Rust + Windows SDK x64 compile/link/run probe. Does not verify SQLite, Tauri, platform startup or Windows product acceptance.' };
  if (evidenceDirectory) {
    const reportPath = path.join(evidenceDirectory, 'report.json');
    report.reportPath = reportPath;
    try { await writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`, { flag: 'wx' }); }
    catch (error) { report.reportWriteError = classify(error); report.status = 'probeError'; }
  }
  console.log(JSON.stringify(report, null, 2));
  if (report.status !== 'available') process.exitCode = 1;
}

await main();
