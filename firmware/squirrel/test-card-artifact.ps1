[CmdletBinding()]
param([Parameter(Mandatory)][string]$PayloadPath)
$ErrorActionPreference = 'Stop'
$workspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$output = Join-Path $workspaceRoot ('target/firmware/squirrel/card-artifact-tests/' + [Guid]::NewGuid().ToString('N'))
& python -B (Join-Path $PSScriptRoot 'package-card-payload.py') --payload $PayloadPath --output $output
if ($LASTEXITCODE -ne 0) { throw 'Card artifact packaging failed.' }
$manifest = Get-Content -Raw -LiteralPath (Join-Path $output 'payload-manifest.json') | ConvertFrom-Json
$savedSlot = $env:SVMVISOR_TEST_CARD_SLOT
$savedDigest = $env:SVMVISOR_TEST_CARD_SHA256
try {
    $env:SVMVISOR_TEST_CARD_SLOT = Join-Path $output 'payload-slot.bin'
    $env:SVMVISOR_TEST_CARD_SHA256 = $manifest.package_sha256
    & cargo test --manifest-path (Join-Path $workspaceRoot 'Cargo.toml') -p svmvisor-dxe --features card-load-only --test card_artifact -- --ignored --nocapture
    if ($LASTEXITCODE -ne 0) { throw 'Cross-language card artifact validation failed.' }
} finally {
    $env:SVMVISOR_TEST_CARD_SLOT = $savedSlot
    $env:SVMVISOR_TEST_CARD_SHA256 = $savedDigest
}
Write-Output "Cross-language card artifact passed. Evidence: $output"
