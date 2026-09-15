[CmdletBinding()]
param([Parameter(Mandatory)][string]$QemuPath,[ValidateSet('Avx','Sse','Fx')][string]$XstateProfile='Avx',[switch]$DisableRdtscp,[switch]$RejectSingleCpu)
$ErrorActionPreference='Stop'
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$qemu=(Resolve-Path -LiteralPath $QemuPath).Path
$image=Join-Path $root 'target/synthetic-harness/concurrent-smp.bin'
if(-not(Test-Path -LiteralPath $image)){throw 'Build -ConcurrentSmp first'}
if((Get-FileHash $qemu -Algorithm SHA256).Hash -ne 'c867dd99822400be12f1aa3fbf9995ea5f103486fd3affede6076cd444cb3047'){throw 'Concurrent profile requires the pinned SMP-corrected backend'}
$session=Join-Path $root ('target/synthetic-harness/concurrent-runs/'+[Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $session | Out-Null
$debug=Join-Path $session 'debug.log'
$stdout=Join-Path $session 'stdout.log'
$stderr=Join-Path $session 'stderr.log'
$cpu='max,svm=on,hypervisor=off'+$(if($DisableRdtscp){',rdtscp=off'}else{''})+$(if($XstateProfile -eq 'Fx'){',xsave=off,avx=off,avx2=off'}elseif($XstateProfile -eq 'Sse'){',avx=off,avx2=off'}else{''})
$topology=if($RejectSingleCpu){'1,sockets=1,cores=1,threads=1'}else{'2,sockets=1,cores=2,threads=1'}
$arguments=@('-machine','pc','-accel','tcg,thread=multi','-cpu',$cpu,'-m','64M','-smp',$topology,
    '-no-reboot','-display','none','-monitor','none','-serial','none','-nic','none',
    '-debugcon',('"file:'+$debug+'"'),'-device','isa-debug-exit,iobase=0xf4,iosize=0x04','-kernel',('"'+$image+'"'))
$process=Start-Process -FilePath $qemu -ArgumentList $arguments -PassThru -WindowStyle Hidden -RedirectStandardOutput $stdout -RedirectStandardError $stderr
$timedOut=-not $process.WaitForExit(30000)
if($timedOut){$process.Kill();$process.WaitForExit()}
$process.Refresh()
$trace=if(Test-Path $debug){Get-Content $debug -Raw}else{''}
$errors=if(Test-Path $stderr){Get-Content $stderr -Raw}else{''}
. (Join-Path $PSScriptRoot 'concurrent-evidence.ps1')
$evidence=Get-ConcurrentEvidence $trace
$pass=-not $timedOut -and $process.ExitCode -eq 33 -and $evidence.validFixture -and ([regex]::Matches($trace,'(?m)^PASS rust-dispatch\r?$').Count -eq 1) -and -not $errors
if($RejectSingleCpu){$pass=-not $timedOut -and $process.ExitCode -eq 35 -and -not $errors -and -not $evidence.validFixture -and ([regex]::Matches($trace,'(?m)^REFUSE concurrent-host-topology expected=2 actual=0000000000000001\r?$').Count -eq 1) -and $trace -notmatch '(?m)^(CONCURRENT |PASS concurrent-|PASS rust-dispatch)'}
$record=[ordered]@{imageSha256=(Get-FileHash $image -Algorithm SHA256).Hash;qemuSha256=(Get-FileHash $qemu -Algorithm SHA256).Hash;cpu=$cpu;acceleration='tcg,thread=multi';hostCpuCount=$(if($RejectSingleCpu){1}else{2});topology=$topology;arguments=$arguments;xstateProfile=$XstateProfile;rdtscpDisabled=[bool]$DisableRdtscp;rejectSingleCpu=[bool]$RejectSingleCpu;exitCode=$process.ExitCode;timedOut=$timedOut;trace=$trace;stderr=$errors;concurrent=$evidence;status=$(if($pass){'passed'}else{'failed'})}
$record | ConvertTo-Json -Depth 9 | Set-Content (Join-Path $session 'result.json')
Write-Output $trace
if($errors){Write-Output $errors}
if(-not $pass){throw "Concurrent SMP failed. Evidence: $session"}
Write-Output "Concurrent SMP passed. Evidence: $session"
