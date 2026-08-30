[CmdletBinding()]
param(
    [string] $EfiPath,
    [string] $OutputPath,
    [string] $MemoryPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$workspaceRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..')).Path
$config = Import-PowerShellDataFile -LiteralPath (Join-Path $PSScriptRoot 'config.psd1')
$romSizeBytes = [int] $config.Pci.ExpansionRomSizeBytes
if ($romSizeBytes -ne 4096) {
    throw "The Squirrel ROM leaf is fixed at 4096 bytes; config.psd1 specifies $romSizeBytes."
}

if ([string]::IsNullOrWhiteSpace($EfiPath)) {
    Push-Location $workspaceRoot
    try {
        & cargo build-dxe
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build-dxe failed with exit code $LASTEXITCODE."
        }
    }
    finally {
        Pop-Location
    }

    $EfiPath = Join-Path $workspaceRoot 'target\x86_64-unknown-uefi\release\svmvisor-dxe.efi'
}

$resolvedEfiPath = (Resolve-Path -LiteralPath $EfiPath).Path

if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    $OutputPath = Join-Path $workspaceRoot 'target\firmware\squirrel\rom\svmvisor-dxe.rom'
} else {
    $OutputPath = [System.IO.Path]::GetFullPath($OutputPath)
}

if ([string]::IsNullOrWhiteSpace($MemoryPath)) {
    $MemoryPath = [System.IO.Path]::ChangeExtension($OutputPath, '.mem')
} else {
    $MemoryPath = [System.IO.Path]::GetFullPath($MemoryPath)
}

$outputParent = Split-Path -Parent $OutputPath
$memoryParent = Split-Path -Parent $MemoryPath
New-Item -ItemType Directory -Force -Path $outputParent,$memoryParent | Out-Null

Push-Location $workspaceRoot
try {
    & cargo run `
        --quiet `
        --release `
        --package svmvisor-rompack `
        -- `
        --input $resolvedEfiPath `
        --output $OutputPath `
        --memory-output $MemoryPath `
        --memory-size $romSizeBytes `
        --vendor $config.Pci.VendorId `
        --device $config.Pci.DeviceId `
        --class $config.Pci.ClassCode

    if ($LASTEXITCODE -ne 0) {
        throw "svmvisor-rompack failed with exit code $LASTEXITCODE."
    }
}
finally {
    Pop-Location
}

$rom = Get-Item -LiteralPath $OutputPath
if (($rom.Length % 512) -ne 0) {
    throw "The generated option ROM is not aligned to a 512-byte image unit: '$OutputPath'."
}

$memory = Get-Item -LiteralPath $MemoryPath
$expectedMemoryLines = $romSizeBytes / 4
$actualMemoryLines = (Get-Content -LiteralPath $MemoryPath).Count
if ($actualMemoryLines -ne $expectedMemoryLines) {
    throw "The generated memory image contains $actualMemoryLines words; expected $expectedMemoryLines."
}

$sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $OutputPath).Hash.ToLowerInvariant()
Write-Host "Option ROM: $OutputPath"
Write-Host "Size: $($rom.Length) bytes"
Write-Host "SHA-256: $sha256"
Write-Host "FPGA memory: $($memory.FullName) ($romSizeBytes bytes)"
