<#
.SYNOPSIS
Verifies a finalized svmvisor M0b probe evidence bundle.

.DESCRIPTION
Self-contained Windows PowerShell 5.1 verifier for the exact file manifest,
M0a-manifest binding, raw-evidence structure, and safety invariants. The copied
Draft 2020-12 schema remains the formal machine-readable contract; this script
does not require an external JSON Schema package.

.PARAMETER BundleDirectory
Finalized eight-file M0b evidence bundle.
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string] $BundleDirectory
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

function Assert-ExactProperties {
    param(
        [Parameter(Mandatory)] $Object,
        [Parameter(Mandatory)] [string[]] $Expected,
        [Parameter(Mandatory)] [string] $Context
    )

    Assert-Condition `
        ($Object -is [System.Management.Automation.PSCustomObject]) `
        "$Context must be a JSON object."
    $actual = @($Object.PSObject.Properties | ForEach-Object { $_.Name })
    Assert-Condition `
        ($actual.Count -eq $Expected.Count) `
        "$Context has an unexpected property count."
    $expectedSet = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($name in $Expected) {
        [void] $expectedSet.Add($name)
    }
    foreach ($name in $actual) {
        Assert-Condition `
            ($expectedSet.Contains($name)) `
            "$Context has an unexpected property: '$name'."
    }
}

function Assert-JsonArray {
    param(
        [Parameter(Mandatory)] [AllowEmptyCollection()] $Value,
        [Parameter(Mandatory)] [string] $Context
    )

    Assert-Condition ($Value -is [System.Array]) "$Context must be a JSON array."
}

function Assert-JsonBoolean {
    param(
        [Parameter(Mandatory)] [AllowNull()] $Value,
        [Parameter(Mandatory)] [string] $Context
    )

    Assert-Condition ($Value -is [bool]) "$Context must be a JSON Boolean."
}

function Assert-JsonString {
    param(
        [Parameter(Mandatory)] [AllowEmptyString()] $Value,
        [Parameter(Mandatory)] [string] $Context,
        [string] $Pattern
    )

    Assert-Condition ($Value -is [string]) "$Context must be a JSON string."
    if ($PSBoundParameters.ContainsKey('Pattern')) {
        Assert-Condition ($Value -cmatch $Pattern) "$Context is not canonical."
    }
}

function Assert-JsonInteger {
    param(
        [Parameter(Mandatory)] [AllowNull()] $Value,
        [Parameter(Mandatory)] [string] $Context,
        [decimal] $Minimum = 0,
        [decimal] $Maximum = [decimal]::MaxValue
    )

    Assert-Condition ($null -ne $Value) "$Context must be an integer, not null."
    Assert-Condition (-not ($Value -is [bool])) "$Context must be an integer, not a Boolean."
    $numericTypes = @(
        [byte], [sbyte], [int16], [uint16], [int32], [uint32], [int64], [uint64],
        [single], [double], [decimal]
    )
    Assert-Condition ($Value.GetType() -in $numericTypes) "$Context must be a JSON integer."
    try {
        $number = [decimal] $Value
    }
    catch {
        throw "$Context is outside the supported integer range."
    }
    Assert-Condition `
        ([decimal]::Truncate($number) -eq $number) `
        "$Context must not have a fractional value."
    Assert-Condition `
        ($number -ge $Minimum -and $number -le $Maximum) `
        "$Context is outside its permitted range."
}

function Assert-LowerSha256 {
    param($Value, [string] $Context)
    Assert-JsonString -Value $Value -Context $Context -Pattern '^[0-9a-f]{64}$'
    Assert-Condition ($Value.Length -eq 64) "$Context must be exactly 64 characters."
}

function Assert-Hex32 {
    param($Value, [string] $Context)
    Assert-JsonString -Value $Value -Context $Context -Pattern '^0x[0-9a-f]{8}$'
    Assert-Condition ($Value.Length -eq 10) "$Context must be fixed-width hex32."
}

function Assert-Hex8 {
    param($Value, [string] $Context)
    Assert-JsonString -Value $Value -Context $Context -Pattern '^0x[0-9a-f]{2}$'
    Assert-Condition ($Value.Length -eq 4) "$Context must be fixed-width hex8."
}

function Assert-Hex16 {
    param($Value, [string] $Context)
    Assert-JsonString -Value $Value -Context $Context -Pattern '^0x[0-9a-f]{4}$'
    Assert-Condition ($Value.Length -eq 6) "$Context must be fixed-width hex16."
}

function Assert-Hex64 {
    param($Value, [string] $Context)
    Assert-JsonString -Value $Value -Context $Context -Pattern '^0x[0-9a-f]{16}$'
    Assert-Condition ($Value.Length -eq 18) "$Context must be fixed-width hex64."
}

function Assert-NoDuplicateJsonPropertyNames {
    param(
        [Parameter(Mandatory)] [string] $JsonText,
        [Parameter(Mandatory)] [string] $Context
    )

    # ConvertFrom-Json materializes only one value for a repeated object name.
    # Scan string tokens before materialization; in valid JSON a string followed
    # by optional whitespace and ':' can only be an object property name.
    $frames = New-Object 'System.Collections.Generic.Stack[object]'
    $index = 0
    while ($index -lt $JsonText.Length) {
        $character = $JsonText[$index]
        if ($character -eq '{') {
            $frames.Push([pscustomobject] @{
                Kind = 'object'
                Keys = [System.Collections.Generic.HashSet[string]]::new(
                    [System.StringComparer]::Ordinal
                )
            })
            $index++
            continue
        }
        if ($character -eq '[') {
            $frames.Push([pscustomobject] @{ Kind = 'array'; Keys = $null })
            $index++
            continue
        }
        if ($character -eq '}' -or $character -eq ']') {
            if ($frames.Count -gt 0) {
                [void] $frames.Pop()
            }
            $index++
            continue
        }
        if ($character -ne '"') {
            $index++
            continue
        }

        $tokenStart = $index
        $index++
        $closed = $false
        while ($index -lt $JsonText.Length) {
            if ($JsonText[$index] -eq '\') {
                $index += 2
                continue
            }
            if ($JsonText[$index] -eq '"') {
                $index++
                $closed = $true
                break
            }
            $index++
        }
        if (-not $closed) {
            throw "$Context contains an unterminated JSON string."
        }

        $next = $index
        while ($next -lt $JsonText.Length -and [char]::IsWhiteSpace($JsonText[$next])) {
            $next++
        }
        if ($next -ge $JsonText.Length -or $JsonText[$next] -ne ':') {
            continue
        }
        Assert-Condition `
            ($frames.Count -gt 0 -and $frames.Peek().Kind -ceq 'object') `
            "$Context contains a property name outside an object."
        $token = $JsonText.Substring($tokenStart, $index - $tokenStart)
        $key = $token | ConvertFrom-Json
        Assert-Condition `
            ($frames.Peek().Keys.Add([string] $key)) `
            "$Context contains a duplicate JSON property name: '$key'."
    }
}

function Convert-Hex32 {
    param([Parameter(Mandatory)] [string] $Value)
    return [System.Convert]::ToUInt32($Value.Substring(2), 16)
}

function Convert-Hex64 {
    param([Parameter(Mandatory)] [string] $Value)
    return [System.Convert]::ToUInt64($Value.Substring(2), 16)
}

function Assert-ObservedOrNotEnumerated {
    param(
        [Parameter(Mandatory)] $Record,
        [Parameter(Mandatory)] [bool] $ShouldBeObserved,
        [Parameter(Mandatory)] $ExpectedValue,
        [Parameter(Mandatory)] [ValidateSet('boolean', 'uint8', 'uint32')] [string] $ValueKind,
        [Parameter(Mandatory)] [string] $Context
    )

    if ($ShouldBeObserved) {
        Assert-ExactProperties -Object $Record -Expected @('status', 'value') -Context $Context
        Assert-Condition (Test-OrdinalEqual $Record.status 'observed') "$Context must be observed."
        switch ($ValueKind) {
            'boolean' {
                Assert-JsonBoolean -Value $Record.value -Context "$Context.value"
                Assert-Condition ($Record.value -eq $ExpectedValue) "$Context decoded value is inconsistent with raw CPUID."
            }
            'uint8' {
                Assert-JsonInteger -Value $Record.value -Context "$Context.value" -Maximum 255
                Assert-Condition ([decimal] $Record.value -eq [decimal] $ExpectedValue) "$Context decoded value is inconsistent with raw CPUID."
            }
            'uint32' {
                Assert-JsonInteger -Value $Record.value -Context "$Context.value" -Maximum 4294967295
                Assert-Condition ([decimal] $Record.value -eq [decimal] $ExpectedValue) "$Context decoded value is inconsistent with raw CPUID."
            }
        }
    }
    else {
        Assert-ExactProperties -Object $Record -Expected @('status') -Context $Context
        Assert-Condition `
            (Test-OrdinalEqual $Record.status 'not-enumerated') `
            "$Context must explicitly be not-enumerated."
    }
}

function Assert-VendorObservedOrNotEnumerated {
    param(
        [Parameter(Mandatory)] $Record,
        [Parameter(Mandatory)] [bool] $AuthenticAmd,
        [Parameter(Mandatory)] [bool] $ShouldBeObserved,
        [Parameter(Mandatory)] $ExpectedValue,
        [Parameter(Mandatory)] [ValidateSet('boolean', 'uint8', 'uint32')] [string] $ValueKind,
        [Parameter(Mandatory)] [string] $Context
    )

    if (-not $AuthenticAmd) {
        Assert-ExactProperties -Object $Record -Expected @('status') -Context $Context
        Assert-Condition `
            (Test-OrdinalEqual $Record.status 'not-applicable-vendor') `
            "$Context must explicitly be not-applicable-vendor on a non-AMD CPU."
        return
    }
    Assert-ObservedOrNotEnumerated `
        -Record $Record `
        -ShouldBeObserved $ShouldBeObserved `
        -ExpectedValue $ExpectedValue `
        -ValueKind $ValueKind `
        -Context $Context
}

function Get-RegisterBytes {
    param([Parameter(Mandatory)] [uint32] $Register)
    return [System.BitConverter]::GetBytes($Register)
}

function Convert-TrimmedCpuidText {
    param([Parameter(Mandatory)] [byte[]] $Bytes)

    $characters = @($Bytes | ForEach-Object { [char] $_ })
    $text = -join $characters
    return $text.TrimEnd([char[]] @([char] 0, [char] 0x20))
}

function Assert-RawCpuid {
    param([Parameter(Mandatory)] $Cpu)

    Assert-ExactProperties -Object $Cpu -Expected @(
        'observation_scope',
        'vendor',
        'authentic_amd',
        'max_basic_leaf',
        'max_extended_leaf',
        'brand',
        'family_model_stepping',
        'raw_leaves',
        'decoded'
    ) -Context 'cpu'
    Assert-Condition `
        (Test-OrdinalEqual $Cpu.observation_scope 'executing-processor-presumed-bsp-not-cross-processor-validated') `
        'Unexpected CPU observation scope.'
    Assert-JsonString -Value $Cpu.vendor -Context 'cpu.vendor'
    Assert-Condition ($Cpu.vendor.Length -le 12) 'cpu.vendor exceeds 12 CPUID bytes.'
    Assert-JsonBoolean -Value $Cpu.authentic_amd -Context 'cpu.authentic_amd'
    Assert-Hex32 -Value $Cpu.max_basic_leaf -Context 'cpu.max_basic_leaf'
    Assert-Hex32 -Value $Cpu.max_extended_leaf -Context 'cpu.max_extended_leaf'
    Assert-JsonArray -Value $Cpu.raw_leaves -Context 'cpu.raw_leaves'

    $leaves = @($Cpu.raw_leaves)
    $leafMap = [System.Collections.Generic.Dictionary[string, object]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($leafRecord in $leaves) {
        Assert-ExactProperties -Object $leafRecord -Expected @(
            'leaf', 'subleaf', 'eax', 'ebx', 'ecx', 'edx'
        ) -Context 'cpu.raw_leaves[]'
        Assert-Hex32 -Value $leafRecord.leaf -Context 'cpu.raw_leaves[].leaf'
        Assert-Condition `
            (Test-OrdinalEqual $leafRecord.subleaf '0x00000000') `
            'Every schema-v1 CPUID subleaf must be zero.'
        foreach ($registerName in @('eax', 'ebx', 'ecx', 'edx')) {
            Assert-Hex32 -Value $leafRecord.$registerName -Context "cpu.raw_leaves[].$registerName"
        }
        Assert-Condition `
            (-not $leafMap.ContainsKey([string] $leafRecord.leaf)) `
            "Duplicate raw CPUID leaf: '$($leafRecord.leaf)'."
        $leafMap.Add([string] $leafRecord.leaf, $leafRecord)
    }

    Assert-Condition ($leafMap.ContainsKey('0x00000000')) 'Raw CPUID leaf 0 is missing.'
    Assert-Condition ($leafMap.ContainsKey('0x80000000')) 'Raw extended CPUID maximum leaf is missing.'
    $basicMaximum = Convert-Hex32 $leafMap['0x00000000'].eax
    $extendedMaximum = Convert-Hex32 $leafMap['0x80000000'].eax
    $expectedLeaves = New-Object System.Collections.Generic.List[string]
    [void] $expectedLeaves.Add('0x00000000')
    if ($basicMaximum -ge 1) { [void] $expectedLeaves.Add('0x00000001') }
    if ($basicMaximum -ge 7) { [void] $expectedLeaves.Add('0x00000007') }
    [void] $expectedLeaves.Add('0x80000000')
    if ($extendedMaximum -ge 2147483649) { [void] $expectedLeaves.Add('0x80000001') }
    if ($extendedMaximum -ge 2147483652) {
        [void] $expectedLeaves.Add('0x80000002')
        [void] $expectedLeaves.Add('0x80000003')
        [void] $expectedLeaves.Add('0x80000004')
    }
    if ($extendedMaximum -ge 2147483656) { [void] $expectedLeaves.Add('0x80000008') }
    if ($extendedMaximum -ge 2147483658) { [void] $expectedLeaves.Add('0x8000000a') }
    if ($extendedMaximum -ge 2147483678) { [void] $expectedLeaves.Add('0x8000001e') }
    if ($extendedMaximum -ge 2147483679) { [void] $expectedLeaves.Add('0x8000001f') }

    Assert-Condition ($leaves.Count -eq $expectedLeaves.Count) 'Raw CPUID leaf count does not match maximum-leaf gating.'
    for ($index = 0; $index -lt $expectedLeaves.Count; $index++) {
        Assert-Condition `
            (Test-OrdinalEqual $leaves[$index].leaf $expectedLeaves[$index]) `
            'Raw CPUID leaves are missing, unexpected, or out of canonical order.'
    }
    Assert-Condition `
        (Test-OrdinalEqual $Cpu.max_basic_leaf $leafMap['0x00000000'].eax) `
        'cpu.max_basic_leaf does not match raw leaf 0 EAX.'
    Assert-Condition `
        (Test-OrdinalEqual $Cpu.max_extended_leaf $leafMap['0x80000000'].eax) `
        'cpu.max_extended_leaf does not match raw extended leaf EAX.'

    $vendorBytes = New-Object System.Collections.Generic.List[byte]
    foreach ($registerName in @('ebx', 'edx', 'ecx')) {
        foreach ($byte in (Get-RegisterBytes (Convert-Hex32 $leafMap['0x00000000'].$registerName))) {
            [void] $vendorBytes.Add($byte)
        }
    }
    $expectedVendor = Convert-TrimmedCpuidText -Bytes $vendorBytes.ToArray()
    Assert-Condition (Test-OrdinalEqual $Cpu.vendor $expectedVendor) 'cpu.vendor does not match raw CPUID leaf 0.'
    $expectedAuthenticAmd = Test-OrdinalEqual $expectedVendor 'AuthenticAMD'
    Assert-Condition ($Cpu.authentic_amd -eq $expectedAuthenticAmd) 'cpu.authentic_amd does not match the raw vendor string.'

    $hasBrand = $leafMap.ContainsKey('0x80000002')
    if ($hasBrand) {
        Assert-ExactProperties -Object $Cpu.brand -Expected @('status', 'value') -Context 'cpu.brand'
        Assert-Condition (Test-OrdinalEqual $Cpu.brand.status 'observed') 'cpu.brand must be observed when its leaves are enumerated.'
        Assert-JsonString -Value $Cpu.brand.value -Context 'cpu.brand.value'
        $brandBytes = New-Object System.Collections.Generic.List[byte]
        foreach ($leafName in @('0x80000002', '0x80000003', '0x80000004')) {
            foreach ($registerName in @('eax', 'ebx', 'ecx', 'edx')) {
                foreach ($byte in (Get-RegisterBytes (Convert-Hex32 $leafMap[$leafName].$registerName))) {
                    [void] $brandBytes.Add($byte)
                }
            }
        }
        $expectedBrand = Convert-TrimmedCpuidText -Bytes $brandBytes.ToArray()
        Assert-Condition (Test-OrdinalEqual $Cpu.brand.value $expectedBrand) 'cpu.brand does not match the raw brand leaves.'
    }
    else {
        Assert-ExactProperties -Object $Cpu.brand -Expected @('status') -Context 'cpu.brand'
        Assert-Condition (Test-OrdinalEqual $Cpu.brand.status 'not-enumerated') 'cpu.brand must explicitly be not-enumerated.'
    }

    $hasLeaf1 = $leafMap.ContainsKey('0x00000001')
    if ($hasLeaf1) {
        Assert-ExactProperties -Object $Cpu.family_model_stepping -Expected @(
            'status', 'family', 'model', 'stepping'
        ) -Context 'cpu.family_model_stepping'
        Assert-Condition (Test-OrdinalEqual $Cpu.family_model_stepping.status 'observed') 'CPU family/model/stepping must be observed.'
        $leaf1Eax = Convert-Hex32 $leafMap['0x00000001'].eax
        $stepping = $leaf1Eax -band 0x0f
        $baseModel = ($leaf1Eax -shr 4) -band 0x0f
        $baseFamily = ($leaf1Eax -shr 8) -band 0x0f
        $extendedModel = ($leaf1Eax -shr 16) -band 0x0f
        $extendedFamily = ($leaf1Eax -shr 20) -band 0xff
        $family = if ($baseFamily -eq 0x0f) { $baseFamily + $extendedFamily } else { $baseFamily }
        $model = if ($baseFamily -in @(0x06, 0x0f)) { $baseModel -bor ($extendedModel -shl 4) } else { $baseModel }
        foreach ($field in @('family', 'model', 'stepping')) {
            Assert-JsonInteger -Value $Cpu.family_model_stepping.$field -Context "cpu.family_model_stepping.$field" -Maximum 65535
        }
        Assert-Condition ($Cpu.family_model_stepping.family -eq $family) 'Decoded CPU family is inconsistent.'
        Assert-Condition ($Cpu.family_model_stepping.model -eq $model) 'Decoded CPU model is inconsistent.'
        Assert-Condition ($Cpu.family_model_stepping.stepping -eq $stepping) 'Decoded CPU stepping is inconsistent.'
    }
    else {
        Assert-ExactProperties -Object $Cpu.family_model_stepping -Expected @('status') -Context 'cpu.family_model_stepping'
        Assert-Condition (Test-OrdinalEqual $Cpu.family_model_stepping.status 'not-enumerated') 'CPU family/model/stepping must explicitly be not-enumerated.'
    }

    Assert-ExactProperties -Object $Cpu.decoded -Expected @(
        'semantics',
        'svm_capable',
        'physical_address_bits',
        'svm_revision',
        'asid_count',
        'nested_paging_capable',
        'svm_lock_capable',
        'nrip_save_capable',
        'vmcb_clean_bits_capable',
        'flush_by_asid_capable',
        'decode_assists_capable',
        'sme_capable',
        'sev_capable',
        'c_bit_position',
        'physical_address_reduction'
    ) -Context 'cpu.decoded'

    Assert-Condition `
        (Test-OrdinalEqual $Cpu.decoded.semantics 'cpuid-capability-enumeration-not-enabled-state') `
        'Unexpected decoded CPUID semantics.'
    $has80000001 = $leafMap.ContainsKey('0x80000001')
    $svm = if ($has80000001) { ((Convert-Hex32 $leafMap['0x80000001'].ecx) -band (1 -shl 2)) -ne 0 } else { $false }
    Assert-VendorObservedOrNotEnumerated -Record $Cpu.decoded.svm_capable -AuthenticAmd $expectedAuthenticAmd -ShouldBeObserved $has80000001 -ExpectedValue $svm -ValueKind boolean -Context 'cpu.decoded.svm_capable'

    $has80000008 = $leafMap.ContainsKey('0x80000008')
    $physicalBits = if ($has80000008) { (Convert-Hex32 $leafMap['0x80000008'].eax) -band 0xff } else { 0 }
    Assert-ObservedOrNotEnumerated -Record $Cpu.decoded.physical_address_bits -ShouldBeObserved $has80000008 -ExpectedValue $physicalBits -ValueKind uint8 -Context 'cpu.decoded.physical_address_bits'

    $has8000000a = $leafMap.ContainsKey('0x8000000a')
    $svmRevision = if ($has8000000a) { (Convert-Hex32 $leafMap['0x8000000a'].eax) -band 0xff } else { 0 }
    $asidCount = if ($has8000000a) { Convert-Hex32 $leafMap['0x8000000a'].ebx } else { 0 }
    $svmFeatures = if ($has8000000a) { Convert-Hex32 $leafMap['0x8000000a'].edx } else { 0 }
    Assert-VendorObservedOrNotEnumerated -Record $Cpu.decoded.svm_revision -AuthenticAmd $expectedAuthenticAmd -ShouldBeObserved $has8000000a -ExpectedValue $svmRevision -ValueKind uint8 -Context 'cpu.decoded.svm_revision'
    Assert-VendorObservedOrNotEnumerated -Record $Cpu.decoded.asid_count -AuthenticAmd $expectedAuthenticAmd -ShouldBeObserved $has8000000a -ExpectedValue $asidCount -ValueKind uint32 -Context 'cpu.decoded.asid_count'
    $svmBitFields = [ordered] @{
        nested_paging_capable = 0
        svm_lock_capable = 2
        nrip_save_capable = 3
        vmcb_clean_bits_capable = 5
        flush_by_asid_capable = 6
        decode_assists_capable = 7
    }
    foreach ($entry in $svmBitFields.GetEnumerator()) {
        $expected = ($svmFeatures -band (1 -shl $entry.Value)) -ne 0
        Assert-VendorObservedOrNotEnumerated -Record $Cpu.decoded.($entry.Key) -AuthenticAmd $expectedAuthenticAmd -ShouldBeObserved $has8000000a -ExpectedValue $expected -ValueKind boolean -Context "cpu.decoded.$($entry.Key)"
    }

    $has8000001f = $leafMap.ContainsKey('0x8000001f')
    $encryptionEax = if ($has8000001f) { Convert-Hex32 $leafMap['0x8000001f'].eax } else { 0 }
    $encryptionEbx = if ($has8000001f) { Convert-Hex32 $leafMap['0x8000001f'].ebx } else { 0 }
    Assert-VendorObservedOrNotEnumerated -Record $Cpu.decoded.sme_capable -AuthenticAmd $expectedAuthenticAmd -ShouldBeObserved $has8000001f -ExpectedValue (($encryptionEax -band 1) -ne 0) -ValueKind boolean -Context 'cpu.decoded.sme_capable'
    Assert-VendorObservedOrNotEnumerated -Record $Cpu.decoded.sev_capable -AuthenticAmd $expectedAuthenticAmd -ShouldBeObserved $has8000001f -ExpectedValue (($encryptionEax -band 2) -ne 0) -ValueKind boolean -Context 'cpu.decoded.sev_capable'
    Assert-VendorObservedOrNotEnumerated -Record $Cpu.decoded.c_bit_position -AuthenticAmd $expectedAuthenticAmd -ShouldBeObserved $has8000001f -ExpectedValue ($encryptionEbx -band 0x3f) -ValueKind uint8 -Context 'cpu.decoded.c_bit_position'
    Assert-VendorObservedOrNotEnumerated -Record $Cpu.decoded.physical_address_reduction -AuthenticAmd $expectedAuthenticAmd -ShouldBeObserved $has8000001f -ExpectedValue (($encryptionEbx -shr 6) -band 0x3f) -ValueKind uint8 -Context 'cpu.decoded.physical_address_reduction'

    return [pscustomobject] @{
        AuthenticAmd = $expectedAuthenticAmd
        Svm = if ($has80000001) { [Nullable[bool]] $svm } else { [Nullable[bool]] $null }
        HasSvmLeaf = $has8000000a
    }
}

function Assert-RawEvidenceV1 {
    param(
        [Parameter(Mandatory)] $Evidence,
        [Parameter(Mandatory)] [string] $ExpectedTargetManifestSha256,
        [Parameter(Mandatory)] [string] $ExpectedRawEvidenceFilename
    )

    Assert-ExactProperties -Object $Evidence -Expected @(
        'schema_version',
        'evidence_kind',
        'qualification_status',
        'launch_authorized',
        'physical_candidate_flash_authorized',
        'control_state_writes_authorized',
        'process_introspection_authorized',
        'confidential_vm_claim',
        'collector',
        'target_profile_manifest_sha256',
        'collected_at',
        'sink',
        'uefi',
        'cpu',
        'vm_cr',
        'mp_services',
        'memory_map',
        'uncollected_blockers'
    ) -Context 'raw evidence'
    Assert-JsonInteger -Value $Evidence.schema_version -Context 'schema_version' -Minimum 1 -Maximum 1
    Assert-Condition (Test-OrdinalEqual $Evidence.evidence_kind 'uefi-read-only-inventory-slice') 'Unexpected evidence_kind.'
    Assert-Condition (Test-OrdinalEqual $Evidence.qualification_status 'blocked') 'Raw evidence must remain qualification_status=blocked.'
    foreach ($field in @(
        'launch_authorized',
        'physical_candidate_flash_authorized',
        'control_state_writes_authorized',
        'process_introspection_authorized',
        'confidential_vm_claim'
    )) {
        Assert-JsonBoolean -Value $Evidence.$field -Context $field
        Assert-Condition ($Evidence.$field -eq $false) "Raw evidence cannot set $field."
    }

    Assert-ExactProperties -Object $Evidence.collector -Expected @('name', 'version', 'slice') -Context 'collector'
    Assert-Condition (Test-OrdinalEqual $Evidence.collector.name 'svmvisor-m0b-probe') 'Unexpected collector name.'
    Assert-Condition (Test-OrdinalEqual $Evidence.collector.version '0.1.0') 'Unexpected collector version.'
    Assert-Condition (Test-OrdinalEqual $Evidence.collector.slice 'cpuid-vm-cr-uefi-mp-memory-map') 'Unexpected collector slice.'
    Assert-LowerSha256 -Value $Evidence.target_profile_manifest_sha256 -Context 'target_profile_manifest_sha256'
    Assert-Condition `
        (Test-OrdinalEqual $Evidence.target_profile_manifest_sha256 $ExpectedTargetManifestSha256) `
        'Raw evidence is bound to the wrong target-profile manifest.'

    Assert-ExactProperties -Object $Evidence.collected_at -Expected @(
        'year', 'month', 'day', 'hour', 'minute', 'second', 'nanosecond', 'timezone'
    ) -Context 'collected_at'
    Assert-JsonInteger $Evidence.collected_at.year 'collected_at.year' 1900 9999
    Assert-JsonInteger $Evidence.collected_at.month 'collected_at.month' 1 12
    Assert-JsonInteger $Evidence.collected_at.day 'collected_at.day' 1 31
    Assert-JsonInteger $Evidence.collected_at.hour 'collected_at.hour' 0 23
    Assert-JsonInteger $Evidence.collected_at.minute 'collected_at.minute' 0 59
    Assert-JsonInteger $Evidence.collected_at.second 'collected_at.second' 0 59
    Assert-JsonInteger $Evidence.collected_at.nanosecond 'collected_at.nanosecond' 0 999999999
    if (Test-OrdinalEqual $Evidence.collected_at.timezone.status 'observed') {
        Assert-ExactProperties $Evidence.collected_at.timezone @('status', 'minutes_from_utc') 'collected_at.timezone'
        Assert-JsonInteger $Evidence.collected_at.timezone.minutes_from_utc 'collected_at.timezone.minutes_from_utc' -1440 1440
    }
    else {
        Assert-ExactProperties $Evidence.collected_at.timezone @('status') 'collected_at.timezone'
        Assert-Condition (Test-OrdinalEqual $Evidence.collected_at.timezone.status 'unspecified') 'Unexpected collected_at.timezone status.'
    }

    Assert-ExactProperties -Object $Evidence.sink -Expected @(
        'output_file', 'selection', 'media_id', 'removable_media', 'media_present',
        'logical_partition', 'read_only', 'block_size', 'last_block'
    ) -Context 'sink'
    Assert-JsonString -Value $Evidence.sink.output_file -Context 'sink.output_file' -Pattern '^\\svmvisor-m0b-[0-9]{8}T[0-9]{6}-[0-9]{9}\.json$'
    Assert-Condition (Test-OrdinalEqual $Evidence.sink.selection 'loaded-image-volume-plus-profile-binding-marker') 'Unexpected sink selection rule.'
    Assert-Hex32 -Value $Evidence.sink.media_id -Context 'sink.media_id'
    foreach ($field in @('removable_media', 'media_present', 'logical_partition', 'read_only')) {
        Assert-JsonBoolean -Value $Evidence.sink.$field -Context "sink.$field"
    }
    Assert-Condition ($Evidence.sink.removable_media -eq $true) 'The evidence sink was not firmware-reported removable media.'
    Assert-Condition ($Evidence.sink.media_present -eq $true) 'The evidence sink did not report media present.'
    Assert-Condition ($Evidence.sink.read_only -eq $false) 'The evidence sink was read-only.'
    Assert-JsonInteger $Evidence.sink.block_size 'sink.block_size' 1 4294967295
    Assert-JsonInteger $Evidence.sink.last_block 'sink.last_block' 0 18446744073709551615
    $expectedOutputFile = '\svmvisor-m0b-{0:D4}{1:D2}{2:D2}T{3:D2}{4:D2}{5:D2}-{6:D9}.json' -f @(
        [int] $Evidence.collected_at.year,
        [int] $Evidence.collected_at.month,
        [int] $Evidence.collected_at.day,
        [int] $Evidence.collected_at.hour,
        [int] $Evidence.collected_at.minute,
        [int] $Evidence.collected_at.second,
        [int] $Evidence.collected_at.nanosecond
    )
    Assert-Condition (Test-OrdinalEqual $Evidence.sink.output_file $expectedOutputFile) 'sink.output_file does not match collected_at.'
    Assert-Condition `
        (Test-OrdinalEqual $ExpectedRawEvidenceFilename $Evidence.sink.output_file.Substring(1)) `
        'The recorded raw-evidence filename does not match sink.output_file.'

    Assert-ExactProperties -Object $Evidence.uefi -Expected @(
        'firmware_vendor', 'firmware_revision', 'specification_revision', 'configuration_tables'
    ) -Context 'uefi'
    Assert-JsonString -Value $Evidence.uefi.firmware_vendor -Context 'uefi.firmware_vendor'
    Assert-JsonInteger $Evidence.uefi.firmware_revision 'uefi.firmware_revision' 0 4294967295
    Assert-ExactProperties $Evidence.uefi.specification_revision @('major', 'minor') 'uefi.specification_revision'
    Assert-JsonInteger $Evidence.uefi.specification_revision.major 'uefi.specification_revision.major' 0 65535
    Assert-JsonInteger $Evidence.uefi.specification_revision.minor 'uefi.specification_revision.minor' 0 65535
    Assert-JsonArray -Value $Evidence.uefi.configuration_tables -Context 'uefi.configuration_tables'
    $knownConfigurationTables = @{
        '8868e871-e4f1-11d3-bc22-0080c73c8881' = 'acpi2-rsdp'
        'eb9d2d30-2d88-11d3-9a16-0090273fc14d' = 'acpi1-rsdp'
        'f2fd1544-9794-4a2c-992e-e5bbcf20e394' = 'smbios3-entry-point'
        'eb9d2d31-2d88-11d3-9a16-0090273fc14d' = 'smbios-entry-point'
        'b122a263-3661-4f68-9929-78f8b0d62180' = 'esrt'
    }
    foreach ($table in @($Evidence.uefi.configuration_tables)) {
        Assert-ExactProperties $table @('kind', 'guid', 'address') 'uefi.configuration_tables[]'
        Assert-JsonString $table.guid 'uefi.configuration_tables[].guid' '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
        Assert-Hex64 $table.address 'uefi.configuration_tables[].address'
        $expectedKind = if ($knownConfigurationTables.ContainsKey($table.guid)) {
            $knownConfigurationTables[$table.guid]
        } else {
            'other'
        }
        Assert-Condition (Test-OrdinalEqual $table.kind $expectedKind) 'Configuration-table kind does not match its GUID.'
    }

    $cpuFacts = Assert-RawCpuid -Cpu $Evidence.cpu
    $vmCrShouldBeObserved = (
        $cpuFacts.AuthenticAmd -and
        $null -ne $cpuFacts.Svm -and
        $cpuFacts.Svm -eq $true -and
        $cpuFacts.HasSvmLeaf
    )
    if ($vmCrShouldBeObserved) {
        Assert-ExactProperties $Evidence.vm_cr @('status', 'raw', 'dpd', 'r_init', 'dis_a20m', 'lock', 'svmdis') 'vm_cr'
        Assert-Condition (Test-OrdinalEqual $Evidence.vm_cr.status 'observed') 'VM_CR must be observed for an enumerated AMD SVM CPU.'
        Assert-Hex64 $Evidence.vm_cr.raw 'vm_cr.raw'
        $vmCrRaw = Convert-Hex64 $Evidence.vm_cr.raw
        $vmCrBits = [ordered] @{ dpd = 0; r_init = 1; dis_a20m = 2; lock = 3; svmdis = 4 }
        foreach ($entry in $vmCrBits.GetEnumerator()) {
            Assert-JsonBoolean $Evidence.vm_cr.($entry.Key) "vm_cr.$($entry.Key)"
            $expected = ($vmCrRaw -band ([uint64] 1 -shl $entry.Value)) -ne 0
            Assert-Condition ($Evidence.vm_cr.($entry.Key) -eq $expected) "vm_cr.$($entry.Key) is inconsistent with vm_cr.raw."
        }
    }
    else {
        Assert-ExactProperties $Evidence.vm_cr @('status', 'reason') 'vm_cr'
        Assert-Condition (Test-OrdinalEqual $Evidence.vm_cr.status 'not-attempted') 'VM_CR must explicitly be not-attempted.'
        Assert-Condition (Test-OrdinalEqual $Evidence.vm_cr.reason 'cpu-is-not-authentic-amd-or-svm-is-not-enumerated') 'Unexpected VM_CR not-attempted reason.'
    }

    if (Test-OrdinalEqual $Evidence.mp_services.status 'observed') {
        Assert-ExactProperties $Evidence.mp_services @(
            'status', 'total', 'enabled', 'record_count', 'enabled_record_count',
            'counts_consistent', 'processors'
        ) 'mp_services'
        foreach ($field in @('total', 'enabled', 'record_count', 'enabled_record_count')) {
            Assert-JsonInteger $Evidence.mp_services.$field "mp_services.$field" 0 ([decimal]::MaxValue)
        }
        Assert-JsonBoolean $Evidence.mp_services.counts_consistent 'mp_services.counts_consistent'
        Assert-JsonArray $Evidence.mp_services.processors 'mp_services.processors'
        $processors = @($Evidence.mp_services.processors)
        $enabledRecords = 0
        for ($index = 0; $index -lt $processors.Count; $index++) {
            $processor = $processors[$index]
            Assert-ExactProperties $processor @(
                'processor_number', 'processor_id', 'id_semantics', 'bsp', 'enabled', 'healthy', 'location'
            ) 'mp_services.processors[]'
            Assert-JsonInteger $processor.processor_number 'processor.processor_number' 0 ([decimal]::MaxValue)
            Assert-Condition ($processor.processor_number -eq $index) 'Processor records are not in contiguous processor-number order.'
            Assert-Hex64 $processor.processor_id 'processor.processor_id'
            Assert-Condition (Test-OrdinalEqual $processor.id_semantics 'firmware-hardware-id-not-yet-acpi-cross-checked') 'Unexpected processor ID semantics.'
            foreach ($field in @('bsp', 'enabled', 'healthy')) {
                Assert-JsonBoolean $processor.$field "processor.$field"
            }
            if ($processor.enabled) { $enabledRecords++ }
            Assert-ExactProperties $processor.location @('package', 'core', 'thread') 'processor.location'
            foreach ($field in @('package', 'core', 'thread')) {
                Assert-JsonInteger $processor.location.$field "processor.location.$field" 0 4294967295
            }
        }
        Assert-Condition ($Evidence.mp_services.record_count -eq $processors.Count) 'MP record_count does not match the processor array.'
        Assert-Condition ($Evidence.mp_services.enabled_record_count -eq $enabledRecords) 'MP enabled_record_count does not match processor records.'
        $expectedConsistency = (
            $Evidence.mp_services.total -eq $processors.Count -and
            $Evidence.mp_services.enabled -eq $enabledRecords
        )
        Assert-Condition ($Evidence.mp_services.counts_consistent -eq $expectedConsistency) 'MP counts_consistent is not an honest witness of the recorded counts.'
    }
    else {
        Assert-ExactProperties $Evidence.mp_services @('status', 'uefi_status') 'mp_services'
        Assert-Condition (Test-OrdinalEqual $Evidence.mp_services.status 'unavailable') 'Unexpected mp_services status.'
        Assert-Hex64 $Evidence.mp_services.uefi_status 'mp_services.uefi_status'
    }

    if (Test-OrdinalEqual $Evidence.memory_map.status 'observed') {
        Assert-ExactProperties $Evidence.memory_map @(
            'status', 'phase', 'descriptor_size', 'descriptor_version', 'descriptors'
        ) 'memory_map'
        Assert-JsonInteger $Evidence.memory_map.descriptor_size 'memory_map.descriptor_size' 1 ([decimal]::MaxValue)
        Assert-JsonInteger $Evidence.memory_map.descriptor_version 'memory_map.descriptor_version' 0 4294967295
        Assert-JsonArray $Evidence.memory_map.descriptors 'memory_map.descriptors'
        foreach ($descriptor in @($Evidence.memory_map.descriptors)) {
            Assert-ExactProperties $descriptor @('type', 'physical_start', 'virtual_start', 'page_count', 'attributes') 'memory_map.descriptors[]'
            Assert-Hex32 $descriptor.type 'memory descriptor type'
            Assert-Hex64 $descriptor.physical_start 'memory descriptor physical_start'
            Assert-Hex64 $descriptor.virtual_start 'memory descriptor virtual_start'
            Assert-JsonInteger $descriptor.page_count 'memory descriptor page_count' 0 18446744073709551615
            Assert-Hex64 $descriptor.attributes 'memory descriptor attributes'
        }
    }
    else {
        Assert-ExactProperties $Evidence.memory_map @('status', 'phase', 'uefi_status') 'memory_map'
        Assert-Condition (Test-OrdinalEqual $Evidence.memory_map.status 'unavailable') 'Unexpected memory_map status.'
        Assert-Hex64 $Evidence.memory_map.uefi_status 'memory_map.uefi_status'
    }
    Assert-Condition (Test-OrdinalEqual $Evidence.memory_map.phase 'collection-time-not-final-exit-boot-services-map') 'Unexpected memory-map phase claim.'

    $expectedBlockers = @(
        'acpi-table-content-validation-and-cross-check',
        'amd-iommu-ivrs-register-ownership-and-pci-isolation',
        'smm-lock-and-ppr-specific-msrs',
        'inherited-memory-encryption-state',
        'mtrrs-iorrs-tom-tom2-and-mmio-apertures',
        'secure-boot-databases-option-rom-policy-and-tcg-log',
        'boot-driver-sysprep-recovery-and-hotkey-namespace',
        'ready-to-boot-after-ready-to-boot-exit-boot-services-order',
        'direct-watchdog-and-durable-attempt-lease',
        'cross-processor-cpuid-and-vm-cr-consistency'
    )
    Assert-JsonArray $Evidence.uncollected_blockers 'uncollected_blockers'
    $actualBlockers = @($Evidence.uncollected_blockers)
    Assert-Condition ($actualBlockers.Count -eq $expectedBlockers.Count) 'Unexpected uncollected-blocker count.'
    for ($index = 0; $index -lt $expectedBlockers.Count; $index++) {
        Assert-Condition (Test-OrdinalEqual $actualBlockers[$index] $expectedBlockers[$index]) 'Uncollected blockers do not match schema v1.'
    }
}

function Convert-BytesToLowerHex {
    param([Parameter(Mandatory)] [byte[]] $Bytes)
    return -join @($Bytes | ForEach-Object { $_.ToString('x2') })
}

function Get-BytesSha256 {
    param([Parameter(Mandatory)] [byte[]] $Bytes)
    $algorithm = [System.Security.Cryptography.SHA256]::Create()
    try {
        return Convert-BytesToLowerHex -Bytes $algorithm.ComputeHash($Bytes)
    }
    finally {
        $algorithm.Dispose()
    }
}

function Convert-LowerHexToBytes {
    param(
        [Parameter(Mandatory)] [AllowEmptyString()] [string] $Value,
        [Parameter(Mandatory)] [string] $Context
    )
    Assert-Condition (($Value.Length % 2) -eq 0) "$Context must contain complete bytes."
    Assert-Condition ($Value -cmatch '^(?:[0-9a-f]{2})*$') "$Context must be canonical lowercase hexadecimal."
    $bytes = New-Object byte[] ($Value.Length / 2)
    for ($index = 0; $index -lt $bytes.Length; $index++) {
        $bytes[$index] = [System.Convert]::ToByte($Value.Substring($index * 2, 2), 16)
    }
    return ,$bytes
}

function Assert-FixedLowerHex {
    param($Value, [int] $Characters, [string] $Context)
    Assert-JsonString -Value $Value -Context $Context
    Assert-Condition ($Value.Length -eq $Characters) "$Context has the wrong fixed width."
    Assert-Condition ($Value -cmatch '^[0-9a-f]+$') "$Context must be lowercase hexadecimal."
}

function Assert-ByteRange {
    param([byte[]] $Bytes, [long] $Offset, [long] $Length, [string] $Context)
    Assert-Condition ($Offset -ge 0 -and $Length -ge 0) "$Context has a negative byte range."
    Assert-Condition ($Offset -le $Bytes.LongLength) "$Context starts beyond the captured bytes."
    Assert-Condition ($Length -le ($Bytes.LongLength - $Offset)) "$Context is truncated."
}

function Get-U16Le {
    param([byte[]] $Bytes, [long] $Offset, [string] $Context)
    Assert-ByteRange $Bytes $Offset 2 $Context
    return [uint16] [System.BitConverter]::ToUInt16($Bytes, [int] $Offset)
}

function Get-U32Le {
    param([byte[]] $Bytes, [long] $Offset, [string] $Context)
    Assert-ByteRange $Bytes $Offset 4 $Context
    return [uint32] [System.BitConverter]::ToUInt32($Bytes, [int] $Offset)
}

function Get-U64Le {
    param([byte[]] $Bytes, [long] $Offset, [string] $Context)
    Assert-ByteRange $Bytes $Offset 8 $Context
    return [uint64] [System.BitConverter]::ToUInt64($Bytes, [int] $Offset)
}

function Get-ByteSlice {
    param([byte[]] $Bytes, [long] $Offset, [long] $Length, [string] $Context)
    Assert-ByteRange $Bytes $Offset $Length $Context
    $slice = New-Object byte[] $Length
    if ($Length -gt 0) {
        [System.Array]::Copy($Bytes, $Offset, $slice, 0, $Length)
    }
    return ,$slice
}

function Format-Hex8 { param([byte] $Value) return ('0x{0:x2}' -f $Value) }
function Format-Hex16 { param([uint16] $Value) return ('0x{0:x4}' -f $Value) }
function Format-Hex32 { param([uint32] $Value) return ('0x{0:x8}' -f $Value) }
function Format-Hex64 { param([uint64] $Value) return ('0x{0:x16}' -f $Value) }

function Assert-Uint64Range {
    param([uint64] $Base, [uint64] $Length, [string] $Context)
    Assert-Condition ($Base -le ([uint64]::MaxValue - $Length)) "$Context overflows the uint64 address space."
}

function Test-AcpiChecksum {
    param([byte[]] $Bytes, [long] $Offset, [long] $Length)
    Assert-ByteRange $Bytes $Offset $Length 'ACPI checksum range'
    [uint32] $sum = 0
    for ($index = $Offset; $index -lt ($Offset + $Length); $index++) {
        $sum = ($sum + $Bytes[$index]) -band 0xff
    }
    return ($sum -eq 0)
}

function Assert-RawEnvelopeV2 {
    param(
        [Parameter(Mandatory)] $Envelope,
        [Parameter(Mandatory)] [string] $Context,
        [Parameter(Mandatory)] [long] $MinimumBytes,
        [Parameter(Mandatory)] [long] $MaximumBytes
    )
    Assert-ExactProperties $Envelope @('encoding', 'length_bytes', 'sha256', 'bytes') $Context
    Assert-Condition (Test-OrdinalEqual $Envelope.encoding 'lowercase-hex') "$Context has an unsupported encoding."
    Assert-JsonInteger $Envelope.length_bytes "$Context.length_bytes" $MinimumBytes $MaximumBytes
    Assert-LowerSha256 $Envelope.sha256 "$Context.sha256"
    Assert-JsonString $Envelope.bytes "$Context.bytes"
    $expectedCharacters = [decimal] $Envelope.length_bytes * 2
    Assert-Condition ($expectedCharacters -le [int]::MaxValue) "$Context hexadecimal allocation is too large."
    Assert-Condition ($Envelope.bytes.Length -eq [int] $expectedCharacters) "$Context byte length does not match its hexadecimal payload."
    [byte[]] $bytes = Convert-LowerHexToBytes $Envelope.bytes "$Context.bytes"
    Assert-Condition ($bytes.LongLength -eq [long] $Envelope.length_bytes) "$Context decoded byte length is inconsistent."
    Assert-Condition (Test-OrdinalEqual (Get-BytesSha256 $bytes) $Envelope.sha256) "$Context SHA-256 does not match its captured bytes."
    return ,$bytes
}

function Get-SdtHeaderFacts {
    param([byte[]] $Bytes, [string] $Context)
    Assert-ByteRange $Bytes 0 36 "$Context ACPI SDT header"
    $signatureBytes = Get-ByteSlice $Bytes 0 4 "$Context signature"
    return [pscustomobject] @{
        Signature = [System.Text.Encoding]::ASCII.GetString($signatureBytes)
        Length = Get-U32Le $Bytes 4 "$Context length"
        Revision = [byte] $Bytes[8]
        Checksum = [byte] $Bytes[9]
        OemIdHex = Convert-BytesToLowerHex (Get-ByteSlice $Bytes 10 6 "$Context OEM ID")
        OemTableIdHex = Convert-BytesToLowerHex (Get-ByteSlice $Bytes 16 8 "$Context OEM table ID")
        OemRevision = Get-U32Le $Bytes 24 "$Context OEM revision"
        CreatorId = Get-U32Le $Bytes 28 "$Context creator ID"
        CreatorRevision = Get-U32Le $Bytes 32 "$Context creator revision"
    }
}

function Assert-SdtHeaderWitnessV2 {
    param($Header, $Facts, [string] $ExpectedSignature, [string] $Context)
    Assert-ExactProperties $Header @(
        'signature', 'length', 'revision', 'checksum', 'oem_id_hex',
        'oem_table_id_hex', 'oem_revision', 'creator_id', 'creator_revision'
    ) "$Context.header"
    Assert-JsonString $Header.signature "$Context.header.signature" '^.{4}$'
    Assert-JsonInteger $Header.length "$Context.header.length" 36 4294967295
    Assert-JsonInteger $Header.revision "$Context.header.revision" 0 255
    Assert-Hex8 $Header.checksum "$Context.header.checksum"
    Assert-FixedLowerHex $Header.oem_id_hex 12 "$Context.header.oem_id_hex"
    Assert-FixedLowerHex $Header.oem_table_id_hex 16 "$Context.header.oem_table_id_hex"
    foreach ($field in @('oem_revision', 'creator_id', 'creator_revision')) {
        Assert-Hex32 $Header.$field "$Context.header.$field"
    }
    Assert-Condition (Test-OrdinalEqual $Facts.Signature $ExpectedSignature) "$Context has the wrong ACPI signature."
    Assert-Condition (Test-OrdinalEqual $Header.signature $Facts.Signature) "$Context header signature witness is inconsistent."
    Assert-Condition ([decimal] $Header.length -eq [decimal] $Facts.Length) "$Context header length witness is inconsistent."
    Assert-Condition ([decimal] $Header.revision -eq [decimal] $Facts.Revision) "$Context header revision witness is inconsistent."
    Assert-Condition (Test-OrdinalEqual $Header.checksum (Format-Hex8 $Facts.Checksum)) "$Context header checksum witness is inconsistent."
    Assert-Condition (Test-OrdinalEqual $Header.oem_id_hex $Facts.OemIdHex) "$Context OEM ID witness is inconsistent."
    Assert-Condition (Test-OrdinalEqual $Header.oem_table_id_hex $Facts.OemTableIdHex) "$Context OEM table ID witness is inconsistent."
    Assert-Condition (Test-OrdinalEqual $Header.oem_revision (Format-Hex32 $Facts.OemRevision)) "$Context OEM revision witness is inconsistent."
    Assert-Condition (Test-OrdinalEqual $Header.creator_id (Format-Hex32 $Facts.CreatorId)) "$Context creator ID witness is inconsistent."
    Assert-Condition (Test-OrdinalEqual $Header.creator_revision (Format-Hex32 $Facts.CreatorRevision)) "$Context creator revision witness is inconsistent."
}

function Assert-StringArrayWitnessV2 {
    param($Actual, [string[]] $Expected, [string] $Context, [ValidateSet('hex64', 'root')] [string] $Kind = 'hex64')
    Assert-JsonArray $Actual $Context
    $values = @($Actual)
    Assert-Condition ($values.Count -eq $Expected.Count) "$Context has the wrong item count."
    for ($index = 0; $index -lt $values.Count; $index++) {
        if ($Kind -ceq 'hex64') { Assert-Hex64 $values[$index] "$Context[]" }
        else { Assert-Condition ($values[$index] -in @('rsdt', 'xsdt')) "$Context contains an invalid root name." }
        Assert-Condition (Test-OrdinalEqual $values[$index] $Expected[$index]) "$Context is inconsistent."
    }
}

function Assert-AcpiRootV2 {
    param(
        [Parameter(Mandatory)] $Root,
        [Parameter(Mandatory)] [ValidateSet('rsdt', 'xsdt')] [string] $Kind,
        [Parameter(Mandatory)] [string] $ExpectedAddress,
        [Parameter(Mandatory)] [int] $MaximumBytes,
        [Parameter(Mandatory)] [int] $MaximumEntries
    )
    $context = "acpi.roots.$Kind"
    Assert-ExactProperties $Root @('kind', 'address', 'raw', 'header', 'entry_width', 'entry_count', 'entries') $context
    Assert-Condition (Test-OrdinalEqual $Root.kind $Kind) "$context.kind is inconsistent."
    Assert-Hex64 $Root.address "$context.address"
    Assert-Condition (Test-OrdinalEqual $Root.address $ExpectedAddress) "$context.address does not match the RSDP pointer."
    [byte[]] $bytes = Assert-RawEnvelopeV2 $Root.raw "$context.raw" 36 $MaximumBytes
    $base = Convert-Hex64 $Root.address
    Assert-Uint64Range $base ([uint64] $bytes.LongLength) "$context captured range"
    $facts = Get-SdtHeaderFacts $bytes $context
    $signature = if ($Kind -ceq 'rsdt') { 'RSDT' } else { 'XSDT' }
    Assert-SdtHeaderWitnessV2 $Root.header $facts $signature $context
    Assert-Condition ($facts.Length -eq $bytes.LongLength) "$context declared length does not match captured bytes."
    Assert-Condition (Test-AcpiChecksum $bytes 0 $bytes.LongLength) "$context has an invalid ACPI checksum."
    $entryWidth = if ($Kind -ceq 'rsdt') { 4 } else { 8 }
    Assert-JsonInteger $Root.entry_width "$context.entry_width" $entryWidth $entryWidth
    $payloadLength = $bytes.LongLength - 36
    Assert-Condition (($payloadLength % $entryWidth) -eq 0) "$context payload length is not divisible by its pointer width."
    $entryCount = [long] ($payloadLength / $entryWidth)
    Assert-Condition ($entryCount -le $MaximumEntries) "$context entry count exceeds its cap."
    Assert-JsonInteger $Root.entry_count "$context.entry_count" 0 $MaximumEntries
    Assert-Condition ([decimal] $Root.entry_count -eq [decimal] $entryCount) "$context.entry_count is inconsistent."
    Assert-JsonArray $Root.entries "$context.entries"
    $entries = @($Root.entries)
    Assert-Condition ($entries.Count -eq $entryCount) "$context.entries has the wrong item count."
    $seen = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
    for ($index = 0; $index -lt $entries.Count; $index++) {
        Assert-Hex64 $entries[$index] "$context.entries[]"
        $offset = 36 + ($index * $entryWidth)
        $pointer = if ($entryWidth -eq 4) { [uint64] (Get-U32Le $bytes $offset "$context entry") } else { Get-U64Le $bytes $offset "$context entry" }
        $expected = Format-Hex64 $pointer
        Assert-Condition ($pointer -ne 0) "$context contains a null table pointer."
        Assert-Condition (Test-OrdinalEqual $entries[$index] $expected) "$context.entries[] is inconsistent with raw bytes."
        Assert-Condition ($seen.Add($expected)) "$context contains a duplicate table pointer: '$expected'."
    }
    return [pscustomobject] @{ Bytes = $bytes; Entries = [string[]] $entries }
}

function Assert-AcpiTableRecordV2 {
    param(
        [Parameter(Mandatory)] $Record,
        [Parameter(Mandatory)] [string] $Context,
        [Parameter(Mandatory)] [string] $ExpectedSignature,
        [Parameter(Mandatory)] [string] $ExpectedAddress,
        [Parameter(Mandatory)] [string[]] $ExpectedReferences,
        [Parameter(Mandatory)] $DirectoryRecord,
        [Parameter(Mandatory)] [int] $MaximumBytes
    )
    Assert-ExactProperties $Record @('address', 'referenced_by', 'raw', 'header', 'body') $Context
    Assert-Hex64 $Record.address "$Context.address"
    Assert-Condition (Test-OrdinalEqual $Record.address $ExpectedAddress) "$Context.address is inconsistent with the root directory."
    Assert-StringArrayWitnessV2 $Record.referenced_by $ExpectedReferences "$Context.referenced_by" root
    [byte[]] $bytes = Assert-RawEnvelopeV2 $Record.raw "$Context.raw" 36 $MaximumBytes
    Assert-Uint64Range (Convert-Hex64 $Record.address) ([uint64] $bytes.LongLength) "$Context captured range"
    $facts = Get-SdtHeaderFacts $bytes $Context
    Assert-SdtHeaderWitnessV2 $Record.header $facts $ExpectedSignature $Context
    Assert-Condition ($facts.Length -eq $bytes.LongLength) "$Context declared length does not match captured bytes."
    Assert-Condition (Test-AcpiChecksum $bytes 0 $bytes.LongLength) "$Context has an invalid ACPI checksum."
    Assert-Condition ($DirectoryRecord.header_raw.bytes -ceq $Record.raw.bytes.Substring(0, 72)) "$Context header bytes do not match its directory probe."
    return [pscustomobject] @{ Bytes = $bytes; Facts = $facts }
}

function Assert-MadtBodyV2 {
    param($Body, [byte[]] $Bytes, [int] $MaximumEntries, [int] $MaximumProcessors)
    $context = 'acpi.tables.madt.body'
    Assert-Condition ($Bytes.Length -ge 44) 'MADT is truncated before its fixed body.'
    Assert-ExactProperties $Body @(
        'local_apic_address', 'flags', 'entry_count', 'processor_entry_count',
        'enabled_processor_count', 'entries'
    ) $context
    Assert-Hex32 $Body.local_apic_address "$context.local_apic_address"
    Assert-Hex32 $Body.flags "$context.flags"
    Assert-Condition (Test-OrdinalEqual $Body.local_apic_address (Format-Hex32 (Get-U32Le $Bytes 36 'MADT local APIC address'))) 'MADT local APIC address witness is inconsistent.'
    Assert-Condition (Test-OrdinalEqual $Body.flags (Format-Hex32 (Get-U32Le $Bytes 40 'MADT flags'))) 'MADT flags witness is inconsistent.'
    Assert-JsonArray $Body.entries "$context.entries"
    $witnesses = @($Body.entries)
    $offset = 44
    $entryIndex = 0
    $processorCount = 0
    $enabledCount = 0
    $processorIds = New-Object System.Collections.Generic.List[string]
    $enabledIds = New-Object System.Collections.Generic.List[string]
    $seenProcessorIds = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
    $seenProcessorUids = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
    while ($offset -lt $Bytes.Length) {
        Assert-Condition ($entryIndex -lt $MaximumEntries) 'MADT entry count exceeds its cap.'
        Assert-ByteRange $Bytes $offset 2 'MADT entry header'
        $type = [byte] $Bytes[$offset]
        $length = [byte] $Bytes[$offset + 1]
        Assert-Condition ($length -ge 2) 'MADT entry length does not make forward progress.'
        Assert-ByteRange $Bytes $offset $length 'MADT entry'
        Assert-Condition ($entryIndex -lt $witnesses.Count) 'MADT entry witness array is truncated.'
        $witness = $witnesses[$entryIndex]
        $entryBytes = Get-ByteSlice $Bytes $offset $length 'MADT entry'
        $entryHash = Get-BytesSha256 $entryBytes
        if ($type -eq 0) {
            Assert-Condition ($length -eq 8) 'MADT Processor Local APIC entry has an invalid length.'
            Assert-ExactProperties $witness @(
                'kind', 'type', 'length', 'offset', 'raw_sha256', 'acpi_processor_uid',
                'apic_id', 'flags', 'enabled', 'online_capable'
            ) 'MADT Processor Local APIC entry'
            Assert-Condition (Test-OrdinalEqual $witness.kind 'processor-local-apic') 'MADT processor entry kind is inconsistent.'
            Assert-JsonInteger $witness.acpi_processor_uid 'MADT ACPI processor UID' 0 255
            Assert-Hex8 $witness.apic_id 'MADT APIC ID'
            $flags = Get-U32Le $Bytes ($offset + 4) 'MADT processor flags'
            $id = [uint64] $Bytes[$offset + 3]
            Assert-Condition ($witness.acpi_processor_uid -eq $Bytes[$offset + 2]) 'MADT ACPI processor UID witness is inconsistent.'
            Assert-Condition (Test-OrdinalEqual $witness.apic_id (Format-Hex8 ([byte] $id))) 'MADT APIC ID witness is inconsistent.'
            $normalizedId = Format-Hex64 $id
            $normalizedUid = Format-Hex64 ([uint64] $Bytes[$offset + 2])
            $processorCount++
        }
        elseif ($type -eq 9) {
            Assert-Condition ($length -eq 16) 'MADT Processor Local x2APIC entry has an invalid length.'
            Assert-ExactProperties $witness @(
                'kind', 'type', 'length', 'offset', 'raw_sha256', 'reserved',
                'x2apic_id', 'flags', 'acpi_processor_uid', 'enabled', 'online_capable'
            ) 'MADT Processor Local x2APIC entry'
            Assert-Condition (Test-OrdinalEqual $witness.kind 'processor-local-x2apic') 'MADT x2APIC entry kind is inconsistent.'
            Assert-Hex16 $witness.reserved 'MADT x2APIC reserved field'
            Assert-Hex32 $witness.x2apic_id 'MADT x2APIC ID'
            Assert-JsonInteger $witness.acpi_processor_uid 'MADT x2APIC ACPI processor UID' 0 4294967295
            $x2ApicReserved = Get-U16Le $Bytes ($offset + 2) 'MADT x2APIC reserved field'
            Assert-Condition ($x2ApicReserved -eq 0) 'MADT Local x2APIC reserved field is nonzero.'
            Assert-Condition (Test-OrdinalEqual $witness.reserved (Format-Hex16 $x2ApicReserved)) 'MADT x2APIC reserved witness is inconsistent.'
            $id32 = Get-U32Le $Bytes ($offset + 4) 'MADT x2APIC ID'
            Assert-Condition (Test-OrdinalEqual $witness.x2apic_id (Format-Hex32 $id32)) 'MADT x2APIC ID witness is inconsistent.'
            $flags = Get-U32Le $Bytes ($offset + 8) 'MADT x2APIC flags'
            $uid32 = Get-U32Le $Bytes ($offset + 12) 'MADT x2APIC UID'
            Assert-Condition ([decimal] $witness.acpi_processor_uid -eq [decimal] $uid32) 'MADT x2APIC UID witness is inconsistent.'
            $normalizedId = Format-Hex64 ([uint64] $id32)
            $normalizedUid = Format-Hex64 ([uint64] $uid32)
            $processorCount++
        }
        else {
            Assert-ExactProperties $witness @('kind', 'type', 'length', 'offset', 'raw_sha256') 'MADT generic entry'
            Assert-Condition (Test-OrdinalEqual $witness.kind 'other') 'MADT generic entry kind is inconsistent.'
            $flags = $null
            $normalizedId = $null
            $normalizedUid = $null
        }
        Assert-JsonInteger $witness.type 'MADT entry type witness' 0 255
        Assert-JsonInteger $witness.length 'MADT entry length witness' 2 255
        Assert-JsonInteger $witness.offset 'MADT entry offset witness' 44 4294967295
        Assert-LowerSha256 $witness.raw_sha256 'MADT entry SHA-256'
        Assert-Condition ($witness.type -eq $type) 'MADT entry type witness is inconsistent.'
        Assert-Condition ($witness.length -eq $length) 'MADT entry length witness is inconsistent.'
        Assert-Condition ($witness.offset -eq $offset) 'MADT entry offset witness is inconsistent.'
        Assert-Condition (Test-OrdinalEqual $witness.raw_sha256 $entryHash) 'MADT entry SHA-256 witness is inconsistent.'
        if ($null -ne $normalizedId) {
            Assert-Condition ($processorCount -le $MaximumProcessors) 'MADT processor entry count exceeds its cap.'
            Assert-Condition ($seenProcessorUids.Add($normalizedUid)) "MADT contains a duplicate ACPI processor UID: '$normalizedUid'."
            Assert-Hex32 $witness.flags 'MADT processor flags witness'
            Assert-JsonBoolean $witness.enabled 'MADT processor enabled witness'
            Assert-JsonBoolean $witness.online_capable 'MADT processor online-capable witness'
            Assert-Condition (Test-OrdinalEqual $witness.flags (Format-Hex32 $flags)) 'MADT processor flags witness is inconsistent.'
            $enabled = ($flags -band 1) -ne 0
            $online = ($flags -band 2) -ne 0
            Assert-Condition ($witness.enabled -eq $enabled) 'MADT processor enabled witness is inconsistent.'
            Assert-Condition ($witness.online_capable -eq $online) 'MADT processor online-capable witness is inconsistent.'
            if ($enabled -or $online) {
                Assert-Condition ($seenProcessorIds.Add($normalizedId)) "MADT contains a duplicate usable processor interrupt-controller ID: '$normalizedId'."
                [void] $processorIds.Add($normalizedId)
            }
            if ($enabled) { $enabledCount++; [void] $enabledIds.Add($normalizedId) }
        }
        $offset += $length
        $entryIndex++
    }
    Assert-Condition ($offset -eq $Bytes.Length) 'MADT entry parsing did not end at the table boundary.'
    Assert-Condition ($witnesses.Count -eq $entryIndex) 'MADT has extra entry witnesses.'
    Assert-JsonInteger $Body.entry_count "$context.entry_count" 0 $MaximumEntries
    Assert-JsonInteger $Body.processor_entry_count "$context.processor_entry_count" 0 $MaximumProcessors
    Assert-JsonInteger $Body.enabled_processor_count "$context.enabled_processor_count" 0 $MaximumProcessors
    Assert-Condition ($Body.entry_count -eq $entryIndex) 'MADT entry_count witness is inconsistent.'
    Assert-Condition ($Body.processor_entry_count -eq $processorCount) 'MADT processor_entry_count witness is inconsistent.'
    Assert-Condition ($Body.enabled_processor_count -eq $enabledCount) 'MADT enabled_processor_count witness is inconsistent.'
    return [pscustomobject] @{ ProcessorIds = $processorIds.ToArray(); EnabledIds = $enabledIds.ToArray() }
}

function Assert-McfgBodyV2 {
    param($Body, [byte[]] $Bytes, [int] $MaximumAllocations)
    $context = 'acpi.tables.mcfg.body'
    Assert-Condition ($Bytes.Length -ge 44) 'MCFG is truncated before its reserved body.'
    Assert-ExactProperties $Body @('reserved_hex', 'allocation_count', 'allocations') $context
    Assert-FixedLowerHex $Body.reserved_hex 16 "$context.reserved_hex"
    $reserved = Convert-BytesToLowerHex (Get-ByteSlice $Bytes 36 8 'MCFG reserved body')
    Assert-Condition (Test-OrdinalEqual $reserved '0000000000000000') 'MCFG reserved body is nonzero.'
    Assert-Condition (Test-OrdinalEqual $Body.reserved_hex $reserved) 'MCFG reserved witness is inconsistent.'
    $payloadLength = $Bytes.Length - 44
    Assert-Condition (($payloadLength % 16) -eq 0) 'MCFG allocation payload is not divisible by 16.'
    $count = [int] ($payloadLength / 16)
    Assert-Condition ($count -le $MaximumAllocations) 'MCFG allocation count exceeds its cap.'
    Assert-JsonInteger $Body.allocation_count "$context.allocation_count" 0 $MaximumAllocations
    Assert-Condition ($Body.allocation_count -eq $count) 'MCFG allocation_count witness is inconsistent.'
    Assert-JsonArray $Body.allocations "$context.allocations"
    $allocations = @($Body.allocations)
    Assert-Condition ($allocations.Count -eq $count) 'MCFG allocation witness count is inconsistent.'
    $ranges = New-Object System.Collections.Generic.List[object]
    for ($index = 0; $index -lt $count; $index++) {
        $offset = 44 + ($index * 16)
        $record = $allocations[$index]
        Assert-ExactProperties $record @(
            'offset', 'base_address', 'segment_group', 'start_bus', 'end_bus',
            'reserved_hex', 'window_end_exclusive'
        ) 'MCFG allocation'
        Assert-JsonInteger $record.offset 'MCFG allocation offset' 44 4294967295
        Assert-Hex64 $record.base_address 'MCFG allocation base address'
        Assert-JsonInteger $record.segment_group 'MCFG segment group' 0 65535
        Assert-JsonInteger $record.start_bus 'MCFG start bus' 0 255
        Assert-JsonInteger $record.end_bus 'MCFG end bus' 0 255
        Assert-FixedLowerHex $record.reserved_hex 8 'MCFG allocation reserved field'
        Assert-Hex64 $record.window_end_exclusive 'MCFG allocation end-exclusive address'
        $base = Get-U64Le $Bytes $offset 'MCFG allocation base address'
        $segment = Get-U16Le $Bytes ($offset + 8) 'MCFG segment group'
        $start = [byte] $Bytes[$offset + 10]
        $end = [byte] $Bytes[$offset + 11]
        $entryReserved = Convert-BytesToLowerHex (Get-ByteSlice $Bytes ($offset + 12) 4 'MCFG allocation reserved field')
        Assert-Condition ($entryReserved -ceq '00000000') 'MCFG allocation reserved field is nonzero.'
        Assert-Condition ($start -le $end) 'MCFG allocation start bus exceeds end bus.'
        # The PCI Firmware specification defines Base Address relative to bus
        # zero, even when Start Bus Number is nonzero.
        $windowStartOffset = [uint64] $start * [uint64] 1048576
        $windowEndOffset = ([uint64] $end + [uint64] 1) * [uint64] 1048576
        Assert-Uint64Range $base $windowEndOffset 'MCFG ECAM bus-zero-relative window'
        Assert-Condition (($base % [uint64] 1048576) -eq 0) 'MCFG allocation base address is not 1 MiB aligned.'
        $windowStart = $base + $windowStartOffset
        $windowEnd = $base + $windowEndOffset
        Assert-Condition ($windowStart -lt $windowEnd) 'MCFG allocation produced an empty ECAM window.'
        Assert-Condition ($record.offset -eq $offset) 'MCFG allocation offset witness is inconsistent.'
        Assert-Condition (Test-OrdinalEqual $record.base_address (Format-Hex64 $base)) 'MCFG base address witness is inconsistent.'
        Assert-Condition ($record.segment_group -eq $segment) 'MCFG segment witness is inconsistent.'
        Assert-Condition ($record.start_bus -eq $start -and $record.end_bus -eq $end) 'MCFG bus range witness is inconsistent.'
        Assert-Condition (Test-OrdinalEqual $record.reserved_hex $entryReserved) 'MCFG reserved witness is inconsistent.'
        Assert-Condition (Test-OrdinalEqual $record.window_end_exclusive (Format-Hex64 $windowEnd)) 'MCFG end-exclusive witness is inconsistent.'
        foreach ($prior in $ranges) {
            if ($prior.Segment -eq $segment) {
                $overlaps = -not ($end -lt $prior.Start -or $start -gt $prior.End)
                Assert-Condition (-not $overlaps) 'MCFG contains overlapping bus ranges in one PCI segment.'
            }
        }
        [void] $ranges.Add([pscustomobject] @{ Segment = $segment; Start = $start; End = $end })
    }
}

function Assert-IvrsBodyV2 {
    param($Body, [byte[]] $Bytes, [int] $Revision, [int] $MaximumBlocks, [int] $MaximumDeviceEntries)
    $context = 'acpi.tables.ivrs.body'
    Assert-Condition ($Bytes.Length -ge 48) 'IVRS is truncated before its fixed body.'
    Assert-ExactProperties $Body @('iv_info', 'reserved_hex', 'block_count', 'device_entry_count', 'blocks') $context
    Assert-Hex32 $Body.iv_info "$context.iv_info"
    Assert-FixedLowerHex $Body.reserved_hex 16 "$context.reserved_hex"
    $ivInfo = Get-U32Le $Bytes 36 'IVRS IVinfo'
    $reserved = Convert-BytesToLowerHex (Get-ByteSlice $Bytes 40 8 'IVRS reserved body')
    Assert-Condition (($ivInfo -band 0xff80001c) -eq 0) 'IVRS IVinfo contains reserved bits.'
    Assert-Condition ($reserved -ceq '0000000000000000') 'IVRS reserved body is nonzero.'
    Assert-Condition (Test-OrdinalEqual $Body.iv_info (Format-Hex32 $ivInfo)) 'IVRS IVinfo witness is inconsistent.'
    Assert-Condition (Test-OrdinalEqual $Body.reserved_hex $reserved) 'IVRS reserved witness is inconsistent.'
    Assert-JsonArray $Body.blocks "$context.blocks"
    $blocks = @($Body.blocks)
    $offset = 48
    $blockIndex = 0
    $deviceEntryCount = 0
    $seenIvhd = $false
    while ($offset -lt $Bytes.Length) {
        Assert-Condition ($blockIndex -lt $MaximumBlocks) 'IVRS block count exceeds its cap.'
        Assert-ByteRange $Bytes $offset 4 'IVRS block header'
        $type = [byte] $Bytes[$offset]
        $flags = [byte] $Bytes[$offset + 1]
        $length = Get-U16Le $Bytes ($offset + 2) 'IVRS block length'
        Assert-Condition ($length -ge 4) 'IVRS block length does not make forward progress.'
        Assert-ByteRange $Bytes $offset $length 'IVRS block'
        Assert-Condition ($blockIndex -lt $blocks.Count) 'IVRS block witness array is truncated.'
        $block = $blocks[$blockIndex]
        $blockBytes = Get-ByteSlice $Bytes $offset $length 'IVRS block'
        $blockHash = Get-BytesSha256 $blockBytes
        if ($type -in @(0x10, 0x11, 0x40)) {
            $seenIvhd = $true
            $headerLength = if ($type -eq 0x10) { 24 } else { 40 }
            Assert-Condition ($length -ge $headerLength) 'IVHD block is shorter than its type-specific header.'
            if ($type -eq 0x40) { Assert-Condition ($Revision -eq 2) 'IVHD Type 40h requires IVRS revision 02h.' }
            if ($type -in @(0x11, 0x40)) { Assert-Condition (($ivInfo -band 1) -ne 0) 'IVHD Type 11h/40h requires IVinfo.EFRSup.' }
            Assert-ExactProperties $block @(
                'kind', 'type', 'flags', 'length', 'offset', 'raw_sha256', 'header_length',
                'device_id', 'capability_offset', 'iommu_base_address', 'pci_segment_group',
                'iommu_info', 'feature_info', 'extended_feature_image',
                'extended_feature_image_2', 'device_entry_count', 'device_entries'
            ) 'IVHD block'
            Assert-Condition (Test-OrdinalEqual $block.kind 'ivhd') 'IVHD block kind witness is inconsistent.'
            Assert-JsonInteger $block.header_length 'IVHD header length witness' $headerLength $headerLength
            Assert-Hex16 $block.device_id 'IVHD IOMMU DeviceID'
            Assert-Hex16 $block.capability_offset 'IVHD capability offset'
            Assert-Hex64 $block.iommu_base_address 'IVHD IOMMU base address'
            Assert-JsonInteger $block.pci_segment_group 'IVHD PCI segment group' 0 65535
            Assert-Hex16 $block.iommu_info 'IVHD IOMMU info'
            Assert-Hex32 $block.feature_info 'IVHD feature information'
            $deviceId = Get-U16Le $Bytes ($offset + 4) 'IVHD IOMMU DeviceID'
            $capabilityOffset = Get-U16Le $Bytes ($offset + 6) 'IVHD capability offset'
            $iommuBase = Get-U64Le $Bytes ($offset + 8) 'IVHD IOMMU base address'
            $segment = Get-U16Le $Bytes ($offset + 16) 'IVHD PCI segment group'
            $iommuInfo = Get-U16Le $Bytes ($offset + 18) 'IVHD IOMMU info'
            $featureInfo = Get-U32Le $Bytes ($offset + 20) 'IVHD feature information'
            Assert-Condition ($capabilityOffset -ne 0 -and ($capabilityOffset % 4) -eq 0) 'IVHD capability offset must be nonzero and 4-byte aligned.'
            Assert-Condition (($iommuBase % [uint64] 16384) -eq 0) 'IVHD IOMMU base address is not 16 KiB aligned.'
            Assert-Uint64Range $iommuBase ([uint64] 16384) 'IVHD minimum IOMMU register range'
            Assert-Condition (($iommuInfo -band 0xe0e0) -eq 0) 'IVHD IOMMU info contains reserved bits.'
            if ($type -eq 0x10) {
                if (($ivInfo -band 1) -eq 0) {
                    Assert-Condition ($featureInfo -eq 0) 'IVHD Type 10h feature information must be zero when IVinfo.EFRSup is clear.'
                }
            }
            else {
                Assert-Condition (($flags -band 0xc0) -eq 0) 'IVHD Type 11h/40h flags contain reserved bits.'
                Assert-Condition ($segment -eq 0) 'IVHD Type 11h/40h PCI segment group must be zero.'
                $attributeReservedMask = [Convert]::ToUInt64('f0001ffe', 16)
                Assert-Condition (([uint64] $featureInfo -band $attributeReservedMask) -eq 0) 'IVHD Type 11h/40h feature information contains reserved attribute bits.'
            }
            Assert-Condition (Test-OrdinalEqual $block.device_id (Format-Hex16 $deviceId)) 'IVHD DeviceID witness is inconsistent.'
            Assert-Condition (Test-OrdinalEqual $block.capability_offset (Format-Hex16 $capabilityOffset)) 'IVHD capability-offset witness is inconsistent.'
            Assert-Condition (Test-OrdinalEqual $block.iommu_base_address (Format-Hex64 $iommuBase)) 'IVHD IOMMU-base witness is inconsistent.'
            Assert-Condition ($block.pci_segment_group -eq $segment) 'IVHD PCI-segment witness is inconsistent.'
            Assert-Condition (Test-OrdinalEqual $block.iommu_info (Format-Hex16 $iommuInfo)) 'IVHD IOMMU-info witness is inconsistent.'
            Assert-Condition (Test-OrdinalEqual $block.feature_info (Format-Hex32 $featureInfo)) 'IVHD feature-info witness is inconsistent.'
            if ($type -eq 0x10) {
                Assert-Condition ($null -eq $block.extended_feature_image) 'IVHD Type 10h extended feature image must be null.'
                Assert-Condition ($null -eq $block.extended_feature_image_2) 'IVHD Type 10h extended feature image 2 must be null.'
            }
            else {
                Assert-Hex64 $block.extended_feature_image 'IVHD extended feature image'
                Assert-Hex64 $block.extended_feature_image_2 'IVHD extended feature image 2'
                Assert-Condition (Test-OrdinalEqual $block.extended_feature_image (Format-Hex64 (Get-U64Le $Bytes ($offset + 24) 'IVHD extended feature image'))) 'IVHD extended feature image witness is inconsistent.'
                Assert-Condition (Test-OrdinalEqual $block.extended_feature_image_2 (Format-Hex64 (Get-U64Le $Bytes ($offset + 32) 'IVHD extended feature image 2'))) 'IVHD extended feature image 2 witness is inconsistent.'
            }
            Assert-JsonArray $block.device_entries 'IVHD device_entries'
            $deviceWitnesses = @($block.device_entries)
            $deviceOffset = $offset + $headerLength
            $deviceIndex = 0
            $seenVariableDeviceEntry = $false
            while ($deviceOffset -lt ($offset + $length)) {
                Assert-Condition ($deviceEntryCount -lt $MaximumDeviceEntries) 'IVRS device entry count exceeds its cap.'
                Assert-ByteRange $Bytes $deviceOffset 1 'IVHD device entry type'
                $deviceType = [byte] $Bytes[$deviceOffset]
                Assert-Condition (-not ($seenVariableDeviceEntry -and $deviceType -lt 0x80)) 'IVHD fixed-length device entries must precede variable-length entries.'
                $uidLength = $null
                if ($deviceType -in @(0x01, 0x02, 0x03, 0x04)) { $deviceLength = 4 }
                elseif ($deviceType -in @(0x42, 0x43, 0x46, 0x47, 0x48)) { $deviceLength = 8 }
                elseif ($deviceType -eq 0xf0) {
                    Assert-Condition ($type -eq 0x40) 'IVHD F0h device entries require a Type 40h IVHD block.'
                    Assert-ByteRange $Bytes $deviceOffset 22 'IVHD F0h device entry header'
                    $uidFormat = [byte] $Bytes[$deviceOffset + 20]
                    $uidLength = [byte] $Bytes[$deviceOffset + 21]
                    Assert-Condition ($uidFormat -le 2) 'IVHD F0h UID format must be in the range 0..2.'
                    Assert-Condition (($uidFormat -eq 0) -eq ($uidLength -eq 0)) 'IVHD F0h UID format/length presence contract is invalid.'
                    $deviceLength = 22 + $uidLength
                }
                else { throw ("IVHD variable device entry type 0x{0:x2} is reserved or unsupported." -f $deviceType) }
                Assert-ByteRange $Bytes $deviceOffset $deviceLength 'IVHD device entry'
                Assert-Condition (($deviceOffset + $deviceLength) -le ($offset + $length)) 'IVHD device entry overruns its block.'
                if ($deviceType -eq 0x04) {
                    Assert-Condition ($Bytes[$deviceOffset + 3] -eq 0) 'IVHD end-of-range DTE setting must be zero.'
                }
                elseif ($deviceType -in @(0x42, 0x43)) {
                    Assert-Condition ($Bytes[$deviceOffset + 4] -eq 0 -and $Bytes[$deviceOffset + 7] -eq 0) 'IVHD alias entry contains nonzero reserved bytes.'
                }
                elseif ($deviceType -in @(0x46, 0x47)) {
                    $extendedSetting = Get-U32Le $Bytes ($deviceOffset + 4) 'IVHD extended DTE setting'
                    Assert-Condition (([uint64] $extendedSetting -band [uint64] 0x7ffffff8) -eq 0) 'IVHD extended DTE setting contains reserved bits.'
                }
                elseif ($deviceType -eq 0x48) {
                    Assert-Condition ($Bytes[$deviceOffset + 1] -eq 0 -and $Bytes[$deviceOffset + 2] -eq 0) 'IVHD special-device reserved DeviceID must be zero.'
                    $variety = [byte] $Bytes[$deviceOffset + 7]
                    Assert-Condition ($variety -in @(1, 2)) 'IVHD special-device variety must be 1 or 2.'
                }
                Assert-Condition ($deviceIndex -lt $deviceWitnesses.Count) 'IVHD device-entry witness array is truncated.'
                $device = $deviceWitnesses[$deviceIndex]
                $expectedProperties = @('type', 'length', 'offset', 'raw_sha256')
                if ($deviceType -eq 0xf0) { $expectedProperties += 'uid_length' }
                Assert-ExactProperties $device $expectedProperties 'IVHD device entry'
                Assert-Hex8 $device.type 'IVHD device entry type'
                Assert-JsonInteger $device.length 'IVHD device entry length' 4 277
                Assert-JsonInteger $device.offset 'IVHD device entry offset' 0 4294967295
                Assert-LowerSha256 $device.raw_sha256 'IVHD device entry SHA-256'
                Assert-Condition (Test-OrdinalEqual $device.type (Format-Hex8 $deviceType)) 'IVHD device entry type witness is inconsistent.'
                Assert-Condition ($device.length -eq $deviceLength) 'IVHD device entry length witness is inconsistent.'
                Assert-Condition ($device.offset -eq $deviceOffset) 'IVHD device entry offset witness is inconsistent.'
                Assert-Condition (Test-OrdinalEqual $device.raw_sha256 (Get-BytesSha256 (Get-ByteSlice $Bytes $deviceOffset $deviceLength 'IVHD device entry'))) 'IVHD device entry SHA-256 witness is inconsistent.'
                if ($deviceType -eq 0xf0) {
                    Assert-JsonInteger $device.uid_length 'IVHD F0h UID length' 0 255
                    Assert-Condition ($device.uid_length -eq $uidLength) 'IVHD F0h UID-length witness is inconsistent.'
                }
                if ($deviceType -ge 0x80) { $seenVariableDeviceEntry = $true }
                $deviceOffset += $deviceLength
                $deviceIndex++
                $deviceEntryCount++
            }
            Assert-Condition ($deviceOffset -eq ($offset + $length)) 'IVHD device-entry parsing did not end at the block boundary.'
            Assert-Condition ($deviceIndex -gt 0) 'IVHD block must contain at least one device entry.'
            Assert-Condition ($deviceWitnesses.Count -eq $deviceIndex) 'IVHD block has extra device-entry witnesses.'
            Assert-JsonInteger $block.device_entry_count 'IVHD device entry count witness' 1 $MaximumDeviceEntries
            Assert-Condition ($block.device_entry_count -eq $deviceIndex) 'IVHD device_entry_count witness is inconsistent.'
        }
        elseif ($type -in @(0x20, 0x21, 0x22)) {
            Assert-Condition $seenIvhd 'IVMD block must follow an IVHD block.'
            Assert-Condition ($length -eq 32) 'IVMD block length must be exactly 32 bytes.'
            Assert-Condition (($flags -band 0xf0) -eq 0) 'IVMD flags contain reserved bits.'
            Assert-Condition (-not (($flags -band 0x08) -eq 0 -and ($flags -band 0x06) -eq 0 -and ($flags -band 1) -ne 0)) 'IVMD flags contain an invalid exclusion-range combination.'
            Assert-ExactProperties $block @(
                'kind', 'type', 'flags', 'length', 'offset', 'raw_sha256', 'device_id',
                'auxiliary_data_or_end_device_id', 'pci_segment_group',
                'reserved_or_segment_area_hex', 'start_address', 'memory_length', 'end_exclusive'
            ) 'IVMD block'
            Assert-Condition (Test-OrdinalEqual $block.kind 'ivmd') 'IVMD block kind witness is inconsistent.'
            Assert-Hex16 $block.device_id 'IVMD DeviceID'
            Assert-Hex16 $block.auxiliary_data_or_end_device_id 'IVMD auxiliary/end DeviceID'
            Assert-FixedLowerHex $block.reserved_or_segment_area_hex 16 'IVMD reserved/segment area'
            Assert-Hex64 $block.start_address 'IVMD start address'
            Assert-Hex64 $block.memory_length 'IVMD memory length'
            Assert-Hex64 $block.end_exclusive 'IVMD end-exclusive address'
            $deviceId = Get-U16Le $Bytes ($offset + 4) 'IVMD DeviceID'
            $aux = Get-U16Le $Bytes ($offset + 6) 'IVMD auxiliary/end DeviceID'
            $area = Convert-BytesToLowerHex (Get-ByteSlice $Bytes ($offset + 8) 8 'IVMD reserved/segment area')
            $segment = Get-U16Le $Bytes ($offset + 8) 'IVMD PCI segment group'
            $areaTail = Convert-BytesToLowerHex (Get-ByteSlice $Bytes ($offset + 10) 6 'IVMD reserved tail')
            if ($type -eq 0x20) {
                Assert-Condition ($deviceId -eq 0 -and $aux -eq 0 -and $area -ceq '0000000000000000') 'IVMD Type 20h reserved fields are nonzero.'
                Assert-Condition ($null -eq $block.pci_segment_group) 'IVMD Type 20h PCI segment must be null.'
            }
            else {
                if ($type -eq 0x21) { Assert-Condition ($aux -eq 0) 'IVMD Type 21h auxiliary field is nonzero.' }
                if ($type -eq 0x22) { Assert-Condition ($deviceId -le $aux) 'IVMD Type 22h DeviceID range is reversed.' }
                Assert-Condition ($areaTail -ceq '000000000000') 'IVMD PCI segment reserved tail is nonzero.'
                Assert-JsonInteger $block.pci_segment_group 'IVMD PCI segment group' 0 65535
                Assert-Condition ($block.pci_segment_group -eq $segment) 'IVMD PCI-segment witness is inconsistent.'
            }
            $start = Get-U64Le $Bytes ($offset + 16) 'IVMD start address'
            $memoryLength = Get-U64Le $Bytes ($offset + 24) 'IVMD memory length'
            Assert-Condition ($memoryLength -gt 0) 'IVMD memory length must be nonzero.'
            Assert-Uint64Range $start $memoryLength 'IVMD memory range'
            Assert-Condition (Test-OrdinalEqual $block.device_id (Format-Hex16 $deviceId)) 'IVMD DeviceID witness is inconsistent.'
            Assert-Condition (Test-OrdinalEqual $block.auxiliary_data_or_end_device_id (Format-Hex16 $aux)) 'IVMD auxiliary/end DeviceID witness is inconsistent.'
            Assert-Condition (Test-OrdinalEqual $block.reserved_or_segment_area_hex $area) 'IVMD reserved/segment witness is inconsistent.'
            Assert-Condition (Test-OrdinalEqual $block.start_address (Format-Hex64 $start)) 'IVMD start-address witness is inconsistent.'
            Assert-Condition (Test-OrdinalEqual $block.memory_length (Format-Hex64 $memoryLength)) 'IVMD memory-length witness is inconsistent.'
            Assert-Condition (Test-OrdinalEqual $block.end_exclusive (Format-Hex64 ($start + $memoryLength))) 'IVMD end-exclusive witness is inconsistent.'
        }
        else { throw ("Unsupported IVRS block type: 0x{0:x2}." -f $type) }
        foreach ($name in @('type', 'flags')) { Assert-Hex8 $block.$name "IVRS block $name" }
        Assert-JsonInteger $block.length 'IVRS block length witness' 4 65535
        Assert-JsonInteger $block.offset 'IVRS block offset witness' 48 4294967295
        Assert-LowerSha256 $block.raw_sha256 'IVRS block SHA-256'
        Assert-Condition (Test-OrdinalEqual $block.type (Format-Hex8 $type)) 'IVRS block type witness is inconsistent.'
        Assert-Condition (Test-OrdinalEqual $block.flags (Format-Hex8 $flags)) 'IVRS block flags witness is inconsistent.'
        Assert-Condition ($block.length -eq $length -and $block.offset -eq $offset) 'IVRS block length/offset witness is inconsistent.'
        Assert-Condition (Test-OrdinalEqual $block.raw_sha256 $blockHash) 'IVRS block SHA-256 witness is inconsistent.'
        $offset += $length
        $blockIndex++
    }
    Assert-Condition ($offset -eq $Bytes.Length) 'IVRS parsing did not end at the table boundary.'
    Assert-Condition $seenIvhd 'IVRS must contain at least one IVHD block.'
    Assert-Condition ($blocks.Count -eq $blockIndex) 'IVRS has extra block witnesses.'
    Assert-JsonInteger $Body.block_count "$context.block_count" 0 $MaximumBlocks
    Assert-JsonInteger $Body.device_entry_count "$context.device_entry_count" 0 $MaximumDeviceEntries
    Assert-Condition ($Body.block_count -eq $blockIndex) 'IVRS block_count witness is inconsistent.'
    Assert-Condition ($Body.device_entry_count -eq $deviceEntryCount) 'IVRS device_entry_count witness is inconsistent.'
}

function Assert-FadtBodyV2 {
    param($Body, [byte[]] $Bytes)
    $context = 'acpi.tables.fadt.body'
    Assert-Condition ($Bytes.Length -ge 148) 'FADT is too short for the required revision fields.'
    Assert-ExactProperties $Body @(
        'firmware_ctrl_32', 'dsdt_32', 'preferred_pm_profile', 'sci_interrupt',
        'iapc_boot_arch', 'flags', 'minor_version', 'x_firmware_ctrl', 'x_dsdt',
        'registers_accessed', 'pointers_followed'
    ) $context
    Assert-Hex32 $Body.firmware_ctrl_32 "$context.firmware_ctrl_32"
    Assert-Hex32 $Body.dsdt_32 "$context.dsdt_32"
    Assert-JsonInteger $Body.preferred_pm_profile "$context.preferred_pm_profile" 0 255
    Assert-JsonInteger $Body.sci_interrupt "$context.sci_interrupt" 0 65535
    Assert-Hex16 $Body.iapc_boot_arch "$context.iapc_boot_arch"
    Assert-Hex32 $Body.flags "$context.flags"
    Assert-JsonInteger $Body.minor_version "$context.minor_version" 0 255
    Assert-Hex64 $Body.x_firmware_ctrl "$context.x_firmware_ctrl"
    Assert-Hex64 $Body.x_dsdt "$context.x_dsdt"
    Assert-JsonBoolean $Body.registers_accessed "$context.registers_accessed"
    Assert-JsonBoolean $Body.pointers_followed "$context.pointers_followed"
    Assert-Condition ($Body.registers_accessed -eq $false) 'FADT-described registers must remain unaccessed.'
    Assert-Condition ($Body.pointers_followed -eq $false) 'FADT pointers must remain unfollowed in this slice.'
    Assert-Condition (Test-OrdinalEqual $Body.firmware_ctrl_32 (Format-Hex32 (Get-U32Le $Bytes 36 'FADT firmware control pointer'))) 'FADT firmware-control witness is inconsistent.'
    Assert-Condition (Test-OrdinalEqual $Body.dsdt_32 (Format-Hex32 (Get-U32Le $Bytes 40 'FADT DSDT pointer'))) 'FADT DSDT witness is inconsistent.'
    Assert-Condition ($Body.preferred_pm_profile -eq $Bytes[45]) 'FADT preferred-profile witness is inconsistent.'
    Assert-Condition ($Body.sci_interrupt -eq (Get-U16Le $Bytes 46 'FADT SCI interrupt')) 'FADT SCI witness is inconsistent.'
    Assert-Condition (Test-OrdinalEqual $Body.iapc_boot_arch (Format-Hex16 (Get-U16Le $Bytes 109 'FADT IA-PC boot architecture'))) 'FADT boot-architecture witness is inconsistent.'
    Assert-Condition (Test-OrdinalEqual $Body.flags (Format-Hex32 (Get-U32Le $Bytes 112 'FADT flags'))) 'FADT flags witness is inconsistent.'
    Assert-Condition ($Body.minor_version -eq $Bytes[131]) 'FADT minor-version witness is inconsistent.'
    Assert-Condition (Test-OrdinalEqual $Body.x_firmware_ctrl (Format-Hex64 (Get-U64Le $Bytes 132 'FADT extended firmware-control pointer'))) 'FADT extended firmware-control witness is inconsistent.'
    Assert-Condition (Test-OrdinalEqual $Body.x_dsdt (Format-Hex64 (Get-U64Le $Bytes 140 'FADT extended DSDT pointer'))) 'FADT extended DSDT witness is inconsistent.'
}

function Assert-AcpiV2 {
    param($Acpi, $Evidence)
    Assert-ExactProperties $Acpi @(
        'status', 'semantics', 'specification', 'limits', 'selection', 'rsdp',
        'roots', 'directory', 'tables', 'madt_mp_cross_check', 'ivrs_claims'
    ) 'acpi'
    Assert-Condition (Test-OrdinalEqual $Acpi.status 'observed') 'ACPI v2 evidence must be observed.'
    Assert-Condition (Test-OrdinalEqual $Acpi.semantics 'firmware-description-only-no-register-or-pci-access') 'Unexpected ACPI evidence semantics.'

    Assert-ExactProperties $Acpi.specification @('publisher', 'publication', 'revision', 'date', 'sha256') 'acpi.specification'
    $expectedSpecification = [ordered] @{
        publisher = 'AMD'
        publication = '48882'
        revision = '3.11'
        date = 'April 2026'
        sha256 = 'f7c375a15db5ed63de760356867211063d164a2ed59f2d38613daec95894ce22'
    }
    foreach ($entry in $expectedSpecification.GetEnumerator()) {
        Assert-Condition (Test-OrdinalEqual $Acpi.specification.($entry.Key) $entry.Value) "acpi.specification.$($entry.Key) is unexpected."
    }

    $expectedLimits = [ordered] @{
        max_rsdp_bytes = 4096
        max_sdt_bytes = 1048576
        max_total_acpi_bytes = 4194304
        max_configuration_tables = 64
        max_root_entries = 256
        max_unique_root_pointers = 256
        max_madt_entries = 512
        max_madt_processor_entries = 256
        max_mcfg_allocations = 256
        max_ivrs_blocks = 256
        max_ivrs_device_entries = 4096
    }
    Assert-ExactProperties $Acpi.limits ([string[]] @($expectedLimits.Keys)) 'acpi.limits'
    foreach ($entry in $expectedLimits.GetEnumerator()) {
        Assert-JsonInteger $Acpi.limits.($entry.Key) "acpi.limits.$($entry.Key)" $entry.Value $entry.Value
    }

    Assert-Condition (@($Evidence.uefi.configuration_tables).Count -le $Acpi.limits.max_configuration_tables) 'UEFI configuration-table count exceeds the ACPI cap.'
    $configurationKeys = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
    foreach ($table in @($Evidence.uefi.configuration_tables)) {
        Assert-Condition ($configurationKeys.Add("$($table.guid)|$($table.address)")) 'UEFI configuration tables contain a duplicate GUID/address pair.'
    }
    Assert-ExactProperties $Acpi.selection @('source_kind', 'source_guid', 'address', 'rule') 'acpi.selection'
    Assert-Condition (Test-OrdinalEqual $Acpi.selection.source_kind 'acpi2-rsdp') 'ACPI v2 requires the ACPI 2 RSDP source.'
    Assert-Condition (Test-OrdinalEqual $Acpi.selection.source_guid '8868e871-e4f1-11d3-bc22-0080c73c8881') 'ACPI source GUID is unexpected.'
    Assert-Hex64 $Acpi.selection.address 'acpi.selection.address'
    Assert-Condition (Test-OrdinalEqual $Acpi.selection.rule 'acpi2-preferred-over-acpi1') 'ACPI selection rule is unexpected.'
    $matchingSources = @($Evidence.uefi.configuration_tables | Where-Object {
        $_.guid -ceq $Acpi.selection.source_guid -and $_.address -ceq $Acpi.selection.address
    })
    Assert-Condition ($matchingSources.Count -eq 1) 'ACPI selection does not identify exactly one UEFI ACPI2 configuration-table entry.'

    Assert-ExactProperties $Acpi.rsdp @(
        'address', 'raw', 'signature', 'checksum', 'oem_id_hex', 'revision', 'length',
        'rsdt_address', 'xsdt_address', 'extended_checksum', 'reserved_hex'
    ) 'acpi.rsdp'
    Assert-Hex64 $Acpi.rsdp.address 'acpi.rsdp.address'
    Assert-Condition (Test-OrdinalEqual $Acpi.rsdp.address $Acpi.selection.address) 'RSDP address does not match ACPI selection.'
    [byte[]] $rsdpBytes = Assert-RawEnvelopeV2 $Acpi.rsdp.raw 'acpi.rsdp.raw' 36 $Acpi.limits.max_rsdp_bytes
    Assert-Uint64Range (Convert-Hex64 $Acpi.rsdp.address) ([uint64] $rsdpBytes.LongLength) 'RSDP captured range'
    $rsdpSignature = [System.Text.Encoding]::ASCII.GetString((Get-ByteSlice $rsdpBytes 0 8 'RSDP signature'))
    Assert-Condition ($rsdpSignature -ceq 'RSD PTR ') 'RSDP signature is invalid.'
    Assert-JsonString $Acpi.rsdp.signature 'acpi.rsdp.signature'
    Assert-Condition (Test-OrdinalEqual $Acpi.rsdp.signature $rsdpSignature) 'RSDP signature witness is inconsistent.'
    Assert-Hex8 $Acpi.rsdp.checksum 'acpi.rsdp.checksum'
    Assert-FixedLowerHex $Acpi.rsdp.oem_id_hex 12 'acpi.rsdp.oem_id_hex'
    Assert-JsonInteger $Acpi.rsdp.revision 'acpi.rsdp.revision' 2 255
    Assert-JsonInteger $Acpi.rsdp.length 'acpi.rsdp.length' 36 $Acpi.limits.max_rsdp_bytes
    Assert-Hex32 $Acpi.rsdp.rsdt_address 'acpi.rsdp.rsdt_address'
    Assert-Hex64 $Acpi.rsdp.xsdt_address 'acpi.rsdp.xsdt_address'
    Assert-Hex8 $Acpi.rsdp.extended_checksum 'acpi.rsdp.extended_checksum'
    Assert-FixedLowerHex $Acpi.rsdp.reserved_hex 6 'acpi.rsdp.reserved_hex'
    $rsdtAddress32 = Get-U32Le $rsdpBytes 16 'RSDP RSDT address'
    $rsdpLength = Get-U32Le $rsdpBytes 20 'RSDP length'
    $xsdtAddress = Get-U64Le $rsdpBytes 24 'RSDP XSDT address'
    Assert-Condition ($rsdtAddress32 -ne 0 -and $xsdtAddress -ne 0) 'ACPI v2 requires nonzero RSDT and XSDT pointers.'
    Assert-Condition ($rsdpLength -eq $rsdpBytes.Length) 'RSDP declared length does not match captured bytes.'
    Assert-Condition (Test-AcpiChecksum $rsdpBytes 0 20) 'RSDP legacy checksum is invalid.'
    Assert-Condition (Test-AcpiChecksum $rsdpBytes 0 $rsdpBytes.Length) 'RSDP extended checksum is invalid.'
    Assert-Condition (Test-OrdinalEqual $Acpi.rsdp.checksum (Format-Hex8 $rsdpBytes[8])) 'RSDP checksum witness is inconsistent.'
    Assert-Condition (Test-OrdinalEqual $Acpi.rsdp.oem_id_hex (Convert-BytesToLowerHex (Get-ByteSlice $rsdpBytes 9 6 'RSDP OEM ID'))) 'RSDP OEM ID witness is inconsistent.'
    Assert-Condition ($Acpi.rsdp.revision -eq $rsdpBytes[15]) 'RSDP revision witness is inconsistent.'
    Assert-Condition ($Acpi.rsdp.length -eq $rsdpLength) 'RSDP length witness is inconsistent.'
    Assert-Condition (Test-OrdinalEqual $Acpi.rsdp.rsdt_address (Format-Hex32 $rsdtAddress32)) 'RSDP RSDT-address witness is inconsistent.'
    Assert-Condition (Test-OrdinalEqual $Acpi.rsdp.xsdt_address (Format-Hex64 $xsdtAddress)) 'RSDP XSDT-address witness is inconsistent.'
    Assert-Condition (Test-OrdinalEqual $Acpi.rsdp.extended_checksum (Format-Hex8 $rsdpBytes[32])) 'RSDP extended-checksum witness is inconsistent.'
    Assert-Condition (Test-OrdinalEqual $Acpi.rsdp.reserved_hex (Convert-BytesToLowerHex (Get-ByteSlice $rsdpBytes 33 3 'RSDP reserved bytes'))) 'RSDP reserved witness is inconsistent.'
    Assert-Condition ($Acpi.rsdp.reserved_hex -ceq '000000') 'RSDP reserved bytes are nonzero.'

    Assert-ExactProperties $Acpi.roots @('rsdt', 'xsdt') 'acpi.roots'
    $rsdtAddress64 = Format-Hex64 ([uint64] $rsdtAddress32)
    $rsdt = Assert-AcpiRootV2 $Acpi.roots.rsdt rsdt $rsdtAddress64 $Acpi.limits.max_sdt_bytes $Acpi.limits.max_root_entries
    $xsdt = Assert-AcpiRootV2 $Acpi.roots.xsdt xsdt (Format-Hex64 $xsdtAddress) $Acpi.limits.max_sdt_bytes $Acpi.limits.max_root_entries

    $expectedDirectoryOrder = New-Object System.Collections.Generic.List[string]
    $referenceMap = [System.Collections.Generic.Dictionary[string, object]]::new([System.StringComparer]::Ordinal)
    foreach ($rootPair in @(@('rsdt', $rsdt.Entries), @('xsdt', $xsdt.Entries))) {
        $rootName = [string] $rootPair[0]
        foreach ($address in @($rootPair[1])) {
            if (-not $referenceMap.ContainsKey($address)) {
                $list = New-Object System.Collections.Generic.List[string]
                $referenceMap.Add($address, $list)
                [void] $expectedDirectoryOrder.Add($address)
            }
            [void] $referenceMap[$address].Add($rootName)
        }
    }
    Assert-Condition ($expectedDirectoryOrder.Count -le $Acpi.limits.max_unique_root_pointers) 'Unique ACPI root-pointer count exceeds its cap.'
    Assert-JsonArray $Acpi.directory 'acpi.directory'
    $directory = @($Acpi.directory)
    Assert-Condition ($directory.Count -eq $expectedDirectoryOrder.Count) 'ACPI directory count does not match unique root pointers.'
    $directoryMap = [System.Collections.Generic.Dictionary[string, object]]::new([System.StringComparer]::Ordinal)
    [long] $totalBytes = $rsdpBytes.LongLength + $rsdt.Bytes.LongLength + $xsdt.Bytes.LongLength
    for ($index = 0; $index -lt $directory.Count; $index++) {
        $record = $directory[$index]
        Assert-ExactProperties $record @('address', 'referenced_by', 'header_raw', 'signature', 'declared_length', 'revision') 'acpi.directory[]'
        Assert-Hex64 $record.address 'acpi.directory[].address'
        $expectedAddress = $expectedDirectoryOrder[$index]
        Assert-Condition (Test-OrdinalEqual $record.address $expectedAddress) 'ACPI directory order/address is inconsistent.'
        $expectedReferences = [string[]] $referenceMap[$expectedAddress].ToArray()
        Assert-StringArrayWitnessV2 $record.referenced_by $expectedReferences 'acpi.directory[].referenced_by' root
        [byte[]] $headerBytes = Assert-RawEnvelopeV2 $record.header_raw 'acpi.directory[].header_raw' 36 36
        $totalBytes += $headerBytes.LongLength
        $facts = Get-SdtHeaderFacts $headerBytes 'ACPI directory entry'
        Assert-JsonString $record.signature 'acpi.directory[].signature' '^[ -~]{4}$'
        Assert-JsonInteger $record.declared_length 'acpi.directory[].declared_length' 36 $Acpi.limits.max_sdt_bytes
        Assert-JsonInteger $record.revision 'acpi.directory[].revision' 0 255
        Assert-Condition (Test-OrdinalEqual $record.signature $facts.Signature) 'ACPI directory signature witness is inconsistent.'
        Assert-Condition ($record.declared_length -eq $facts.Length) 'ACPI directory length witness is inconsistent.'
        Assert-Condition ($record.revision -eq $facts.Revision) 'ACPI directory revision witness is inconsistent.'
        Assert-Uint64Range (Convert-Hex64 $record.address) ([uint64] $record.declared_length) 'ACPI directory table range'
        Assert-Condition (-not $directoryMap.ContainsKey($record.address)) 'ACPI directory contains a duplicate address.'
        $directoryMap.Add($record.address, $record)
    }

    Assert-ExactProperties $Acpi.tables @('madt', 'mcfg', 'ivrs', 'fadt') 'acpi.tables'
    $tableDefinitions = [ordered] @{
        madt = 'APIC'
        mcfg = 'MCFG'
        ivrs = 'IVRS'
        fadt = 'FACP'
    }
    $tableFacts = @{}
    foreach ($definition in $tableDefinitions.GetEnumerator()) {
        $matches = @($directory | Where-Object { $_.signature -ceq $definition.Value })
        Assert-Condition ($matches.Count -eq 1) "ACPI directory must contain exactly one $($definition.Value) table."
        $directoryRecord = $matches[0]
        $references = [string[]] @($directoryRecord.referenced_by)
        $context = "acpi.tables.$($definition.Key)"
        $facts = Assert-AcpiTableRecordV2 $Acpi.tables.($definition.Key) $context $definition.Value $directoryRecord.address $references $directoryRecord $Acpi.limits.max_sdt_bytes
        $totalBytes += $facts.Bytes.LongLength
        $tableFacts[$definition.Key] = $facts
    }
    Assert-Condition ($totalBytes -le $Acpi.limits.max_total_acpi_bytes) 'Cumulative ACPI capture exceeds its allocation cap.'

    $madt = Assert-MadtBodyV2 $Acpi.tables.madt.body $tableFacts.madt.Bytes $Acpi.limits.max_madt_entries $Acpi.limits.max_madt_processor_entries
    Assert-McfgBodyV2 $Acpi.tables.mcfg.body $tableFacts.mcfg.Bytes $Acpi.limits.max_mcfg_allocations
    Assert-Condition ($tableFacts.ivrs.Facts.Revision -in @(1, 2)) 'IVRS revision must be 01h or 02h.'
    Assert-IvrsBodyV2 $Acpi.tables.ivrs.body $tableFacts.ivrs.Bytes $tableFacts.ivrs.Facts.Revision $Acpi.limits.max_ivrs_blocks $Acpi.limits.max_ivrs_device_entries
    Assert-FadtBodyV2 $Acpi.tables.fadt.body $tableFacts.fadt.Bytes

    Assert-Condition (Test-OrdinalEqual $Evidence.mp_services.status 'observed') 'ACPI v2 topology cross-check requires observed MP Services records.'
    $mpAllSet = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
    $mpEnabled = New-Object System.Collections.Generic.List[string]
    foreach ($processor in @($Evidence.mp_services.processors)) {
        Assert-Condition ($mpAllSet.Add([string] $processor.processor_id)) "MP Services contains a duplicate processor ID: '$($processor.processor_id)'."
        if ($processor.enabled) { [void] $mpEnabled.Add([string] $processor.processor_id) }
    }
    $expectedMp = [string[]] @($mpEnabled.ToArray() | Sort-Object)
    $expectedMadt = [string[]] @($madt.EnabledIds | Sort-Object)
    $mpSet = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
    foreach ($id in $expectedMp) { [void] $mpSet.Add($id) }
    $madtSet = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
    foreach ($id in $expectedMadt) { [void] $madtSet.Add($id) }
    $missingFromMadt = [string[]] @($expectedMp | Where-Object { -not $madtSet.Contains($_) })
    $missingFromMp = [string[]] @($expectedMadt | Where-Object { -not $mpSet.Contains($_) })
    $cross = $Acpi.madt_mp_cross_check
    Assert-ExactProperties $cross @(
        'scope', 'mp_enabled_ids', 'madt_enabled_ids', 'missing_from_madt',
        'missing_from_mp', 'mp_enabled_count', 'madt_enabled_count', 'consistent'
    ) 'acpi.madt_mp_cross_check'
    Assert-Condition (Test-OrdinalEqual $cross.scope 'processor-hardware-id-and-enabled-membership-only') 'MADT/MP cross-check scope is unexpected.'
    Assert-StringArrayWitnessV2 $cross.mp_enabled_ids $expectedMp 'acpi.madt_mp_cross_check.mp_enabled_ids'
    Assert-StringArrayWitnessV2 $cross.madt_enabled_ids $expectedMadt 'acpi.madt_mp_cross_check.madt_enabled_ids'
    Assert-StringArrayWitnessV2 $cross.missing_from_madt $missingFromMadt 'acpi.madt_mp_cross_check.missing_from_madt'
    Assert-StringArrayWitnessV2 $cross.missing_from_mp $missingFromMp 'acpi.madt_mp_cross_check.missing_from_mp'
    Assert-JsonInteger $cross.mp_enabled_count 'acpi.madt_mp_cross_check.mp_enabled_count' 0 $Acpi.limits.max_madt_processor_entries
    Assert-JsonInteger $cross.madt_enabled_count 'acpi.madt_mp_cross_check.madt_enabled_count' 0 $Acpi.limits.max_madt_processor_entries
    Assert-JsonBoolean $cross.consistent 'acpi.madt_mp_cross_check.consistent'
    Assert-Condition ($cross.mp_enabled_count -eq $expectedMp.Count) 'MADT/MP MP count witness is inconsistent.'
    Assert-Condition ($cross.madt_enabled_count -eq $expectedMadt.Count) 'MADT/MP MADT count witness is inconsistent.'
    $consistent = ($missingFromMadt.Count -eq 0 -and $missingFromMp.Count -eq 0)
    Assert-Condition ($cross.consistent -eq $consistent) 'MADT/MP consistency witness is falsified.'

    $claims = $Acpi.ivrs_claims
    Assert-ExactProperties $claims @(
        'table_present', 'runtime_ownership_assessed', 'runtime_ownership_claim',
        'pci_isolation_assessed', 'pci_isolation_claim', 'semantics'
    ) 'acpi.ivrs_claims'
    foreach ($field in @('table_present', 'runtime_ownership_assessed', 'runtime_ownership_claim', 'pci_isolation_assessed', 'pci_isolation_claim')) {
        Assert-JsonBoolean $claims.$field "acpi.ivrs_claims.$field"
    }
    Assert-Condition ($claims.table_present -eq $true) 'IVRS table presence witness must be true.'
    foreach ($field in @('runtime_ownership_assessed', 'runtime_ownership_claim', 'pci_isolation_assessed', 'pci_isolation_claim')) {
        Assert-Condition ($claims.$field -eq $false) "acpi.ivrs_claims.$field must remain false."
    }
    Assert-Condition (Test-OrdinalEqual $claims.semantics 'firmware-description-only') 'IVRS claim semantics are unexpected.'
}

function Assert-RawEvidenceV2 {
    param(
        [Parameter(Mandatory)] $Evidence,
        [Parameter(Mandatory)] [string] $ExpectedTargetManifestSha256,
        [Parameter(Mandatory)] [string] $ExpectedRawEvidenceFilename
    )
    Assert-ExactProperties $Evidence @(
        'schema_version', 'evidence_kind', 'qualification_status', 'launch_authorized',
        'physical_candidate_flash_authorized', 'control_state_writes_authorized',
        'process_introspection_authorized', 'confidential_vm_claim',
        'amd_iommu_ownership_claim', 'pci_isolation_claim', 'collector',
        'target_profile_manifest_sha256', 'collected_at', 'sink', 'uefi', 'cpu',
        'vm_cr', 'mp_services', 'memory_map', 'acpi', 'uncollected_blockers'
    ) 'raw evidence'
    Assert-JsonInteger $Evidence.schema_version 'schema_version' 2 2
    foreach ($field in @('amd_iommu_ownership_claim', 'pci_isolation_claim')) {
        Assert-JsonBoolean $Evidence.$field $field
        Assert-Condition ($Evidence.$field -eq $false) "Raw evidence cannot set $field."
    }
    Assert-ExactProperties $Evidence.collector @('name', 'version', 'slice') 'collector'
    Assert-Condition (Test-OrdinalEqual $Evidence.collector.name 'svmvisor-m0b-probe') 'Unexpected collector name.'
    Assert-Condition (Test-OrdinalEqual $Evidence.collector.version '0.2.0') 'Unexpected v2 collector version.'
    Assert-Condition (Test-OrdinalEqual $Evidence.collector.slice 'bounded-acpi-rsdp-root-whitelist-topology') 'Unexpected v2 collector slice.'
    Assert-Condition ($Evidence.cpu.authentic_amd -is [bool] -and $Evidence.cpu.authentic_amd) 'Schema v2 is restricted to the AuthenticAMD target.'
    Assert-Condition (Test-OrdinalEqual $Evidence.cpu.vendor 'AuthenticAMD') 'Schema v2 CPU vendor must be AuthenticAMD.'

    if (Test-OrdinalEqual $Evidence.mp_services.status 'observed') {
        foreach ($processor in @($Evidence.mp_services.processors)) {
            Assert-Condition (Test-OrdinalEqual $processor.id_semantics 'uefi-mp-services-processor-id') 'Unexpected v2 MP processor ID semantics.'
        }
        $legacyProcessors = @(
            foreach ($processor in @($Evidence.mp_services.processors)) {
                [pscustomobject] [ordered] @{
                    processor_number = $processor.processor_number
                    processor_id = $processor.processor_id
                    id_semantics = 'firmware-hardware-id-not-yet-acpi-cross-checked'
                    bsp = $processor.bsp
                    enabled = $processor.enabled
                    healthy = $processor.healthy
                    location = $processor.location
                }
            }
        )
        $legacyMp = [pscustomobject] [ordered] @{
            status = $Evidence.mp_services.status
            total = $Evidence.mp_services.total
            enabled = $Evidence.mp_services.enabled
            record_count = $Evidence.mp_services.record_count
            enabled_record_count = $Evidence.mp_services.enabled_record_count
            counts_consistent = $Evidence.mp_services.counts_consistent
            processors = $legacyProcessors
        }
    }
    else { $legacyMp = $Evidence.mp_services }

    $v2Blockers = @(
        'amd-iommu-ivrs-register-ownership-and-pci-isolation',
        'smm-lock-and-ppr-specific-msrs',
        'inherited-memory-encryption-state',
        'mtrrs-iorrs-tom-tom2-and-mmio-apertures',
        'secure-boot-databases-option-rom-policy-and-tcg-log',
        'boot-driver-sysprep-recovery-and-hotkey-namespace',
        'ready-to-boot-after-ready-to-boot-exit-boot-services-order',
        'direct-watchdog-and-durable-attempt-lease',
        'cross-processor-cpuid-and-vm-cr-consistency'
    )
    Assert-JsonArray $Evidence.uncollected_blockers 'uncollected_blockers'
    Assert-Condition (@($Evidence.uncollected_blockers).Count -eq $v2Blockers.Count) 'Unexpected v2 uncollected-blocker count.'
    for ($index = 0; $index -lt $v2Blockers.Count; $index++) {
        Assert-Condition (Test-OrdinalEqual $Evidence.uncollected_blockers[$index] $v2Blockers[$index]) 'Uncollected blockers do not match schema v2.'
    }

    $legacy = [pscustomobject] [ordered] @{
        schema_version = 1
        evidence_kind = $Evidence.evidence_kind
        qualification_status = $Evidence.qualification_status
        launch_authorized = $Evidence.launch_authorized
        physical_candidate_flash_authorized = $Evidence.physical_candidate_flash_authorized
        control_state_writes_authorized = $Evidence.control_state_writes_authorized
        process_introspection_authorized = $Evidence.process_introspection_authorized
        confidential_vm_claim = $Evidence.confidential_vm_claim
        collector = [pscustomobject] [ordered] @{ name = 'svmvisor-m0b-probe'; version = '0.1.0'; slice = 'cpuid-vm-cr-uefi-mp-memory-map' }
        target_profile_manifest_sha256 = $Evidence.target_profile_manifest_sha256
        collected_at = $Evidence.collected_at
        sink = $Evidence.sink
        uefi = $Evidence.uefi
        cpu = $Evidence.cpu
        vm_cr = $Evidence.vm_cr
        mp_services = $legacyMp
        memory_map = $Evidence.memory_map
        uncollected_blockers = @('acpi-table-content-validation-and-cross-check') + $v2Blockers
    }
    Assert-RawEvidenceV1 $legacy $ExpectedTargetManifestSha256 $ExpectedRawEvidenceFilename
    Assert-AcpiV2 $Evidence.acpi $Evidence
}

function Convert-V3CpuToLegacyScope {
    param(
        [Parameter(Mandatory)] $Cpu,
        [Parameter(Mandatory)] [string] $ExpectedScope,
        [Parameter(Mandatory)] [string] $Context
    )

    Assert-ExactProperties $Cpu @(
        'observation_scope', 'vendor', 'authentic_amd', 'max_basic_leaf',
        'max_extended_leaf', 'brand', 'family_model_stepping', 'raw_leaves',
        'decoded'
    ) $Context
    Assert-Condition (Test-OrdinalEqual $Cpu.observation_scope $ExpectedScope) "$Context has an unexpected observation scope."
    return [pscustomobject] [ordered] @{
        observation_scope = 'executing-processor-presumed-bsp-not-cross-processor-validated'
        vendor = $Cpu.vendor
        authentic_amd = $Cpu.authentic_amd
        max_basic_leaf = $Cpu.max_basic_leaf
        max_extended_leaf = $Cpu.max_extended_leaf
        brand = $Cpu.brand
        family_model_stepping = $Cpu.family_model_stepping
        raw_leaves = $Cpu.raw_leaves
        decoded = $Cpu.decoded
    }
}

function Test-JsonValueEqual {
    param(
        [AllowNull()] $Left,
        [AllowNull()] $Right
    )

    if ($null -eq $Left -or $null -eq $Right) {
        return $null -eq $Left -and $null -eq $Right
    }
    if ($Left -is [System.Management.Automation.PSCustomObject] -or
        $Right -is [System.Management.Automation.PSCustomObject]) {
        if (-not ($Left -is [System.Management.Automation.PSCustomObject]) -or
            -not ($Right -is [System.Management.Automation.PSCustomObject])) {
            return $false
        }
        $leftNames = @($Left.PSObject.Properties | ForEach-Object { $_.Name })
        $rightNames = @($Right.PSObject.Properties | ForEach-Object { $_.Name })
        if ($leftNames.Count -ne $rightNames.Count) { return $false }
        $rightSet = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
        foreach ($name in $rightNames) { [void] $rightSet.Add($name) }
        foreach ($name in $leftNames) {
            if (-not $rightSet.Contains($name)) { return $false }
            if (-not (Test-JsonValueEqual $Left.$name $Right.$name)) { return $false }
        }
        return $true
    }
    if ($Left -is [System.Array] -or $Right -is [System.Array]) {
        if (-not ($Left -is [System.Array]) -or -not ($Right -is [System.Array])) { return $false }
        if ($Left.Count -ne $Right.Count) { return $false }
        for ($index = 0; $index -lt $Left.Count; $index++) {
            if (-not (Test-JsonValueEqual $Left[$index] $Right[$index])) { return $false }
        }
        return $true
    }
    if ($Left -is [string] -or $Right -is [string]) {
        return ($Left -is [string]) -and ($Right -is [string]) -and (Test-OrdinalEqual $Left $Right)
    }
    if ($Left -is [bool] -or $Right -is [bool]) {
        return ($Left -is [bool]) -and ($Right -is [bool]) -and ($Left -eq $Right)
    }
    return $Left.GetType() -eq $Right.GetType() -and $Left -eq $Right
}

function Test-V6SystemRegistersEqual {
    param(
        [Parameter(Mandatory)] $Left,
        [Parameter(Mandatory)] $Right
    )

    # Both values have already passed the exact inventory/status shape checks.
    # A not-attempted observation remains wholly exact, including its reason.
    # For observed inventories, SMM_BASE is processor-thread scoped and is the
    # sole cross-processor exclusion; every other root field is recursively
    # exact. SMM_BASE itself is still independently decoded and validated.
    if (-not (Test-OrdinalEqual $Left.status $Right.status)) { return $false }
    if (-not (Test-OrdinalEqual $Left.status 'observed')) {
        return Test-JsonValueEqual $Left $Right
    }

    $leftNames = @($Left.PSObject.Properties | ForEach-Object { $_.Name })
    $rightNames = @($Right.PSObject.Properties | ForEach-Object { $_.Name })
    if ($leftNames.Count -ne $rightNames.Count) { return $false }
    $rightSet = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
    foreach ($name in $rightNames) { [void] $rightSet.Add($name) }
    foreach ($name in $leftNames) {
        if (-not $rightSet.Contains($name)) { return $false }
        if ($name -ceq 'smm_base') { continue }
        if (-not (Test-JsonValueEqual $Left.$name $Right.$name)) { return $false }
    }
    return $true
}

function Assert-V3VmCr {
    param(
        [Parameter(Mandatory)] $VmCr,
        [Parameter(Mandatory)] $CpuFacts,
        [Parameter(Mandatory)] [string] $Context
    )

    $shouldBeObserved = (
        $CpuFacts.AuthenticAmd -and
        $null -ne $CpuFacts.Svm -and
        $CpuFacts.Svm -eq $true -and
        $CpuFacts.HasSvmLeaf
    )
    if ($shouldBeObserved) {
        Assert-ExactProperties $VmCr @('status', 'raw', 'dpd', 'r_init', 'dis_a20m', 'lock', 'svmdis') $Context
        Assert-Condition (Test-OrdinalEqual $VmCr.status 'observed') "$Context must be observed for an enumerated AMD SVM CPU."
        Assert-Hex64 $VmCr.raw "$Context.raw"
        $raw = Convert-Hex64 $VmCr.raw
        $bits = [ordered] @{ dpd = 0; r_init = 1; dis_a20m = 2; lock = 3; svmdis = 4 }
        foreach ($entry in $bits.GetEnumerator()) {
            Assert-JsonBoolean $VmCr.($entry.Key) "$Context.$($entry.Key)"
            $expected = ($raw -band ([uint64] 1 -shl $entry.Value)) -ne 0
            Assert-Condition ($VmCr.($entry.Key) -eq $expected) "$Context.$($entry.Key) is inconsistent with $Context.raw."
        }
    }
    else {
        Assert-ExactProperties $VmCr @('status', 'reason') $Context
        Assert-Condition (Test-OrdinalEqual $VmCr.status 'not-attempted') "$Context must explicitly be not-attempted."
        Assert-Condition (Test-OrdinalEqual $VmCr.reason 'cpu-is-not-authentic-amd-or-svm-is-not-enumerated') "$Context has an unexpected not-attempted reason."
    }
}

function Get-V3CpuidLeafMap {
    param([Parameter(Mandatory)] $Cpu)
    $map = [System.Collections.Generic.Dictionary[string, object]]::new([System.StringComparer]::Ordinal)
    foreach ($leaf in @($Cpu.raw_leaves)) { $map.Add([string] $leaf.leaf, $leaf) }
    return $map
}

function Test-V3CpuidMatchesReference {
    param(
        [Parameter(Mandatory)] $Cpu,
        [Parameter(Mandatory)] $ReferenceCpu
    )

    $leaves = @($Cpu.raw_leaves)
    $referenceLeaves = @($ReferenceCpu.raw_leaves)
    if ($leaves.Count -ne $referenceLeaves.Count) { return $false }
    for ($index = 0; $index -lt $leaves.Count; $index++) {
        $leaf = $leaves[$index]
        $reference = $referenceLeaves[$index]
        if (-not (Test-OrdinalEqual $leaf.leaf $reference.leaf) -or
            -not (Test-OrdinalEqual $leaf.subleaf $reference.subleaf)) {
            return $false
        }
        foreach ($register in @('eax', 'ebx', 'ecx', 'edx')) {
            [uint32] $mask = [uint32]::MaxValue
            if (Test-OrdinalEqual $leaf.leaf '0x00000001') {
                if ($register -ceq 'ebx') { $mask = [uint32] 0x00ffffff }
            }
            elseif (Test-OrdinalEqual $leaf.leaf '0x8000001e') {
                switch ($register) {
                    'eax' { $mask = [uint32] 0 }
                    'ebx' { $mask = [Convert]::ToUInt32('ffffff00', 16) }
                    'ecx' { $mask = [Convert]::ToUInt32('ffffff00', 16) }
                    'edx' { $mask = [uint32]::MaxValue }
                }
            }
            [uint32] $value = Convert-Hex32 $leaf.$register
            [uint32] $referenceValue = Convert-Hex32 $reference.$register
            if (($value -band $mask) -ne ($referenceValue -band $mask)) { return $false }
        }
    }
    return $true
}

function Assert-RawEvidenceV3 {
    param(
        [Parameter(Mandatory)] $Evidence,
        [Parameter(Mandatory)] [string] $ExpectedTargetManifestSha256,
        [Parameter(Mandatory)] [string] $ExpectedRawEvidenceFilename
    )

    Assert-ExactProperties $Evidence @(
        'schema_version', 'evidence_kind', 'qualification_status', 'launch_authorized',
        'physical_candidate_flash_authorized', 'control_state_writes_authorized',
        'process_introspection_authorized', 'confidential_vm_claim',
        'amd_iommu_ownership_claim', 'pci_isolation_claim', 'collector',
        'target_profile_manifest_sha256', 'collected_at', 'sink', 'uefi', 'cpu',
        'vm_cr', 'processor_consistency', 'mp_services', 'memory_map', 'acpi',
        'uncollected_blockers'
    ) 'raw evidence'
    Assert-JsonInteger $Evidence.schema_version 'schema_version' 3 3
    Assert-Condition (Test-OrdinalEqual $Evidence.evidence_kind 'uefi-record-only-inventory-slice') 'Unexpected v3 evidence_kind.'
    Assert-ExactProperties $Evidence.collector @('name', 'version', 'slice') 'collector'
    Assert-Condition (Test-OrdinalEqual $Evidence.collector.name 'svmvisor-m0b-probe') 'Unexpected collector name.'
    Assert-Condition (Test-OrdinalEqual $Evidence.collector.version '0.3.0') 'Unexpected v3 collector version.'
    Assert-Condition (Test-OrdinalEqual $Evidence.collector.slice 'bounded-per-processor-cpuid-vm-cr-consistency') 'Unexpected v3 collector slice.'

    Assert-Condition (Test-OrdinalEqual $Evidence.mp_services.status 'observed') 'Schema v3 requires observed MP Services evidence.'
    Assert-JsonInteger $Evidence.mp_services.total 'mp_services.total' 1 256
    Assert-JsonInteger $Evidence.mp_services.enabled 'mp_services.enabled' 1 256
    Assert-JsonBoolean $Evidence.mp_services.counts_consistent 'mp_services.counts_consistent'
    Assert-Condition ($Evidence.mp_services.counts_consistent -eq $true) 'Schema v3 requires mp_services.counts_consistent=true.'
    Assert-JsonArray $Evidence.mp_services.processors 'mp_services.processors'
    $v3MpProcessors = @($Evidence.mp_services.processors)
    Assert-Condition ($v3MpProcessors.Count -ge 1 -and $v3MpProcessors.Count -le 256) 'Schema v3 MP processor array count must be within 1..256.'

    Assert-Condition (Test-OrdinalEqual $Evidence.memory_map.status 'observed') 'Schema v3 requires observed memory-map evidence.'
    Assert-JsonArray $Evidence.memory_map.descriptors 'memory_map.descriptors'
    Assert-Condition (@($Evidence.memory_map.descriptors).Count -le 4096) 'Schema v3 observed memory-map descriptor count exceeds 4096.'

    $topCpu = Convert-V3CpuToLegacyScope $Evidence.cpu 'bsp-identified-by-uefi-mp-services-and-cross-processor-compared' 'cpu'
    $v3Blockers = @(
        'amd-iommu-ivrs-register-ownership-and-pci-isolation',
        'smm-lock-and-ppr-specific-msrs',
        'inherited-memory-encryption-state',
        'mtrrs-iorrs-tom-tom2-and-mmio-apertures',
        'secure-boot-databases-option-rom-policy-and-tcg-log',
        'boot-driver-sysprep-recovery-and-hotkey-namespace',
        'ready-to-boot-after-ready-to-boot-exit-boot-services-order',
        'direct-watchdog-and-durable-attempt-lease',
        'mp-services-ap-dispatch-pre-measurement-control-state-preservation'
    )
    Assert-JsonArray $Evidence.uncollected_blockers 'uncollected_blockers'
    Assert-Condition (@($Evidence.uncollected_blockers).Count -eq $v3Blockers.Count) 'Unexpected v3 uncollected-blocker count.'
    for ($index = 0; $index -lt $v3Blockers.Count; $index++) {
        Assert-Condition (Test-OrdinalEqual $Evidence.uncollected_blockers[$index] $v3Blockers[$index]) 'Uncollected blockers do not match schema v3.'
    }

    $v2Projection = [pscustomobject] [ordered] @{
        schema_version = 2
        evidence_kind = 'uefi-read-only-inventory-slice'
        qualification_status = $Evidence.qualification_status
        launch_authorized = $Evidence.launch_authorized
        physical_candidate_flash_authorized = $Evidence.physical_candidate_flash_authorized
        control_state_writes_authorized = $Evidence.control_state_writes_authorized
        process_introspection_authorized = $Evidence.process_introspection_authorized
        confidential_vm_claim = $Evidence.confidential_vm_claim
        amd_iommu_ownership_claim = $Evidence.amd_iommu_ownership_claim
        pci_isolation_claim = $Evidence.pci_isolation_claim
        collector = [pscustomobject] [ordered] @{ name = 'svmvisor-m0b-probe'; version = '0.2.0'; slice = 'bounded-acpi-rsdp-root-whitelist-topology' }
        target_profile_manifest_sha256 = $Evidence.target_profile_manifest_sha256
        collected_at = $Evidence.collected_at
        sink = $Evidence.sink
        uefi = $Evidence.uefi
        cpu = $topCpu
        vm_cr = $Evidence.vm_cr
        mp_services = $Evidence.mp_services
        memory_map = $Evidence.memory_map
        acpi = $Evidence.acpi
        uncollected_blockers = @(
            $v3Blockers | Where-Object {
                $_ -cne 'mp-services-ap-dispatch-pre-measurement-control-state-preservation'
            }
        ) + @('cross-processor-cpuid-and-vm-cr-consistency')
    }
    Assert-RawEvidenceV2 $v2Projection $ExpectedTargetManifestSha256 $ExpectedRawEvidenceFilename

    $consistency = $Evidence.processor_consistency
    Assert-ExactProperties $consistency @(
        'status', 'scope', 'dispatch', 'comparison_policy', 'bsp_processor_number',
        'enabled_processor_count', 'observation_count', 'all_enabled_processors_observed',
        'observations', 'identity_consistent', 'cpuid_consistent', 'vm_cr_consistent',
        'consistent'
    ) 'processor_consistency'
    Assert-Condition (Test-OrdinalEqual $consistency.status 'observed') 'processor_consistency must be observed.'
    Assert-Condition (Test-OrdinalEqual $consistency.scope 'all-enabled-healthy-processors-from-matching-pre-and-post-mp-services-enumerations') 'processor_consistency scope is unexpected.'
    Assert-ExactProperties $consistency.dispatch @(
        'method', 'timeout_microseconds_per_ap', 'callback_boot_services_table_calls',
        'callback_mp_services_who_am_i', 'callback_control_state_writes',
        'firmware_dispatch_mechanism', 'firmware_dispatch_may_use_init_sipi_or_reset',
        'pre_dispatch_vm_cr_preservation', 'mp_services_revalidated_after_dispatch'
    ) 'processor_consistency.dispatch'
    Assert-Condition (Test-OrdinalEqual $consistency.dispatch.method 'bsp-local-plus-blocking-sequential-startup-this-ap') 'processor_consistency dispatch method is unexpected.'
    Assert-JsonInteger $consistency.dispatch.timeout_microseconds_per_ap 'processor_consistency.dispatch.timeout_microseconds_per_ap' 0 0
    foreach ($field in @('callback_boot_services_table_calls', 'callback_control_state_writes')) {
        Assert-JsonBoolean $consistency.dispatch.$field "processor_consistency.dispatch.$field"
        Assert-Condition ($consistency.dispatch.$field -eq $false) "processor_consistency.dispatch.$field must remain false."
    }
    foreach ($field in @(
        'callback_mp_services_who_am_i',
        'firmware_dispatch_may_use_init_sipi_or_reset',
        'mp_services_revalidated_after_dispatch'
    )) {
        Assert-JsonBoolean $consistency.dispatch.$field "processor_consistency.dispatch.$field"
        Assert-Condition ($consistency.dispatch.$field -eq $true) "processor_consistency.dispatch.$field must be true."
    }
    Assert-Condition (Test-OrdinalEqual $consistency.dispatch.firmware_dispatch_mechanism 'opaque-uefi-mp-services') 'processor_consistency firmware dispatch mechanism is unexpected.'
    Assert-Condition (Test-OrdinalEqual $consistency.dispatch.pre_dispatch_vm_cr_preservation 'not-proven') 'processor_consistency pre-dispatch VM_CR preservation claim is unexpected.'
    Assert-ExactProperties $consistency.comparison_policy @(
        'reference', 'raw_cpuid_compared', 'leaf_00000001_ebx_compared_mask',
        'leaf_8000001e_eax_compared_mask', 'leaf_8000001e_ebx_compared_mask',
        'leaf_8000001e_ecx_compared_mask', 'leaf_8000001e_edx_compared_mask',
        'all_other_collected_register_masks', 'vm_cr_comparison', 'identity_comparison'
    ) 'processor_consistency.comparison_policy'
    $policy = $consistency.comparison_policy
    Assert-Condition (Test-OrdinalEqual $policy.reference 'uefi-mp-services-bsp') 'processor_consistency reference policy is unexpected.'
    Assert-JsonBoolean $policy.raw_cpuid_compared 'processor_consistency.comparison_policy.raw_cpuid_compared'
    Assert-Condition ($policy.raw_cpuid_compared -eq $true) 'processor_consistency must compare raw CPUID.'
    $policyConstants = [ordered] @{
        leaf_00000001_ebx_compared_mask = '0x00ffffff'
        leaf_8000001e_eax_compared_mask = '0x00000000'
        leaf_8000001e_ebx_compared_mask = '0xffffff00'
        leaf_8000001e_ecx_compared_mask = '0xffffff00'
        leaf_8000001e_edx_compared_mask = '0xffffffff'
        all_other_collected_register_masks = '0xffffffff'
        vm_cr_comparison = 'exact-raw-and-observation-status'
        identity_comparison = 'cpuid-apic-low8-matches-pi-processor-id-low8-with-zero-reserved-bits'
    }
    foreach ($entry in $policyConstants.GetEnumerator()) {
        Assert-Condition (Test-OrdinalEqual $policy.($entry.Key) $entry.Value) "processor_consistency comparison policy '$($entry.Key)' is unexpected."
    }

    Assert-JsonInteger $consistency.bsp_processor_number 'processor_consistency.bsp_processor_number' 0 ([decimal]::MaxValue)
    Assert-JsonInteger $consistency.enabled_processor_count 'processor_consistency.enabled_processor_count' 1 256
    Assert-JsonInteger $consistency.observation_count 'processor_consistency.observation_count' 1 256
    Assert-JsonBoolean $consistency.all_enabled_processors_observed 'processor_consistency.all_enabled_processors_observed'
    Assert-Condition ($consistency.all_enabled_processors_observed -eq $true) 'processor_consistency must observe all enabled processors.'
    Assert-JsonArray $consistency.observations 'processor_consistency.observations'
    $observations = @($consistency.observations)
    Assert-Condition ($observations.Count -le 256) 'processor_consistency observations exceed their cap.'

    $processors = @($Evidence.mp_services.processors)
    $bspRecords = @($processors | Where-Object { $_.bsp })
    Assert-Condition ($bspRecords.Count -eq 1) 'MP Services must identify exactly one BSP for processor consistency.'
    $bspRecord = $bspRecords[0]
    Assert-Condition ($bspRecord.enabled -and $bspRecord.healthy) 'The MP Services BSP must be enabled and healthy.'
    Assert-Condition ($consistency.bsp_processor_number -eq $bspRecord.processor_number) 'processor_consistency BSP processor number does not match MP Services.'
    $enabledRecords = @($processors | Where-Object { $_.enabled })
    $eligibleRecords = @($processors | Where-Object { $_.enabled -and $_.healthy })
    Assert-Condition ($eligibleRecords.Count -eq $enabledRecords.Count) 'Every enabled MP Services processor must be healthy for this slice.'
    Assert-Condition ($consistency.enabled_processor_count -eq $eligibleRecords.Count) 'processor_consistency enabled_processor_count does not match MP Services.'
    Assert-Condition ($consistency.observation_count -eq $observations.Count) 'processor_consistency observation_count does not match observations.'
    Assert-Condition ($observations.Count -eq $eligibleRecords.Count) 'processor_consistency is missing an enabled healthy processor observation.'

    $observationCpus = New-Object System.Collections.Generic.List[object]
    $cpuFacts = New-Object System.Collections.Generic.List[object]
    for ($index = 0; $index -lt $observations.Count; $index++) {
        $observation = $observations[$index]
        $processor = $eligibleRecords[$index]
        Assert-ExactProperties $observation @(
            'processor_number', 'processor_id', 'bsp', 'dispatch', 'who_am_i', 'cpu', 'vm_cr',
            'leaf_00000001_initial_apic_id', 'leaf_8000001e_extended_apic_id',
            'identity_matches_mp', 'cpuid_matches_bsp', 'vm_cr_matches_bsp'
        ) 'processor_consistency.observations[]'
        Assert-JsonInteger $observation.processor_number 'processor_consistency.observations[].processor_number' 0 ([decimal]::MaxValue)
        Assert-Hex64 $observation.processor_id 'processor_consistency.observations[].processor_id'
        Assert-JsonBoolean $observation.bsp 'processor_consistency.observations[].bsp'
        Assert-Condition (
            $observation.processor_number -eq $processor.processor_number -and
            (Test-OrdinalEqual $observation.processor_id $processor.processor_id) -and
            $observation.bsp -eq $processor.bsp
        ) 'processor_consistency observation order or MP Services mapping is inconsistent.'
        $expectedDispatch = if ($processor.bsp) { 'bsp-direct' } else { 'startup-this-ap-success' }
        Assert-Condition (Test-OrdinalEqual $observation.dispatch $expectedDispatch) 'processor_consistency per-processor dispatch witness is inconsistent.'
        Assert-ExactProperties $observation.who_am_i @('status', 'processor_number') 'processor_consistency.observations[].who_am_i'
        Assert-Condition (Test-OrdinalEqual $observation.who_am_i.status 'success') 'processor_consistency WhoAmI status must be success.'
        Assert-JsonInteger $observation.who_am_i.processor_number 'processor_consistency.observations[].who_am_i.processor_number' 0 ([decimal]::MaxValue)
        Assert-Condition ($observation.who_am_i.processor_number -eq $observation.processor_number) 'processor_consistency WhoAmI processor number does not match the dispatched processor number.'

        $legacyCpu = Convert-V3CpuToLegacyScope $observation.cpu 'processor-selected-by-uefi-mp-services' 'processor_consistency.observations[].cpu'
        $facts = Assert-RawCpuid $legacyCpu
        Assert-V3VmCr $observation.vm_cr $facts 'processor_consistency.observations[].vm_cr'
        [void] $observationCpus.Add($legacyCpu)
        [void] $cpuFacts.Add($facts)
    }

    $bspObservationIndexes = @(
        for ($index = 0; $index -lt $observations.Count; $index++) {
            if ($observations[$index].bsp) { $index }
        }
    )
    Assert-Condition ($bspObservationIndexes.Count -eq 1) 'processor_consistency observations must contain exactly one BSP.'
    $bspIndex = $bspObservationIndexes[0]
    $bspObservation = $observations[$bspIndex]
    $bspCpu = $observationCpus[$bspIndex]
    Assert-Condition (Test-JsonValueEqual $topCpu $bspCpu) 'Top-level CPU does not exactly match the MP Services BSP observation.'
    Assert-Condition (Test-JsonValueEqual $Evidence.vm_cr $bspObservation.vm_cr) 'Top-level VM_CR does not exactly match the MP Services BSP observation.'

    $identityAll = $true
    $cpuidAll = $true
    $vmCrAll = $true
    for ($index = 0; $index -lt $observations.Count; $index++) {
        $observation = $observations[$index]
        $processorId = Convert-Hex64 $observation.processor_id
        $leafMap = Get-V3CpuidLeafMap $observationCpus[$index]
        Assert-Condition ($leafMap.ContainsKey('0x00000001')) 'processor_consistency requires CPUID leaf 0x00000001 on every processor.'
        Assert-Condition ($leafMap.ContainsKey('0x80000001')) 'processor_consistency requires CPUID leaf 0x80000001 on every processor.'
        Assert-Condition ($leafMap.ContainsKey('0x8000001e')) 'processor_consistency requires CPUID leaf 0x8000001e on every processor.'
        $topologyExtensions = ((Convert-Hex32 $leafMap['0x80000001'].ecx) -band (1 -shl 22)) -ne 0
        Assert-Condition $topologyExtensions 'processor_consistency requires the CPUID topology-extensions bit on every processor.'
        [byte] $initialApicId = (Convert-Hex32 $leafMap['0x00000001'].ebx) -shr 24
        [uint32] $extendedApicId = Convert-Hex32 $leafMap['0x8000001e'].eax
        Assert-Hex8 $observation.leaf_00000001_initial_apic_id 'processor_consistency.observations[].leaf_00000001_initial_apic_id'
        Assert-Hex32 $observation.leaf_8000001e_extended_apic_id 'processor_consistency.observations[].leaf_8000001e_extended_apic_id'
        Assert-Condition (Test-OrdinalEqual $observation.leaf_00000001_initial_apic_id ('0x{0:x2}' -f $initialApicId)) 'CPUID initial APIC ID witness is inconsistent with raw leaf 0x00000001.'
        Assert-Condition (Test-OrdinalEqual $observation.leaf_8000001e_extended_apic_id ('0x{0:x8}' -f $extendedApicId)) 'CPUID extended APIC ID witness is inconsistent with raw leaf 0x8000001e.'
        [uint64] $processorIdLow8 = $processorId -band [uint64] 0xff
        $identityMatches = (
            $processorId -le [uint64] 0xff -and
            [uint64] $initialApicId -eq $processorIdLow8 -and
            $topologyExtensions -and
            [uint64] ($extendedApicId -band [uint32] 0xff) -eq $processorIdLow8
        )
        $cpuidMatches = Test-V3CpuidMatchesReference $observationCpus[$index] $bspCpu
        $vmCrMatches = Test-JsonValueEqual $observation.vm_cr $bspObservation.vm_cr
        foreach ($field in @('identity_matches_mp', 'cpuid_matches_bsp', 'vm_cr_matches_bsp')) {
            Assert-JsonBoolean $observation.$field "processor_consistency.observations[].$field"
        }
        Assert-Condition ($observation.identity_matches_mp -eq $identityMatches) 'processor_consistency identity_matches_mp witness is falsified.'
        Assert-Condition ($observation.cpuid_matches_bsp -eq $cpuidMatches) 'processor_consistency cpuid_matches_bsp witness is falsified.'
        Assert-Condition ($observation.vm_cr_matches_bsp -eq $vmCrMatches) 'processor_consistency vm_cr_matches_bsp witness is falsified.'
        if (-not $identityMatches) { $identityAll = $false }
        if (-not $cpuidMatches) { $cpuidAll = $false }
        if (-not $vmCrMatches) { $vmCrAll = $false }
    }

    foreach ($field in @('identity_consistent', 'cpuid_consistent', 'vm_cr_consistent', 'consistent')) {
        Assert-JsonBoolean $consistency.$field "processor_consistency.$field"
    }
    Assert-Condition ($consistency.identity_consistent -eq $identityAll) 'processor_consistency identity_consistent aggregate is falsified.'
    Assert-Condition ($consistency.cpuid_consistent -eq $cpuidAll) 'processor_consistency cpuid_consistent aggregate is falsified.'
    Assert-Condition ($consistency.vm_cr_consistent -eq $vmCrAll) 'processor_consistency vm_cr_consistent aggregate is falsified.'
    Assert-Condition ($consistency.consistent -eq ($identityAll -and $cpuidAll -and $vmCrAll)) 'processor_consistency consistent aggregate is falsified.'
}

function Get-V4IommuLocatorFacts {
    param([Parameter(Mandatory)] $Evidence)

    [byte[]] $ivrs = Assert-RawEnvelopeV2 `
        $Evidence.acpi.tables.ivrs.raw `
        'amd_iommu_live locator IVRS source' `
        48 `
        $Evidence.acpi.limits.max_sdt_bytes
    $sources = New-Object System.Collections.Generic.List[object]
    $offset = 48
    $blockIndex = 0
    while ($offset -lt $ivrs.Length) {
        Assert-ByteRange $ivrs $offset 4 'amd_iommu_live IVRS block header'
        [byte] $entryType = $ivrs[$offset]
        [uint16] $length = Get-U16Le $ivrs ($offset + 2) 'amd_iommu_live IVRS block length'
        Assert-Condition ($length -ge 4) 'amd_iommu_live IVRS block does not make forward progress.'
        Assert-ByteRange $ivrs $offset $length 'amd_iommu_live IVRS block'
        if ($entryType -in @(0x10, 0x11, 0x40)) {
            $headerLength = if ($entryType -eq 0x10) { 24 } else { 40 }
            Assert-Condition ($length -ge $headerLength) 'amd_iommu_live IVHD header is truncated.'
            [uint16] $deviceId = Get-U16Le $ivrs ($offset + 4) 'amd_iommu_live IVHD DeviceID'
            [uint16] $capabilityOffset = Get-U16Le $ivrs ($offset + 6) 'amd_iommu_live IVHD capability offset'
            [uint64] $mmioBase = Get-U64Le $ivrs ($offset + 8) 'amd_iommu_live IVHD MMIO base'
            [uint16] $segment = Get-U16Le $ivrs ($offset + 16) 'amd_iommu_live IVHD segment'
            Assert-Condition (
                $capabilityOffset -ge 0x40 -and
                $capabilityOffset -le 0xe8 -and
                ($capabilityOffset % 4) -eq 0
            ) 'amd_iommu_live IVHD capability offset is outside the bounded conventional capability range.'
            Assert-Condition (($mmioBase % [uint64] 16384) -eq 0) 'amd_iommu_live IVHD MMIO base is not 16-KiB aligned.'
            [void] $sources.Add([pscustomobject] [ordered] @{
                BlockIndex = $blockIndex
                EntryType = $entryType
                Segment = $segment
                DeviceId = $deviceId
                CapabilityOffset = $capabilityOffset
                MmioBase = $mmioBase
                Efr = if ($entryType -eq 0x10) { $null } else { Get-U64Le $ivrs ($offset + 24) 'amd_iommu_live IVHD EFR image' }
                Efr2 = if ($entryType -eq 0x10) { $null } else { Get-U64Le $ivrs ($offset + 32) 'amd_iommu_live IVHD EFR2 image' }
            })
        }
        $offset += $length
        $blockIndex++
    }
    Assert-Condition ($sources.Count -gt 0) 'amd_iommu_live requires at least one IVHD source.'

    $first = $sources[0]
    $seenTypes = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
    [Nullable[uint64]] $expectedEfr = $null
    [Nullable[uint64]] $expectedEfr2 = $null
    foreach ($source in $sources) {
        Assert-Condition (
            $source.Segment -eq $first.Segment -and
            $source.DeviceId -eq $first.DeviceId
        ) 'amd_iommu_live requires exactly one unique IVHD IOMMU unit.'
        Assert-Condition ($source.CapabilityOffset -eq $first.CapabilityOffset) 'amd_iommu_live IVHD capability offsets conflict.'
        Assert-Condition ($source.MmioBase -eq $first.MmioBase) 'amd_iommu_live IVHD MMIO bases conflict.'
        Assert-Condition ($seenTypes.Add((Format-Hex8 $source.EntryType))) 'amd_iommu_live contains a duplicate IVHD source type.'
        if ($source.EntryType -ne 0x10) {
            if ($null -eq $expectedEfr) {
                $expectedEfr = [uint64] $source.Efr
                $expectedEfr2 = [uint64] $source.Efr2
            }
            else {
                Assert-Condition (
                    [uint64] $expectedEfr -eq [uint64] $source.Efr -and
                    [uint64] $expectedEfr2 -eq [uint64] $source.Efr2
                ) 'amd_iommu_live IVHD extended-feature images conflict.'
            }
        }
    }

    [byte] $bus = $first.DeviceId -shr 8
    [byte] $device = ($first.DeviceId -shr 3) -band 0x1f
    [byte] $function = $first.DeviceId -band 0x07

    [byte[]] $mcfg = Assert-RawEnvelopeV2 `
        $Evidence.acpi.tables.mcfg.raw `
        'amd_iommu_live locator MCFG source' `
        44 `
        $Evidence.acpi.limits.max_sdt_bytes
    $matching = New-Object System.Collections.Generic.List[object]
    $allocationIndex = 0
    for ($mcfgOffset = 44; $mcfgOffset -lt $mcfg.Length; $mcfgOffset += 16) {
        [uint64] $base = Get-U64Le $mcfg $mcfgOffset 'amd_iommu_live MCFG base'
        [uint16] $candidateSegment = Get-U16Le $mcfg ($mcfgOffset + 8) 'amd_iommu_live MCFG segment'
        [byte] $startBus = $mcfg[$mcfgOffset + 10]
        [byte] $endBus = $mcfg[$mcfgOffset + 11]
        if ($candidateSegment -eq $first.Segment -and $startBus -le $bus -and $bus -le $endBus) {
            Assert-Uint64Range $base (([uint64] $endBus + 1) * [uint64] 1048576) 'amd_iommu_live MCFG range'
            [uint64] $usableStart = $base + ([uint64] $startBus * [uint64] 1048576)
            [uint64] $usableEnd = $base + (([uint64] $endBus + 1) * [uint64] 1048576)
            [uint64] $functionAddress = $base + ([uint64] $bus * [uint64] 1048576) + ([uint64] $device * [uint64] 32768) + ([uint64] $function * [uint64] 4096)
            [uint64] $capabilityAddress = $functionAddress + [uint64] $first.CapabilityOffset
            Assert-Condition ($functionAddress -ge $usableStart -and ($functionAddress + 4096) -le $usableEnd) 'amd_iommu_live ECAM function lies outside its MCFG allocation.'
            [void] $matching.Add([pscustomobject] [ordered] @{
                AllocationIndex = $allocationIndex
                Base = $base
                Segment = $candidateSegment
                StartBus = $startBus
                EndBus = $endBus
                UsableStart = $usableStart
                UsableEnd = $usableEnd
                FunctionAddress = $functionAddress
                CapabilityAddress = $capabilityAddress
            })
        }
        $allocationIndex++
    }
    Assert-Condition ($matching.Count -eq 1) 'amd_iommu_live requires exactly one covering MCFG allocation.'

    return [pscustomobject] [ordered] @{
        Sources = $sources.ToArray()
        Segment = [uint16] $first.Segment
        DeviceId = [uint16] $first.DeviceId
        CapabilityOffset = [uint16] $first.CapabilityOffset
        MmioBase = [uint64] $first.MmioBase
        Bus = $bus
        Device = $device
        Function = $function
        Type10Present = $seenTypes.Contains('0x10')
        Type11Present = $seenTypes.Contains('0x11')
        Type40Present = $seenTypes.Contains('0x40')
        ExpectedEfr = $expectedEfr
        ExpectedEfr2 = $expectedEfr2
        Mcfg = $matching[0]
    }
}

function Assert-V4MemoryBinding {
    param(
        [Parameter(Mandatory)] $Binding,
        [Parameter(Mandatory)] $Evidence,
        [Parameter(Mandatory)] [uint64] $ExpectedStart,
        [Parameter(Mandatory)] [uint64] $ExpectedEnd,
        [Parameter(Mandatory)] [string] $Context
    )

    Assert-ExactProperties $Binding @(
        'descriptor_index', 'descriptor', 'requested_start', 'requested_end_exclusive'
    ) $Context
    Assert-JsonInteger $Binding.descriptor_index "$Context.descriptor_index" 0 4095
    Assert-Hex64 $Binding.requested_start "$Context.requested_start"
    Assert-Hex64 $Binding.requested_end_exclusive "$Context.requested_end_exclusive"
    Assert-Condition (Test-OrdinalEqual $Binding.requested_start (Format-Hex64 $ExpectedStart)) "$Context requested-start witness is inconsistent."
    Assert-Condition (Test-OrdinalEqual $Binding.requested_end_exclusive (Format-Hex64 $ExpectedEnd)) "$Context requested-end witness is inconsistent."
    Assert-Condition ($ExpectedStart -lt $ExpectedEnd) "$Context requested range is empty."

    $physicalBits = [byte] $Evidence.cpu.decoded.physical_address_bits.value
    if ($physicalBits -lt 64) {
        [uint64] $physicalLimit = [uint64] 1 -shl $physicalBits
        Assert-Condition ($ExpectedEnd -le $physicalLimit) "$Context exceeds the CPUID physical-address width."
    }

    $covering = New-Object System.Collections.Generic.List[int]
    $descriptors = @($Evidence.memory_map.descriptors)
    for ($index = 0; $index -lt $descriptors.Count; $index++) {
        $candidate = $descriptors[$index]
        [uint64] $start = Convert-Hex64 $candidate.physical_start
        [uint64] $pages = [uint64] $candidate.page_count
        Assert-Condition ($pages -gt 0) 'amd_iommu_live cannot use a zero-page memory descriptor.'
        Assert-Condition ($pages -le ([uint64]::MaxValue / [uint64] 4096)) 'amd_iommu_live memory descriptor length overflows.'
        [uint64] $length = $pages * [uint64] 4096
        Assert-Uint64Range $start $length 'amd_iommu_live memory descriptor range'
        [uint64] $end = $start + $length
        if ($start -le $ExpectedStart -and $ExpectedEnd -le $end) {
            [void] $covering.Add($index)
        }
    }
    Assert-Condition ($covering.Count -eq 1) "$Context must be covered by exactly one memory descriptor."
    Assert-Condition ($Binding.descriptor_index -eq $covering[0]) "$Context descriptor-index witness is inconsistent."
    $selected = $descriptors[$covering[0]]
    Assert-ExactProperties $Binding.descriptor @(
        'memory_type', 'physical_start', 'virtual_start', 'page_count', 'attributes'
    ) "$Context.descriptor"
    foreach ($field in @('physical_start', 'virtual_start', 'attributes')) {
        Assert-Condition (Test-OrdinalEqual $Binding.descriptor.$field $selected.$field) "$Context descriptor $field witness is inconsistent."
    }
    Assert-JsonInteger $Binding.descriptor.page_count "$Context.descriptor.page_count" 1 ([decimal]::MaxValue)
    Assert-Condition ($Binding.descriptor.page_count -eq $selected.page_count) "$Context descriptor page-count witness is inconsistent."
    Assert-JsonInteger $Binding.descriptor.memory_type "$Context.descriptor.memory_type" 11 11
    Assert-Condition ($Binding.descriptor.memory_type -eq (Convert-Hex32 $selected.type)) "$Context memory-type witness is inconsistent."
    [uint64] $attributes = Convert-Hex64 $Binding.descriptor.attributes
    Assert-Condition (($attributes -band [uint64] 1) -ne 0) "$Context does not advertise EFI_MEMORY_UC."
    Assert-Condition (($attributes -band ([uint64] 1 -shl 13)) -eq 0) "$Context is EFI_MEMORY_RP read-protected."
}

function Assert-V4Locator {
    param(
        [Parameter(Mandatory)] $Locator,
        [Parameter(Mandatory)] $Evidence
    )

    $facts = Get-V4IommuLocatorFacts $Evidence
    Assert-ExactProperties $Locator @(
        'derivation', 'unique_unit_count', 'ivhd_sources', 'unit', 'mcfg',
        'ecam_memory_binding'
    ) 'amd_iommu_live.locator'
    Assert-Condition (Test-OrdinalEqual $Locator.derivation 'same-run-validated-ivrs-mcfg-memory-map') 'amd_iommu_live locator derivation is unexpected.'
    Assert-JsonInteger $Locator.unique_unit_count 'amd_iommu_live.locator.unique_unit_count' 1 1
    Assert-JsonArray $Locator.ivhd_sources 'amd_iommu_live.locator.ivhd_sources'
    $sourceWitnesses = @($Locator.ivhd_sources)
    Assert-Condition ($sourceWitnesses.Count -eq $facts.Sources.Count) 'amd_iommu_live IVHD source count is inconsistent.'
    for ($index = 0; $index -lt $facts.Sources.Count; $index++) {
        $source = $sourceWitnesses[$index]
        $expected = $facts.Sources[$index]
        Assert-ExactProperties $source @(
            'block_index', 'entry_type', 'segment_group', 'device_id',
            'capability_offset', 'iommu_base_address', 'extended_feature_image',
            'extended_feature_image_2'
        ) 'amd_iommu_live.locator.ivhd_sources[]'
        Assert-JsonInteger $source.block_index 'amd_iommu_live IVHD block index' 0 255
        Assert-Condition ($source.block_index -eq $expected.BlockIndex) 'amd_iommu_live IVHD block-index witness is inconsistent.'
        Assert-Condition (Test-OrdinalEqual $source.entry_type (Format-Hex8 $expected.EntryType)) 'amd_iommu_live IVHD type witness is inconsistent.'
        Assert-Hex16 $source.segment_group 'amd_iommu_live IVHD segment'
        Assert-Condition (Test-OrdinalEqual $source.segment_group (Format-Hex16 $expected.Segment)) 'amd_iommu_live IVHD segment witness is inconsistent.'
        Assert-Condition (Test-OrdinalEqual $source.device_id (Format-Hex16 $expected.DeviceId)) 'amd_iommu_live IVHD DeviceID witness is inconsistent.'
        Assert-Condition (Test-OrdinalEqual $source.capability_offset (Format-Hex16 $expected.CapabilityOffset)) 'amd_iommu_live IVHD capability-offset witness is inconsistent.'
        Assert-Condition (Test-OrdinalEqual $source.iommu_base_address (Format-Hex64 $expected.MmioBase)) 'amd_iommu_live IVHD MMIO-base witness is inconsistent.'
        if ($expected.EntryType -eq 0x10) {
            Assert-Condition ($null -eq $source.extended_feature_image -and $null -eq $source.extended_feature_image_2) 'amd_iommu_live Type 10h source must have null EFR images.'
        }
        else {
            Assert-Condition (Test-OrdinalEqual $source.extended_feature_image (Format-Hex64 $expected.Efr)) 'amd_iommu_live IVHD EFR witness is inconsistent.'
            Assert-Condition (Test-OrdinalEqual $source.extended_feature_image_2 (Format-Hex64 $expected.Efr2)) 'amd_iommu_live IVHD EFR2 witness is inconsistent.'
        }
    }

    Assert-ExactProperties $Locator.unit @(
        'segment_group', 'device_id', 'bdf', 'capability_offset',
        'iommu_base_address', 'ivhd_type_10_present', 'ivhd_type_11_present',
        'ivhd_type_40_present', 'expected_extended_feature_image',
        'expected_extended_feature_image_2'
    ) 'amd_iommu_live.locator.unit'
    $unit = $Locator.unit
    Assert-Hex16 $unit.segment_group 'amd_iommu_live.locator.unit.segment_group'
    Assert-Condition (Test-OrdinalEqual $unit.segment_group (Format-Hex16 $facts.Segment)) 'amd_iommu_live unit segment witness is inconsistent.'
    Assert-Condition (Test-OrdinalEqual $unit.device_id (Format-Hex16 $facts.DeviceId)) 'amd_iommu_live unit DeviceID witness is inconsistent.'
    Assert-ExactProperties $unit.bdf @('bus', 'device', 'function', 'formatted') 'amd_iommu_live.locator.unit.bdf'
    foreach ($field in @('bus', 'device', 'function')) { Assert-Hex8 $unit.bdf.$field "amd_iommu_live.locator.unit.bdf.$field" }
    Assert-Condition (
        (Test-OrdinalEqual $unit.bdf.bus (Format-Hex8 $facts.Bus)) -and
        (Test-OrdinalEqual $unit.bdf.device (Format-Hex8 $facts.Device)) -and
        (Test-OrdinalEqual $unit.bdf.function (Format-Hex8 $facts.Function))
    ) 'amd_iommu_live BDF decoding is inconsistent.'
    $formattedBdf = '{0:x2}:{1:x2}.{2:x1}' -f $facts.Bus, $facts.Device, $facts.Function
    Assert-Condition (Test-OrdinalEqual $unit.bdf.formatted $formattedBdf) 'amd_iommu_live formatted BDF is inconsistent.'
    Assert-Condition (Test-OrdinalEqual $unit.capability_offset (Format-Hex16 $facts.CapabilityOffset)) 'amd_iommu_live unit capability-offset witness is inconsistent.'
    Assert-Condition (Test-OrdinalEqual $unit.iommu_base_address (Format-Hex64 $facts.MmioBase)) 'amd_iommu_live unit MMIO-base witness is inconsistent.'
    foreach ($field in @('ivhd_type_10_present', 'ivhd_type_11_present', 'ivhd_type_40_present')) { Assert-JsonBoolean $unit.$field "amd_iommu_live.locator.unit.$field" }
    Assert-Condition ($unit.ivhd_type_10_present -eq $facts.Type10Present -and $unit.ivhd_type_11_present -eq $facts.Type11Present -and $unit.ivhd_type_40_present -eq $facts.Type40Present) 'amd_iommu_live IVHD-type presence witnesses are inconsistent.'
    if ($null -eq $facts.ExpectedEfr) {
        Assert-Condition ($null -eq $unit.expected_extended_feature_image -and $null -eq $unit.expected_extended_feature_image_2) 'amd_iommu_live unexpected IVRS EFR expectation.'
    }
    else {
        Assert-Condition (Test-OrdinalEqual $unit.expected_extended_feature_image (Format-Hex64 ([uint64] $facts.ExpectedEfr))) 'amd_iommu_live expected EFR witness is inconsistent.'
        Assert-Condition (Test-OrdinalEqual $unit.expected_extended_feature_image_2 (Format-Hex64 ([uint64] $facts.ExpectedEfr2))) 'amd_iommu_live expected EFR2 witness is inconsistent.'
    }

    $mcfg = $Locator.mcfg
    $expectedMcfg = $facts.Mcfg
    Assert-ExactProperties $mcfg @(
        'allocation_index', 'base_address', 'segment_group', 'start_bus', 'end_bus',
        'usable_start', 'usable_end_exclusive', 'ecam_function_address',
        'ecam_capability_address'
    ) 'amd_iommu_live.locator.mcfg'
    Assert-JsonInteger $mcfg.allocation_index 'amd_iommu_live MCFG allocation index' 0 255
    Assert-Condition ($mcfg.allocation_index -eq $expectedMcfg.AllocationIndex) 'amd_iommu_live MCFG allocation-index witness is inconsistent.'
    foreach ($pair in @(
        @('base_address', $expectedMcfg.Base), @('usable_start', $expectedMcfg.UsableStart),
        @('usable_end_exclusive', $expectedMcfg.UsableEnd), @('ecam_function_address', $expectedMcfg.FunctionAddress),
        @('ecam_capability_address', $expectedMcfg.CapabilityAddress)
    )) {
        Assert-Condition (Test-OrdinalEqual $mcfg.($pair[0]) (Format-Hex64 ([uint64] $pair[1]))) "amd_iommu_live MCFG $($pair[0]) witness is inconsistent."
    }
    Assert-Hex16 $mcfg.segment_group 'amd_iommu_live.locator.mcfg.segment_group'
    Assert-Hex8 $mcfg.start_bus 'amd_iommu_live.locator.mcfg.start_bus'
    Assert-Hex8 $mcfg.end_bus 'amd_iommu_live.locator.mcfg.end_bus'
    Assert-Condition (
        (Test-OrdinalEqual $mcfg.segment_group (Format-Hex16 $expectedMcfg.Segment)) -and
        (Test-OrdinalEqual $mcfg.start_bus (Format-Hex8 $expectedMcfg.StartBus)) -and
        (Test-OrdinalEqual $mcfg.end_bus (Format-Hex8 $expectedMcfg.EndBus))
    ) 'amd_iommu_live MCFG segment/bus witnesses are inconsistent.'
    Assert-V4MemoryBinding $Locator.ecam_memory_binding $Evidence $expectedMcfg.FunctionAddress ($expectedMcfg.FunctionAddress + 4096) 'amd_iommu_live.locator.ecam_memory_binding'
    return $facts
}

function Assert-V4Bdf {
    param($Bdf, $Facts, [string] $Context)
    Assert-ExactProperties $Bdf @('bus', 'device', 'function', 'formatted') $Context
    Assert-Hex8 $Bdf.bus "$Context.bus"
    Assert-Hex8 $Bdf.device "$Context.device"
    Assert-Hex8 $Bdf.function "$Context.function"
    Assert-Condition (
        (Test-OrdinalEqual $Bdf.bus (Format-Hex8 $Facts.Bus)) -and
        (Test-OrdinalEqual $Bdf.device (Format-Hex8 $Facts.Device)) -and
        (Test-OrdinalEqual $Bdf.function (Format-Hex8 $Facts.Function))
    ) "$Context does not match the IVRS DeviceID."
    Assert-Condition (Test-OrdinalEqual $Bdf.formatted ('{0:x2}:{1:x2}.{2:x1}' -f $Facts.Bus, $Facts.Device, $Facts.Function)) "$Context formatted witness is inconsistent."
}

function Assert-V4PciCapabilityRaw {
    param($Raw, [string] $Context)
    Assert-ExactProperties $Raw @(
        'header', 'base_low', 'base_high', 'range', 'miscellaneous_0',
        'miscellaneous_1'
    ) $Context
    foreach ($field in @('header', 'base_low', 'base_high', 'range', 'miscellaneous_0')) {
        Assert-Hex32 $Raw.$field "$Context.$field"
    }
    if ($null -ne $Raw.miscellaneous_1) { Assert-Hex32 $Raw.miscellaneous_1 "$Context.miscellaneous_1" }
}

function Assert-V4Pci {
    param(
        [Parameter(Mandatory)] $Pci,
        [Parameter(Mandatory)] $Access,
        [Parameter(Mandatory)] $Facts
    )

    Assert-ExactProperties $Pci @(
        'selected_root_bridge_handle_index', 'segment_group', 'bdf', 'identity',
        'capability_chain', 'capability'
    ) 'amd_iommu_live.pci'
    Assert-JsonInteger $Pci.selected_root_bridge_handle_index 'amd_iommu_live.pci.selected_root_bridge_handle_index' 0 63
    Assert-Condition ($Pci.selected_root_bridge_handle_index -lt $Access.root_bridge_handle_count) 'amd_iommu_live selected root-bridge handle index is out of range.'
    Assert-Hex16 $Pci.segment_group 'amd_iommu_live.pci.segment_group'
    Assert-Condition (Test-OrdinalEqual $Pci.segment_group (Format-Hex16 $Facts.Segment)) 'amd_iommu_live selected PCI segment does not match IVRS.'
    Assert-V4Bdf $Pci.bdf $Facts 'amd_iommu_live.pci.bdf'

    Assert-ExactProperties $Pci.identity @('raw', 'decoded') 'amd_iommu_live.pci.identity'
    $rawIdentity = $Pci.identity.raw
    Assert-ExactProperties $rawIdentity @(
        'vendor_device', 'command_status', 'class_revision', 'header_type',
        'first_capability_pointer'
    ) 'amd_iommu_live.pci.identity.raw'
    foreach ($field in @('vendor_device', 'command_status', 'class_revision')) { Assert-Hex32 $rawIdentity.$field "amd_iommu_live.pci.identity.raw.$field" }
    Assert-Hex8 $rawIdentity.header_type 'amd_iommu_live.pci.identity.raw.header_type'
    Assert-Hex8 $rawIdentity.first_capability_pointer 'amd_iommu_live.pci.identity.raw.first_capability_pointer'
    [uint32] $vendorDevice = Convert-Hex32 $rawIdentity.vendor_device
    [uint32] $commandStatus = Convert-Hex32 $rawIdentity.command_status
    [uint32] $classRevision = Convert-Hex32 $rawIdentity.class_revision
    [byte] $headerType = [Convert]::ToByte($rawIdentity.header_type.Substring(2), 16)
    [byte] $firstCapabilityPointer = [Convert]::ToByte($rawIdentity.first_capability_pointer.Substring(2), 16)
    [uint16] $vendorId = $vendorDevice -band 0xffff
    [uint16] $pciDeviceId = $vendorDevice -shr 16
    [byte] $classCode = $classRevision -shr 24
    [byte] $subclass = ($classRevision -shr 16) -band 0xff
    [byte] $programmingInterface = ($classRevision -shr 8) -band 0xff
    [byte] $revisionId = $classRevision -band 0xff
    [byte] $headerLayout = $headerType -band 0x7f
    $multifunction = ($headerType -band 0x80) -ne 0
    $capabilitiesPresent = ($commandStatus -band (1 -shl 20)) -ne 0
    Assert-Condition ($vendorId -eq 0x1022) 'amd_iommu_live selected PCI function is not AMD.'
    Assert-Condition ($classCode -eq 0x08 -and $subclass -eq 0x06 -and $programmingInterface -eq 0) 'amd_iommu_live selected PCI function has the wrong IOMMU class tuple.'
    Assert-Condition ($headerLayout -eq 0) 'amd_iommu_live selected PCI function has an unsupported header layout.'
    Assert-Condition $capabilitiesPresent 'amd_iommu_live selected PCI function does not advertise a conventional capability list.'
    Assert-Condition ($firstCapabilityPointer -ge 0x40 -and $firstCapabilityPointer -le 0xfc -and ($firstCapabilityPointer % 4) -eq 0) 'amd_iommu_live first PCI capability pointer is invalid.'

    $decodedIdentity = $Pci.identity.decoded
    Assert-ExactProperties $decodedIdentity @(
        'vendor_id', 'device_id', 'capabilities_list_present', 'class_code',
        'subclass', 'programming_interface', 'revision_id', 'header_layout',
        'multifunction', 'first_capability_pointer'
    ) 'amd_iommu_live.pci.identity.decoded'
    Assert-Condition (Test-OrdinalEqual $decodedIdentity.vendor_id (Format-Hex16 $vendorId)) 'amd_iommu_live decoded PCI vendor ID is inconsistent.'
    Assert-Condition (Test-OrdinalEqual $decodedIdentity.device_id (Format-Hex16 $pciDeviceId)) 'amd_iommu_live decoded PCI device ID is inconsistent.'
    foreach ($field in @('capabilities_list_present', 'multifunction')) { Assert-JsonBoolean $decodedIdentity.$field "amd_iommu_live.pci.identity.decoded.$field" }
    Assert-Condition ($decodedIdentity.capabilities_list_present -eq $capabilitiesPresent -and $decodedIdentity.multifunction -eq $multifunction) 'amd_iommu_live decoded PCI identity booleans are inconsistent.'
    foreach ($entry in @(
        @('class_code', $classCode), @('subclass', $subclass),
        @('programming_interface', $programmingInterface), @('revision_id', $revisionId),
        @('header_layout', $headerLayout)
    )) {
        Assert-Condition (Test-OrdinalEqual $decodedIdentity.($entry[0]) (Format-Hex8 ([byte] $entry[1]))) "amd_iommu_live decoded $($entry[0]) is inconsistent."
    }
    Assert-Condition (Test-OrdinalEqual $decodedIdentity.first_capability_pointer (Format-Hex8 $firstCapabilityPointer)) 'amd_iommu_live decoded first capability pointer is inconsistent.'

    Assert-JsonArray $Pci.capability_chain 'amd_iommu_live.pci.capability_chain'
    $links = @($Pci.capability_chain)
    Assert-Condition ($links.Count -ge 1 -and $links.Count -le 48) 'amd_iommu_live PCI capability chain is outside its hop bound.'
    [byte] $expectedPointer = $firstCapabilityPointer
    $seenPointers = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
    for ($index = 0; $index -lt $links.Count; $index++) {
        $link = $links[$index]
        Assert-ExactProperties $link @('offset', 'capability_id', 'next') 'amd_iommu_live.pci.capability_chain[]'
        foreach ($field in @('offset', 'capability_id', 'next')) { Assert-Hex8 $link.$field "amd_iommu_live.pci.capability_chain[].$field" }
        [byte] $offset = [Convert]::ToByte($link.offset.Substring(2), 16)
        [byte] $capabilityId = [Convert]::ToByte($link.capability_id.Substring(2), 16)
        [byte] $next = [Convert]::ToByte($link.next.Substring(2), 16)
        Assert-Condition ($offset -eq $expectedPointer) 'amd_iommu_live PCI capability chain is discontinuous.'
        Assert-Condition ($offset -ge 0x40 -and $offset -le 0xfc -and ($offset % 4) -eq 0) 'amd_iommu_live PCI capability link offset is invalid.'
        Assert-Condition ($seenPointers.Add($link.offset)) 'amd_iommu_live PCI capability chain is cyclic.'
        if ($offset -eq $Facts.CapabilityOffset) {
            Assert-Condition ($index -eq ($links.Count - 1)) 'amd_iommu_live recorded capability reads beyond the selected IVRS capability.'
            Assert-Condition ($capabilityId -eq 0x0f) 'amd_iommu_live target PCI capability ID is not 0x0f.'
        }
        else {
            Assert-Condition ($next -ne 0) 'amd_iommu_live PCI capability chain ended before the IVRS-selected capability.'
            Assert-Condition ($next -ge 0x40 -and $next -le 0xfc -and ($next % 4) -eq 0) 'amd_iommu_live next PCI capability pointer is invalid.'
            $expectedPointer = $next
        }
    }
    Assert-Condition ([Convert]::ToByte($links[-1].offset.Substring(2), 16) -eq $Facts.CapabilityOffset) 'amd_iommu_live IVRS-selected PCI capability was not found in the conventional chain.'

    Assert-ExactProperties $Pci.capability @('first_raw', 'second_raw', 'stable', 'decoded') 'amd_iommu_live.pci.capability'
    Assert-V4PciCapabilityRaw $Pci.capability.first_raw 'amd_iommu_live.pci.capability.first_raw'
    Assert-V4PciCapabilityRaw $Pci.capability.second_raw 'amd_iommu_live.pci.capability.second_raw'
    Assert-JsonBoolean $Pci.capability.stable 'amd_iommu_live.pci.capability.stable'
    Assert-Condition ($Pci.capability.stable -eq $true -and (Test-JsonValueEqual $Pci.capability.first_raw $Pci.capability.second_raw)) 'amd_iommu_live PCI capability snapshot is not stable.'
    $capRaw = $Pci.capability.first_raw
    [uint32] $capHeader = Convert-Hex32 $capRaw.header
    [uint32] $baseLow = Convert-Hex32 $capRaw.base_low
    [uint32] $baseHigh = Convert-Hex32 $capRaw.base_high
    [uint32] $rangeRaw = Convert-Hex32 $capRaw.range
    [uint32] $misc0 = Convert-Hex32 $capRaw.miscellaneous_0
    [byte] $capabilityId = $capHeader -band 0xff
    [byte] $nextPointer = ($capHeader -shr 8) -band 0xff
    [byte] $capabilityType = ($capHeader -shr 16) -band 0x07
    [byte] $capabilityRevision = ($capHeader -shr 19) -band 0x1f
    $efrSupported = ($capHeader -band (1 -shl 27)) -ne 0
    $capExtSupported = ($capHeader -band (1 -shl 28)) -ne 0
    $mmioEnabled = ($baseLow -band 1) -ne 0
    [uint64] $decodedBase = ([uint64] $baseHigh -shl 32) -bor [uint64] ($baseLow -band 0xffffc000)
    [byte] $paSize = ($misc0 -shr 8) -band 0x7f
    $terminalLink = $links[-1]
    Assert-Condition (
        (Test-OrdinalEqual $terminalLink.capability_id (Format-Hex8 $capabilityId)) -and
        (Test-OrdinalEqual $terminalLink.next (Format-Hex8 $nextPointer))
    ) 'amd_iommu_live terminal capability-chain link disagrees with the capability header.'
    Assert-Condition (
        $nextPointer -eq 0 -or (
            $nextPointer -ge 0x40 -and $nextPointer -le 0xfc -and
            ($nextPointer % 4) -eq 0 -and
            -not $seenPointers.Contains((Format-Hex8 $nextPointer))
        )
    ) 'amd_iommu_live target capability next pointer is invalid or points back into the read chain.'
    Assert-Condition ($capabilityId -eq 0x0f -and $capabilityType -eq 3) 'amd_iommu_live PCI capability header is not an AMD IOMMU capability.'
    Assert-Condition ($decodedBase -eq $Facts.MmioBase) 'amd_iommu_live live PCI MMIO base does not match IVRS.'
    Assert-Condition ($paSize -in @(40, 48, 52)) 'amd_iommu_live PCI IOMMU physical-address width is unsupported.'
    Assert-Condition (($capExtSupported -and $null -ne $capRaw.miscellaneous_1) -or (-not $capExtSupported -and $null -eq $capRaw.miscellaneous_1)) 'amd_iommu_live Miscellaneous Information 1 presence does not match CapExt.'

    $decoded = $Pci.capability.decoded
    Assert-ExactProperties $decoded @(
        'capability_id', 'next_pointer', 'capability_type', 'capability_revision',
        'extended_feature_register_supported', 'capability_extension_supported',
        'mmio_enabled', 'decoded_mmio_base', 'range',
        'iommu_physical_address_width', 'miscellaneous_1'
    ) 'amd_iommu_live.pci.capability.decoded'
    foreach ($entry in @(
        @('capability_id', $capabilityId), @('next_pointer', $nextPointer),
        @('capability_type', $capabilityType), @('capability_revision', $capabilityRevision)
    )) {
        Assert-Condition (Test-OrdinalEqual $decoded.($entry[0]) (Format-Hex8 ([byte] $entry[1]))) "amd_iommu_live decoded capability $($entry[0]) is inconsistent."
    }
    foreach ($entry in @(
        @('extended_feature_register_supported', $efrSupported),
        @('capability_extension_supported', $capExtSupported), @('mmio_enabled', $mmioEnabled)
    )) {
        Assert-JsonBoolean $decoded.($entry[0]) "amd_iommu_live.pci.capability.decoded.$($entry[0])"
        Assert-Condition ($decoded.($entry[0]) -eq $entry[1]) "amd_iommu_live decoded capability $($entry[0]) is inconsistent."
    }
    Assert-Condition (Test-OrdinalEqual $decoded.decoded_mmio_base (Format-Hex64 $decodedBase)) 'amd_iommu_live decoded PCI MMIO base witness is inconsistent.'
    Assert-Condition (Test-OrdinalEqual $decoded.range (Format-Hex32 $rangeRaw)) 'amd_iommu_live decoded PCI range witness is inconsistent.'
    Assert-JsonInteger $decoded.iommu_physical_address_width 'amd_iommu_live.pci.capability.decoded.iommu_physical_address_width' 1 64
    Assert-Condition ($decoded.iommu_physical_address_width -eq $paSize) 'amd_iommu_live decoded IOMMU physical-address width is inconsistent.'
    if ($null -eq $capRaw.miscellaneous_1) {
        Assert-Condition ($null -eq $decoded.miscellaneous_1) 'amd_iommu_live decoded Miscellaneous Information 1 should be null.'
    }
    else {
        Assert-Condition (Test-OrdinalEqual $decoded.miscellaneous_1 $capRaw.miscellaneous_1) 'amd_iommu_live decoded Miscellaneous Information 1 is inconsistent.'
    }
    [int] $expectedPciOperations = (5 * [int] $Access.matching_segment_handle_count) + $links.Count + $(if ($null -ne $capRaw.miscellaneous_1) { 12 } else { 10 })
    [int] $expectedPciBytes = (14 * [int] $Access.matching_segment_handle_count) + (2 * $links.Count) + $(if ($null -ne $capRaw.miscellaneous_1) { 48 } else { 40 })
    Assert-Condition ($Access.pci_read_operations -eq $expectedPciOperations) 'amd_iommu_live PCI read-operation count is inconsistent with the bounded access plan.'
    Assert-Condition ($Access.pci_read_bytes -eq $expectedPciBytes) 'amd_iommu_live PCI read-byte count is inconsistent with the bounded access plan.'
    return [pscustomobject] @{ EfrSupported = $efrSupported; MmioEnabled = $mmioEnabled; PaSize = $paSize }
}

function Assert-V4Access {
    param([Parameter(Mandatory)] $Access)
    Assert-ExactProperties $Access @(
        'pci_transport', 'mmio_transport', 'root_bridge_handle_count',
        'matching_segment_handle_count', 'full_match_handle_count',
        'pci_read_operations', 'pci_read_bytes', 'mmio_read_operations',
        'mmio_read_bytes', 'pci_write_operations', 'mmio_write_operations',
        'direct_ecam_access', 'direct_mmio_access', 'cf8_cfc_access',
        'configured_pointer_dereferences'
    ) 'amd_iommu_live.access'
    Assert-Condition (Test-OrdinalEqual $Access.pci_transport 'uefi-pci-root-bridge-io-pci-read') 'amd_iommu_live PCI transport is unexpected.'
    Assert-Condition (Test-OrdinalEqual $Access.mmio_transport 'uefi-pci-root-bridge-io-memory-read') 'amd_iommu_live MMIO transport is unexpected.'
    Assert-JsonInteger $Access.root_bridge_handle_count 'amd_iommu_live.access.root_bridge_handle_count' 1 64
    Assert-JsonInteger $Access.matching_segment_handle_count 'amd_iommu_live.access.matching_segment_handle_count' 1 64
    Assert-Condition ($Access.matching_segment_handle_count -le $Access.root_bridge_handle_count) 'amd_iommu_live matching root-bridge count exceeds all handles.'
    Assert-JsonInteger $Access.full_match_handle_count 'amd_iommu_live.access.full_match_handle_count' 1 1
    foreach ($field in @('pci_read_operations', 'pci_read_bytes', 'mmio_read_operations', 'mmio_read_bytes')) {
        Assert-JsonInteger $Access.$field "amd_iommu_live.access.$field" 0 65535
    }
    Assert-Condition ($Access.pci_read_operations -gt 0 -and $Access.pci_read_bytes -gt 0) 'amd_iommu_live must record bounded PCI reads.'
    Assert-JsonInteger $Access.pci_write_operations 'amd_iommu_live.access.pci_write_operations' 0 0
    Assert-JsonInteger $Access.mmio_write_operations 'amd_iommu_live.access.mmio_write_operations' 0 0
    foreach ($field in @('direct_ecam_access', 'direct_mmio_access', 'cf8_cfc_access')) {
        Assert-JsonBoolean $Access.$field "amd_iommu_live.access.$field"
        Assert-Condition ($Access.$field -eq $false) "amd_iommu_live.access.$field must remain false."
    }
    Assert-JsonInteger $Access.configured_pointer_dereferences 'amd_iommu_live.access.configured_pointer_dereferences' 0 0
}

function Assert-V4OffsetArray {
    param($Value, [uint16[]] $Expected, [string] $Context)
    Assert-JsonArray $Value $Context
    $actual = @($Value)
    Assert-Condition ($actual.Count -eq $Expected.Count) "$Context count is inconsistent with the read plan."
    for ($index = 0; $index -lt $Expected.Count; $index++) {
        Assert-Hex16 $actual[$index] "$Context`[]"
        Assert-Condition (Test-OrdinalEqual $actual[$index] (Format-Hex16 $Expected[$index])) "$Context is inconsistent with the read allowlist."
    }
}

function Assert-V4Snapshot {
    param(
        [Parameter(Mandatory)] $Snapshot,
        [Parameter(Mandatory)] [bool] $ExpectExtendedFeatures,
        [Parameter(Mandatory)] [int] $AdditionalSegmentCount,
        [Parameter(Mandatory)] [string] $Context
    )
    Assert-ExactProperties $Snapshot @(
        'device_table_base', 'command_buffer_base', 'event_log_base', 'control',
        'exclusion_base_or_completion_store_base',
        'exclusion_limit_or_completion_store_limit', 'extended_feature',
        'extended_feature_2', 'device_table_segments'
    ) $Context
    foreach ($field in @(
        'device_table_base', 'command_buffer_base', 'event_log_base', 'control',
        'exclusion_base_or_completion_store_base',
        'exclusion_limit_or_completion_store_limit'
    )) { Assert-Hex64 $Snapshot.$field "$Context.$field" }
    if ($ExpectExtendedFeatures) {
        Assert-Hex64 $Snapshot.extended_feature "$Context.extended_feature"
        Assert-Hex64 $Snapshot.extended_feature_2 "$Context.extended_feature_2"
    }
    else {
        Assert-Condition ($null -eq $Snapshot.extended_feature -and $null -eq $Snapshot.extended_feature_2) "$Context contains unexpected extended-feature registers."
    }
    Assert-JsonArray $Snapshot.device_table_segments "$Context.device_table_segments"
    $segments = @($Snapshot.device_table_segments)
    Assert-Condition ($segments.Count -eq $AdditionalSegmentCount) "$Context device-table segment count is inconsistent with the read plan."
    for ($index = 0; $index -lt $segments.Count; $index++) {
        Assert-ExactProperties $segments[$index] @('segment', 'raw') "$Context.device_table_segments[]"
        Assert-JsonInteger $segments[$index].segment "$Context.device_table_segments[].segment" 1 7
        Assert-Condition ($segments[$index].segment -eq ($index + 1)) "$Context device-table segments are not canonical and contiguous."
        Assert-Hex64 $segments[$index].raw "$Context.device_table_segments[].raw"
    }
}

function Assert-V4MmioCommon {
    param(
        [Parameter(Mandatory)] $Mmio,
        [Parameter(Mandatory)] $Access,
        [Parameter(Mandatory)] $Evidence,
        [Parameter(Mandatory)] $Facts,
        [Parameter(Mandatory)] [uint16[]] $ExpectedStableOffsets,
        [Parameter(Mandatory)] [AllowEmptyCollection()] [uint16[]] $ExpectedFeatureDependentOffsets,
        [Parameter(Mandatory)] [bool] $ExpectExtendedFeatures,
        [Parameter(Mandatory)] [int] $AdditionalSegmentCount,
        [Parameter(Mandatory)] [uint64] $ExpectedApertureLength,
        [Parameter(Mandatory)] [byte] $IommuPhysicalWidth,
        [Parameter(Mandatory)] [string] $ExpectedStatus
    )
    $commonProperties = @(
        'status', 'aperture', 'stable_offsets', 'feature_dependent_offsets',
        'first_snapshot', 'second_snapshot', 'stable', 'status_offset', 'status_raw'
    )
    foreach ($property in $commonProperties) {
        Assert-Condition ($null -ne $Mmio.PSObject.Properties[$property]) "amd_iommu_live.mmio is missing '$property'."
    }
    Assert-Condition (Test-OrdinalEqual $Mmio.status $ExpectedStatus) 'amd_iommu_live MMIO status is inconsistent with its variant.'
    Assert-JsonInteger $Mmio.read_operations 'amd_iommu_live.mmio.read_operations' 1 31
    Assert-Condition ($Mmio.read_operations -eq $Access.mmio_read_operations) 'amd_iommu_live MMIO read-operation witness is inconsistent.'
    Assert-ExactProperties $Mmio.aperture @('length_bytes', 'memory_binding') 'amd_iommu_live.mmio.aperture'
    Assert-JsonInteger $Mmio.aperture.length_bytes 'amd_iommu_live.mmio.aperture.length_bytes' 16384 524288
    Assert-Condition ([uint64] $Mmio.aperture.length_bytes -eq $ExpectedApertureLength) 'amd_iommu_live MMIO aperture length is inconsistent with PCSup.'
    Assert-Condition (($Facts.MmioBase % $ExpectedApertureLength) -eq 0) 'amd_iommu_live MMIO base is not aligned to the required aperture.'
    Assert-Uint64Range $Facts.MmioBase $ExpectedApertureLength 'amd_iommu_live MMIO aperture'
    [byte] $cpuPhysicalWidth = [byte] $Evidence.cpu.decoded.physical_address_bits.value
    [byte] $effectivePhysicalWidth = [Math]::Min($cpuPhysicalWidth, $IommuPhysicalWidth)
    if ($effectivePhysicalWidth -lt 64) {
        [uint64] $physicalLimit = [uint64] 1 -shl $effectivePhysicalWidth
        Assert-Condition (($Facts.MmioBase + $ExpectedApertureLength) -le $physicalLimit) 'amd_iommu_live MMIO aperture exceeds the CPU/IOMMU physical-address width.'
    }
    Assert-V4MemoryBinding $Mmio.aperture.memory_binding $Evidence $Facts.MmioBase ($Facts.MmioBase + $ExpectedApertureLength) 'amd_iommu_live.mmio.aperture.memory_binding'
    Assert-V4OffsetArray $Mmio.stable_offsets $ExpectedStableOffsets 'amd_iommu_live.mmio.stable_offsets'
    Assert-V4OffsetArray $Mmio.feature_dependent_offsets $ExpectedFeatureDependentOffsets 'amd_iommu_live.mmio.feature_dependent_offsets'
    Assert-V4Snapshot $Mmio.first_snapshot $ExpectExtendedFeatures $AdditionalSegmentCount 'amd_iommu_live.mmio.first_snapshot'
    Assert-V4Snapshot $Mmio.second_snapshot $ExpectExtendedFeatures $AdditionalSegmentCount 'amd_iommu_live.mmio.second_snapshot'
    Assert-JsonBoolean $Mmio.stable 'amd_iommu_live.mmio.stable'
    Assert-Condition ($Mmio.stable -eq $true -and (Test-JsonValueEqual $Mmio.first_snapshot $Mmio.second_snapshot)) 'amd_iommu_live stable MMIO snapshots disagree.'
    Assert-Condition (Test-OrdinalEqual $Mmio.status_offset '0x2020') 'amd_iommu_live MMIO status offset is outside the allowlist.'
    Assert-Hex64 $Mmio.status_raw 'amd_iommu_live.mmio.status_raw'
    [int] $expectedOperations = (2 * $ExpectedStableOffsets.Count) + 1
    Assert-Condition ($Access.mmio_read_operations -eq $expectedOperations) 'amd_iommu_live MMIO read-operation count is inconsistent with the snapshots.'
    Assert-Condition ($Access.mmio_read_bytes -eq (8 * $expectedOperations)) 'amd_iommu_live MMIO read-byte count is inconsistent with naturally aligned 64-bit reads.'
}

function Assert-V4Ownership {
    param([Parameter(Mandatory)] $Ownership)
    Assert-ExactProperties $Ownership @(
        'assessed', 'claim', 'requester_dma_isolation_claim',
        'interrupt_remapping_claim', 'pci_isolation_claim'
    ) 'amd_iommu_live.ownership'
    foreach ($field in @(
        'assessed', 'claim', 'requester_dma_isolation_claim',
        'interrupt_remapping_claim', 'pci_isolation_claim'
    )) {
        Assert-JsonBoolean $Ownership.$field "amd_iommu_live.ownership.$field"
        Assert-Condition ($Ownership.$field -eq $false) "amd_iommu_live.ownership.$field must remain false."
    }
}

function Get-V4SnapshotRaw {
    param($Snapshot, [uint16] $Offset)
    switch ($Offset) {
        0x0000 { return Convert-Hex64 $Snapshot.device_table_base }
        0x0008 { return Convert-Hex64 $Snapshot.command_buffer_base }
        0x0010 { return Convert-Hex64 $Snapshot.event_log_base }
        0x0018 { return Convert-Hex64 $Snapshot.control }
        0x0020 { return Convert-Hex64 $Snapshot.exclusion_base_or_completion_store_base }
        0x0028 { return Convert-Hex64 $Snapshot.exclusion_limit_or_completion_store_limit }
        0x0030 { if ($null -ne $Snapshot.extended_feature) { return Convert-Hex64 $Snapshot.extended_feature } }
        0x01a0 { if ($null -ne $Snapshot.extended_feature_2) { return Convert-Hex64 $Snapshot.extended_feature_2 } }
        default {
            if ($Offset -ge 0x0100 -and $Offset -le 0x0130 -and (($Offset - 0x0100) % 8) -eq 0) {
                $segment = (($Offset - 0x0100) / 8) + 1
                $matches = @($Snapshot.device_table_segments | Where-Object { $_.segment -eq $segment })
                if ($matches.Count -eq 1) { return Convert-Hex64 $matches[0].raw }
            }
        }
    }
    return $null
}

function Assert-V4ConfiguredMemoryBinding {
    param(
        [Parameter(Mandatory)] $Binding,
        [Parameter(Mandatory)] $Evidence,
        [Parameter(Mandatory)] [uint64] $ExpectedStart,
        [Parameter(Mandatory)] [uint64] $ExpectedEnd
    )
    Assert-ExactProperties $Binding @(
        'descriptor_index', 'descriptor', 'requested_start', 'requested_end_exclusive'
    ) 'amd_iommu_live configured-range memory binding'
    Assert-JsonInteger $Binding.descriptor_index 'amd_iommu_live configured-range descriptor index' 0 4095
    Assert-Condition (Test-OrdinalEqual $Binding.requested_start (Format-Hex64 $ExpectedStart)) 'amd_iommu_live configured-range binding start is inconsistent.'
    Assert-Condition (Test-OrdinalEqual $Binding.requested_end_exclusive (Format-Hex64 $ExpectedEnd)) 'amd_iommu_live configured-range binding end is inconsistent.'
    $descriptors = @($Evidence.memory_map.descriptors)
    $covering = New-Object System.Collections.Generic.List[int]
    for ($index = 0; $index -lt $descriptors.Count; $index++) {
        $candidate = $descriptors[$index]
        [uint64] $start = Convert-Hex64 $candidate.physical_start
        [uint64] $pages = [uint64] $candidate.page_count
        Assert-Condition ($pages -le ([uint64]::MaxValue / [uint64] 4096)) 'amd_iommu_live configured-range descriptor length overflows.'
        [uint64] $length = $pages * [uint64] 4096
        Assert-Uint64Range $start $length 'amd_iommu_live configured-range descriptor'
        if ($start -le $ExpectedStart -and $ExpectedEnd -le ($start + $length)) { [void] $covering.Add($index) }
    }
    Assert-Condition ($covering.Count -eq 1) 'amd_iommu_live configured range is not covered by exactly one same-run memory descriptor.'
    Assert-Condition ($Binding.descriptor_index -eq $covering[0]) 'amd_iommu_live configured-range descriptor index is inconsistent.'
    $selected = $descriptors[$covering[0]]
    Assert-ExactProperties $Binding.descriptor @('memory_type', 'physical_start', 'virtual_start', 'page_count', 'attributes') 'amd_iommu_live configured-range descriptor'
    Assert-JsonInteger $Binding.descriptor.memory_type 'amd_iommu_live configured-range memory type' 0 4294967295
    Assert-Condition ($Binding.descriptor.memory_type -eq (Convert-Hex32 $selected.type)) 'amd_iommu_live configured-range memory type is inconsistent.'
    foreach ($field in @('physical_start', 'virtual_start', 'attributes')) {
        Assert-Condition (Test-OrdinalEqual $Binding.descriptor.$field $selected.$field) "amd_iommu_live configured-range descriptor $field is inconsistent."
    }
    Assert-Condition ($Binding.descriptor.page_count -eq $selected.page_count) 'amd_iommu_live configured-range descriptor page count is inconsistent.'
}

function Assert-V4ConfiguredRanges {
    param(
        [Parameter(Mandatory)] $Ranges,
        [Parameter(Mandatory)] $Mmio,
        [Parameter(Mandatory)] $Evidence,
        [Parameter(Mandatory)] [byte] $IommuPhysicalWidth
    )
    Assert-JsonArray $Ranges 'amd_iommu_live.mmio.configured_ranges'
    $records = @($Ranges)
    $segmentCount = @($Mmio.first_snapshot.device_table_segments).Count
    [uint16[]] $expectedOffsets = [uint16[]] @(
        [uint16[]] @(0x0000, 0x0008, 0x0010) +
        [uint16[]] @(
            for ($index = 0; $index -lt $segmentCount; $index++) {
                [uint16] (0x0100 + (8 * $index))
            }
        )
    )
    Assert-Condition ($records.Count -eq $expectedOffsets.Count) 'amd_iommu_live configured-range set is incomplete or excessive.'
    [int] $priorOffset = -1
    [byte] $cpuPhysicalWidth = [byte] $Evidence.cpu.decoded.physical_address_bits.value
    [byte] $effectiveWidth = [Math]::Min($IommuPhysicalWidth, $cpuPhysicalWidth)
    [uint64] $control = Convert-Hex64 $Mmio.first_snapshot.control
    $iommuEnabled = ($control -band 1) -ne 0
    for ($recordIndex = 0; $recordIndex -lt $records.Count; $recordIndex++) {
        $record = $records[$recordIndex]
        Assert-ExactProperties $record @(
            'source_offset', 'raw', 'enabled', 'base', 'length', 'alignment',
            'validated_range', 'memory_binding'
        ) 'amd_iommu_live.mmio.configured_ranges[]'
        Assert-Hex16 $record.source_offset 'amd_iommu_live configured-range source offset'
        [uint16] $sourceOffset = [Convert]::ToUInt16($record.source_offset.Substring(2), 16)
        Assert-Condition ($sourceOffset -eq $expectedOffsets[$recordIndex]) 'amd_iommu_live configured-range source set or order is inconsistent.'
        Assert-Condition ($sourceOffset -gt $priorOffset) 'amd_iommu_live configured ranges are not in strict source-offset order.'
        $priorOffset = $sourceOffset
        Assert-Condition (@($Mmio.stable_offsets | Where-Object { $_ -ceq $record.source_offset }).Count -eq 1) 'amd_iommu_live configured-range source is absent from the stable read plan.'
        Assert-Hex64 $record.raw 'amd_iommu_live configured-range raw value'
        $snapshotRaw = Get-V4SnapshotRaw $Mmio.first_snapshot $sourceOffset
        Assert-Condition ($null -ne $snapshotRaw -and [uint64] $snapshotRaw -eq (Convert-Hex64 $record.raw)) 'amd_iommu_live configured-range raw witness is inconsistent with the stable snapshot.'
        [uint64] $raw = Convert-Hex64 $record.raw
        [uint64] $expectedBase = $raw -band [uint64] 0x000ffffffffff000
        [uint64] $expectedLength = 0
        $expectedEnabled = $iommuEnabled
        if ($sourceOffset -eq 0x0000) {
            $expectedLength = (($raw -band [uint64] 0x1ff) + 1) * [uint64] 4096
        }
        elseif ($sourceOffset -in @(0x0008, 0x0010)) {
            [byte] $lengthCode = ($raw -shr 56) -band 0x0f
            Assert-Condition ($lengthCode -ge 8 -and $lengthCode -le 15) 'amd_iommu_live command/event configured range has a reserved length code.'
            $expectedLength = [uint64] 1 -shl ($lengthCode + 4)
            if ($sourceOffset -eq 0x0008) { $expectedEnabled = $iommuEnabled -and (($control -band ([uint64] 1 -shl 12)) -ne 0) }
            else { $expectedEnabled = $iommuEnabled -and (($control -band ([uint64] 1 -shl 2)) -ne 0) }
        }
        else {
            $expectedLength = (($raw -band [uint64] 0xff) + 1) * [uint64] 4096
        }
        Assert-JsonBoolean $record.enabled 'amd_iommu_live configured-range enabled'
        Assert-Condition ($record.enabled -eq $expectedEnabled) 'amd_iommu_live configured-range enable gate is inconsistent with IOMMU Control.'
        Assert-Hex64 $record.base 'amd_iommu_live configured-range base'
        Assert-JsonInteger $record.length 'amd_iommu_live configured-range length' 1 ([decimal]::MaxValue)
        Assert-JsonInteger $record.alignment 'amd_iommu_live configured-range alignment' 1 ([decimal]::MaxValue)
        [uint64] $base = Convert-Hex64 $record.base
        [uint64] $length = [uint64] $record.length
        [uint64] $alignment = [uint64] $record.alignment
        Assert-Condition ($base -eq $expectedBase -and $length -eq $expectedLength -and $alignment -eq 4096) 'amd_iommu_live configured-range decode is inconsistent with its raw register.'
        Assert-Condition (($alignment -band ($alignment - 1)) -eq 0) 'amd_iommu_live configured-range alignment is not a power of two.'
        Assert-Condition (($base % $alignment) -eq 0) 'amd_iommu_live configured-range base violates its alignment.'
        if ($base -eq 0) {
            Assert-Condition (-not $record.enabled) 'amd_iommu_live enabled configured range has a zero base.'
            Assert-Condition ($null -eq $record.validated_range) 'amd_iommu_live disabled zero-base range must have a null validated range.'
            Assert-Condition ($null -eq $record.memory_binding) 'amd_iommu_live disabled zero-base range must have a null memory binding.'
            continue
        }
        Assert-Uint64Range $base $length 'amd_iommu_live configured range'
        [uint64] $end = $base + $length
        if ($effectiveWidth -lt 64) {
            [uint64] $limit = [uint64] 1 -shl $effectiveWidth
            Assert-Condition ($end -le $limit) 'amd_iommu_live configured range exceeds the CPU/IOMMU physical-address width.'
        }
        Assert-ExactProperties $record.validated_range @('start', 'end_exclusive') 'amd_iommu_live configured-range validated_range'
        Assert-Condition (Test-OrdinalEqual $record.validated_range.start (Format-Hex64 $base)) 'amd_iommu_live configured-range start witness is inconsistent.'
        Assert-Condition (Test-OrdinalEqual $record.validated_range.end_exclusive (Format-Hex64 $end)) 'amd_iommu_live configured-range end witness is inconsistent.'
        Assert-V4ConfiguredMemoryBinding $record.memory_binding $Evidence $base $end
    }
}

function Assert-V4Mmio {
    param(
        [Parameter(Mandatory)] $Mmio,
        [Parameter(Mandatory)] $Access,
        [Parameter(Mandatory)] $Evidence,
        [Parameter(Mandatory)] $Facts,
        [Parameter(Mandatory)] $PciFacts,
        [Parameter(Mandatory)] [string] $TopStatus
    )

    if (-not $PciFacts.MmioEnabled) {
        Assert-Condition (Test-OrdinalEqual $TopStatus 'pci-observed-mmio-disabled') 'amd_iommu_live top status is inconsistent with PCI Enable=0.'
        Assert-ExactProperties $Mmio @('status', 'read_operations') 'amd_iommu_live.mmio'
        Assert-Condition (Test-OrdinalEqual $Mmio.status 'not-read-capability-disabled') 'amd_iommu_live disabled MMIO status is unexpected.'
        Assert-JsonInteger $Mmio.read_operations 'amd_iommu_live.mmio.read_operations' 0 0
        Assert-Condition ($Access.mmio_read_operations -eq 0 -and $Access.mmio_read_bytes -eq 0) 'amd_iommu_live performed MMIO reads while PCI Enable was clear.'
        return
    }

    $baseOffsets = [uint16[]] @(0x0000, 0x0008, 0x0010, 0x0018, 0x0020, 0x0028)
    if ($PciFacts.EfrSupported) {
        Assert-Condition ($null -ne $Facts.ExpectedEfr -and $null -ne $Facts.ExpectedEfr2) 'amd_iommu_live PCI EFRSup lacks IVRS Type 11h/40h feature images.'
        $preflightOffsets = [uint16[]] @($baseOffsets + [uint16[]] @(0x0030, 0x01a0))
        $liveEfr = Convert-Hex64 $Mmio.first_snapshot.extended_feature
        $liveEfr2 = Convert-Hex64 $Mmio.first_snapshot.extended_feature_2
        $isConflict = ($liveEfr -ne [uint64] $Facts.ExpectedEfr -or $liveEfr2 -ne [uint64] $Facts.ExpectedEfr2)
        [uint64] $expectedApertureLength = if ((([uint64] $Facts.ExpectedEfr -bor $liveEfr) -band ([uint64] 1 -shl 9)) -ne 0) { 524288 } else { 16384 }
        if ($isConflict) {
            Assert-Condition (Test-OrdinalEqual $TopStatus 'pci-observed-efr-conflict') 'amd_iommu_live top status is inconsistent with the EFR conflict.'
            Assert-ExactProperties $Mmio @(
                'status', 'read_operations', 'aperture', 'stable_offsets', 'feature_dependent_offsets',
                'first_snapshot', 'second_snapshot', 'stable', 'status_offset',
                'status_raw', 'extended_feature_match', 'expected_extended_features',
                'live_extended_features'
            ) 'amd_iommu_live.mmio'
            Assert-V4MmioCommon $Mmio $Access $Evidence $Facts $preflightOffsets ([uint16[]] @()) $true 0 $expectedApertureLength ([byte] $PciFacts.PaSize) 'efr-conflict'
            Assert-Condition ($Mmio.extended_feature_match -is [bool] -and $Mmio.extended_feature_match -eq $false) 'amd_iommu_live EFR conflict must set extended_feature_match=false.'
            foreach ($recordName in @('expected_extended_features', 'live_extended_features')) {
                Assert-ExactProperties $Mmio.$recordName @('efr', 'efr2') "amd_iommu_live.mmio.$recordName"
                Assert-Hex64 $Mmio.$recordName.efr "amd_iommu_live.mmio.$recordName.efr"
                Assert-Hex64 $Mmio.$recordName.efr2 "amd_iommu_live.mmio.$recordName.efr2"
            }
            Assert-Condition (Test-OrdinalEqual $Mmio.expected_extended_features.efr (Format-Hex64 ([uint64] $Facts.ExpectedEfr)) -and Test-OrdinalEqual $Mmio.expected_extended_features.efr2 (Format-Hex64 ([uint64] $Facts.ExpectedEfr2))) 'amd_iommu_live expected conflict features are inconsistent with IVRS.'
            Assert-Condition (Test-OrdinalEqual $Mmio.live_extended_features.efr (Format-Hex64 $liveEfr) -and Test-OrdinalEqual $Mmio.live_extended_features.efr2 (Format-Hex64 $liveEfr2)) 'amd_iommu_live live conflict features are inconsistent with the snapshot.'
            return
        }

        Assert-Condition (Test-OrdinalEqual $TopStatus 'mmio-observed') 'amd_iommu_live top status is inconsistent with the matching MMIO snapshot.'
        [byte] $segmentSupport = ($liveEfr -shr 38) -band 0x03
        [uint64] $control = Convert-Hex64 $Mmio.first_snapshot.control
        [byte] $segmentEncoding = ($control -shr 34) -band 0x07
        Assert-Condition ($segmentEncoding -le 3) 'amd_iommu_live control contains a reserved device-table segment encoding.'
        Assert-Condition ($segmentEncoding -le $segmentSupport) 'amd_iommu_live active device-table segmentation exceeds live EFR support.'
        [int] $activeSegments = 1 -shl $segmentEncoding
        [int] $additionalSegments = $activeSegments - 1
        $segmentOffsets = New-Object System.Collections.Generic.List[uint16]
        for ($segment = 1; $segment -lt $activeSegments; $segment++) {
            [void] $segmentOffsets.Add([uint16] (0x0100 + (8 * ($segment - 1))))
        }
        [uint16[]] $featureOffsets = $segmentOffsets.ToArray()
        [uint16[]] $stableOffsets = [uint16[]] @($preflightOffsets + $featureOffsets)
        [uint64] $liveApertureLength = if (($liveEfr -band ([uint64] 1 -shl 9)) -ne 0) { 524288 } else { 16384 }
        Assert-Condition ($liveApertureLength -eq $expectedApertureLength) 'amd_iommu_live matching EFR produced a conflicting aperture requirement.'
        Assert-ExactProperties $Mmio @(
            'status', 'read_operations', 'aperture', 'stable_offsets', 'feature_dependent_offsets',
            'first_snapshot', 'second_snapshot', 'stable', 'status_offset',
            'status_raw', 'extended_feature_match', 'extended_features',
            'configured_ranges', 'decoded_state'
        ) 'amd_iommu_live.mmio'
        Assert-V4MmioCommon $Mmio $Access $Evidence $Facts $stableOffsets $featureOffsets $true $additionalSegments $liveApertureLength ([byte] $PciFacts.PaSize) 'observed'
        Assert-Condition ($Mmio.extended_feature_match -is [bool] -and $Mmio.extended_feature_match -eq $true) 'amd_iommu_live matching EFR must set extended_feature_match=true.'
        Assert-ExactProperties $Mmio.extended_features @(
            'status', 'live', 'performance_counters_supported',
            'device_table_segments_supported'
        ) 'amd_iommu_live.mmio.extended_features'
        Assert-Condition (Test-OrdinalEqual $Mmio.extended_features.status 'match') 'amd_iommu_live extended-feature status is unexpected.'
        Assert-ExactProperties $Mmio.extended_features.live @('efr', 'efr2') 'amd_iommu_live.mmio.extended_features.live'
        Assert-Condition (Test-OrdinalEqual $Mmio.extended_features.live.efr (Format-Hex64 $liveEfr) -and Test-OrdinalEqual $Mmio.extended_features.live.efr2 (Format-Hex64 $liveEfr2)) 'amd_iommu_live extended-feature live witnesses are inconsistent.'
        Assert-JsonBoolean $Mmio.extended_features.performance_counters_supported 'amd_iommu_live.mmio.extended_features.performance_counters_supported'
        Assert-Condition ($Mmio.extended_features.performance_counters_supported -eq (($liveEfr -band ([uint64] 1 -shl 9)) -ne 0)) 'amd_iommu_live PCSup decode is inconsistent.'
        Assert-JsonInteger $Mmio.extended_features.device_table_segments_supported 'amd_iommu_live.mmio.extended_features.device_table_segments_supported' 0 3
        Assert-Condition ($Mmio.extended_features.device_table_segments_supported -eq $segmentSupport) 'amd_iommu_live DevTblSegSup decode is inconsistent.'
    }
    else {
        Assert-Condition ($null -eq $Facts.ExpectedEfr -and $null -eq $Facts.ExpectedEfr2) 'amd_iommu_live IVRS EFR images are present while PCI EFRSup is clear.'
        Assert-Condition (Test-OrdinalEqual $TopStatus 'mmio-observed') 'amd_iommu_live top status is inconsistent with MMIO observation.'
        [uint64] $control = Convert-Hex64 $Mmio.first_snapshot.control
        [byte] $segmentEncoding = ($control -shr 34) -band 0x07
        Assert-Condition ($segmentEncoding -eq 0) 'amd_iommu_live device-table segmentation requires EFR support.'
        Assert-ExactProperties $Mmio @(
            'status', 'read_operations', 'aperture', 'stable_offsets', 'feature_dependent_offsets',
            'first_snapshot', 'second_snapshot', 'stable', 'status_offset',
            'status_raw', 'extended_feature_match', 'extended_features',
            'configured_ranges', 'decoded_state'
        ) 'amd_iommu_live.mmio'
        Assert-V4MmioCommon $Mmio $Access $Evidence $Facts $baseOffsets ([uint16[]] @()) $false 0 16384 ([byte] $PciFacts.PaSize) 'observed'
        Assert-Condition ($Mmio.extended_feature_match -is [bool] -and $Mmio.extended_feature_match -eq $true) 'amd_iommu_live no-EFR state must set extended_feature_match=true.'
        Assert-ExactProperties $Mmio.extended_features @('status') 'amd_iommu_live.mmio.extended_features'
        Assert-Condition (Test-OrdinalEqual $Mmio.extended_features.status 'not-supported') 'amd_iommu_live no-EFR feature status is unexpected.'
    }

    Assert-V4ConfiguredRanges $Mmio.configured_ranges $Mmio $Evidence ([byte] $PciFacts.PaSize)

    $decodedState = $Mmio.decoded_state
    Assert-ExactProperties $decodedState @(
        'iommu_enabled', 'event_log_enabled', 'command_buffer_enabled',
        'device_table_segment_encoding', 'event_log_running',
        'command_buffer_running', 'event_overflow'
    ) 'amd_iommu_live.mmio.decoded_state'
    [uint64] $finalControl = Convert-Hex64 $Mmio.first_snapshot.control
    [uint64] $statusRaw = Convert-Hex64 $Mmio.status_raw
    $decodedExpected = [ordered] @{
        iommu_enabled = ($finalControl -band 1) -ne 0
        event_log_enabled = ($finalControl -band ([uint64] 1 -shl 2)) -ne 0
        command_buffer_enabled = ($finalControl -band ([uint64] 1 -shl 12)) -ne 0
        event_log_running = ($statusRaw -band ([uint64] 1 -shl 3)) -ne 0
        command_buffer_running = ($statusRaw -band ([uint64] 1 -shl 4)) -ne 0
        event_overflow = ($statusRaw -band 1) -ne 0
    }
    foreach ($entry in $decodedExpected.GetEnumerator()) {
        Assert-JsonBoolean $decodedState.($entry.Key) "amd_iommu_live.mmio.decoded_state.$($entry.Key)"
        Assert-Condition ($decodedState.($entry.Key) -eq $entry.Value) "amd_iommu_live decoded state '$($entry.Key)' is inconsistent."
    }
    Assert-JsonInteger $decodedState.device_table_segment_encoding 'amd_iommu_live.mmio.decoded_state.device_table_segment_encoding' 0 7
    Assert-Condition ($decodedState.device_table_segment_encoding -eq (($finalControl -shr 34) -band 0x07)) 'amd_iommu_live decoded device-table segment encoding is inconsistent.'
}

function Assert-AmdIommuLiveV4 {
    param(
        [Parameter(Mandatory)] $Live,
        [Parameter(Mandatory)] $Evidence
    )
    Assert-ExactProperties $Live @('status', 'access', 'locator', 'pci', 'mmio', 'ownership') 'amd_iommu_live'
    Assert-Condition ($Live.status -in @('pci-observed-mmio-disabled', 'pci-observed-efr-conflict', 'mmio-observed')) 'amd_iommu_live status is unsupported.'
    Assert-V4Access $Live.access
    $facts = Assert-V4Locator $Live.locator $Evidence
    $pciFacts = Assert-V4Pci $Live.pci $Live.access $facts
    Assert-V4Mmio $Live.mmio $Live.access $Evidence $facts $pciFacts $Live.status
    Assert-V4Ownership $Live.ownership
}

function Get-V5CpuGateFacts {
    param([Parameter(Mandatory)] $Cpu)

    $leafMap = Get-V3CpuidLeafMap $Cpu
    Assert-Condition ($leafMap.ContainsKey('0x00000000')) 'system_registers gate requires CPUID leaf 0x00000000.'
    Assert-Condition ($leafMap.ContainsKey('0x00000001')) 'system_registers gate requires CPUID leaf 0x00000001.'
    Assert-Condition ($leafMap.ContainsKey('0x80000001')) 'system_registers gate requires CPUID leaf 0x80000001.'
    Assert-Condition ($leafMap.ContainsKey('0x80000000')) 'system_registers gate requires CPUID leaf 0x80000000.'
    $vendor = [System.Text.Encoding]::ASCII.GetString(
        ([System.BitConverter]::GetBytes((Convert-Hex32 $leafMap['0x00000000'].ebx)) +
            [System.BitConverter]::GetBytes((Convert-Hex32 $leafMap['0x00000000'].edx)) +
            [System.BitConverter]::GetBytes((Convert-Hex32 $leafMap['0x00000000'].ecx)))
    )
    $authenticAmd = Test-OrdinalEqual $vendor 'AuthenticAMD'
    $svm = ((Convert-Hex32 $leafMap['0x80000001'].ecx) -band (1 -shl 2)) -ne 0
    $maxExtended = Convert-Hex32 $leafMap['0x80000000'].eax
    $hasSvmLeaf = ($maxExtended -ge 0x8000000a) -and $leafMap.ContainsKey('0x8000000a')
    $eax1 = Convert-Hex32 $leafMap['0x00000001'].eax
    $baseFamily = ($eax1 -shr 8) -band 0x0f
    $extendedFamily = ($eax1 -shr 20) -band 0xff
    $baseModel = ($eax1 -shr 4) -band 0x0f
    $extendedModel = ($eax1 -shr 16) -band 0x0f
    $family = if ($baseFamily -eq 0x0f) { $baseFamily + $extendedFamily } else { $baseFamily }
    $model = if ($baseFamily -eq 0x06 -or $baseFamily -eq 0x0f) { $baseModel -bor ($extendedModel -shl 4) } else { $baseModel }
    return [pscustomobject] [ordered] @{
        AuthenticAmd = $authenticAmd
        Svm = $svm
        HasSvmLeaf = $hasSvmLeaf
        Family = [uint32] $family
        Model = [uint32] $model
    }
}

function Get-V5ExpectedReadOperations {
    param(
        [Parameter(Mandatory)] [uint32] $Vcnt,
        [Parameter(Mandatory)] [bool] $FixedObserved,
        [Parameter(Mandatory)] [bool] $IorrObserved
    )
    $operations = [uint64] 1 + [uint64] 10 + ([uint64] 2 * [uint64] $Vcnt)
    if ($FixedObserved) { $operations += [uint64] 11 }
    if ($IorrObserved) { $operations += [uint64] 4 }
    return $operations
}

function Assert-V5Inventory {
    param(
        [Parameter(Mandatory)] $Inventory,
        [Parameter(Mandatory)] $CpuFacts,
        [Parameter(Mandatory)] [bool] $SvmGated,
        [Parameter(Mandatory)] [string] $Context,
        [switch] $RequireSmmBaseLowNibbleZero
    )

    if (-not $SvmGated) {
        Assert-ExactProperties $Inventory @('status', 'reason') $Context
        Assert-Condition (Test-OrdinalEqual $Inventory.status 'not-attempted') "$Context must explicitly be not-attempted."
        Assert-Condition (Test-OrdinalEqual $Inventory.reason 'cpu-is-not-authentic-amd-or-svm-is-not-enumerated') "$Context has an unexpected not-attempted reason."
        return [uint64] 0
    }

    Assert-ExactProperties $Inventory @(
        'status', 'mtrr_cap', 'mtrr_def_type', 'pat', 'variable_mtrrs', 'fixed_mtrrs',
        'sys_cfg', 'hwcr', 'top_mem', 'tom2', 'smm_base', 'smm_addr', 'smm_mask', 'iorr',
        'smm_immutability_claim'
    ) $Context
    Assert-Condition (Test-OrdinalEqual $Inventory.status 'observed') "$Context must be observed for an enumerated AMD SVM CPU."
    Assert-JsonBoolean $Inventory.smm_immutability_claim "$Context.smm_immutability_claim"
    Assert-Condition ($Inventory.smm_immutability_claim -eq $false) "$Context.smm_immutability_claim must remain false."

    Assert-ExactProperties $Inventory.mtrr_cap @('raw', 'vcnt', 'fix', 'wc', 'smrr') "$Context.mtrr_cap"
    Assert-Hex64 $Inventory.mtrr_cap.raw "$Context.mtrr_cap.raw"
    $mtrrCap = Convert-Hex64 $Inventory.mtrr_cap.raw
    Assert-JsonInteger $Inventory.mtrr_cap.vcnt "$Context.mtrr_cap.vcnt" 0 8
    foreach ($field in @('fix', 'wc', 'smrr')) {
        Assert-JsonBoolean $Inventory.mtrr_cap.$field "$Context.mtrr_cap.$field"
    }
    Assert-Condition ($Inventory.mtrr_cap.vcnt -eq ($mtrrCap -band 0xff)) "$Context.mtrr_cap.vcnt is inconsistent with raw."
    Assert-Condition ($Inventory.mtrr_cap.fix -eq (($mtrrCap -band ([uint64] 1 -shl 8)) -ne 0)) "$Context.mtrr_cap.fix is inconsistent with raw."
    Assert-Condition ($Inventory.mtrr_cap.wc -eq (($mtrrCap -band ([uint64] 1 -shl 10)) -ne 0)) "$Context.mtrr_cap.wc is inconsistent with raw."
    Assert-Condition ($Inventory.mtrr_cap.smrr -eq (($mtrrCap -band ([uint64] 1 -shl 11)) -ne 0)) "$Context.mtrr_cap.smrr is inconsistent with raw."
    [uint32] $vcnt = $Inventory.mtrr_cap.vcnt
    [bool] $fixedObserved = $Inventory.mtrr_cap.fix

    Assert-ExactProperties $Inventory.mtrr_def_type @('raw', 'mem_type', 'fixed_range_enable', 'mtrr_def_type_en') "$Context.mtrr_def_type"
    Assert-Hex64 $Inventory.mtrr_def_type.raw "$Context.mtrr_def_type.raw"
    $mtrrDefType = Convert-Hex64 $Inventory.mtrr_def_type.raw
    Assert-Hex8 $Inventory.mtrr_def_type.mem_type "$Context.mtrr_def_type.mem_type"
    Assert-Condition (Test-OrdinalEqual $Inventory.mtrr_def_type.mem_type ('0x{0:x2}' -f ($mtrrDefType -band 0xff))) "$Context.mtrr_def_type.mem_type is inconsistent with raw."
    Assert-JsonBoolean $Inventory.mtrr_def_type.fixed_range_enable "$Context.mtrr_def_type.fixed_range_enable"
    Assert-Condition ($Inventory.mtrr_def_type.fixed_range_enable -eq (($mtrrDefType -band ([uint64] 1 -shl 10)) -ne 0)) "$Context.mtrr_def_type.fixed_range_enable is inconsistent with raw."
    Assert-JsonBoolean $Inventory.mtrr_def_type.mtrr_def_type_en "$Context.mtrr_def_type.mtrr_def_type_en"
    Assert-Condition ($Inventory.mtrr_def_type.mtrr_def_type_en -eq (($mtrrDefType -band ([uint64] 1 -shl 11)) -ne 0)) "$Context.mtrr_def_type.mtrr_def_type_en is inconsistent with raw."

    Assert-ExactProperties $Inventory.pat @('raw') "$Context.pat"
    Assert-Hex64 $Inventory.pat.raw "$Context.pat.raw"

    Assert-ExactProperties $Inventory.variable_mtrrs @('pair_count', 'pairs') "$Context.variable_mtrrs"
    Assert-JsonInteger $Inventory.variable_mtrrs.pair_count "$Context.variable_mtrrs.pair_count" 0 8
    Assert-Condition ($Inventory.variable_mtrrs.pair_count -eq $vcnt) "$Context.variable_mtrrs.pair_count must equal the recomputed MTRRcap VCNT."
    Assert-JsonArray $Inventory.variable_mtrrs.pairs "$Context.variable_mtrrs.pairs"
    $pairs = @($Inventory.variable_mtrrs.pairs)
    Assert-Condition ($pairs.Count -eq $vcnt) "$Context.variable_mtrrs.pairs must contain exactly VCNT entries."
    for ($index = 0; $index -lt $pairs.Count; $index++) {
        $pair = $pairs[$index]
        Assert-ExactProperties $pair @('index', 'base_raw', 'mask_raw', 'mem_type', 'phys_base', 'phys_mask', 'valid') "$Context.variable_mtrrs.pairs[]"
        Assert-JsonInteger $pair.index "$Context.variable_mtrrs.pairs[].index" 0 7
        Assert-Condition ($pair.index -eq $index) "$Context.variable_mtrrs.pairs[].index must be sequential."
        Assert-Hex64 $pair.base_raw "$Context.variable_mtrrs.pairs[].base_raw"
        Assert-Hex64 $pair.mask_raw "$Context.variable_mtrrs.pairs[].mask_raw"
        $baseRaw = Convert-Hex64 $pair.base_raw
        $maskRaw = Convert-Hex64 $pair.mask_raw
        Assert-Hex8 $pair.mem_type "$Context.variable_mtrrs.pairs[].mem_type"
        Assert-Condition (Test-OrdinalEqual $pair.mem_type ('0x{0:x2}' -f ($baseRaw -band 0xff))) "$Context.variable_mtrrs.pairs[].mem_type is inconsistent with base_raw."
        Assert-Hex64 $pair.phys_base "$Context.variable_mtrrs.pairs[].phys_base"
        Assert-Condition (Test-OrdinalEqual $pair.phys_base ('0x{0:x16}' -f ($baseRaw -band ([uint64] 0x0000fffffffff000)))) "$Context.variable_mtrrs.pairs[].phys_base is inconsistent with base_raw."
        Assert-Hex64 $pair.phys_mask "$Context.variable_mtrrs.pairs[].phys_mask"
        Assert-Condition (Test-OrdinalEqual $pair.phys_mask ('0x{0:x16}' -f ($maskRaw -band ([uint64] 0x0000fffffffff000)))) "$Context.variable_mtrrs.pairs[].phys_mask is inconsistent with mask_raw."
        Assert-JsonBoolean $pair.valid "$Context.variable_mtrrs.pairs[].valid"
        Assert-Condition ($pair.valid -eq (($maskRaw -band ([uint64] 1 -shl 11)) -ne 0)) "$Context.variable_mtrrs.pairs[].valid is inconsistent with mask_raw."
    }

    if ($fixedObserved) {
        Assert-ExactProperties $Inventory.fixed_mtrrs @('status', 'raw') "$Context.fixed_mtrrs"
        Assert-Condition (Test-OrdinalEqual $Inventory.fixed_mtrrs.status 'observed') "$Context.fixed_mtrrs must be observed when MTRRcap[FIX] is set."
        Assert-JsonArray $Inventory.fixed_mtrrs.raw "$Context.fixed_mtrrs.raw"
        Assert-Condition (@($Inventory.fixed_mtrrs.raw).Count -eq 11) "$Context.fixed_mtrrs.raw must contain exactly 11 registers."
        foreach ($raw in @($Inventory.fixed_mtrrs.raw)) {
            Assert-Hex64 $raw "$Context.fixed_mtrrs.raw[]"
        }
    }
    else {
        Assert-ExactProperties $Inventory.fixed_mtrrs @('status', 'reason') "$Context.fixed_mtrrs"
        Assert-Condition (Test-OrdinalEqual $Inventory.fixed_mtrrs.status 'not-attempted') "$Context.fixed_mtrrs must be not-attempted when MTRRcap[FIX] is clear."
        Assert-Condition (Test-OrdinalEqual $Inventory.fixed_mtrrs.reason 'mtrrcap-fix-clear') "$Context.fixed_mtrrs has an unexpected not-attempted reason."
    }

    Assert-ExactProperties $Inventory.sys_cfg @(
        'raw', 'mtrr_fix_dram_en', 'mtrr_fix_dram_mod_en', 'mtrr_var_dram_en',
        'mtrr_tom2_en', 'tom2_force_mem_type_wb', 'smee_raw',
        'secure_nested_paging_en_raw', 'vmpl_en_raw', 'hmkee_raw',
        'encryption_state_claim'
    ) "$Context.sys_cfg"
    Assert-Hex64 $Inventory.sys_cfg.raw "$Context.sys_cfg.raw"
    $sysCfg = Convert-Hex64 $Inventory.sys_cfg.raw
    $sysCfgBits = [ordered] @{
        mtrr_fix_dram_en = 18
        mtrr_fix_dram_mod_en = 19
        mtrr_var_dram_en = 20
        mtrr_tom2_en = 21
        tom2_force_mem_type_wb = 22
        smee_raw = 23
        secure_nested_paging_en_raw = 24
        vmpl_en_raw = 25
        hmkee_raw = 26
    }
    foreach ($entry in $sysCfgBits.GetEnumerator()) {
        Assert-JsonBoolean $Inventory.sys_cfg.($entry.Key) "$Context.sys_cfg.$($entry.Key)"
        $expected = ($sysCfg -band ([uint64] 1 -shl $entry.Value)) -ne 0
        Assert-Condition ($Inventory.sys_cfg.($entry.Key) -eq $expected) "$Context.sys_cfg.$($entry.Key) is inconsistent with raw."
    }
    Assert-JsonBoolean $Inventory.sys_cfg.encryption_state_claim "$Context.sys_cfg.encryption_state_claim"
    Assert-Condition ($Inventory.sys_cfg.encryption_state_claim -eq $false) "$Context.sys_cfg.encryption_state_claim must remain false."

    Assert-ExactProperties $Inventory.hwcr @('raw', 'smm_lock', 'smm_pg_cfg_lock') "$Context.hwcr"
    Assert-Hex64 $Inventory.hwcr.raw "$Context.hwcr.raw"
    $hwcr = Convert-Hex64 $Inventory.hwcr.raw
    Assert-JsonBoolean $Inventory.hwcr.smm_lock "$Context.hwcr.smm_lock"
    Assert-Condition ($Inventory.hwcr.smm_lock -eq (($hwcr -band 1) -ne 0)) "$Context.hwcr.smm_lock is inconsistent with raw."
    Assert-JsonBoolean $Inventory.hwcr.smm_pg_cfg_lock "$Context.hwcr.smm_pg_cfg_lock"
    Assert-Condition ($Inventory.hwcr.smm_pg_cfg_lock -eq (($hwcr -band ([uint64] 1 -shl 33)) -ne 0)) "$Context.hwcr.smm_pg_cfg_lock is inconsistent with raw."

    Assert-ExactProperties $Inventory.top_mem @('raw', 'tom') "$Context.top_mem"
    Assert-Hex64 $Inventory.top_mem.raw "$Context.top_mem.raw"
    Assert-Hex64 $Inventory.top_mem.tom "$Context.top_mem.tom"
    Assert-Condition (Test-OrdinalEqual $Inventory.top_mem.tom ('0x{0:x16}' -f ((Convert-Hex64 $Inventory.top_mem.raw) -band ([uint64] 0x0000ffffff800000)))) "$Context.top_mem.tom is inconsistent with raw."

    Assert-ExactProperties $Inventory.tom2 @('raw', 'tom2') "$Context.tom2"
    Assert-Hex64 $Inventory.tom2.raw "$Context.tom2.raw"
    Assert-Hex64 $Inventory.tom2.tom2 "$Context.tom2.tom2"
    Assert-Condition (Test-OrdinalEqual $Inventory.tom2.tom2 ('0x{0:x16}' -f ((Convert-Hex64 $Inventory.tom2.raw) -band ([uint64] 0x0000ffffff800000)))) "$Context.tom2.tom2 is inconsistent with raw."

    Assert-ExactProperties $Inventory.smm_base @('raw', 'smm_base_address') "$Context.smm_base"
    Assert-Hex64 $Inventory.smm_base.raw "$Context.smm_base.raw"
    Assert-Hex32 $Inventory.smm_base.smm_base_address "$Context.smm_base.smm_base_address"
    $smmBaseRaw = Convert-Hex64 $Inventory.smm_base.raw
    Assert-Condition (Test-OrdinalEqual $Inventory.smm_base.smm_base_address ('0x{0:x8}' -f ($smmBaseRaw -band ([uint64] 4294967295)))) "$Context.smm_base.smm_base_address is inconsistent with raw."
    if ($RequireSmmBaseLowNibbleZero) {
        Assert-Condition (($smmBaseRaw -band 0x0f) -eq 0) "$Context.smm_base.raw has nonzero required-zero SmmBase[3:0] bits."
    }

    Assert-ExactProperties $Inventory.smm_addr @('raw', 'tseg_base') "$Context.smm_addr"
    Assert-Hex64 $Inventory.smm_addr.raw "$Context.smm_addr.raw"
    Assert-Hex64 $Inventory.smm_addr.tseg_base "$Context.smm_addr.tseg_base"
    # PPR MSRC001_0112 exposes TSegBase[47:17]. Keep all 31 address bits;
    # 0x0000fffffe0000 would silently truncate bits 47:40.
    Assert-Condition (Test-OrdinalEqual $Inventory.smm_addr.tseg_base ('0x{0:x16}' -f ((Convert-Hex64 $Inventory.smm_addr.raw) -band ([uint64] 0x0000fffffffe0000)))) "$Context.smm_addr.tseg_base is inconsistent with raw."

    Assert-ExactProperties $Inventory.smm_mask @(
        'raw', 'tseg_mask', 'a_valid', 't_valid', 'a_close', 't_close',
        'am_type_io_wc', 'tm_type_io_wc', 'am_type_dram', 'tm_type_dram'
    ) "$Context.smm_mask"
    Assert-Hex64 $Inventory.smm_mask.raw "$Context.smm_mask.raw"
    $smmMask = Convert-Hex64 $Inventory.smm_mask.raw
    Assert-Hex64 $Inventory.smm_mask.tseg_mask "$Context.smm_mask.tseg_mask"
    # PPR MSRC001_0113 exposes TSegMask[47:17], with the same full-width mask.
    Assert-Condition (Test-OrdinalEqual $Inventory.smm_mask.tseg_mask ('0x{0:x16}' -f ($smmMask -band ([uint64] 0x0000fffffffe0000)))) "$Context.smm_mask.tseg_mask is inconsistent with raw."
    $smmMaskBits = [ordered] @{
        a_valid = 0
        t_valid = 1
        a_close = 2
        t_close = 3
        am_type_io_wc = 4
        tm_type_io_wc = 5
    }
    foreach ($entry in $smmMaskBits.GetEnumerator()) {
        Assert-JsonBoolean $Inventory.smm_mask.($entry.Key) "$Context.smm_mask.$($entry.Key)"
        $expected = ($smmMask -band ([uint64] 1 -shl $entry.Value)) -ne 0
        Assert-Condition ($Inventory.smm_mask.($entry.Key) -eq $expected) "$Context.smm_mask.$($entry.Key) is inconsistent with raw."
    }
    Assert-Hex8 $Inventory.smm_mask.am_type_dram "$Context.smm_mask.am_type_dram"
    Assert-Condition (Test-OrdinalEqual $Inventory.smm_mask.am_type_dram ('0x{0:x2}' -f (($smmMask -shr 8) -band 0x7))) "$Context.smm_mask.am_type_dram is inconsistent with raw."
    Assert-Hex8 $Inventory.smm_mask.tm_type_dram "$Context.smm_mask.tm_type_dram"
    Assert-Condition (Test-OrdinalEqual $Inventory.smm_mask.tm_type_dram ('0x{0:x2}' -f (($smmMask -shr 12) -band 0x7))) "$Context.smm_mask.tm_type_dram is inconsistent with raw."

    $iorrGated = $CpuFacts.AuthenticAmd -and $CpuFacts.Family -eq 26 -and $CpuFacts.Model -eq 68
    if ($iorrGated) {
        Assert-ExactProperties $Inventory.iorr @('status', 'ranges') "$Context.iorr"
        Assert-Condition (Test-OrdinalEqual $Inventory.iorr.status 'observed') "$Context.iorr must be observed on the pinned PPR."
        Assert-JsonArray $Inventory.iorr.ranges "$Context.iorr.ranges"
        $ranges = @($Inventory.iorr.ranges)
        Assert-Condition ($ranges.Count -eq 2) "$Context.iorr.ranges must contain exactly two ranges."
        for ($index = 0; $index -lt $ranges.Count; $index++) {
            $range = $ranges[$index]
            Assert-ExactProperties $range @('index', 'base_raw', 'mask_raw', 'phys_base', 'phys_mask', 'rd_mem', 'wr_mem', 'valid') "$Context.iorr.ranges[]"
            Assert-JsonInteger $range.index "$Context.iorr.ranges[].index" 0 1
            Assert-Condition ($range.index -eq $index) "$Context.iorr.ranges[].index must be sequential."
            Assert-Hex64 $range.base_raw "$Context.iorr.ranges[].base_raw"
            Assert-Hex64 $range.mask_raw "$Context.iorr.ranges[].mask_raw"
            $baseRaw = Convert-Hex64 $range.base_raw
            $maskRaw = Convert-Hex64 $range.mask_raw
            Assert-Hex64 $range.phys_base "$Context.iorr.ranges[].phys_base"
            Assert-Condition (Test-OrdinalEqual $range.phys_base ('0x{0:x16}' -f ($baseRaw -band ([uint64] 0x0000fffffffff000)))) "$Context.iorr.ranges[].phys_base is inconsistent with base_raw."
            Assert-Hex64 $range.phys_mask "$Context.iorr.ranges[].phys_mask"
            Assert-Condition (Test-OrdinalEqual $range.phys_mask ('0x{0:x16}' -f ($maskRaw -band ([uint64] 0x0000fffffffff000)))) "$Context.iorr.ranges[].phys_mask is inconsistent with mask_raw."
            Assert-JsonBoolean $range.rd_mem "$Context.iorr.ranges[].rd_mem"
            Assert-Condition ($range.rd_mem -eq (($baseRaw -band ([uint64] 1 -shl 4)) -ne 0)) "$Context.iorr.ranges[].rd_mem is inconsistent with base_raw."
            Assert-JsonBoolean $range.wr_mem "$Context.iorr.ranges[].wr_mem"
            Assert-Condition ($range.wr_mem -eq (($baseRaw -band ([uint64] 1 -shl 3)) -ne 0)) "$Context.iorr.ranges[].wr_mem is inconsistent with base_raw."
            Assert-JsonBoolean $range.valid "$Context.iorr.ranges[].valid"
            Assert-Condition ($range.valid -eq (($maskRaw -band ([uint64] 1 -shl 11)) -ne 0)) "$Context.iorr.ranges[].valid is inconsistent with mask_raw."
        }
    }
    else {
        Assert-ExactProperties $Inventory.iorr @('status', 'reason') "$Context.iorr"
        Assert-Condition (Test-OrdinalEqual $Inventory.iorr.status 'not-attempted') "$Context.iorr must be not-attempted off the pinned PPR."
        Assert-Condition (Test-OrdinalEqual $Inventory.iorr.reason 'cpu-family-model-not-documented-by-pinned-ppr') "$Context.iorr has an unexpected not-attempted reason."
    }

    return Get-V5ExpectedReadOperations -Vcnt $vcnt -FixedObserved $fixedObserved -IorrObserved $iorrGated
}

function Assert-SystemRegistersV5OrV6 {
    param(
        [Parameter(Mandatory)] $Section,
        [Parameter(Mandatory)] $Evidence,
        [Parameter(Mandatory)] [string] $ExpectedComparison,
        [switch] $ExcludeThreadScopedSmmBase,
        [switch] $RequireSmmBaseLowNibbleZero
    )

    Assert-ExactProperties $Section @('scope', 'policy', 'access', 'bsp') 'system_registers'
    Assert-Condition (Test-OrdinalEqual $Section.scope 'all-enabled-healthy-processors-from-matching-pre-and-post-mp-services-enumerations') 'system_registers scope is unexpected.'
    Assert-ExactProperties $Section.policy @(
        'document', 'comparison', 'callback_system_register_reads',
        'callback_control_state_writes', 'ppr'
    ) 'system_registers.policy'
    Assert-Condition (Test-OrdinalEqual $Section.policy.document 'docs/m0b-msr-read-policy.md') 'system_registers policy document is unexpected.'
    Assert-Condition (Test-OrdinalEqual $Section.policy.comparison $ExpectedComparison) 'system_registers comparison policy is unexpected.'
    Assert-JsonBoolean $Section.policy.callback_system_register_reads 'system_registers.policy.callback_system_register_reads'
    Assert-Condition ($Section.policy.callback_system_register_reads -eq $true) 'system_registers must record callback register reads as true.'
    Assert-JsonBoolean $Section.policy.callback_control_state_writes 'system_registers.policy.callback_control_state_writes'
    Assert-Condition ($Section.policy.callback_control_state_writes -eq $false) 'system_registers callback writes must remain false.'
    Assert-ExactProperties $Section.policy.ppr @(
        'publisher', 'publication', 'revision', 'date', 'coverage', 'sha256'
    ) 'system_registers.policy.ppr'
    $pprConstants = [ordered] @{
        publisher = 'AMD'
        publication = '57896'
        revision = '3.00'
        date = 'August 28, 2024'
        coverage = 'AMD Family 1Ah Model 44h B0'
        sha256 = '643cae09d0bdae788ab090c0c4185168482b424e79f9feeced3f14c6de1817e5'
    }
    foreach ($entry in $pprConstants.GetEnumerator()) {
        Assert-Condition (Test-OrdinalEqual $Section.policy.ppr.($entry.Key) $entry.Value) "system_registers PPR pin '$($entry.Key)' is unexpected."
    }
    Assert-ExactProperties $Section.access @(
        'msr_read_operations', 'msr_read_bytes', 'msr_write_operations'
    ) 'system_registers.access'
    Assert-JsonInteger $Section.access.msr_write_operations 'system_registers.access.msr_write_operations' 0 0
    Assert-JsonInteger $Section.access.msr_read_operations 'system_registers.access.msr_read_operations' 0 10960
    Assert-JsonInteger $Section.access.msr_read_bytes 'system_registers.access.msr_read_bytes' 0 ([decimal]::MaxValue)
    Assert-Condition ($Section.access.msr_read_bytes -eq (8 * $Section.access.msr_read_operations)) 'system_registers read bytes must equal eight times the read operations.'

    $consistency = $Evidence.processor_consistency
    Assert-ExactProperties $consistency @(
        'status', 'scope', 'dispatch', 'comparison_policy', 'bsp_processor_number',
        'enabled_processor_count', 'observation_count', 'all_enabled_processors_observed',
        'observations', 'identity_consistent', 'cpuid_consistent', 'vm_cr_consistent',
        'system_registers_consistent', 'consistent'
    ) 'processor_consistency'
    Assert-ExactProperties $consistency.comparison_policy @(
        'reference', 'raw_cpuid_compared', 'leaf_00000001_ebx_compared_mask',
        'leaf_8000001e_eax_compared_mask', 'leaf_8000001e_ebx_compared_mask',
        'leaf_8000001e_ecx_compared_mask', 'leaf_8000001e_edx_compared_mask',
        'all_other_collected_register_masks', 'vm_cr_comparison',
        'system_registers_comparison', 'identity_comparison'
    ) 'processor_consistency.comparison_policy'
    Assert-Condition (Test-OrdinalEqual $consistency.comparison_policy.system_registers_comparison $ExpectedComparison) 'processor_consistency system-register comparison policy is unexpected.'

    $observations = @($consistency.observations)
    $bspObservationIndexes = @(
        for ($index = 0; $index -lt $observations.Count; $index++) {
            if ($observations[$index].bsp) { $index }
        }
    )
    Assert-Condition ($bspObservationIndexes.Count -eq 1) 'processor_consistency observations must contain exactly one BSP.'
    $bspObservation = $observations[$bspObservationIndexes[0]]

    [uint64] $expectedOperations = 0
    $systemRegistersAll = $true
    foreach ($observation in $observations) {
        Assert-ExactProperties $observation @(
            'processor_number', 'processor_id', 'bsp', 'dispatch', 'who_am_i', 'cpu', 'vm_cr',
            'system_registers', 'leaf_00000001_initial_apic_id', 'leaf_8000001e_extended_apic_id',
            'identity_matches_mp', 'cpuid_matches_bsp', 'vm_cr_matches_bsp',
            'system_registers_matches_bsp'
        ) 'processor_consistency.observations[]'
        $cpuFacts = Get-V5CpuGateFacts $observation.cpu
        $svmGated = $cpuFacts.AuthenticAmd -and $cpuFacts.Svm -and $cpuFacts.HasSvmLeaf
        Assert-Condition (($observation.system_registers.status -ceq 'observed') -eq ($svmGated -and (Test-OrdinalEqual $observation.vm_cr.status 'observed'))) 'system_registers and vm_cr must share the same enumeration gate on every observation.'
        $operations = Assert-V5Inventory `
            -Inventory $observation.system_registers `
            -CpuFacts $cpuFacts `
            -SvmGated $svmGated `
            -Context 'processor_consistency.observations[].system_registers' `
            -RequireSmmBaseLowNibbleZero:$RequireSmmBaseLowNibbleZero
        $expectedOperations += [uint64] $operations
        Assert-JsonBoolean $observation.system_registers_matches_bsp 'processor_consistency.observations[].system_registers_matches_bsp'
        $matches = if ($ExcludeThreadScopedSmmBase) {
            Test-V6SystemRegistersEqual $observation.system_registers $bspObservation.system_registers
        }
        else {
            Test-JsonValueEqual $observation.system_registers $bspObservation.system_registers
        }
        Assert-Condition ($observation.system_registers_matches_bsp -eq $matches) 'processor_consistency system_registers_matches_bsp witness is falsified.'
        if (-not $matches) { $systemRegistersAll = $false }
    }
    Assert-Condition ($Section.access.msr_read_operations -eq $expectedOperations) 'system_registers read operations do not match the recomputed site total.'
    Assert-Condition (Test-JsonValueEqual $Section.bsp $bspObservation.system_registers) 'system_registers.bsp does not exactly match the MP Services BSP observation.'

    Assert-JsonBoolean $consistency.system_registers_consistent 'processor_consistency.system_registers_consistent'
    Assert-Condition ($consistency.system_registers_consistent -eq $systemRegistersAll) 'processor_consistency system_registers_consistent aggregate is falsified.'
    $expectedConsistent = (
        $consistency.identity_consistent -and
        $consistency.cpuid_consistent -and
        $consistency.vm_cr_consistent -and
        $consistency.system_registers_consistent
    )
    Assert-Condition ($consistency.consistent -eq $expectedConsistent) 'processor_consistency consistent aggregate is falsified.'
}

function Assert-RawEvidenceV5OrV6 {
    param(
        [Parameter(Mandatory)] $Evidence,
        [Parameter(Mandatory)] [string] $ExpectedTargetManifestSha256,
        [Parameter(Mandatory)] [string] $ExpectedRawEvidenceFilename,
        [Parameter(Mandatory)] [int] $SchemaVersion,
        [Parameter(Mandatory)] [string] $CollectorVersion,
        [Parameter(Mandatory)] [string] $ExpectedComparison,
        [switch] $ExcludeThreadScopedSmmBase
    )

    Assert-ExactProperties $Evidence @(
        'schema_version', 'evidence_kind', 'qualification_status', 'launch_authorized',
        'physical_candidate_flash_authorized', 'control_state_writes_authorized',
        'process_introspection_authorized', 'confidential_vm_claim',
        'amd_iommu_ownership_claim', 'pci_isolation_claim', 'collector',
        'target_profile_manifest_sha256', 'collected_at', 'sink', 'uefi', 'cpu',
        'vm_cr', 'processor_consistency', 'mp_services', 'memory_map', 'acpi',
        'amd_iommu_live', 'system_registers', 'uncollected_blockers'
    ) 'raw evidence'
    Assert-JsonInteger $Evidence.schema_version 'schema_version' $SchemaVersion $SchemaVersion
    Assert-Condition (Test-OrdinalEqual $Evidence.evidence_kind 'uefi-record-only-inventory-slice') "Unexpected v$SchemaVersion evidence_kind."
    Assert-ExactProperties $Evidence.collector @('name', 'version', 'slice') 'collector'
    Assert-Condition (Test-OrdinalEqual $Evidence.collector.name 'svmvisor-m0b-probe') 'Unexpected collector name.'
    Assert-Condition (Test-OrdinalEqual $Evidence.collector.version $CollectorVersion) "Unexpected v$SchemaVersion collector version."
    Assert-Condition (Test-OrdinalEqual $Evidence.collector.slice 'read-only-per-processor-system-register-inventory') "Unexpected v$SchemaVersion collector slice."

    $systemRegisterBlockers = @(
        'amd-iommu-dte-requester-ownership-and-pci-isolation',
        'inherited-memory-encryption-state',
        'secure-boot-databases-option-rom-policy-and-tcg-log',
        'boot-driver-sysprep-recovery-and-hotkey-namespace',
        'ready-to-boot-after-ready-to-boot-exit-boot-services-order',
        'direct-watchdog-and-durable-attempt-lease',
        'mp-services-ap-dispatch-pre-measurement-control-state-preservation'
    )
    Assert-JsonArray $Evidence.uncollected_blockers 'uncollected_blockers'
    Assert-Condition (@($Evidence.uncollected_blockers).Count -eq $systemRegisterBlockers.Count) "Unexpected v$SchemaVersion uncollected-blocker count."
    for ($index = 0; $index -lt $systemRegisterBlockers.Count; $index++) {
        Assert-Condition (Test-OrdinalEqual $Evidence.uncollected_blockers[$index] $systemRegisterBlockers[$index]) "Uncollected blockers do not match schema v$SchemaVersion."
    }

    # Schemas v5 and v6 are deliberately cumulative. Project every
    # pre-system-register
    # field to the exact v4 contract first so later checks cannot weaken
    # earlier gates. The processor_consistency projection strips the later
    # additions so the v3 per-processor contract still applies unchanged.
    $projectedObservations = @(
        foreach ($observation in @($Evidence.processor_consistency.observations)) {
            [pscustomobject] [ordered] @{
                processor_number = $observation.processor_number
                processor_id = $observation.processor_id
                bsp = $observation.bsp
                dispatch = $observation.dispatch
                who_am_i = $observation.who_am_i
                cpu = $observation.cpu
                vm_cr = $observation.vm_cr
                leaf_00000001_initial_apic_id = $observation.leaf_00000001_initial_apic_id
                leaf_8000001e_extended_apic_id = $observation.leaf_8000001e_extended_apic_id
                identity_matches_mp = $observation.identity_matches_mp
                cpuid_matches_bsp = $observation.cpuid_matches_bsp
                vm_cr_matches_bsp = $observation.vm_cr_matches_bsp
            }
        }
    )
    $consistency = $Evidence.processor_consistency
    $projectedPolicy = [pscustomobject] [ordered] @{
        reference = $consistency.comparison_policy.reference
        raw_cpuid_compared = $consistency.comparison_policy.raw_cpuid_compared
        leaf_00000001_ebx_compared_mask = $consistency.comparison_policy.leaf_00000001_ebx_compared_mask
        leaf_8000001e_eax_compared_mask = $consistency.comparison_policy.leaf_8000001e_eax_compared_mask
        leaf_8000001e_ebx_compared_mask = $consistency.comparison_policy.leaf_8000001e_ebx_compared_mask
        leaf_8000001e_ecx_compared_mask = $consistency.comparison_policy.leaf_8000001e_ecx_compared_mask
        leaf_8000001e_edx_compared_mask = $consistency.comparison_policy.leaf_8000001e_edx_compared_mask
        all_other_collected_register_masks = $consistency.comparison_policy.all_other_collected_register_masks
        vm_cr_comparison = $consistency.comparison_policy.vm_cr_comparison
        identity_comparison = $consistency.comparison_policy.identity_comparison
    }
    $projectedConsistency = [pscustomobject] [ordered] @{
        status = $consistency.status
        scope = $consistency.scope
        dispatch = $consistency.dispatch
        comparison_policy = $projectedPolicy
        bsp_processor_number = $consistency.bsp_processor_number
        enabled_processor_count = $consistency.enabled_processor_count
        observation_count = $consistency.observation_count
        all_enabled_processors_observed = $consistency.all_enabled_processors_observed
        observations = $projectedObservations
        identity_consistent = $consistency.identity_consistent
        cpuid_consistent = $consistency.cpuid_consistent
        vm_cr_consistent = $consistency.vm_cr_consistent
        consistent = (
            $consistency.identity_consistent -and
            $consistency.cpuid_consistent -and
            $consistency.vm_cr_consistent
        )
    }
    $v4Projection = [pscustomobject] [ordered] @{
        schema_version = 4
        evidence_kind = $Evidence.evidence_kind
        qualification_status = $Evidence.qualification_status
        launch_authorized = $Evidence.launch_authorized
        physical_candidate_flash_authorized = $Evidence.physical_candidate_flash_authorized
        control_state_writes_authorized = $Evidence.control_state_writes_authorized
        process_introspection_authorized = $Evidence.process_introspection_authorized
        confidential_vm_claim = $Evidence.confidential_vm_claim
        amd_iommu_ownership_claim = $Evidence.amd_iommu_ownership_claim
        pci_isolation_claim = $Evidence.pci_isolation_claim
        collector = [pscustomobject] [ordered] @{
            name = 'svmvisor-m0b-probe'
            version = '0.4.0'
            slice = 'ivrs-derived-read-only-amd-iommu-live-state'
        }
        target_profile_manifest_sha256 = $Evidence.target_profile_manifest_sha256
        collected_at = $Evidence.collected_at
        sink = $Evidence.sink
        uefi = $Evidence.uefi
        cpu = $Evidence.cpu
        vm_cr = $Evidence.vm_cr
        processor_consistency = $projectedConsistency
        mp_services = $Evidence.mp_services
        memory_map = $Evidence.memory_map
        acpi = $Evidence.acpi
        amd_iommu_live = $Evidence.amd_iommu_live
        uncollected_blockers = @(
            'amd-iommu-dte-requester-ownership-and-pci-isolation',
            'smm-lock-and-ppr-specific-msrs',
            'inherited-memory-encryption-state',
            'mtrrs-iorrs-tom-tom2-and-mmio-apertures',
            'secure-boot-databases-option-rom-policy-and-tcg-log',
            'boot-driver-sysprep-recovery-and-hotkey-namespace',
            'ready-to-boot-after-ready-to-boot-exit-boot-services-order',
            'direct-watchdog-and-durable-attempt-lease',
            'mp-services-ap-dispatch-pre-measurement-control-state-preservation'
        )
    }
    Assert-RawEvidenceV4 $v4Projection $ExpectedTargetManifestSha256 $ExpectedRawEvidenceFilename
    Assert-SystemRegistersV5OrV6 `
        -Section $Evidence.system_registers `
        -Evidence $Evidence `
        -ExpectedComparison $ExpectedComparison `
        -ExcludeThreadScopedSmmBase:$ExcludeThreadScopedSmmBase `
        -RequireSmmBaseLowNibbleZero:($SchemaVersion -eq 6)
}

function Assert-RawEvidenceV5 {
    param(
        [Parameter(Mandatory)] $Evidence,
        [Parameter(Mandatory)] [string] $ExpectedTargetManifestSha256,
        [Parameter(Mandatory)] [string] $ExpectedRawEvidenceFilename
    )
    Assert-RawEvidenceV5OrV6 `
        -Evidence $Evidence `
        -ExpectedTargetManifestSha256 $ExpectedTargetManifestSha256 `
        -ExpectedRawEvidenceFilename $ExpectedRawEvidenceFilename `
        -SchemaVersion 5 `
        -CollectorVersion '0.5.0' `
        -ExpectedComparison 'exact-raw-and-observation-status'
}

function Assert-RawEvidenceV6 {
    param(
        [Parameter(Mandatory)] $Evidence,
        [Parameter(Mandatory)] [string] $ExpectedTargetManifestSha256,
        [Parameter(Mandatory)] [string] $ExpectedRawEvidenceFilename
    )
    Assert-RawEvidenceV5OrV6 `
        -Evidence $Evidence `
        -ExpectedTargetManifestSha256 $ExpectedTargetManifestSha256 `
        -ExpectedRawEvidenceFilename $ExpectedRawEvidenceFilename `
        -SchemaVersion 6 `
        -CollectorVersion '0.6.0' `
        -ExpectedComparison 'exact-raw-and-observation-status-except-thread-scoped-smm-base' `
        -ExcludeThreadScopedSmmBase
}

function Assert-RawEvidenceV4 {
    param(
        [Parameter(Mandatory)] $Evidence,
        [Parameter(Mandatory)] [string] $ExpectedTargetManifestSha256,
        [Parameter(Mandatory)] [string] $ExpectedRawEvidenceFilename
    )

    Assert-ExactProperties $Evidence @(
        'schema_version', 'evidence_kind', 'qualification_status', 'launch_authorized',
        'physical_candidate_flash_authorized', 'control_state_writes_authorized',
        'process_introspection_authorized', 'confidential_vm_claim',
        'amd_iommu_ownership_claim', 'pci_isolation_claim', 'collector',
        'target_profile_manifest_sha256', 'collected_at', 'sink', 'uefi', 'cpu',
        'vm_cr', 'processor_consistency', 'mp_services', 'memory_map', 'acpi',
        'amd_iommu_live', 'uncollected_blockers'
    ) 'raw evidence'
    Assert-JsonInteger $Evidence.schema_version 'schema_version' 4 4
    Assert-Condition (Test-OrdinalEqual $Evidence.evidence_kind 'uefi-record-only-inventory-slice') 'Unexpected v4 evidence_kind.'
    Assert-ExactProperties $Evidence.collector @('name', 'version', 'slice') 'collector'
    Assert-Condition (Test-OrdinalEqual $Evidence.collector.name 'svmvisor-m0b-probe') 'Unexpected collector name.'
    Assert-Condition (Test-OrdinalEqual $Evidence.collector.version '0.4.0') 'Unexpected v4 collector version.'
    Assert-Condition (Test-OrdinalEqual $Evidence.collector.slice 'ivrs-derived-read-only-amd-iommu-live-state') 'Unexpected v4 collector slice.'

    $v4Blockers = @(
        'amd-iommu-dte-requester-ownership-and-pci-isolation',
        'smm-lock-and-ppr-specific-msrs',
        'inherited-memory-encryption-state',
        'mtrrs-iorrs-tom-tom2-and-mmio-apertures',
        'secure-boot-databases-option-rom-policy-and-tcg-log',
        'boot-driver-sysprep-recovery-and-hotkey-namespace',
        'ready-to-boot-after-ready-to-boot-exit-boot-services-order',
        'direct-watchdog-and-durable-attempt-lease',
        'mp-services-ap-dispatch-pre-measurement-control-state-preservation'
    )
    Assert-JsonArray $Evidence.uncollected_blockers 'uncollected_blockers'
    Assert-Condition (@($Evidence.uncollected_blockers).Count -eq $v4Blockers.Count) 'Unexpected v4 uncollected-blocker count.'
    for ($index = 0; $index -lt $v4Blockers.Count; $index++) {
        Assert-Condition (Test-OrdinalEqual $Evidence.uncollected_blockers[$index] $v4Blockers[$index]) 'Uncollected blockers do not match schema v4.'
    }

    # Schema v4 is deliberately cumulative. Project every pre-IOMMU field to
    # the exact v3 contract first so later checks cannot weaken earlier gates.
    $v3Projection = [pscustomobject] [ordered] @{
        schema_version = 3
        evidence_kind = $Evidence.evidence_kind
        qualification_status = $Evidence.qualification_status
        launch_authorized = $Evidence.launch_authorized
        physical_candidate_flash_authorized = $Evidence.physical_candidate_flash_authorized
        control_state_writes_authorized = $Evidence.control_state_writes_authorized
        process_introspection_authorized = $Evidence.process_introspection_authorized
        confidential_vm_claim = $Evidence.confidential_vm_claim
        amd_iommu_ownership_claim = $Evidence.amd_iommu_ownership_claim
        pci_isolation_claim = $Evidence.pci_isolation_claim
        collector = [pscustomobject] [ordered] @{
            name = 'svmvisor-m0b-probe'
            version = '0.3.0'
            slice = 'bounded-per-processor-cpuid-vm-cr-consistency'
        }
        target_profile_manifest_sha256 = $Evidence.target_profile_manifest_sha256
        collected_at = $Evidence.collected_at
        sink = $Evidence.sink
        uefi = $Evidence.uefi
        cpu = $Evidence.cpu
        vm_cr = $Evidence.vm_cr
        processor_consistency = $Evidence.processor_consistency
        mp_services = $Evidence.mp_services
        memory_map = $Evidence.memory_map
        acpi = $Evidence.acpi
        uncollected_blockers = @(
            'amd-iommu-ivrs-register-ownership-and-pci-isolation',
            'smm-lock-and-ppr-specific-msrs',
            'inherited-memory-encryption-state',
            'mtrrs-iorrs-tom-tom2-and-mmio-apertures',
            'secure-boot-databases-option-rom-policy-and-tcg-log',
            'boot-driver-sysprep-recovery-and-hotkey-namespace',
            'ready-to-boot-after-ready-to-boot-exit-boot-services-order',
            'direct-watchdog-and-durable-attempt-lease',
            'mp-services-ap-dispatch-pre-measurement-control-state-preservation'
        )
    }
    Assert-RawEvidenceV3 $v3Projection $ExpectedTargetManifestSha256 $ExpectedRawEvidenceFilename
    Assert-AmdIommuLiveV4 $Evidence.amd_iommu_live $Evidence
}

$bundleItem = Get-Item -Force -LiteralPath $BundleDirectory
Assert-Condition ([bool] $bundleItem.PSIsContainer) "Bundle path is not a directory: '$BundleDirectory'."
Assert-Condition `
    (($bundleItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -eq 0) `
    "Bundle directory must not be a reparse point: '$BundleDirectory'."
$bundle = $bundleItem.FullName
$bundleItems = @(Get-ChildItem -Force -LiteralPath $bundle)
foreach ($item in $bundleItems) {
    Assert-Condition (-not [bool] $item.PSIsContainer) "Bundle contains an unexpected subdirectory: '$($item.Name)'."
    Assert-Condition `
        (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -eq 0) `
        "Bundle contains a reparse point: '$($item.Name)'."
}
$hasV1Schema = @($bundleItems | Where-Object { $_.Name -ceq 'm0b-probe-v1.schema.json' }).Count -eq 1
$hasV2Schema = @($bundleItems | Where-Object { $_.Name -ceq 'm0b-probe-v2.schema.json' }).Count -eq 1
$hasV3Schema = @($bundleItems | Where-Object { $_.Name -ceq 'm0b-probe-v3.schema.json' }).Count -eq 1
$hasV4Schema = @($bundleItems | Where-Object { $_.Name -ceq 'm0b-probe-v4.schema.json' }).Count -eq 1
$hasV5Schema = @($bundleItems | Where-Object { $_.Name -ceq 'm0b-probe-v5.schema.json' }).Count -eq 1
$hasV6Schema = @($bundleItems | Where-Object { $_.Name -ceq 'm0b-probe-v6.schema.json' }).Count -eq 1
$recognizedSchemaCount = @(@($hasV1Schema, $hasV2Schema, $hasV3Schema, $hasV4Schema, $hasV5Schema, $hasV6Schema) | Where-Object { $_ }).Count
Assert-Condition ($recognizedSchemaCount -eq 1) 'Bundle must contain exactly one recognized M0b schema file.'
$bundleSchemaVersion = if ($hasV6Schema) { 6 } elseif ($hasV5Schema) { 5 } elseif ($hasV4Schema) { 4 } elseif ($hasV3Schema) { 3 } elseif ($hasV2Schema) { 2 } else { 1 }
$schemaName = "m0b-probe-v$bundleSchemaVersion.schema.json"
$expectedBundleFiles = @(
    'finalize-evidence.ps1',
    $schemaName,
    'manifest.json',
    'prepare-media.ps1',
    'raw-probe.json',
    'svmvisor-m0b-probe.efi',
    'target-profile-manifest.json',
    'verify-bundle.ps1'
)
$expectedManifestFiles = @(
    'finalize-evidence.ps1',
    $schemaName,
    'prepare-media.ps1',
    'raw-probe.json',
    'svmvisor-m0b-probe.efi',
    'target-profile-manifest.json',
    'verify-bundle.ps1'
)
$shapeMessage = "Bundle must contain exactly the eight schema-v$bundleSchemaVersion evidence files."
Assert-Condition ($bundleItems.Count -eq $expectedBundleFiles.Count) $shapeMessage
$expectedBundleSet = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
foreach ($name in $expectedBundleFiles) { [void] $expectedBundleSet.Add($name) }
foreach ($item in $bundleItems) {
    Assert-Condition ($expectedBundleSet.Contains($item.Name)) "Unexpected bundle file: '$($item.Name)'."
}

$manifestPath = Join-Path $bundle 'manifest.json'
$rawPath = Join-Path $bundle 'raw-probe.json'
$targetManifestPath = Join-Path $bundle 'target-profile-manifest.json'
$schemaPath = Join-Path $bundle $schemaName
$rawItem = Get-Item -Force -LiteralPath $rawPath
Assert-Condition ($rawItem.Length -le 16777216) 'raw-probe.json exceeds the 16 MiB pre-parse cap.'
$manifestText = Get-Content -Raw -Encoding UTF8 -LiteralPath $manifestPath
$rawEvidenceText = Get-Content -Raw -Encoding UTF8 -LiteralPath $rawPath
$targetManifestText = Get-Content -Raw -Encoding UTF8 -LiteralPath $targetManifestPath
$schemaText = Get-Content -Raw -Encoding UTF8 -LiteralPath $schemaPath
Assert-NoDuplicateJsonPropertyNames $manifestText 'manifest.json'
Assert-NoDuplicateJsonPropertyNames $rawEvidenceText 'raw-probe.json'
Assert-NoDuplicateJsonPropertyNames $targetManifestText 'target-profile-manifest.json'
Assert-NoDuplicateJsonPropertyNames $schemaText $schemaName
$manifest = $manifestText | ConvertFrom-Json
$rawEvidence = $rawEvidenceText | ConvertFrom-Json
$targetManifest = $targetManifestText | ConvertFrom-Json
$schema = $schemaText | ConvertFrom-Json

Assert-ExactProperties $manifest @(
    'schema_version',
    'bundle_kind',
    'created_at_utc',
    'target_profile_manifest_sha256',
    'raw_evidence_filename',
    'files'
) 'manifest'
Assert-JsonInteger $manifest.schema_version 'manifest.schema_version' $bundleSchemaVersion $bundleSchemaVersion
Assert-JsonInteger $rawEvidence.schema_version 'schema_version' $bundleSchemaVersion $bundleSchemaVersion
Assert-Condition (Test-OrdinalEqual $manifest.bundle_kind 'svmvisor-m0b-probe-evidence') 'Unexpected manifest bundle_kind.'
Assert-JsonString $manifest.created_at_utc 'manifest.created_at_utc' '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}\.[0-9]{7}Z$'
Assert-LowerSha256 $manifest.target_profile_manifest_sha256 'manifest.target_profile_manifest_sha256'
Assert-JsonString $manifest.raw_evidence_filename 'manifest.raw_evidence_filename' '^svmvisor-m0b-[0-9]{8}T[0-9]{6}-[0-9]{9}\.json$'
Assert-JsonArray $manifest.files 'manifest.files'

$manifestPathSet = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
foreach ($file in @($manifest.files)) {
    Assert-ExactProperties $file @('path', 'size_bytes', 'sha256') 'manifest.files[]'
    Assert-JsonString $file.path 'manifest.files[].path'
    Assert-Condition ($file.path -ceq [System.IO.Path]::GetFileName($file.path)) "Manifest path is not a bundle-local basename: '$($file.path)'."
    Assert-Condition ($manifestPathSet.Add([string] $file.path)) "Duplicate manifest path: '$($file.path)'."
    Assert-JsonInteger $file.size_bytes "manifest size for '$($file.path)'" 0 ([decimal]::MaxValue)
    Assert-LowerSha256 $file.sha256 "manifest hash for '$($file.path)'"
}
Assert-Condition ($manifest.files.Count -eq $expectedManifestFiles.Count) "Manifest file set does not match schema v$bundleSchemaVersion."
$expectedManifestSet = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
foreach ($name in $expectedManifestFiles) { [void] $expectedManifestSet.Add($name) }
foreach ($name in $manifestPathSet) {
    Assert-Condition ($expectedManifestSet.Contains($name)) "Unexpected manifest path: '$name'."
}
foreach ($name in $expectedManifestFiles) {
    Assert-Condition ($manifestPathSet.Contains($name)) "Manifest path is missing: '$name'."
}

foreach ($file in @($manifest.files)) {
    $path = Join-Path $bundle $file.path
    $item = Get-Item -Force -LiteralPath $path
    Assert-Condition (-not [bool] $item.PSIsContainer) "Manifest entry is not a file: '$($file.path)'."
    Assert-Condition (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -eq 0) "Manifest entry is a reparse point: '$($file.path)'."
    Assert-Condition ([decimal] $file.size_bytes -eq [decimal] $item.Length) "Manifest size mismatch: '$($file.path)'."
    $actualHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash.ToLowerInvariant()
    Assert-Condition (Test-OrdinalEqual $actualHash $file.sha256) "Manifest hash mismatch: '$($file.path)'."
}

$targetManifestHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $targetManifestPath).Hash.ToLowerInvariant()
Assert-Condition `
    (Test-OrdinalEqual $targetManifestHash $manifest.target_profile_manifest_sha256) `
    'Copied target-profile manifest hash does not match the M0b manifest binding.'

Assert-ExactProperties $targetManifest @('schema_version', 'run_id', 'created_at_utc', 'files') 'target-profile-manifest'
Assert-JsonInteger $targetManifest.schema_version 'target-profile-manifest.schema_version' 1 1
Assert-JsonString $targetManifest.run_id 'target-profile-manifest.run_id' '^[0-9]{8}T[0-9]{9}Z$'
Assert-JsonString $targetManifest.created_at_utc 'target-profile-manifest.created_at_utc'
Assert-JsonArray $targetManifest.files 'target-profile-manifest.files'
$expectedM0aFiles = @('collect-windows.ps1', 'target-profile-v1.schema.json', 'target-profile.json', 'verify-bundle.ps1')
$m0aPathSet = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
foreach ($file in @($targetManifest.files)) {
    Assert-ExactProperties $file @('path', 'size_bytes', 'sha256') 'target-profile-manifest.files[]'
    Assert-JsonString $file.path 'target-profile-manifest.files[].path'
    Assert-Condition ($file.path -ceq [System.IO.Path]::GetFileName($file.path)) "Target-profile manifest path is not local: '$($file.path)'."
    Assert-Condition ($m0aPathSet.Add([string] $file.path)) "Duplicate target-profile manifest path: '$($file.path)'."
    Assert-JsonInteger $file.size_bytes "target-profile manifest size for '$($file.path)'" 0 ([decimal]::MaxValue)
    Assert-LowerSha256 $file.sha256 "target-profile manifest hash for '$($file.path)'"
}
Assert-Condition ($targetManifest.files.Count -eq $expectedM0aFiles.Count) 'Copied target-profile manifest has the wrong file count.'
foreach ($name in $expectedM0aFiles) {
    Assert-Condition ($m0aPathSet.Contains($name)) "Copied target-profile manifest is missing '$name'."
}

Assert-Condition (Test-OrdinalEqual $schema.'$schema' 'https://json-schema.org/draft/2020-12/schema') 'Copied M0b schema is not Draft 2020-12.'
Assert-Condition ($schema.additionalProperties -is [bool] -and $schema.additionalProperties -eq $false) 'Copied M0b schema is not fail-closed at its root.'
if ($bundleSchemaVersion -eq 2) {
    Assert-Condition (Test-OrdinalEqual $schema.'$id' 'https://svmvisor.invalid/schema/m0b-probe-v2.schema.json') 'Copied M0b v2 schema has an unexpected identifier.'
}
if ($bundleSchemaVersion -eq 3) {
    Assert-Condition (Test-OrdinalEqual $schema.'$id' 'https://svmvisor.invalid/schema/m0b-probe-v3.schema.json') 'Copied M0b v3 schema has an unexpected identifier.'
}
if ($bundleSchemaVersion -eq 4) {
    Assert-Condition (Test-OrdinalEqual $schema.'$id' 'https://svmvisor.invalid/schema/m0b-probe-v4.schema.json') 'Copied M0b v4 schema has an unexpected identifier.'
}
if ($bundleSchemaVersion -eq 5) {
    Assert-Condition (Test-OrdinalEqual $schema.'$id' 'https://svmvisor.invalid/schema/m0b-probe-v5.schema.json') 'Copied M0b v5 schema has an unexpected identifier.'
}
if ($bundleSchemaVersion -eq 6) {
    Assert-Condition (Test-OrdinalEqual $schema.'$id' 'https://svmvisor.invalid/schema/m0b-probe-v6.schema.json') 'Copied M0b v6 schema has an unexpected identifier.'
}

if ($bundleSchemaVersion -eq 1) {
    Assert-RawEvidenceV1 `
        -Evidence $rawEvidence `
        -ExpectedTargetManifestSha256 $targetManifestHash `
        -ExpectedRawEvidenceFilename $manifest.raw_evidence_filename
}
elseif ($bundleSchemaVersion -eq 2) {
    Assert-RawEvidenceV2 `
        -Evidence $rawEvidence `
        -ExpectedTargetManifestSha256 $targetManifestHash `
        -ExpectedRawEvidenceFilename $manifest.raw_evidence_filename
}
elseif ($bundleSchemaVersion -eq 3) {
    Assert-RawEvidenceV3 `
        -Evidence $rawEvidence `
        -ExpectedTargetManifestSha256 $targetManifestHash `
        -ExpectedRawEvidenceFilename $manifest.raw_evidence_filename
}
elseif ($bundleSchemaVersion -eq 4) {
    Assert-RawEvidenceV4 `
        -Evidence $rawEvidence `
        -ExpectedTargetManifestSha256 $targetManifestHash `
        -ExpectedRawEvidenceFilename $manifest.raw_evidence_filename
}
elseif ($bundleSchemaVersion -eq 5) {
    Assert-RawEvidenceV5 `
        -Evidence $rawEvidence `
        -ExpectedTargetManifestSha256 $targetManifestHash `
        -ExpectedRawEvidenceFilename $manifest.raw_evidence_filename
    throw 'Schema v5 is withdrawn: its cross-processor comparison included thread-scoped SMM_BASE and its TMTypeDram decode was incorrect; preserve the raw record as diagnostic evidence and recapture with schema v6.'
}
elseif ($bundleSchemaVersion -eq 6) {
    Assert-RawEvidenceV6 `
        -Evidence $rawEvidence `
        -ExpectedTargetManifestSha256 $targetManifestHash `
        -ExpectedRawEvidenceFilename $manifest.raw_evidence_filename
}
else {
    throw "Unrecognized M0b schema version $bundleSchemaVersion; refusing to verify."
}

Write-Host "M0b evidence bundle verification: PASS ($bundle)"
