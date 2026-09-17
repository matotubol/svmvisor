# Full-image candidate helpers for flash-card.ps1 (dot-sourced, hash-pinned there).
# Pure offline logic: candidate-pin.psd1 schema validation and the 5 MiB layout
# self-consistency check, both driven entirely by the pin. No hardware, and only
# APIs present in Windows PowerShell 5.1 as well as PowerShell 7 (no Array.Fill,
# Convert.ToHexString or 3-argument File.Move).
Set-StrictMode -Version Latest

# Lowercase hex of a byte array, PS 5.1-safe (Convert.ToHexString is Core-only).
function ConvertTo-CardHex([byte[]]$Bytes) {
    $builder=[Text.StringBuilder]::new($Bytes.Length*2)
    foreach ($b in $Bytes) { $null=$builder.Append($b.ToString('x2')) }
    return $builder.ToString()
}

# A byte[] of $Count 0xff, filled by native doubling Array.Copy (no per-element
# PowerShell loop, no Array.Fill) so multi-MiB padding checks stay fast on 5.1.
function New-CardErasedBytes([int]$Count) {
    $bytes=New-Object 'byte[]' $Count
    if ($Count -gt 0) {
        $bytes[0]=255; $filled=1
        while ($filled -lt $Count) {
            $n=[Math]::Min($filled,$Count-$filled)
            [Array]::Copy($bytes,0,$bytes,$filled,$n)
            $filled+=$n
        }
    }
    return ,$bytes
}

function Get-CardSliceHex([byte[]]$Data,[int]$Offset,[int]$Count) {
    if ($Offset -lt 0 -or $Count -lt 0 -or [long]$Offset+$Count -gt $Data.Length) { throw 'Slice escapes its buffer.' }
    $sha=[Security.Cryptography.SHA256]::Create()
    try { return ConvertTo-CardHex $sha.ComputeHash($Data,$Offset,$Count) } finally { $sha.Dispose() }
}

# The parent loader feature a candidate's LoaderMode implies. `dev` is the
# card-resident-dev-loader (adopts the slot header); `pinned` compiles it in.
function Get-CardFeatureForMode([string]$LoaderMode) {
    switch ($LoaderMode) {
        'dev'    { return 'card-resident-dev-loader' }
        'pinned' { return 'card-resident-loader' }
        default  { throw "candidate-pin.psd1 LoaderMode must be 'pinned' or 'dev', not '$LoaderMode'." }
    }
}

# Every candidate-specific value lives here and is strictly validated. Names that
# the full path requires are explicit; any extra Inputs entry is allowed but must
# still be well-formed. Existence of the referenced files is checked later, when
# they are staged, so this also validates a pin whose target/ was cleaned.
function Assert-CardCandidatePin($Pin, [string]$Root) {
    if ($null -eq $Pin -or $Pin -isnot [hashtable]) { throw 'candidate-pin.psd1 did not load as a data table.' }
    if ((@($Pin.Keys | Sort-Object) -join ',') -cne 'Candidate,ChildBytes,Inputs,KnownWorking,LoaderMode,ResidentBuild') { throw 'candidate-pin.psd1 has an unexpected key set.' }
    Assert-CardReturningPin $Pin.KnownWorking
    $null=Get-CardFeatureForMode $Pin.LoaderMode
    if (($Pin.ChildBytes -isnot [int] -and $Pin.ChildBytes -isnot [long]) -or [long]$Pin.ChildBytes -lt 512 -or [long]$Pin.ChildBytes -gt 1048576-128) { throw 'candidate-pin.psd1 ChildBytes must be an integer in 512..1048448.' }
    foreach ($relative in @($Pin.Candidate,$Pin.ResidentBuild)) {
        if ([string]::IsNullOrWhiteSpace($relative)) { throw 'candidate-pin.psd1 has an empty candidate path.' }
        $null=Assert-CardReturningPath (Join-Path $Root $relative) $Root
    }
    if ($Pin.Inputs -isnot [object[]] -or $Pin.Inputs.Count -lt 1) { throw 'candidate-pin.psd1 Inputs must be a non-empty list.' }
    $names=@()
    foreach ($entry in $Pin.Inputs) {
        if ($entry -isnot [hashtable]) { throw 'candidate-pin.psd1 input is not a data table.' }
        if ((@($entry.Keys | Sort-Object) -join ',') -cne 'Bytes,Hash,Name,Path') { throw "candidate-pin.psd1 input has an unexpected key set: $($entry.Name)" }
        if ([string]::IsNullOrWhiteSpace($entry.Name) -or [string]::IsNullOrWhiteSpace($entry.Path)) { throw 'candidate-pin.psd1 input has an empty Name or Path.' }
        Assert-CardReturningPin $entry.Hash
        if (($entry.Bytes -isnot [int] -and $entry.Bytes -isnot [long]) -or [long]$entry.Bytes -lt 0) { throw "candidate-pin.psd1 input has an invalid byte count: $($entry.Name)" }
        $null=Assert-CardReturningPath (Join-Path $Root $entry.Path) $Root
        $names+=$entry.Name
    }
    foreach ($required in @('combined.bin','configuration.bin','payload-slot.bin','pe-header.bin','native-child.efi','candidate-manifest.json','payload-manifest.json')) {
        if ($required -notin $names) { throw "candidate-pin.psd1 Inputs is missing the required entry: $required" }
    }
}

# The 5 MiB image is self-consistent: combined = configuration + 0xff pad + slot;
# slot = header + child + 0xff pad; and the header describes exactly this child.
# Component digests come from the pin ($Hashes); only ChildBytes and the pinned
# child digest are read for the header field comparison.
function Assert-CardResidentLayout([string]$Directory, [hashtable]$Hashes, [long]$ChildBytes) {
    $combined=[IO.File]::ReadAllBytes((Join-Path $Directory 'combined.bin'))
    $configuration=[IO.File]::ReadAllBytes((Join-Path $Directory 'configuration.bin'))
    $slot=[IO.File]::ReadAllBytes((Join-Path $Directory 'payload-slot.bin'))
    $child=[IO.File]::ReadAllBytes((Join-Path $Directory 'native-child.efi'))
    $header=[IO.File]::ReadAllBytes((Join-Path $Directory 'pe-header.bin'))
    if ($combined.Length -ne 5242880 -or $configuration.Length -le 0 -or $configuration.Length -gt 4194304 -or
        $slot.Length -ne 1048576 -or $child.Length -ne $ChildBytes -or $header.Length -ne 128) { throw 'Resident layout has wrong extent.' }
    if ((Get-CardSliceHex $combined 0 $configuration.Length) -cne $Hashes['configuration.bin'] -or
        (Get-CardSliceHex $combined 4194304 1048576) -cne $Hashes['payload-slot.bin'] -or
        (Get-CardSliceHex $slot 0 128) -cne $Hashes['pe-header.bin'] -or
        (Get-CardSliceHex $slot 128 $child.Length) -cne $Hashes['native-child.efi']) { throw 'Resident layout component mismatch.' }
    foreach ($part in @(@($combined,$configuration.Length,(4194304-$configuration.Length)), @($slot,(128+$child.Length),(1048576-128-$child.Length)))) {
        $erased=New-CardErasedBytes $part[2]
        if ((Get-CardSliceHex $part[0] $part[1] $part[2]) -cne (Get-CardSliceHex $erased 0 $erased.Length)) { throw 'Resident layout padding is not erased 0xff.' }
    }
    if ([Text.Encoding]::ASCII.GetString($header,0,8) -cne 'SVMBPE01' -or
        [BitConverter]::ToUInt32($header,8) -ne 1 -or [BitConverter]::ToUInt32($header,12) -ne 128 -or
        [BitConverter]::ToUInt64($header,16) -ne [uint64]$ChildBytes -or [BitConverter]::ToUInt64($header,24) -ne 1048576 -or
        [BitConverter]::ToUInt64($header,32) -ne 128 -or [BitConverter]::ToUInt64($header,40) -ne 4 -or
        (ConvertTo-CardHex $header[48..79]) -cne $Hashes['native-child.efi']) { throw 'Resident PE header contract mismatch.' }
}
