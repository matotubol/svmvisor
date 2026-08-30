<#
.SYNOPSIS
Verifies a raw svmvisor target-profile evidence bundle.

.DESCRIPTION
Checks the immutable-file manifest and the fixed safety invariants that every
schema-v1 raw inventory must preserve. This is a self-contained verifier for
Windows PowerShell 5.1; formal qualification additionally requires validation by
a JSON Schema Draft 2020-12 implementation.

.PARAMETER BundleDirectory
Directory containing target-profile.json, its schema, the collector, verifier,
and manifest.
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string] $BundleDirectory
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$expectedAmdApmSha256 = '3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c'
Add-Type -AssemblyName System.Runtime.Serialization

function Assert-Condition {
    param(
        [Parameter(Mandatory)] [bool] $Condition,
        [Parameter(Mandatory)] [string] $Message
    )

    if (-not $Condition) {
        throw $Message
    }
}

function Assert-UniqueJsonObjectNames {
    param(
        [Parameter(Mandatory)] [string] $Path
    )

    # Windows PowerShell 5.1 ConvertFrom-Json silently keeps only one value for
    # duplicate object names. The .NET Framework JSON-to-XML projection retains
    # every member, including escape-equivalent names, so inspect it before any
    # lossy materialization.
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    $reader = [System.Runtime.Serialization.Json.JsonReaderWriterFactory]::CreateJsonReader(
        $bytes,
        [System.Xml.XmlDictionaryReaderQuotas]::Max
    )
    try {
        $document = New-Object System.Xml.XmlDocument
        $document.Load($reader)
    }
    finally {
        $reader.Dispose()
    }

    $pending = New-Object System.Collections.Queue
    $pending.Enqueue($document.DocumentElement)
    while ($pending.Count -gt 0) {
        $element = $pending.Dequeue()
        if ($element.GetAttribute('type') -eq 'object') {
            $names = [System.Collections.Generic.HashSet[string]]::new(
                [System.StringComparer]::Ordinal
            )

            # WCF represents a leading JSON "__type" property as an XML
            # attribute rather than a child element.
            if ($element.HasAttribute('__type')) {
                [void] $names.Add('__type')
            }

            foreach ($child in $element.ChildNodes) {
                if ($child -isnot [System.Xml.XmlElement]) {
                    continue
                }
                $name = if (
                    $child.LocalName -eq 'item' -and
                    $child.NamespaceURI -eq 'item'
                ) {
                    $child.GetAttribute('item')
                }
                else {
                    $child.LocalName
                }
                Assert-Condition `
                    -Condition ($names.Add($name)) `
                    -Message "Duplicate JSON object property '$name' in '$Path'."
            }
        }

        foreach ($child in $element.ChildNodes) {
            if ($child -is [System.Xml.XmlElement]) {
                $pending.Enqueue($child)
            }
        }
    }
}

$bundleItem = Get-Item -Force -LiteralPath $BundleDirectory
Assert-Condition `
    -Condition ([bool] $bundleItem.PSIsContainer) `
    -Message "Bundle path is not a directory: '$BundleDirectory'."
Assert-Condition `
    -Condition (($bundleItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -eq 0) `
    -Message "Bundle directory must not be a reparse point: '$BundleDirectory'."
$bundle = $bundleItem.FullName
$profilePath = Join-Path $bundle 'target-profile.json'
$schemaPath = Join-Path $bundle 'target-profile-v1.schema.json'
$collectorPath = Join-Path $bundle 'collect-windows.ps1'
$verifierPath = Join-Path $bundle 'verify-bundle.ps1'
$manifestPath = Join-Path $bundle 'manifest.json'
$expectedBundleFiles = @(
    'collect-windows.ps1',
    'manifest.json',
    'target-profile-v1.schema.json',
    'target-profile.json',
    'verify-bundle.ps1'
)
$bundleItems = @(Get-ChildItem -Force -LiteralPath $bundle)
foreach ($item in $bundleItems) {
    Assert-Condition `
        -Condition (-not [bool] $item.PSIsContainer) `
        -Message "Bundle contains an unexpected subdirectory: '$($item.Name)'."
    Assert-Condition `
        -Condition (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -eq 0) `
        -Message "Bundle contains a reparse point: '$($item.Name)'."
}
$actualBundleFiles = @($bundleItems | ForEach-Object { $_.Name })
$bundleDifference = @(
    Compare-Object `
        -ReferenceObject ($expectedBundleFiles | Sort-Object) `
        -DifferenceObject ($actualBundleFiles | Sort-Object)
)
Assert-Condition `
    -Condition (
        $actualBundleFiles.Count -eq $expectedBundleFiles.Count -and
        $bundleDifference.Count -eq 0
    ) `
    -Message 'Bundle must contain exactly the five schema-v1 evidence files.'
foreach ($path in @(
    $profilePath,
    $schemaPath,
    $collectorPath,
    $verifierPath,
    $manifestPath
)) {
    Assert-Condition `
        -Condition (Test-Path -LiteralPath $path -PathType Leaf) `
        -Message "Required bundle file is missing: '$path'."
}

foreach ($jsonPath in @($profilePath, $manifestPath, $schemaPath)) {
    Assert-UniqueJsonObjectNames -Path $jsonPath
}

$profile = Get-Content -Raw -Encoding UTF8 -LiteralPath $profilePath |
    ConvertFrom-Json
$manifest = Get-Content -Raw -Encoding UTF8 -LiteralPath $manifestPath |
    ConvertFrom-Json

$referenceKeys = [System.Collections.Generic.HashSet[string]]::new(
    [System.StringComparer]::Ordinal
)
foreach ($reference in @($profile.references)) {
    Assert-Condition `
        -Condition ($referenceKeys.Add([string] $reference.key)) `
        -Message "Duplicate reference key: '$($reference.key)'."
}

Assert-Condition ($profile.schema_version -eq 1) 'Unexpected profile schema version.'
Assert-Condition `
    ($profile.profile_kind -eq 'windows-read-only-inventory') `
    'Unexpected profile kind.'
Assert-Condition `
    ($profile.qualification_status -eq 'blocked') `
    'A raw inventory must remain qualification_status=blocked.'
Assert-Condition `
    ($profile.scope.virtualization_model -eq 'classic-amd-svm-npt') `
    'Unexpected virtualization scope.'
Assert-Condition `
    ($profile.scope.initial_guest_trust -eq 'trusted-only') `
    'A raw inventory may authorize only trusted bring-up.'
Assert-Condition `
    (
        $profile.scope.confidential_vm_claim -is [bool] -and
        $profile.scope.confidential_vm_claim -eq $false
    ) `
    'A raw inventory cannot make a confidential-VM claim.'
Assert-Condition `
    (
        $profile.scope.physical_candidate_flash_authorized -is [bool] -and
        $profile.scope.physical_candidate_flash_authorized -eq $false
    ) `
    'A raw inventory cannot authorize candidate flashing.'
Assert-Condition `
    (
        $profile.scope.svm_control_writes_authorized -is [bool] -and
        $profile.scope.svm_control_writes_authorized -eq $false
    ) `
    'A raw inventory cannot authorize SVM control writes.'
Assert-Condition `
    (
        $profile.scope.process_introspection_authorized -is [bool] -and
        $profile.scope.process_introspection_authorized -eq $false
    ) `
    'A raw inventory cannot authorize process introspection.'
Assert-Condition `
    ($profile.scope.detection_claim -eq 'agentless and out-of-guest; no undetectability guarantee') `
    'Unexpected detection claim.'

foreach ($privacyField in @(
    'host_name_included',
    'system_serial_numbers_included',
    'processor_ids_included',
    'bitlocker_recovery_material_included'
)) {
    Assert-Condition `
        (
            $profile.privacy.$privacyField -is [bool] -and
            $profile.privacy.$privacyField -eq $false
        ) `
        "Forbidden privacy field is enabled: '$privacyField'."
}
$pciInstanceIdsIncluded = $profile.privacy.pci_instance_ids_included
Assert-Condition `
    ($pciInstanceIdsIncluded -is [bool]) `
    'PCI instance-ID privacy flag must be a Boolean.'
if ($pciInstanceIdsIncluded -eq $false) {
    foreach ($function in @($profile.squirrel.functions)) {
        Assert-Condition `
            ($null -eq $function.pnp_device_id) `
            'Raw PCI instance ID is present without explicit opt-in.'
    }
}

$expectedGateIds = @(
    'source-provenance',
    'exact-amd-cpuid-svm-npt',
    'processor-ppr-and-revision-guide',
    'amd-iommu-and-slot-isolation',
    'complete-disk-recovery',
    'bitlocker-recovery-key-offline',
    'card-bypass-electrical-design',
    'ebs-persistent-direct-watchdog',
    'durable-attempt-lease',
    'secure-boot-option-rom-policy',
    'firmware-event-ordering',
    'stage1-storage-and-rom-capacity'
)
$actualGateIds = @($profile.manual_gates | ForEach-Object { $_.id })
Assert-Condition `
    ($actualGateIds.Count -eq $expectedGateIds.Count) `
    'Raw inventory has the wrong number of safety gates.'
$gateDifference = @(
    Compare-Object `
        -ReferenceObject ($expectedGateIds | Sort-Object) `
        -DifferenceObject ($actualGateIds | Sort-Object)
)
Assert-Condition `
    ($gateDifference.Count -eq 0) `
    'Raw inventory safety-gate IDs do not match schema v1.'

foreach ($gate in @($profile.manual_gates)) {
    if ($gate.id -eq 'source-provenance') {
        Assert-Condition `
            ($gate.status -in @('blocked', 'verified')) `
            'Source-provenance gate must be blocked or verified.'
    } else {
        Assert-Condition `
            ($gate.status -eq 'unknown') `
            "Raw inventory gate '$($gate.id)' must remain unknown."
    }
}

$sourceGate = $profile.manual_gates |
    Where-Object id -eq 'source-provenance' |
    Select-Object -First 1
Assert-Condition `
    ($profile.source_control.kind -in @('none', 'git')) `
    'Unexpected source-control kind.'
if ($profile.source_control.kind -eq 'none') {
    Assert-Condition `
        ($null -eq $profile.source_control.revision) `
        'Source-control revision must be null when kind=none.'
    Assert-Condition `
        ($null -eq $profile.source_control.dirty) `
        'Source-control dirty flag must be null when kind=none.'
} else {
    Assert-Condition `
        (
            $null -eq $profile.source_control.revision -or
            (
                $profile.source_control.revision -is [string] -and
                $profile.source_control.revision -match '^(?:[0-9a-f]{40}|[0-9a-f]{64})$'
            )
        ) `
        'Git revision must be a lowercase object ID or null after a failed probe.'
    Assert-Condition `
        (
            $null -eq $profile.source_control.dirty -or
            $profile.source_control.dirty -is [bool]
        ) `
        'Git dirty flag must be a Boolean or null after a failed probe.'
}
$cleanGitRevisionRecorded = (
    $profile.source_control.kind -eq 'git' -and
    $profile.source_control.revision -is [string] -and
    $profile.source_control.revision -match '^(?:[0-9a-f]{40}|[0-9a-f]{64})$' -and
    $profile.source_control.dirty -is [bool] -and
    $profile.source_control.dirty -eq $false
)
$expectedSourceGateStatus = if ($cleanGitRevisionRecorded) { 'verified' } else { 'blocked' }
Assert-Condition `
    ($sourceGate.status -eq $expectedSourceGateStatus) `
    'Source-provenance gate is inconsistent with the recorded source state.'
if ($cleanGitRevisionRecorded) {
    Assert-Condition `
        ($profile.source_control.kind -eq 'git') `
        'Source provenance is verified without Git.'
    Assert-Condition `
        ($profile.source_control.revision -match '^(?:[0-9a-f]{40}|[0-9a-f]{64})$') `
        'Source provenance is verified without a valid Git revision.'
    Assert-Condition `
        ($profile.source_control.dirty -eq $false) `
        'Source provenance is verified for a dirty or unknown worktree.'
}

$amdApm = $profile.references |
    Where-Object key -eq 'AMD-APM2-24593-r3.44' |
    Select-Object -First 1
Assert-Condition ($null -ne $amdApm) 'Pinned AMD APM reference is missing.'
Assert-Condition `
    ($amdApm.kind -eq 'normative-local') `
    'AMD APM PDF must be the normative local reference.'
Assert-Condition `
    ($amdApm.sha256 -eq $expectedAmdApmSha256) `
    'AMD APM PDF does not match the pinned rev. 3.44 hash.'
foreach ($key in @(
    'AMD-APM2-24593-r3.44-CH05',
    'AMD-APM2-24593-r3.44-CH15'
)) {
    $derived = $profile.references | Where-Object key -eq $key | Select-Object -First 1
    Assert-Condition ($null -ne $derived) "Derived reference '$key' is missing."
    Assert-Condition `
        ($derived.kind -eq 'derived-navigation') `
        "Reference '$key' is not labeled as derivative."
    Assert-Condition `
        ($derived.derived_from_sha256 -eq $amdApm.sha256) `
        "Reference '$key' is not bound to the AMD APM PDF hash."
}

Assert-Condition ($manifest.schema_version -eq 1) 'Unexpected manifest schema version.'
Assert-Condition ($manifest.run_id -eq $profile.run_id) 'Manifest/profile run IDs differ.'
$expectedManifestFiles = @(
    'collect-windows.ps1',
    'target-profile-v1.schema.json',
    'target-profile.json',
    'verify-bundle.ps1'
)
$actualManifestFiles = @($manifest.files | ForEach-Object { $_.path })
$manifestDifference = @(
    Compare-Object `
        -ReferenceObject ($expectedManifestFiles | Sort-Object) `
        -DifferenceObject ($actualManifestFiles | Sort-Object)
)
Assert-Condition `
    (
        $actualManifestFiles.Count -eq $expectedManifestFiles.Count -and
        $manifestDifference.Count -eq 0
    ) `
    'Manifest file set does not match schema v1.'

foreach ($file in $manifest.files) {
    Assert-Condition `
        ($file.path -eq [System.IO.Path]::GetFileName($file.path)) `
        "Manifest path is not a bundle-local file name: '$($file.path)'."
    $path = Join-Path $bundle $file.path
    Assert-Condition `
        (Test-Path -LiteralPath $path -PathType Leaf) `
        "Manifest file is missing: '$($file.path)'."
    $item = Get-Item -LiteralPath $path
    Assert-Condition `
        ([long] $file.size_bytes -eq [long] $item.Length) `
        "Manifest size mismatch: '$($file.path)'."
    $actualHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash.ToLowerInvariant()
    Assert-Condition `
        ($actualHash -eq $file.sha256) `
        "Manifest hash mismatch: '$($file.path)'."
}

$collectorHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $collectorPath).Hash.ToLowerInvariant()
$schemaHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $schemaPath).Hash.ToLowerInvariant()
Assert-Condition `
    ($collectorHash -eq $profile.collector.sha256) `
    'Copied collector does not match the profile hash.'
Assert-Condition `
    ($schemaHash -eq $profile.collector.schema_sha256) `
    'Copied schema does not match the profile hash.'

Write-Host "Target-profile bundle verification: PASS ($bundle)"
