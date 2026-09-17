[CmdletBinding()]
param(
    [string] $EfiPath,
    [string] $OutputPath,
    [string] $MemoryPath,
    [switch] $CardLoadOnly,
    [string] $CardPayloadSha256,
    [int] $RomSizeBytes = 0
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$workspaceRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..')).Path
$config = Import-PowerShellDataFile -LiteralPath (Join-Path $PSScriptRoot 'config.psd1')
$romSizeBytes = if ($RomSizeBytes) { $RomSizeBytes } elseif ($CardLoadOnly) { 32768 } else { [int] $config.Pci.ExpansionRomSizeBytes }
if (-not $CardLoadOnly -and $romSizeBytes -ne 8192) {
    throw "The completion-only Squirrel ROM is fixed at 8192 bytes; config.psd1 specifies $romSizeBytes."
}
if ($CardLoadOnly -and ($romSizeBytes -lt 8192 -or $romSizeBytes -gt 131072 -or ($romSizeBytes -band ($romSizeBytes - 1)))) { throw 'Candidate ROM size must be a power of two from 8192 through 131072.' }
if ($CardLoadOnly -and $CardPayloadSha256 -cnotmatch '^[0-9a-f]{64}$') { throw 'Card load-only requires the exact lowercase package SHA-256.' }
if ($CardLoadOnly -and $EfiPath) { throw 'Card load-only must build its digest-bound EFI image; EfiPath substitution is not accepted.' }

if ([string]::IsNullOrWhiteSpace($EfiPath)) {
    Push-Location $workspaceRoot
    try {
        if ($CardLoadOnly) {
            $savedDigest = $env:SVMVISOR_CARD_PAYLOAD_SHA256
            try {
                $env:SVMVISOR_CARD_PAYLOAD_SHA256 = $CardPayloadSha256
                & cargo build --profile dxe --package svmvisor-dxe --target x86_64-unknown-uefi --target-dir (Join-Path $workspaceRoot 'target/card-load-only-cargo') --features card-load-only
            } finally { $env:SVMVISOR_CARD_PAYLOAD_SHA256 = $savedDigest }
        } else { & cargo build-dxe }
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build-dxe failed with exit code $LASTEXITCODE."
        }
    }
    finally {
        Pop-Location
    }

    $EfiPath = if ($CardLoadOnly) { Join-Path $workspaceRoot 'target/card-load-only-cargo/x86_64-unknown-uefi/dxe/svmvisor-dxe.efi' } else { Join-Path $workspaceRoot 'target\x86_64-unknown-uefi\dxe\svmvisor-dxe.efi' }
}

$resolvedEfiPath = (Resolve-Path -LiteralPath $EfiPath).Path

if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    $OutputPath = Join-Path $workspaceRoot 'target\firmware\card\rom\svmvisor-dxe.rom'
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
