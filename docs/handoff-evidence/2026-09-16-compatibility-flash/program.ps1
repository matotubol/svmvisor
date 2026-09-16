[CmdletBinding()]
param([switch]$Program)
# User authorized the committed compatibility reflash. Two full backups and exact predecessor verification precede erase. Defaults to local check.
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../..'))
$validation=Join-Path $root 'firmware/squirrel/card-multi-exit-validation.ps1'
if ((Get-FileHash -LiteralPath $validation).Hash.ToLowerInvariant() -cne '45f41b513371abf6a1317e75d71dc69acda6a44602a833a95cc302c7ec5d8e17') { throw 'Validation helper changed.' }
. $validation
$candidate='target/firmware/squirrel/native-resident/3a704487861c4b0792c93d16cfcb3a11'
$expected='e11170c790a34cb190182a26fb1612efb008f4a6a5aac8cc32d05a5f0f2267c1'
$previous='cd852f9192925f447d856cbe826c2b06ffa1a16ddde1b36d702bcd824a2c61fd'
$assets=@(
 @{Name='combined.bin';Path='target/firmware/squirrel/native-resident/3a704487861c4b0792c93d16cfcb3a11/combined/combined-review.bin';Bytes=5242880;Hash='e11170c790a34cb190182a26fb1612efb008f4a6a5aac8cc32d05a5f0f2267c1'},
 @{Name='known-working.bin';Path='work/compat-flash-2026-09-16/delivery/predecessor.bin';Bytes=5242880;Hash='cd852f9192925f447d856cbe826c2b06ffa1a16ddde1b36d702bcd824a2c61fd'},
 @{Name='candidate-manifest.json';Path='target/firmware/squirrel/native-resident/3a704487861c4b0792c93d16cfcb3a11/manifest.json';Bytes=49986;Hash='669c6e6a5c23788bc3324fcd617aa3a922897fb11e72ea4d4f966cfb0b0fa2ce'},
 @{Name='review.json';Path='work/compat-flash-2026-09-16/flash-review/candidate-review.json';Bytes=57696;Hash='cbe6ef15c5a6e9e480bdd8e9ca6ff0b00de55ba7bf2e39aa19fba81334560e8d'},
 @{Name='production-summary.json';Path='work/compat-flash-2026-09-16/production-low/summary.json';Bytes=3184;Hash='021a7c392b386c9e8c6822d6f55c33f4b63cbd31f85f013f5920b1dc9bcf159f'},
 @{Name='production-source-manifest.json';Path='work/compat-flash-2026-09-16/production-low/source-manifest.json';Bytes=32960;Hash='7cc96e511211c3bad6328ef7373123b22829333388417df3240d18dff9edf6bd'},
 @{Name='host-checks.json';Path='work/compat-flash-2026-09-16/host-checks.json';Bytes=697;Hash='826c84222738e1a5587ae154f8d70fd7ce052656d0f6c5f0f1e76bec372c235c'},
 @{Name='source-commit.json';Path='work/compat-flash-2026-09-16/delivery/source-commit.json';Bytes=153;Hash='067a1c8186695c882c984b961490a13d970adff7039309cbf2a8ff7aa5e3b3ce'},
 @{Name='openocd.exe';Path='target/firmware/tools/openocd/bin/openocd.exe';Bytes=13664247;Hash='9732b05af7e0f6a05a0051371e49af42515662ad309ddcc87f86f9b434ce96d8'},
 @{Name='proxy.bit';Path='target/firmware/tools/lambda-squirrel/flash_screamer/bscan_spi_xc7a35t.bit';Bytes=261513;Hash='ef8af1e277a7fe556e1ed7ace4680d4993cfc4174616485e1c354793d784b7f6'},
 @{Name='transport.cfg';Path='firmware/squirrel/openocd/card-load-transport.cfg';Bytes=1795;Hash='dd2176b5cb7652aceb2f58ea1aed2d8312e8535ef91939fe8fbf5216b7ef3112'},
 @{Name='program.cfg';Path='work/compat-flash-2026-09-16/delivery/program.cfg';Bytes=1436;Hash='3dd9a46fe96eb0c60c64fcf2ba4e18b5042e17182603d47000bc61046e03897c'},
 @{Name='validation.ps1';Path='firmware/squirrel/card-multi-exit-validation.ps1';Bytes=9226;Hash='45f41b513371abf6a1317e75d71dc69acda6a44602a833a95cc302c7ec5d8e17'},
 @{Name='card-positive.json';Path='work/compat-flash-2026-09-16/delivery/card-positive/result.json';Bytes=5841;Hash='4cafddcefa7120af058c8fea90fea0618c336d6e4f53d199f56efd4f6a457914'},
 @{Name='card-positive-x2off.json';Path='work/compat-flash-2026-09-16/delivery/card-positive-x2off/result.json';Bytes=5840;Hash='977c32e9b77b8d7e6c7e4e131f8031e6278a3b20e265fef7d56b95502b9d6e49'},
 @{Name='card-header.json';Path='work/compat-flash-2026-09-16/delivery/card-header/result.json';Bytes=5652;Hash='e57cc8741c71a6879c9fa1310c0d46bbd398893a6c9e6ba59b21eb1bfd28b30f'},
 @{Name='card-digest.json';Path='work/compat-flash-2026-09-16/delivery/card-digest/result.json';Bytes=5652;Hash='d50d7ae47cccd083ace2b715a41b855ddb18d80d2620aa2625c9ad23a29da278'},
 @{Name='card-admission.json';Path='work/compat-flash-2026-09-16/delivery/card-admission/result.json';Bytes=5751;Hash='caf852b72dfa8cc0872c0e8bc91dfd41d40a262830d8a5b536ba2c73497894f7'},
 @{Name='nrip-cpuid-execution.json';Path='work/qemu-nrip/cpuid-execution/summary.json';Bytes=5413;Hash='7d0cc374ec66675932fcb722122fb5612fb6386375c9dd2c04b1b4856c1ce77f'},
 @{Name='nrip-msr-execution.json';Path='work/qemu-nrip/msr-execution/summary.json';Bytes=5375;Hash='7d2945117c48c5b004d93e9276b5bdc2847bcf763ce9783e5ee2c9775ea13f89'},
 @{Name='nrip-cache-execution.json';Path='work/qemu-nrip/cache-execution/summary.json';Bytes=5943;Hash='dfbacfd23cc0ff76df963e7d304e298f7019a6044f5e5262257da31e1c28948e'},
 @{Name='nrip-feature-off-baseline.json';Path='work/qemu-nrip/feature-off-baseline/summary.json';Bytes=5340;Hash='888b607c66809c343a383c808e5d35b10f8b7894a10070c00dcea305f1fb5a2b'}
)
$sessionId=[guid]::NewGuid().ToString('N')
$session=Join-Path $root ('target/firmware/squirrel/card-resident-reviewed-sessions/'+$sessionId)
$null=Assert-CardReturningPath $session $root
$sessionTcl=ConvertTo-CardReturningTclPath $session
$handles=[Collections.Generic.List[IO.FileStream]]::new()
$process=$null
$record=[ordered]@{source_commit='d9d07501330eb3456c79741db9168b0340a09da8';schema_version=1;procedure='card-resident-two-backup-v1';session_id=$sessionId;candidate=$candidate;action=$(if($Program){'Program'}else{'CheckOnly'});status='running';phase='staging';new_backups_created=$false;backup_sha256=$null;hardware_accessed=$false;activation_performed=$false;write_possible=$false;write_started=$false;image_sha256=$expected;canonical_predecessor_sha256=$previous;readback_sha256=$null;started_utc=[DateTime]::UtcNow.ToString('o');error=$null}
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
