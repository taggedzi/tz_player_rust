[CmdletBinding()]
param(
    [string] $Target = '',
    [switch] $SkipBuild,
    [switch] $SkipLicenseCheck
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

function ManifestValue {
    param([Parameter(Mandatory)][string] $Manifest, [Parameter(Mandatory)][string] $Name)
    $match = [regex]::Match($Manifest, '(?m)^' + [regex]::Escape($Name) + '\s*=\s*"([^"]+)"')
    if (-not $match.Success) { throw "FFmpeg manifest value is missing: $Name" }
    $match.Groups[1].Value
}

function ManifestArray {
    param([Parameter(Mandatory)][string] $Manifest, [Parameter(Mandatory)][string] $Name)
    $match = [regex]::Match($Manifest, '(?ms)^' + [regex]::Escape($Name) + '\s*=\s*\[(.*?)\]')
    if (-not $match.Success) { throw "FFmpeg manifest array is missing: $Name" }
    @([regex]::Matches($match.Groups[1].Value, '"([^"]+)"') | ForEach-Object { $_.Groups[1].Value })
}

function ManifestMajor {
    param([Parameter(Mandatory)][string] $Manifest, [Parameter(Mandatory)][string] $Library)
    $match = [regex]::Match($Manifest, '(?m)^library_majors\s*=\s*\{[^}]*\b' + [regex]::Escape($Library) + '\s*=\s*(\d+)')
    if (-not $match.Success) { throw "FFmpeg manifest library major is missing: $Library" }
    $match.Groups[1].Value
}

function Write-Sha256File {
    param([Parameter(Mandatory)][string] $Path)
    $item = Get-Item -LiteralPath $Path
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $item.FullName).Hash.ToLowerInvariant()
    $contents = "$hash  $($item.Name)`n"
    [IO.File]::WriteAllText(
        $item.FullName + '.sha256',
        $contents,
        [Text.UTF8Encoding]::new($false)
    )
}

$originalRustFlags = [Environment]::GetEnvironmentVariable('RUSTFLAGS', 'Process')
Push-Location $repoRoot
try {
    if (-not $SkipLicenseCheck) {
        & (Join-Path $PSScriptRoot 'check-distribution-licenses.ps1')
    }

    $hasExplicitTarget = -not [string]::IsNullOrWhiteSpace($Target)
    if (-not $hasExplicitTarget) {
        $hostLine = & rustc -vV | Where-Object { $_ -like 'host: *' }
        Assert-LastExitCode 'Rust host detection'
        $Target = $hostLine.Substring('host: '.Length)
    }
    if ($Target -match 'windows' -and $originalRustFlags -notmatch 'target-feature=\+crt-static') {
        $env:RUSTFLAGS = (($originalRustFlags + ' -C target-feature=+crt-static').Trim())
    }

    $defaultFfmpegPrefix = Join-Path $repoRoot 'native/ffmpeg/build/sdk'
    if ([string]::IsNullOrWhiteSpace($env:TZ_FFMPEG_PREFIX) -and (Test-Path -LiteralPath $defaultFfmpegPrefix -PathType Container)) {
        $env:TZ_FFMPEG_PREFIX = $defaultFfmpegPrefix
    }
    if ([string]::IsNullOrWhiteSpace($env:TZ_FFMPEG_PREFIX)) {
        throw 'TZ_FFMPEG_PREFIX must point to the audited FFmpeg install prefix.'
    }
    if ([string]::IsNullOrWhiteSpace($env:TZ_FFMPEG_LIB_DIR)) {
        $libraryCandidate = if ($Target -match 'windows') { Join-Path $env:TZ_FFMPEG_PREFIX 'bin' } else { Join-Path $env:TZ_FFMPEG_PREFIX 'lib' }
        if (Test-Path -LiteralPath $libraryCandidate -PathType Container) { $env:TZ_FFMPEG_LIB_DIR = $libraryCandidate }
    }
    if (-not (Test-Path -LiteralPath $env:TZ_FFMPEG_LIB_DIR -PathType Container)) {
        throw "TZ_FFMPEG_LIB_DIR does not exist: $env:TZ_FFMPEG_LIB_DIR"
    }
    $env:FFMPEG_DIR = $env:TZ_FFMPEG_PREFIX
    $pkgConfigDirectory = Join-Path $env:TZ_FFMPEG_PREFIX 'lib/pkgconfig'
    if (Test-Path -LiteralPath $pkgConfigDirectory -PathType Container) { $env:PKG_CONFIG_PATH = $pkgConfigDirectory }

    $buildArguments = @('build', '--release', '--locked', '-p', 'tz-player')
    if ($hasExplicitTarget) { $buildArguments += @('--target', $Target) }
    if (-not $SkipBuild) {
        & cargo @buildArguments
        Assert-LastExitCode 'release build'
        $helperArguments = @('build', '--release', '--locked', '-p', 'tz-audio-decoder', '--features', 'ffmpeg-native')
        if ($hasExplicitTarget) { $helperArguments += @('--target', $Target) }
        & cargo @helperArguments
        Assert-LastExitCode 'bundled audio helper build'
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
    $helperName = if ($Target -match 'windows') { 'tz-audio-decoder.exe' } else { 'tz-audio-decoder' }
    $helperReleaseRoot = if ($hasExplicitTarget) { Join-Path (Join-Path 'target' $Target) 'release' } else { 'target/release' }
    $helperExecutable = Join-Path $helperReleaseRoot $helperName
    if (-not (Test-Path -LiteralPath $helperExecutable -PathType Leaf)) {
        throw "Bundled audio helper not found at $helperExecutable. Build with --features ffmpeg-native."
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
        $packageRoot = $staging
        $binaryDirectory = $staging
        if ($Target -match 'darwin|apple') {
            $contentsDirectory = Join-Path $staging 'tz-player.app/Contents'
            $binaryDirectory = Join-Path $contentsDirectory 'MacOS'
            $packageRoot = Join-Path $contentsDirectory 'Resources'
            [void](New-Item -ItemType Directory -Path $binaryDirectory -Force)
            [void](New-Item -ItemType Directory -Path $packageRoot -Force)
        }
        Copy-Item -LiteralPath $executable -Destination $binaryDirectory
        $audioDirectory = Join-Path $packageRoot 'audio'
        [void](New-Item -ItemType Directory -Path $audioDirectory)
        Copy-Item -LiteralPath $helperExecutable -Destination $audioDirectory
        $manifestPath = Join-Path $repoRoot 'native/ffmpeg/manifest.toml'
        $manifestText = Get-Content -Raw -LiteralPath $manifestPath
        $libraryPatterns = foreach ($library in 'avcodec', 'avformat', 'avutil', 'swresample') {
            $major = ManifestMajor -Manifest $manifestText -Library $library
            if ($Target -match 'windows') { "$library-$major.dll" }
            elseif ($Target -match 'darwin|apple') { "lib$library.$major.dylib" }
            else { "lib$library.so.$major" }
        }
        foreach ($pattern in $libraryPatterns) {
            $matches = @(Get-ChildItem -LiteralPath $env:TZ_FFMPEG_LIB_DIR -Filter $pattern -File)
            if ($matches.Count -ne 1) { throw "Expected exactly one FFmpeg library in TZ_FFMPEG_LIB_DIR for $pattern; found $($matches.Count)." }
            Copy-Item -LiteralPath $matches[0].FullName -Destination $audioDirectory
        }
        $ffmpegBuildMetadata = Join-Path $repoRoot 'native/ffmpeg/build/FFMPEG_BUILD.json'
        $ffmpegChanges = Join-Path $repoRoot 'native/ffmpeg/build/FFMPEG_CHANGES.diff'
        $ffmpegComponents = Join-Path $repoRoot 'native/ffmpeg/build/FFMPEG_COMPONENTS.json'
        $ffmpegConfigureLog = Join-Path $repoRoot 'native/ffmpeg/build/FFMPEG_CONFIGURE.log'
        if (-not (Test-Path -LiteralPath $ffmpegBuildMetadata) -or -not (Test-Path -LiteralPath $ffmpegChanges) -or -not (Test-Path -LiteralPath $ffmpegComponents) -or -not (Test-Path -LiteralPath $ffmpegConfigureLog)) { throw 'FFMPEG_BUILD.json, FFMPEG_COMPONENTS.json, FFMPEG_CONFIGURE.log, and FFMPEG_CHANGES.diff are required before packaging.' }
        Copy-Item -LiteralPath $ffmpegBuildMetadata -Destination $audioDirectory
        Copy-Item -LiteralPath $ffmpegChanges -Destination $audioDirectory
        Copy-Item -LiteralPath $ffmpegComponents -Destination $audioDirectory
        Copy-Item -LiteralPath $ffmpegConfigureLog -Destination $audioDirectory
        Copy-Item -LiteralPath LICENSE -Destination $packageRoot
        Copy-Item -LiteralPath THIRD_PARTY_LICENSES.html -Destination $packageRoot
        Copy-Item -LiteralPath FFMPEG_SOURCE.md -Destination $packageRoot
        Copy-Item -LiteralPath NATIVE_DEPENDENCIES.md -Destination $packageRoot
        Copy-Item -LiteralPath README.md -Destination $packageRoot
        $licenseDirectory = Join-Path $packageRoot 'licenses'
        [void](New-Item -ItemType Directory -Path $licenseDirectory)
        Copy-Item -LiteralPath licenses/LGPL-2.1.txt -Destination (Join-Path $licenseDirectory 'LGPL-2.1-or-later.txt')

        & (Join-Path $PSScriptRoot 'inspect-native-dependencies.ps1') -Directory $audioDirectory

        $expectedVersion = ManifestValue -Manifest $manifestText -Name 'version'
        $expectedCommit = ManifestValue -Manifest $manifestText -Name 'ffmpeg_release_commit'
        $expectedSourceSha = ManifestValue -Manifest $manifestText -Name 'source_sha256'
        $expectedPatch = ManifestValue -Manifest $manifestText -Name 'patch'
        $expectedPatchSha = ManifestValue -Manifest $manifestText -Name 'patch_sha256'
        $expectedDemuxers = @(ManifestArray -Manifest $manifestText -Name 'demuxers' | Sort-Object)
        $expectedDecoders = @(ManifestArray -Manifest $manifestText -Name 'decoders' | Sort-Object)
        $buildIdentity = Get-Content -Raw -LiteralPath $ffmpegBuildMetadata | ConvertFrom-Json
        $builtComponents = Get-Content -Raw -LiteralPath $ffmpegComponents | ConvertFrom-Json
        $configureLog = Get-Content -Raw -LiteralPath $ffmpegConfigureLog
        $packagedPatchSha = (Get-FileHash -Algorithm SHA256 -LiteralPath $ffmpegChanges).Hash.ToLowerInvariant()
        if ($buildIdentity.version -ne $expectedVersion -or
            $buildIdentity.source_sha256 -ne $expectedSourceSha -or
            $buildIdentity.patch -ne $expectedPatch -or
            $buildIdentity.patch_sha256 -ne $expectedPatchSha -or
            $packagedPatchSha -ne $expectedPatchSha) {
            throw 'FFMPEG_BUILD.json does not match native/ffmpeg/manifest.toml.'
        }
        if (@(Compare-Object $expectedDemuxers @($builtComponents.demuxers | Sort-Object)).Count -ne 0 -or
            @(Compare-Object $expectedDecoders @($builtComponents.decoders | Sort-Object)).Count -ne 0) {
            throw 'FFMPEG_COMPONENTS.json does not match native/ffmpeg/manifest.toml.'
        }
        if ($configureLog -match '--enable-gpl|--enable-nonfree|--enable-network|--enable-protocols') {
            throw 'FFMPEG_CONFIGURE.log contains a forbidden packaged feature.'
        }
        $capabilitiesJson = & (Join-Path $audioDirectory $helperName) capabilities --json
        Assert-LastExitCode 'staged helper capability check'
        $capabilities = $capabilitiesJson | ConvertFrom-Json
        if ($capabilities.ffmpeg_version -ne $expectedVersion -or $capabilities.ffmpeg_commit -ne $expectedCommit -or
            [string]::IsNullOrWhiteSpace($capabilities.configuration_hash)) {
            throw 'Staged helper FFmpeg identity does not match native/ffmpeg/manifest.toml.'
        }
        if (@(Compare-Object $expectedDemuxers @($capabilities.demuxers | Sort-Object)).Count -ne 0 -or
            @(Compare-Object $expectedDecoders @($capabilities.decoders | Sort-Object)).Count -ne 0) {
            throw 'Staged helper component capabilities do not match native/ffmpeg/manifest.toml.'
        }

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

    $sourceArchive = Join-Path $repoRoot "native/ffmpeg/build/ffmpeg-$expectedVersion.tar.xz"
    if (-not (Test-Path -LiteralPath $sourceArchive -PathType Leaf)) { throw "Matching FFmpeg source archive is missing: $sourceArchive" }
    $actualSourceSha = (Get-FileHash -Algorithm SHA256 -LiteralPath $sourceArchive).Hash.ToLowerInvariant()
    if ($actualSourceSha -ne $expectedSourceSha) { throw "Matching FFmpeg source archive hash is $actualSourceSha; expected $expectedSourceSha." }
    $sourceAsset = Join-Path $distRoot (Split-Path -Leaf $sourceArchive)
    Copy-Item -LiteralPath $sourceArchive -Destination $sourceAsset -Force
    $patchAsset = Join-Path $distRoot "ffmpeg-$expectedVersion-tz-player.patch"
    Copy-Item -LiteralPath $ffmpegChanges -Destination $patchAsset -Force
    Write-Sha256File -Path $archive
    Write-Sha256File -Path $sourceAsset
    Write-Sha256File -Path $patchAsset

    Write-Host "Created $archive"
    Write-Host "Prepared matching source asset $sourceAsset"
    Write-Host "Prepared matching source patch $patchAsset"
    Write-Host 'Contents: player, audio helper/libraries/audit metadata, notices, source offer, and README'
}
finally {
    [Environment]::SetEnvironmentVariable('RUSTFLAGS', $originalRustFlags, 'Process')
    Pop-Location
}
