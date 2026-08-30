<#
.SYNOPSIS
Regression tests for duplicate target-profile semantic and raw JSON keys.

.DESCRIPTION
Builds an isolated, internally consistent evidence bundle and verifies the
valid baseline. It then requires rejection of a reused reference key and of
escape-equivalent duplicate JSON object names in both the profile and manifest,
without reading or modifying any existing evidence bundle.
#>

[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Write-JsonUtf8NoBom {
    param(
        [Parameter(Mandatory)] [object] $Value,
        [Parameter(Mandatory)] [string] $Path
    )

    $encoding = New-Object System.Text.UTF8Encoding($false)
    $json = $Value | ConvertTo-Json -Depth 100
    [System.IO.File]::WriteAllText(
        $Path,
        $json + [Environment]::NewLine,
        $encoding
    )
}

function Write-TestManifest {
    param(
        [Parameter(Mandatory)] [string] $BundleDirectory,
        [Parameter(Mandatory)] [string] $RunId
    )

    $files = @(
        'collect-windows.ps1',
        'target-profile-v1.schema.json',
        'target-profile.json',
        'verify-bundle.ps1'
    ) | Sort-Object | ForEach-Object {
        $item = Get-Item -LiteralPath (Join-Path $BundleDirectory $_)
        [pscustomobject] [ordered] @{
            path = $item.Name
            size_bytes = [long] $item.Length
            sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $item.FullName).Hash.ToLowerInvariant()
        }
    }
    $manifest = [pscustomobject] [ordered] @{
        schema_version = 1
        run_id = $RunId
        created_at_utc = '2000-01-01T00:00:00.0000000Z'
        files = @($files)
    }
    Write-JsonUtf8NoBom `
        -Value $manifest `
        -Path (Join-Path $BundleDirectory 'manifest.json')
}

function Get-VerifierFailure {
    param([Parameter(Mandatory)] [string] $BundleDirectory)

    try {
        & (Join-Path $BundleDirectory 'verify-bundle.ps1') `
            -BundleDirectory $BundleDirectory
    }
    catch {
        return $_.Exception.Message
    }
    return $null
}

$testRoot = Join-Path `
    ([System.IO.Path]::GetTempPath()) `
    ('svmvisor-target-profile-test-' + [Guid]::NewGuid().ToString('N'))
$bundle = Join-Path $testRoot 'bundle'

try {
    $null = New-Item -ItemType Directory -Path $bundle
    foreach ($fileName in @(
        'collect-windows.ps1',
        'target-profile-v1.schema.json',
        'verify-bundle.ps1'
    )) {
        Copy-Item `
            -LiteralPath (Join-Path $PSScriptRoot $fileName) `
            -Destination (Join-Path $bundle $fileName)
    }

    $collectorHash = (
        Get-FileHash `
            -Algorithm SHA256 `
            -LiteralPath (Join-Path $bundle 'collect-windows.ps1')
    ).Hash.ToLowerInvariant()
    $schemaHash = (
        Get-FileHash `
            -Algorithm SHA256 `
            -LiteralPath (Join-Path $bundle 'target-profile-v1.schema.json')
    ).Hash.ToLowerInvariant()
    $runId = '20000101T000000000Z'
    $gateIds = @(
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
    $manualGates = @(
        foreach ($gateId in $gateIds) {
            [pscustomobject] [ordered] @{
                id = $gateId
                status = if ($gateId -eq 'source-provenance') { 'blocked' } else { 'unknown' }
                required_before = 'regression test'
                reason = 'Synthetic verifier regression fixture.'
            }
        }
    )
    $references = @(
        [pscustomobject] [ordered] @{
            key = 'AMD-APM2-24593-r3.44'
            kind = 'normative-local'
            publication = '24593'
            revision = '3.44'
            local_path = 'docs\24593_3.44_APM_Vol2.pdf'
            sha256 = '3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c'
            derived_from_sha256 = $null
            url = 'https://docs.amd.com/v/u/en-US/24593_3.44_APM_Vol2'
        }
        [pscustomobject] [ordered] @{
            key = 'AMD-APM2-24593-r3.44-CH05'
            kind = 'derived-navigation'
            publication = '24593'
            revision = '3.44'
            local_path = 'docs\amd64_apm_vol2_markdown\chapters\05-page-translation-and-protection.md'
            sha256 = ('0' * 64)
            derived_from_sha256 = '3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c'
            url = $null
        }
        [pscustomobject] [ordered] @{
            key = 'AMD-APM2-24593-r3.44-CH15'
            kind = 'derived-navigation'
            publication = '24593'
            revision = '3.44'
            local_path = 'docs\amd64_apm_vol2_markdown\chapters\15-secure-virtual-machine.md'
            sha256 = ('1' * 64)
            derived_from_sha256 = '3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c'
            url = $null
        }
        [pscustomobject] [ordered] @{
            key = 'UEFI-2.11'
            kind = 'normative-external-unpinned'
            publication = 'UEFI'
            revision = '2.11'
            local_path = $null
            sha256 = $null
            derived_from_sha256 = $null
            url = 'https://uefi.org/specs/UEFI/2.11/'
        }
    )
    $profile = [pscustomobject] [ordered] @{
        schema_version = 1
        profile_kind = 'windows-read-only-inventory'
        qualification_status = 'blocked'
        run_id = $runId
        captured_at_utc = '2000-01-01T00:00:00.0000000Z'
        collector = [pscustomobject] [ordered] @{
            path = 'tools/target-profile/collect-windows.ps1'
            sha256 = $collectorHash
            schema_path = 'tools/target-profile/target-profile-v1.schema.json'
            schema_sha256 = $schemaHash
            powershell_version = $PSVersionTable.PSVersion.ToString()
            elevated = $false
        }
        source_control = [pscustomobject] [ordered] @{
            kind = 'none'
            revision = $null
            dirty = $null
        }
        scope = [pscustomobject] [ordered] @{
            virtualization_model = 'classic-amd-svm-npt'
            initial_guest_trust = 'trusted-only'
            confidential_vm_claim = $false
            physical_candidate_flash_authorized = $false
            svm_control_writes_authorized = $false
            process_introspection_authorized = $false
            detection_claim = 'agentless and out-of-guest; no undetectability guarantee'
        }
        privacy = [pscustomobject] [ordered] @{
            host_name_included = $false
            system_serial_numbers_included = $false
            processor_ids_included = $false
            bitlocker_recovery_material_included = $false
            pci_instance_ids_included = $false
        }
        platform = [pscustomobject] [ordered] @{
            computer_system = $null
            baseboard = $null
            bios = $null
            processors = @()
        }
        operating_system = $null
        virtualization = [pscustomobject] [ordered] @{
            device_guard = $null
        }
        security = [pscustomobject] [ordered] @{
            secure_boot_enabled = $null
            tpm = $null
        }
        squirrel = [pscustomobject] [ordered] @{
            queried_vendor_id = '10ee'
            queried_device_id = '7021'
            functions = @()
        }
        manual_gates = @($manualGates)
        collection_errors = @()
        references = @($references)
    }
    $profilePath = Join-Path $bundle 'target-profile.json'
    Write-JsonUtf8NoBom -Value $profile -Path $profilePath
    Write-TestManifest -BundleDirectory $bundle -RunId $runId

    & (Join-Path $bundle 'verify-bundle.ps1') -BundleDirectory $bundle

    $profile.references += [pscustomobject] [ordered] @{
        key = 'UEFI-2.11'
        kind = 'informative'
        publication = 'Different publication'
        revision = '1.0'
        local_path = $null
        sha256 = $null
        derived_from_sha256 = $null
        url = 'https://example.invalid/different-reference'
    }
    Write-JsonUtf8NoBom -Value $profile -Path $profilePath
    Write-TestManifest -BundleDirectory $bundle -RunId $runId

    $failureMessage = $null
    try {
        & (Join-Path $bundle 'verify-bundle.ps1') -BundleDirectory $bundle
    } catch {
        $failureMessage = $_.Exception.Message
    }
    if ($failureMessage -ne "Duplicate reference key: 'UEFI-2.11'.") {
        throw "Expected duplicate-reference rejection; verifier returned: '$failureMessage'."
    }

    Write-Host 'Duplicate reference-key regression: PASS'

    $profile.references = @($references)
    Write-JsonUtf8NoBom -Value $profile -Path $profilePath
    Write-TestManifest -BundleDirectory $bundle -RunId $runId
    & (Join-Path $bundle 'verify-bundle.ps1') -BundleDirectory $bundle

    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    $profileText = [System.IO.File]::ReadAllText($profilePath)
    $profilePattern = '"svm_control_writes_authorized"\s*:\s*false'
    if ([regex]::Matches($profileText, $profilePattern).Count -ne 1) {
        throw 'Could not locate the unique profile safety property for the raw duplicate-key test.'
    }
    $profileText = [regex]::Replace(
        $profileText,
        $profilePattern,
        '"svm_control_writes_authorized": true, "\u0073vm_control_writes_authorized": false',
        1
    )
    [System.IO.File]::WriteAllText($profilePath, $profileText, $utf8NoBom)
    Write-TestManifest -BundleDirectory $bundle -RunId $runId

    $failureMessage = Get-VerifierFailure -BundleDirectory $bundle
    if ($failureMessage -notlike "Duplicate JSON object property 'svm_control_writes_authorized'*") {
        throw "Expected duplicate profile-property rejection; verifier returned: '$failureMessage'."
    }
    Write-Host 'Duplicate escaped profile-property regression: PASS'

    Write-JsonUtf8NoBom -Value $profile -Path $profilePath
    Write-TestManifest -BundleDirectory $bundle -RunId $runId
    $manifestPath = Join-Path $bundle 'manifest.json'
    $manifestText = [System.IO.File]::ReadAllText($manifestPath)
    $manifestPattern = '"schema_version"\s*:\s*1'
    if ([regex]::Matches($manifestText, $manifestPattern).Count -ne 1) {
        throw 'Could not locate the unique manifest schema version for the raw duplicate-key test.'
    }
    $manifestText = [regex]::Replace(
        $manifestText,
        $manifestPattern,
        '"schema_version": 999, "\u0073chema_version": 1',
        1
    )
    [System.IO.File]::WriteAllText($manifestPath, $manifestText, $utf8NoBom)

    $failureMessage = Get-VerifierFailure -BundleDirectory $bundle
    if ($failureMessage -notlike "Duplicate JSON object property 'schema_version'*") {
        throw "Expected duplicate manifest-property rejection; verifier returned: '$failureMessage'."
    }
    Write-Host 'Duplicate escaped manifest-property regression: PASS'
} finally {
    if (Test-Path -LiteralPath $testRoot) {
        $resolvedTestRoot = (Get-Item -LiteralPath $testRoot).FullName
        $expectedPrefix = [System.IO.Path]::GetFullPath(
            (Join-Path ([System.IO.Path]::GetTempPath()) 'svmvisor-target-profile-test-')
        )
        if ($resolvedTestRoot.StartsWith($expectedPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
            Remove-Item -Recurse -Force -LiteralPath $resolvedTestRoot
        }
    }
}
