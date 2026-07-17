param(
    [string]$TargetTriple = "x86_64-pc-windows-msvc",
    [string]$OutputPath = "evidence/build-surface-windows-x64.json"
)

$ErrorActionPreference = "Stop"
$Root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$Evidence = Join-Path $Root "evidence"
[IO.Directory]::CreateDirectory($Evidence) | Out-Null

function Get-ProbeRelativePath([string]$Path) {
    $Full = [IO.Path]::GetFullPath($Path)
    $Prefix = $Root.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    if (-not $Full.StartsWith($Prefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "path is outside probe root: $Full"
    }
    $Full.Substring($Prefix.Length).Replace("\", "/")
}

function Remove-ProbeTarget([string]$Path) {
    $Full = [IO.Path]::GetFullPath($Path)
    $ExpectedPrefix = $Root.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    if (-not $Full.StartsWith($ExpectedPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "refusing to remove target outside probe root: $Full"
    }
    if (-not ([IO.Path]::GetFileName($Full)).StartsWith("target-measure-", [StringComparison]::Ordinal)) {
        throw "refusing to remove non-measurement target: $Full"
    }
    if (Test-Path -LiteralPath $Full) {
        Remove-Item -LiteralPath $Full -Recurse -Force
    }
}

function Invoke-CargoBuild(
    [string]$Backend,
    [string]$Feature,
    [string]$TargetDirectory,
    [string]$Label
) {
    $Log = Join-Path $Evidence "build-$Backend-$Label.log"
    $Arguments = @(
        "build", "--release", "--locked", "--no-default-features",
        "--features", $Feature, "--target-dir", $TargetDirectory
    )
    $Watch = [Diagnostics.Stopwatch]::StartNew()
    $PreviousErrorAction = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & cargo @Arguments 2>&1 |
        ForEach-Object { $_.ToString() } |
        Tee-Object -FilePath $Log |
        Out-Host
    $ExitCode = $LASTEXITCODE
    $ErrorActionPreference = $PreviousErrorAction
    $Watch.Stop()
    if ($ExitCode -ne 0) {
        throw "cargo build failed for $Backend/$Label with exit $ExitCode"
    }
    [pscustomobject][ordered]@{
        command = "cargo " + ($Arguments -join " ")
        elapsed_millis = [int64]$Watch.Elapsed.TotalMilliseconds
        log = Get-ProbeRelativePath $Log
        log_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $Log).Hash.ToLowerInvariant()
    }
}

function Get-DependencySurface([string]$Backend, [string]$Feature) {
    $TreePath = Join-Path $Evidence "dependency-tree-$Backend.txt"
    $TreeArguments = @(
        "tree", "--locked", "--no-default-features", "--features", $Feature,
        "--target", $TargetTriple, "--prefix", "none"
    )
    $PreviousErrorAction = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $Tree = @(& cargo @TreeArguments 2>&1 | ForEach-Object { $_.ToString() })
    $TreeExitCode = $LASTEXITCODE
    $ErrorActionPreference = $PreviousErrorAction
    if ($TreeExitCode -ne 0) {
        throw "cargo tree failed for $Backend with exit $TreeExitCode"
    }
    [IO.File]::WriteAllLines($TreePath, $Tree, [Text.UTF8Encoding]::new($false))

    $MetadataArguments = @(
        "metadata", "--locked", "--format-version", "1", "--no-default-features",
        "--features", $Feature, "--filter-platform", $TargetTriple
    )
    $MetadataError = Join-Path $Evidence "metadata-$Backend.stderr.log"
    $PreviousErrorAction = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $MetadataJson = (& cargo @MetadataArguments 2> $MetadataError)
    $MetadataExitCode = $LASTEXITCODE
    $ErrorActionPreference = $PreviousErrorAction
    if ($MetadataExitCode -ne 0) {
        throw "cargo metadata failed for $Backend with exit $MetadataExitCode"
    }
    $Metadata = ($MetadataJson | ConvertFrom-Json)
    Remove-Item -LiteralPath $MetadataError -Force
    $SelectedIds = @($Metadata.resolve.nodes | ForEach-Object id)
    $Selected = @($Metadata.packages | Where-Object { $SelectedIds -contains $_.id })
    $DependencyPackages = @($Selected | Where-Object { $_.name -ne "lumin-phase0-store-probe" })

    $UnsafeLines = 0L
    $UnsafePackages = 0
    $NativeFiles = 0
    $NativeBytes = 0L
    foreach ($Package in $DependencyPackages) {
        $PackageRoot = Split-Path -Parent $Package.manifest_path
        $PackageUnsafe = 0L
        foreach ($RustFile in Get-ChildItem -LiteralPath $PackageRoot -Recurse -File -Filter *.rs) {
            $Matches = @(Select-String -LiteralPath $RustFile.FullName -Pattern '\bunsafe\b')
            $PackageUnsafe += $Matches.Count
        }
        if ($PackageUnsafe -gt 0) {
            $UnsafePackages++
            $UnsafeLines += $PackageUnsafe
        }
        foreach ($NativeFile in Get-ChildItem -LiteralPath $PackageRoot -Recurse -File | Where-Object {
            $_.Extension -in @(".c", ".h", ".cc", ".cpp", ".S", ".asm")
        }) {
            $NativeFiles++
            $NativeBytes += $NativeFile.Length
        }
    }

    [pscustomobject][ordered]@{
        selected_package_count_including_probe = $Selected.Count
        transitive_package_count = $DependencyPackages.Count
        rust_unsafe_keyword_line_count = $UnsafeLines
        packages_with_rust_unsafe_keyword = $UnsafePackages
        native_source_file_count = $NativeFiles
        native_source_bytes = $NativeBytes
        dependency_tree = Get-ProbeRelativePath $TreePath
        dependency_tree_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $TreePath).Hash.ToLowerInvariant()
        method = "Selected Cargo resolve nodes; dependency .rs lines matching word unsafe; native .c/.h/.cc/.cpp/.S/.asm files and bytes. This is a reproducible surface metric, not a safety audit."
    }
}

$Results = [Collections.Generic.List[object]]::new()
$HarnessBinary = Join-Path $Root "target/release/lumin-phase0-store-probe.exe"
if (-not (Test-Path -LiteralPath $HarnessBinary -PathType Leaf)) {
    throw "missing all-features release harness binary: $HarnessBinary"
}
$HarnessExecutable = [pscustomobject][ordered]@{
    path = Get-ProbeRelativePath $HarnessBinary
    bytes = (Get-Item -LiteralPath $HarnessBinary).Length
    sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $HarnessBinary).Hash.ToLowerInvariant()
}
foreach ($Spec in @(
    [pscustomobject][ordered]@{ backend = "redb"; feature = "redb-backend" },
    [pscustomobject][ordered]@{ backend = "sqlite"; feature = "sqlite-backend" }
)) {
    $Backend = $Spec.backend
    $Feature = $Spec.feature
    $TargetDirectory = Join-Path $Root "target-measure-$Backend"
    Remove-ProbeTarget $TargetDirectory
    $Clean = Invoke-CargoBuild $Backend $Feature $TargetDirectory "clean"
    $Incremental = Invoke-CargoBuild $Backend $Feature $TargetDirectory "incremental"
    $Binary = Join-Path $TargetDirectory "release/lumin-phase0-store-probe.exe"
    if (-not (Test-Path -LiteralPath $Binary)) {
        throw "missing release binary: $Binary"
    }
    $Results.Add([pscustomobject][ordered]@{
        backend = $Backend
        feature = $Feature
        target_directory = Get-ProbeRelativePath $TargetDirectory
        binary = Get-ProbeRelativePath $Binary
        clean_build = $Clean
        incremental_build = $Incremental
        binary_bytes = (Get-Item -LiteralPath $Binary).Length
        binary_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $Binary).Hash.ToLowerInvariant()
        dependency_surface = Get-DependencySurface $Backend $Feature
    })
}

$Report = [pscustomobject][ordered]@{
    probe_id = "lumin-store-build-surface-v1"
    architecture_commit = "65e60216891bb3d826a4778f84cb8aaa377abe92"
    architecture_manifest_sha256 = "66925583362be22257dd7357072d176cd3f1ae3d4f99874a53f6689b7cf535e0"
    target = $TargetTriple
    host_os = [Environment]::OSVersion.VersionString
    rustc = (& rustc -Vv) -join "`n"
    cargo = (& cargo -V) -join "`n"
    collector_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $PSCommandPath).Hash.ToLowerInvariant()
    harness_executable = $HarnessExecutable
    results = @($Results)
}

$Output = if ([IO.Path]::IsPathRooted($OutputPath)) {
    [IO.Path]::GetFullPath($OutputPath)
} else {
    [IO.Path]::GetFullPath((Join-Path $Root $OutputPath))
}
[IO.Directory]::CreateDirectory((Split-Path -Parent $Output)) | Out-Null
[IO.File]::WriteAllText(
    $Output,
    ($Report | ConvertTo-Json -Depth 20) + "`n",
    [Text.UTF8Encoding]::new($false)
)
Write-Output $Output
