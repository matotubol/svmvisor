[CmdletBinding()]
param()
# Offline only. Retained test records are explicitly fixtures, never sessions
# accepted by the production entry point's fixed card-returning-sessions root.
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../..'))
. (Join-Path $root 'firmware/squirrel/card-returning-validation.ps1')
$entry=Join-Path $root 'firmware/squirrel/card-returning-test.ps1'
$testRoot=Join-Path $root ('work/card-returning-validation-tests/'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $testRoot | Out-Null
$script:passed=0
function Pass([string]$Name) { $script:passed++; Write-Output "PASS $Name" }
function Refuses([string]$Name,[scriptblock]$Operation,[string]$Pattern) {
    $caught=$null
    try { & $Operation | Out-Null } catch { $caught=$_.Exception.Message }
    if ($null -eq $caught -or $caught -notmatch $Pattern) { throw "Expected refusal: $Name / $Pattern; received: $caught" }
    Pass $Name
}
function Write-Json([string]$Path,$Object) { [IO.File]::WriteAllText($Path,($Object | ConvertTo-Json -Depth 8),[Text.UTF8Encoding]::new($false)) }
Refuses 'entry refuses unconfirmed Program before pins or tools' { & $entry -Action Program } 'explicit -ConfirmFlash'
Refuses 'entry refuses unconfirmed Restore before pins or tools' { & $entry -Action Restore -RestoreSession ('1'*32) } 'explicit -ConfirmFlash'
foreach ($id in @('../outside',(('1'*32)+'/child'),'C:\absolute',('A'*32))) {
    Refuses "session path rejected: $id" { & $entry -RestoreSession $id } '32-hex session ID'
}
Refuses 'Restore requires session' { Assert-CardReturningAction 'Restore' $true '' } 'prior returning Program'
Refuses 'Program rejects restore selection' { Assert-CardReturningAction 'Program' $true ('1'*32) } 'cannot select'
foreach ($pin in @('',('0'*64),'abc',('A'*64))) { Refuses 'missing/malformed exact pin' { Assert-CardReturningPin $pin } 'unpopulated or malformed' }
Refuses 'lexical escape' { Assert-CardReturningPath (Join-Path $testRoot '../escape.bin') $testRoot } 'escapes'
Refuses 'sibling prefix escape' { Assert-CardReturningPath ($testRoot+'-sibling/file.bin') $testRoot } 'escapes'
Refuses 'boolean string review rejected' { Assert-CardReturningFields ([pscustomobject]@{offline_only='True'}) @{offline_only=$true} 'Review' } 'field mismatch'
Refuses 'missing review field rejected' { Assert-CardReturningFields ([pscustomobject]@{}) @{status='pass'} 'Review' } 'field mismatch'

$known='c686655e362313bb32e5590077dc58a5ed1e6ff549db27cf1f4f55395615d885'
$knownFile=Join-Path $root 'target/firmware/squirrel/endpoint/aee684bd8ead4ec3aa8de384b3f7f5f8/payload/combined-review.bin'
Assert-CardReturningFile $knownFile 5242880 $known
$zero=Join-Path $testRoot 'zero-5mib.bin'; [IO.File]::WriteAllBytes($zero,[byte[]]::new(5242880))
$short=Join-Path $testRoot 'short.bin'; [IO.File]::WriteAllBytes($short,[byte[]]::new(5242879))
Refuses 'wrong backup extent' { Assert-CardReturningBackups $knownFile $short $known } 'exactly 5 MiB'
Refuses 'different full backup reads' { Assert-CardReturningBackups $knownFile $zero $known } 'Digest mismatch'
Refuses 'matching full backups from wrong installed image' { Assert-CardReturningBackups $zero $zero $known } 'currently installed known-working'
Refuses 'file hash mismatch' { Assert-CardReturningFile $zero 5242880 $known } 'Digest mismatch'
$oldBackup=Join-Path $root 'target/firmware/squirrel/card-load-sessions/e3bd32ee8d5d45bdaa5b94c756f8d271/before-a.bin'
Assert-CardReturningFile $oldBackup 5242880 '2e9bda17a815eb2b5ba90bc696928dd569cdbba3e7c8b8c9b641baeb4d622937'
Refuses 'historical 2e9b backups cannot restore current working image' { Assert-CardReturningBackups $oldBackup $oldBackup $known } 'currently installed known-working'
$null=Assert-CardReturningBackups $knownFile $knownFile $known; Pass 'exact installed working full extent accepted'

$candidate='target/firmware/squirrel/native-returning/c9a606dd2e304e04a7a59bb2854de940'
$imageHash='dd24babba5a19d60203593ceb806dae37100a461ac1f2808941c89c367ac7a3d'
$id='1'*32; $sessions=Join-Path $testRoot 'fixture-sessions'; $prior=Join-Path $sessions $id
New-Item -ItemType Directory -Path $prior | Out-Null
foreach ($name in @('before-a.bin','before-b.bin')) { [IO.File]::Copy($knownFile,(Join-Path $prior $name),$false) }
$log=Join-Path $prior 'backup.cfg.log'
[IO.File]::WriteAllText($log,"OFFLINE TEST FIXTURE ONLY; no hardware`nPASS card-target-id-and-geometry`nPASS card-double-backup`n")
$provenance=[ordered]@{fixture_only=$true;schema_version=1;procedure='card-returning-v1';session_id=$id;action='Program';candidate=$candidate;image_sha256=$imageHash;bytes=5242880;first_file='before-a.bin';second_file='before-b.bin';first_sha256=$known;second_sha256=$known;known_working_match=$true;flash_verify_completed=$true;backup_config_sha256='45adafe304102dfbdf4f468d9ca857b2d2b134f299025c18307d7b3f6cfc75cf';transport_sha256='dd2176b5cb7652aceb2f58ea1aed2d8312e8535ef91939fe8fbf5216b7ef3112';log_sha256=(Get-CardReturningHash $log)}
$provenancePath=Join-Path $prior 'backup-provenance.json'; Write-Json $provenancePath $provenance
$result=[ordered]@{fixture_only=$true;schema_version=1;procedure='card-returning-v1';session_id=$id;action='Program';candidate=$candidate;image_sha256=$imageHash;activation_performed=$false;backup_complete=$true;backup_sha256=$known;backup_bytes=5242880;hardware_accessed=$true;status='failed';phase='programming';backup_provenance_sha256=(Get-CardReturningHash $provenancePath)}
$resultPath=Join-Path $prior 'result.json'; Write-Json $resultPath $result
function Check-RestoreFixture { $null=Assert-CardReturningRestoreSource $sessions $id $candidate $imageHash $known }
Check-RestoreFixture; Pass 'partial programming failure retains usable exact rollback'
foreach($variant in @('program','PROGRAM','pRoGrAm')) {
    $result.action=ConvertTo-CardReturningAction $variant; Write-Json $resultPath $result
    Check-RestoreFixture; Pass "case-variant $variant produces restorable Program provenance"
}
$result.status='running'; Write-Json $resultPath $result; Check-RestoreFixture; Pass 'interrupted process during programming retains usable rollback'
$result.phase='backing_up'; Write-Json $resultPath $result
Refuses 'incomplete backup phase cannot restore' { Check-RestoreFixture } 'recoverable backup admission'
$result.phase='programming'; $result.action='Restore'; Write-Json $resultPath $result
Refuses 'restore session cannot masquerade as prior Program' { Check-RestoreFixture } 'field mismatch: action'
$result.action='Program'; $result.candidate='wrong-candidate'; Write-Json $resultPath $result
Refuses 'other candidate session refused' { Check-RestoreFixture } 'field mismatch: candidate'
$result.candidate=$candidate; Write-Json $resultPath $result
[IO.File]::AppendAllText($log,'tampered')
Refuses 'altered backup transcript refused' { Check-RestoreFixture } 'Digest mismatch'
$provenance.log_sha256=Get-CardReturningHash $log; $provenance.flash_verify_completed=$false
Write-Json $provenancePath $provenance; $result.backup_provenance_sha256=Get-CardReturningHash $provenancePath; Write-Json $resultPath $result
Refuses 'unverified backup provenance refused' { Check-RestoreFixture } 'field mismatch: flash_verify_completed'
$provenance.flash_verify_completed=$true; Write-Json $provenancePath $provenance; $result.backup_provenance_sha256=Get-CardReturningHash $provenancePath; Write-Json $resultPath $result
[IO.File]::Copy($zero,(Join-Path $prior 'before-b.bin'),$true)
Refuses 'prior backup changed since provenance refuses rollback' { Check-RestoreFixture } 'Digest mismatch'

$layout=Join-Path $testRoot 'layout'; New-Item -ItemType Directory -Path $layout | Out-Null
$parts=@{'combined.bin'='combined/combined-review.bin';'configuration.bin'='svmvisor-endpoint.bin';'payload-slot.bin'='combined/payload-slot.bin';'native-child.efi'='reviewed-child.efi';'pe-header.bin'='combined/pe-header.bin'}
$hashes=@{}
foreach($name in $parts.Keys) { $source=Join-Path (Join-Path $root $candidate) $parts[$name]; [IO.File]::Copy($source,(Join-Path $layout $name),$false); $hashes[$name]=Get-CardReturningHash $source }
Assert-CardReturningLayout $layout $hashes; Pass 'exact built configuration/gap/slot/header/child layout'
$combinedPath=Join-Path $layout 'combined.bin'; $bytes=[IO.File]::ReadAllBytes($combinedPath); $bytes[2000000]=0; [IO.File]::WriteAllBytes($combinedPath,$bytes)
Refuses 'erased gap corruption refused' { Assert-CardReturningLayout $layout $hashes } 'padding is not erased'
$bytes[2000000]=255; $bytes[4194304+128]=0; [IO.File]::WriteAllBytes($combinedPath,$bytes)
Refuses 'slot mismatch in combined refused' { Assert-CardReturningLayout $layout $hashes } 'component mismatch'

# Execute the actual bounded-process function, extracted from the production
# AST. The only child programs are a tiny offline PowerShell test script.
$tokens=$null; $errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile($entry,[ref]$tokens,[ref]$errors)
if ($errors.Count) { throw 'Production entry point has parse errors.' }
$function=$ast.Find({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Invoke-BoundedProcess'},$true)
. ([scriptblock]::Create($function.Extent.Text))
$session=Join-Path $testRoot 'processes'; New-Item -ItemType Directory -Path $session | Out-Null
$child=Join-Path $session 'offline-child.ps1'
[IO.File]::WriteAllText($child,"param([string]`$Mode)`nif (`$Mode -eq 'fail') { exit 7 }`nif (`$Mode -eq 'wait') { Start-Sleep -Seconds 5 }`nWrite-Output 'PASS offline-child'`n")
$shell=(Get-Process -Id $PID).Path
Invoke-BoundedProcess $shell @('-NoProfile','-File',('"'+$child+'"'),'pass') 'pass' 10 @('PASS offline-child'); Pass 'actual bounded process success'
Refuses 'actual bounded process nonzero exit' { Invoke-BoundedProcess $shell @('-NoProfile','-File',('"'+$child+'"'),'fail') 'exit' 10 @() } 'exit=7'
Refuses 'actual bounded process missing marker' { Invoke-BoundedProcess $shell @('-NoProfile','-File',('"'+$child+'"'),'pass') 'marker' 10 @('missing-marker') } 'Missing stage success marker'
Refuses 'actual bounded process timeout kills child' { Invoke-BoundedProcess $shell @('-NoProfile','-File',('"'+$child+'"'),'wait') 'timeout' 1 @() } 'timed out'
if (-not (Test-Path -LiteralPath (Join-Path $session 'timeout.log'))) { throw 'Timeout transcript was not retained.' }
Pass 'timeout transcript retained'

# Execute the production try/catch/finally AST without changing its control
# flow. Only external stages are mocked; real staging, locks, manifest/layout
# checks, backup checks, admission/provenance, failure phases and final readback
# hashing execute. Every output is constrained to this work-only fixture root.
$orchestration=$ast.EndBlock.Statements | Where-Object { $_ -is [Management.Automation.Language.TryStatementAst] }
if (@($orchestration).Count -ne 1) { throw 'Expected one production orchestration try block.' }
$assignments=@{}
foreach($name in @('inputs','auditTools','record')) {
    $node=$ast.EndBlock.Statements | Where-Object { $_ -is [Management.Automation.Language.AssignmentStatementAst] -and $_.Left.Extent.Text -ceq ('$'+$name) }
    if(@($node).Count -ne 1){throw "Expected one production assignment: $name"}
    $assignments[$name]=[scriptblock]::Create($node.Extent.Text)
}
# Dynamic scriptblocks have no PSCommandPath; bind that automatic identity to
# the unchanged production entry file. No branch or validation is replaced.
$orchestrationBlock=[scriptblock]::Create($orchestration.Extent.Text.Replace('$PSCommandPath','$entry'))
$fixtureSessions=Join-Path $testRoot 'orchestration-fixtures'
function Invoke-OrchestrationFixture([string]$Mode,[string]$SourceSession='') {
    $Action=if($Mode -eq 'restore'){'Restore'}else{'Program'}
    $isRestore=($Action -eq 'Restore'); $RestoreSession=$SourceSession
    $sessionId=[guid]::NewGuid().ToString('N'); $sessionsRoot=$fixtureSessions
    $session=Join-Path $sessionsRoot $sessionId; $sessionTcl=ConvertTo-CardReturningTclPath $session
    $stack='work/native-stack-audit-final-20260910-d'; $knownWorking=$known
    $validationPin=Get-CardReturningHash (Join-Path $root 'firmware/squirrel/card-returning-validation.ps1')
    . $assignments['inputs']; . $assignments['auditTools']
    $hashes=@{}; foreach($asset in $inputs){$hashes[$asset.Name]=$asset.Hash}
    $reviewFixture=Join-Path $testRoot ('fixture-review-'+$sessionId+'.json')
    Write-Json $reviewFixture @{fixture_only=$true;status='pass';scope='returning-programming-offline-review';candidate=$candidate;offline_only=$true;activation_performed=$false;child_sha256=$hashes['native-child.efi'];combined_sha256=$hashes['combined.bin'];configuration_sha256=$hashes['configuration.bin'];slot_sha256=$hashes['payload-slot.bin'];header_sha256=$hashes['pe-header.bin'];stack_result_sha256=$hashes['stack-result.json'];stack_manifest_sha256=$hashes['stack-manifest.json']}
    $reviewAsset=$inputs | Where-Object Name -eq 'local-review.json'
    $reviewAsset.Path=[IO.Path]::GetRelativePath($root,$reviewFixture); $reviewAsset.Hash=Get-CardReturningHash $reviewFixture
    $hashes['local-review.json']=$reviewAsset.Hash
    $selected=@($inputs | Where-Object { -not $isRestore -or $_.Group -eq 'Hardware' })
    $prior=if($isRestore){Assert-CardReturningRestoreSource $sessionsRoot $RestoreSession $candidate $imageHash $known}else{$null}
    $locks=[Collections.Generic.List[IO.FileStream]]::new(); $operationLock=$null
    . $assignments['record']; $record.fixture_only=$true
    foreach($name in @('Save-Record','Lock-CheckedInput')) {
        $definition=$ast.Find({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -ceq $name},$true)
        . ([scriptblock]::Create($definition.Extent.Text))
    }
    $stageCalls=[Collections.Generic.List[string]]::new()
    # Defense in depth: no subprocess is permitted anywhere in this fixture.
    function Start-Process { throw 'OFFLINE FIXTURE: subprocess launch forbidden.' }
    function Invoke-BoundedProcess {
        param($Exe,$Arguments,$Name,$Seconds,$Markers)
        if ($isRestore -or $Name -cne 'stack-consumer') { throw 'Unexpected offline consumer in restore fixture.' }
        $stageCalls.Add('offline-stack-consumer-mock')
    }
    function Invoke-CardStage([string]$Config,[string[]]$Markers) {
        $record.hardware_accessed=$true # Simulated in a clearly marked fixture.
        Save-Record; $stageCalls.Add($Config)
        if($Config -eq 'backup.cfg') {
            $backupSource=if($Mode -in @('wrong-era','restore')){$zero}else{$knownFile}
            [IO.File]::Copy($backupSource,(Join-Path $session 'before-a.bin'),$false)
            if($Mode -eq 'backup-fail'){throw 'Simulated second backup failure.'}
            [IO.File]::Copy($backupSource,(Join-Path $session 'before-b.bin'),$false)
            [IO.File]::WriteAllText((Join-Path $session 'backup.cfg.log'),"OFFLINE FIXTURE; no hardware`nPASS card-target-id-and-geometry`nPASS card-double-backup`n")
        } elseif ($Config -in @('program.cfg','restore.cfg')) {
            if(-not $record.backup_complete){throw 'Write reached without backup admission.'}
            $stream=$null; $denied=$false
            try{$stream=[IO.File]::Open((Join-Path $session 'before-a.bin'),[IO.FileMode]::Open,[IO.FileAccess]::Write,[IO.FileShare]::ReadWrite)}catch{$denied=$true}finally{if($null -ne $stream){$stream.Dispose()}}
            if(-not $denied){throw 'Write reached without locked fresh backup.'}
            if($Mode -eq 'write-fail'){throw 'Simulated partial erase/program failure.'}
            $readbackSource=if($Mode -eq 'readback-fail'){$zero}elseif($isRestore){Join-Path $session 'rollback-a.bin'}else{Join-Path $session 'combined.bin'}
            [IO.File]::Copy($readbackSource,(Join-Path $session 'readback.bin'),$false)
        } else {throw 'Unexpected hardware stage.'}
    }
    $caught=$null
    try{. $orchestrationBlock | Out-Null}catch{$caught=$_.Exception.Message}
    $saved=Get-Content -Raw -LiteralPath (Join-Path $session 'result.json') | ConvertFrom-Json
    if($saved.activation_performed -ne $false){throw 'Fixture recorded activation.'}
    if($Mode -in @('wrong-era','backup-fail')) {
        if($stageCalls -contains 'program.cfg' -or $saved.phase -cne 'backing_up' -or $saved.backup_complete -or $saved.write_attempted){throw 'Backup refusal reached write/admission.'}
        if($saved.status -cne 'failed' -or -not $caught){throw 'Backup failure was not retained.'}
    } elseif($Mode -in @('write-fail','readback-fail')) {
        $wantedPhase=if($Mode -eq 'write-fail'){'programming'}else{'readback_verification'}
        if($saved.phase -cne $wantedPhase -or $saved.status -cne 'failed' -or -not $caught){throw 'Failure lost its write/readback phase.'}
        $null=Assert-CardReturningRestoreSource $sessionsRoot $sessionId $candidate $imageHash $known
    } else {
        if($saved.status -cne 'verified_not_activated' -or $caught){throw "Expected fixture success: $caught"}
    }
    if($Mode -eq 'restore') {
        if(($stageCalls -join ',') -cne 'backup.cfg,restore.cfg'){throw 'Restore invoked candidate consumer/program stage.'}
        $null=Assert-CardReturningBackups (Join-Path $session 'before-a.bin') (Join-Path $session 'before-b.bin') (Get-CardReturningHash $zero)
        $null=Assert-CardReturningBackups (Join-Path $session 'rollback-a.bin') (Join-Path $session 'rollback-b.bin') $known
        Assert-CardReturningFile (Join-Path $session 'readback.bin') 5242880 $known
        if(-not $saved.restore_source_provenance_sha256){throw 'Restore lost original backup provenance.'}
    }
    Write-Json (Join-Path $session 'fixture-execution.json') @{fixture_only=$true;actual_hardware_accessed=$false;stage_calls=@($stageCalls);mode=$Mode;error=$caught}
    return $sessionId
}
foreach($mode in @('wrong-era','backup-fail','readback-fail','success')) {
    $null=Invoke-OrchestrationFixture $mode; Pass "actual orchestration: $mode"
}
$partial=Invoke-OrchestrationFixture 'write-fail'; Pass 'actual orchestration: partial write failure retains rollback'
$null=Invoke-OrchestrationFixture 'restore' $partial; Pass 'actual orchestration: independent full rollback retains failed-state double backup'

Write-Json (Join-Path $testRoot 'test-result.json') @{status='pass';tests=$script:passed;hardware_accessed=$false;activation_performed=$false;fixture_only=$true}
Write-Output "PASS $script:passed offline returning procedure tests. Evidence: $testRoot"
