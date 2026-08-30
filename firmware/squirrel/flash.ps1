[CmdletBinding(SupportsShouldProcess, ConfirmImpact = 'Medium')]
param(
    [Parameter(Mandatory)]
    [string] $ImagePath,

    [switch] $ConfirmFlash
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$workspaceRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..')).Path
$resolvedImage = (Resolve-Path -LiteralPath $ImagePath).Path

if ([System.IO.Path]::GetExtension($resolvedImage) -ne '.bin') {
    throw "Expected a .bin FPGA configuration image, got '$resolvedImage'."
}
if (-not $ConfirmFlash) {
    throw 'Flashing is destructive. Re-run with -ConfirmFlash after checking the image and target board.'
}
if (-not $PSCmdlet.ShouldProcess('Screamer PCIe Squirrel SPI flash', "Program '$resolvedImage'")) {
    return
}

& (Join-Path $PSScriptRoot 'bootstrap.ps1') -SkipGateware
if ($LASTEXITCODE -ne 0) {
    throw 'OpenOCD bootstrap failed.'
}

$openOcdExe = Join-Path $workspaceRoot 'target\firmware\tools\openocd\bin\openocd.exe'
$bscanProxy = Join-Path $workspaceRoot 'target\firmware\tools\lambda-squirrel\flash_screamer\bscan_spi_xc7a35t.bit'
$flashConfig = Join-Path $PSScriptRoot 'openocd\flash_squirrel.cfg'

foreach ($requiredPath in @($openOcdExe, $bscanProxy, $flashConfig)) {
    if (-not (Test-Path -LiteralPath $requiredPath)) {
        throw "Required flashing file is missing: '$requiredPath'."
    }
}

$imageTclPath = $resolvedImage.Replace('\', '/')
$bscanTclPath = $bscanProxy.Replace('\', '/')
$imageCommand = "set FPGAIMAGE {$imageTclPath}"
$bscanCommand = "set BSCAN_FILE {$bscanTclPath}"

Write-Warning 'The Squirrel must be powered from its PCIe slot, with its update USB-C port connected and WinUSB assigned to FTDI interface 0.'

& $openOcdExe `
    '-c' $bscanCommand `
    '-c' $imageCommand `
    '-f' $flashConfig

if ($LASTEXITCODE -ne 0) {
    throw "OpenOCD failed with exit code $LASTEXITCODE. The flash must not be assumed valid."
}

Write-Host 'Flash programming and verification completed. Power-cycle the target to load the new gateware.'
