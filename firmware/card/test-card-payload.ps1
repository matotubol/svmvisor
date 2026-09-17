# Focused host-only checks of the payload-slot-only flashing logic. Never starts
# OpenOCD, never passes -ConfirmFlash, never accesses a device.
[CmdletBinding()]
param()
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'card-validation.ps1')
. (Join-Path $PSScriptRoot 'card-payload.ps1')
$script:passed = 0
function Test-Case([string]$Name, [scriptblock]$Body) {
    & $Body
    $script:passed++
    Write-Host "PASS: $Name"
}
function Assert-Equal($Actual, $Expected, [string]$What) {
    if ((@($Actual) -join ',') -cne (@($Expected) -join ',')) { throw "FAIL: $What (expected '$(@($Expected) -join ',')', got '$(@($Actual) -join ',')')" }
}
function Assert-Refused([scriptblock]$Body, [string]$Pattern, [string]$What) {
    $message = $null
    try { $null = & $Body } catch { $message = $_.Exception.Message }
    if ($null -eq $message) { throw "FAIL: $What was accepted" }
    if ($message -notmatch $Pattern) { throw "FAIL: $What refused for the wrong reason: $message" }
}
function New-Bytes([int]$Count, [byte]$Value) {
    $bytes = New-Object byte[] $Count
    if ($Value -ne 0) { for ($i = 0; $i -lt $Count; $i++) { $bytes[$i] = $Value } }
    return ,$bytes
}
# A slot image: header(128) + child of ChildBytes + 0xff. Seed varies the content.
function New-Slot([int]$ChildBytes, [byte]$Seed) {
    $slot = New-Bytes 1048576 255
    $child = New-Object byte[] $ChildBytes
    for ($i = 0; $i -lt $ChildBytes; $i++) { $child[$i] = [byte](($i * 7 + $Seed) -band 0xfe) }   # never 0xff
    $sha = [Security.Cryptography.SHA256]::Create()
    try { $digest = $sha.ComputeHash($child) } finally { $sha.Dispose() }
    $header = New-Object byte[] 128
    [Text.Encoding]::ASCII.GetBytes('SVMBPE01').CopyTo($header, 0)
    [BitConverter]::GetBytes([uint32]1).CopyTo($header, 8); [BitConverter]::GetBytes([uint32]128).CopyTo($header, 12)
    [BitConverter]::GetBytes([uint64]$ChildBytes).CopyTo($header, 16); [BitConverter]::GetBytes([uint64]1048576).CopyTo($header, 24)
    [BitConverter]::GetBytes([uint64]128).CopyTo($header, 32); [BitConverter]::GetBytes([uint64]4).CopyTo($header, 40)
    $digest.CopyTo($header, 48)
    $header.CopyTo($slot, 0); $child.CopyTo($slot, 128)
    return @{Slot = $slot; Child = $child; Header = $header}
}
$erased = New-Bytes 1048576 255
$old300 = New-Slot (300 * 1024) 1        # 128 + 307200 bytes -> sectors 64..68
$new150 = New-Slot 153088 2              # 128 + 153088 bytes -> sectors 64..66

Test-Case 'first payload into an erased slot: write covering sectors, erase nothing' {
    $plan = Get-CardPayloadPlan $erased $new150.Slot
    Assert-Equal $plan.Touch @(64, 65, 66) 'touch'; Assert-Equal $plan.Erase @() 'erase'; Assert-Equal $plan.Write @(64, 65, 66) 'write'
}
Test-Case 'shrinking payload (150 KiB replaces 300 KiB): stale sectors 67,68 are erased, not written' {
    $plan = Get-CardPayloadPlan $old300.Slot $new150.Slot
    Assert-Equal $plan.Touch @(64, 65, 66, 67, 68) 'touch'; Assert-Equal $plan.Erase @(64, 65, 66, 67, 68) 'erase'; Assert-Equal $plan.Write @(64, 65, 66) 'write'
    Assert-Equal (Get-CardPayloadCoveringSectors (128 + 153088)) @(64, 65, 66) 'covering'
}
Test-Case 'growing payload: previously erased sectors are written without an erase' {
    $plan = Get-CardPayloadPlan $new150.Slot $old300.Slot
    Assert-Equal $plan.Erase @(64, 65, 66) 'erase'; Assert-Equal $plan.Write @(64, 65, 66, 67, 68) 'write'
}
Test-Case 'stale bytes anywhere beyond the new extent are found, including the last sector' {
    $dirty = [byte[]]$old300.Slot.Clone(); $dirty[1048575] = 0; $dirty[10 * 65536 + 5] = 0x7f
    $plan = Get-CardPayloadPlan $dirty $new150.Slot
    Assert-Equal $plan.Erase @(64, 65, 66, 67, 68, 74, 79) 'erase'; Assert-Equal $plan.Write @(64, 65, 66) 'write'
}
Test-Case 'identical slot: nothing to do' {
    $plan = Get-CardPayloadPlan $new150.Slot ([byte[]]$new150.Slot.Clone())
    Assert-Equal $plan.Touch @() 'touch'
}
Test-Case 'same-size rebuild touches exactly the covering sectors' {
    $plan = Get-CardPayloadPlan (New-Slot 153088 9).Slot $new150.Slot
    Assert-Equal $plan.Touch @(64, 65, 66) 'touch'; Assert-Equal $plan.Erase @(64, 65, 66) 'erase'
}
Test-Case 'restore of an arbitrary prior slot (non-0xff tail allowed) rewrites only differing sectors' {
    $prior = [byte[]]$old300.Slot.Clone(); $prior[15 * 65536] = 1
    $plan = Get-CardPayloadPlan $new150.Slot $prior
    Assert-Equal $plan.Touch @(64, 65, 66, 67, 68, 79) 'touch'; Assert-Equal $plan.Erase @(64, 65, 66) 'erase'; Assert-Equal $plan.Write @(64, 65, 66, 67, 68, 79) 'write'
}
Test-Case 'plan needs two exact 1 MiB images' {
    Assert-Refused { Get-CardPayloadPlan (New-Bytes 1048575 255) $new150.Slot } 'exact 1 MiB' 'short current image'
    Assert-Refused { Get-CardPayloadPlan $erased (New-Bytes 1048577 255) } 'exact 1 MiB' 'long target image'
}
Test-Case 'sector range assertions: only ascending integers 64..79' {
    Assert-CardPayloadSectors @(64, 79) 'ok'; Assert-CardPayloadSectors @() 'empty'
    foreach ($bad in @(@(63), @(80), @(0), @(-1), @(64, 64), @(66, 65), @(1024))) { Assert-Refused { Assert-CardPayloadSectors $bad 'bad' } 'outside the payload slot|strictly ascending' "sectors $($bad -join ',')" }
    Assert-Refused { Assert-CardPayloadSectors @('64') 'bad' } 'non-integer' 'string sector'
    Assert-Refused { Assert-CardPayloadSectors @('64; flash erase_sector xc7.spi 0 79') 'bad' } 'non-integer' 'injected text'
    Assert-Refused { Assert-CardPayloadSectors @(64.5) 'bad' } 'non-integer' 'fractional sector'
    Assert-Refused { Get-CardPayloadCoveringSectors 1048577 } 'escapes the slot' 'extent beyond the slot'
    Assert-Refused { Get-CardPayloadCoveringSectors 0 } 'escapes the slot' 'empty extent'
}
Test-Case 'Tcl list serialization is digits and spaces only' {
    Assert-Equal (ConvertTo-CardPayloadTclList @(64, 65, 79) 'x') '64 65 79' 'list'
    Assert-Equal (ConvertTo-CardPayloadTclList @() 'x') '' 'empty list'
    Assert-Refused { ConvertTo-CardPayloadTclList @('64}; shutdown; #') 'x' } 'non-integer' 'Tcl injection'
    Assert-Refused { ConvertTo-CardPayloadTclList @(5) 'x' } 'outside the payload slot' 'configuration sector'
}
Test-Case 'adapter speed is a bounded integer' {
    Assert-Equal (Assert-CardPayloadAdapterKhz 1000) 1000 'default'; Assert-Equal (Assert-CardPayloadAdapterKhz 30000) 30000 'max'
    foreach ($bad in @(99, 30001, 0, -1000)) { Assert-Refused { Assert-CardPayloadAdapterKhz $bad } 'within 100\.\.30000' "speed $bad" }
    foreach ($bad in @('1000', '1000; shutdown', 1000.5)) { Assert-Refused { Assert-CardPayloadAdapterKhz $bad } 'must be an integer' "speed '$bad'" }
}
Test-Case 'action arguments' {
    Assert-CardPayloadAction 'CheckPayload' $false 'target/card-dev/x' ''
    Assert-CardPayloadAction 'ProgramPayload' $true 'target/card-dev/x' ''
    Assert-CardPayloadAction 'RestorePayload' $true '' ('a' * 32)
    Assert-Refused { Assert-CardPayloadAction 'ProgramPayload' $false 'x' '' } 'requires explicit -ConfirmFlash' 'unconfirmed program'
    Assert-Refused { Assert-CardPayloadAction 'RestorePayload' $false '' ('a' * 32) } 'requires explicit -ConfirmFlash' 'unconfirmed restore'
    Assert-Refused { Assert-CardPayloadAction 'ProgramPayload' $true '' '' } 'requires -BuildPath' 'program without build'
    Assert-Refused { Assert-CardPayloadAction 'CheckPayload' $false '' '' } 'requires -BuildPath' 'check without build'
    Assert-Refused { Assert-CardPayloadAction 'ProgramPayload' $true 'x' ('a' * 32) } 'cannot select a restore session' 'program with session'
    Assert-Refused { Assert-CardPayloadAction 'RestorePayload' $true '' '' } 'requires a prior payload session' 'restore without session'
    Assert-Refused { Assert-CardPayloadAction 'RestorePayload' $true 'x' ('a' * 32) } 'takes no -BuildPath' 'restore with build'
    Assert-Refused { Assert-CardPayloadAction 'RestorePayload' $true '' '..\..\x' } 'never a path' 'session path'
    Assert-Refused { Assert-CardPayloadAction 'Program' $true 'x' '' } 'Unknown payload action' 'full-image action'
}
Test-Case 'slot self-consistency: header + child + 0xff, header describes the child' {
    Assert-Equal (Assert-CardPayloadSlot $new150.Slot $new150.Child $new150.Header) (Get-CardPayloadSliceHash $new150.Child 0 153088) 'child hash'
    $flip = [byte[]]$new150.Slot.Clone(); $flip[128 + 4000] = $flip[128 + 4000] -bxor 1
    Assert-Refused { Assert-CardPayloadSlot $flip $new150.Child $new150.Header } 'component mismatch' 'payload bit flip'
    $tail = [byte[]]$new150.Slot.Clone(); $tail[900000] = 0
    Assert-Refused { Assert-CardPayloadSlot $tail $new150.Child $new150.Header } 'padding is not erased' 'stale tail'
    Assert-Refused { Assert-CardPayloadSlot $new150.Slot $old300.Child $new150.Header } 'component mismatch' 'other child'
    Assert-Refused { Assert-CardPayloadSlot (New-Bytes 1048575 255) $new150.Child $new150.Header } 'wrong extent' 'short slot'
    foreach ($case in @(@(0, 'magic'), @(8, 'version'), @(12, 'header size'), @(16, 'payload size'), @(24, 'slot size'), @(32, 'payload offset'), @(40, 'flags'), @(48, 'digest'))) {
        $header = [byte[]]$new150.Header.Clone(); $header[$case[0]] = $header[$case[0]] -bxor 1
        $slot = [byte[]]$new150.Slot.Clone(); $header.CopyTo($slot, 0)
        Assert-Refused { Assert-CardPayloadSlot $slot $new150.Child $header } 'header contract mismatch' "header $($case[1])"
    }
}
Test-Case 'flash-card.ps1 refuses payload programming without -ConfirmFlash, offline' {
    $flash = Join-Path $PSScriptRoot 'flash-card.ps1'
    Assert-Refused { & $flash -Action ProgramPayload -BuildPath 'target/card-dev/none' } 'requires explicit -ConfirmFlash' 'ProgramPayload'
    Assert-Refused { & $flash -Action RestorePayload -RestoreSession ('a' * 32) } 'requires explicit -ConfirmFlash' 'RestorePayload'
    Assert-Refused { & $flash -Action CheckPayload } 'requires -BuildPath' 'CheckPayload without a build'
    Assert-Refused { & $flash -Action CheckPayload -BuildPath (Join-Path $PSScriptRoot '..') } 'escapes constrained directory' 'build outside target/card-dev'
    Assert-Refused { & $flash -Action CheckPayload -BuildPath 'target/card-dev/x' -AdapterKhz 50 } 'AdapterKhz' 'slow adapter'
    Assert-Refused { & $flash -Action CheckOnly -BuildPath 'target/card-dev/x' } 'apply only to the payload-slot actions' 'full-image action with payload option'
    Assert-Refused { & $flash -Action CheckOnly -AdapterKhz 10000 } 'apply only to the payload-slot actions' 'full-image action with a speed'
}
Write-Host "$script:passed payload flashing test groups passed."
