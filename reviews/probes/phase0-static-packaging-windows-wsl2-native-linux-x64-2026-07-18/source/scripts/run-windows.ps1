[CmdletBinding()]
param(
    [string]$EvidencePath
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Invoke-RecordedProcess {
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][string[]]$Arguments,
        [Parameter(Mandatory = $true)][string]$StdoutPath,
        [Parameter(Mandatory = $true)][string]$StderrPath
    )

    $start = @{
        FilePath               = $FilePath
        WorkingDirectory       = $script:SourceRoot
        NoNewWindow            = $true
        Wait                   = $true
        PassThru               = $true
        RedirectStandardOutput = $StdoutPath
        RedirectStandardError  = $StderrPath
    }
    if ($Arguments.Count -gt 0) {
        $start.ArgumentList = $Arguments
    }
    $process = Start-Process @start
    if ($process.ExitCode -ne 0) {
        $stderr = if (Test-Path -LiteralPath $StderrPath) {
            Get-Content -Raw -LiteralPath $StderrPath
        } else {
            '<missing stderr>'
        }
        throw "$FilePath exited $($process.ExitCode): $stderr"
    }
}

$ScriptRoot = Split-Path -Parent $PSCommandPath
$SourceRoot = (Resolve-Path (Join-Path $ScriptRoot '..')).Path
$PacketRoot = (Resolve-Path (Join-Path $SourceRoot '..')).Path
if (-not $EvidencePath) {
    $EvidencePath = Join-Path $PacketRoot 'evidence\windows-ntfs'
}
$EvidencePath = [IO.Path]::GetFullPath($EvidencePath)

if ($env:OS -ne 'Windows_NT' -or $env:PROCESSOR_ARCHITECTURE -ne 'AMD64') {
    throw "Windows x64 runner required; observed OS=$env:OS arch=$env:PROCESSOR_ARCHITECTURE"
}
$driveRoot = [IO.Path]::GetPathRoot($SourceRoot)
$driveLetter = $driveRoot.Substring(0, 1)
$volume = Get-Volume -DriveLetter $driveLetter
if ([string]$volume.FileSystemType -ne 'NTFS') {
    throw "NTFS source worktree required; observed $($volume.FileSystemType) at $driveRoot"
}
if (Test-Path -LiteralPath $EvidencePath) {
    throw "refusing existing evidence directory: $EvidencePath"
}

$Python = (Get-Command python -ErrorAction Stop).Source
$Cargo = (Get-Command cargo -ErrorAction Stop).Source
$Rustc = (Get-Command rustc -ErrorAction Stop).Source
$Packager = Join-Path $ScriptRoot 'package_evidence.py'
& $Python $Packager verify-source --source $SourceRoot
if ($LASTEXITCODE -ne 0) {
    throw 'source manifest verification failed'
}

$rustcVersion = (& $Rustc --version).Trim()
$rustcVerbose = ((& $Rustc --version --verbose) -join "`n").Trim()
$cargoVersion = (& $Cargo --version).Trim()
if (-not $rustcVersion.StartsWith('rustc 1.96.0 ') -or -not $cargoVersion.StartsWith('cargo 1.96.0 ')) {
    throw "exact Rust 1.96.0 required; observed $rustcVersion / $cargoVersion"
}

New-Item -ItemType Directory -Path $EvidencePath | Out-Null
$quality = Join-Path $SourceRoot 'target\probe-runner-logs\windows-msvc'
New-Item -ItemType Directory -Force -Path $quality | Out-Null

$hostIdentity = [ordered]@{
    schema           = 'lumin-phase0-static-packaging-host-v1'
    scope            = 'windows-ntfs'
    hostKind         = 'windows'
    os               = 'windows'
    arch             = 'x86_64'
    filesystemType   = 'ntfs'
    filesystemDetail = "$driveRoot $($volume.FileSystemLabel) $($volume.HealthStatus)"
    sourcePath       = $SourceRoot
    rustcVersion     = $rustcVersion
    rustcVerbose     = $rustcVerbose
    cargoVersion     = $cargoVersion
    windowsVersion   = [Environment]::OSVersion.VersionString
}
$hostJson = $hostIdentity | ConvertTo-Json -Depth 4
[IO.File]::WriteAllText(
    (Join-Path $EvidencePath 'host.json'),
    $hostJson + "`n",
    [Text.UTF8Encoding]::new($false)
)

Invoke-RecordedProcess $Cargo @('fmt', '--all', '--', '--check') `
    (Join-Path $quality 'fmt.stdout.log') (Join-Path $quality 'fmt.stderr.log')
Invoke-RecordedProcess $Cargo @('test', '--locked', '--target', 'x86_64-pc-windows-msvc') `
    (Join-Path $quality 'test.stdout.log') (Join-Path $quality 'test.stderr.log')
Invoke-RecordedProcess $Cargo @(
    'clippy', '--all-targets', '--locked', '--target', 'x86_64-pc-windows-msvc',
    '--', '-D', 'warnings'
) (Join-Path $quality 'clippy.stdout.log') (Join-Path $quality 'clippy.stderr.log')

Invoke-RecordedProcess $Cargo @('metadata', '--locked', '--format-version', '1') `
    (Join-Path $EvidencePath 'cargo-metadata.json') `
    (Join-Path $EvidencePath 'cargo-metadata.stderr.log')
Invoke-RecordedProcess $Cargo @('tree', '--locked', '--target', 'x86_64-pc-windows-msvc') `
    (Join-Path $EvidencePath 'cargo-tree-windows-msvc.txt') `
    (Join-Path $EvidencePath 'cargo-tree-windows-msvc.stderr.log')
Invoke-RecordedProcess $Cargo @(
    'build', '--release', '--locked', '--target', 'x86_64-pc-windows-msvc'
) (Join-Path $EvidencePath 'build-windows-msvc.stdout.log') `
    (Join-Path $EvidencePath 'build-windows-msvc.stderr.log')

$binary = Join-Path $SourceRoot `
    'target\x86_64-pc-windows-msvc\release\lumin-phase0-static-packaging-probe.exe'
if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
    throw "release artifact missing: $binary"
}
Invoke-RecordedProcess $binary @() `
    (Join-Path $EvidencePath 'run-windows-msvc.json') `
    (Join-Path $EvidencePath 'run-windows-msvc.stderr.log')

$artifact = Get-Item -LiteralPath $binary
$artifactHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $binary).Hash.ToLowerInvariant()
$linkage = @(
    'artifact-format-check: package_evidence.py parses PE signature, machine, and PE32+ magic'
    "artifact-path: $binary"
    "artifact-bytes: $($artifact.Length)"
    "artifact-sha256: $artifactHash"
    "rust-target: x86_64-pc-windows-msvc"
)
$dumpbin = Get-Command dumpbin.exe -ErrorAction SilentlyContinue
if ($dumpbin) {
    $linkage += "dumpbin: $($dumpbin.Source)"
    $linkage += (& $dumpbin.Source /headers /dependents $binary 2>&1 | Out-String).TrimEnd()
} else {
    $linkage += 'dumpbin: unavailable; independent PE parsing remains mandatory'
}
[IO.File]::WriteAllText(
    (Join-Path $EvidencePath 'linkage-windows-msvc.txt'),
    ($linkage -join "`n") + "`n",
    [Text.UTF8Encoding]::new($false)
)

& $Python $Packager seal `
    --scope windows-ntfs `
    --source $SourceRoot `
    --evidence $EvidencePath `
    --artifact "windows-msvc=$binary"
if ($LASTEXITCODE -ne 0) {
    throw 'evidence sealing failed'
}
& $Python $Packager verify --source $SourceRoot --evidence $EvidencePath
if ($LASTEXITCODE -ne 0) {
    throw 'sealed evidence verification failed'
}

Write-Host "PASS: $EvidencePath"
