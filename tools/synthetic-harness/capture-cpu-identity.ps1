[CmdletBinding()]
param([Parameter(Mandatory)][string]$OutputPath)
$ErrorActionPreference='Stop'
if(Test-Path -LiteralPath $OutputPath){throw 'Preserve existing identity evidence'}
if(-not [System.Runtime.Intrinsics.X86.X86Base]::IsSupported){throw 'CPUID unavailable'}
function Read-Cpuid([uint32]$Leaf,[uint32]$Subleaf=0) {
    $a=[BitConverter]::ToInt32([BitConverter]::GetBytes($Leaf),0)
    $c=[BitConverter]::ToInt32([BitConverter]::GetBytes($Subleaf),0)
    $v=[System.Runtime.Intrinsics.X86.X86Base]::CpuId($a,$c)
    ,@($v.Item1,$v.Item2,$v.Item3,$v.Item4 | ForEach-Object {[BitConverter]::ToUInt32([BitConverter]::GetBytes([int]$_),0)})
}
$basic=Read-Cpuid 0
$extended=Read-Cpuid 2147483648
$rows=[Collections.Generic.List[object]]::new()
# Bounded raw observation. Leaf-specific availability still needs feature-bit
# interpretation; max-leaf enumeration alone is not a capability declaration.
if($basic[0] -gt 256 -or $extended[0] -lt 2147483648 -or $extended[0] -gt 2147483904){throw 'Unadmitted enumeration extent'}
$leaves=[Collections.Generic.List[uint32]]::new()
for([uint32]$leaf=0;$leaf -le $basic[0];$leaf++){$leaves.Add($leaf)}
for([long]$leaf=2147483648;$leaf -le $extended[0];$leaf++){$leaves.Add([uint32]$leaf)}
foreach($leaf in $leaves) {
    $rows.Add([ordered]@{leaf=('{0:x8}' -f $leaf);subleaf=0;registers=(Read-Cpuid ([uint32]$leaf))})
}
# Capture bounded indexed CPU descriptions, including explicit termination
# records. This tool is read-only and does not alter affinity or CPU settings.
foreach($leaf in @(7,11,13,2147483677L,2147483686L)) {
    $maximum=if($leaf -ge 2147483648){$extended[0]}else{$basic[0]}
    if($leaf -gt $maximum){continue}
    foreach($sub in 1..63) {
        $v=Read-Cpuid ([uint32]$leaf) $sub
        $rows.Add([ordered]@{leaf=('{0:x8}' -f $leaf);subleaf=$sub;registers=$v})
        if($leaf -eq 7 -and $sub -ge (Read-Cpuid 7)[0]){break}
        if($leaf -eq 11 -and $v[1] -eq 0){break}
        if($leaf -eq 2147483677L -and ($v[0] -band 31) -eq 0){break}
        if($leaf -eq 2147483686L -and (($v[2] -shr 8) -band 255) -eq 0){break}
    }
}
$brand=$null
if($extended[0] -ge 2147483652L){
    $brandBytes=[Collections.Generic.List[byte]]::new()
    foreach($leaf in @(2147483650L,2147483651L,2147483652L)){foreach($value in (Read-Cpuid ([uint32]$leaf))){$brandBytes.AddRange([BitConverter]::GetBytes([uint32]$value))}}
    $brand=[Text.Encoding]::ASCII.GetString($brandBytes.ToArray()).Trim([char]0).Trim()
}
$parent=Split-Path -Parent ([IO.Path]::GetFullPath($OutputPath))
New-Item -ItemType Directory -Force -Path $parent | Out-Null
[ordered]@{source='Windows-visible CPUID; not a bare-metal or per-CPU inventory claim';
    brand=$brand;maxBasic=$basic[0];maxExtended=$extended[0];raw=$rows} |
    ConvertTo-Json -Depth 7 | Set-Content -LiteralPath $OutputPath -Encoding utf8
Write-Output "Captured identity: $brand; evidence=$OutputPath"
