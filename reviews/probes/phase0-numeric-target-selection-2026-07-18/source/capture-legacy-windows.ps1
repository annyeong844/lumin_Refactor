param(
    [Parameter(Mandatory = $true)] [string] $RepoRoot,
    [Parameter(Mandatory = $true)] [string] $OutputRoot,
    [Parameter(Mandatory = $true)] [ValidateSet('cold', 'warm')] [string] $Mode
)

$ErrorActionPreference = 'Stop'
$ExpectedCommit = '35290cb683a37e83bc8c915d1d0f9ca0f3f96fd0'
$RepoRoot = (Resolve-Path -LiteralPath $RepoRoot).Path
$OutputRoot = [IO.Path]::GetFullPath($OutputRoot)

if (Test-Path -LiteralPath $OutputRoot) {
    throw "refusing to overwrite output: $OutputRoot"
}
if ((git -C $RepoRoot rev-parse HEAD).Trim() -ne $ExpectedCommit) {
    throw 'legacy repository is not at the exact baseline commit'
}
if ((git -C $RepoRoot status --porcelain).Count -ne 0) {
    throw 'legacy baseline worktree must be clean'
}

New-Item -ItemType Directory -Path $OutputRoot | Out-Null
$Utf8 = [Text.UTF8Encoding]::new($false)

function Get-ProcessTreeRss([int] $RootPid) {
    $rows = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId
    $ids = [Collections.Generic.HashSet[int]]::new()
    [void] $ids.Add($RootPid)
    do {
        $changed = $false
        foreach ($row in $rows) {
            if ($ids.Contains([int] $row.ParentProcessId) -and $ids.Add([int] $row.ProcessId)) {
                $changed = $true
            }
        }
    } while ($changed)

    $rss = 0L
    foreach ($id in $ids) {
        $process = Get-Process -Id $id -ErrorAction SilentlyContinue
        if ($null -ne $process) {
            $rss += [int64] $process.WorkingSet64
        }
    }
    return $rss
}

function Invoke-LegacyAudit([string] $Name, [string[]] $ExtraArgs, [bool] $Measure) {
    $artifactRoot = Join-Path $OutputRoot "$Name-artifacts"
    $stdoutPath = Join-Path $OutputRoot "$Name.stdout.txt"
    $stderrPath = Join-Path $OutputRoot "$Name.stderr.txt"
    $arguments = @(
        (Join-Path $RepoRoot 'audit-repo.mjs'),
        '--root', $RepoRoot,
        '--output', $artifactRoot,
        '--profile', 'full',
        '--no-self-audit-excludes'
    ) + $ExtraArgs

    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = (Get-Command node).Source
    $startInfo.UseShellExecute = $false
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    foreach ($argument in $arguments) {
        [void] $startInfo.ArgumentList.Add($argument)
    }

    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    $clock = [Diagnostics.Stopwatch]::StartNew()
    if (-not $process.Start()) {
        throw 'failed to start legacy audit'
    }
    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()
    $samples = [Collections.Generic.List[object]]::new()
    while (-not $process.HasExited) {
        if ($Measure) {
            $samples.Add([ordered]@{
                elapsedMs = $clock.ElapsedMilliseconds
                processTreeRssBytes = Get-ProcessTreeRss $process.Id
            })
        }
        Start-Sleep -Milliseconds 250
    }
    $process.WaitForExit()
    $clock.Stop()
    [IO.File]::WriteAllText($stdoutPath, $stdoutTask.Result, $Utf8)
    [IO.File]::WriteAllText($stderrPath, $stderrTask.Result, $Utf8)
    if ($process.ExitCode -ne 0) {
        throw "legacy audit failed with exit $($process.ExitCode)"
    }
    if (-not $Measure) {
        return
    }

    $peak = ($samples | ForEach-Object { [int64] $_['processTreeRssBytes'] } | Measure-Object -Maximum).Maximum
    $measurement = [ordered]@{
        schemaVersion = 'legacy-full-baseline.v1'
        baselineCommit = $ExpectedCommit
        mode = $Mode
        command = @($startInfo.FileName) + $arguments
        elapsedMs = $clock.ElapsedMilliseconds
        processTreePeakRssBytes = [int64] $peak
        sampleCount = $samples.Count
        samplingDelayMs = 250
        samples = $samples
    }
    [IO.File]::WriteAllText(
        (Join-Path $OutputRoot 'measurement.json'),
        ($measurement | ConvertTo-Json -Depth 6) + "`n",
        $Utf8
    )
}

if ($Mode -eq 'cold') {
    Invoke-LegacyAudit 'measured' @('--no-incremental') $true
} else {
    $cacheRoot = Join-Path $OutputRoot 'cache'
    Invoke-LegacyAudit 'seed' @('--cache-root', $cacheRoot) $false
    Invoke-LegacyAudit 'measured' @('--cache-root', $cacheRoot) $true
}

$computer = Get-CimInstance Win32_ComputerSystem
$os = Get-CimInstance Win32_OperatingSystem
$cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
$hostRecord = [ordered]@{
    schemaVersion = 'numeric-target-host.v1'
    platform = 'windows-ntfs'
    os = $os.Caption
    osVersion = $os.Version
    cpu = $cpu.Name.Trim()
    physicalCores = [int] $cpu.NumberOfCores
    logicalProcessors = [int] $cpu.NumberOfLogicalProcessors
    memoryBytes = [int64] $computer.TotalPhysicalMemory
    node = (node --version).Trim()
    npm = (npm --version).Trim()
    filesystem = 'NTFS'
}
[IO.File]::WriteAllText(
    (Join-Path $OutputRoot 'host.json'),
    ($hostRecord | ConvertTo-Json -Depth 4) + "`n",
    $Utf8
)
