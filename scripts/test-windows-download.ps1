# Network substitutes exercise the installer's real online branch.
param([string]$Archive, [string]$Checksums, [string]$Version, [string]$BinDir)
$ErrorActionPreference = 'Stop'
# This file runs in a dedicated pwsh process. Functions called by another
# script inherit that script's script scope, so keep fixture state explicit.
$global:CutcraftInstallerTestState = @{
    Urls = [Collections.Generic.List[string]]::new()
    FailDownload = $false
    Tag = "v$Version"
    FixtureArchive = $Archive
    FixtureChecksums = $Checksums
}
function Invoke-RestMethod {
    param($Uri, $TimeoutSec)
    $global:CutcraftInstallerTestState.Urls.Add($Uri)
    return @{ tag_name = $global:CutcraftInstallerTestState.Tag }
}
function Invoke-WebRequest {
    param($Uri, $OutFile, $TimeoutSec, [switch]$UseBasicParsing)
    $global:CutcraftInstallerTestState.Urls.Add($Uri)
    if ($global:CutcraftInstallerTestState.FailDownload) { throw 'simulated network failure' }
    if ($Uri.EndsWith('/SHA256SUMS')) {
        Copy-Item -LiteralPath $global:CutcraftInstallerTestState.FixtureChecksums -Destination $OutFile
    } else {
        Copy-Item -LiteralPath $global:CutcraftInstallerTestState.FixtureArchive -Destination $OutFile
    }
}
$installer = Join-Path $PSScriptRoot 'install.ps1'
$base = "https://github.com/XGHeaven/cutcraft/releases/download/v$Version"
$name = [IO.Path]::GetFileName($Archive)
& $installer -BinDir $BinDir
$expected = @('https://api.github.com/repos/XGHeaven/cutcraft/releases/latest', "$base/$name", "$base/SHA256SUMS")
if (($global:CutcraftInstallerTestState.Urls -join "`n") -cne ($expected -join "`n")) { throw 'Latest URLs are not pinned.' }
$global:CutcraftInstallerTestState.Urls.Clear()
& $installer -BinDir $BinDir -Version $Version -Force
if (($global:CutcraftInstallerTestState.Urls -join "`n") -cne (@("$base/$name", "$base/SHA256SUMS") -join "`n")) {
    throw 'Explicit version requested wrong URLs.'
}
$binary = Join-Path $BinDir 'cutcraft.exe'
$before = (Get-FileHash $binary).Hash
$global:CutcraftInstallerTestState.FailDownload = $true
try {
    & $installer -BinDir $BinDir -Version $Version -Force
    throw 'Expected failure was not raised.'
} catch {
    if ($_.ToString() -notmatch 'simulated network failure') { throw }
}
if ((Get-FileHash $binary).Hash -ne $before) { throw 'Failed download replaced installation.' }
$global:CutcraftInstallerTestState.FailDownload = $false
$global:CutcraftInstallerTestState.Tag = 'v../bad'
try {
    & $installer -BinDir $BinDir -Force
    throw 'Expected failure was not raised.'
} catch {
    if ($_.ToString() -notmatch 'Latest Release must use') { throw }
}
if ((Get-FileHash $binary).Hash -ne $before) { throw 'Invalid tag replaced installation.' }
