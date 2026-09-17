[CmdletBinding(SupportsShouldProcess, ConfirmImpact = 'Medium')]
param(
    [Parameter(Mandatory)]
    [string] $ImagePath,

    [Parameter(Mandatory)]
    [string] $ManifestPath,

    [switch] $Recovery,
    [switch] $CheckOnly,
    [switch] $ConfirmFlash
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$workspaceRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..')).Path
$approved = & (Join-Path $PSScriptRoot 'check-flash-manifest.ps1') -ImagePath $ImagePath -ManifestPath $ManifestPath -Recovery:$Recovery
$resolvedImage = $approved.ImagePath
if ($CheckOnly) {
    Write-Host "Flash manifest accepted: $($approved.ImageKind), SHA-256 $($approved.ImageSha256). No hardware accessed."
    return
}
if (-not $ConfirmFlash) {
    throw 'Flashing is destructive. Re-run with -ConfirmFlash after checking the image and target board.'
}
if (-not $PSCmdlet.ShouldProcess('Screamer PCIe Squirrel SPI flash', "Program '$resolvedImage'")) {
    return
}

$LASTEXITCODE = 0
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

# Program a private snapshot of the checked bytes, retaining the readback beside
# it. Do not pass user-selected names directly into Tcl command text.
$sessionRoot = Join-Path $workspaceRoot ('target\firmware\card\flash-sessions\' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $sessionRoot | Out-Null
$stagedImage = Join-Path $sessionRoot 'image.bin'
Copy-Item -LiteralPath $resolvedImage -Destination $stagedImage
Copy-Item -LiteralPath $ManifestPath -Destination (Join-Path $sessionRoot 'manifest.json')
if ((Get-FileHash -LiteralPath $stagedImage -Algorithm SHA256).Hash.ToLowerInvariant() -cne $approved.ImageSha256) {
    throw 'Image changed after manifest validation.'
}
$readback = Join-Path $sessionRoot 'readback.bin'
foreach ($tclPath in @($stagedImage, $readback, $bscanProxy)) {
    if ($tclPath -match '[{}\r\n]') { throw 'Unsupported Tcl path characters.' }
}
$imageTclPath = $stagedImage.Replace('\', '/')
$bscanTclPath = $bscanProxy.Replace('\', '/')
$imageCommand = "set FPGAIMAGE {$imageTclPath}"
$bscanCommand = "set BSCAN_FILE {$bscanTclPath}"
$readbackCommand = 'set READBACK_FILE {' + $readback.Replace('\', '/') + '}'
$sizeCommand = "set IMAGE_SIZE $($approved.ImageSizeBytes)"

Write-Warning 'The Squirrel must be powered from its PCIe slot, with its update USB-C port connected and WinUSB assigned to FTDI interface 0.'

& $openOcdExe `
    '-c' $bscanCommand `
    '-c' $imageCommand `
    '-c' $readbackCommand `
    '-c' $sizeCommand `
    '-f' $flashConfig

if ($LASTEXITCODE -ne 0) {
    throw "OpenOCD failed with exit code $LASTEXITCODE. The flash must not be assumed valid."
}

if ((Get-Item -LiteralPath $readback).Length -ne $approved.ImageSizeBytes -or
    (Get-FileHash -LiteralPath $readback -Algorithm SHA256).Hash.ToLowerInvariant() -cne $approved.ImageSha256) {
    throw 'Flash readback does not match the approved image. Do not load the new gateware.'
}
Write-Host "Flash programming and image-range readback verified. Evidence: $sessionRoot"
Write-Host 'The image has not been activated by this script. Power-cycle only as specified by the physical-run procedure.'
