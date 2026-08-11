param(
    [string]$SourceArchive = "",
    [string]$OutputDirectory = "build",
    [string]$Prefix = ""
)
$ErrorActionPreference = "Stop"

$manifestPath = Join-Path $PSScriptRoot "manifest.toml"
$manifest = Get-Content -Raw -LiteralPath $manifestPath
function ManifestValue([string]$Name) {
    $pattern = '(?m)^' + [regex]::Escape($Name) + '\s*=\s*"([^"]+)"'
    $match = [regex]::Match($manifest, $pattern)
    if (-not $match.Success) { throw "Manifest value '$Name' is missing" }
    return $match.Groups[1].Value
}
function ManifestArray([string]$Name) {
    $pattern = '(?ms)^' + [regex]::Escape($Name) + '\s*=\s*\[(.*?)\]'
    $match = [regex]::Match($manifest, $pattern)
    if (-not $match.Success) { throw "Manifest array '$Name' is missing" }
    return @([regex]::Matches($match.Groups[1].Value, '"([^"]+)"') | ForEach-Object { $_.Groups[1].Value })
}

$version = ManifestValue "version"
$url = ManifestValue "source_url"
$expectedSha = (ManifestValue "source_sha256").ToLowerInvariant()
$patchRelativePath = ManifestValue "patch"
$expectedPatchSha = (ManifestValue "patch_sha256").ToLowerInvariant()
$manifestConfigure = ManifestArray "configure"
$allowedDemuxers = ManifestArray "demuxers"
$allowedParsers = ManifestArray "parsers"
$allowedDecoders = ManifestArray "decoders"
$allowedBsfs = ManifestArray "bitstream_filters"
$root = (Resolve-Path -LiteralPath $PSScriptRoot).Path
$out = [IO.Path]::GetFullPath((Join-Path $root $OutputDirectory))
New-Item -ItemType Directory -Force -Path $out | Out-Null

if ([string]::IsNullOrWhiteSpace($SourceArchive)) {
    $SourceArchive = Join-Path $out "ffmpeg-$version.tar.xz"
    if (-not (Test-Path -LiteralPath $SourceArchive)) {
        Invoke-WebRequest -Uri $url -OutFile $SourceArchive
    }
}
$SourceArchive = (Resolve-Path -LiteralPath $SourceArchive).Path
$actualSha = (Get-FileHash -Algorithm SHA256 -LiteralPath $SourceArchive).Hash.ToLowerInvariant()
if ($actualSha -ne $expectedSha) { throw "FFmpeg source SHA-256 mismatch: expected $expectedSha, got $actualSha" }

$nasmPath = (Get-Command nasm -ErrorAction SilentlyContinue).Source
$nasmCandidates = @(
    'C:\Program Files\NASM\nasm.exe',
    (Join-Path $env:LOCALAPPDATA 'bin\NASM\nasm.exe')
)
if (-not $nasmPath) {
    $nasmPath = $nasmCandidates | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
}
if (-not $nasmPath) { throw 'Required FFmpeg build tool is missing: nasm' }
$nasmDirectory = Split-Path -Parent $nasmPath
$env:PATH = "$nasmDirectory;$env:PATH"
$bashPath = if (Test-Path -LiteralPath 'C:\msys64\usr\bin\bash.exe') { 'C:\msys64\usr\bin\bash.exe' } else { (Get-Command bash -ErrorAction SilentlyContinue).Source }
if (-not $bashPath) { throw 'Required FFmpeg build tool is missing: MSYS2 bash' }
$makePath = if (Test-Path -LiteralPath 'C:\msys64\usr\bin\make.exe') { 'C:\msys64\usr\bin\make.exe' } else { (Get-Command make -ErrorAction SilentlyContinue).Source }
if (-not $makePath) { throw 'Required FFmpeg build tool is missing: make' }
if (-not (Get-Command tar -ErrorAction SilentlyContinue)) { throw 'Required FFmpeg build tool is missing: tar' }
$vswhere = 'C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe'
$clPath = $null
if (Test-Path -LiteralPath $vswhere) {
    $vsPath = (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath | Select-Object -First 1)
    $vsDevCmd = if ($vsPath) { Join-Path $vsPath 'Common7\Tools\VsDevCmd.bat' }
    if ($vsDevCmd -and (Test-Path -LiteralPath $vsDevCmd)) {
        $environmentDump = & cmd.exe /d /s /c "call `"$vsDevCmd`" -arch=x64 >nul && set"
        foreach ($line in $environmentDump) {
            $separator = $line.IndexOf('=')
            if ($separator -gt 0) { [Environment]::SetEnvironmentVariable($line.Substring(0, $separator), $line.Substring($separator + 1), 'Process') }
        }
    }
}
if ($vsPath) {
    $clPath = Get-ChildItem -LiteralPath (Join-Path $vsPath 'VC\Tools\MSVC') -Filter 'cl.exe' -Recurse -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -match '\\bin\\Hostx64\\x64\\cl\.exe$' } |
        Select-Object -First 1 -ExpandProperty FullName
}
if (-not $clPath) { throw 'Required FFmpeg build tool is missing: MSVC cl.exe' }
if (-not $Prefix) { $Prefix = Join-Path $out "sdk" }
$prefixFull = [IO.Path]::GetFullPath($Prefix)
$sourceDir = Join-Path $out "src\ffmpeg-$version"
if (-not (Test-Path -LiteralPath $sourceDir)) {
    New-Item -ItemType Directory -Force -Path (Split-Path $sourceDir) | Out-Null
    & tar -xf $SourceArchive -C (Split-Path $sourceDir)
}
if (-not (Test-Path -LiteralPath (Join-Path $sourceDir "configure"))) { throw "FFmpeg source extraction did not produce configure" }
$patchPath = Join-Path $root ($patchRelativePath -replace '/', '\')
if (-not (Test-Path -LiteralPath $patchPath -PathType Leaf)) { throw "Required FFmpeg patch is missing: $patchPath" }
$actualPatchSha = (Get-FileHash -Algorithm SHA256 -LiteralPath $patchPath).Hash.ToLowerInvariant()
if ($actualPatchSha -ne $expectedPatchSha) { throw "FFmpeg patch SHA-256 mismatch: expected $expectedPatchSha, got $actualPatchSha" }
$originalGitCeiling = $env:GIT_CEILING_DIRECTORIES
$env:GIT_CEILING_DIRECTORIES = Split-Path -Parent $sourceDir
Push-Location $sourceDir
try {
    & git apply --reverse --check $patchPath 2>$null
    if ($LASTEXITCODE -ne 0) {
        & git apply --check $patchPath
        if ($LASTEXITCODE -ne 0) { throw 'FFmpeg source does not match the audited Speex patch.' }
        & git apply $patchPath
        if ($LASTEXITCODE -ne 0) { throw 'Could not apply the audited Speex patch.' }
    }
}
finally {
    Pop-Location
    $env:GIT_CEILING_DIRECTORIES = $originalGitCeiling
}

$configure = @("--prefix=$prefixFull", "--target-os=win32", "--arch=x86_64", "--toolchain=msvc") +
    $manifestConfigure + @(
        "--enable-demuxer=$($allowedDemuxers -join ',')",
        "--enable-parser=$($allowedParsers -join ',')",
        "--enable-decoder=$($allowedDecoders -join ',')",
        "--enable-bsf=$($allowedBsfs -join ',')"
    )
$configureText = ($configure -join " ")
if ($configureText -match "--enable-gpl|--enable-nonfree") { throw "Refusing GPL/nonfree FFmpeg configuration" }
$bashSource = $sourceDir.Replace('\', '/')
$clDirectoryMsys = (Split-Path -Parent $clPath) -replace '\\', '/'
if ($clDirectoryMsys -match '^([A-Za-z]):/(.*)$') { $clDirectoryMsys = "/$($Matches[1].ToLower())/$($Matches[2])" }
$nasmDirectoryMsys = $nasmDirectory -replace '\\', '/'
if ($nasmDirectoryMsys -match '^([A-Za-z]):/(.*)$') { $nasmDirectoryMsys = "/$($Matches[1].ToLower())/$($Matches[2])" }
$configure += '--cc=cl.exe'
$bashConfigure = ($configure | ForEach-Object { "'" + $_ + "'" }) -join " "
& $bashPath -lc "export PATH='${clDirectoryMsys}:${nasmDirectoryMsys}:/c/msys64/usr/bin:/usr/bin:`$PATH' && cd '$bashSource' && ./configure $bashConfigure"
if ($LASTEXITCODE -ne 0) { throw "FFmpeg configure/build/install failed with exit code $LASTEXITCODE" }

$componentHeader = Join-Path $sourceDir 'config_components.h'
if (-not (Test-Path -LiteralPath $componentHeader)) { throw 'FFmpeg did not emit config_components.h' }
$componentText = Get-Content -Raw -LiteralPath $componentHeader
function EnabledComponents([string]$Suffix) {
    $pattern = '(?m)^#define CONFIG_([A-Z0-9_]+)_' + [regex]::Escape($Suffix) + ' 1$'
    return @([regex]::Matches($componentText, $pattern) | ForEach-Object { $_.Groups[1].Value.ToLowerInvariant() } | Sort-Object)
}
function Assert-ExactComponents([string]$Kind, [string[]]$Expected, [string[]]$Actual) {
    $difference = @(Compare-Object ($Expected | Sort-Object) ($Actual | Sort-Object))
    if ($difference.Count -ne 0) {
        throw "FFmpeg enabled $Kind do not match manifest. Difference: $($difference | Out-String)"
    }
}
$actualComponents = [ordered]@{
    demuxers = @(EnabledComponents 'DEMUXER')
    parsers = @(EnabledComponents 'PARSER')
    decoders = @(EnabledComponents 'DECODER')
    bitstream_filters = @(EnabledComponents 'BSF')
}
Assert-ExactComponents 'demuxers' $allowedDemuxers $actualComponents.demuxers
Assert-ExactComponents 'parsers' $allowedParsers $actualComponents.parsers
Assert-ExactComponents 'decoders' $allowedDecoders $actualComponents.decoders
Assert-ExactComponents 'bitstream filters' $allowedBsfs $actualComponents.bitstream_filters

& $bashPath -lc "export PATH='${clDirectoryMsys}:${nasmDirectoryMsys}:/c/msys64/usr/bin:/usr/bin:`$PATH' && cd '$bashSource' && make -j2 && make install"
if ($LASTEXITCODE -ne 0) { throw "FFmpeg build/install failed with exit code $LASTEXITCODE" }

$configOutput = Join-Path $sourceDir "ffbuild\config.log"
if (-not (Test-Path -LiteralPath $configOutput)) { throw "FFmpeg did not emit config.log" }
$configText = Get-Content -Raw -LiteralPath $configOutput
if ($configText -match "--enable-gpl|--enable-nonfree|--enable-network|--enable-programs") { throw "FFmpeg configuration contains a forbidden feature" }
$configMake = Join-Path $sourceDir 'ffbuild\config.mak'
$configurationLine = Get-Content -LiteralPath $configMake | Where-Object { $_ -like 'FFMPEG_CONFIGURATION=*' } | Select-Object -First 1
if (-not $configurationLine) { throw 'FFmpeg did not record FFMPEG_CONFIGURATION in ffbuild/config.mak' }
$configuration = $configurationLine.Substring('FFMPEG_CONFIGURATION='.Length)
Set-Content -Encoding UTF8 -LiteralPath (Join-Path $out "FFMPEG_BUILD.json") -Value (@{
    version = $version
    source_url = $url
    source_sha256 = $actualSha
    patch = $patchRelativePath
    patch_sha256 = $actualPatchSha
    configure = $configure
    configuration = $configuration
    prefix = $prefixFull
    platform = [System.Runtime.InteropServices.RuntimeInformation]::OSDescription
} | ConvertTo-Json -Depth 5)
Copy-Item -LiteralPath $patchPath -Destination (Join-Path $out "FFMPEG_CHANGES.diff") -Force
Set-Content -Encoding UTF8 -LiteralPath (Join-Path $out "FFMPEG_COMPONENTS.json") -Value ($actualComponents | ConvertTo-Json -Depth 5)
Copy-Item -LiteralPath $configOutput -Destination (Join-Path $out "FFMPEG_CONFIGURE.log") -Force
Write-Output "Built FFmpeg $version with the audited shared-only audio configuration."
