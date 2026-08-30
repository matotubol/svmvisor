<#
.SYNOPSIS
Finalizes one raw svmvisor M0b probe result into a new evidence bundle.

.DESCRIPTION
Validates the source M0a bundle with both the current and copied M0a verifiers,
checks the raw probe's profile binding and fail-closed safety fields, and builds
a new non-overwriting eight-file bundle. Raw JSON, the exact M0a manifest, the
probe EFI, schema, and scripts are copied byte-for-byte and hashed. A staging
directory is verified before it is atomically renamed to OutputDirectory.

.PARAMETER RawEvidencePath
The one raw svmvisor-m0b-*.json file returned by the probe.

.PARAMETER TargetProfileBundle
The immutable five-file M0a target-profile bundle used to prepare the medium.

.PARAMETER ProbeEfiPath
The exact probe EFI image whose bytes are to be preserved with the evidence.

.PARAMETER OutputDirectory
New finalized bundle path. It and its staging path must not already exist.
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string] $RawEvidencePath,

    [Parameter(Mandatory)]
    [string] $TargetProfileBundle,

    [Parameter(Mandatory)]
    [string] $ProbeEfiPath,

    [Parameter(Mandatory)]
    [string] $OutputDirectory
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Assert-Condition {
    param(
        [Parameter(Mandatory)] $Condition,
        [Parameter(Mandatory)] [string] $Message
    )

    if (-not $Condition) {
        throw $Message
    }
}

function Test-OrdinalEqual {
    param($Left, $Right)
    return [System.StringComparer]::Ordinal.Equals([string] $Left, [string] $Right)
}

function Assert-NoDuplicateJsonPropertyNames {
    param([string] $JsonText, [string] $Context)
    $frames = New-Object 'System.Collections.Generic.Stack[object]'
    $index = 0
    while ($index -lt $JsonText.Length) {
        $character = $JsonText[$index]
        if ($character -eq '{') {
            $frames.Push([pscustomobject] @{ Kind = 'object'; Keys = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal) })
            $index++
            continue
        }
        if ($character -eq '[') {
            $frames.Push([pscustomobject] @{ Kind = 'array'; Keys = $null })
            $index++
            continue
        }
        if ($character -eq '}' -or $character -eq ']') {
            if ($frames.Count -gt 0) { [void] $frames.Pop() }
            $index++
            continue
        }
        if ($character -ne '"') { $index++; continue }
        $start = $index
        $index++
        $closed = $false
        while ($index -lt $JsonText.Length) {
            if ($JsonText[$index] -eq '\') { $index += 2; continue }
            if ($JsonText[$index] -eq '"') { $index++; $closed = $true; break }
            $index++
        }
        Assert-Condition $closed "$Context contains an unterminated JSON string."
        $next = $index
        while ($next -lt $JsonText.Length -and [char]::IsWhiteSpace($JsonText[$next])) { $next++ }
        if ($next -ge $JsonText.Length -or $JsonText[$next] -ne ':') { continue }
        Assert-Condition ($frames.Count -gt 0 -and $frames.Peek().Kind -ceq 'object') "$Context contains a property name outside an object."
        $key = $JsonText.Substring($start, $index - $start) | ConvertFrom-Json
        Assert-Condition ($frames.Peek().Keys.Add([string] $key)) "$Context contains a duplicate JSON property name: '$key'."
    }
}

function Assert-JsonSchemaVersion {
    param($Value)
    Assert-Condition ($Value -is [int32] -or $Value -is [int64]) 'Raw evidence schema_version must be a JSON integer.'
    Assert-Condition ($Value -ne 5) 'Schema v5 is withdrawn and cannot be newly finalized; preserve it as diagnostic raw evidence and recapture with schema v6.'
    Assert-Condition ($Value -in @(1, 2, 3, 4, 6)) 'Unexpected raw evidence schema version.'
    return [int] $Value
}

function Get-ExistingDirectory {
    param([string] $LiteralPath, [string] $Description)
    $item = Get-Item -Force -LiteralPath $LiteralPath
    Assert-Condition ([bool] $item.PSIsContainer) "$Description is not a directory: '$LiteralPath'."
    Assert-Condition `
        (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -eq 0) `
        "$Description must not be a reparse point: '$LiteralPath'."
    return $item.FullName
}

function Get-ExistingLeafFile {
    param([string] $LiteralPath, [string] $Description)
    $item = Get-Item -Force -LiteralPath $LiteralPath
    Assert-Condition (-not [bool] $item.PSIsContainer) "$Description is not a file: '$LiteralPath'."
    Assert-Condition `
        (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -eq 0) `
        "$Description must not be a reparse point: '$LiteralPath'."
    return $item.FullName
}

function Assert-ProbeEfiApplication {
    param([Parameter(Mandatory)] [string] $LiteralPath)

    $bytes = [System.IO.File]::ReadAllBytes($LiteralPath)
    Assert-Condition ($bytes.Length -ge 64) 'Probe EFI is too short to contain a DOS/PE header.'
    Assert-Condition `
        ($bytes[0] -eq 0x4d -and $bytes[1] -eq 0x5a) `
        'Probe EFI does not have an MZ header.'
    $peOffset = [long] [System.BitConverter]::ToUInt32($bytes, 0x3c)
    Assert-Condition `
        ($peOffset -ge 64 -and $peOffset -le ($bytes.Length - 24)) `
        'Probe EFI has an out-of-range PE header offset.'
    Assert-Condition `
        ([System.BitConverter]::ToUInt32($bytes, [int] $peOffset) -eq 0x00004550) `
        'Probe EFI does not have a PE signature.'
    Assert-Condition `
        ([System.BitConverter]::ToUInt16($bytes, [int] ($peOffset + 4)) -eq 0x8664) `
        'Probe EFI is not an AMD64 PE image.'
    Assert-Condition `
        ([System.BitConverter]::ToUInt16($bytes, [int] ($peOffset + 6)) -gt 0) `
        'Probe EFI has no PE sections.'
    $characteristics = [System.BitConverter]::ToUInt16($bytes, [int] ($peOffset + 22))
    Assert-Condition `
        (($characteristics -band 0x0002) -ne 0) `
        'Probe EFI is not marked as an executable image.'

    $optionalSize = [long] [System.BitConverter]::ToUInt16($bytes, [int] ($peOffset + 20))
    $optionalOffset = $peOffset + 24
    Assert-Condition `
        ($optionalSize -ge 160 -and ($optionalOffset + $optionalSize) -le $bytes.Length) `
        'Probe EFI has a truncated PE32+ optional header.'
    Assert-Condition `
        ([System.BitConverter]::ToUInt16($bytes, [int] $optionalOffset) -eq 0x020b) `
        'Probe EFI is not PE32+.'
    Assert-Condition `
        ([System.BitConverter]::ToUInt16($bytes, [int] ($optionalOffset + 68)) -eq 10) `
        'Probe EFI is not an EFI_APPLICATION image.'
    $directoryCount = [System.BitConverter]::ToUInt32($bytes, [int] ($optionalOffset + 108))
    Assert-Condition ($directoryCount -ge 6) 'Probe EFI has no base-relocation directory slot.'
    $relocationOffset = $optionalOffset + 112 + (5 * 8)
    $relocationRva = [System.BitConverter]::ToUInt32($bytes, [int] $relocationOffset)
    $relocationSize = [System.BitConverter]::ToUInt32($bytes, [int] ($relocationOffset + 4))
    Assert-Condition `
        ($relocationRva -ne 0 -and $relocationSize -ne 0) `
        'Probe EFI has no non-empty base-relocation directory.'
}

function Test-PathContainedBy {
    param([string] $Root, [string] $Candidate)
    $rootPrefix = $Root.TrimEnd(
        [System.IO.Path]::DirectorySeparatorChar,
        [System.IO.Path]::AltDirectorySeparatorChar
    ) + [System.IO.Path]::DirectorySeparatorChar
    return (
        [System.StringComparer]::OrdinalIgnoreCase.Equals($Root, $Candidate) -or
        $Candidate.StartsWith($rootPrefix, [System.StringComparison]::OrdinalIgnoreCase)
    )
}

function Get-ContainedChildPath {
    param([string] $Root, [string] $Name)
    Assert-Condition (-not [System.IO.Path]::IsPathRooted($Name)) "Bundle child path is rooted: '$Name'."
    Assert-Condition ($Name -ceq [System.IO.Path]::GetFileName($Name)) "Bundle child path is not a basename: '$Name'."
    $candidate = [System.IO.Path]::GetFullPath((Join-Path $Root $Name))
    Assert-Condition (Test-PathContainedBy $Root $candidate) "Bundle child path escaped its root: '$candidate'."
    return $candidate
}

function Assert-ExactM0aFileSet {
    param([string] $Bundle)
    $expected = @(
        'collect-windows.ps1',
        'manifest.json',
        'target-profile-v1.schema.json',
        'target-profile.json',
        'verify-bundle.ps1'
    )
    $set = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
    foreach ($name in $expected) { [void] $set.Add($name) }
    $items = @(Get-ChildItem -Force -LiteralPath $Bundle)
    Assert-Condition ($items.Count -eq $expected.Count) 'The M0a bundle must contain exactly five files.'
    foreach ($item in $items) {
        Assert-Condition (-not [bool] $item.PSIsContainer) "The M0a bundle contains a directory: '$($item.Name)'."
        Assert-Condition (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -eq 0) "The M0a bundle contains a reparse point: '$($item.Name)'."
        Assert-Condition ($set.Contains($item.Name)) "Unexpected M0a bundle file: '$($item.Name)'."
    }
}

function Invoke-M0aVerification {
    param([string] $Bundle)
    Assert-ExactM0aFileSet $Bundle
    $toolsRoot = Split-Path -Parent $PSScriptRoot
    $currentVerifier = Get-ExistingLeafFile `
        (Join-Path $toolsRoot 'target-profile\verify-bundle.ps1') `
        'Current repository M0a verifier'
    $copiedVerifier = Get-ExistingLeafFile `
        (Join-Path $Bundle 'verify-bundle.ps1') `
        'Copied M0a verifier'
    & $currentVerifier -BundleDirectory $Bundle
    & $copiedVerifier -BundleDirectory $Bundle
}

function Assert-StrictFalse {
    param($Value, [string] $Name)
    Assert-Condition `
        ($Value -is [bool] -and $Value -eq $false) `
        "Raw evidence field '$Name' must be the JSON Boolean false."
}

$rawEvidenceFile = Get-ExistingLeafFile $RawEvidencePath 'Raw M0b evidence'
$m0aBundle = Get-ExistingDirectory $TargetProfileBundle 'M0a target-profile bundle'
$probeEfi = Get-ExistingLeafFile $ProbeEfiPath 'Probe EFI'
Assert-Condition `
    ([System.StringComparer]::OrdinalIgnoreCase.Equals([System.IO.Path]::GetExtension($probeEfi), '.efi')) `
    "ProbeEfiPath must identify an .efi file: '$probeEfi'."
Assert-ProbeEfiApplication -LiteralPath $probeEfi

Invoke-M0aVerification $m0aBundle

$m0aManifestPath = Get-ExistingLeafFile `
    (Join-Path $m0aBundle 'manifest.json') `
    'M0a manifest'
$m0aManifestSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $m0aManifestPath).Hash.ToLowerInvariant()
Assert-Condition ($m0aManifestSha256 -cmatch '^[0-9a-f]{64}$') 'Computed M0a manifest SHA-256 is not canonical.'

$rawEvidenceItem = Get-Item -Force -LiteralPath $rawEvidenceFile
Assert-Condition ($rawEvidenceItem.Length -le 16777216) 'Raw evidence exceeds the 16 MiB pre-parse cap.'
$rawEvidenceText = Get-Content -Raw -Encoding UTF8 -LiteralPath $rawEvidenceFile
Assert-NoDuplicateJsonPropertyNames $rawEvidenceText 'raw evidence'
$rawEvidence = $rawEvidenceText | ConvertFrom-Json
$evidenceSchemaVersion = Assert-JsonSchemaVersion $rawEvidence.schema_version
$schemaName = "m0b-probe-v$evidenceSchemaVersion.schema.json"
$expectedEvidenceKind = if ($evidenceSchemaVersion -ge 3) {
    'uefi-record-only-inventory-slice'
}
else {
    'uefi-read-only-inventory-slice'
}
Assert-Condition (Test-OrdinalEqual $rawEvidence.evidence_kind $expectedEvidenceKind) 'Unexpected raw evidence kind.'
Assert-Condition (Test-OrdinalEqual $rawEvidence.qualification_status 'blocked') 'Raw evidence must remain qualification_status=blocked.'
foreach ($field in @(
    'launch_authorized',
    'physical_candidate_flash_authorized',
    'control_state_writes_authorized',
    'process_introspection_authorized',
    'confidential_vm_claim'
)) {
    Assert-StrictFalse $rawEvidence.$field $field
}
if ($evidenceSchemaVersion -ge 2) {
    foreach ($field in @('amd_iommu_ownership_claim', 'pci_isolation_claim')) {
        Assert-StrictFalse $rawEvidence.$field $field
    }
}
Assert-Condition `
    ($rawEvidence.target_profile_manifest_sha256 -is [string] -and
        $rawEvidence.target_profile_manifest_sha256 -cmatch '^[0-9a-f]{64}$') `
    'Raw evidence target-profile binding is not canonical lowercase SHA-256.'
Assert-Condition `
    (Test-OrdinalEqual $rawEvidence.target_profile_manifest_sha256 $m0aManifestSha256) `
    'Raw evidence is bound to the wrong target-profile manifest.'
Assert-Condition `
    ($rawEvidence.sink.removable_media -is [bool] -and $rawEvidence.sink.removable_media) `
    'Raw evidence does not report a removable sink.'
Assert-Condition `
    ($rawEvidence.sink.media_present -is [bool] -and $rawEvidence.sink.media_present) `
    'Raw evidence does not report present media.'
Assert-Condition `
    ($rawEvidence.sink.read_only -is [bool] -and $rawEvidence.sink.read_only -eq $false) `
    'Raw evidence reports a read-only sink or a non-Boolean read_only field.'
Assert-Condition `
    ($rawEvidence.sink.output_file -is [string] -and
        $rawEvidence.sink.output_file -cmatch '^\\svmvisor-m0b-[0-9]{8}T[0-9]{6}-[0-9]{9}\.json$') `
    'Raw evidence sink.output_file is not canonical.'
$rawEvidenceFilename = [System.IO.Path]::GetFileName($rawEvidenceFile)
Assert-Condition `
    (Test-OrdinalEqual $rawEvidenceFilename $rawEvidence.sink.output_file.Substring(1)) `
    'Raw evidence input filename does not match sink.output_file.'

$output = [System.IO.Path]::GetFullPath($OutputDirectory)
Assert-Condition (-not (Test-Path -LiteralPath $output)) "Refusing to overwrite an existing output path: '$output'."
$outputParentPath = [System.IO.Path]::GetDirectoryName($output)
Assert-Condition (-not [string]::IsNullOrWhiteSpace($outputParentPath)) 'OutputDirectory must have an existing parent directory.'
$outputParent = Get-ExistingDirectory $outputParentPath 'Output parent'
Assert-Condition `
    (-not (Test-PathContainedBy $m0aBundle $output)) `
    'OutputDirectory must not be inside the immutable M0a bundle.'
foreach ($inputPath in @($rawEvidenceFile, $probeEfi, $m0aBundle)) {
    Assert-Condition `
        (-not (Test-PathContainedBy $output $inputPath)) `
        'An input path must not be contained by OutputDirectory.'
}

$stagingName = '.svmvisor-m0b-staging-' + [guid]::NewGuid().ToString('N')
$staging = Get-ContainedChildPath $outputParent $stagingName
Assert-Condition (-not (Test-Path -LiteralPath $staging)) "Staging path already exists: '$staging'."

$toolFiles = @(
    'finalize-evidence.ps1',
    $schemaName,
    'prepare-media.ps1',
    'verify-bundle.ps1'
)
foreach ($name in $toolFiles) {
    [void] (Get-ExistingLeafFile (Join-Path $PSScriptRoot $name) "M0b tool '$name'")
}

$stagingCreated = $false
$movedToFinal = $false
try {
    [void] [System.IO.Directory]::CreateDirectory($staging)
    $stagingCreated = $true

    foreach ($name in $toolFiles) {
        [System.IO.File]::Copy(
            (Join-Path $PSScriptRoot $name),
            (Get-ContainedChildPath $staging $name),
            $false
        )
    }
    [System.IO.File]::Copy($rawEvidenceFile, (Get-ContainedChildPath $staging 'raw-probe.json'), $false)
    [System.IO.File]::Copy($probeEfi, (Get-ContainedChildPath $staging 'svmvisor-m0b-probe.efi'), $false)
    [System.IO.File]::Copy($m0aManifestPath, (Get-ContainedChildPath $staging 'target-profile-manifest.json'), $false)

    $manifestedNames = @(
        'finalize-evidence.ps1',
        $schemaName,
        'prepare-media.ps1',
        'raw-probe.json',
        'svmvisor-m0b-probe.efi',
        'target-profile-manifest.json',
        'verify-bundle.ps1'
    )
    $manifestFiles = @(
        foreach ($name in ($manifestedNames | Sort-Object)) {
            $path = Get-ContainedChildPath $staging $name
            $item = Get-Item -Force -LiteralPath $path
            [pscustomobject] [ordered] @{
                path = $name
                size_bytes = [long] $item.Length
                sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash.ToLowerInvariant()
            }
        }
    )
    $manifest = [pscustomobject] [ordered] @{
        schema_version = $evidenceSchemaVersion
        bundle_kind = 'svmvisor-m0b-probe-evidence'
        created_at_utc = [DateTime]::UtcNow.ToString('o', [System.Globalization.CultureInfo]::InvariantCulture)
        target_profile_manifest_sha256 = $m0aManifestSha256
        raw_evidence_filename = $rawEvidenceFilename
        files = $manifestFiles
    }
    $manifestJson = $manifest | ConvertTo-Json -Depth 6
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText(
        (Get-ContainedChildPath $staging 'manifest.json'),
        $manifestJson + "`n",
        $utf8NoBom
    )

    $stagedVerifier = Get-ContainedChildPath $staging 'verify-bundle.ps1'
    & $stagedVerifier -BundleDirectory $staging

    [System.IO.Directory]::Move($staging, $output)
    $movedToFinal = $true
    $stagingCreated = $false
}
catch {
    if ($stagingCreated -and (Test-Path -LiteralPath $staging -PathType Container)) {
        $resolvedStaging = (Get-Item -Force -LiteralPath $staging).FullName
        Assert-Condition `
            (Test-PathContainedBy $outputParent $resolvedStaging) `
            "Refusing to clean an escaped staging path: '$resolvedStaging'."
        Remove-Item -Recurse -Force -LiteralPath $resolvedStaging
    }
    throw
}

Assert-Condition $movedToFinal 'Internal error: the verified bundle was not finalized.'
$finalVerifier = Get-ContainedChildPath $output 'verify-bundle.ps1'
& $finalVerifier -BundleDirectory $output

$probeEfiSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $output 'svmvisor-m0b-probe.efi')).Hash.ToLowerInvariant()
$rawEvidenceSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $output 'raw-probe.json')).Hash.ToLowerInvariant()
Write-Host "M0b evidence finalization: PASS ($output)"
Write-Host "M0a manifest SHA-256: $m0aManifestSha256"
Write-Host "Probe EFI SHA-256: $probeEfiSha256"
Write-Host "Raw evidence SHA-256: $rawEvidenceSha256"
