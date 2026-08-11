[CmdletBinding()]
param(
    [string] $Target = '',
    [switch] $SkipBuild
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))

function Assert-LastExitCode {
    param([Parameter(Mandatory)][string] $Action)

    if ($LASTEXITCODE -ne 0) {
        throw "$Action failed with exit code $LASTEXITCODE."
    }
}

Push-Location $repoRoot
try {
    & (Join-Path $PSScriptRoot 'check-distribution-licenses.ps1')

    $hasExplicitTarget = -not [string]::IsNullOrWhiteSpace($Target)
    $buildArguments = @('build', '--release', '--locked', '-p', 'tz-player')
    if ($hasExplicitTarget) { $buildArguments += @('--target', $Target) }
    if (-not $SkipBuild) {
        & cargo @buildArguments
        Assert-LastExitCode 'release build'
    }

    if (-not $hasExplicitTarget) {
        $hostLine = & rustc -vV | Where-Object { $_ -like 'host: *' }
        Assert-LastExitCode 'Rust host detection'
        $Target = $hostLine.Substring('host: '.Length)
    }

    $metadataJson = & cargo metadata --locked --format-version 1 --no-deps
    Assert-LastExitCode 'Cargo package metadata'
    $metadata = $metadataJson | ConvertFrom-Json -Depth 100
    $version = ($metadata.packages | Where-Object { $_.name -eq 'tz-player' }).version

    $releaseRoot = if ($hasExplicitTarget) { Join-Path (Join-Path 'target' $Target) 'release' } else { 'target/release' }
    $executableName = if ($Target -match 'windows') { 'tz-player.exe' } else { 'tz-player' }
    $executable = Join-Path $releaseRoot $executableName
    if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
        throw "Release executable not found at $executable. Run without -SkipBuild first."
    }

    $distRoot = [IO.Path]::GetFullPath((Join-Path $repoRoot 'target/dist'))
    [void](New-Item -ItemType Directory -Force -Path $distRoot)
    $baseName = "tz-player-$version-$Target"
    $staging = [IO.Path]::GetFullPath((Join-Path $distRoot ('.stage-' + [guid]::NewGuid())))
    if (-not $staging.StartsWith($distRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Refusing to create a staging directory outside target/dist.'
    }

    [void](New-Item -ItemType Directory -Path $staging)
    try {
        Copy-Item -LiteralPath $executable -Destination $staging
        Copy-Item -LiteralPath LICENSE -Destination $staging
        Copy-Item -LiteralPath THIRD_PARTY_LICENSES.html -Destination $staging
        Copy-Item -LiteralPath NATIVE_DEPENDENCIES.md -Destination $staging
        Copy-Item -LiteralPath README.md -Destination $staging
        $licenseDirectory = Join-Path $staging 'licenses'
        [void](New-Item -ItemType Directory -Path $licenseDirectory)
        Copy-Item -LiteralPath licenses/LGPL-2.1.txt -Destination $licenseDirectory

        if ($Target -match 'windows') {
            $archive = Join-Path $distRoot "$baseName.zip"
            Compress-Archive -Path (Join-Path $staging '*') -DestinationPath $archive -Force
        }
        else {
            $archive = Join-Path $distRoot "$baseName.tar.gz"
            & tar -czf $archive -C $staging .
            Assert-LastExitCode 'release archive creation'
        }
    }
    finally {
        if (Test-Path -LiteralPath $staging) {
            Remove-Item -LiteralPath $staging -Recurse -Force
        }
    }

    Write-Host "Created $archive"
    Write-Host 'Contents: executable, LICENSE, THIRD_PARTY_LICENSES.html, NATIVE_DEPENDENCIES.md, licenses/LGPL-2.1.txt, README.md'
}
finally {
    Pop-Location
}
