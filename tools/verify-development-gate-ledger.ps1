[CmdletBinding()]
param(
    [Parameter()]
    [string]$RepositoryRoot
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Assert-Condition {
    param(
        [Parameter(Mandatory = $true)]
        [bool]$Condition,

        [Parameter(Mandatory = $true)]
        [string]$Message
    )

    if (-not $Condition) {
        throw $Message
    }
}

function Get-LowerSha256 {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    return (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

if ([string]::IsNullOrWhiteSpace($RepositoryRoot)) {
    $RepositoryRoot = Split-Path -Parent $PSScriptRoot
}

$RepositoryRoot = [IO.Path]::GetFullPath($RepositoryRoot)
$ledgerPath = Join-Path $RepositoryRoot 'docs\development-gate-ledger-v1.json'
Assert-Condition (Test-Path -LiteralPath $ledgerPath -PathType Leaf) `
    "Development gate ledger is missing: $ledgerPath"

$ledger = Get-Content -Raw -LiteralPath $ledgerPath | ConvertFrom-Json
Assert-Condition ($ledger.schema_version -eq 1) 'Unexpected development gate ledger schema.'
Assert-Condition ($ledger.ledger_kind -ceq 'svmvisor-development-gate-ledger') `
    'Unexpected development gate ledger kind.'
Assert-Condition ($ledger.semantics.planning_only -eq $true) `
    'Development gate ledger must remain planning-only.'

foreach ($field in @(
    'promotion_decision',
    'qualification_decision',
    'boot_authorization',
    'flash_authorization',
    'control_state_write_authorization',
    'svm_enable_authorization',
    'vmrun_authorization',
    'untrusted_guest_authorization'
)) {
    Assert-Condition ($ledger.semantics.$field -eq $false) `
        "Development gate ledger must keep semantics.$field false."
}

Assert-Condition ($ledger.canonical_m0b.schema_version -eq 4) `
    'Schema v4 must remain canonical until an explicit promotion decision.'
Assert-Condition ($ledger.canonical_m0b.status -ceq 'canonical') `
    'Canonical M0b role is inconsistent.'
Assert-Condition ($ledger.m0b_promotion_candidate.schema_version -eq 6) `
    'Schema v6 must remain the promotion candidate.'
Assert-Condition `
    ($ledger.m0b_promotion_candidate.status -ceq 'awaiting-explicit-promotion-decision') `
    'Schema v6 promotion status changed without updating the ledger contract.'
Assert-Condition `
    ($ledger.m0b_promotion_candidate.one_shot_usb_authorization -ceq 'consumed-no-retry') `
    'Schema v6 USB authorization must remain consumed with no retry.'

$rawRecords = @{}
foreach ($entry in @($ledger.canonical_m0b, $ledger.m0b_promotion_candidate)) {
    $bundleDirectory = Join-Path $RepositoryRoot $entry.bundle_path
    Assert-Condition (Test-Path -LiteralPath $bundleDirectory -PathType Container) `
        "Evidence bundle is missing: $bundleDirectory"
    Assert-Condition `
        (@(Get-ChildItem -LiteralPath $bundleDirectory -File).Count -eq $entry.bundle_file_count) `
        "Evidence bundle file count changed: $bundleDirectory"

    $rawPath = Join-Path $bundleDirectory 'raw-probe.json'
    $manifestPath = Join-Path $bundleDirectory 'manifest.json'
    Assert-Condition ((Get-LowerSha256 $rawPath) -ceq $entry.raw_sha256) `
        "Raw evidence SHA-256 mismatch: $rawPath"
    Assert-Condition ((Get-LowerSha256 $manifestPath) -ceq $entry.manifest_sha256) `
        "Manifest SHA-256 mismatch: $manifestPath"

    $raw = Get-Content -Raw -LiteralPath $rawPath | ConvertFrom-Json
    $rawRecords[[string]$entry.schema_version] = $raw
    Assert-Condition ($raw.schema_version -eq $entry.schema_version) `
        "Raw evidence schema mismatch: $rawPath"
    Assert-Condition ($raw.qualification_status -ceq 'blocked') `
        "Raw evidence must remain qualification-blocked: $rawPath"

    foreach ($field in @(
        'launch_authorized',
        'physical_candidate_flash_authorized',
        'control_state_writes_authorized',
        'process_introspection_authorized',
        'amd_iommu_ownership_claim',
        'pci_isolation_claim'
    )) {
        Assert-Condition ($raw.$field -eq $false) `
            "Raw evidence field $field must remain false: $rawPath"
    }
}

$v4 = $rawRecords['4']
$v6 = $rawRecords['6']
$groupedBlockers = @(
    $ledger.remaining_v6_blocker_groups |
        ForEach-Object { @($_.blocker_ids) }
)
Assert-Condition ($groupedBlockers.Count -eq 7) `
    'Development gate ledger must partition exactly seven V6 blockers.'
Assert-Condition (@($groupedBlockers | Select-Object -Unique).Count -eq 7) `
    'Development gate ledger contains duplicate V6 blocker IDs.'
Assert-Condition `
    ((@($groupedBlockers | Sort-Object) -join "`n") -ceq
        (@($v6.uncollected_blockers | Sort-Object) -join "`n")) `
    'Development gate ledger blocker partition does not equal the V6 raw blocker set.'

$candidateRetired = @(
    $v4.uncollected_blockers |
        Where-Object { $_ -cnotin @($v6.uncollected_blockers) } |
        Sort-Object
)
$declaredRetired = @(
    $ledger.m0b_promotion_candidate.candidate_retired_inventory_ids |
        Sort-Object
)
Assert-Condition `
    (($candidateRetired -join "`n") -ceq ($declaredRetired -join "`n")) `
    'Candidate-retired inventory IDs do not equal the V4-minus-V6 blocker set.'
Assert-Condition `
    ($ledger.m0b_promotion_candidate.candidate_retired_ids_canonical_status -ceq
        'open-until-explicit-promotion') `
    'Candidate-retired IDs must remain canonically open before promotion.'

$targetProfileDirectory = Join-Path $RepositoryRoot $ledger.target_profile_binding.bundle_path
$targetProfilePath = Join-Path $targetProfileDirectory 'target-profile.json'
$targetManifestPath = Join-Path $targetProfileDirectory 'manifest.json'
Assert-Condition `
    ((Get-LowerSha256 $targetProfilePath) -ceq $ledger.target_profile_binding.profile_sha256) `
    'Target-profile SHA-256 mismatch.'
Assert-Condition `
    ((Get-LowerSha256 $targetManifestPath) -ceq $ledger.target_profile_binding.manifest_sha256) `
    'Target-profile manifest SHA-256 mismatch.'

Write-Host 'Development gate ledger verification passed.'
Write-Host 'Canonical M0b: schema v4'
Write-Host 'Promotion candidate: schema v6 (awaiting explicit decision)'
Write-Host 'Physical first light: blocked by listed prerequisites'
