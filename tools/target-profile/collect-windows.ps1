<#
.SYNOPSIS
Collects a read-only Windows platform profile for svmvisor Milestone 0.

.DESCRIPTION
The collector records facts that Windows exposes without installing a driver or
changing existing firmware, boot, BitLocker, OS, or virtualization state. Its
only write is a new evidence directory containing a versioned JSON profile, a
copy of its JSON Schema and verifier, and a SHA-256 manifest beneath
target/evidence by default.

The result is intentionally incomplete. Facts that require a UEFI probe,
hardware documentation, recovery exercise, or physical inspection remain
explicit manual gates instead of being guessed.

.PARAMETER OutputDirectory
Optional new directory for the evidence bundle. The collector refuses to
overwrite an existing directory.

.PARAMETER PciVendorId
Four hexadecimal digits used to find the current Squirrel PCI function.

.PARAMETER PciDeviceId
Four hexadecimal digits used to find the current Squirrel PCI function.

.PARAMETER IncludePciInstanceId
Includes the raw Windows PCI instance identifier. By default, only its SHA-256
digest is retained because the raw value is stable identifying data.

.LINK
https://learn.microsoft.com/en-us/windows/win32/cimwin32prov/win32-processor

.LINK
https://learn.microsoft.com/en-us/windows/win32/cimwin32prov/win32-bios

.LINK
https://learn.microsoft.com/en-us/windows/win32/cimwin32prov/win32-baseboard

.LINK
https://learn.microsoft.com/en-us/powershell/module/secureboot/confirm-securebootuefi
#>

[CmdletBinding()]
param(
    [string] $OutputDirectory,

    [ValidatePattern('^[0-9A-Fa-f]{4}$')]
    [string] $PciVendorId = '10ee',

    [ValidatePattern('^[0-9A-Fa-f]{4}$')]
    [string] $PciDeviceId = '0666',

    [switch] $IncludePciInstanceId
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$workspaceRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..')).Path
$schemaPath = Join-Path $PSScriptRoot 'target-profile-v1.schema.json'
$verifierPath = Join-Path $PSScriptRoot 'verify-bundle.ps1'
$expectedAmdApmSha256 = '3d9dcb3f68222392d0ede9970efc95e31a047a247d54b454123d6981d278c48c'
foreach ($requiredToolFile in @($schemaPath, $verifierPath)) {
    if (-not (Test-Path -LiteralPath $requiredToolFile -PathType Leaf)) {
        throw "Target-profile tool file is missing: '$requiredToolFile'."
    }
}
$amdApmPath = Join-Path $workspaceRoot 'docs\24593_3.44_APM_Vol2.pdf'
if (-not (Test-Path -LiteralPath $amdApmPath -PathType Leaf)) {
    throw "Pinned AMD APM rev. 3.44 PDF is missing: '$amdApmPath'."
}
$actualAmdApmSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $amdApmPath).Hash.ToLowerInvariant()
if ($actualAmdApmSha256 -ne $expectedAmdApmSha256) {
    throw "Pinned AMD APM rev. 3.44 PDF hash mismatch."
}

$collectionErrors = [System.Collections.Generic.List[object]]::new()

function Add-CollectionError {
    param(
        [Parameter(Mandatory)] [string] $Probe,
        [Parameter(Mandatory)] [System.Management.Automation.ErrorRecord] $ErrorRecord
    )

    $exceptionType = $ErrorRecord.Exception.GetType().FullName
    $message = $ErrorRecord.Exception.Message
    $code = if (
        $ErrorRecord.Exception -is [System.UnauthorizedAccessException] -or
        $message -match '(?i)access.*denied|administrator privilege|required privilege'
    ) {
        'access-denied'
    } elseif ($ErrorRecord.FullyQualifiedErrorId -match 'CommandNotFound') {
        'command-unavailable'
    } else {
        'probe-failed'
    }

    [void] $collectionErrors.Add([pscustomobject] [ordered] @{
        probe = $Probe
        code = $code
        exception_type = $exceptionType
    })
}

function Invoke-ReadOnlyProbe {
    param(
        [Parameter(Mandatory)] [string] $Name,
        [Parameter(Mandatory)] [scriptblock] $Action
    )

    try {
        & $Action
    }
    catch {
        Add-CollectionError -Probe $Name -ErrorRecord $_
        return $null
    }
}

function ConvertTo-NullableString {
    param([AllowNull()] [object] $Value)

    if ($null -eq $Value) {
        return $null
    }
    return [string] $Value
}

function ConvertTo-NullableInteger {
    param([AllowNull()] [object] $Value)

    if ($null -eq $Value) {
        return $null
    }
    return [long] $Value
}

function ConvertTo-NullableBoolean {
    param([AllowNull()] [object] $Value)

    if ($null -eq $Value) {
        return $null
    }
    return [bool] $Value
}

function ConvertTo-UtcText {
    param([AllowNull()] [object] $Value)

    if ($null -eq $Value) {
        return $null
    }

    try {
        return ([datetime] $Value).ToUniversalTime().ToString('o')
    }
    catch {
        return [string] $Value
    }
}

function Get-StringSha256 {
    param([Parameter(Mandatory)] [string] $Value)

    $bytes = [System.Text.Encoding]::UTF8.GetBytes($Value)
    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    try {
        return ([BitConverter]::ToString($sha256.ComputeHash($bytes))).Replace('-', '').ToLowerInvariant()
    }
    finally {
        $sha256.Dispose()
    }
}

function Write-Utf8Json {
    param(
        [Parameter(Mandatory)] [object] $Value,
        [Parameter(Mandatory)] [string] $Path
    )

    $json = $Value | ConvertTo-Json -Depth 12
    [System.IO.File]::WriteAllText(
        $Path,
        $json + [Environment]::NewLine,
        [System.Text.UTF8Encoding]::new($false)
    )
}

function New-LocalReference {
    param(
        [Parameter(Mandatory)] [string] $Key,
        [Parameter(Mandatory)] [string] $Kind,
        [Parameter(Mandatory)] [string] $Publication,
        [Parameter(Mandatory)] [string] $Revision,
        [Parameter(Mandatory)] [string] $RelativePath,
        [AllowNull()] [string] $DerivedFromSha256,
        [AllowNull()] [string] $Url
    )

    $path = Join-Path $workspaceRoot $RelativePath
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Pinned local reference is missing: '$path'."
    }

    return [pscustomobject] [ordered] @{
        key = $Key
        kind = $Kind
        publication = $Publication
        revision = $Revision
        local_path = $RelativePath.Replace('\', '/')
        sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash.ToLowerInvariant()
        derived_from_sha256 = if ([string]::IsNullOrWhiteSpace($DerivedFromSha256)) {
            $null
        } else {
            $DerivedFromSha256
        }
        url = if ([string]::IsNullOrWhiteSpace($Url)) {
            $null
        } else {
            $Url
        }
    }
}

$capturedAtUtc = [DateTime]::UtcNow
$runId = $capturedAtUtc.ToString('yyyyMMddTHHmmssfffZ')
if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $OutputDirectory = Join-Path $workspaceRoot "target\evidence\target-profile-$runId"
} else {
    $OutputDirectory = [System.IO.Path]::GetFullPath($OutputDirectory)
}

if (Test-Path -LiteralPath $OutputDirectory) {
    throw "Refusing to overwrite existing evidence directory '$OutputDirectory'."
}
[void] (New-Item -ItemType Directory -Path $OutputDirectory)

$processors = @(
    Invoke-ReadOnlyProbe -Name 'Win32_Processor' -Action {
        Get-CimInstance -ClassName Win32_Processor -ErrorAction Stop |
            Sort-Object -Property DeviceID |
            ForEach-Object {
                [pscustomobject] [ordered] @{
                    device_id = ConvertTo-NullableString $_.DeviceID
                    manufacturer = ConvertTo-NullableString $_.Manufacturer
                    name = ConvertTo-NullableString $_.Name
                    description = ConvertTo-NullableString $_.Description
                    family = ConvertTo-NullableInteger $_.Family
                    revision = ConvertTo-NullableInteger $_.Revision
                    stepping = ConvertTo-NullableString $_.Stepping
                    socket = ConvertTo-NullableString $_.SocketDesignation
                    address_width_bits = ConvertTo-NullableInteger $_.AddressWidth
                    data_width_bits = ConvertTo-NullableInteger $_.DataWidth
                    cores = ConvertTo-NullableInteger $_.NumberOfCores
                    enabled_cores = ConvertTo-NullableInteger $_.NumberOfEnabledCore
                    logical_processors = ConvertTo-NullableInteger $_.NumberOfLogicalProcessors
                    vm_monitor_mode_extensions = ConvertTo-NullableBoolean $_.VMMonitorModeExtensions
                    virtualization_firmware_enabled = ConvertTo-NullableBoolean $_.VirtualizationFirmwareEnabled
                    second_level_address_translation = ConvertTo-NullableBoolean $_.SecondLevelAddressTranslationExtensions
                }
            }
    }
)

$baseboard = Invoke-ReadOnlyProbe -Name 'Win32_BaseBoard' -Action {
    $item = Get-CimInstance -ClassName Win32_BaseBoard -ErrorAction Stop |
        Select-Object -First 1
    if ($null -ne $item) {
        [pscustomobject] [ordered] @{
            manufacturer = ConvertTo-NullableString $item.Manufacturer
            product = ConvertTo-NullableString $item.Product
            model = ConvertTo-NullableString $item.Model
            version = ConvertTo-NullableString $item.Version
        }
    }
}

$bios = Invoke-ReadOnlyProbe -Name 'Win32_BIOS' -Action {
    $item = Get-CimInstance -ClassName Win32_BIOS -ErrorAction Stop |
        Select-Object -First 1
    if ($null -ne $item) {
        [pscustomobject] [ordered] @{
            manufacturer = ConvertTo-NullableString $item.Manufacturer
            smbios_bios_version = ConvertTo-NullableString $item.SMBIOSBIOSVersion
            version = ConvertTo-NullableString $item.Version
            release_date_utc = ConvertTo-UtcText $item.ReleaseDate
            smbios_major = ConvertTo-NullableInteger $item.SMBIOSMajorVersion
            smbios_minor = ConvertTo-NullableInteger $item.SMBIOSMinorVersion
            system_bios_major = ConvertTo-NullableInteger $item.SystemBiosMajorVersion
            system_bios_minor = ConvertTo-NullableInteger $item.SystemBiosMinorVersion
        }
    }
}

$computerSystem = Invoke-ReadOnlyProbe -Name 'Win32_ComputerSystem' -Action {
    $item = Get-CimInstance -ClassName Win32_ComputerSystem -ErrorAction Stop |
        Select-Object -First 1
    if ($null -ne $item) {
        [pscustomobject] [ordered] @{
            manufacturer = ConvertTo-NullableString $item.Manufacturer
            model = ConvertTo-NullableString $item.Model
            system_family = ConvertTo-NullableString $item.SystemFamily
            system_sku = ConvertTo-NullableString $item.SystemSKUNumber
            hypervisor_present = ConvertTo-NullableBoolean $item.HypervisorPresent
        }
    }
}

$windows = Invoke-ReadOnlyProbe -Name 'WindowsCurrentVersion' -Action {
    $item = Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
    [pscustomobject] [ordered] @{
        product_name = ConvertTo-NullableString $item.ProductName
        edition_id = ConvertTo-NullableString $item.EditionID
        display_version = ConvertTo-NullableString $item.DisplayVersion
        current_build = ConvertTo-NullableString $item.CurrentBuild
        update_build_revision = ConvertTo-NullableInteger $item.UBR
        installation_type = ConvertTo-NullableString $item.InstallationType
    }
}

$deviceGuard = Invoke-ReadOnlyProbe -Name 'Win32_DeviceGuard' -Action {
    $item = Get-CimInstance `
        -Namespace 'root\Microsoft\Windows\DeviceGuard' `
        -ClassName Win32_DeviceGuard `
        -ErrorAction Stop |
        Select-Object -First 1
    if ($null -ne $item) {
        [pscustomobject] [ordered] @{
            virtualization_based_security_status =
                ConvertTo-NullableInteger $item.VirtualizationBasedSecurityStatus
            security_services_configured = @($item.SecurityServicesConfigured)
            security_services_running = @($item.SecurityServicesRunning)
            required_security_properties = @($item.RequiredSecurityProperties)
            available_security_properties = @($item.AvailableSecurityProperties)
        }
    }
}

$secureBootEnabled = Invoke-ReadOnlyProbe -Name 'Confirm-SecureBootUEFI' -Action {
    [void] (Get-Command -Name Confirm-SecureBootUEFI -ErrorAction Stop)
    [bool] (Confirm-SecureBootUEFI -ErrorAction Stop)
}

$tpm = Invoke-ReadOnlyProbe -Name 'Get-Tpm' -Action {
    [void] (Get-Command -Name Get-Tpm -ErrorAction Stop)
    $item = Get-Tpm -ErrorAction Stop
    if ($null -eq $item.PSObject.Properties['TpmPresent']) {
        throw "Get-Tpm did not return TPM state: $item"
    }
    [pscustomobject] [ordered] @{
        present = ConvertTo-NullableBoolean $item.TpmPresent
        ready = ConvertTo-NullableBoolean $item.TpmReady
        enabled = ConvertTo-NullableBoolean $item.TpmEnabled
        activated = ConvertTo-NullableBoolean $item.TpmActivated
        owned = ConvertTo-NullableBoolean $item.TpmOwned
        auto_provisioning = ConvertTo-NullableString $item.AutoProvisioning
    }
}

$pciNeedle = 'VEN_{0}&DEV_{1}' -f `
    $PciVendorId.ToUpperInvariant(), `
    $PciDeviceId.ToUpperInvariant()
$squirrelFunctions = @(
    Invoke-ReadOnlyProbe -Name 'SquirrelPciFunction' -Action {
        Get-CimInstance `
            -ClassName Win32_PnPEntity `
            -Filter "PNPDeviceID LIKE 'PCI%'" `
            -ErrorAction Stop |
            Where-Object { $_.PNPDeviceID -like "*$pciNeedle*" } |
            Sort-Object -Property PNPDeviceID |
            ForEach-Object {
                [pscustomobject] [ordered] @{
                    name = ConvertTo-NullableString $_.Name
                    status = ConvertTo-NullableString $_.Status
                    pnp_class = ConvertTo-NullableString $_.PNPClass
                    pnp_device_id_sha256 = Get-StringSha256 ([string] $_.PNPDeviceID)
                    pnp_device_id = if ($IncludePciInstanceId) {
                        ConvertTo-NullableString $_.PNPDeviceID
                    } else {
                        $null
                    }
                }
            }
    }
)

$isAdministrator = Invoke-ReadOnlyProbe -Name 'AdministratorRole' -Action {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

$sourceControl = [pscustomobject] [ordered] @{
    kind = 'none'
    revision = $null
    dirty = $null
}
if (Test-Path -LiteralPath (Join-Path $workspaceRoot '.git')) {
    $revision = Invoke-ReadOnlyProbe -Name 'GitRevision' -Action {
        $output = @(& git -C $workspaceRoot rev-parse HEAD 2>&1)
        if ($LASTEXITCODE -ne 0) {
            throw "git rev-parse failed: $($output -join [Environment]::NewLine)"
        }
        return ($output -join [Environment]::NewLine).Trim()
    }
    $statusProbe = Invoke-ReadOnlyProbe -Name 'GitStatus' -Action {
        $output = @(& git -C $workspaceRoot status --porcelain 2>&1)
        if ($LASTEXITCODE -ne 0) {
            throw "git status failed: $($output -join [Environment]::NewLine)"
        }
        return [pscustomobject] @{
            dirty = $output.Count -ne 0
        }
    }
    $sourceControl = [pscustomobject] [ordered] @{
        kind = 'git'
        revision = $revision
        dirty = if ($null -eq $statusProbe) { $null } else { $statusProbe.dirty }
    }
}

$collectorHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $PSCommandPath).Hash.ToLowerInvariant()
$schemaHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $schemaPath).Hash.ToLowerInvariant()

$manualGates = @(
    [pscustomobject] [ordered] @{
        id = 'source-provenance'
        status = if (
            $sourceControl.kind -eq 'git' -and
            $sourceControl.revision -match '^(?:[0-9a-f]{40}|[0-9a-f]{64})$' -and
            $sourceControl.dirty -eq $false
        ) { 'verified' } else { 'blocked' }
        required_before = 'candidate artifact qualification'
        reason = if ($sourceControl.kind -ne 'git') {
            'The workspace root is not a Git worktree; no source revision can be recorded.'
        } elseif ($sourceControl.revision -notmatch '^(?:[0-9a-f]{40}|[0-9a-f]{64})$') {
            'Git was detected, but the revision probe failed or returned an invalid object ID.'
        } elseif ($null -eq $sourceControl.dirty) {
            'The Git revision is known, but the worktree-status probe failed.'
        } elseif ($sourceControl.dirty) {
            'The source revision is known, but the worktree contains uncommitted changes.'
        } else {
            'A clean Git revision is recorded in this profile.'
        }
    }
    [pscustomobject] [ordered] @{
        id = 'exact-amd-cpuid-svm-npt'
        status = 'unknown'
        required_before = 'any SVM control-state write'
        reason = 'Windows CIM is not an architectural CPUID/MSR inventory.'
    }
    [pscustomobject] [ordered] @{
        id = 'processor-ppr-and-revision-guide'
        status = 'unknown'
        required_before = 'hardware SVM implementation'
        reason = 'Must be selected from the exact family, model, and stepping.'
    }
    [pscustomobject] [ordered] @{
        id = 'amd-iommu-and-slot-isolation'
        status = 'unknown'
        required_before = 'untrusted guest execution'
        reason = 'Requires IVRS, requester aliases, firmware ownership, and topology evidence.'
    }
    [pscustomobject] [ordered] @{
        id = 'complete-disk-recovery'
        status = 'unknown'
        required_before = 'experimental physical launch'
        reason = 'Requires a verified clone of GPT, ESP, Windows, and WinRE plus external recovery media.'
    }
    [pscustomobject] [ordered] @{
        id = 'bitlocker-recovery-key-offline'
        status = 'unknown'
        required_before = 'measured-boot or option-ROM experiments'
        reason = 'Record proof only; never store a recovery password in this profile.'
    }
    [pscustomobject] [ordered] @{
        id = 'card-bypass-electrical-design'
        status = 'unknown'
        required_before = 'enumerating candidate flash'
        reason = 'Pin, polarity, latch timing, benign image, and physical indication are not software facts.'
    }
    [pscustomobject] [ordered] @{
        id = 'ebs-persistent-direct-watchdog'
        status = 'unknown'
        required_before = 'persistent launch'
        reason = 'A UEFI boot-services watchdog alone is insufficient.'
    }
    [pscustomobject] [ordered] @{
        id = 'durable-attempt-lease'
        status = 'unknown'
        required_before = 'persistent launch'
        reason = 'UEFI variable versus fixed-function card-local journal remains undecided.'
    }
    [pscustomobject] [ordered] @{
        id = 'secure-boot-option-rom-policy'
        status = 'unknown'
        required_before = 'signed physical candidate'
        reason = 'Enabled state does not identify PK, KEK, db, dbx, or slot option-ROM policy.'
    }
    [pscustomobject] [ordered] @{
        id = 'firmware-event-ordering'
        status = 'unknown'
        required_before = 'ReadyToBoot launch design'
        reason = 'Requires a record-only UEFI probe on the exact target.'
    }
    [pscustomobject] [ordered] @{
        id = 'stage1-storage-and-rom-capacity'
        status = 'unknown'
        required_before = 'payload layout freeze'
        reason = 'The current 4 KiB ROM aperture has insufficient growth room.'
    }
)

$amdApmReference = New-LocalReference `
    -Key 'AMD-APM2-24593-r3.44' `
    -Kind 'normative-local' `
    -Publication '24593' `
    -Revision '3.44' `
    -RelativePath 'docs\24593_3.44_APM_Vol2.pdf' `
    -DerivedFromSha256 $null `
    -Url 'https://docs.amd.com/v/u/en-US/24593_3.44_APM_Vol2'
if ($amdApmReference.sha256 -ne $expectedAmdApmSha256) {
    throw 'AMD APM rev. 3.44 changed during collection; refusing the evidence bundle.'
}
$amdApmChapter5Reference = New-LocalReference `
    -Key 'AMD-APM2-24593-r3.44-CH05' `
    -Kind 'derived-navigation' `
    -Publication '24593' `
    -Revision '3.44' `
    -RelativePath 'docs\amd64_apm_vol2_markdown\chapters\05-page-translation-and-protection.md' `
    -DerivedFromSha256 $amdApmReference.sha256 `
    -Url $null
$amdApmChapter15Reference = New-LocalReference `
    -Key 'AMD-APM2-24593-r3.44-CH15' `
    -Kind 'derived-navigation' `
    -Publication '24593' `
    -Revision '3.44' `
    -RelativePath 'docs\amd64_apm_vol2_markdown\chapters\15-secure-virtual-machine.md' `
    -DerivedFromSha256 $amdApmReference.sha256 `
    -Url $null
$uefiReference = [pscustomobject] [ordered] @{
    key = 'UEFI-2.11'
    kind = 'normative-external-unpinned'
    publication = 'UEFI'
    revision = '2.11'
    local_path = $null
    sha256 = $null
    derived_from_sha256 = $null
    url = 'https://uefi.org/specs/UEFI/2.11/'
}

$profile = [pscustomobject] [ordered] @{
    schema_version = 1
    profile_kind = 'windows-read-only-inventory'
    qualification_status = 'blocked'
    run_id = $runId
    captured_at_utc = $capturedAtUtc.ToString('o')
    collector = [pscustomobject] [ordered] @{
        path = 'tools/target-profile/collect-windows.ps1'
        sha256 = $collectorHash
        schema_path = 'tools/target-profile/target-profile-v1.schema.json'
        schema_sha256 = $schemaHash
        powershell_version = $PSVersionTable.PSVersion.ToString()
        elevated = $isAdministrator
    }
    source_control = $sourceControl
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
        pci_instance_ids_included = [bool] $IncludePciInstanceId
    }
    platform = [pscustomobject] [ordered] @{
        computer_system = $computerSystem
        baseboard = $baseboard
        bios = $bios
        processors = $processors
    }
    operating_system = $windows
    virtualization = [pscustomobject] [ordered] @{
        device_guard = $deviceGuard
    }
    security = [pscustomobject] [ordered] @{
        secure_boot_enabled = $secureBootEnabled
        tpm = $tpm
    }
    squirrel = [pscustomobject] [ordered] @{
        queried_vendor_id = $PciVendorId.ToLowerInvariant()
        queried_device_id = $PciDeviceId.ToLowerInvariant()
        functions = $squirrelFunctions
    }
    manual_gates = $manualGates
    collection_errors = @($collectionErrors)
    references = @(
        $amdApmReference
        $amdApmChapter5Reference
        $amdApmChapter15Reference
        $uefiReference
    )
}

$profilePath = Join-Path $OutputDirectory 'target-profile.json'
$copiedSchemaPath = Join-Path $OutputDirectory 'target-profile-v1.schema.json'
$copiedCollectorPath = Join-Path $OutputDirectory 'collect-windows.ps1'
$copiedVerifierPath = Join-Path $OutputDirectory 'verify-bundle.ps1'
Write-Utf8Json -Value $profile -Path $profilePath
Copy-Item -LiteralPath $schemaPath -Destination $copiedSchemaPath
Copy-Item -LiteralPath $PSCommandPath -Destination $copiedCollectorPath
Copy-Item -LiteralPath $verifierPath -Destination $copiedVerifierPath

$manifestFiles = @(
    Get-Item -LiteralPath `
        $profilePath, `
        $copiedSchemaPath, `
        $copiedCollectorPath, `
        $copiedVerifierPath |
        Sort-Object -Property Name |
        ForEach-Object {
            [pscustomobject] [ordered] @{
                path = $_.Name
                size_bytes = [long] $_.Length
                sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $_.FullName).Hash.ToLowerInvariant()
            }
        }
)
$manifest = [pscustomobject] [ordered] @{
    schema_version = 1
    run_id = $runId
    created_at_utc = [DateTime]::UtcNow.ToString('o')
    files = $manifestFiles
}
$manifestPath = Join-Path $OutputDirectory 'manifest.json'
Write-Utf8Json -Value $manifest -Path $manifestPath

& $copiedVerifierPath -BundleDirectory $OutputDirectory

Write-Host "Read-only target profile: $profilePath"
Write-Host "Evidence manifest: $manifestPath"
$unresolvedGateCount = @($manualGates | Where-Object status -ne 'verified').Count
Write-Host "Unresolved manual gates: $unresolvedGateCount"
Write-Host "Collection errors: $($collectionErrors.Count)"
