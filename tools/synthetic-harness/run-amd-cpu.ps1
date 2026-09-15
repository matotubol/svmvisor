[CmdletBinding()]
param([Parameter(Mandatory)][string]$QemuPath,
    [ValidateSet('Avx','Sse','Fx')][string]$XstateProfile='Avx',
    [switch]$DisableRdtscp,[switch]$RejectSingleCpu)
$ErrorActionPreference='Stop'
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$qemu=(Resolve-Path -LiteralPath $QemuPath).Path
if((Get-FileHash -LiteralPath $qemu).Hash -ne 'c867dd99822400be12f1aa3fbf9995ea5f103486fd3affede6076cd444cb3047') {
    throw 'Requires pinned corrected SMP QEMU backend'
}
$image=Join-Path $root 'target/synthetic-harness/amd-cpu-model.bin'
$session=Join-Path $root ('target/synthetic-harness/amd-cpu-runs/'+[Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $session | Out-Null
$cpu='max,svm=on,hypervisor=off'+$(if($DisableRdtscp){',rdtscp=off'}else{''})+
    $(if($XstateProfile -eq 'Fx'){',xsave=off,avx=off,avx2=off'}elseif($XstateProfile -eq 'Sse'){',avx=off,avx2=off'}else{''})
$count=if($RejectSingleCpu){1}else{2}
$debug=Join-Path $session 'debug.log'
$stderr=Join-Path $session 'stderr.log'
$arguments=@('-machine','pc','-accel','tcg,thread=multi','-cpu',$cpu,'-m','64M',
    '-smp',"$count,sockets=1,cores=$count,threads=1",'-no-reboot','-display','none',
    '-monitor','none','-serial','none','-nic','none','-debugcon',('"file:'+$debug+'"'),
    '-device','isa-debug-exit,iobase=0xf4,iosize=0x04','-kernel',('"'+$image+'"'))
$process=Start-Process -FilePath $qemu -ArgumentList $arguments -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $session 'stdout.log') -RedirectStandardError $stderr
$timedOut=-not $process.WaitForExit(30000)
if($timedOut){$process.Kill();$process.WaitForExit()}
$process.Refresh()
$trace=if(Test-Path $debug){Get-Content -LiteralPath $debug -Raw}else{''}
$errors=Get-Content -LiteralPath $stderr -Raw
$metrics=@{}
$valid=$true
$matches=[regex]::Matches($trace,'(?m)^AMD-CPU cpu=([01]) ([a-z]+)=([0-9a-f]{16})\r?$')
foreach($match in $matches){
    $key=$match.Groups[1].Value+'/'+$match.Groups[2].Value
    if($metrics.ContainsKey($key)){$valid=$false}
    $metrics[$key]=[Convert]::ToUInt64($match.Groups[3].Value,16)
}
$expectedQueries=if($XstateProfile -eq 'Fx'){1896}else{1898}
$expectedEntries=if($XstateProfile -eq 'Fx'){3792}else{3811}
$expectedRefused=if($XstateProfile -eq 'Fx'){1896}else{1903}
$expectedXsetbv=if($XstateProfile -eq 'Fx'){0}else{4}
foreach($id in 0..1){
    $valid=$valid -and $metrics["$id/queries"] -eq $expectedQueries -and $metrics["$id/entries"] -eq $expectedEntries `
        -and $metrics["$id/refused"] -eq $expectedRefused -and $metrics["$id/xsetbv"] -eq $expectedXsetbv `
        -and $metrics["$id/digest"] -gt 0
}
$valid=$valid -and $matches.Count -eq 10 -and ([regex]::Matches($trace,'(?m)^AMD-CPU ').Count -eq 10)
foreach($marker in @('PASS amd-cpu-model two-cpu-real-cpuid-native-vendor-no-hypervisor-leaves',
    'PASS amd-xcr0 dynamic-owned-state-and-unchanged-refusals','PASS rust-dispatch')) {
    $valid=$valid -and ([regex]::Matches($trace,'(?m)^'+[regex]::Escape($marker)+'\r?$').Count -eq 1)
}
$pass=$valid -and -not $timedOut -and -not $errors -and $process.ExitCode -eq 33 -and $trace -notmatch '(?m)^(FAIL|REFUSE|GAP) '
if($RejectSingleCpu){
    $pass=-not $timedOut -and -not $errors -and $process.ExitCode -eq 35 -and
        $trace -match '(?m)^REFUSE concurrent-host-topology expected=2 actual=0000000000000001\r?$' -and
        $trace -notmatch 'AMD-CPU|PASS amd|PASS rust-dispatch'
}
[ordered]@{status=$(if($pass){'passed'}else{'failed'});imageSha256=(Get-FileHash $image).Hash;
    qemuSha256=(Get-FileHash $qemu).Hash;cpu=$cpu;profile=$XstateProfile;rdtscpDisabled=[bool]$DisableRdtscp;
    rejectSingleCpu=[bool]$RejectSingleCpu;arguments=$arguments;exitCode=$process.ExitCode;timedOut=$timedOut;
    metrics=$metrics;trace=$trace;stderr=$errors} | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $session 'result.json')
if(-not $pass){throw "AMD CPU fixture failed; evidence=$session"}
Write-Output "PASS AMD CPU fixture; evidence=$session"
