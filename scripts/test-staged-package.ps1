[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string] $Archive
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$targetRoot = [IO.Path]::GetFullPath((Join-Path $repoRoot 'target'))
$archivePath = (Resolve-Path -LiteralPath $Archive).Path
$smokeRoot = [IO.Path]::GetFullPath((Join-Path $targetRoot ("package smoke Ω " + [guid]::NewGuid())))
if (-not $smokeRoot.StartsWith($targetRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'Refusing to create package smoke files outside target.'
}

function Assert-ExitCode {
    param([Parameter(Mandatory)][int] $Expected, [Parameter(Mandatory)][string] $Action)
    if ($LASTEXITCODE -ne $Expected) {
        throw "$Action returned $LASTEXITCODE; expected $Expected."
    }
}

function Invoke-Doctor {
    param([Parameter(Mandatory)][string] $Player, [switch] $ExpectFailure)
    $output = (& $Player --backend fake doctor 2>&1 | Out-String)
    if ($ExpectFailure) {
        if ($LASTEXITCODE -eq 0) { throw "Tampered package unexpectedly passed doctor:`n$output" }
    }
    elseif ($LASTEXITCODE -ne 0) {
        throw "Staged package doctor failed:`n$output"
    }
    $output
}

[void](New-Item -ItemType Directory -Path $smokeRoot)
try {
    if ($archivePath.EndsWith('.zip', [StringComparison]::OrdinalIgnoreCase)) {
        Expand-Archive -LiteralPath $archivePath -DestinationPath $smokeRoot
    }
    else {
        & tar -xzf $archivePath -C $smokeRoot
        Assert-ExitCode -Expected 0 -Action 'archive extraction'
    }

    $playerName = if ($IsWindows) { 'tz-player.exe' } else { 'tz-player' }
    $helperName = if ($IsWindows) { 'tz-audio-decoder.exe' } else { 'tz-audio-decoder' }
    $players = @(Get-ChildItem -LiteralPath $smokeRoot -Recurse -File -Filter $playerName)
    if ($players.Count -ne 1) { throw "Expected exactly one staged $playerName; found $($players.Count)." }
    $player = $players[0].FullName
    $packageRoot = if ($IsMacOS -and (Split-Path -Leaf (Split-Path -Parent $player)) -eq 'MacOS') {
        Join-Path (Split-Path -Parent (Split-Path -Parent $player)) 'Resources'
    }
    else {
        Split-Path -Parent $player
    }
    $audioDirectory = Join-Path $packageRoot 'audio'
    $helper = Join-Path $audioDirectory $helperName
    if (-not (Test-Path -LiteralPath $helper -PathType Leaf)) { throw "Staged helper is missing: $helper" }

    $fakePath = Join-Path $smokeRoot 'fake PATH'
    [void](New-Item -ItemType Directory -Path $fakePath)
    $marker = Join-Path $smokeRoot 'external-tool-was-run.txt'
    if ($IsWindows) {
        foreach ($name in 'ffmpeg.cmd', 'vlc.cmd') {
            Set-Content -Encoding ASCII -LiteralPath (Join-Path $fakePath $name) -Value "@echo off`r`necho called>`"$marker`"`r`nexit /b 99"
        }
    }
    else {
        foreach ($name in 'ffmpeg', 'vlc') {
            $path = Join-Path $fakePath $name
            Set-Content -Encoding UTF8 -LiteralPath $path -Value "#!/bin/sh`nprintf called > '$marker'`nexit 99"
            & chmod +x $path
            Assert-ExitCode -Expected 0 -Action "chmod $name"
        }
    }
    # Package validation must not inherit build SDK paths or helper overrides.
    # Otherwise a removed staged library can be silently loaded from the SDK.
    $originalRuntimeEnvironment = @{
        PATH = [Environment]::GetEnvironmentVariable('PATH', 'Process')
        LD_LIBRARY_PATH = [Environment]::GetEnvironmentVariable('LD_LIBRARY_PATH', 'Process')
        LD_PRELOAD = [Environment]::GetEnvironmentVariable('LD_PRELOAD', 'Process')
        DYLD_LIBRARY_PATH = [Environment]::GetEnvironmentVariable('DYLD_LIBRARY_PATH', 'Process')
        DYLD_FALLBACK_LIBRARY_PATH = [Environment]::GetEnvironmentVariable('DYLD_FALLBACK_LIBRARY_PATH', 'Process')
        DYLD_INSERT_LIBRARIES = [Environment]::GetEnvironmentVariable('DYLD_INSERT_LIBRARIES', 'Process')
        TZ_PLAYER_AUDIO_HELPER = [Environment]::GetEnvironmentVariable('TZ_PLAYER_AUDIO_HELPER', 'Process')
        TZ_FFMPEG_PREFIX = [Environment]::GetEnvironmentVariable('TZ_FFMPEG_PREFIX', 'Process')
        TZ_FFMPEG_INCLUDE_DIR = [Environment]::GetEnvironmentVariable('TZ_FFMPEG_INCLUDE_DIR', 'Process')
        TZ_FFMPEG_LIB_DIR = [Environment]::GetEnvironmentVariable('TZ_FFMPEG_LIB_DIR', 'Process')
        TZ_FFMPEG_RUNTIME_DIR = [Environment]::GetEnvironmentVariable('TZ_FFMPEG_RUNTIME_DIR', 'Process')
        FFMPEG_DIR = [Environment]::GetEnvironmentVariable('FFMPEG_DIR', 'Process')
        PKG_CONFIG_PATH = [Environment]::GetEnvironmentVariable('PKG_CONFIG_PATH', 'Process')
    }
    $env:PATH = $fakePath
    foreach ($name in $originalRuntimeEnvironment.Keys | Where-Object { $_ -ne 'PATH' }) {
        [Environment]::SetEnvironmentVariable($name, $null, 'Process')
    }
    try {
        $capabilities = (& $helper capabilities --json 2>&1 | Out-String)
        if ($LASTEXITCODE -ne 0) { throw "Staged helper capability check failed:`n$capabilities" }
        $parsed = $capabilities | ConvertFrom-Json
        if ($parsed.ffmpeg_version -ne '7.1.5' -or [string]::IsNullOrWhiteSpace($parsed.configuration_hash)) {
            throw 'Staged helper reported an unexpected FFmpeg identity.'
        }
        [void](Invoke-Doctor -Player $player)
        if (Test-Path -LiteralPath $marker) { throw 'The staged package executed ffmpeg or VLC from PATH.' }

        $fixtureRoot = Join-Path $repoRoot 'crates/tz-playback/tests/fixtures'
        $nativeFixtures = @(
            'tone.wav', 'tone.mp3', 'tone.flac', 'tone.ogg', 'tone-aac.m4a',
            'tone-alac.m4a', 'tone.aiff', 'tone.caf', 'tone.mka'
        ) | ForEach-Object { Join-Path $fixtureRoot $_ }
        $helperFixtures = @(
            'tone-opus.ogg', 'tone-wma.wma', 'tone-wavpack.wv', 'tone-ac3.ac3',
            'tone-eac3.eac3', 'tone-dts.dts', 'tone-tta.tta', 'tone-speex.ogg',
            'tone-ape.ape', 'tone-musepack7.mpc', 'tone-musepack8.mpc'
        ) | ForEach-Object { Join-Path $fixtureRoot $_ }
        $smokeDatabase = Join-Path $smokeRoot 'package-smoke.db'
        $smokeArguments = @('--backend', 'fake', 'package-smoke', '--database', $smokeDatabase)
        foreach ($fixture in $nativeFixtures) { $smokeArguments += @('--native', $fixture) }
        foreach ($fixture in $helperFixtures) { $smokeArguments += @('--helper', $fixture) }
        $playbackSmoke = (& $player @smokeArguments 2>&1 | Out-String)
        if ($LASTEXITCODE -ne 0) { throw "Staged package playback/analysis smoke failed:`n$playbackSmoke" }
        if ($playbackSmoke -notmatch 'Package smoke PASS') { throw "Staged package smoke did not report success:`n$playbackSmoke" }
        if (Test-Path -LiteralPath $marker) { throw 'The staged playback/analysis smoke executed ffmpeg or VLC from PATH.' }

        $corruptFixture = Join-Path $smokeRoot 'corrupt-media.bin'
        Set-Content -Encoding ASCII -LiteralPath $corruptFixture -Value 'not audio data'
        $bothFailed = (& $player --backend fake package-smoke --database (Join-Path $smokeRoot 'both-failed.db') --native $corruptFixture 2>&1 | Out-String)
        if ($LASTEXITCODE -eq 0 -or
            $bothFailed -notmatch 'native decoder rejected' -or
            $bothFailed -notmatch 'bundled helper failed') {
            throw "Corrupt-media error did not retain both decoder contexts:`n$bothFailed"
        }

        $removedHelper = $helper + '.removed'
        Move-Item -LiteralPath $helper -Destination $removedHelper
        try {
            [void](Invoke-Doctor -Player $player -ExpectFailure)
            $nativeOnlyDatabase = Join-Path $smokeRoot 'native-without-helper.db'
            $nativeOnlySmoke = (& $player --backend fake package-smoke --database $nativeOnlyDatabase --native $nativeFixtures[0] 2>&1 | Out-String)
            if ($LASTEXITCODE -ne 0 -or $nativeOnlySmoke -notmatch 'Package smoke PASS') {
                throw "Native playback/analysis failed when the helper was absent:`n$nativeOnlySmoke"
            }
        }
        finally {
            Move-Item -LiteralPath $removedHelper -Destination $helper
        }
        [void](Invoke-Doctor -Player $player)

        $libraries = @(Get-ChildItem -LiteralPath $audioDirectory -File | Where-Object {
            $_.Name -match '^(avcodec|avformat|avutil|swresample)-\d+\.dll$' -or
            $_.Name -match '^lib(avcodec|avformat|avutil|swresample)(\.so\.\d+|\.\d+\.dylib)$'
        })
        if ($libraries.Count -ne 4) { throw "Expected four staged FFmpeg libraries; found $($libraries.Count)." }
        foreach ($library in $libraries) {
            $removed = $library.FullName + '.removed'
            Move-Item -LiteralPath $library.FullName -Destination $removed
            try {
                & $helper capabilities --json *> $null
                if ($LASTEXITCODE -eq 0) { throw "Helper unexpectedly started without $($library.Name)." }
            }
            finally {
                Move-Item -LiteralPath $removed -Destination $library.FullName
            }
        }

        $metadata = @(
            (Join-Path $packageRoot 'FFMPEG_SOURCE.md'),
            (Join-Path $packageRoot 'NATIVE_DEPENDENCIES.md'),
            (Join-Path $packageRoot 'licenses/LGPL-2.1-or-later.txt'),
            (Join-Path $audioDirectory 'FFMPEG_BUILD.json'),
            (Join-Path $audioDirectory 'FFMPEG_COMPONENTS.json'),
            (Join-Path $audioDirectory 'FFMPEG_CONFIGURE.log'),
            (Join-Path $audioDirectory 'FFMPEG_CHANGES.diff')
        )
        foreach ($path in $metadata) {
            $removed = $path + '.removed'
            Move-Item -LiteralPath $path -Destination $removed
            try { [void](Invoke-Doctor -Player $player -ExpectFailure) }
            finally { Move-Item -LiteralPath $removed -Destination $path }
        }
        [void](Invoke-Doctor -Player $player)
    }
    finally {
        foreach ($name in $originalRuntimeEnvironment.Keys) {
            [Environment]::SetEnvironmentVariable($name, $originalRuntimeEnvironment[$name], 'Process')
        }
    }

    Write-Host "Staged package smoke passed: $archivePath"
}
finally {
    if (Test-Path -LiteralPath $smokeRoot) {
        Remove-Item -LiteralPath $smokeRoot -Recurse -Force
    }
}
