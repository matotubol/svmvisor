Set-StrictMode -Version Latest

function ConvertTo-CardReturningAction([string]$Action) {
    switch ($Action) {
        'CheckOnly' { return 'CheckOnly' }
        'Program' { return 'Program' }
        'Restore' { return 'Restore' }
        default { throw 'Unknown returning action.' }
    }
}

function Assert-CardReturningAction([string]$Action, [bool]$Confirmed, [string]$RestoreSession) {
    if ($Action -notin @('CheckOnly','Program','Restore')) { throw 'Unknown returning action.' }
    if ($Action -ne 'CheckOnly' -and -not $Confirmed) { throw "$Action requires explicit -ConfirmFlash." }
    if ($RestoreSession -and $RestoreSession -cnotmatch '^[0-9a-f]{32}$') { throw 'RestoreSession must be a lowercase 32-hex session ID, never a path.' }
    if ($Action -eq 'Restore' -and -not $RestoreSession) { throw 'Restore requires a prior returning Program session ID.' }
    if ($Action -eq 'Program' -and $RestoreSession) { throw 'Program cannot select a restore session.' }
}

function Assert-CardReturningPin([string]$Sha256) {
    if ($Sha256 -cnotmatch '^[0-9a-f]{64}$' -or $Sha256 -eq ('0' * 64)) { throw 'Required exact returning pin is unpopulated or malformed; review is incomplete.' }
}

function Assert-CardReturningPath([string]$Path, [string]$Within) {
    $full=[IO.Path]::GetFullPath($Path)
    if ($Within) {
        $base=[IO.Path]::GetFullPath($Within).TrimEnd('\','/') + [IO.Path]::DirectorySeparatorChar
        if (-not $full.StartsWith($base,[StringComparison]::OrdinalIgnoreCase)) { throw "Path escapes constrained directory: $Path" }
    }
    # Reject junctions/symlinks at every existing component, including ancestors.
    $cursor=$full
    while ($cursor) {
        if (Test-Path -LiteralPath $cursor) {
            $item=Get-Item -LiteralPath $cursor -Force
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw "Reparse point refused: $cursor" }
        }
        $parent=[IO.Path]::GetDirectoryName($cursor)
        if ($parent -eq $cursor) { break }
        $cursor=$parent
    }
    return $full
}

function Assert-CardReturningFile([string]$Path, [long]$Bytes, [string]$Sha256) {
    Assert-CardReturningPin $Sha256
    $null=Assert-CardReturningPath $Path ''
    $item=Get-Item -LiteralPath $Path -Force -ErrorAction Stop
    if ($item.PSIsContainer -or ($Bytes -ge 0 -and $item.Length -ne $Bytes)) { throw "Invalid file extent: $Path" }
    if ((Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() -cne $Sha256) { throw "Digest mismatch: $Path" }
}

function Get-CardReturningHash([string]$Path) { return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }

function Assert-CardReturningBackups([string]$First, [string]$Second, [string]$ExpectedHash) {
    foreach ($path in @($First,$Second)) {
        $null=Assert-CardReturningPath $path ''
        $item=Get-Item -LiteralPath $path -ErrorAction Stop
        if ($item.PSIsContainer -or $item.Length -ne 5242880) { throw "Backup must contain exactly 5 MiB: $path" }
    }
    $hash=Get-CardReturningHash $First
    Assert-CardReturningFile $Second 5242880 $hash
    if ($ExpectedHash) {
        Assert-CardReturningPin $ExpectedHash
        if ($hash -cne $ExpectedHash) { throw 'Fresh backup is not the pinned currently installed known-working image; refusing before erase.' }
    }
    return $hash
}

function ConvertTo-CardReturningTclPath([string]$Path) {
    if ($Path -match '[{}"\r\n]') { throw 'Unsupported Tcl path characters.' }
    return $Path.Replace('\','/')
}

function Assert-CardReturningFields($Object, [hashtable]$Expected, [string]$Name) {
    foreach ($field in $Expected.Keys) {
        $property=$Object.PSObject.Properties[$field]
        if ($null -eq $property) { throw "$Name field mismatch: $field" }
        if ($null -eq $property.Value) { throw "$Name field mismatch: $field" }
        $numericTypes=($property.Value -is [ValueType] -and $Expected[$field] -is [ValueType] -and
                       $property.Value -isnot [bool] -and $Expected[$field] -isnot [bool])
        if (($property.Value.GetType() -ne $Expected[$field].GetType() -and -not $numericTypes) -or
            $property.Value -cne $Expected[$field]) { throw "$Name field mismatch: $field" }
    }
}

function Assert-CardReturningLayout([string]$Directory, [hashtable]$Hashes) {
    $combined=[IO.File]::ReadAllBytes((Join-Path $Directory 'combined.bin'))
    $configuration=[IO.File]::ReadAllBytes((Join-Path $Directory 'configuration.bin'))
    $slot=[IO.File]::ReadAllBytes((Join-Path $Directory 'payload-slot.bin'))
    $child=[IO.File]::ReadAllBytes((Join-Path $Directory 'native-child.efi'))
    $header=[IO.File]::ReadAllBytes((Join-Path $Directory 'pe-header.bin'))
    if ($combined.Length -ne 5242880 -or $configuration.Length -le 0 -or $configuration.Length -gt 4194304 -or
        $slot.Length -ne 1048576 -or $child.Length -ne 59904 -or $header.Length -ne 128) { throw 'Returning layout has wrong extent.' }
    $sha=[Security.Cryptography.SHA256]::Create()
    try {
        function Slice-Hash([byte[]]$Data,[int]$Offset,[int]$Count) {
            return [Convert]::ToHexString($sha.ComputeHash($Data,$Offset,$Count)).ToLowerInvariant()
        }
        if ((Slice-Hash $combined 0 $configuration.Length) -cne $Hashes['configuration.bin'] -or
            (Slice-Hash $combined 4194304 1048576) -cne $Hashes['payload-slot.bin'] -or
            (Slice-Hash $slot 0 128) -cne $Hashes['pe-header.bin'] -or
            (Slice-Hash $slot 128 $child.Length) -cne $Hashes['native-child.efi']) { throw 'Returning layout component mismatch.' }
        foreach ($part in @(@($combined,$configuration.Length,(4194304-$configuration.Length)), @($slot,(128+$child.Length),(1048576-128-$child.Length)))) {
            $padding=[byte[]]::new($part[2]); [Array]::Fill($padding,[byte]255)
            if ((Slice-Hash $part[0] $part[1] $part[2]) -cne (Slice-Hash $padding 0 $padding.Length)) { throw 'Returning layout padding is not erased 0xff.' }
        }
    } finally { $sha.Dispose() }
    if ([Text.Encoding]::ASCII.GetString($header,0,8) -cne 'SVMPE001' -or
        [BitConverter]::ToUInt32($header,8) -ne 1 -or [BitConverter]::ToUInt32($header,12) -ne 128 -or
        [BitConverter]::ToUInt64($header,16) -ne 59904 -or [BitConverter]::ToUInt64($header,24) -ne 1048576 -or
        [BitConverter]::ToUInt64($header,32) -ne 128 -or [BitConverter]::ToUInt64($header,40) -ne 2 -or
        [Convert]::ToHexString($header,48,32).ToLowerInvariant() -cne $Hashes['native-child.efi']) { throw 'Returning PE header contract mismatch.' }
}

function Assert-CardReturningRestoreSource([string]$SessionsRoot, [string]$Id, [string]$Candidate, [string]$ImageHash, [string]$KnownHash) {
    Assert-CardReturningAction 'CheckOnly' $false $Id
    if (-not $Id) { throw 'Missing restore session ID.' }
    $prior=Assert-CardReturningPath (Join-Path $SessionsRoot $Id) $SessionsRoot
    foreach ($name in @('result.json','backup-provenance.json','backup.cfg.log','before-a.bin','before-b.bin')) {
        $null=Assert-CardReturningPath (Join-Path $prior $name) $prior
    }
    $result=Get-Content -Raw -LiteralPath (Join-Path $prior 'result.json') | ConvertFrom-Json
    Assert-CardReturningFields $result @{schema_version=1;procedure='card-returning-v1';session_id=$Id;action='Program';candidate=$Candidate;image_sha256=$ImageHash;activation_performed=$false;backup_complete=$true;backup_sha256=$KnownHash;backup_bytes=5242880;hardware_accessed=$true} 'Prior program result'
    # A process killed after backup admission or during write is recoverable too.
    if ($result.status -notin @('running','failed','verified_not_activated') -or
        $result.phase -notin @('backup_admitted','programming','readback_verification','complete')) { throw 'Prior Program did not reach recoverable backup admission.' }
    Assert-CardReturningFile (Join-Path $prior 'backup-provenance.json') -1 $result.backup_provenance_sha256
    $provenance=Get-Content -Raw -LiteralPath (Join-Path $prior 'backup-provenance.json') | ConvertFrom-Json
    Assert-CardReturningFields $provenance @{schema_version=1;procedure='card-returning-v1';session_id=$Id;action='Program';candidate=$Candidate;image_sha256=$ImageHash;bytes=5242880;first_file='before-a.bin';second_file='before-b.bin';first_sha256=$KnownHash;second_sha256=$KnownHash;known_working_match=$true;flash_verify_completed=$true;backup_config_sha256='fb376387fa53257dfcfe539a22fbeb0c8e12c0c403c6522b1cf21b65bbb059e2';transport_sha256='3a31fe2838566fc9653bd0306c7378da93c86beb2bde611f3038272b1c75e642'} 'Prior backup provenance'
    Assert-CardReturningFile (Join-Path $prior 'backup.cfg.log') -1 $provenance.log_sha256
    $log=Get-Content -Raw -LiteralPath (Join-Path $prior 'backup.cfg.log')
    if ($log -notmatch 'PASS card-double-backup' -or $log -notmatch 'PASS card-target-id-and-geometry') { throw 'Prior backup transcript is incomplete.' }
    $null=Assert-CardReturningBackups (Join-Path $prior 'before-a.bin') (Join-Path $prior 'before-b.bin') $KnownHash
    return $prior
}
