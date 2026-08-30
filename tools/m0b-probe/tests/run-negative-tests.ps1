<#
.SYNOPSIS
Runs the checked-in M0b verifier negative regression cases.
#>

[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$toolRoot = Split-Path -Parent $PSScriptRoot
$fixturePath = Join-Path $PSScriptRoot 'fixtures\raw-probe-valid.json'
$fixtureV2Path = Join-Path $PSScriptRoot 'fixtures\raw-probe-v2-valid.json'
$fixtureV3Path = Join-Path $PSScriptRoot 'fixtures\raw-probe-v3-valid.json'
$fixtureV4Path = Join-Path $PSScriptRoot 'fixtures\raw-probe-v4-valid.json'
$fixtureV5Path = Join-Path $PSScriptRoot 'fixtures\raw-probe-v5-valid.json'
$fixtureV6Path = Join-Path $PSScriptRoot 'fixtures\raw-probe-v6-valid.json'
$verifierPath = Join-Path $toolRoot 'verify-bundle.ps1'
$schemaPath = Join-Path $toolRoot 'm0b-probe-v1.schema.json'
$schemaV2Path = Join-Path $toolRoot 'm0b-probe-v2.schema.json'
$schemaV3Path = Join-Path $toolRoot 'm0b-probe-v3.schema.json'
$schemaV4Path = Join-Path $toolRoot 'm0b-probe-v4.schema.json'
$schemaV5Path = Join-Path $toolRoot 'm0b-probe-v5.schema.json'
$schemaV6Path = Join-Path $toolRoot 'm0b-probe-v6.schema.json'
$preparePath = Join-Path $toolRoot 'prepare-media.ps1'
$finalizePath = Join-Path $toolRoot 'finalize-evidence.ps1'
$testRoot = Join-Path ([System.IO.Path]::GetTempPath()) ('svmvisor-m0b-tests-' + [guid]::NewGuid().ToString('N'))
$baseline = Join-Path $testRoot 'baseline'
$baselineV2 = Join-Path $testRoot 'baseline-v2'
$baselineV3 = Join-Path $testRoot 'baseline-v3'
$baselineV4 = Join-Path $testRoot 'baseline-v4'
$baselineV5 = Join-Path $testRoot 'baseline-v5'
$baselineV6 = Join-Path $testRoot 'baseline-v6'
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)

function Write-JsonFile {
    param([string] $Path, $Value, [int] $Depth = 20)
    $json = $Value | ConvertTo-Json -Depth $Depth
    [System.IO.File]::WriteAllText($Path, $json + "`n", $utf8NoBom)
}

function New-ManifestFileRecord {
    param([string] $Directory, [string] $Name)
    $path = Join-Path $Directory $Name
    $item = Get-Item -Force -LiteralPath $path
    return [pscustomobject] [ordered] @{
        path = $Name
        size_bytes = [long] $item.Length
        sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash.ToLowerInvariant()
    }
}

function Update-RawManifestRecord {
    param([string] $Bundle)
    $manifestPath = Join-Path $Bundle 'manifest.json'
    $manifest = Get-Content -Raw -Encoding UTF8 -LiteralPath $manifestPath | ConvertFrom-Json
    $replacement = New-ManifestFileRecord $Bundle 'raw-probe.json'
    $files = @(
        foreach ($file in @($manifest.files)) {
            if ($file.path -ceq 'raw-probe.json') { $replacement } else { $file }
        }
    )
    $manifest.files = $files
    Write-JsonFile $manifestPath $manifest 8
}

function Convert-TestHexToBytes {
    param([string] $Hex)
    $bytes = New-Object byte[] ($Hex.Length / 2)
    for ($index = 0; $index -lt $bytes.Length; $index++) {
        $bytes[$index] = [Convert]::ToByte($Hex.Substring($index * 2, 2), 16)
    }
    return ,$bytes
}

function Convert-TestBytesToHex {
    param([byte[]] $Bytes)
    return -join @($Bytes | ForEach-Object { $_.ToString('x2') })
}

function Get-TestBytesSha256 {
    param([byte[]] $Bytes)
    $algorithm = [Security.Cryptography.SHA256]::Create()
    try { return Convert-TestBytesToHex $algorithm.ComputeHash($Bytes) }
    finally { $algorithm.Dispose() }
}

function Update-TestEnvelope {
    param($Envelope, [byte[]] $Bytes)
    $Envelope.length_bytes = $Bytes.Length
    $Envelope.bytes = Convert-TestBytesToHex $Bytes
    $Envelope.sha256 = Get-TestBytesSha256 $Bytes
}

function Repair-TestSdtChecksum {
    param([byte[]] $Bytes)
    $Bytes[9] = 0
    [int] $sum = 0
    foreach ($value in $Bytes) { $sum = ($sum + $value) -band 0xff }
    $Bytes[9] = [byte] ((256 - $sum) -band 0xff)
}

function New-TestF0DeviceEntry {
    param([byte] $UidFormat, [byte[]] $UidBytes)
    $entry = New-Object byte[] (22 + $UidBytes.Length)
    $entry[0] = 0xf0
    $entry[20] = $UidFormat
    $entry[21] = [byte] $UidBytes.Length
    if ($UidBytes.Length -gt 0) {
        [Array]::Copy($UidBytes, 0, $entry, 22, $UidBytes.Length)
    }
    return ,$entry
}

function New-TestType40IvrsBytes {
    param([byte[]] $Template, [byte[]] $DeviceEntries)
    $tableLength = 88 + $DeviceEntries.Length
    $blockLength = 40 + $DeviceEntries.Length
    if ($blockLength -gt [uint16]::MaxValue) { throw 'Test IVHD block is too large.' }
    $bytes = New-Object byte[] $tableLength
    [Array]::Copy($Template, 0, $bytes, 0, 36)
    [Array]::Copy([BitConverter]::GetBytes([uint32] $tableLength), 0, $bytes, 4, 4)
    $bytes[8] = 2
    [Array]::Copy([BitConverter]::GetBytes([uint32] 1), 0, $bytes, 36, 4)
    $bytes[48] = 0x40
    [Array]::Copy([BitConverter]::GetBytes([uint16] $blockLength), 0, $bytes, 50, 2)
    [Array]::Copy($Template, 52, $bytes, 52, 20)
    [Array]::Copy($DeviceEntries, 0, $bytes, 88, $DeviceEntries.Length)
    Repair-TestSdtChecksum $bytes
    return ,$bytes
}

function New-TestF0DeviceWitness {
    param([byte[]] $Bytes, [int] $Offset)
    $length = 22 + [byte] $Bytes[$Offset + 21]
    $entry = New-Object byte[] $length
    [Array]::Copy($Bytes, $Offset, $entry, 0, $length)
    return [pscustomobject] [ordered] @{
        type = '0xf0'
        length = $length
        offset = $Offset
        raw_sha256 = Get-TestBytesSha256 $entry
        uid_length = [byte] $Bytes[$Offset + 21]
    }
}

function New-TestMadtLocalApicWitness {
    param([byte[]] $Bytes, [int] $Offset)
    $entry = New-Object byte[] 8
    [Array]::Copy($Bytes, $Offset, $entry, 0, 8)
    $flags = [BitConverter]::ToUInt32($Bytes, $Offset + 4)
    return [pscustomobject] [ordered] @{
        kind = 'processor-local-apic'
        type = 0
        length = 8
        offset = $Offset
        raw_sha256 = Get-TestBytesSha256 $entry
        acpi_processor_uid = [byte] $Bytes[$Offset + 2]
        apic_id = '0x{0:x2}' -f [byte] $Bytes[$Offset + 3]
        flags = '0x{0:x8}' -f $flags
        enabled = ($flags -band 1) -ne 0
        online_capable = ($flags -band 2) -ne 0
    }
}

function Sync-TestAcpiTable {
    param($Raw, [string] $Name, [byte[]] $Bytes)
    $record = $Raw.acpi.tables.$Name
    Update-TestEnvelope $record.raw $Bytes
    $record.header.length = $Bytes.Length
    $record.header.revision = [byte] $Bytes[8]
    $record.header.checksum = '0x{0:x2}' -f $Bytes[9]
    $directory = @($Raw.acpi.directory | Where-Object { $_.address -ceq $record.address })
    if ($directory.Count -ne 1) { throw "Could not find one directory record for $Name." }
    $headerBytes = New-Object byte[] 36
    [Array]::Copy($Bytes, 0, $headerBytes, 0, 36)
    Update-TestEnvelope $directory[0].header_raw $headerBytes
    $directory[0].declared_length = $Bytes.Length
    $directory[0].revision = [byte] $Bytes[8]
}

function Sync-TestType40IvrsWitness {
    param($Raw, [byte[]] $Bytes, [object[]] $DeviceWitnesses, [int] $DeviceEntryCount)
    Sync-TestAcpiTable $Raw 'ivrs' $Bytes
    $record = $Raw.acpi.tables.ivrs
    $record.body.iv_info = '0x00000001'
    $record.body.block_count = 1
    $record.body.device_entry_count = $DeviceEntryCount
    $block = $record.body.blocks[0]
    $block.kind = 'ivhd'
    $block.type = '0x40'
    $block.flags = '0x00'
    $block.length = $Bytes.Length - 48
    $block.offset = 48
    $block.header_length = 40
    $block.extended_feature_image = '0x0000000000000000'
    $block.extended_feature_image_2 = '0x0000000000000000'
    $block.device_entry_count = $DeviceEntryCount
    $block.device_entries = [object[]] @($DeviceWitnesses)
    $blockBytes = New-Object byte[] $block.length
    [Array]::Copy($Bytes, 48, $blockBytes, 0, $block.length)
    $block.raw_sha256 = Get-TestBytesSha256 $blockBytes
}

function Save-TestRawMutation {
    param([string] $Bundle, $Raw)
    Write-JsonFile (Join-Path $Bundle 'raw-probe.json') $Raw 40
    Update-RawManifestRecord $Bundle
}

function Invoke-NegativeCase {
    param(
        [string] $Name,
        [scriptblock] $Mutation,
        [string] $ExpectedMessage,
        [string] $SourceBundle = $baseline
    )
    $caseDirectory = Join-Path $testRoot $Name
    Copy-Item -Recurse -LiteralPath $SourceBundle -Destination $caseDirectory
    & $Mutation $caseDirectory
    $savedErrorActionPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        $output = @(
            powershell.exe -NoProfile -ExecutionPolicy Bypass `
                -File (Join-Path $caseDirectory 'verify-bundle.ps1') `
                -BundleDirectory $caseDirectory 2>&1
        ) | Out-String
        $caseExitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $savedErrorActionPreference
    }
    if ($caseExitCode -eq 0) {
        throw "Negative case '$Name' unexpectedly passed."
    }
    if ($output -notmatch [regex]::Escape($ExpectedMessage)) {
        throw "Negative case '$Name' failed for the wrong reason: $output"
    }
    Write-Host "Negative M0b verifier case '$Name': PASS"
}

function Invoke-PositiveCase {
    param(
        [string] $Name,
        [scriptblock] $Mutation,
        [string] $SourceBundle = $baseline
    )
    $caseDirectory = Join-Path $testRoot $Name
    Copy-Item -Recurse -LiteralPath $SourceBundle -Destination $caseDirectory
    & $Mutation $caseDirectory
    $savedErrorActionPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        $output = @(
            powershell.exe -NoProfile -ExecutionPolicy Bypass `
                -File (Join-Path $caseDirectory 'verify-bundle.ps1') `
                -BundleDirectory $caseDirectory 2>&1
        ) | Out-String
        $caseExitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $savedErrorActionPreference
    }
    if ($caseExitCode -ne 0) {
        throw "Positive case '$Name' unexpectedly failed: $output"
    }
    Write-Host "Positive M0b verifier case '$Name': PASS"
}

try {
    [void] [System.IO.Directory]::CreateDirectory($baseline)
    [System.IO.File]::Copy($verifierPath, (Join-Path $baseline 'verify-bundle.ps1'), $false)
    [System.IO.File]::Copy($schemaPath, (Join-Path $baseline 'm0b-probe-v1.schema.json'), $false)
    [System.IO.File]::Copy($preparePath, (Join-Path $baseline 'prepare-media.ps1'), $false)
    [System.IO.File]::Copy($finalizePath, (Join-Path $baseline 'finalize-evidence.ps1'), $false)
    [System.IO.File]::WriteAllBytes(
        (Join-Path $baseline 'svmvisor-m0b-probe.efi'),
        [byte[]] @(0x4d, 0x5a, 0x00, 0x00)
    )

    $dummyHash = '1111111111111111111111111111111111111111111111111111111111111111'
    $targetManifest = [pscustomobject] [ordered] @{
        schema_version = 1
        run_id = '20260806T210000000Z'
        created_at_utc = '2026-08-06T21:00:00.0000000Z'
        files = @(
            foreach ($name in @(
                'collect-windows.ps1',
                'target-profile-v1.schema.json',
                'target-profile.json',
                'verify-bundle.ps1'
            )) {
                [pscustomobject] [ordered] @{
                    path = $name
                    size_bytes = 1
                    sha256 = $dummyHash
                }
            }
        )
    }
    $targetManifestPath = Join-Path $baseline 'target-profile-manifest.json'
    Write-JsonFile $targetManifestPath $targetManifest 8
    $targetManifestHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $targetManifestPath).Hash.ToLowerInvariant()

    $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath $fixturePath | ConvertFrom-Json
    $raw.target_profile_manifest_sha256 = $targetManifestHash
    Write-JsonFile (Join-Path $baseline 'raw-probe.json') $raw 20

    $manifestedNames = @(
        'finalize-evidence.ps1',
        'm0b-probe-v1.schema.json',
        'prepare-media.ps1',
        'raw-probe.json',
        'svmvisor-m0b-probe.efi',
        'target-profile-manifest.json',
        'verify-bundle.ps1'
    )
    $manifest = [pscustomobject] [ordered] @{
        schema_version = 1
        bundle_kind = 'svmvisor-m0b-probe-evidence'
        created_at_utc = '2026-08-06T21:00:00.0000000Z'
        target_profile_manifest_sha256 = $targetManifestHash
        raw_evidence_filename = 'svmvisor-m0b-20260806T210000-000000000.json'
        files = @(
            foreach ($name in $manifestedNames) {
                New-ManifestFileRecord $baseline $name
            }
        )
    }
    Write-JsonFile (Join-Path $baseline 'manifest.json') $manifest 8

    powershell.exe -NoProfile -ExecutionPolicy Bypass `
        -File (Join-Path $baseline 'verify-bundle.ps1') `
        -BundleDirectory $baseline
    if ($LASTEXITCODE -ne 0) {
        throw 'The internally consistent baseline bundle did not verify.'
    }

    Invoke-NegativeCase 'null-authorization-boolean' {
        param($bundle)
        $path = Join-Path $bundle 'raw-probe.json'
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath $path | ConvertFrom-Json
        $raw.launch_authorized = $null
        Write-JsonFile $path $raw 20
        Update-RawManifestRecord $bundle
    } 'launch_authorized must be a JSON Boolean.'

    Invoke-NegativeCase 'wrong-profile-binding' {
        param($bundle)
        $path = Join-Path $bundle 'raw-probe.json'
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath $path | ConvertFrom-Json
        $raw.target_profile_manifest_sha256 = 'ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff'
        Write-JsonFile $path $raw 20
        Update-RawManifestRecord $bundle
    } 'Raw evidence is bound to the wrong target-profile manifest.'

    Invoke-NegativeCase 'duplicate-escaped-json-property' {
        param($bundle)
        $path = Join-Path $bundle 'raw-probe.json'
        $text = [System.IO.File]::ReadAllText($path)
        $pattern = '"launch_authorized"\s*:\s*false'
        if ([regex]::Matches($text, $pattern).Count -ne 1) {
            throw 'Could not locate launch_authorized for the duplicate-property mutation.'
        }
        $text = [regex]::Replace(
            $text,
            $pattern,
            '"launch_authorized": true, "\u006caunch_authorized": false',
            1
        )
        [System.IO.File]::WriteAllText($path, $text, $utf8NoBom)
        Update-RawManifestRecord $bundle
    } "duplicate JSON property name: 'launch_authorized'"

    Invoke-NegativeCase 'non-amd-reserved-bit-label' {
        param($bundle)
        $path = Join-Path $bundle 'raw-probe.json'
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath $path | ConvertFrom-Json
        $raw.cpu.decoded.svm_capable.status = 'not-enumerated'
        Write-JsonFile $path $raw 20
        Update-RawManifestRecord $bundle
    } 'cpu.decoded.svm_capable must explicitly be not-applicable-vendor on a non-AMD CPU.'

    Invoke-NegativeCase 'extra-file' {
        param($bundle)
        [System.IO.File]::WriteAllText(
            (Join-Path $bundle 'unmanifested.txt'),
            'unexpected',
            $utf8NoBom
        )
    } 'Bundle must contain exactly the eight schema-v1 evidence files.'

    Invoke-NegativeCase 'duplicate-manifest-path' {
        param($bundle)
        $path = Join-Path $bundle 'manifest.json'
        $manifest = Get-Content -Raw -Encoding UTF8 -LiteralPath $path | ConvertFrom-Json
        $manifest.files = @($manifest.files) + @($manifest.files[0])
        Write-JsonFile $path $manifest 8
    } 'Duplicate manifest path:'

    [void] [System.IO.Directory]::CreateDirectory($baselineV2)
    foreach ($source in @(
        @{ Path = $verifierPath; Name = 'verify-bundle.ps1' },
        @{ Path = $schemaV2Path; Name = 'm0b-probe-v2.schema.json' },
        @{ Path = $preparePath; Name = 'prepare-media.ps1' },
        @{ Path = $finalizePath; Name = 'finalize-evidence.ps1' }
    )) {
        [System.IO.File]::Copy($source.Path, (Join-Path $baselineV2 $source.Name), $false)
    }
    [System.IO.File]::Copy((Join-Path $baseline 'svmvisor-m0b-probe.efi'), (Join-Path $baselineV2 'svmvisor-m0b-probe.efi'), $false)
    [System.IO.File]::Copy((Join-Path $baseline 'target-profile-manifest.json'), (Join-Path $baselineV2 'target-profile-manifest.json'), $false)
    $v2TargetHash = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $baselineV2 'target-profile-manifest.json')).Hash.ToLowerInvariant()
    $rawV2 = Get-Content -Raw -Encoding UTF8 -LiteralPath $fixtureV2Path | ConvertFrom-Json
    $rawV2.target_profile_manifest_sha256 = $v2TargetHash
    Write-JsonFile (Join-Path $baselineV2 'raw-probe.json') $rawV2 40
    $manifestedNamesV2 = @(
        'finalize-evidence.ps1',
        'm0b-probe-v2.schema.json',
        'prepare-media.ps1',
        'raw-probe.json',
        'svmvisor-m0b-probe.efi',
        'target-profile-manifest.json',
        'verify-bundle.ps1'
    )
    $manifestV2 = [pscustomobject] [ordered] @{
        schema_version = 2
        bundle_kind = 'svmvisor-m0b-probe-evidence'
        created_at_utc = '2026-08-06T21:00:00.0000000Z'
        target_profile_manifest_sha256 = $v2TargetHash
        raw_evidence_filename = 'svmvisor-m0b-20260806T210000-000000000.json'
        files = @(
            foreach ($name in $manifestedNamesV2) {
                New-ManifestFileRecord $baselineV2 $name
            }
        )
    }
    Write-JsonFile (Join-Path $baselineV2 'manifest.json') $manifestV2 8
    powershell.exe -NoProfile -ExecutionPolicy Bypass `
        -File (Join-Path $baselineV2 'verify-bundle.ps1') `
        -BundleDirectory $baselineV2
    if ($LASTEXITCODE -ne 0) {
        throw 'The internally consistent v2 AMD ACPI baseline bundle did not verify.'
    }

    Invoke-PositiveCase -Name 'v2-madt-duplicate-disabled-placeholder-id' -SourceBundle $baselineV2 -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        [byte[]] $original = Convert-TestHexToBytes $raw.acpi.tables.madt.raw.bytes
        [byte[]] $bytes = New-Object byte[] ($original.Length + 16)
        [Array]::Copy($original, 0, $bytes, 0, $original.Length)
        [Array]::Copy([BitConverter]::GetBytes([uint32] $bytes.Length), 0, $bytes, 4, 4)
        $bytes[60] = 0
        $bytes[61] = 8
        $bytes[62] = 2
        $bytes[63] = 0
        $bytes[68] = 0
        $bytes[69] = 8
        $bytes[70] = 3
        $bytes[71] = 0
        Repair-TestSdtChecksum $bytes
        Sync-TestAcpiTable $raw 'madt' $bytes
        $firstPlaceholder = New-TestMadtLocalApicWitness -Bytes $bytes -Offset 60
        $secondPlaceholder = New-TestMadtLocalApicWitness -Bytes $bytes -Offset 68
        $raw.acpi.tables.madt.body.entries = [object[]] @(
            @($raw.acpi.tables.madt.body.entries) + @($firstPlaceholder, $secondPlaceholder)
        )
        $raw.acpi.tables.madt.body.entry_count = 4
        $raw.acpi.tables.madt.body.processor_entry_count = 4
        $raw.acpi.tables.madt.body.enabled_processor_count = 2
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v2-madt-duplicate-enabled-id' -SourceBundle $baselineV2 -ExpectedMessage 'MADT contains a duplicate usable processor interrupt-controller ID:' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        [byte[]] $bytes = Convert-TestHexToBytes $raw.acpi.tables.madt.raw.bytes
        $bytes[55] = 0
        Repair-TestSdtChecksum $bytes
        Sync-TestAcpiTable $raw 'madt' $bytes
        $raw.acpi.tables.madt.body.entries[1] = New-TestMadtLocalApicWitness -Bytes $bytes -Offset 52
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v2-madt-duplicate-online-capable-id' -SourceBundle $baselineV2 -ExpectedMessage 'MADT contains a duplicate usable processor interrupt-controller ID:' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        [byte[]] $bytes = Convert-TestHexToBytes $raw.acpi.tables.madt.raw.bytes
        $bytes[55] = 0
        [Array]::Copy([BitConverter]::GetBytes([uint32] 2), 0, $bytes, 48, 4)
        [Array]::Copy([BitConverter]::GetBytes([uint32] 2), 0, $bytes, 56, 4)
        Repair-TestSdtChecksum $bytes
        Sync-TestAcpiTable $raw 'madt' $bytes
        $raw.acpi.tables.madt.body.entries[0] = New-TestMadtLocalApicWitness -Bytes $bytes -Offset 44
        $raw.acpi.tables.madt.body.entries[1] = New-TestMadtLocalApicWitness -Bytes $bytes -Offset 52
        $raw.acpi.tables.madt.body.enabled_processor_count = 0
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v2-madt-malformed-entry-length' -SourceBundle $baselineV2 -ExpectedMessage 'MADT entry length does not make forward progress.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        [byte[]] $bytes = Convert-TestHexToBytes $raw.acpi.tables.madt.raw.bytes
        $bytes[45] = 1
        Repair-TestSdtChecksum $bytes
        Sync-TestAcpiTable $raw 'madt' $bytes
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v2-fadt-truncated' -SourceBundle $baselineV2 -ExpectedMessage 'acpi.tables.fadt declared length does not match captured bytes.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        [byte[]] $bytes = Convert-TestHexToBytes $raw.acpi.tables.fadt.raw.bytes
        $truncated = New-Object byte[] ($bytes.Length - 1)
        [Array]::Copy($bytes, 0, $truncated, 0, $truncated.Length)
        Update-TestEnvelope $raw.acpi.tables.fadt.raw $truncated
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v2-mcfg-bad-checksum' -SourceBundle $baselineV2 -ExpectedMessage 'acpi.tables.mcfg has an invalid ACPI checksum.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        [byte[]] $bytes = Convert-TestHexToBytes $raw.acpi.tables.mcfg.raw.bytes
        $bytes[$bytes.Length - 1] = $bytes[$bytes.Length - 1] -bxor 1
        Update-TestEnvelope $raw.acpi.tables.mcfg.raw $bytes
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v2-mcfg-window-overflow' -SourceBundle $baselineV2 -ExpectedMessage 'MCFG ECAM bus-zero-relative window overflows the uint64 address space.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        [byte[]] $bytes = Convert-TestHexToBytes $raw.acpi.tables.mcfg.raw.bytes
        [uint64] $base = [uint64]::MaxValue - 65535
        [byte[]] $baseBytes = [BitConverter]::GetBytes($base)
        [Array]::Copy($baseBytes, 0, $bytes, 44, 8)
        Repair-TestSdtChecksum $bytes
        Sync-TestAcpiTable $raw 'mcfg' $bytes
        $raw.acpi.tables.mcfg.body.allocations[0].base_address = '0x{0:x16}' -f $base
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v2-mcfg-misaligned-base' -SourceBundle $baselineV2 -ExpectedMessage 'MCFG allocation base address is not 1 MiB aligned.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        [byte[]] $bytes = Convert-TestHexToBytes $raw.acpi.tables.mcfg.raw.bytes
        [uint64] $base = [Convert]::ToUInt64('80001000', 16)
        [byte[]] $baseBytes = [BitConverter]::GetBytes($base)
        [Array]::Copy($baseBytes, 0, $bytes, 44, 8)
        Repair-TestSdtChecksum $bytes
        Sync-TestAcpiTable $raw 'mcfg' $bytes
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v2-xsdt-duplicate-pointer' -SourceBundle $baselineV2 -ExpectedMessage 'acpi.roots.xsdt contains a duplicate table pointer:' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        [byte[]] $bytes = Convert-TestHexToBytes $raw.acpi.roots.xsdt.raw.bytes
        [Array]::Copy($bytes, 36, $bytes, 44, 8)
        Repair-TestSdtChecksum $bytes
        Update-TestEnvelope $raw.acpi.roots.xsdt.raw $bytes
        $raw.acpi.roots.xsdt.header.checksum = '0x{0:x2}' -f $bytes[9]
        $raw.acpi.roots.xsdt.entries[1] = $raw.acpi.roots.xsdt.entries[0]
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v2-falsified-topology' -SourceBundle $baselineV2 -ExpectedMessage 'MADT/MP consistency witness is falsified.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.acpi.madt_mp_cross_check.consistent = $false
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v2-ivrs-ownership-claim' -SourceBundle $baselineV2 -ExpectedMessage 'acpi.ivrs_claims.runtime_ownership_claim must remain false.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.acpi.ivrs_claims.runtime_ownership_claim = $true
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v2-ivrs-reserved-variable-entry' -SourceBundle $baselineV2 -ExpectedMessage 'IVHD variable device entry type 0x80 is reserved or unsupported.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        [byte[]] $bytes = Convert-TestHexToBytes $raw.acpi.tables.ivrs.raw.bytes
        $bytes[72] = 0x80
        Repair-TestSdtChecksum $bytes
        Sync-TestAcpiTable $raw 'ivrs' $bytes
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v2-ivhd-misaligned-iommu-base' -SourceBundle $baselineV2 -ExpectedMessage 'IVHD IOMMU base address is not 16 KiB aligned.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        [byte[]] $bytes = Convert-TestHexToBytes $raw.acpi.tables.ivrs.raw.bytes
        [uint64] $base = [Convert]::ToUInt64('fed80001', 16)
        [byte[]] $baseBytes = [BitConverter]::GetBytes($base)
        [Array]::Copy($baseBytes, 0, $bytes, 56, 8)
        Repair-TestSdtChecksum $bytes
        Sync-TestAcpiTable $raw 'ivrs' $bytes
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v2-ivhd-reserved-fixed-device-type' -SourceBundle $baselineV2 -ExpectedMessage 'IVHD variable device entry type 0x05 is reserved or unsupported.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        [byte[]] $bytes = Convert-TestHexToBytes $raw.acpi.tables.ivrs.raw.bytes
        $bytes[72] = 0x05
        Repair-TestSdtChecksum $bytes
        Sync-TestAcpiTable $raw 'ivrs' $bytes
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v2-ivhd-fixed-entry-after-f0' -SourceBundle $baselineV2 -ExpectedMessage 'IVHD fixed-length device entries must precede variable-length entries.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        [byte[]] $template = Convert-TestHexToBytes $raw.acpi.tables.ivrs.raw.bytes
        [byte[]] $f0 = New-TestF0DeviceEntry -UidFormat 0 -UidBytes (New-Object byte[] 0)
        [byte[]] $deviceEntries = New-Object byte[] ($f0.Length + 4)
        [Array]::Copy($f0, 0, $deviceEntries, 0, $f0.Length)
        $deviceEntries[$f0.Length] = 0x01
        [byte[]] $bytes = New-TestType40IvrsBytes -Template $template -DeviceEntries $deviceEntries
        $f0Witness = New-TestF0DeviceWitness -Bytes $bytes -Offset 88
        Sync-TestType40IvrsWitness -Raw $raw -Bytes $bytes -DeviceWitnesses ([object[]] @($f0Witness)) -DeviceEntryCount 2
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v2-ivhd-f0-reserved-uid-format' -SourceBundle $baselineV2 -ExpectedMessage 'IVHD F0h UID format must be in the range 0..2.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        [byte[]] $template = Convert-TestHexToBytes $raw.acpi.tables.ivrs.raw.bytes
        [byte[]] $deviceEntries = New-TestF0DeviceEntry -UidFormat 3 -UidBytes (New-Object byte[] 0)
        [byte[]] $bytes = New-TestType40IvrsBytes -Template $template -DeviceEntries $deviceEntries
        $f0Witness = New-TestF0DeviceWitness -Bytes $bytes -Offset 88
        Sync-TestType40IvrsWitness -Raw $raw -Bytes $bytes -DeviceWitnesses ([object[]] @($f0Witness)) -DeviceEntryCount 1
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v2-ivhd-f0-absent-uid-with-bytes' -SourceBundle $baselineV2 -ExpectedMessage 'IVHD F0h UID format/length presence contract is invalid.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        [byte[]] $template = Convert-TestHexToBytes $raw.acpi.tables.ivrs.raw.bytes
        [byte[]] $deviceEntries = New-TestF0DeviceEntry -UidFormat 0 -UidBytes ([byte[]] @(0x58))
        [byte[]] $bytes = New-TestType40IvrsBytes -Template $template -DeviceEntries $deviceEntries
        $f0Witness = New-TestF0DeviceWitness -Bytes $bytes -Offset 88
        Sync-TestType40IvrsWitness -Raw $raw -Bytes $bytes -DeviceWitnesses ([object[]] @($f0Witness)) -DeviceEntryCount 1
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v2-ivhd-f0-present-uid-without-bytes' -SourceBundle $baselineV2 -ExpectedMessage 'IVHD F0h UID format/length presence contract is invalid.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        [byte[]] $template = Convert-TestHexToBytes $raw.acpi.tables.ivrs.raw.bytes
        [byte[]] $deviceEntries = New-TestF0DeviceEntry -UidFormat 2 -UidBytes (New-Object byte[] 0)
        [byte[]] $bytes = New-TestType40IvrsBytes -Template $template -DeviceEntries $deviceEntries
        $f0Witness = New-TestF0DeviceWitness -Bytes $bytes -Offset 88
        Sync-TestType40IvrsWitness -Raw $raw -Bytes $bytes -DeviceWitnesses ([object[]] @($f0Witness)) -DeviceEntryCount 1
        Save-TestRawMutation $bundle $raw
    }

    [void] [System.IO.Directory]::CreateDirectory($baselineV3)
    foreach ($source in @(
        @{ Path = $verifierPath; Name = 'verify-bundle.ps1' },
        @{ Path = $schemaV3Path; Name = 'm0b-probe-v3.schema.json' },
        @{ Path = $preparePath; Name = 'prepare-media.ps1' },
        @{ Path = $finalizePath; Name = 'finalize-evidence.ps1' }
    )) {
        [System.IO.File]::Copy($source.Path, (Join-Path $baselineV3 $source.Name), $false)
    }
    [System.IO.File]::Copy((Join-Path $baseline 'svmvisor-m0b-probe.efi'), (Join-Path $baselineV3 'svmvisor-m0b-probe.efi'), $false)
    [System.IO.File]::Copy((Join-Path $baseline 'target-profile-manifest.json'), (Join-Path $baselineV3 'target-profile-manifest.json'), $false)
    $v3TargetHash = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $baselineV3 'target-profile-manifest.json')).Hash.ToLowerInvariant()
    $rawV3 = Get-Content -Raw -Encoding UTF8 -LiteralPath $fixtureV3Path | ConvertFrom-Json
    $rawV3.target_profile_manifest_sha256 = $v3TargetHash
    Write-JsonFile (Join-Path $baselineV3 'raw-probe.json') $rawV3 50
    $manifestedNamesV3 = @(
        'finalize-evidence.ps1',
        'm0b-probe-v3.schema.json',
        'prepare-media.ps1',
        'raw-probe.json',
        'svmvisor-m0b-probe.efi',
        'target-profile-manifest.json',
        'verify-bundle.ps1'
    )
    $manifestV3 = [pscustomobject] [ordered] @{
        schema_version = 3
        bundle_kind = 'svmvisor-m0b-probe-evidence'
        created_at_utc = '2026-08-06T21:00:00.0000000Z'
        target_profile_manifest_sha256 = $v3TargetHash
        raw_evidence_filename = 'svmvisor-m0b-20260806T210000-000000000.json'
        files = @(
            foreach ($name in $manifestedNamesV3) {
                New-ManifestFileRecord $baselineV3 $name
            }
        )
    }
    Write-JsonFile (Join-Path $baselineV3 'manifest.json') $manifestV3 8
    powershell.exe -NoProfile -ExecutionPolicy Bypass `
        -File (Join-Path $baselineV3 'verify-bundle.ps1') `
        -BundleDirectory $baselineV3
    if ($LASTEXITCODE -ne 0) {
        throw 'The internally consistent v3 per-processor baseline bundle did not verify.'
    }

    Invoke-PositiveCase -Name 'v3-extended-apic-high-bits-ignored' -SourceBundle $baselineV3 -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $leaf = @($raw.processor_consistency.observations[1].cpu.raw_leaves | Where-Object { $_.leaf -ceq '0x8000001e' })
        if ($leaf.Count -ne 1) { throw 'Could not locate the AP extended-APIC positive-test leaf.' }
        $leaf[0].eax = '0xa5a50001'
        $raw.processor_consistency.observations[1].leaf_8000001e_extended_apic_id = '0xa5a50001'
        Save-TestRawMutation $bundle $raw
    }

    Invoke-PositiveCase -Name 'v3-honest-cpuid-inconsistency' -SourceBundle $baselineV3 -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $observation = $raw.processor_consistency.observations[1]
        $leaf = @($observation.cpu.raw_leaves | Where-Object { $_.leaf -ceq '0x8000000a' })
        if ($leaf.Count -ne 1) { throw 'Could not locate the AP SVM-capability positive-test leaf.' }
        $leaf[0].edx = '0x00000001'
        $observation.cpu.decoded.nested_paging_capable.value = $true
        $observation.cpuid_matches_bsp = $false
        $raw.processor_consistency.cpuid_consistent = $false
        $raw.processor_consistency.consistent = $false
        Save-TestRawMutation $bundle $raw
    }

    Invoke-PositiveCase -Name 'v3-honest-vm-cr-inconsistency' -SourceBundle $baselineV3 -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $observation = $raw.processor_consistency.observations[1]
        $observation.vm_cr.raw = '0x0000000000000001'
        $observation.vm_cr.dpd = $true
        $observation.vm_cr_matches_bsp = $false
        $raw.processor_consistency.vm_cr_consistent = $false
        $raw.processor_consistency.consistent = $false
        Save-TestRawMutation $bundle $raw
    }

    Invoke-PositiveCase -Name 'v3-honest-identity-inconsistency' -SourceBundle $baselineV3 -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $observation = $raw.processor_consistency.observations[1]
        $leaf = @($observation.cpu.raw_leaves | Where-Object { $_.leaf -ceq '0x00000001' })
        if ($leaf.Count -ne 1) { throw 'Could not locate the AP identity positive-test leaf.' }
        $leaf[0].ebx = '0x02000800'
        $observation.leaf_00000001_initial_apic_id = '0x02'
        $observation.identity_matches_mp = $false
        $raw.processor_consistency.identity_consistent = $false
        $raw.processor_consistency.consistent = $false
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v3-legacy-evidence-kind' -SourceBundle $baselineV3 -ExpectedMessage 'Unexpected v3 evidence_kind.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.evidence_kind = 'uefi-read-only-inventory-slice'
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v3-mp-count-parity-falsified' -SourceBundle $baselineV3 -ExpectedMessage 'Schema v3 requires mp_services.counts_consistent=true.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.mp_services.total = 3
        $raw.mp_services.counts_consistent = $false
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v3-memory-map-descriptor-cap-exceeded' -SourceBundle $baselineV3 -ExpectedMessage 'Schema v3 observed memory-map descriptor count exceeds 4096.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $descriptors = @(
            for ($index = 0; $index -lt 4097; $index++) {
                [pscustomobject] [ordered] @{
                    type = '0x00000007'
                    physical_start = '0x0000000000100000'
                    virtual_start = '0x0000000000000000'
                    page_count = 1
                    attributes = '0x0000000000000008'
                }
            }
        )
        $raw.memory_map = [pscustomobject] [ordered] @{
            status = 'observed'
            phase = 'collection-time-not-final-exit-boot-services-map'
            descriptor_size = 48
            descriptor_version = 1
            descriptors = [object[]] $descriptors
        }
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v3-memory-map-unavailable' -SourceBundle $baselineV3 -ExpectedMessage 'Schema v3 requires observed memory-map evidence.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.memory_map = [pscustomobject] [ordered] @{
            status = 'unavailable'
            phase = 'collection-time-not-final-exit-boot-services-map'
            uefi_status = '0x8000000000000009'
        }
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v3-ap-observation-missing' -SourceBundle $baselineV3 -ExpectedMessage 'processor_consistency is missing an enabled healthy processor observation.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.processor_consistency.observations = @($raw.processor_consistency.observations[0])
        $raw.processor_consistency.observation_count = 1
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v3-observation-mapping-mismatch' -SourceBundle $baselineV3 -ExpectedMessage 'processor_consistency observation order or MP Services mapping is inconsistent.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.processor_consistency.observations[1].processor_number = 0
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v3-who-am-i-mismatch' -SourceBundle $baselineV3 -ExpectedMessage 'processor_consistency WhoAmI processor number does not match the dispatched processor number.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.processor_consistency.observations[1].who_am_i.processor_number = 0
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v3-dispatch-contract-mutation' -SourceBundle $baselineV3 -ExpectedMessage 'processor_consistency firmware dispatch mechanism is unexpected.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.processor_consistency.dispatch.firmware_dispatch_mechanism = 'architecturally-assumed-init-sipi'
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v3-finite-ap-timeout' -SourceBundle $baselineV3 -ExpectedMessage 'processor_consistency.dispatch.timeout_microseconds_per_ap is outside its permitted range.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.processor_consistency.dispatch.timeout_microseconds_per_ap = 1000000
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v3-comparison-policy-mutation' -SourceBundle $baselineV3 -ExpectedMessage "processor_consistency comparison policy 'identity_comparison' is unexpected." -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.processor_consistency.comparison_policy.identity_comparison = 'full-width-apic-id-equality'
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v3-cpuid-mismatch' -SourceBundle $baselineV3 -ExpectedMessage 'processor_consistency cpuid_matches_bsp witness is falsified.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $leaf = @($raw.processor_consistency.observations[1].cpu.raw_leaves | Where-Object { $_.leaf -ceq '0x80000008' })
        if ($leaf.Count -ne 1) { throw 'Could not locate the AP CPUID mismatch test leaf.' }
        $leaf[0].ebx = '0x00000001'
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v3-topology-extension-missing' -SourceBundle $baselineV3 -ExpectedMessage 'processor_consistency requires the CPUID topology-extensions bit on every processor.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $leaf = @($raw.processor_consistency.observations[1].cpu.raw_leaves | Where-Object { $_.leaf -ceq '0x80000001' })
        if ($leaf.Count -ne 1) { throw 'Could not locate the AP topology-extension test leaf.' }
        $leaf[0].ecx = '0x00000004'
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v3-apic-witness-raw-disagreement' -SourceBundle $baselineV3 -ExpectedMessage 'CPUID extended APIC ID witness is inconsistent with raw leaf 0x8000001e.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.processor_consistency.observations[1].leaf_8000001e_extended_apic_id = '0x00000002'
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v3-falsified-cpuid-boolean' -SourceBundle $baselineV3 -ExpectedMessage 'processor_consistency cpuid_matches_bsp witness is falsified.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.processor_consistency.observations[1].cpuid_matches_bsp = $false
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v3-top-level-bsp-cpu-mismatch' -SourceBundle $baselineV3 -ExpectedMessage 'Top-level CPU does not exactly match the MP Services BSP observation.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $leaf = @($raw.cpu.raw_leaves | Where-Object { $_.leaf -ceq '0x80000008' })
        if ($leaf.Count -ne 1) { throw 'Could not locate the top-level BSP CPU mismatch test leaf.' }
        $leaf[0].ebx = '0x00000001'
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v3-top-level-bsp-vm-cr-mismatch' -SourceBundle $baselineV3 -ExpectedMessage 'Top-level VM_CR does not exactly match the MP Services BSP observation.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.vm_cr.raw = '0x0000000000000001'
        $raw.vm_cr.dpd = $true
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v3-decoded-vm-cr-bit-falsified' -SourceBundle $baselineV3 -ExpectedMessage 'vm_cr.dpd is inconsistent' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.processor_consistency.observations[1].vm_cr.dpd = $true
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v3-falsified-identity-aggregate' -SourceBundle $baselineV3 -ExpectedMessage 'processor_consistency identity_consistent aggregate is falsified.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.processor_consistency.identity_consistent = $false
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v3-falsified-cpuid-aggregate' -SourceBundle $baselineV3 -ExpectedMessage 'processor_consistency cpuid_consistent aggregate is falsified.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.processor_consistency.cpuid_consistent = $false
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v3-falsified-vm-cr-aggregate' -SourceBundle $baselineV3 -ExpectedMessage 'processor_consistency vm_cr_consistent aggregate is falsified.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.processor_consistency.vm_cr_consistent = $false
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v3-falsified-consistency-aggregate' -SourceBundle $baselineV3 -ExpectedMessage 'processor_consistency consistent aggregate is falsified.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.processor_consistency.consistent = $false
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v3-vm-cr-mismatch' -SourceBundle $baselineV3 -ExpectedMessage 'processor_consistency vm_cr_matches_bsp witness is falsified.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.processor_consistency.observations[1].vm_cr.raw = '0x0000000000000001'
        $raw.processor_consistency.observations[1].vm_cr.dpd = $true
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v3-falsified-identity-boolean' -SourceBundle $baselineV3 -ExpectedMessage 'processor_consistency identity_matches_mp witness is falsified.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.processor_consistency.observations[1].identity_matches_mp = $false
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v3-cpuid-mp-identity-mismatch' -SourceBundle $baselineV3 -ExpectedMessage 'processor_consistency identity_matches_mp witness is falsified.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $leaf = @($raw.processor_consistency.observations[1].cpu.raw_leaves | Where-Object { $_.leaf -ceq '0x00000001' })
        if ($leaf.Count -ne 1) { throw 'Could not locate the AP identity mismatch test leaf.' }
        $leaf[0].ebx = '0x02000800'
        $raw.processor_consistency.observations[1].leaf_00000001_initial_apic_id = '0x02'
        Save-TestRawMutation $bundle $raw
    }

    [void] [System.IO.Directory]::CreateDirectory($baselineV4)
    foreach ($source in @(
        @{ Path = $verifierPath; Name = 'verify-bundle.ps1' },
        @{ Path = $schemaV4Path; Name = 'm0b-probe-v4.schema.json' },
        @{ Path = $preparePath; Name = 'prepare-media.ps1' },
        @{ Path = $finalizePath; Name = 'finalize-evidence.ps1' }
    )) {
        [System.IO.File]::Copy($source.Path, (Join-Path $baselineV4 $source.Name), $false)
    }
    [System.IO.File]::Copy((Join-Path $baseline 'svmvisor-m0b-probe.efi'), (Join-Path $baselineV4 'svmvisor-m0b-probe.efi'), $false)
    [System.IO.File]::Copy((Join-Path $baseline 'target-profile-manifest.json'), (Join-Path $baselineV4 'target-profile-manifest.json'), $false)
    $v4TargetHash = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $baselineV4 'target-profile-manifest.json')).Hash.ToLowerInvariant()
    $rawV4 = Get-Content -Raw -Encoding UTF8 -LiteralPath $fixtureV4Path | ConvertFrom-Json
    $rawV4.target_profile_manifest_sha256 = $v4TargetHash
    Write-JsonFile (Join-Path $baselineV4 'raw-probe.json') $rawV4 60
    $manifestedNamesV4 = @(
        'finalize-evidence.ps1',
        'm0b-probe-v4.schema.json',
        'prepare-media.ps1',
        'raw-probe.json',
        'svmvisor-m0b-probe.efi',
        'target-profile-manifest.json',
        'verify-bundle.ps1'
    )
    $manifestV4 = [pscustomobject] [ordered] @{
        schema_version = 4
        bundle_kind = 'svmvisor-m0b-probe-evidence'
        created_at_utc = '2026-08-06T21:00:00.0000000Z'
        target_profile_manifest_sha256 = $v4TargetHash
        raw_evidence_filename = 'svmvisor-m0b-20260806T210000-000000000.json'
        files = @(
            foreach ($name in $manifestedNamesV4) {
                New-ManifestFileRecord $baselineV4 $name
            }
        )
    }
    Write-JsonFile (Join-Path $baselineV4 'manifest.json') $manifestV4 8
    powershell.exe -NoProfile -ExecutionPolicy Bypass `
        -File (Join-Path $baselineV4 'verify-bundle.ps1') `
        -BundleDirectory $baselineV4
    if ($LASTEXITCODE -ne 0) {
        throw 'The internally consistent v4 live-IOMMU baseline bundle did not verify.'
    }

    Invoke-PositiveCase -Name 'v4-pci-observed-mmio-disabled' -SourceBundle $baselineV4 -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.amd_iommu_live.status = 'pci-observed-mmio-disabled'
        $raw.amd_iommu_live.pci.capability.first_raw.base_low = '0xfed80000'
        $raw.amd_iommu_live.pci.capability.second_raw.base_low = '0xfed80000'
        $raw.amd_iommu_live.pci.capability.decoded.mmio_enabled = $false
        $raw.amd_iommu_live.access.mmio_read_operations = 0
        $raw.amd_iommu_live.access.mmio_read_bytes = 0
        $raw.amd_iommu_live.mmio = [pscustomobject] [ordered] @{
            status = 'not-read-capability-disabled'
            read_operations = 0
        }
        Save-TestRawMutation $bundle $raw
    }

    Invoke-PositiveCase -Name 'v4-pci-observed-efr-conflict' -SourceBundle $baselineV4 -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        [byte[]] $template = Convert-TestHexToBytes $raw.acpi.tables.ivrs.raw.bytes
        $deviceLength = $template.Length - 72
        [byte[]] $deviceEntries = New-Object byte[] $deviceLength
        [Array]::Copy($template, 72, $deviceEntries, 0, $deviceLength)
        [byte[]] $bytes = New-TestType40IvrsBytes -Template $template -DeviceEntries $deviceEntries
        $deviceWitnesses = @($raw.acpi.tables.ivrs.body.blocks[0].device_entries)
        foreach ($witness in $deviceWitnesses) { $witness.offset += 16 }
        Sync-TestType40IvrsWitness -Raw $raw -Bytes $bytes -DeviceWitnesses $deviceWitnesses -DeviceEntryCount $deviceWitnesses.Count

        $source = $raw.amd_iommu_live.locator.ivhd_sources[0]
        $source.entry_type = '0x40'
        $source.extended_feature_image = '0x0000000000000000'
        $source.extended_feature_image_2 = '0x0000000000000000'
        $unit = $raw.amd_iommu_live.locator.unit
        $unit.ivhd_type_10_present = $false
        $unit.ivhd_type_40_present = $true
        $unit.expected_extended_feature_image = '0x0000000000000000'
        $unit.expected_extended_feature_image_2 = '0x0000000000000000'
        $raw.amd_iommu_live.pci.capability.first_raw.header = '0x080b000f'
        $raw.amd_iommu_live.pci.capability.second_raw.header = '0x080b000f'
        $raw.amd_iommu_live.pci.capability.decoded.extended_feature_register_supported = $true
        $raw.amd_iommu_live.status = 'pci-observed-efr-conflict'
        $raw.amd_iommu_live.access.mmio_read_operations = 17
        $raw.amd_iommu_live.access.mmio_read_bytes = 136
        $baseSnapshot = $raw.amd_iommu_live.mmio.first_snapshot
        $baseSnapshot.extended_feature = '0x0000000000000001'
        $baseSnapshot.extended_feature_2 = '0x0000000000000000'
        $secondSnapshot = $baseSnapshot | ConvertTo-Json -Depth 10 | ConvertFrom-Json
        $raw.amd_iommu_live.mmio = [pscustomobject] [ordered] @{
            status = 'efr-conflict'
            read_operations = 17
            aperture = $raw.amd_iommu_live.mmio.aperture
            stable_offsets = @('0x0000', '0x0008', '0x0010', '0x0018', '0x0020', '0x0028', '0x0030', '0x01a0')
            feature_dependent_offsets = @()
            first_snapshot = $baseSnapshot
            second_snapshot = $secondSnapshot
            stable = $true
            status_offset = '0x2020'
            status_raw = '0x0000000000000000'
            extended_feature_match = $false
            expected_extended_features = [pscustomobject] [ordered] @{ efr = '0x0000000000000000'; efr2 = '0x0000000000000000' }
            live_extended_features = [pscustomobject] [ordered] @{ efr = '0x0000000000000001'; efr2 = '0x0000000000000000' }
        }
        Save-TestRawMutation $bundle $raw
    }

    Invoke-PositiveCase -Name 'v4-segmented-plan-order' -SourceBundle $baselineV4 -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        [byte[]] $template = Convert-TestHexToBytes $raw.acpi.tables.ivrs.raw.bytes
        $deviceLength = $template.Length - 72
        [byte[]] $deviceEntries = New-Object byte[] $deviceLength
        [Array]::Copy($template, 72, $deviceEntries, 0, $deviceLength)
        [byte[]] $bytes = New-TestType40IvrsBytes -Template $template -DeviceEntries $deviceEntries
        [uint64] $efr = [uint64] 1 -shl 38
        [Array]::Copy([BitConverter]::GetBytes($efr), 0, $bytes, 72, 8)
        Repair-TestSdtChecksum $bytes
        $deviceWitnesses = @($raw.acpi.tables.ivrs.body.blocks[0].device_entries)
        foreach ($witness in $deviceWitnesses) { $witness.offset += 16 }
        Sync-TestType40IvrsWitness -Raw $raw -Bytes $bytes -DeviceWitnesses $deviceWitnesses -DeviceEntryCount $deviceWitnesses.Count
        $raw.acpi.tables.ivrs.body.blocks[0].extended_feature_image = '0x0000004000000000'

        $source = $raw.amd_iommu_live.locator.ivhd_sources[0]
        $source.entry_type = '0x40'
        $source.extended_feature_image = '0x0000004000000000'
        $source.extended_feature_image_2 = '0x0000000000000000'
        $unit = $raw.amd_iommu_live.locator.unit
        $unit.ivhd_type_10_present = $false
        $unit.ivhd_type_40_present = $true
        $unit.expected_extended_feature_image = '0x0000004000000000'
        $unit.expected_extended_feature_image_2 = '0x0000000000000000'
        $raw.amd_iommu_live.pci.capability.first_raw.header = '0x080b000f'
        $raw.amd_iommu_live.pci.capability.second_raw.header = '0x080b000f'
        $raw.amd_iommu_live.pci.capability.decoded.extended_feature_register_supported = $true
        $raw.amd_iommu_live.access.mmio_read_operations = 19
        $raw.amd_iommu_live.access.mmio_read_bytes = 152

        $mmio = $raw.amd_iommu_live.mmio
        $snapshot = $mmio.first_snapshot
        $snapshot.control = '0x0000000400000000'
        $snapshot.extended_feature = '0x0000004000000000'
        $snapshot.extended_feature_2 = '0x0000000000000000'
        $snapshot.device_table_segments = @(
            [pscustomobject] [ordered] @{ segment = 1; raw = '0x0000000000000000' }
        )
        $secondSnapshot = $snapshot | ConvertTo-Json -Depth 10 | ConvertFrom-Json
        $configured = @($mmio.configured_ranges) + @(
            [pscustomobject] [ordered] @{
                source_offset = '0x0100'
                raw = '0x0000000000000000'
                enabled = $false
                base = '0x0000000000000000'
                length = 4096
                alignment = 4096
                validated_range = $null
                memory_binding = $null
            }
        )
        $raw.amd_iommu_live.mmio = [pscustomobject] [ordered] @{
            status = 'observed'
            read_operations = 19
            aperture = $mmio.aperture
            stable_offsets = @('0x0000', '0x0008', '0x0010', '0x0018', '0x0020', '0x0028', '0x0030', '0x01a0', '0x0100')
            feature_dependent_offsets = @('0x0100')
            first_snapshot = $snapshot
            second_snapshot = $secondSnapshot
            stable = $true
            status_offset = '0x2020'
            status_raw = '0x0000000000000000'
            extended_feature_match = $true
            extended_features = [pscustomobject] [ordered] @{
                status = 'match'
                live = [pscustomobject] [ordered] @{ efr = '0x0000004000000000'; efr2 = '0x0000000000000000' }
                performance_counters_supported = $false
                device_table_segments_supported = 1
            }
            configured_ranges = $configured
            decoded_state = [pscustomobject] [ordered] @{
                iommu_enabled = $false
                event_log_enabled = $false
                command_buffer_enabled = $false
                device_table_segment_encoding = 1
                event_log_running = $false
                command_buffer_running = $false
                event_overflow = $false
            }
        }
        Save-TestRawMutation $bundle $raw
    }

    Invoke-PositiveCase -Name 'v4-configured-range-oem-memory-type' -SourceBundle $baselineV4 -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.memory_map.descriptors[0].type = '0x80000000'
        $raw.amd_iommu_live.mmio.first_snapshot.device_table_base = '0x0000000000100000'
        $raw.amd_iommu_live.mmio.second_snapshot.device_table_base = '0x0000000000100000'
        $range = $raw.amd_iommu_live.mmio.configured_ranges[0]
        $range.raw = '0x0000000000100000'
        $range.base = '0x0000000000100000'
        $range.validated_range = [pscustomobject] [ordered] @{
            start = '0x0000000000100000'
            end_exclusive = '0x0000000000101000'
        }
        $range.memory_binding = [pscustomobject] [ordered] @{
            descriptor_index = 0
            descriptor = [pscustomobject] [ordered] @{
                memory_type = 2147483648
                physical_start = '0x0000000000100000'
                virtual_start = '0x0000000000000000'
                page_count = 1
                attributes = '0x0000000000000008'
            }
            requested_start = '0x0000000000100000'
            requested_end_exclusive = '0x0000000000101000'
        }
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v4-bdf-witness-falsified' -SourceBundle $baselineV4 -ExpectedMessage 'amd_iommu_live BDF decoding is inconsistent.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.amd_iommu_live.locator.unit.bdf.function = '0x01'
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v4-ecam-read-protected' -SourceBundle $baselineV4 -ExpectedMessage 'EFI_MEMORY_RP read-protected.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.memory_map.descriptors[1].attributes = '0x0000000000002001'
        $raw.amd_iommu_live.locator.ecam_memory_binding.descriptor.attributes = '0x0000000000002001'
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v4-wrong-pci-class' -SourceBundle $baselineV4 -ExpectedMessage 'wrong IOMMU class tuple' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.amd_iommu_live.pci.identity.raw.class_revision = '0x06040001'
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v4-unstable-pci-capability' -SourceBundle $baselineV4 -ExpectedMessage 'PCI capability snapshot is not stable.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.amd_iommu_live.pci.capability.second_raw.range = '0x00000001'
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v4-target-capability-invalid-next' -SourceBundle $baselineV4 -ExpectedMessage 'target capability next pointer is invalid or points back' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.amd_iommu_live.pci.capability_chain[0].next = '0x41'
        $raw.amd_iommu_live.pci.capability.first_raw.header = '0x000b410f'
        $raw.amd_iommu_live.pci.capability.second_raw.header = '0x000b410f'
        $raw.amd_iommu_live.pci.capability.decoded.next_pointer = '0x41'
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v4-pci-read-counter-falsified' -SourceBundle $baselineV4 -ExpectedMessage 'PCI read-operation count is inconsistent' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.amd_iommu_live.access.pci_read_operations++
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v4-forbidden-mmio-plan-offset' -SourceBundle $baselineV4 -ExpectedMessage 'count is inconsistent with the read plan.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.amd_iommu_live.mmio.stable_offsets = @($raw.amd_iommu_live.mmio.stable_offsets) + @('0x0038')
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v4-unstable-mmio-snapshot' -SourceBundle $baselineV4 -ExpectedMessage 'stable MMIO snapshots disagree.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.amd_iommu_live.mmio.second_snapshot.control = '0x0000000000000001'
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v4-configured-range-decode-falsified' -SourceBundle $baselineV4 -ExpectedMessage 'configured-range decode is inconsistent' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.amd_iommu_live.mmio.configured_ranges[0].length = 8192
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v4-pci-write-claimed' -SourceBundle $baselineV4 -ExpectedMessage 'pci_write_operations is outside its permitted range.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.amd_iommu_live.access.pci_write_operations = 1
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v4-ownership-claimed' -SourceBundle $baselineV4 -ExpectedMessage 'ownership.claim must remain false.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.amd_iommu_live.ownership.claim = $true
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v4-status-variant-mismatch' -SourceBundle $baselineV4 -ExpectedMessage 'top status is inconsistent with MMIO observation.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.amd_iommu_live.status = 'pci-observed-efr-conflict'
        Save-TestRawMutation $bundle $raw
    }

    [void] [System.IO.Directory]::CreateDirectory($baselineV5)
    foreach ($source in @(
        @{ Path = $verifierPath; Name = 'verify-bundle.ps1' },
        @{ Path = $schemaV5Path; Name = 'm0b-probe-v5.schema.json' },
        @{ Path = $preparePath; Name = 'prepare-media.ps1' },
        @{ Path = $finalizePath; Name = 'finalize-evidence.ps1' }
    )) {
        [System.IO.File]::Copy($source.Path, (Join-Path $baselineV5 $source.Name), $false)
    }
    [System.IO.File]::Copy((Join-Path $baseline 'svmvisor-m0b-probe.efi'), (Join-Path $baselineV5 'svmvisor-m0b-probe.efi'), $false)
    [System.IO.File]::Copy((Join-Path $baseline 'target-profile-manifest.json'), (Join-Path $baselineV5 'target-profile-manifest.json'), $false)
    $v5TargetHash = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $baselineV5 'target-profile-manifest.json')).Hash.ToLowerInvariant()
    $rawV5 = Get-Content -Raw -Encoding UTF8 -LiteralPath $fixtureV5Path | ConvertFrom-Json
    $rawV5.target_profile_manifest_sha256 = $v5TargetHash
    Write-JsonFile (Join-Path $baselineV5 'raw-probe.json') $rawV5 60
    $manifestedNamesV5 = @(
        'finalize-evidence.ps1',
        'm0b-probe-v5.schema.json',
        'prepare-media.ps1',
        'raw-probe.json',
        'svmvisor-m0b-probe.efi',
        'target-profile-manifest.json',
        'verify-bundle.ps1'
    )
    $manifestV5 = [pscustomobject] [ordered] @{
        schema_version = 5
        bundle_kind = 'svmvisor-m0b-probe-evidence'
        created_at_utc = '2026-08-06T21:00:00.0000000Z'
        target_profile_manifest_sha256 = $v5TargetHash
        raw_evidence_filename = 'svmvisor-m0b-20260806T210000-000000000.json'
        files = @(
            foreach ($name in $manifestedNamesV5) {
                New-ManifestFileRecord $baselineV5 $name
            }
        )
    }
    Write-JsonFile (Join-Path $baselineV5 'manifest.json') $manifestV5 8
    Invoke-NegativeCase `
        -Name 'v5-withdrawn-schema' `
        -SourceBundle $baselineV5 `
        -ExpectedMessage 'Schema v5 is withdrawn' `
        -Mutation { param($bundle) }

    [void] [System.IO.Directory]::CreateDirectory($baselineV6)
    foreach ($source in @(
        @{ Path = $verifierPath; Name = 'verify-bundle.ps1' },
        @{ Path = $schemaV6Path; Name = 'm0b-probe-v6.schema.json' },
        @{ Path = $preparePath; Name = 'prepare-media.ps1' },
        @{ Path = $finalizePath; Name = 'finalize-evidence.ps1' }
    )) {
        [System.IO.File]::Copy($source.Path, (Join-Path $baselineV6 $source.Name), $false)
    }
    [System.IO.File]::Copy((Join-Path $baseline 'svmvisor-m0b-probe.efi'), (Join-Path $baselineV6 'svmvisor-m0b-probe.efi'), $false)
    [System.IO.File]::Copy((Join-Path $baseline 'target-profile-manifest.json'), (Join-Path $baselineV6 'target-profile-manifest.json'), $false)
    $v6TargetHash = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $baselineV6 'target-profile-manifest.json')).Hash.ToLowerInvariant()
    $rawV6 = Get-Content -Raw -Encoding UTF8 -LiteralPath $fixtureV6Path | ConvertFrom-Json
    $rawV6.target_profile_manifest_sha256 = $v6TargetHash
    Write-JsonFile (Join-Path $baselineV6 'raw-probe.json') $rawV6 60
    $manifestedNamesV6 = @(
        'finalize-evidence.ps1',
        'm0b-probe-v6.schema.json',
        'prepare-media.ps1',
        'raw-probe.json',
        'svmvisor-m0b-probe.efi',
        'target-profile-manifest.json',
        'verify-bundle.ps1'
    )
    $manifestV6 = [pscustomobject] [ordered] @{
        schema_version = 6
        bundle_kind = 'svmvisor-m0b-probe-evidence'
        created_at_utc = '2026-08-09T01:00:00.0000000Z'
        target_profile_manifest_sha256 = $v6TargetHash
        raw_evidence_filename = 'svmvisor-m0b-20260806T210000-000000000.json'
        files = @(
            foreach ($name in $manifestedNamesV6) {
                New-ManifestFileRecord $baselineV6 $name
            }
        )
    }
    Write-JsonFile (Join-Path $baselineV6 'manifest.json') $manifestV6 8

    if ($rawV6.processor_consistency.observations[0].system_registers.smm_base.raw -ceq
        $rawV6.processor_consistency.observations[1].system_registers.smm_base.raw) {
        throw 'The v6 baseline must exercise distinct thread-scoped SMM_BASE values.'
    }
    if ($rawV6.processor_consistency.observations[0].system_registers.smm_mask.tm_type_dram -cne '0x06') {
        throw 'The v6 baseline must exercise TMTypeDram[14:12].'
    }
    if ($rawV6.processor_consistency.observations[0].system_registers.smm_mask.tseg_mask -cne '0x0000fffffe000000') {
        throw 'The v6 baseline must exercise the full 48-bit TSegMask decode.'
    }
    powershell.exe -NoProfile -ExecutionPolicy Bypass `
        -File (Join-Path $baselineV6 'verify-bundle.ps1') `
        -BundleDirectory $baselineV6
    if ($LASTEXITCODE -ne 0) {
        throw 'The corrected v6 system-register baseline bundle did not verify.'
    }

    Invoke-PositiveCase -Name 'v6-fixed-mtrrs-not-attempted' -SourceBundle $baselineV6 -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $inventories = @(
            $raw.system_registers.bsp
            foreach ($observation in @($raw.processor_consistency.observations)) {
                $observation.system_registers
            }
        )
        foreach ($inventory in $inventories) {
            $inventory.mtrr_cap.raw = '0x0000000000000408'
            $inventory.mtrr_cap.fix = $false
            $inventory.fixed_mtrrs = [pscustomobject] [ordered] @{
                status = 'not-attempted'
                reason = 'mtrrcap-fix-clear'
            }
        }
        $raw.system_registers.access.msr_read_operations = 54
        $raw.system_registers.access.msr_read_bytes = 432
        Save-TestRawMutation $bundle $raw
    }

    Invoke-PositiveCase -Name 'v6-honest-non-smm-base-inconsistency' -SourceBundle $baselineV6 -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $ap = $raw.processor_consistency.observations[1]
        $ap.system_registers.hwcr.raw = '0x0000000000000000'
        $ap.system_registers.hwcr.smm_lock = $false
        $ap.system_registers_matches_bsp = $false
        $raw.processor_consistency.system_registers_consistent = $false
        $raw.processor_consistency.consistent = $false
        Save-TestRawMutation $bundle $raw
    }

    Invoke-PositiveCase -Name 'v6-smm-48-bit-address-fields' -SourceBundle $baselineV6 -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $inventories = @(
            $raw.system_registers.bsp
            foreach ($observation in @($raw.processor_consistency.observations)) {
                $observation.system_registers
            }
        )
        foreach ($inventory in $inventories) {
            $inventory.smm_addr.raw = '0x0000abcd80000000'
            $inventory.smm_addr.tseg_base = '0x0000abcd80000000'
            $inventory.smm_mask.raw = '0x0000ffffe0006603'
            $inventory.smm_mask.tseg_mask = '0x0000ffffe0000000'
        }
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v6-smm-addr-high-bits-truncated' -SourceBundle $baselineV6 -ExpectedMessage 'processor_consistency.observations[].system_registers.smm_addr.tseg_base is inconsistent with raw.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.processor_consistency.observations[0].system_registers.smm_addr.raw = '0x0000abcd80000000'
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v6-smm-mask-high-bits-truncated' -SourceBundle $baselineV6 -ExpectedMessage 'processor_consistency.observations[].system_registers.smm_mask.tseg_mask is inconsistent with raw.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.processor_consistency.observations[0].system_registers.smm_mask.tseg_mask = '0x000000fffe000000'
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v6-msr-write-claimed' -SourceBundle $baselineV6 -ExpectedMessage 'system_registers.access.msr_write_operations is outside its permitted range.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.system_registers.access.msr_write_operations = 1
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v6-falsified-hwcr-decode' -SourceBundle $baselineV6 -ExpectedMessage 'processor_consistency.observations[].system_registers.hwcr.smm_lock is inconsistent with raw.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.processor_consistency.observations[0].system_registers.hwcr.smm_lock = $false
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v6-falsified-read-counter' -SourceBundle $baselineV6 -ExpectedMessage 'system_registers read operations do not match the recomputed site total.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.system_registers.access.msr_read_operations = 75
        $raw.system_registers.access.msr_read_bytes = 600
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v6-smm-base-only-false-witness' -SourceBundle $baselineV6 -ExpectedMessage 'processor_consistency system_registers_matches_bsp witness is falsified.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.processor_consistency.observations[1].system_registers_matches_bsp = $false
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v6-non-smm-base-difference-true-witness' -SourceBundle $baselineV6 -ExpectedMessage 'processor_consistency system_registers_matches_bsp witness is falsified.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $ap = $raw.processor_consistency.observations[1]
        $ap.system_registers.hwcr.raw = '0x0000000000000000'
        $ap.system_registers.hwcr.smm_lock = $false
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v6-wrong-tm-type-dram-decode' -SourceBundle $baselineV6 -ExpectedMessage 'processor_consistency.observations[].system_registers.smm_mask.tm_type_dram is inconsistent with raw.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.processor_consistency.observations[0].system_registers.smm_mask.tm_type_dram = '0x00'
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v6-smm-base-low-nibble-nonzero' -SourceBundle $baselineV6 -ExpectedMessage 'has nonzero required-zero SmmBase' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $ap = $raw.processor_consistency.observations[1]
        $ap.system_registers.smm_base.raw = '0x0000000000032001'
        $ap.system_registers.smm_base.smm_base_address = '0x00032001'
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v6-falsified-aggregate' -SourceBundle $baselineV6 -ExpectedMessage 'processor_consistency system_registers_consistent aggregate is falsified.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.processor_consistency.system_registers_consistent = $false
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v6-tampered-comparison-policy' -SourceBundle $baselineV6 -ExpectedMessage 'processor_consistency system-register comparison policy is unexpected.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.processor_consistency.comparison_policy.system_registers_comparison = 'exact-raw-and-observation-status'
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v6-bsp-copy-not-excluded' -SourceBundle $baselineV6 -ExpectedMessage 'system_registers.bsp does not exactly match the MP Services BSP observation.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.system_registers.bsp.smm_base.raw = '0x0000000000032000'
        $raw.system_registers.bsp.smm_base.smm_base_address = '0x00032000'
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v6-tampered-blockers' -SourceBundle $baselineV6 -ExpectedMessage 'Uncollected blockers do not match schema v6.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.uncollected_blockers[1] = 'smm-lock-and-ppr-specific-msrs'
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v6-missing-system-registers' -SourceBundle $baselineV6 -ExpectedMessage 'processor_consistency.observations[] has an unexpected property count.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.processor_consistency.observations[1].PSObject.Properties.Remove('system_registers')
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v6-vcnt-beyond-bound' -SourceBundle $baselineV6 -ExpectedMessage 'processor_consistency.observations[].system_registers.mtrr_cap.vcnt is outside its permitted range.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.processor_consistency.observations[0].system_registers.mtrr_cap.raw = '0x0000000000000509'
        $raw.processor_consistency.observations[0].system_registers.mtrr_cap.vcnt = 9
        Save-TestRawMutation $bundle $raw
    }

    Invoke-NegativeCase -Name 'v6-iorr-gate-violation' -SourceBundle $baselineV6 -ExpectedMessage 'system_registers.iorr must be not-attempted off the pinned PPR.' -Mutation {
        param($bundle)
        $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $bundle 'raw-probe.json') | ConvertFrom-Json
        $raw.processor_consistency.observations[0].system_registers.iorr.status = 'observed'
        Save-TestRawMutation $bundle $raw
    }

    Write-Host 'M0b negative verifier regressions: PASS'
}
finally {
    if (Test-Path -LiteralPath $testRoot -PathType Container) {
        $resolvedTestRoot = (Get-Item -Force -LiteralPath $testRoot).FullName
        $tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd('\') + '\'
        if (-not $resolvedTestRoot.StartsWith($tempRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Refusing to clean a test directory outside the temporary root: '$resolvedTestRoot'."
        }
        Remove-Item -Recurse -Force -LiteralPath $resolvedTestRoot
    }
}
