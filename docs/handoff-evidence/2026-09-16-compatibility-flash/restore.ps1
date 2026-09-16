#requires -Version 7.0
[CmdletBinding()]
param(
 [Parameter(Mandatory=$true)][ValidatePattern('^[0-9a-f]{32}$')][string]$SessionId,
 [switch]$Program
)
# Defaults to offline validation. -Program explicitly restores the prior 5 MiB.
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../..'))
$validation=Join-Path $root 'firmware/squirrel/card-multi-exit-validation.ps1'
if ((Get-FileHash -LiteralPath $validation).Hash.ToLowerInvariant() -cne '45f41b513371abf6a1317e75d71dc69acda6a44602a833a95cc302c7ec5d8e17') { throw 'Validation helper changed.' }
. $validation
# Exact reviewed source programmer. Populated only after its final offline generation.
$programmerHash='d695ced9bdcd1c4e72616386e52dd5681d4c7c5ca48dc5ca00f4fae4e2152694'
$candidate='target/firmware/squirrel/native-resident/3a704487861c4b0792c93d16cfcb3a11'
$candidateHash='e11170c790a34cb190182a26fb1612efb008f4a6a5aac8cc32d05a5f0f2267c1'
$previous='cd852f9192925f447d856cbe826c2b06ffa1a16ddde1b36d702bcd824a2c61fd'
$assets=@(
 @{Name='openocd.exe';Path='target/firmware/tools/openocd/bin/openocd.exe';Bytes=13664247;Hash='9732b05af7e0f6a05a0051371e49af42515662ad309ddcc87f86f9b434ce96d8'},
 @{Name='proxy.bit';Path='target/firmware/tools/lambda-squirrel/flash_screamer/bscan_spi_xc7a35t.bit';Bytes=261513;Hash='ef8af1e277a7fe556e1ed7ace4680d4993cfc4174616485e1c354793d784b7f6'},
 @{Name='transport.cfg';Path='firmware/squirrel/openocd/card-load-transport.cfg';Bytes=1795;Hash='dd2176b5cb7652aceb2f58ea1aed2d8312e8535ef91939fe8fbf5216b7ef3112'},
 @{Name='backup.cfg';Path='firmware/squirrel/openocd/card-load-backup.cfg';Bytes=425;Hash='fb376387fa53257dfcfe539a22fbeb0c8e12c0c403c6522b1cf21b65bbb059e2'},
 @{Name='restore.cfg';Path='work/compat-flash-2026-09-16/delivery/restore.cfg';Bytes=-1;Hash='eb430382cfeb50d6f4e7264c9167a473c62c2510f1af80642928b29fb307b959'},
 @{Name='validation.ps1';Path='firmware/squirrel/card-multi-exit-validation.ps1';Bytes=9226;Hash='45f41b513371abf6a1317e75d71dc69acda6a44602a833a95cc302c7ec5d8e17'}
)
$sourceRoot=Join-Path $root 'target/firmware/squirrel/card-resident-reviewed-sessions'
$prior=Assert-CardReturningPath (Join-Path $sourceRoot $SessionId) $sourceRoot
$restoreId=[guid]::NewGuid().ToString('N')
$session=Assert-CardReturningPath (Join-Path $root ('target/firmware/squirrel/card-resident-restore-sessions/'+$restoreId)) $root
$sessionTcl=ConvertTo-CardReturningTclPath $session
$handles=[Collections.Generic.List[IO.FileStream]]::new()
$process=$null
$record=[ordered]@{schema_version=1;procedure='card-resident-session-restore-v1';session_id=$restoreId;source_session=$SessionId;candidate=$candidate;action=$(if($Program){'Restore'}else{'CheckOnly'});status='running';phase='source_validation';hardware_accessed=$false;activation_performed=$false;write_possible=$false;write_started=$false;image_sha256=$previous;current_backup_sha256=$null;readback_sha256=$null;started_utc=[DateTime]::UtcNow.ToString('o');error=$null}
function Save-Record {
 $record.updated_utc=[DateTime]::UtcNow.ToString('o')
 $temp=Join-Path $session 'result.json.tmp'
 [IO.File]::WriteAllText($temp,($record|ConvertTo-Json -Depth 8),[Text.UTF8Encoding]::new($false))
 [IO.File]::Move($temp,(Join-Path $session 'result.json'),$true)
}
function Lock-Input([string]$Path,[long]$Bytes,[string]$Hash) {
 $null=Assert-CardReturningPath $Path ''
 $handles.Add([IO.File]::Open($Path,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read))
 Assert-CardReturningFile $Path $Bytes $Hash
}
function Copy-Locked([string]$Source,[string]$Name,[long]$Bytes,[string]$Hash) {
 Lock-Input $Source $Bytes $Hash
 $dest=Join-Path $session $Name
 [IO.File]::Copy($Source,$dest,$false)
 Lock-Input $dest $Bytes $Hash
}
function Run-OpenOcd([string]$Config,[string[]]$Markers) {
 $stdout=Join-Path $session ($Config+'.stdout.log')
 $stderr=Join-Path $session ($Config+'.stderr.log')
 $arguments=@('-c','"gdb_port disabled"','-c','"tcl_port disabled"','-c','"telnet_port disabled"','-c',('"set SESSION {'+$sessionTcl+'}"'),'-f',('"'+(Join-Path $session $Config)+'"'))
 $script:process=Start-Process -FilePath (Join-Path $session 'openocd.exe') -ArgumentList $arguments -WorkingDirectory $session -WindowStyle Hidden -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
 $null=$script:process.Handle
 if(-not $script:process.WaitForExit(1800000)) {$script:process.Kill($true);$script:process.WaitForExit();throw "OpenOCD $Config timed out."}
 $script:process.Refresh()
 $trace=[IO.File]::ReadAllText($stdout)+[IO.File]::ReadAllText($stderr)
 [IO.File]::WriteAllText((Join-Path $session ($Config+'.log')),$trace,[Text.UTF8Encoding]::new($false))
 if($null -eq $script:process.ExitCode -or $script:process.ExitCode -ne 0) {throw "OpenOCD $Config failed with exit $($script:process.ExitCode)."}
 foreach($marker in $Markers) {if(-not $trace.Contains($marker)) {throw "Missing stage marker: $marker"}}
 $script:process.Dispose();$script:process=$null
}
try {
 if($SessionId -cnotmatch '^[0-9a-f]{32}$') {throw 'SessionId must be lowercase hexadecimal.'}
 Assert-CardReturningPin $programmerHash
 if($Program) {
  foreach($relative in @('target/firmware/squirrel/card-multi-exit-sessions','target/firmware/squirrel/card-resident-sessions')) {
   $directory=Assert-CardReturningPath (Join-Path $root $relative) $root
   [IO.Directory]::CreateDirectory($directory)|Out-Null
   $lockPath=Assert-CardReturningPath (Join-Path $directory 'operation.lock') $root
   $handles.Add([IO.File]::Open($lockPath,[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None))
  }
 }
 # Source remains read-only. Validate exact programmer and typed provenance.
 Lock-Input (Join-Path $prior 'procedure.ps1') -1 $programmerHash
 $resultPath=Join-Path $prior 'result.json'
 Lock-Input $resultPath -1 (Get-CardReturningHash $resultPath)
 $result=Get-Content -LiteralPath $resultPath -Raw|ConvertFrom-Json
 Assert-CardReturningFields $result @{schema_version=1;procedure='card-resident-two-backup-v1';session_id=$SessionId;candidate=$candidate;action='Program';image_sha256=$candidateHash;canonical_predecessor_sha256=$previous;hardware_accessed=$true;activation_performed=$false;procedure_sha256=$programmerHash} 'Prior programming session'
 if($result.status -notin @('running','failed','verified_not_activated') -or $result.phase -notin @('programming','readback_verification','complete')) {throw 'Prior session did not reach programming.'}
 Lock-Input (Join-Path $prior 'before-a.bin') 5242880 $previous
 Lock-Input (Join-Path $prior 'before-b.bin') 5242880 $previous
 Lock-Input (Join-Path $prior 'known-working.bin') 5242880 $previous
 # The source cfg pin binds its backup-before-write sequence even after interruption.
 Lock-Input (Join-Path $prior 'program.cfg') -1 '3dd9a46fe96eb0c60c64fcf2ba4e18b5042e17182603d47000bc61046e03897c'
 $trace=''
 foreach($name in @('program.stdout.log','program.stderr.log')) {
  $path=Join-Path $prior $name
  Lock-Input $path -1 (Get-CardReturningHash $path)
  $trace += [IO.File]::ReadAllText($path)+"`n"
 }
 foreach($marker in @('PASS card-target-id-and-geometry','PASS card-reviewed-backups-and-predecessor-verified','BEGIN card-reviewed-write')) {
  if(-not $trace.Contains($marker)) {throw "Source backup admission not evidenced: $marker"}
 }
 # Bind every recorded source asset to the exact hashes embedded in the pinned
 # source procedure; parse AST data only, never execute the historical script.
 $tokens=$null;$errors=$null
 $ast=[Management.Automation.Language.Parser]::ParseFile((Join-Path $prior 'procedure.ps1'),[ref]$tokens,[ref]$errors)
 if($errors.Count) {throw 'Source procedure parse failed.'}
 $assignment=@($ast.FindAll({param($node) $node -is [Management.Automation.Language.AssignmentStatementAst] -and $node.Left.Extent.Text -ceq '$assets'},$true))
 if($assignment.Count -ne 1) {throw 'Source procedure asset table ambiguous.'}
 $sourceAssets=@($assignment[0].Right.FindAll({param($node) $node -is [Management.Automation.Language.HashtableAst]},$true)|ForEach-Object {$_.SafeGetValue()})
 if(@($result.inputs).Count -ne @($sourceAssets).Count) {throw 'Source provenance asset count mismatch.'}
 foreach($asset in $sourceAssets) {
  $recordedInput=@($result.inputs|Where-Object {$_.name -ceq $asset.Name})
  if($recordedInput.Count -ne 1 -or $recordedInput[0].sha256 -cne $asset.Hash) {throw 'Source provenance asset mismatch.'}
  Lock-Input (Join-Path $prior $asset.Name) $asset.Bytes $asset.Hash
 }
 [IO.Directory]::CreateDirectory($session)|Out-Null
 Save-Record
 Copy-Locked $resultPath 'source-result.json' -1 (Get-CardReturningHash $resultPath)
 Copy-Locked (Join-Path $prior 'procedure.ps1') 'source-procedure.ps1' -1 $programmerHash
 foreach($name in @('program.stdout.log','program.stderr.log')) {Copy-Locked (Join-Path $prior $name) ('source-'+$name) -1 (Get-CardReturningHash (Join-Path $prior $name))}
 Copy-Locked (Join-Path $prior 'before-a.bin') 'rollback-a.bin' 5242880 $previous
 Copy-Locked (Join-Path $prior 'before-b.bin') 'rollback-b.bin' 5242880 $previous
 foreach($asset in $assets) {Copy-Locked (Join-Path $root $asset.Path) $asset.Name $asset.Bytes $asset.Hash}
 $callerHash=Get-CardReturningHash $PSCommandPath
 Copy-Locked $PSCommandPath 'procedure.ps1' -1 $callerHash
 $record.procedure_sha256=$callerHash
 $record.source_result_sha256=Get-CardReturningHash (Join-Path $session 'source-result.json')
 $record.inputs=@($assets|ForEach-Object {[ordered]@{name=$_.Name;sha256=$_.Hash}})
 if(-not $Program) {$record.status='offline_pass';$record.phase='complete';Save-Record;Write-Output "PASS restore source offline check: $session";return}
 $record.phase='fresh_backup';$record.hardware_accessed=$true;Save-Record
 Write-Output "Restore session: $session"
 Run-OpenOcd 'backup.cfg' @('PASS card-target-id-and-geometry','PASS card-double-backup')
 # Accept stable partial programming as current state; only rollback has a fixed hash.
 $current=Assert-CardReturningBackups (Join-Path $session 'before-a.bin') (Join-Path $session 'before-b.bin') ''
 Lock-Input (Join-Path $session 'before-a.bin') 5242880 $current
 Lock-Input (Join-Path $session 'before-b.bin') 5242880 $current
 $record.current_backup_sha256=$current;$record.phase='restoring';$record.write_possible=$true;Save-Record
 Run-OpenOcd 'restore.cfg' @('PASS card-target-id-and-geometry','PASS card-restore-prewrite-backups-verified','BEGIN card-reviewed-restore-write','PASS card-reviewed-restore-readback')
 $record.write_started=$true;$record.phase='readback_verification';Save-Record
 Lock-Input (Join-Path $session 'readback.bin') 5242880 $previous
 $record.readback_sha256=$previous;$record.status='verified_not_activated';$record.phase='complete';Save-Record
 Write-Output "PASS prior exact 5 MiB restored and readback verified; no activation. Evidence: $session"
} catch {
 $record.status='failed';$record.error=$_.Exception.Message
 if(Test-Path -LiteralPath $session) {Save-Record}
 throw
} finally {
 if($null -ne $process) {if(-not $process.HasExited) {$process.Kill($true);$process.WaitForExit()};$process.Dispose()}
 try {
  if(Test-Path -LiteralPath $session) {
   $trace=''
   foreach($name in @('restore.cfg.stdout.log','restore.cfg.stderr.log')) {$log=Join-Path $session $name;if(Test-Path -LiteralPath $log) {$trace += [IO.File]::ReadAllText($log)+"`n"}}
   if($trace) {$record.write_started=$trace.Contains('BEGIN card-reviewed-restore-write');[IO.File]::WriteAllText((Join-Path $session 'restore.cfg.log'),$trace,[Text.UTF8Encoding]::new($false));$record.log_sha256=Get-CardReturningHash (Join-Path $session 'restore.cfg.log');Save-Record}
  }
 } finally {foreach($handle in $handles) {$handle.Dispose()}}
}
