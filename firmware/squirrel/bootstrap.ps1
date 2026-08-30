[CmdletBinding()]
param(
    [switch] $SkipGateware,
    [switch] $SkipOpenOcd
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$workspaceRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..')).Path
$generatedRoot = Join-Path $workspaceRoot 'target\firmware'
$config = Import-PowerShellDataFile -LiteralPath (Join-Path $PSScriptRoot 'config.psd1')

function Invoke-CheckedGit {
    param([Parameter(Mandatory)][string[]] $GitArguments)

    & git @GitArguments
    if ($LASTEXITCODE -ne 0) {
        throw "git failed with exit code $LASTEXITCODE"
    }
}

function Get-VerifiedArchive {
    param(
        [Parameter(Mandatory)][string] $Uri,
        [Parameter(Mandatory)][string] $ExpectedSha256,
        [Parameter(Mandatory)][string] $ArchivePath
    )

    if (-not (Test-Path -LiteralPath $ArchivePath)) {
        Write-Host "Downloading $Uri"
        Invoke-WebRequest -UseBasicParsing -Uri $Uri -OutFile $ArchivePath
    }

    $actualSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $ArchivePath).Hash.ToLowerInvariant()
    if ($actualSha256 -ne $ExpectedSha256) {
        throw "Hash mismatch for '$ArchivePath'. Expected $ExpectedSha256, got $actualSha256."
    }
}

New-Item -ItemType Directory -Force -Path $generatedRoot | Out-Null

if (-not $SkipGateware) {
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
        throw 'Git is required to fetch the pinned PCIeSquirrel gateware.'
    }

    $upstreamRoot = Join-Path $workspaceRoot $config.Upstream.CheckoutPath
    $upstreamParent = Split-Path -Parent $upstreamRoot
    New-Item -ItemType Directory -Force -Path $upstreamParent | Out-Null

    if (-not (Test-Path -LiteralPath (Join-Path $upstreamRoot '.git'))) {
        Invoke-CheckedGit @(
            'clone',
            '--filter=blob:none',
            '--sparse',
            $config.Upstream.Repository,
            $upstreamRoot
        )
        Invoke-CheckedGit @('-C', $upstreamRoot, 'sparse-checkout', 'set', 'PCIeSquirrel')
    }

    $dirtyFiles = @(& git -C $upstreamRoot status --porcelain --untracked-files=no)
    if ($LASTEXITCODE -ne 0) {
        throw "Could not inspect '$upstreamRoot'."
    }
    if ($dirtyFiles.Count -ne 0) {
        throw "The generated upstream checkout has local changes: '$upstreamRoot'."
    }

    $currentCommit = (& git -C $upstreamRoot rev-parse HEAD 2>$null)
    if ($LASTEXITCODE -ne 0 -or $currentCommit -ne $config.Upstream.Commit) {
        Invoke-CheckedGit @('-C', $upstreamRoot, 'fetch', '--depth', '1', 'origin', $config.Upstream.Commit)
        Invoke-CheckedGit @('-C', $upstreamRoot, 'checkout', '--detach', $config.Upstream.Commit)
    }

    Write-Host "Pinned gateware: $($config.Upstream.Commit)"
}

if (-not $SkipOpenOcd) {
    $downloadsRoot = Join-Path $generatedRoot 'downloads'
    $toolsRoot = Join-Path $generatedRoot 'tools'
    $openOcdArchive = Join-Path $downloadsRoot 'openocd-win.zip'
    $flashSupportArchive = Join-Path $downloadsRoot 'flash_screamer.zip'
    $openOcdExe = Join-Path $toolsRoot 'openocd\bin\openocd.exe'
    $flashSupportRoot = Join-Path $toolsRoot 'lambda-squirrel'
    $bscanProxy = Join-Path $flashSupportRoot 'flash_screamer\bscan_spi_xc7a35t.bit'

    New-Item -ItemType Directory -Force -Path $downloadsRoot,$toolsRoot | Out-Null

    Get-VerifiedArchive `
        -Uri $config.OpenOcd.ArchiveUri `
        -ExpectedSha256 $config.OpenOcd.ArchiveSha256 `
        -ArchivePath $openOcdArchive

    Get-VerifiedArchive `
        -Uri $config.OpenOcd.FlashSupportUri `
        -ExpectedSha256 $config.OpenOcd.FlashSupportSha256 `
        -ArchivePath $flashSupportArchive

    if (-not (Test-Path -LiteralPath $openOcdExe)) {
        Expand-Archive -LiteralPath $openOcdArchive -DestinationPath $toolsRoot -Force
    }
    if (-not (Test-Path -LiteralPath $bscanProxy)) {
        New-Item -ItemType Directory -Force -Path $flashSupportRoot | Out-Null
        Expand-Archive -LiteralPath $flashSupportArchive -DestinationPath $flashSupportRoot -Force
    }

    if (-not (Test-Path -LiteralPath $openOcdExe)) {
        throw "OpenOCD was not installed at '$openOcdExe'."
    }
    if (-not (Test-Path -LiteralPath $bscanProxy)) {
        throw "The XC7A35T BSCAN-SPI proxy was not installed at '$bscanProxy'."
    }

    Write-Host "OpenOCD: $openOcdExe"
    Write-Host "BSCAN-SPI proxy: $bscanProxy"
}
