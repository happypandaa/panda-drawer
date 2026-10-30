[CmdletBinding()]
param(
    [string]$InstallDirectory = 'C:\actions-runner-panda',
    [string]$RegistrationTokenFile,
    [switch]$PrepareOnly,
    [string]$CargoHome,
    [string]$RustupHome,
    [string]$RustBin
)

$ErrorActionPreference = 'Stop'
$runnerVersion = '2.338.2'
$runnerSha256 = 'a1f17aa987fc8158bc7521d284912267a0d01b7c2c4684b33c86783b5e8221cc'
$runnerUrl = "https://github.com/actions/runner/releases/download/v$runnerVersion/actions-runner-win-x64-$runnerVersion.zip"
$runnerRepository = 'https://github.com/happypandaa/panda-drawer'
if (![Environment]::Is64BitProcess -or $env:OS -ne 'Windows_NT') {
    throw 'The runner requires Windows x64 PowerShell.'
}
$runnerRoot = [System.IO.Path]::GetFullPath($InstallDirectory)
if ($runnerRoot -eq [System.IO.Path]::GetPathRoot($runnerRoot)) {
    throw 'Select a dedicated runner directory, not a drive root.'
}
if (Test-Path -LiteralPath (Join-Path $runnerRoot '.runner')) {
    throw 'This runner is already registered. Inspect it instead of replacing its configuration.'
}
if (!$PrepareOnly) {
    $runnerPrincipal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
    if (!$runnerPrincipal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'Registration as a Windows service requires an administrator PowerShell. No registration was attempted.'
    }
    if (!$RegistrationTokenFile -or !(Test-Path -LiteralPath $RegistrationTokenFile -PathType Leaf)) {
        throw 'Supply a local file containing the one-hour repository runner registration token. Do not commit the token.'
    }
}
New-Item -ItemType Directory -Path $runnerRoot -Force | Out-Null
$runnerInstallLock = [System.IO.File]::Open((Join-Path $runnerRoot '.install.lock'), [System.IO.FileMode]::OpenOrCreate, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::None)
try {
    if (Test-Path -LiteralPath (Join-Path $runnerRoot '.runner')) { throw 'A concurrent installer already registered this runner.' }
    $runnerArchive = Join-Path $runnerRoot "actions-runner-win-x64-$runnerVersion.zip"
    if (!(Test-Path -LiteralPath $runnerArchive)) {
        Invoke-WebRequest -Uri $runnerUrl -OutFile $runnerArchive -UseBasicParsing
    }
    $runnerObservedSha = (Get-FileHash -LiteralPath $runnerArchive -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($runnerObservedSha -ne $runnerSha256) { throw 'The runner archive does not match the official release SHA256.' }
    if (!(Test-Path -LiteralPath (Join-Path $runnerRoot 'config.cmd'))) {
        Expand-Archive -LiteralPath $runnerArchive -DestinationPath $runnerRoot
    }
    $runnerListener = Join-Path $runnerRoot 'bin/Runner.Listener.exe'
    $runnerObservedVersion = (& $runnerListener --version | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $runnerObservedVersion -ne $runnerVersion) {
        throw 'The extracted runner did not report the expected version.'
    }
    Write-Output "Official runner $runnerVersion verified at $runnerRoot"
    if ($PrepareOnly) {
        Write-Output 'Prepared only: no GitHub registration, service installation or claim of an online runner.'
        exit 0
    }

    $runnerDotEnv = Join-Path $runnerRoot '.env'
    $runnerRegistrationToken = [System.IO.File]::ReadAllText((Resolve-Path -LiteralPath $RegistrationTokenFile)).Trim()
    if ($runnerRegistrationToken -notmatch '^[A-Za-z0-9_-]{10,256}$') {
        throw 'The registration token file has an invalid format.'
    }
    $runnerEnvironment = New-Object System.Collections.Generic.List[string]
    foreach ($entry in @(@('CARGO_HOME', $CargoHome), @('RUSTUP_HOME', $RustupHome))) {
        if ($entry[1]) {
            $runnerToolPath = [System.IO.Path]::GetFullPath($entry[1])
            if (!(Test-Path -LiteralPath $runnerToolPath -PathType Container) -or $runnerToolPath -match "[`r`n]") {
                throw "Invalid existing toolchain directory: $($entry[0])"
            }
            $runnerEnvironment.Add("$($entry[0])=$runnerToolPath")
        }
    }
    if ($RustBin -or $CargoHome) {
        $runnerCargoBin = if ($RustBin) { [System.IO.Path]::GetFullPath($RustBin) } else { Join-Path ([System.IO.Path]::GetFullPath($CargoHome)) 'bin' }
        if ($runnerCargoBin -match "[`r`n]") { throw 'Invalid Rust binary directory.' }
        if (!(Test-Path -LiteralPath (Join-Path $runnerCargoBin 'cargo.exe'))) { throw 'The selected Rust binary directory has no cargo.exe.' }
        $runnerEnvironment.Add("PANDA_CI_RUST_BIN=$runnerCargoBin")
    }
    if ($runnerEnvironment.Count) {
        $runnerExpectedEnvironment = ($runnerEnvironment -join [Environment]::NewLine) + [Environment]::NewLine
        if (Test-Path -LiteralPath $runnerDotEnv) {
            if ([System.IO.File]::ReadAllText($runnerDotEnv) -ne $runnerExpectedEnvironment) {
                throw 'An existing runner .env differs from these toolchain settings and must be reviewed rather than overwritten.'
            }
        } else {
            [System.IO.File]::WriteAllText($runnerDotEnv, $runnerExpectedEnvironment, (New-Object System.Text.UTF8Encoding($false)))
        }
    }
    # The runner accepts inputs through environment variables, keeping the token out
    # of this script's output and the config.cmd command line. Its own secret handling
    # remains the upstream runner's responsibility. Never print the environment.
    $runnerPreviousToken = $env:ACTIONS_RUNNER_INPUT_TOKEN
    try {
        $env:ACTIONS_RUNNER_INPUT_TOKEN = $runnerRegistrationToken
        Push-Location -LiteralPath $runnerRoot
        try {
            & (Join-Path $runnerRoot 'config.cmd') --unattended --url $runnerRepository --name panda-windows-x64 --work _work --runasservice
            if ($LASTEXITCODE -ne 0) { throw 'Runner registration or service setup failed. Review the upstream runner diagnostic log without sharing credentials.' }
        } finally { Pop-Location }
    } finally {
        $env:ACTIONS_RUNNER_INPUT_TOKEN = $runnerPreviousToken
        $runnerRegistrationToken = $null
    }
    $runnerServiceNameFile = Join-Path $runnerRoot '.service'
    if (!(Test-Path -LiteralPath $runnerServiceNameFile)) { throw 'Registration did not produce a service identity.' }
    $runnerServiceName = [System.IO.File]::ReadAllText($runnerServiceNameFile).Trim()
    $runnerService = Get-Service -Name $runnerServiceName
    if ($runnerService.Status -ne 'Running') { Start-Service -Name $runnerServiceName }
    Get-Service -Name $runnerServiceName | Select-Object Name, Status
    Write-Output 'Service installed and running locally. Verify the Windows job is assigned in GitHub before recording CI success.'
} finally { $runnerInstallLock.Dispose() }
