// Read-only W1.1 inventory. This does not start the platform or initialize its data root.
import { execFile } from 'node:child_process';
import { stat, readdir } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { promisify } from 'node:util';
import { root, rustEnvironment } from './process.mjs';

const execute = promisify(execFile);

function failure(error) {
  const status = ['EACCES', 'EPERM'].includes(error.code) ? 'inaccessible'
    : ['ENOENT', 'ENOTDIR'].includes(error.code) ? 'missing' : 'probeError';
  return { status, code: String(error.code ?? error.name ?? 'UNKNOWN'), message: String(error.message ?? error).slice(0, 500) };
}

async function fileInfo(filename) {
  try {
    const information = await stat(filename);
    return { status: information.isFile() ? 'available' : 'probeError', path: filename, ...(information.isFile() ? {} : { reason: 'notAFile' }) };
  } catch (error) { return { ...failure(error), path: filename }; }
}

async function commandPath(name, environment = process.env) {
  const pathKey = Object.hasOwn(environment, 'PATH') ? 'PATH' : Object.keys(environment).find((key) => key.toLowerCase() === 'path');
  const directories = (environment[pathKey] ?? '').split(path.delimiter).filter(Boolean);
  const failures = [];
  for (const directory of directories) {
    for (const extension of ['.exe', '.cmd']) {
      const candidate = path.join(directory.replace(/^"|"$/g, ''), `${name}${extension}`);
      const result = await fileInfo(candidate);
      if (result.status === 'available') return result;
      if (result.status !== 'missing') failures.push(result);
    }
  }
  return failures[0] ?? { status: 'missing', reason: 'notOnPath' };
}

async function run(executable, args, environment = process.env) {
  try {
    // Windows treats PATH names case-insensitively; Node may otherwise select an
    // inherited Path instead of the project-specific PATH added by its runner.
    const normalized = { ...environment };
    const pathKey = Object.hasOwn(environment, 'PATH') ? 'PATH' : Object.keys(environment).find((key) => key.toLowerCase() === 'path');
    if (pathKey) {
      for (const key of Object.keys(normalized)) if (key.toLowerCase() === 'path') delete normalized[key];
      normalized.PATH = environment[pathKey];
    }
    const result = await execute(executable, args, {
      cwd: root, env: normalized, windowsHide: true, encoding: 'utf8', timeout: 20_000, maxBuffer: 512 * 1024,
    });
    return { status: 'available', stdout: result.stdout.trim() };
  } catch (error) {
    // A nonzero exit is a failed probe, never evidence that a component is absent.
    return { ...failure(error), ...(error.code === 'ERR_CHILD_PROCESS_STDIO_MAXBUFFER' ? { reason: 'outputLimit' }
      : error.killed ? { reason: 'timeout' } : {}),
      ...(typeof error.code === 'number' ? { exitCode: error.code, status: 'probeError' } : {}),
      ...(error.stderr ? { stderr: String(error.stderr).trim().slice(0, 1000) } : {}) };
  }
}

async function versionProbe(location, args, environment) {
  if (location.status !== 'available') return location;
  const result = await run(location.path, args, environment);
  return result.status === 'available' ? { status: 'available', path: location.path, version: result.stdout }
    : { ...result, path: location.path };
}

// Static PowerShell, with only the repository path supplied via a dedicated environment
// variable. Registry/.NET probes avoid CIM/WMI and never create directories or files.
const inventoryScript = String.raw`
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$view = if ([Environment]::Is64BitOperatingSystem) { [Microsoft.Win32.RegistryView]::Registry64 } else { [Microsoft.Win32.RegistryView]::Registry32 }
function Failed($record) {
  $exception = $record.Exception
  while ($exception.InnerException) { $exception = $exception.InnerException }
  $state = 'probeError'
  if ($exception -is [UnauthorizedAccessException] -or $exception -is [System.Security.SecurityException]) { $state = 'inaccessible' }
  elseif ($exception -is [System.IO.FileNotFoundException] -or $exception -is [System.IO.DirectoryNotFoundException]) { $state = 'missing' }
  return @{ status = $state; exception = $exception.GetType().FullName; message = $exception.Message }
}
function Probe($body) { try { & $body } catch { Failed $_ } }
function Registry([Microsoft.Win32.RegistryHive]$hive, [string]$key, [string]$name, $registryView = $view) {
  $base = $null; $opened = $null
  try {
    $base = [Microsoft.Win32.RegistryKey]::OpenBaseKey($hive, $registryView)
    $opened = $base.OpenSubKey($key, $false)
    if (!$opened) { return @{ status = 'missing'; reason = 'registryKeyAbsent' } }
    $value = $opened.GetValue($name, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    if ($null -eq $value) { return @{ status = 'missing'; reason = 'registryValueAbsent' } }
    return @{ status = 'available'; value = $value }
  } catch { Failed $_ }
  finally { if ($opened) { $opened.Dispose() }; if ($base) { $base.Dispose() } }
}
function Volume([string]$location) {
  Probe {
    $volumeRoot = [System.IO.Path]::GetPathRoot($location)
    if (!$volumeRoot) { throw 'The path has no absolute volume root.' }
    $drive = New-Object System.IO.DriveInfo($volumeRoot)
    if (!$drive.IsReady) { return @{ status = 'probeError'; path = $location; reason = 'volumeNotReady' } }
    return @{ status = 'available'; path = $location; volumeRoot = $volumeRoot; fileSystem = $drive.DriveFormat; driveType = $drive.DriveType.ToString(); isNTFS = ($drive.DriveFormat -eq 'NTFS'); isFixed = ($drive.DriveType -eq [System.IO.DriveType]::Fixed) }
  }
}
$result = [ordered]@{}
$result.system = Probe {
  $key = 'SOFTWARE\Microsoft\Windows NT\CurrentVersion'
  $values = [ordered]@{}
  foreach ($name in @('ProductName', 'DisplayVersion', 'CurrentBuildNumber', 'UBR', 'EditionID', 'InstallationType')) {
    $values[$name] = Registry LocalMachine $key $name
  }
  @{ status = 'available'; osVersion = [Environment]::OSVersion.Version.ToString(); osArchitecture = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString(); is64BitOperatingSystem = [Environment]::Is64BitOperatingSystem; registry = $values }
}
$result.privileges = Probe {
  $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
  try {
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    @{ status = 'available'; isAdministrator = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator) }
  } finally { $identity.Dispose() }
}
$result.repositoryVolume = Volume $env:LOCAL_PROJECTS_DIAGNOSTIC_REPO
$result.localAppData = Probe {
  # KnownFolder and LOCALAPPDATA may both be redirected by a sandbox. Resolve the
  # user's registered location instead; never turn a sandbox path into a data root.
  $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
  try { $sid = $identity.User.Value } finally { $identity.Dispose() }
  $profile = Registry LocalMachine ('SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\' + $sid) 'ProfileImagePath'
  $local = Registry CurrentUser 'Software\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders' 'Local AppData'
  if ($local.status -ne 'available') { return $local }
  $location = [string]$local.value
  $systemDrive = [System.IO.Path]::GetPathRoot([Environment]::GetFolderPath([Environment+SpecialFolder]::Windows)).TrimEnd('\')
  if ($location -match '(?i)%USERPROFILE%') {
    if ($profile.status -ne 'available') { return @{ status = $profile.status; reason = 'profileRegistryUnavailable'; profileProbe = $profile } }
    $profilePath = ([string]$profile.value) -replace '(?i)%SystemDrive%', $systemDrive
    if ($profilePath.Contains('%')) { throw 'The registered profile contains unsupported environment substitutions.' }
    $location = [string]::Join($profilePath, [regex]::Split($location, '(?i)%USERPROFILE%'))
  }
  $location = $location -replace '(?i)%SystemDrive%', $systemDrive
  if ($location.Contains('%') -or ![System.IO.Path]::IsPathRooted($location)) { throw 'The registered Local AppData could not be resolved without process environment overrides.' }
  $knownFolder = [Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData, [Environment+SpecialFolderOption]::DoNotVerify)
  @{ status = 'available'; path = $location; source = 'User Shell Folders registry, resolved using ProfileList'; knownFolderPath = $knownFolder; knownFolderMatchesRegistry = ($knownFolder -eq $location); environmentMatchesRegistry = ($env:LOCALAPPDATA -eq $location); platformRootInitialized = $false; volume = (Volume $location) }
}
# Runtime GUID, deliberately not an Edge browser registration. Documented at:
# https://learn.microsoft.com/microsoft-edge/webview2/concepts/distribution
$runtimeKey = 'Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
$machineKey = if ([Environment]::Is64BitOperatingSystem) { 'SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}' } else { $runtimeKey }
$result.webView2Registrations = @(
  @{ scope = 'machine'; registryKey = ('HKLM\' + $machineKey); probe = (Registry LocalMachine $machineKey 'pv') },
  @{ scope = 'currentUser'; registryKey = ('HKCU\' + $runtimeKey); probe = (Registry CurrentUser $runtimeKey 'pv') }
)
$result.programFilesX86 = Registry LocalMachine 'SOFTWARE\Microsoft\Windows\CurrentVersion' 'ProgramFilesDir (x86)'
$result.windowsSdkRoots = @(
  @{ view = 'Registry64'; probe = (Registry LocalMachine 'SOFTWARE\Microsoft\Windows Kits\Installed Roots' 'KitsRoot10' ([Microsoft.Win32.RegistryView]::Registry64)) },
  @{ view = 'Registry32'; probe = (Registry LocalMachine 'SOFTWARE\Microsoft\Windows Kits\Installed Roots' 'KitsRoot10' ([Microsoft.Win32.RegistryView]::Registry32)) }
)
$result | ConvertTo-Json -Depth 12 -Compress
`;

async function nativeInventory() {
  const windows = process.env.SystemRoot ?? process.env.WINDIR;
  const powershell = windows ? path.join(windows, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe') : 'powershell.exe';
  const result = await run(powershell, ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command', inventoryScript], {
    ...process.env, LOCAL_PROJECTS_DIAGNOSTIC_REPO: root,
  });
  if (result.status !== 'available') return result;
  try { return { status: 'available', ...JSON.parse(result.stdout.replace(/^\uFEFF/, '')) }; }
  catch { return { status: 'probeError', reason: 'invalidPowerShellJson' }; }
}

function combine(probes) {
  if (probes.some((probe) => probe.status === 'available')) return 'available';
  if (probes.some((probe) => probe.status === 'inaccessible')) return 'inaccessible';
  if (probes.some((probe) => probe.status === 'probeError')) return 'probeError';
  return 'missing';
}

function requiredStatus(probes) {
  if (probes.length && probes.every((probe) => probe.status === 'available')) return 'available';
  if (probes.some((probe) => probe.status === 'inaccessible')) return 'inaccessible';
  if (probes.some((probe) => probe.status === 'probeError')) return 'probeError';
  return 'missing';
}

function webView2(registrations) {
  if (!registrations) return { status: 'probeError', reason: 'nativeInventoryUnavailable' };
  const entries = registrations.map((entry) => {
    const probe = { ...entry.probe };
    if (probe.status === 'available') {
      const version = String(probe.value);
      probe.status = /^\d+\.\d+\.\d+\.\d+$/.test(version) ? (version === '0.0.0.0' ? 'missing' : 'available') : (version.trim() ? 'probeError' : 'missing');
      probe.version = version;
      delete probe.value;
    }
    return { scope: entry.scope, registryKey: entry.registryKey, ...probe };
  });
  return { status: combine(entries), registrations: entries, runtimeLaunchVerified: false };
}

async function npmProbe() {
  const location = await commandPath('npm');
  if (location.status !== 'available') return location;
  if (path.extname(location.path).toLowerCase() === '.exe') return versionProbe(location, ['--version']);
  // Run the actual CLI via Node, avoiding cmd.exe quoting or npm.cmd execution.
  const cli = await fileInfo(path.join(path.dirname(location.path), 'node_modules', 'npm', 'bin', 'npm-cli.js'));
  if (cli.status !== 'available') return { status: cli.status === 'missing' ? 'probeError' : cli.status,
    launcher: location.path, reason: 'npmCliBesideLauncherUnavailable', cliProbe: cli };
  const result = await run(process.execPath, [cli.path, '--version'], { ...process.env, npm_config_update_notifier: 'false' });
  return result.status === 'available' ? { status: 'available', path: location.path, cliPath: cli.path, nodePath: process.execPath, version: result.stdout }
    : { ...result, path: location.path };
}

async function rustProbe() {
  const localBin = path.join(root, '.tools', 'cargo', 'bin');
  const selected = rustEnvironment();
  const system = { ...process.env };
  const pathKey = Object.hasOwn(system, 'PATH') ? 'PATH' : Object.keys(system).find((key) => key.toLowerCase() === 'path') ?? 'PATH';
  system[pathKey] = (system[pathKey] ?? '').split(path.delimiter).filter((item) => path.resolve(item.replace(/^"|"$/g, '')).toLowerCase() !== localBin.toLowerCase()).join(path.delimiter);
  for (const key of ['CARGO_HOME', 'RUSTUP_HOME']) {
    if (system[key] && path.resolve(system[key]).toLowerCase().startsWith(path.join(root, '.tools').toLowerCase() + path.sep)) delete system[key];
  }
  // A diagnostic must not let rustup proxies fetch missing toolchains.
  const readOnly = (environment) => ({ ...environment, RUSTUP_AUTO_INSTALL: '0', RUSTUP_NO_UPDATE_CHECK: '1' });
  async function scope(environment) {
    const [cargoPath, rustcPath] = await Promise.all([commandPath('cargo', environment), commandPath('rustc', environment)]);
    const [cargo, rustc] = await Promise.all([versionProbe(cargoPath, ['--version'], readOnly(environment)), versionProbe(rustcPath, ['-vV'], readOnly(environment))]);
    if (rustc.status === 'available') {
      rustc.host = rustc.version.match(/^host: (.+)$/m)?.[1] ?? null;
      rustc.isX64MsvcHost = rustc.host === 'x86_64-pc-windows-msvc';
    }
    return { cargo, rustc };
  }
  const localEnvironment = readOnly({ ...selected, CARGO_HOME: path.join(root, '.tools', 'cargo'), RUSTUP_HOME: path.join(root, '.tools', 'rustup') });
  const [localCargo, localRustc, selectedTools, systemTools] = await Promise.all([
    fileInfo(path.join(localBin, 'cargo.exe')).then((location) => versionProbe(location, ['--version'], localEnvironment)),
    fileInfo(path.join(localBin, 'rustc.exe')).then((location) => versionProbe(location, ['-vV'], localEnvironment)), scope(selected), scope(system),
  ]);
  return { repositoryLocal: { cargo: localCargo, rustc: localRustc }, selectedByProject: selectedTools,
    systemPath: systemTools, note: 'System PATH excludes repository .tools; version probes do not establish native compilation readiness.' };
}

async function versionDirectories(directory) {
  try { return { status: 'available', versions: (await readdir(directory, { withFileTypes: true })).filter((entry) => entry.isDirectory() && /^\d+(\.\d+)+$/.test(entry.name)).map((entry) => entry.name).sort((a, b) => b.localeCompare(a, 'en', { numeric: true })) }; }
  catch (error) { return { ...failure(error), path: directory }; }
}

async function visualStudio(native) {
  if (native.programFilesX86?.status !== 'available') return { status: native.programFilesX86?.status ?? 'probeError', reason: 'programFilesRegistryUnavailable' };
  const location = await fileInfo(path.join(native.programFilesX86.value, 'Microsoft Visual Studio', 'Installer', 'vswhere.exe'));
  if (location.status !== 'available') return { status: location.status, vswhere: location, installations: { status: 'probeError', reason: 'vswhereUnavailable' } };
  const result = await run(location.path, ['-all', '-products', '*', '-format', 'json', '-utf8']);
  if (result.status !== 'available') return { status: result.status, vswhere: { ...result, path: location.path } };
  let instances;
  try { instances = JSON.parse(result.stdout.replace(/^\uFEFF/, '')); if (!Array.isArray(instances)) throw new Error(); }
  catch { return { status: 'probeError', vswhere: location, reason: 'invalidVswhereJson' }; }
  const installations = await Promise.all(instances.map(async (instance) => {
    const toolsets = await versionDirectories(path.join(instance.installationPath, 'VC', 'Tools', 'MSVC'));
    const tools = await Promise.all((toolsets.versions ?? []).map(async (version) => {
      const bin = path.join(instance.installationPath, 'VC', 'Tools', 'MSVC', version, 'bin', 'Hostx64', 'x64');
      const [compiler, linker] = await Promise.all([fileInfo(path.join(bin, 'cl.exe')), fileInfo(path.join(bin, 'link.exe'))]);
      return { version, compiler, linker };
    }));
    return { installationPath: instance.installationPath, installationVersion: instance.installationVersion,
      isComplete: instance.isComplete, isLaunchable: instance.isLaunchable, toolsets: { ...toolsets, tools } };
  }));
  return { status: installations.length ? 'available' : 'missing', vswhere: location, installations,
    x64ToolsStatus: combine(installations.map((instance) => ({ status: instance.toolsets.status === 'available'
      ? combine(instance.toolsets.tools.map((tool) => ({ status: requiredStatus([tool.compiler, tool.linker]) }))) : instance.toolsets.status }))), compilationVerified: false };
}

async function windowsSdk(roots) {
  if (!roots) return { status: 'probeError', reason: 'nativeInventoryUnavailable' };
  const installations = [];
  const seen = new Set();
  for (const entry of roots) {
    if (entry.probe.status !== 'available') continue;
    const sdkRoot = String(entry.probe.value);
    if (seen.has(sdkRoot.toLowerCase())) continue;
    seen.add(sdkRoot.toLowerCase());
    const inventory = await versionDirectories(path.join(sdkRoot, 'Include'));
    const versions = await Promise.all((inventory.versions ?? []).map(async (version) => {
      const required = [path.join('Include', version, 'um', 'Windows.h'), path.join('Include', version, 'ucrt', 'stdio.h'),
        path.join('Lib', version, 'um', 'x64', 'kernel32.lib'), path.join('Lib', version, 'ucrt', 'x64', 'ucrt.lib'), path.join('bin', version, 'x64', 'rc.exe')];
      const files = await Promise.all(required.map((relative) => fileInfo(path.join(sdkRoot, relative))));
      return { version, status: requiredStatus(files), files };
    }));
    installations.push({ root: sdkRoot, status: inventory.status === 'available' ? combine(versions) : inventory.status, versions,
      ...(inventory.status === 'available' ? {} : { inventory }) });
  }
  return { status: combine([...installations, ...roots.filter((entry) => entry.probe.status !== 'available').map((entry) => entry.probe)]), registryRoots: roots, installations, compilationVerified: false };
}

async function main() {
  try {
    const [native, npm, rust] = await Promise.all([nativeInventory(), npmProbe(), rustProbe()]);
    const [msvc, sdk] = await Promise.all([visualStudio(native), windowsSdk(native.windowsSdkRoots)]);
    const system = native.system ?? { status: native.status, reason: 'nativeInventoryUnavailable' };
    const registry = system.registry;
    const build = registry?.CurrentBuildNumber.status === 'available' ? Number(registry.CurrentBuildNumber.value) : null;
    const target = system.status === 'available' && build !== null && registry?.InstallationType.status === 'available'
      ? { status: 'available', matchesConfirmedOsTarget: system.osArchitecture === 'X64' && registry.InstallationType.value === 'Client'
        && (build === 19045 || build >= 22000), note: 'OS target check only; this is not a Windows compatibility acceptance result.' }
      : { status: 'probeError', reason: 'systemIdentityIncomplete' };
    const report = {
      schemaVersion: 1, generatedAt: new Date().toISOString(), readOnly: true,
      context: { platform: process.platform, processArchitecture: process.arch, osReleaseReportedByNode: os.release(), repository: root },
      system, target, privileges: native.privileges ?? { status: native.status }, repositoryVolume: native.repositoryVolume ?? { status: native.status },
      localAppData: native.localAppData ?? { status: native.status }, webView2: webView2(native.webView2Registrations),
      node: { status: 'available', path: process.execPath, version: process.version, architecture: process.arch,
        supportedByProject: /^v22\./.test(process.version) ? Number(process.versions.node.split('.')[1]) >= 12 : /^v24\./.test(process.version) },
      npm, rust, visualStudio: msvc, windowsSdk: sdk,
      limitations: ['No installations, platform startup, data-root initialization, builds, writes or device tests are performed.',
        'Fixed drive type and NTFS do not establish ACL, reparse-point, durability, long-path or removable-device safety.',
        'Registry presence and tool files do not prove WebView2 startup or successful MSVC linking.'],
      ...(native.status === 'available' ? {} : { nativeProbe: native }),
    };
    console.log(JSON.stringify(report, null, 2));
    if (native.status !== 'available') process.exitCode = 1;
  } catch (error) {
    console.log(JSON.stringify({ schemaVersion: 1, status: 'probeError', error: failure(error) }, null, 2));
    process.exitCode = 1;
  }
}

const options = process.argv.slice(2);
if (options.some((option) => option !== '--json')) {
  console.error('Usage: node scripts/windows-environment.mjs [--json]');
  process.exitCode = 2;
} else if (process.platform !== 'win32') {
  console.log(JSON.stringify({ schemaVersion: 1, status: 'probeError', code: 'WINDOWS_REQUIRED', platform: process.platform }, null, 2));
  process.exitCode = 1;
} else {
  await main();
}
