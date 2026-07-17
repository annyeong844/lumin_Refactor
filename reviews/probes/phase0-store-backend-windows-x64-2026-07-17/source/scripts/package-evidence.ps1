param(
    [string]$EvidencePath = "evidence",
    [int]$ExpectedFaultCasesPerBackend = 47
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
$ExpectedBackends = @("redb", "sqlite")
$ExpectedFaultCases = @(
    "backend-contract|indexed-query",
    "backend-contract|corruption-visible",
    "publication|before-attempt-catalog-allocation",
    "publication|after-catalog-allocation",
    "publication|after-running-envelope",
    "publication|after-latest-running",
    "publication|after-run-rename",
    "publication|after-terminal-attempt",
    "publication|after-latest-temp",
    "publication|after-latest-replace",
    "publication-concurrency|reverse-sequence-independent-fields",
    "publication-concurrency|same-sequence-terminal-beats-running",
    "publication-retention-race|publication-first-makes-retention-stale",
    "publication-retention-race|retention-first-blocks-publication",
    "retention|before-prepared-plan",
    "retention|after-prepared-plan",
    "retention|after-pruning-commit",
    "retention|after-payload-move",
    "retention|after-pruned-commit",
    "retention|after-physical-reclamation",
    "retention-integrity|both-canonical-and-trash",
    "retention-integrity|neither-canonical-nor-trash",
    "migration|before-migration-intent",
    "migration|after-migration-intent",
    "migration|after-validated-replacement",
    "migration|after-canonical-replace",
    "migration|after-intent-removal",
    "migration|stale-generation-writer",
    "namespace|state-directory-copy-swap",
    "namespace|lifecycle-lock-replacement",
    "namespace|lifecycle-lock-content-mutation",
    "namespace|lifecycle-lock-extra-link",
    "namespace|attempts-parent-replacement",
    "namespace|runs-parent-replacement",
    "namespace|trash-parent-replacement",
    "namespace|cache-parent-replacement",
    "namespace|attempts-anchor-replacement",
    "namespace|runs-anchor-replacement",
    "namespace|trash-anchor-replacement",
    "namespace|cache-anchor-replacement",
    "namespace|runs-anchor-content-mutation",
    "namespace|runs-anchor-extra-link",
    "namespace|runs-parent-replacement-after-run-rename",
    "namespace|runs-parent-replacement-before-final-commit",
    "namespace|trash-parent-replacement-before-trash-move",
    "namespace|trash-parent-replacement-after-trash-move",
    "namespace|trash-parent-replacement-before-final-commit"
)
$KernelPreventionEligibleCases = @(
    "namespace|state-directory-copy-swap",
    "namespace|attempts-parent-replacement",
    "namespace|runs-parent-replacement",
    "namespace|trash-parent-replacement",
    "namespace|cache-parent-replacement",
    "namespace|runs-parent-replacement-after-run-rename",
    "namespace|runs-parent-replacement-before-final-commit",
    "namespace|trash-parent-replacement-before-trash-move",
    "namespace|trash-parent-replacement-after-trash-move",
    "namespace|trash-parent-replacement-before-final-commit"
)

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

function Assert-BackendSet($Items, [string]$Name) {
    $Names = @($Items | ForEach-Object { $_.backend } | Sort-Object -CaseSensitive)
    $Expected = @($ExpectedBackends | Sort-Object -CaseSensitive)
    $Difference = @(Compare-Object -ReferenceObject $Expected -DifferenceObject $Names -CaseSensitive)
    if ($Difference.Count -ne 0 -or $Names.Count -ne $Expected.Count) {
        throw "$Name backend set mismatch: $($Names -join ', ')"
    }
}

function Get-Sha256Text([string]$Text) {
    $Hasher = [Security.Cryptography.SHA256]::Create()
    try {
        $Bytes = [Text.UTF8Encoding]::new($false).GetBytes($Text)
        -join ($Hasher.ComputeHash($Bytes) | ForEach-Object { $_.ToString("x2") })
    } finally {
        $Hasher.Dispose()
    }
}

function Get-SourceFingerprint($Document, [string]$Name) {
    if ($null -eq $Document.source_files -or $Document.source_files.Count -eq 0) {
        throw "$Name has no embedded source identity"
    }
    $Seen = @{}
    $Lines = foreach ($Source in $Document.source_files) {
        if ([string]::IsNullOrWhiteSpace($Source.path) -or $Source.sha256 -notmatch '^[0-9a-f]{64}$') {
            throw "$Name has malformed source identity"
        }
        if ($Seen.ContainsKey($Source.path)) {
            throw "$Name repeats source path $($Source.path)"
        }
        $Seen[$Source.path] = $true
        "$($Source.sha256)  $($Source.path)"
    }
    $Text = ((@($Lines | Sort-Object -CaseSensitive) -join "`n") + "`n")
    $Digest = Get-Sha256Text $Text
    if ($Digest -ne $Document.source_manifest_sha256) {
        throw "$Name source manifest digest mismatch"
    }
    $Text
}

function Assert-ExecutableIdentity($Document, $Expected, [string]$Name) {
    $ExpectedSha256 = if ($null -ne $Expected.sha256) {
        $Expected.sha256
    } elseif ($null -ne $Expected.binary_sha256) {
        $Expected.binary_sha256
    } else {
        $Expected.executable_sha256
    }
    $ExpectedBytes = if ($null -ne $Expected.bytes) {
        $Expected.bytes
    } elseif ($null -ne $Expected.binary_bytes) {
        $Expected.binary_bytes
    } else {
        $Expected.executable_bytes
    }
    if ($Document.executable_sha256 -notmatch '^[0-9a-f]{64}$' -or
        $Document.executable_sha256 -ne $ExpectedSha256 -or
        [int64]$Document.executable_bytes -ne [int64]$ExpectedBytes) {
        throw "$Name executable identity mismatch"
    }
}

function Read-LiveBinaryIdentity([string]$RelativePath, $Expected, [string]$Name) {
    $Path = [IO.Path]::GetFullPath((Join-Path $Root $RelativePath))
    $RootPrefix = $Root.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    if (-not $Path.StartsWith($RootPrefix, [StringComparison]::OrdinalIgnoreCase) -or
        -not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "$Name binary is missing or outside the probe: $Path"
    }
    $Actual = [pscustomobject]@{
        sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
        bytes = (Get-Item -LiteralPath $Path).Length
    }
    if ($Actual.sha256 -ne $Expected.sha256 -or [int64]$Actual.bytes -ne [int64]$Expected.bytes) {
        throw "$Name binary file differs from build evidence"
    }
    $IdentityJson = @(& $Path identity)
    if ($LASTEXITCODE -ne 0) { throw "$Name identity command failed with exit $LASTEXITCODE" }
    $Identity = ($IdentityJson -join "`n") | ConvertFrom-Json
    Assert-Candidate $Identity "$Name live identity"
    Assert-ExecutableIdentity $Identity $Actual "$Name live identity"
    [pscustomobject]@{
        document = $Identity
        source_fingerprint = Get-SourceFingerprint $Identity "$Name live identity"
    }
}

function Assert-CurrentSourceTree($Document) {
    $RootPrefix = $Root.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    foreach ($Source in $Document.source_files) {
        $Path = [IO.Path]::GetFullPath((Join-Path $Root $Source.path))
        if (-not $Path.StartsWith($RootPrefix, [StringComparison]::OrdinalIgnoreCase) -or
            -not (Test-Path -LiteralPath $Path -PathType Leaf)) {
            throw "embedded source path is missing or outside the probe: $($Source.path)"
        }
        $CurrentHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
        if ($CurrentHash -ne $Source.sha256) {
            throw "current source differs from measured executable: $($Source.path)"
        }
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
if ([int]$Admission.rounds -ne 32) { throw "admission must contain exactly 32 rounds" }
Assert-BackendSet $Admission.backends "admission"
foreach ($Backend in $Admission.backends) {
    if ($Backend.status -ne "PASS" -or $null -ne $Backend.error) {
        throw "$($Backend.backend) admission backend failed"
    }
    if ($Backend.contention_rounds.Count -ne 32 -or $Backend.disjoint_rounds.Count -ne 32) {
        throw "$($Backend.backend) admission round count mismatch"
    }
    foreach ($Round in $Backend.contention_rounds) {
        $Admitted = @($Round.child_results | Where-Object { $_.outcome.status -eq "admitted" }).Count
        $Conflicts = @($Round.child_results | Where-Object { $_.outcome.status -eq "conflict" }).Count
        if (-not $Round.conflicting -or $Admitted -ne 1 -or $Conflicts -ne 1) {
            throw "$($Backend.backend) contention round $($Round.round) has wrong truth"
        }
    }
    foreach ($Round in $Backend.disjoint_rounds) {
        $Admitted = @($Round.child_results | Where-Object { $_.outcome.status -eq "admitted" }).Count
        $Conflicts = @($Round.child_results | Where-Object { $_.outcome.status -eq "conflict" }).Count
        if ($Round.conflicting -or $Admitted -ne 2 -or $Conflicts -ne 0) {
            throw "$($Backend.backend) disjoint round $($Round.round) has wrong truth"
        }
    }
}

Assert-BackendSet $Fault.backends "fault matrix"
$InjectedAndDetected = 0
$KernelPrevented = 0
foreach ($Backend in $Fault.backends) {
    if ($Backend.status -ne "PASS") { throw "$($Backend.backend) fault matrix failed" }
    if ($Backend.cases.Count -ne $ExpectedFaultCasesPerBackend) {
        throw "$($Backend.backend) fault count was $($Backend.cases.Count), expected $ExpectedFaultCasesPerBackend"
    }
    $CaseKeys = @($Backend.cases | ForEach-Object { "$($_.domain)|$($_.crash_point)" })
    $Duplicate = @($CaseKeys | Group-Object | Where-Object Count -ne 1)
    $Difference = @(Compare-Object -ReferenceObject $ExpectedFaultCases -DifferenceObject $CaseKeys -CaseSensitive)
    if ($Duplicate.Count -ne 0 -or $Difference.Count -ne 0) {
        throw "$($Backend.backend) fault case identity mismatch"
    }
    foreach ($Case in $Backend.cases) {
        if ($Case.status -ne "PASS" -or $null -ne $Case.error) {
            throw "$($Backend.backend) fault case $($Case.domain)/$($Case.crash_point) failed"
        }
        if ($Case.domain -ne "namespace") { continue }
        $Key = "$($Case.domain)|$($Case.crash_point)"
        switch ($Case.observation.injection_outcome) {
            "injected-and-detected" {
                if ($null -eq $Case.observation.child_result -or
                    -not $Case.observation.child_result.hard_stop -or
                    $Case.observation.child_result.canonical_commit_written -or
                    $Case.observation.canonical_commit_written) {
                    throw "$($Backend.backend) $Key did not hard-stop an injected fault"
                }
                $InjectedAndDetected++
            }
            "kernel-prevented-before-displacement" {
                if ($KernelPreventionEligibleCases -notcontains $Key -or
                    $null -ne $Case.observation.child_result -or
                    [string]::IsNullOrWhiteSpace($Case.observation.injection_error) -or
                    $Case.observation.canonical_commit_written) {
                    throw "$($Backend.backend) $Key has invalid kernel-prevention evidence"
                }
                $KernelPrevented++
            }
            default { throw "$($Backend.backend) $Key has unknown injection outcome" }
        }
    }
}

if ($Build.results.Count -ne 2) { throw "build report must contain two backends" }
Assert-BackendSet $Build.results "build report"
if ($Build.harness_executable.sha256 -notmatch '^[0-9a-f]{64}$' -or
    [int64]$Build.harness_executable.bytes -le 0) {
    throw "build report has no valid all-features harness identity"
}
foreach ($Result in $Build.results) {
    if ($Result.clean_build -is [string] -or $Result.incremental_build -is [string] -or
        $Result.dependency_surface -is [string]) {
        throw "$($Result.backend) build metrics were stringified instead of structured"
    }
    if ($Result.binary_sha256 -notmatch '^[0-9a-f]{64}$' -or [int64]$Result.binary_bytes -le 0) {
        throw "$($Result.backend) build binary identity is invalid"
    }
}

$AdmissionSource = Get-SourceFingerprint $Admission "admission"
Assert-CurrentSourceTree $Admission
foreach ($Pair in @(
    @($Fault, "fault-matrix"),
    @($Redb, "benchmark-redb"),
    @($Sqlite, "benchmark-sqlite")
)) {
    if ((Get-SourceFingerprint $Pair[0] $Pair[1]) -cne $AdmissionSource) {
        throw "$($Pair[1]) embedded source identity differs from admission"
    }
}
Assert-ExecutableIdentity $Admission $Build.harness_executable "admission"
Assert-ExecutableIdentity $Fault $Build.harness_executable "fault-matrix"
$HarnessIdentity = Read-LiveBinaryIdentity $Build.harness_executable.path $Build.harness_executable "all-features harness"
if ($HarnessIdentity.source_fingerprint -cne $AdmissionSource) {
    throw "live all-features harness source identity differs from admission"
}

$CollectorSource = @($Admission.source_files | Where-Object path -eq "scripts/collect-build-metrics.ps1")
if ($CollectorSource.Count -ne 1 -or $Build.collector_sha256 -ne $CollectorSource[0].sha256) {
    throw "build collector identity differs from measured executable source"
}

$BuildByBackend = @{}
foreach ($Result in $Build.results) { $BuildByBackend[$Result.backend] = $Result }
foreach ($Benchmark in @($Redb, $Sqlite)) {
    $BuildResult = $BuildByBackend[$Benchmark.backend]
    Assert-ExecutableIdentity $Benchmark $BuildResult "benchmark-$($Benchmark.backend)"
    $ExpectedBinary = [pscustomobject]@{
        sha256 = $BuildResult.binary_sha256
        bytes = $BuildResult.binary_bytes
    }
    $LiveIdentity = Read-LiveBinaryIdentity $BuildResult.binary $ExpectedBinary "benchmark-$($Benchmark.backend)"
    if ($LiveIdentity.source_fingerprint -cne $AdmissionSource) {
        throw "live benchmark-$($Benchmark.backend) source identity differs from admission"
    }
}

$Summary = [pscustomobject][ordered]@{
    probe_id = "lumin-store-windows-x64-evidence-summary-v1"
    scope = "windows-x64-partial-phase0-evidence"
    status = "PASS"
    architecture_commit = $ArchitectureCommit
    architecture_manifest_sha256 = $ArchitectureManifest
    source_manifest_sha256 = $Admission.source_manifest_sha256
    harness_executable_sha256 = $Build.harness_executable.sha256
    backend_selected = $false
    correctness = [pscustomobject][ordered]@{
        admission_rounds_per_backend = $Admission.rounds
        admission_backends = @($Admission.backends | ForEach-Object { $_.backend })
        fault_cases_per_backend = $ExpectedFaultCasesPerBackend
        total_fault_cases = ($Fault.backends | ForEach-Object { $_.cases.Count } | Measure-Object -Sum).Sum
        namespace_injected_and_detected = $InjectedAndDetected
        namespace_kernel_prevented_before_displacement = $KernelPrevented
    }
    measurements = @(
        foreach ($Benchmark in @($Redb, $Sqlite)) {
            $BuildResult = $BuildByBackend[$Benchmark.backend]
            [pscustomobject][ordered]@{
                backend = $Benchmark.backend
                executable_sha256 = $Benchmark.executable_sha256
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
        "Correctness passed for this Windows x64 harness and exact case set.",
        "Namespace outcomes distinguish injected-and-detected faults from kernel-prevented displacement.",
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
