[CmdletBinding()]
param([Parameter(Mandatory)][string]$ResultPath,
    [Parameter(Mandatory)][string]$OutputPath)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '../uefi-smp-evidence.ps1')
$source = (Resolve-Path -LiteralPath $ResultPath).Path
$originalJson = Get-Content -Raw -LiteralPath $source
$original = $originalJson | ConvertFrom-Json
$baseline = Get-UefiSmpEvidence -Record $original
if (-not $baseline.validFixture) { throw ('Positive input failed: ' + ($baseline.errors -join ', ')) }
$controls = [System.Collections.Generic.List[object]]::new()
function Test-Corruption([string]$ControlName, [scriptblock]$Mutation) {
    $candidate = $originalJson | ConvertFrom-Json
    & $Mutation $candidate
    $result = Get-UefiSmpEvidence -Record $candidate
    if ($result.validFixture) { throw "Corrupted evidence accepted: $ControlName" }
    $controls.Add([ordered]@{name=$ControlName;rejected=$true;errors=@($result.errors)})
}

# Each real lifecycle, startup, identity and ownership witness is independently
# indispensable, unique and ordered. The original QEMU execution remains intact.
$witnesses = @($original.trace -split '\r?\n' | Where-Object {
    $_ -match '^(uefi-launcher-entry|uefi-driver-loaded|uefi-driver-entry|uefi-smp-firmware-count |uefi-smp-mp-callbacks-returned=|uefi-smp-low-page=|uefi-smp-cpu |uefi-smp-pre-ebs-admitted|uefi-payload-base=|uefi-payload-reserved|uefi-boot-services-exited|uefi-smp-post-ebs-owned|uefi-private-entry|rust-core|PASS resident-|host-idt-installed|host-memory-rx-ro-rw|host-mappings-restricted|xstate-profile=|SMP-STARTUP |UEFI-SMP |PASS uefi-smp-owned-ranges=|PASS rust-dispatch)'
})
foreach ($line in $witnesses) {
    $pattern = '(?m)^' + [regex]::Escape($line) + '\r?\n?'
    Test-Corruption "missing:$line" { param($c) $c.trace = [regex]::Replace($c.trace,$pattern,'') }
    Test-Corruption "duplicate:$line" { param($c) $c.trace += "`n$line`n" }
}
foreach ($pair in @(
    @('uefi-smp-mp-callbacks-returned=1','uefi-boot-services-exited'),
    @('uefi-smp-pre-ebs-admitted','uefi-boot-services-exited'),
    @('uefi-boot-services-exited','uefi-private-entry'),
    @('uefi-smp-post-ebs-owned','uefi-private-entry'),
    @('PASS resident-ownership-retained','host-mappings-restricted'),
    @('PASS resident-smp-low-reservation','PASS resident-ownership-private'))) {
    Test-Corruption "reordered:$($pair[0])" {
        param($c)
        $a = $pair[0]; $b = $pair[1]
        $c.trace = $c.trace.Replace($a,'__SWAP__').Replace($b,$a).Replace('__SWAP__',$b)
    }
}
foreach ($field in @('qemuSha256','firmwareSha256','varsTemplateSha256',
    'payloadSha256','relocationPackageSha256','originalPackageSha256','driverSha256','loaderSha256')) {
    Test-Corruption "missing-field:$field" { param($c) $c.PSObject.Properties.Remove($field) }
    Test-Corruption "malformed-hash:$field" { param($c) $c.$field = 'invalid' }
}
foreach ($field in @('qemuSha256','firmwareSha256','varsTemplateSha256')) {
    Test-Corruption "different-pin:$field" { param($c) $c.$field = '0' * 64 }
}
foreach ($field in @('uefiSmp','timedOut','rdtscpDisabled')) {
    Test-Corruption "flipped:$field" { param($c) $c.$field = -not $c.$field }
    Test-Corruption "string-boolean:$field" { param($c) $c.$field = [string]$c.$field }
}
foreach ($entry in @(
    @('hostCpuCount',1),@('hostCpuCount',3),@('exitCode',35),@('stderr','unexpected warning'),
    @('expectedSmpRefusal','Topology'),@('machine','q35'),@('cpu','max,svm=on'),
    @('actualArenaBase',0),@('requestedArenaBase',4096),@('linkedEntryAddress',0),
    @('entryAddress',0),@('sipiPageRequest','1'),@('sipiPageRequest','garbage'),
    @('sipiPageRequest','18446744073709551616'),@('xstateProfile','invalid'))) {
    Test-Corruption "metadata:$($entry[0]):$($entry[1])" { param($c) $c.($entry[0]) = $entry[1] }
}
foreach ($flag in @('rejectMmio','reconnect','pciRom','omittedRom','productionDxe',
    'noAuthorization','rejectHandoff','rejectOwnership','rejectRelocation',
    'rejectResidentOwnership','expectArenaRejection','expectAllocationRejection')) {
    Test-Corruption "negative-flag:$flag" { param($c) $c.$flag = $true }
}
foreach ($option in @('-machine','-cpu','-m','-smp','-accel','-nic','-display','-monitor','-serial')) {
    Test-Corruption "argument:$option" {
        param($c)
        $index = [Array]::IndexOf($c.arguments,$option)
        $c.arguments[$index+1] = 'wrong'
    }
    Test-Corruption "duplicate-argument:$option" { param($c) $c.arguments += @($option,'wrong') }
}
foreach ($entry in @(@('page','0000000000000000'),@('page','0000000000100000'),
    @('page','000000000009f001'),@('vector','0000000000000000'),
    @('root','0000000000001000'),@('root','ffffffffffffffff'),
    @('resident','0000000000000000'),@('patched','0000000000000002'),
    @('rdtscp','0000000000000002'))) {
    Test-Corruption "startup-value:$($entry[0]):$($entry[1])" {
        param($c)
        $c.trace = [regex]::Replace($c.trace,'(?m)^(SMP-STARTUP '+$entry[0]+'=)[0-9a-f]{16}',('${1}'+$entry[1]))
    }
}
foreach ($field in @('processor','apic','signature')) {
    Test-Corruption "cpu1:$field" {
        param($c)
        $rows = [regex]::Matches($c.trace,'(?m)^uefi-smp-cpu .*$')
        $old = $rows[1].Value
        $new = [regex]::Replace($old,'('+ $field +'=0x)[0-9a-f]{16}','${1}0000000000000000')
        $c.trace = $c.trace.Replace($old,$new)
    }
}
Test-Corruption 'wrong-vendor' { param($c) $c.trace = $c.trace.Replace('vendor=AuthenticAMD','vendor=GenuineIntel') }
Test-Corruption 'signature-disagreement' {
    param($c)
    $rows=[regex]::Matches($c.trace,'(?m)^uefi-smp-cpu .*$');$old=$rows[1].Value
    $c.trace=$c.trace.Replace($old,[regex]::Replace($old,'signature=0x[0-9a-f]{16}','signature=0x0000000000000001'))
}
Test-Corruption 'firmware-extra-cpu' {
    param($c) $c.trace=$c.trace.Replace('total=0x0000000000000002','total=0x0000000000000003')
}
foreach ($name in @('host-rsp','hsave','vmcb','guest-xstate','gpr','controller','host-xstate','clock')) {
    Test-Corruption "owned-outside:$name" {
        param($c)
        $c.trace=[regex]::Replace($c.trace,'(?m)^(UEFI-SMP cpu1-'+$name+'=)[0-9a-f]{16}','${1}0000000000001000')
    }
    Test-Corruption "cross-cpu-alias:$name" {
        param($c)
        $other=[regex]::Match($c.trace,'(?m)^UEFI-SMP cpu0-'+$name+'=([0-9a-f]{16})').Groups[1].Value
        $c.trace=[regex]::Replace($c.trace,'(?m)^(UEFI-SMP cpu1-'+$name+'=)[0-9a-f]{16}',('${1}'+$other))
    }
}
Test-Corruption 'concurrent-false-delivery' {
    param($c) $c.trace=[regex]::Replace($c.trace,'(?m)^(CONCURRENT cpu0-delivered=)[0-9a-f]{16}','${1}0000000000000000')
}
Test-Corruption 'forbidden-failure-marker' { param($c) $c.trace += "`nFAIL unexpected`n" }
Test-Corruption 'truncated-trace' { param($c) $c.trace = $c.trace.Substring(0,[int]($c.trace.Length/2)) }
$result = [ordered]@{
    valid=$true; baselineAccepted=$true; corruptedControls=$controls.Count
    sourceResult=$source; sourceResultSha256=(Get-FileHash -LiteralPath $source).Hash
    payloadSha256=$original.payloadSha256; sourceTraceSha256=$null
    helperSha256=(Get-FileHash (Join-Path $PSScriptRoot '../uefi-smp-evidence.ps1')).Hash
    concurrentHelperSha256=(Get-FileHash (Join-Path $PSScriptRoot '../concurrent-evidence.ps1')).Hash
    controlsSha256=(Get-FileHash -LiteralPath $PSCommandPath).Hash; controls=$controls.ToArray()
    scope='retained-evidence-validation-only-no-new-execution'
}
$result.sourceTraceSha256 = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData(
    [Text.Encoding]::UTF8.GetBytes([string]$original.trace))).ToLowerInvariant()
$result | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $OutputPath
Write-Output "UEFI SMP evidence controls passed: $($controls.Count) rejected. $OutputPath"
