[CmdletBinding()]
param([switch]$Program)
# User authorized the fault-capture reflash. Two full backups and exact predecessor verification precede erase. Defaults to local check.
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../..'))
$validation=Join-Path $root 'firmware/squirrel/card-multi-exit-validation.ps1'
if ((Get-FileHash -LiteralPath $validation).Hash.ToLowerInvariant() -cne '45f41b513371abf6a1317e75d71dc69acda6a44602a833a95cc302c7ec5d8e17') { throw 'Validation helper changed.' }
. $validation
$candidate='target/firmware/squirrel/native-resident/822d3756592b4c9fa8614788fea9baf6'
$expected='c50e5eab6007d84bc37169bd3c86fcfbcbc0876c8f8eb51a2bfbdcee74e62c40'
$previous='e11170c790a34cb190182a26fb1612efb008f4a6a5aac8cc32d05a5f0f2267c1'
$assets=@(
 @{Name='combined.bin';Path='target/firmware/squirrel/native-resident/822d3756592b4c9fa8614788fea9baf6/combined/combined-review.bin';Bytes=5242880;Hash='c50e5eab6007d84bc37169bd3c86fcfbcbc0876c8f8eb51a2bfbdcee74e62c40'},
 @{Name='known-working.bin';Path='work/fault-capture-2026-09-16/delivery/predecessor.bin';Bytes=5242880;Hash='e11170c790a34cb190182a26fb1612efb008f4a6a5aac8cc32d05a5f0f2267c1'},
 @{Name='candidate-manifest.json';Path='target/firmware/squirrel/native-resident/822d3756592b4c9fa8614788fea9baf6/manifest.json';Bytes=49983;Hash='a6849eed279a0af5560d976662ed93459ded3413f791f3023114838e0de5a211'},
 @{Name='review.json';Path='work/fault-capture-2026-09-16/flash-review/candidate-review.json';Bytes=20950;Hash='27b1d2834555d7a020391fe4f2f4150bd33dd41c29a1d11200e07cd3480ec69c'},
 @{Name='production-summary.json';Path='work/fault-capture-2026-09-16/production/summary.json';Bytes=3184;Hash='59352c492ee0d8ab7d6989b40657d5c379e3852e5d34c2c593befaf0fe32955d'},
 @{Name='production-source-manifest.json';Path='work/fault-capture-2026-09-16/production/source-manifest.json';Bytes=32960;Hash='e244a9c4d1c0c5a9e6fa52465c022f1f0eaf0ee87b0b40ae5a57f4b55b6e3771'},
 @{Name='source-commit.json';Path='work/fault-capture-2026-09-16/delivery/source-commit.json';Bytes=155;Hash='c77f7847bef466dc95e165829e40468ffa6025f7cc8442a9ebc3c12333cf0b58'},
 @{Name='openocd.exe';Path='target/firmware/tools/openocd/bin/openocd.exe';Bytes=13664247;Hash='9732b05af7e0f6a05a0051371e49af42515662ad309ddcc87f86f9b434ce96d8'},
 @{Name='proxy.bit';Path='target/firmware/tools/lambda-squirrel/flash_screamer/bscan_spi_xc7a35t.bit';Bytes=261513;Hash='ef8af1e277a7fe556e1ed7ace4680d4993cfc4174616485e1c354793d784b7f6'},
 @{Name='transport.cfg';Path='firmware/squirrel/openocd/card-load-transport.cfg';Bytes=1795;Hash='dd2176b5cb7652aceb2f58ea1aed2d8312e8535ef91939fe8fbf5216b7ef3112'},
 @{Name='program.cfg';Path='work/fault-capture-2026-09-16/delivery/program.cfg';Bytes=1436;Hash='3dd9a46fe96eb0c60c64fcf2ba4e18b5042e17182603d47000bc61046e03897c'},
 @{Name='validation.ps1';Path='firmware/squirrel/card-multi-exit-validation.ps1';Bytes=9226;Hash='45f41b513371abf6a1317e75d71dc69acda6a44602a833a95cc302c7ec5d8e17'},
 @{Name='card-positive-x2off.json';Path='work/fault-capture-2026-09-16/delivery/card-positive-x2off/result.json';Bytes=5840;Hash='bc4b44e464197e1fb4b024ea6462d1bd75c4602833b369c46170115fc198df78'}
)
$sessionId=[guid]::NewGuid().ToString('N')
$session=Join-Path $root ('target/firmware/squirrel/card-resident-reviewed-sessions/'+$sessionId)
$null=Assert-CardReturningPath $session $root
$sessionTcl=ConvertTo-CardReturningTclPath $session
$handles=[Collections.Generic.List[IO.FileStream]]::new()
$process=$null
$record=[ordered]@{source_commit='80a55c5ce4b59c99f1406f8a8af467f006c649e8';schema_version=1;procedure='card-resident-two-backup-v1';session_id=$sessionId;candidate=$candidate;action=$(if($Program){'Program'}else{'CheckOnly'});status='running';phase='staging';new_backups_created=$false;backup_sha256=$null;hardware_accessed=$false;activation_performed=$false;write_possible=$false;write_started=$false;image_sha256=$expected;canonical_predecessor_sha256=$previous;readback_sha256=$null;started_utc=[DateTime]::UtcNow.ToString('o');error=$null}
function Save-Record {
 $record.updated_utc=[DateTime]::UtcNow.ToString('o')
 $temp=Join-Path $session 'result.json.tmp'
 [IO.File]::WriteAllText($temp,($record|ConvertTo-Json -Depth 7),[Text.UTF8Encoding]::new($false))
 [IO.File]::Move($temp,(Join-Path $session 'result.json'),$true)
}
function Lock-Input([string]$Path,[long]$Bytes,[string]$Hash){
 $null=Assert-CardReturningPath $Path ''
 $handles.Add([IO.File]::Open($Path,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read))
 Assert-CardReturningFile $Path $Bytes $Hash
}
try {
 if($Program){
  foreach($relative in @('target/firmware/squirrel/card-multi-exit-sessions','target/firmware/squirrel/card-resident-sessions')){
   $directory=Assert-CardReturningPath (Join-Path $root $relative) $root
   [IO.Directory]::CreateDirectory($directory)|Out-Null
   $lockPath=Assert-CardReturningPath (Join-Path $directory 'operation.lock') $root
   $handles.Add([IO.File]::Open($lockPath,[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None))
  }
 }
 [IO.Directory]::CreateDirectory($session)|Out-Null
 Save-Record
 foreach($asset in $assets){
  $source=Join-Path $root $asset.Path
  Lock-Input $source $asset.Bytes $asset.Hash
  $dest=Join-Path $session $asset.Name
  [IO.File]::Copy($source,$dest,$false)
  Lock-Input $dest $asset.Bytes $asset.Hash
 }
 $callerHash=Get-CardReturningHash $PSCommandPath
 Lock-Input $PSCommandPath -1 $callerHash
 [IO.File]::Copy($PSCommandPath,(Join-Path $session 'procedure.ps1'),$false)
 Lock-Input (Join-Path $session 'procedure.ps1') -1 $callerHash
 $record.procedure_sha256=$callerHash
 $record.inputs=@($assets|ForEach-Object{[ordered]@{name=$_.Name;sha256=$_.Hash}})
 $manifest=Get-Content -Raw -LiteralPath (Join-Path $session 'candidate-manifest.json')|ConvertFrom-Json
 Assert-CardReturningFields $manifest @{status='built_review_required';payload_kind='NativeResidentBoot';combined_sha256=$expected;combined_bytes=5242880;hardware_accessed=$false;activation_performed=$false} 'Reviewed candidate'
 $review=Get-Content -Raw -LiteralPath (Join-Path $session 'review.json')|ConvertFrom-Json
 Assert-CardReturningFields $review @{status='pass';candidate=$candidate;firmware_fixture_status='pass';combined_sha256=$expected} 'Independent review'
 $guard=Get-CimInstance -Namespace root\Microsoft\Windows\DeviceGuard -ClassName Win32_DeviceGuard
 $computer=Get-CimInstance -ClassName Win32_ComputerSystem
 $record.platform=[ordered]@{vbs_status=$guard.VirtualizationBasedSecurityStatus;security_services_running=@($guard.SecurityServicesRunning);hypervisor_present=$computer.HypervisorPresent}
 if($guard.VirtualizationBasedSecurityStatus -ne 0 -or $computer.HypervisorPresent){throw 'Target still has VBS or a hypervisor running.'}
 if(-not $Program){$record.status='offline_pass';$record.phase='complete';Save-Record;Write-Output "PASS reviewed two-backup offline check: $session";return}
 $record.phase='programming';$record.hardware_accessed=$true;$record.write_possible=$true;Save-Record
 $stdout=Join-Path $session 'program.stdout.log';$stderr=Join-Path $session 'program.stderr.log'
 $arguments=@('-c','"gdb_port disabled"','-c','"tcl_port disabled"','-c','"telnet_port disabled"','-c',('"set SESSION {'+$sessionTcl+'}"'),'-f',('"'+(Join-Path $session 'program.cfg')+'"'))
 $process=Start-Process -FilePath (Join-Path $session 'openocd.exe') -ArgumentList $arguments -WorkingDirectory $session -WindowStyle Hidden -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
 $null=$process.Handle
 Write-Output "Programming session: $session"
 if(-not $process.WaitForExit(1800000)){$process.Kill($true);$process.WaitForExit();throw 'OpenOCD programming timed out.'}
 $process.Refresh()
 if($null -eq $process.ExitCode -or $process.ExitCode -ne 0){throw "OpenOCD failed with exit $($process.ExitCode)."}
 $trace=[IO.File]::ReadAllText($stdout)+[IO.File]::ReadAllText($stderr)
 foreach($marker in @('PASS card-target-id-and-geometry','PASS card-reviewed-backups-and-predecessor-verified','BEGIN card-reviewed-write','PASS card-reviewed-program-readback')){if(-not $trace.Contains($marker)){throw "Missing stage marker: $marker"}}
 $record.write_started=$true;$record.phase='readback_verification';Save-Record
 Lock-Input (Join-Path $session 'before-a.bin') 5242880 $previous
 Lock-Input (Join-Path $session 'before-b.bin') 5242880 $previous
 $record.new_backups_created=$true
 $record.backup_sha256=$previous
 Lock-Input (Join-Path $session 'readback.bin') 5242880 $expected
 $record.readback_sha256=$expected;$record.status='verified_not_activated';$record.phase='complete';Save-Record
 Write-Output "PASS exact 5 MiB programmed and readback verified; two full predecessor backups retained; no activation. Evidence: $session"
} catch {
 $record.status='failed';$record.error=$_.Exception.Message
 if(Test-Path -LiteralPath $session){Save-Record}
 throw
} finally {
 if($null -ne $process){if(-not $process.HasExited){$process.Kill($true);$process.WaitForExit()};$process.Dispose()}
 if(Test-Path -LiteralPath $session){
  $trace=''
  foreach($name in @('program.stdout.log','program.stderr.log')){$log=Join-Path $session $name;if(Test-Path -LiteralPath $log){$trace += [IO.File]::ReadAllText($log)+"`n"}}
  if($trace){[IO.File]::WriteAllText((Join-Path $session 'program.log'),$trace,[Text.UTF8Encoding]::new($false));$record.write_started=$trace.Contains('BEGIN card-reviewed-write');
   $backupA=Join-Path $session 'before-a.bin'; $backupB=Join-Path $session 'before-b.bin'
   if ((Test-Path -LiteralPath $backupA) -and (Test-Path -LiteralPath $backupB)) {
    if ((Get-Item -LiteralPath $backupA).Length -eq 5242880 -and (Get-Item -LiteralPath $backupB).Length -eq 5242880 -and
        (Get-CardReturningHash $backupA) -ceq $previous -and (Get-CardReturningHash $backupB) -ceq $previous) {
     $record.new_backups_created=$true; $record.backup_sha256=$previous
    }
   }
   $record.log_sha256=Get-CardReturningHash (Join-Path $session 'program.log');Save-Record}
 }
 foreach($handle in $handles){$handle.Dispose()}
}
