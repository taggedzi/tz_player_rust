[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string] $Directory
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$root = (Resolve-Path -LiteralPath $Directory).Path
$runningOnWindows = [Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
    [Runtime.InteropServices.OSPlatform]::Windows
)
$runningOnMacOS = [Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
    [Runtime.InteropServices.OSPlatform]::OSX
)

if ($runningOnWindows) {
    $dumpbinCommand = Get-Command dumpbin.exe -ErrorAction SilentlyContinue
    $dumpbin = if ($dumpbinCommand) { $dumpbinCommand.Source } else { $null }
    if (-not $dumpbin) {
        $vswhere = 'C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe'
        if (Test-Path -LiteralPath $vswhere) {
            $vsPath = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
            $dumpbin = Get-ChildItem -LiteralPath (Join-Path $vsPath 'VC\Tools\MSVC') -Filter dumpbin.exe -Recurse |
                Where-Object { $_.FullName -match '\\bin\\Hostx64\\x64\\dumpbin\.exe$' } |
                Select-Object -First 1 -ExpandProperty FullName
        }
    }
    if (-not $dumpbin) { throw 'dumpbin.exe is required to inspect Windows package dependencies.' }

    $binaries = @(Get-ChildItem -LiteralPath $root -File | Where-Object { $_.Extension -in '.exe', '.dll' })
    $packaged = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($binary in $binaries) { [void]$packaged.Add($binary.Name) }
    $allowedSystem = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($name in 'bcrypt.dll', 'KERNEL32.dll', 'ntdll.dll') { [void]$allowedSystem.Add($name) }

    foreach ($binary in $binaries) {
        $dependencies = & $dumpbin /dependents $binary.FullName |
            Where-Object { $_ -match '^\s+([A-Za-z0-9_.-]+\.dll)\s*$' } |
            ForEach-Object { $Matches[1] }
        foreach ($dependency in $dependencies) {
            if ($packaged.Contains($dependency) -or $allowedSystem.Contains($dependency) -or
                $dependency -match '^(api|ext)-ms-win-.*\.dll$') {
                continue
            }
            throw "Unexpected non-packaged dependency for $($binary.Name): $dependency"
        }
    }
}
elseif ($runningOnMacOS) {
    if (-not (Get-Command otool -ErrorAction SilentlyContinue)) { throw 'otool is required to inspect macOS package dependencies.' }
    $binaries = @(Get-ChildItem -LiteralPath $root -File | Where-Object { $_.Name -eq 'tz-audio-decoder' -or $_.Extension -eq '.dylib' })
    $packaged = @($binaries.Name)
    foreach ($binary in $binaries) {
        $dependencies = & otool -L $binary.FullName | Select-Object -Skip 1 | ForEach-Object { ($_ -split '\s+')[1] }
        foreach ($dependency in $dependencies) {
            $name = Split-Path -Leaf $dependency
            if ($packaged -contains $name -or $dependency -like '/usr/lib/*' -or
                $dependency -like '/System/Library/*') {
                continue
            }
            throw "Unexpected non-packaged dependency for $($binary.Name): $dependency"
        }
    }
}
else {
    if (-not (Get-Command ldd -ErrorAction SilentlyContinue)) { throw 'ldd is required to inspect Linux package dependencies.' }
    $binaries = @(Get-ChildItem -LiteralPath $root -File | Where-Object { $_.Name -eq 'tz-audio-decoder' -or $_.Name -match '\.so(\.|$)' })
    $allowedSystem = 'ld-linux', 'libc.so', 'libdl.so', 'libgcc_s.so', 'libm.so', 'libpthread.so', 'librt.so'
    foreach ($binary in $binaries) {
        $lines = & ldd $binary.FullName
        if ($LASTEXITCODE -ne 0 -or $lines -match 'not found') { throw "Unresolved dependency for $($binary.Name): $($lines -join '; ')" }
        foreach ($line in $lines) {
            if ($line -notmatch '^\s*([^\s]+)\s+=>\s+(.+?)\s+\(0x[0-9a-fA-F]+\)\s*$') { continue }
            $name = $Matches[1]
            $path = $Matches[2]
            if ($path.StartsWith($root, [StringComparison]::Ordinal) -or
                ($allowedSystem | Where-Object { $name.StartsWith($_, [StringComparison]::Ordinal) })) {
                continue
            }
            throw "Unexpected non-packaged dependency for $($binary.Name): $name => $path"
        }
    }
}

Write-Host "Native dependency inspection passed for $root"
