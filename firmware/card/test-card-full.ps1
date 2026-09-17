# Host-only checks of the full-image helpers (card-full.ps1) and the pin schema,
# plus offline refusals of flash-card.ps1's full path. Never accesses hardware,
# never passes -ConfirmFlash. Runs under Windows PowerShell 5.1 and PowerShell 7.
[CmdletBinding()]
param()
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'card-validation.ps1')
. (Join-Path $PSScriptRoot 'card-full.ps1')
$script:passed = 0
function Test-Case([string]$Name, [scriptblock]$Body) { & $Body; $script:passed++; Write-Host "PASS: $Name" }
function Assert-Equal($Actual, $Expected, [string]$What) { if ($Actual -cne $Expected) { throw "FAIL: $What (expected '$Expected', got '$Actual')" } }
function Assert-Refused([scriptblock]$Body, [string]$Pattern, [string]$What) {
    $message = $null
    try { $null = & $Body } catch { $message = $_.Exception.Message }
    if ($null -eq $message) { throw "FAIL: $What was accepted" }
    if ($message -notmatch $Pattern) { throw "FAIL: $What refused for the wrong reason: $message" }
}
function Sha([byte[]]$Data) { $s=[Security.Cryptography.SHA256]::Create(); try { return (ConvertTo-CardHex $s.ComputeHash($Data)) } finally { $s.Dispose() } }

Test-Case 'ConvertTo-CardHex is lowercase and matches Get-FileHash' {
    Assert-Equal (ConvertTo-CardHex ([byte[]]@(0,1,255,171))) '0001ffab' 'hex'
    Assert-Equal (ConvertTo-CardHex ([byte[]]@())) '' 'empty'
    $tmp = Join-Path $env:TEMP ('cardhex-'+[guid]::NewGuid().ToString('N')+'.bin')
    try { [IO.File]::WriteAllBytes($tmp, [byte[]](1..64)); Assert-Equal (Sha ([byte[]](1..64))) ((Get-FileHash -LiteralPath $tmp -Algorithm SHA256).Hash.ToLowerInvariant()) 'sha' }
    finally { Remove-Item -LiteralPath $tmp -Force }
}
Test-Case 'New-CardErasedBytes fills 0xff at several sizes' {
    foreach ($n in @(0,1,2,3,65536,1048577)) {
        $b = New-CardErasedBytes $n
        Assert-Equal $b.Length $n "length $n"
        if ($n -gt 0) { Assert-Equal (@($b | Where-Object { $_ -ne 255 }).Count) 0 "all 0xff $n" }
    }
}
Test-Case 'Get-CardFeatureForMode maps modes and refuses others' {
    Assert-Equal (Get-CardFeatureForMode 'pinned') 'card-resident-loader' 'pinned'
    Assert-Equal (Get-CardFeatureForMode 'dev') 'card-resident-dev-loader' 'dev'
    Assert-Refused { Get-CardFeatureForMode 'other' } "must be 'pinned' or 'dev'" 'bad mode'
}

# A synthetic self-consistent 5 MiB candidate directory for layout tests.
function New-Candidate([int]$ChildBytes, [int]$ConfigBytes) {
    $dir = Join-Path $env:TEMP ('cardlayout-'+[guid]::NewGuid().ToString('N')); New-Item -ItemType Directory -Path $dir | Out-Null
    $child = New-Object byte[] $ChildBytes; for ($i=0;$i -lt $ChildBytes;$i++){ $child[$i]=[byte](($i*13+7) -band 0xfe) }
    $header = New-Object byte[] 128
    [Text.Encoding]::ASCII.GetBytes('SVMBPE01').CopyTo($header,0)
    [BitConverter]::GetBytes([uint32]1).CopyTo($header,8); [BitConverter]::GetBytes([uint32]128).CopyTo($header,12)
    [BitConverter]::GetBytes([uint64]$ChildBytes).CopyTo($header,16); [BitConverter]::GetBytes([uint64]1048576).CopyTo($header,24)
    [BitConverter]::GetBytes([uint64]128).CopyTo($header,32); [BitConverter]::GetBytes([uint64]4).CopyTo($header,40)
    $sha=[Security.Cryptography.SHA256]::Create(); try { $sha.ComputeHash($child).CopyTo($header,48) } finally { $sha.Dispose() }
    $slot = New-CardErasedBytes 1048576; $header.CopyTo($slot,0); $child.CopyTo($slot,128)
    $config = New-Object byte[] $ConfigBytes; for ($i=0;$i -lt $ConfigBytes;$i++){ $config[$i]=[byte](($i*7+1) -band 0xfe) }
    $combined = New-CardErasedBytes 5242880; $config.CopyTo($combined,0); $slot.CopyTo($combined,4194304)
    [IO.File]::WriteAllBytes((Join-Path $dir 'combined.bin'),$combined)
    [IO.File]::WriteAllBytes((Join-Path $dir 'configuration.bin'),$config)
    [IO.File]::WriteAllBytes((Join-Path $dir 'payload-slot.bin'),$slot)
    [IO.File]::WriteAllBytes((Join-Path $dir 'native-child.efi'),$child)
    [IO.File]::WriteAllBytes((Join-Path $dir 'pe-header.bin'),$header)
    $hashes = @{ 'configuration.bin'=(Sha $config); 'payload-slot.bin'=(Sha $slot); 'pe-header.bin'=(Sha $header); 'native-child.efi'=(Sha $child) }
    return @{ Dir=$dir; Hashes=$hashes; ChildBytes=$ChildBytes; ConfigBytes=$ConfigBytes }
}
function Patch([string]$Dir,[string]$File,[int]$Offset,[byte]$Value) {
    $path = Join-Path $Dir $File; $b = [IO.File]::ReadAllBytes($path); $b[$Offset]=$Value; [IO.File]::WriteAllBytes($path,$b)
}

Test-Case 'layout self-consistency accepts a valid candidate and is driven by ChildBytes' {
    $c = New-Candidate 263168 1502380
    try {
        Assert-CardResidentLayout $c.Dir $c.Hashes $c.ChildBytes
        Assert-Refused { Assert-CardResidentLayout $c.Dir $c.Hashes 153088 } 'wrong extent' 'wrong ChildBytes'
    } finally { Remove-Item -LiteralPath $c.Dir -Recurse -Force }
}
Test-Case 'layout refuses each corruption class' {
    $c = New-Candidate 4096 65536
    try {
        Patch $c.Dir 'combined.bin' 10 0     ; Assert-Refused { Assert-CardResidentLayout $c.Dir $c.Hashes $c.ChildBytes } 'component mismatch' 'configuration byte'
        $c = New-Candidate 4096 65536; Patch $c.Dir 'combined.bin' 4194304 0 ; Assert-Refused { Assert-CardResidentLayout $c.Dir $c.Hashes $c.ChildBytes } 'component mismatch' 'slot header byte in combined'
        $c = New-Candidate 4096 65536; Patch $c.Dir 'payload-slot.bin' 900000 0 ; Assert-Refused { Assert-CardResidentLayout $c.Dir $c.Hashes $c.ChildBytes } 'padding is not erased' 'slot tail not 0xff'
        $c = New-Candidate 4096 65536; Patch $c.Dir 'combined.bin' 5000000 0 ; Assert-Refused { Assert-CardResidentLayout $c.Dir $c.Hashes $c.ChildBytes } 'component mismatch' 'combined slot region'
        # Corrupt the flags field consistently in header, slot and combined so the
        # component hashes still match and only the header-field check can fire.
        $c = New-Candidate 4096 65536; Patch $c.Dir 'pe-header.bin' 40 2 ; Patch $c.Dir 'payload-slot.bin' 40 2 ; Patch $c.Dir 'combined.bin' (4194304+40) 2 ; $c.Hashes['pe-header.bin']=(Sha ([IO.File]::ReadAllBytes((Join-Path $c.Dir 'pe-header.bin')))); $c.Hashes['payload-slot.bin']=(Sha ([IO.File]::ReadAllBytes((Join-Path $c.Dir 'payload-slot.bin')))); Assert-Refused { Assert-CardResidentLayout $c.Dir $c.Hashes $c.ChildBytes } 'header contract mismatch' 'header flags field'
    } finally { Remove-Item -LiteralPath $c.Dir -Recurse -Force }
}

# candidate-pin schema
function New-PinTable {
    $inputs = @()
    foreach ($n in @('combined.bin','configuration.bin','payload-slot.bin','pe-header.bin','native-child.efi','candidate-manifest.json','payload-manifest.json')) {
        $inputs += @{ Name=$n; Path=('target/firmware/card/x/'+$n); Bytes=[long]16; Hash=('a'*64) }
    }
    return @{ Candidate='target/firmware/card/x'; ChildBytes=[long]263168; LoaderMode='dev'; ResidentBuild='target/card-dev/x/resident'; KnownWorking=('b'*64); Inputs=$inputs }
}
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
Test-Case 'pin schema accepts a well-formed dev/pinned pin' {
    Assert-CardCandidatePin (New-PinTable) $root
    $p = New-PinTable; $p.LoaderMode='pinned'; Assert-CardCandidatePin $p $root
}
Test-Case 'pin schema refuses malformed pins' {
    $p = New-PinTable; $p.Remove('ChildBytes'); Assert-Refused { Assert-CardCandidatePin $p $root } 'unexpected key set' 'missing ChildBytes'
    $p = New-PinTable; $p.LoaderMode='prod'; Assert-Refused { Assert-CardCandidatePin $p $root } "must be 'pinned' or 'dev'" 'bad LoaderMode'
    $p = New-PinTable; $p.ChildBytes=[long]511; Assert-Refused { Assert-CardCandidatePin $p $root } 'ChildBytes must be an integer' 'child too small'
    $p = New-PinTable; $p.ChildBytes=[long](1048576-127); Assert-Refused { Assert-CardCandidatePin $p $root } 'ChildBytes must be an integer' 'child too big'
    $p = New-PinTable; $p.ChildBytes='263168'; Assert-Refused { Assert-CardCandidatePin $p $root } 'ChildBytes must be an integer' 'string child'
    $p = New-PinTable; $p.KnownWorking=('0'*64); Assert-Refused { Assert-CardCandidatePin $p $root } 'unpopulated or malformed' 'zero KnownWorking'
    $p = New-PinTable; $p.Inputs = @($p.Inputs | Where-Object { $_.Name -ne 'pe-header.bin' }); Assert-Refused { Assert-CardCandidatePin $p $root } 'missing the required entry: pe-header.bin' 'missing required input'
    $p = New-PinTable; $p.Inputs[0].Path='../../../etc/passwd'; Assert-Refused { Assert-CardCandidatePin $p $root } 'escapes constrained directory' 'path escape'
}

Test-Case 'the committed candidate-pin.psd1 validates and names a dev candidate' {
    $pin = Import-PowerShellDataFile -LiteralPath (Join-Path $PSScriptRoot 'candidate-pin.psd1')
    Assert-CardCandidatePin $pin $root
    Assert-Equal $pin.LoaderMode 'dev' 'committed LoaderMode'
    Assert-Equal (Get-CardFeatureForMode $pin.LoaderMode) 'card-resident-dev-loader' 'committed feature'
}

Test-Case 'flash-card full path refuses offline misuse without -ConfirmFlash' {
    $flash = Join-Path $PSScriptRoot 'flash-card.ps1'
    Assert-Refused { & $flash -Action Backup } 'requires explicit -ConfirmFlash' 'Backup unconfirmed'
    Assert-Refused { & $flash -Action Program } 'requires explicit -ConfirmFlash' 'Program unconfirmed'
    Assert-Refused { & $flash -Action Backup -ConfirmFlash -BuildPath x } 'apply only to the payload-slot actions' 'Backup with BuildPath'
    Assert-Refused { & $flash -Action Backup -ConfirmFlash -RestoreSession ('a'*32) } 'takes no restore session' 'Backup with session'
}
Test-Case 'Backup never stages an erase/write cfg (structural)' {
    $text = Get-Content -Raw -LiteralPath (Join-Path $PSScriptRoot 'flash-card.ps1')
    if ($text -notmatch [regex]::Escape("`$backupInputNames=@('openocd.exe','proxy.bit','transport.cfg','backup.cfg','validation.ps1','full.ps1')")) { throw 'FAIL: Backup input set changed; re-check that program.cfg/restore.cfg are excluded.' }
    if ($text -match "Invoke-CardStage 'program.cfg'.*isBackup" -or $text -match "isBackup.*program\.cfg") { throw 'FAIL: program.cfg reachable from Backup.' }
}
Write-Host "$script:passed full-image test groups passed."
