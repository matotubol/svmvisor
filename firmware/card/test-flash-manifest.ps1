# Focused host-only checks: never bootstrap OpenOCD or access a device.
[CmdletBinding()]
param()
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$workspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$testRoot = Join-Path $workspaceRoot ('target\firmware\card\manifest-tests\' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $testRoot | Out-Null
$image = Join-Path $testRoot 'fixture.bin'
$manifestPath = Join-Path $testRoot 'manifest.json'
[IO.File]::WriteAllBytes($image, [byte[]] @(1, 2, 3, 4))
$digest = (Get-FileHash -LiteralPath $image -Algorithm SHA256).Hash.ToLowerInvariant()
$checker = Join-Path $PSScriptRoot 'check-flash-manifest.ps1'
$script:passed = 0
function Test-Case {
    param([string] $Name, [hashtable] $Overrides, [bool] $Accept, [switch] $Recovery)
    $manifest = @{
        schema_version = 1; target_part = 'xc7a35tfgg484-2'; image_kind = 'completion_only'
        image_sha256 = $digest; image_size_bytes = 4; completion_only_netlist_policy = 'PASS'
    }
    foreach ($key in $Overrides.Keys) { $manifest[$key] = $Overrides[$key] }
    $manifest | ConvertTo-Json | Set-Content -LiteralPath $manifestPath -Encoding UTF8
    $accepted = $false
    try { & $checker -ImagePath $image -ManifestPath $manifestPath -Recovery:$Recovery | Out-Null; $accepted = $true } catch { }
    if ($accepted -ne $Accept) { throw "FAIL: $Name (accepted=$accepted)" }
    $script:passed++
    Write-Host "PASS: $Name"
}
Test-Case 'matching completion-only manifest' @{} $true
Test-Case 'wrong part' @{target_part='xc7a100tfgg484-2'} $false
Test-Case 'unknown schema' @{schema_version=2} $false
Test-Case 'failed policy' @{completion_only_netlist_policy='FAIL'} $false
Test-Case 'missing policy' @{completion_only_netlist_policy=$null} $false
Test-Case 'transitional image' @{image_kind='transitional'} $false
Test-Case 'card load-only review is not flash-approved' @{image_kind='card_load_only_review'} $false
Test-Case 'changed image digest' @{image_sha256=('0' * 64)} $false
Test-Case 'wrong image size' @{image_size_bytes=5} $false
Test-Case 'recovery cannot use experimental path' @{image_kind='non_enumerating_recovery'} $false
Test-Case 'unapproved recovery hash' @{image_kind='non_enumerating_recovery';non_enumerating_netlist_policy='PASS'} $false -Recovery
# The actual pinned candidate must pass the public entry point without tools or hardware.
$pin = Import-PowerShellDataFile (Join-Path $PSScriptRoot 'recovery-pin.psd1')
$candidate = Join-Path $workspaceRoot $pin.ArtifactDirectory
if (Test-Path -LiteralPath $candidate) {
    & (Join-Path $PSScriptRoot 'flash.ps1') -ImagePath (Join-Path $candidate 'svmvisor-recovery.bin') -ManifestPath (Join-Path $candidate 'manifest.json') -Recovery -CheckOnly
    $script:passed++
    # A changed manifest must fail even when its image hash still matches.
    $modified = Get-Content -LiteralPath (Join-Path $candidate 'manifest.json') -Raw | ConvertFrom-Json
    $modified.physical_fixture_test = 'PASS'
    $modified | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $manifestPath -Encoding UTF8
    $rejected = $false
    try { & $checker -ImagePath (Join-Path $candidate 'svmvisor-recovery.bin') -ManifestPath $manifestPath -Recovery | Out-Null } catch { $rejected = $true }
    if (-not $rejected) { throw 'Altered recovery manifest was accepted.' }
    $script:passed++
} else { Write-Host 'Pinned local artifact unavailable; candidate integration checks skipped.' }
Write-Host "$script:passed focused checks passed. No hardware accessed."
