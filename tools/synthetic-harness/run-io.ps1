[CmdletBinding()]
param([Parameter(Mandatory)][string]$QemuPath,
    [ValidateSet('Avx','Sse','Fx')][string]$XstateProfile='Avx',
    [switch]$DisableRdtscp,[switch]$MissingEndpoint,[switch]$BypassBoundary)
$ErrorActionPreference='Stop'
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$qemu=(Resolve-Path -LiteralPath $QemuPath).Path
if((Get-FileHash -LiteralPath $qemu).Hash -ne 'c867dd99822400be12f1aa3fbf9995ea5f103486fd3affede6076cd444cb3047') {
    throw 'Requires pinned corrected QEMU backend'
}
$name=if($BypassBoundary){'io-intercept-bypass'}else{'io-intercept'}
$image=Join-Path $root "target/synthetic-harness/$name.bin"
$session=Join-Path $root ('target/synthetic-harness/io-runs/'+[Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $session | Out-Null
$cpu='max,svm=on,hypervisor=off'+$(if($DisableRdtscp){',rdtscp=off'}else{''})+
    $(if($XstateProfile -eq 'Fx'){',xsave=off,avx=off,avx2=off'}elseif($XstateProfile -eq 'Sse'){',avx=off,avx2=off'}else{''})
$debug=Join-Path $session 'debug.log'
$stderr=Join-Path $session 'stderr.log'
$arguments=@('-machine','pc-i440fx-10.1','-accel','tcg,thread=single','-cpu',$cpu,'-m','64M',
    '-smp','1','-no-reboot','-display','none','-monitor','none','-serial','none','-nic','none',
    '-debugcon',('"file:'+$debug+'"'),'-device','isa-debug-exit,iobase=0xf4,iosize=0x04',
    '-kernel',('"'+$image+'"'))
if(-not $MissingEndpoint){$arguments+=@('-chardev','null,id=io-witness','-device','isa-serial,chardev=io-witness,iobase=0x3f8,irq=4')}
$process=Start-Process -FilePath $qemu -ArgumentList $arguments -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $session 'stdout.log') -RedirectStandardError $stderr
$timedOut=-not $process.WaitForExit(30000)
if($timedOut){$process.Kill();$process.WaitForExit()}
$process.Refresh()
$trace=if(Test-Path $debug){Get-Content -LiteralPath $debug -Raw}else{''}
$errors=Get-Content -LiteralPath $stderr -Raw
$expected=if($MissingEndpoint -or $BypassBoundary){35}else{33}
$pass=-not $timedOut -and -not $errors -and $process.ExitCode -eq $expected
if($MissingEndpoint){$pass=$pass -and $trace -match 'FAIL rust-dispatch' -and $trace -notmatch '(?m)^IO id='}
elseif($BypassBoundary){$pass=$pass -and $trace -match 'IO id=0000000000000002' -and $trace -match 'FAIL rust-dispatch' -and $trace -notmatch 'PASS io-boundary'}
else{$pass=$pass -and $trace -match 'PASS io-boundary stopped-in-out-live-endpoint-pending-preserved' -and $trace -notmatch '(?m)^(FAIL|REFUSE|GAP) '}
$result=Join-Path $session 'result.json'
[ordered]@{status=$(if($pass){'passed'}else{'failed'});imageSha256=(Get-FileHash $image).Hash;
    qemuSha256=(Get-FileHash $qemu).Hash;cpu=$cpu;profile=$XstateProfile;rdtscpDisabled=[bool]$DisableRdtscp;
    missingEndpoint=[bool]$MissingEndpoint;bypassBoundary=[bool]$BypassBoundary;arguments=$arguments;
    exitCode=$process.ExitCode;timedOut=$timedOut;trace=$trace;stderr=$errors} |
    ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $result
if($pass){
    & python (Join-Path $PSScriptRoot 'audit-io.py') $result
    $pass=$LASTEXITCODE -eq 0
    if(-not $pass){$r=Get-Content $result -Raw | ConvertFrom-Json; $r.status='failed'; $r | ConvertTo-Json -Depth 5 | Set-Content $result}
}
if(-not $pass){throw "I/O fixture failed; evidence=$session"}
Write-Output "PASS I/O fixture; evidence=$session"
