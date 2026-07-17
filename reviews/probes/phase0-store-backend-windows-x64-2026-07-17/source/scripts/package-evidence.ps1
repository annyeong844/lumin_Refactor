param(
    [string]$EvidencePath = "evidence",
    [int]$ExpectedFaultCasesPerBackend = 42
)

$ErrorActionPreference = "Stop"
$Root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$Evidence = if ([IO.Path]::IsPathRooted($EvidencePath)) {
    [IO.Path]::GetFullPath($EvidencePath)
} else {
    [IO.Path]::GetFullPath((Join-Path $Root $EvidencePath))
}
$ArchitectureCommit = "65e60216891bb3d826a4778f84cb8aaa377abe92"
$ArchitectureManifest = "66925583362be22257dd7357072d176cd3f1ae3d4f99874a53f6689b7cf535e0"

function Read-EvidenceJson([string]$Name) {
    $Path = Join-Path $Evidence $Name
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "missing evidence file: $Path"
    }
    Get-Content -Raw -LiteralPath $Path | ConvertFrom-Json
}

function Assert-Candidate($Document, [string]$Name) {
    if ($Document.architecture_commit -ne $ArchitectureCommit) {
        throw "$Name architecture commit mismatch"
    }
    if ($Document.architecture_manifest_sha256 -ne $ArchitectureManifest) {
        throw "$Name architecture manifest mismatch"
    }
}

$Admission = Read-EvidenceJson "admission-windows-x64.json"
$Fault = Read-EvidenceJson "fault-matrix-windows-x64.json"
$Build = Read-EvidenceJson "build-surface-windows-x64.json"
$Redb = Read-EvidenceJson "benchmark-redb-windows-x64.json"
$Sqlite = Read-EvidenceJson "benchmark-sqlite-windows-x64.json"

foreach ($Pair in @(
    @($Admission, "admission"),
    @($Fault, "fault-matrix"),
    @($Build, "build-surface"),
    @($Redb, "benchmark-redb"),
    @($Sqlite, "benchmark-sqlite")
)) {
    Assert-Candidate $Pair[0] $Pair[1]
}
if ($Admission.overall_status -ne "PASS") { throw "admission did not pass" }
if ($Fault.overall_status -ne "PASS") { throw "fault matrix did not pass" }
if ($Redb.status -ne "PASS" -or $Sqlite.status -ne "PASS") {
    throw "one or more benchmarks did not pass"
}
foreach ($Backend in $Fault.backends) {
    if ($Backend.status -ne "PASS") { throw "$($Backend.backend) fault matrix failed" }
    if ($Backend.cases.Count -ne $ExpectedFaultCasesPerBackend) {
        throw "$($Backend.backend) fault count was $($Backend.cases.Count), expected $ExpectedFaultCasesPerBackend"
    }
}
if ($Build.results.Count -ne 2) { throw "build report must contain two backends" }
foreach ($Result in $Build.results) {
    if ($Result.clean_build -is [string] -or $Result.incremental_build -is [string] -or
        $Result.dependency_surface -is [string]) {
        throw "$($Result.backend) build metrics were stringified instead of structured"
    }
}

$BuildByBackend = @{}
foreach ($Result in $Build.results) { $BuildByBackend[$Result.backend] = $Result }
$Summary = [pscustomobject][ordered]@{
    probe_id = "lumin-store-windows-x64-evidence-summary-v1"
    scope = "windows-x64-partial-phase0-evidence"
    status = "PASS"
    architecture_commit = $ArchitectureCommit
    architecture_manifest_sha256 = $ArchitectureManifest
    backend_selected = $false
    correctness = [pscustomobject][ordered]@{
        admission_rounds_per_backend = $Admission.rounds
        admission_backends = @($Admission.backends | ForEach-Object { $_.backend })
        fault_cases_per_backend = $ExpectedFaultCasesPerBackend
        total_fault_cases = ($Fault.backends | ForEach-Object { $_.cases.Count } | Measure-Object -Sum).Sum
    }
    measurements = @(
        foreach ($Benchmark in @($Redb, $Sqlite)) {
            $BuildResult = $BuildByBackend[$Benchmark.backend]
            [pscustomobject][ordered]@{
                backend = $Benchmark.backend
                initialize_micros = $Benchmark.initialize_micros
                bulk_insert_micros = $Benchmark.bulk_insert_micros
                first_reopen_query_micros = $Benchmark.first_reopen_query_micros
                warm_reopen_query = $Benchmark.warm_reopen_query
                durable_admission = $Benchmark.durable_admission
                peak_working_set_bytes = $Benchmark.peak_working_set_bytes
                store_bytes = $Benchmark.store_bytes
                binary_bytes = $BuildResult.binary_bytes
                clean_build_millis = $BuildResult.clean_build.elapsed_millis
                incremental_build_millis = $BuildResult.incremental_build.elapsed_millis
                transitive_package_count = $BuildResult.dependency_surface.transitive_package_count
                rust_unsafe_keyword_line_count = $BuildResult.dependency_surface.rust_unsafe_keyword_line_count
                native_source_file_count = $BuildResult.dependency_surface.native_source_file_count
                native_source_bytes = $BuildResult.dependency_surface.native_source_bytes
            }
        }
    )
    interpretation = @(
        "Correctness passed for this Windows x64 harness and case set.",
        "The first reopen query is not an operating-system cold-cache measurement.",
        "The unsafe keyword count is a comparison surface, not a safety audit.",
        "No backend is selected until all blocking platform and correctness evidence passes."
    )
    pending = @(
        "Linux ext4 and Linux musl store/fault/package evidence",
        "required filesystem durable-flush and lock semantics",
        "native path/root and packaged skill evidence",
        "OXC memory and worker-stack evidence",
        "approved cross-platform numeric budgets"
    )
}

$SummaryPath = Join-Path $Evidence "windows-x64-summary.json"
$SummaryJson = ($Summary | ConvertTo-Json -Depth 30).Replace("`r`n", "`n") + "`n"
[IO.File]::WriteAllText($SummaryPath, $SummaryJson, [Text.UTF8Encoding]::new($false))

$RawFiles = @(
    "admission-windows-x64.json",
    "fault-matrix-windows-x64.json",
    "build-surface-windows-x64.json",
    "benchmark-redb-windows-x64.json",
    "benchmark-sqlite-windows-x64.json",
    "build-redb-clean.log",
    "build-redb-incremental.log",
    "build-sqlite-clean.log",
    "build-sqlite-incremental.log",
    "dependency-tree-redb.txt",
    "dependency-tree-sqlite.txt",
    "windows-x64-summary.json"
)
$ManifestLines = foreach ($Name in $RawFiles | Sort-Object -CaseSensitive) {
    $Path = Join-Path $Evidence $Name
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "missing manifest input: $Path"
    }
    $Hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
    "$Hash  $Name"
}
$ManifestPath = Join-Path $Evidence "SHA256SUMS"
[IO.File]::WriteAllText(
    $ManifestPath,
    (($ManifestLines -join "`n") + "`n"),
    [Text.UTF8Encoding]::new($false)
)
Write-Output $SummaryPath
Write-Output $ManifestPath
