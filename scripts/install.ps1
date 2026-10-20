# Download from repository main; binaries/checksums always use one Release tag.
# Requires Windows PowerShell 5.1+ or PowerShell 7+, no Rust/Python dependency.
[CmdletBinding()]
param(
    [string]$Repo = 'XGHeaven/cutcraft',
    [string]$Version = 'latest',
    [string]$BinDir = (Join-Path $env:LOCALAPPDATA 'Cutcraft\bin'),
    [string]$Archive,
    [string]$Checksums,
    [switch]$Force
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:OS -ne 'Windows_NT') { throw 'This installer requires Windows.' }
# Use OS architecture even when launched from 32-bit PowerShell.
$arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
if ($arch -ne 'AMD64') { throw 'Supported Windows platform: x64 (MSVC).' }
if ($Repo -cnotmatch '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$') { throw 'Invalid -Repo OWNER/REPO.' }
if ($Version.StartsWith('v')) { $Version = $Version.Substring(1) }
$versionPattern = '^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.-]+)?$'
if ($Archive) {
    if ($PSBoundParameters.ContainsKey('Repo')) { throw '-Archive and -Repo are mutually exclusive.' }
    if (!$Checksums -or $Version -cnotmatch $versionPattern) {
        throw 'Offline installation requires -Checksums and an explicit -Version.'
    }
} else {
    if ($Checksums) { throw '-Checksums requires -Archive.' }
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    if ($Version -eq 'latest') {
        $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -TimeoutSec 60
        if ($release.tag_name -cnotmatch '^v[0-9]+\.[0-9]+\.[0-9]+$') {
            throw 'Latest Release must use a stable vVERSION tag; specify -Version explicitly.'
        }
        $Version = $release.tag_name.Substring(1)
    }
    if ($Version -cnotmatch $versionPattern) { throw 'Invalid -Version.' }
}
$prefix = "cutcraft-v$Version-x86_64-pc-windows-msvc"
$name = "$prefix.zip"
$destination = [IO.Path]::GetFullPath($BinDir)
$binary = Join-Path $destination 'cutcraft.exe'
if ((Test-Path -LiteralPath $binary) -and !$Force) { throw "Already exists: $binary; use -Force to replace." }
$temp = Join-Path ([IO.Path]::GetTempPath()) ("cutcraft-install-" + [Guid]::NewGuid().ToString('N'))
$staged = $null
try {
    $null = New-Item -ItemType Directory -Path $temp
    $zip = Join-Path $temp $name
    $sums = Join-Path $temp 'SHA256SUMS'
    if ($Archive) {
        if ([IO.Path]::GetFileName($Archive) -cne $name) { throw "Expected archive $name" }
        Copy-Item -LiteralPath $Archive -Destination $zip
        Copy-Item -LiteralPath $Checksums -Destination $sums
    } else {
        $base = "https://github.com/$Repo/releases/download/v$Version"
        Invoke-WebRequest -UseBasicParsing -Uri "$base/$name" -OutFile $zip -TimeoutSec 300
        Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS" -OutFile $sums -TimeoutSec 60
    }
    $matchesFound = @(Get-Content -LiteralPath $sums | Where-Object {
        $_ -cmatch ('^[0-9a-fA-F]{64}  ' + [Regex]::Escape($name) + '$')
    })
    if ($matchesFound.Count -ne 1) { throw 'Missing or duplicate checksum.' }
    $expected = $matchesFound[0].Substring(0, 64)
    if ((Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash -ine $expected) {
        throw 'SHA-256 mismatch; installation aborted.'
    }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $package = [IO.Compression.ZipFile]::OpenRead($zip)
    $extracted = Join-Path $temp 'cutcraft.exe'
    try {
        # Extract only the exact executable; never trust archive paths.
        $entries = @($package.Entries | Where-Object { $_.FullName -ceq "$prefix/cutcraft.exe" })
        if ($entries.Count -ne 1) { throw 'Missing or duplicate cutcraft.exe in archive.' }
        [IO.Compression.ZipFileExtensions]::ExtractToFile($entries[0], $extracted)
    } finally {
        $package.Dispose()
    }
    $actualVersion = & $extracted --version
    if ($LASTEXITCODE -ne 0 -or "$actualVersion".Trim() -cne "cutcraft $Version") {
        throw 'Binary version does not match Release.'
    }
    $null = New-Item -ItemType Directory -Path $destination -Force
    $staged = Join-Path $destination ('.cutcraft-install-' + [Guid]::NewGuid().ToString('N') + '.exe')
    Copy-Item -LiteralPath $extracted -Destination $staged
    if ($Force -and (Test-Path -LiteralPath $binary)) {
        [IO.File]::Replace($staged, $binary, [NullString]::Value)
    } else {
        # Move refuses an existing destination, including a concurrent install.
        [IO.File]::Move($staged, $binary)
    }
    Write-Output "Installed cutcraft $Version to $binary"
    Write-Output "Add this directory to PATH if needed: $destination"
} finally {
    if ($staged -and (Test-Path -LiteralPath $staged)) { Remove-Item -LiteralPath $staged -Force }
    if (Test-Path -LiteralPath $temp) { Remove-Item -LiteralPath $temp -Recurse -Force }
}
