[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$expectedAboutVersion = 'cargo-about 0.9.1'
$targets = @(
    'x86_64-pc-windows-msvc',
    'aarch64-pc-windows-msvc',
    'x86_64-unknown-linux-gnu',
    'aarch64-unknown-linux-gnu',
    'x86_64-apple-darwin',
    'aarch64-apple-darwin'
)

function Assert-LastExitCode {
    param([Parameter(Mandatory)][string] $Action)

    if ($LASTEXITCODE -ne 0) {
        throw "$Action failed with exit code $LASTEXITCODE."
    }
}

Push-Location $repoRoot
try {
    foreach ($requiredFile in @(
        'LICENSE',
        'THIRD_PARTY_LICENSES.html',
        'NATIVE_DEPENDENCIES.md',
        'licenses/LGPL-2.1.txt'
    )) {
        if (-not (Test-Path -LiteralPath $requiredFile -PathType Leaf)) {
            throw "Required distribution notice is missing: $requiredFile"
        }
    }

    $aboutVersion = & cargo about --version 2>$null
    if ($LASTEXITCODE -ne 0 -or $aboutVersion.Trim() -ne $expectedAboutVersion) {
        throw "Install the pinned reporter with: cargo install cargo-about --version 0.9.1 --locked --features cli"
    }

    & cargo deny --locked check licenses
    Assert-LastExitCode 'cargo-deny license policy'

    $temporaryReport = Join-Path ([IO.Path]::GetTempPath()) ("tz-player-licenses-{0}.html" -f [guid]::NewGuid())
    try {
        & cargo about generate --locked --manifest-path crates/tz-player/Cargo.toml about.hbs --output-file $temporaryReport
        Assert-LastExitCode 'third-party license report generation'

        $expectedHash = (Get-FileHash -Algorithm SHA256 -LiteralPath THIRD_PARTY_LICENSES.html).Hash
        $actualHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $temporaryReport).Hash
        if ($expectedHash -ne $actualHash) {
            throw 'THIRD_PARTY_LICENSES.html is stale. Regenerate it with cargo about generate --locked --manifest-path crates/tz-player/Cargo.toml about.hbs --output-file THIRD_PARTY_LICENSES.html and review the diff.'
        }
    }
    finally {
        if (Test-Path -LiteralPath $temporaryReport) {
            Remove-Item -LiteralPath $temporaryReport -Force
        }
    }

    # Apache-2.0 requires preservation of an upstream NOTICE file when one is
    # supplied. cargo-about harvests license texts, so fail closed if a future
    # release dependency introduces a separate NOTICE that needs manual review.
    $noticeFindings = [System.Collections.Generic.HashSet[string]]::new()
    foreach ($target in $targets) {
        $metadataJson = & cargo metadata --locked --format-version 1 --filter-platform $target
        Assert-LastExitCode "cargo metadata for $target"
        $metadata = $metadataJson | ConvertFrom-Json -Depth 100

        $rootPackage = $metadata.packages | Where-Object { $_.name -eq 'tz-player' }
        $packagesById = @{}
        $nodesById = @{}
        foreach ($package in $metadata.packages) { $packagesById[$package.id] = $package }
        foreach ($node in $metadata.resolve.nodes) { $nodesById[$node.id] = $node }

        $seen = @{}
        $queue = [System.Collections.Generic.Queue[string]]::new()
        $queue.Enqueue($rootPackage.id)
        while ($queue.Count -gt 0) {
            $id = $queue.Dequeue()
            if ($seen.ContainsKey($id)) { continue }
            $seen[$id] = $true

            foreach ($dependency in $nodesById[$id].deps) {
                $isShippedEdge = $dependency.dep_kinds | Where-Object { $_.kind -ne 'dev' }
                if ($isShippedEdge) { $queue.Enqueue($dependency.pkg) }
            }
        }

        foreach ($id in $seen.Keys) {
            $package = $packagesById[$id]
            if (-not $package.source) { continue }

            $packageRoot = Split-Path $package.manifest_path
            Get-ChildItem -LiteralPath $packageRoot -File |
                Where-Object { $_.Name -match '^NOTICE(?:[._-]|$)' } |
                ForEach-Object {
                    [void]$noticeFindings.Add("$($package.name) $($package.version): $($_.Name)")
                }
        }
    }

    if ($noticeFindings.Count -gt 0) {
        $details = ($noticeFindings | Sort-Object) -join [Environment]::NewLine
        throw "Release dependencies contain separate NOTICE files that require manual inclusion review:$([Environment]::NewLine)$details"
    }

    Write-Host 'Distribution license checks passed: policy, report freshness, native notices, source links, and NOTICE-file scan.'
}
finally {
    Pop-Location
}
