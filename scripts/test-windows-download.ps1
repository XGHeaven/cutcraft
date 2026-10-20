# Network substitutes exercise the installer's real online branch.
param([string]$Archive, [string]$Checksums, [string]$Version, [string]$BinDir)
$ErrorActionPreference = 'Stop'
$script:urls = [Collections.Generic.List[string]]::new()
$script:failDownload = $false
$script:tag = "v$Version"
$script:fixtureArchive = $Archive
$script:fixtureChecksums = $Checksums
function Invoke-RestMethod {
    param($Uri, $TimeoutSec)
    $script:urls.Add($Uri)
    return @{ tag_name = $script:tag }
}
function Invoke-WebRequest {
    param($Uri, $OutFile, $TimeoutSec, [switch]$UseBasicParsing)
    $script:urls.Add($Uri)
    if ($script:failDownload) { throw 'simulated network failure' }
    if ($Uri.EndsWith('/SHA256SUMS')) {
        Copy-Item -LiteralPath $script:fixtureChecksums -Destination $OutFile
    } else {
        Copy-Item -LiteralPath $script:fixtureArchive -Destination $OutFile
    }
}
$installer = Join-Path $PSScriptRoot 'install.ps1'
$base = "https://github.com/XGHeaven/cutcraft/releases/download/v$Version"
$name = [IO.Path]::GetFileName($Archive)
& $installer -BinDir $BinDir
$expected = @('https://api.github.com/repos/XGHeaven/cutcraft/releases/latest', "$base/$name", "$base/SHA256SUMS")
if (($script:urls -join "`n") -cne ($expected -join "`n")) { throw 'Latest URLs are not pinned.' }
$script:urls.Clear()
& $installer -BinDir $BinDir -Version $Version -Force
if (($script:urls -join "`n") -cne (@("$base/$name", "$base/SHA256SUMS") -join "`n")) {
    throw 'Explicit version requested wrong URLs.'
}
$binary = Join-Path $BinDir 'cutcraft.exe'
$before = (Get-FileHash $binary).Hash
$script:failDownload = $true
try {
    & $installer -BinDir $BinDir -Version $Version -Force
    throw 'Expected failure was not raised.'
} catch {
    if ($_.ToString() -notmatch 'simulated network failure') { throw }
}
if ((Get-FileHash $binary).Hash -ne $before) { throw 'Failed download replaced installation.' }
$script:failDownload = $false
$script:tag = 'v../bad'
try {
    & $installer -BinDir $BinDir -Force
    throw 'Expected failure was not raised.'
} catch {
    if ($_.ToString() -notmatch 'Latest Release must use') { throw }
}
if ((Get-FileHash $binary).Hash -ne $before) { throw 'Invalid tag replaced installation.' }
