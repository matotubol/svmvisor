[CmdletBinding()]
param()
# Offline fixtures only. No fixture can be selected by the production session root.
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../..'))
$entry=Join-Path $root 'firmware/card/card-diagnostics-test.ps1'
$oldEntry=Join-Path $root 'firmware/card/card-returning-test.ps1'
$testRoot=Join-Path $root ('work/card-diagnostics-procedure-tests/'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $testRoot | Out-Null
. (Join-Path $root 'firmware/card/card-returning-validation.ps1')
$script:passed=0
function Pass([string]$Name) { $script:passed++; Write-Output "PASS $Name" }
function Refuses([string]$Name,[scriptblock]$Operation,[string]$Pattern) {
    $caught=$null
    try { & $Operation | Out-Null } catch { $caught=$_.Exception.Message }
    if ($null -eq $caught -or $caught -notmatch $Pattern) { throw "Expected refusal: $Name / $Pattern; received: $caught" }
    Pass $Name
}
function Write-Json([string]$Path,$Object) { [IO.File]::WriteAllText($Path,($Object | ConvertTo-Json -Depth 12),[Text.UTF8Encoding]::new($false)) }
function Parse-Entry([string]$Path) {
    $tokens=$null; $errors=$null
    $parsed=[Management.Automation.Language.Parser]::ParseFile($Path,[ref]$tokens,[ref]$errors)
    if ($errors.Count) { throw "Entry parse errors: $Path" }
    return $parsed
}
function Assignment($Ast,[string]$Name) {
    $node=@($Ast.EndBlock.Statements | Where-Object { $_ -is [Management.Automation.Language.AssignmentStatementAst] -and $_.Left.Extent.Text -ceq ('$'+$Name) })
    if ($node.Count -ne 1) { throw "Expected one assignment: $Name" }
    return $node[0]
}
$ast=Parse-Entry $entry; $oldAst=Parse-Entry $oldEntry
Assert-CardReturningFile $oldEntry -1 '9f8c4b5f041c57fae5d2c95fdeba1b8b7d65a04a71bd5df8f60ee8e5b7e300ed'
Assert-CardReturningFile (Join-Path $root 'firmware/card/card-returning-validation.ps1') -1 'dd19a3fb93b1320db8b2699d860b44cd96fc29676bd324f04e43eb3df2677234'
Pass 'reviewed v1 entry and validator preserved byte for byte'
$allowed=@('candidate','stack','knownWorking','sessionsRoot','session','inputs')
function Algorithm-Statements($Ast) {
    return @($Ast.EndBlock.Statements | Where-Object { -not ($_ -is [Management.Automation.Language.AssignmentStatementAst] -and $_.Left.Extent.Text.Substring(1) -cin $allowed) } | ForEach-Object { $_.Extent.Text }) -join "`n"
}
if ($ast.ParamBlock.Extent.Text -cne $oldAst.ParamBlock.Extent.Text -or (Algorithm-Statements $ast) -cne (Algorithm-Statements $oldAst)) { throw 'Diagnostics changed the reviewed v1 algorithm outside the six permitted constant assignments.' }
Pass 'all parameters, functions and orchestration AST statements unchanged'
if ((Assignment $ast 'knownWorking').Extent.Text -cne "`$knownWorking='dd24babba5a19d60203593ceb806dae37100a461ac1f2808941c89c367ac7a3d'") { throw 'Diagnostics before-image gate is not literal dd24.' }
if ((Assignment $ast 'stack').Extent.Text -cne "`$stack='work/native-stack-audit-diagnostics-20260910-a'") { throw 'Unexpected audit directory.' }
$candidateText=(Assignment $ast 'candidate').Extent.Text
if ($candidateText -cnotin @("`$candidate=''","`$candidate='target/firmware/card/native-returning/80d4c81a79484ba1a04fc1a79af89342'")) { throw 'Unexpected candidate path or fallback.' }
foreach ($name in @('sessionsRoot','session')) {
    $expected=(Assignment $oldAst $name).Extent.Text.Replace('card-returning-sessions','card-diagnostics-sessions').Replace('card-returning-checks','card-diagnostics-checks')
    if ((Assignment $ast $name).Extent.Text -cne $expected) { throw "Session namespace changed beyond its literal name: $name" }
}
Pass 'literal dd24 before-image and distinct diagnostics session/check namespaces'
# Compare the input table after removing only candidate and audit evidence pins.
function Normalize-Inputs([string]$Text) {
    return [regex]::Replace($Text,"(?m)^    @\{Name='(?:combined.bin|configuration.bin|payload-slot.bin|pe-header.bin|native-child.efi|candidate-manifest.json|payload-manifest.json|local-review.json|child-stack-result.json|child-stack-manifest.json|stack-result.json|stack-manifest.json)';[^\r\n]+",{param($m) [regex]::Replace($m.Value,"Bytes=[0-9]+;Hash='[0-9a-f]*'","Bytes=PIN;Hash='PIN'")})
}
if ((Normalize-Inputs (Assignment $ast 'inputs').Extent.Text) -cne (Normalize-Inputs (Assignment $oldAst 'inputs').Extent.Text)) { throw 'Input table changed outside candidate/result Bytes and Hash constants.' }
Pass 'hardware tools, transport, backup, program, restore and audit tool pins unchanged'
Refuses 'unconfirmed diagnostics Program refuses before staging' { & $entry -Action Program } 'explicit -ConfirmFlash'
Refuses 'unconfirmed diagnostics Restore refuses before staging' { & $entry -Action Restore -RestoreSession ('1'*32) } 'explicit -ConfirmFlash'
$validationPin='dd19a3fb93b1320db8b2699d860b44cd96fc29676bd324f04e43eb3df2677234'
. ([scriptblock]::Create((Assignment $ast 'candidate').Extent.Text))
. ([scriptblock]::Create((Assignment $ast 'stack').Extent.Text))
. ([scriptblock]::Create((Assignment $ast 'inputs').Extent.Text))
$hasEmptyPins=@($inputs | Where-Object { -not $_.Hash }).Count -gt 0
if ($hasEmptyPins) {
    Refuses 'unpopulated production pins fail closed offline' { & $entry } 'unpopulated or malformed'
    Refuses 'unpopulated production pins cannot select old rollback sessions' { & $entry -RestoreSession ('1'*32) } 'unpopulated or malformed'
}
$known='dd24babba5a19d60203593ceb806dae37100a461ac1f2808941c89c367ac7a3d'
$baseline=Join-Path $root 'target/firmware/card/native-returning/c9a606dd2e304e04a7a59bb2854de940'
$knownFile=Join-Path $baseline 'combined/combined-review.bin'
$c686=Join-Path $root 'target/firmware/card/endpoint/aee684bd8ead4ec3aa8de384b3f7f5f8/payload/combined-review.bin'
$twoE9=Join-Path $root 'target/firmware/card/card-load-sessions/e3bd32ee8d5d45bdaa5b94c756f8d271/before-a.bin'
Assert-CardReturningFile $knownFile 5242880 $known
Assert-CardReturningFile $c686 5242880 'c686655e362313bb32e5590077dc58a5ed1e6ff549db27cf1f4f55395615d885'
Assert-CardReturningFile $twoE9 5242880 '2e9bda17a815eb2b5ba90bc696928dd569cdbba3e7c8b8c9b641baeb4d622937'
$null=Assert-CardReturningBackups $knownFile $knownFile $known; Pass 'exact full dd24 backup pair admitted'
Refuses 'actual c686 image refused by literal diagnostics before-image gate' { Assert-CardReturningBackups $c686 $c686 $known } 'currently installed known-working'
Refuses 'actual 2e9b historical backup refused by diagnostics before-image gate' { Assert-CardReturningBackups $twoE9 $twoE9 $known } 'currently installed known-working'

# A synthetic configuration differs from dd24, so rollback cannot accidentally
# pass by restoring the fixture candidate. All generated assets stay under work.
$fixtureCandidate=Join-Path $testRoot 'candidate-fixture'
$fixtureStack=Join-Path $testRoot 'stack-fixture'
New-Item -ItemType Directory -Path (Join-Path $fixtureCandidate 'combined'),$fixtureStack | Out-Null
$parts=@{'combined/combined-review.bin'='combined/combined-review.bin';'svmvisor-endpoint.bin'='svmvisor-endpoint.bin';'combined/payload-slot.bin'='combined/payload-slot.bin';'combined/pe-header.bin'='combined/pe-header.bin';'reviewed-child.efi'='reviewed-child.efi'}
foreach ($path in $parts.Keys) { [IO.File]::Copy((Join-Path $baseline $parts[$path]),(Join-Path $fixtureCandidate $path),$false) }
foreach ($path in @('combined/combined-review.bin','svmvisor-endpoint.bin')) {
    $file=Join-Path $fixtureCandidate $path; $bytes=[IO.File]::ReadAllBytes($file); $bytes[100]=$bytes[100] -bxor 1; [IO.File]::WriteAllBytes($file,$bytes)
}
$candidate=[IO.Path]::GetRelativePath($root,$fixtureCandidate).Replace('\','/')
$stack=[IO.Path]::GetRelativePath($root,$fixtureStack).Replace('\','/')
$sourceMarker=Join-Path $testRoot 'source-fixture.txt'; [IO.File]::WriteAllText($sourceMarker,'OFFLINE FIXTURE SOURCE; NEVER HARDWARE')
$objectMarker=Join-Path $fixtureStack 'object-fixture.txt'; [IO.File]::WriteAllText($objectMarker,'OFFLINE FIXTURE OBJECT; NEVER EXECUTE')
$sourcePath=[IO.Path]::GetRelativePath($root,$sourceMarker).Replace('\','/')
Write-Json (Join-Path $fixtureStack 'manifest.json') @{fixture_only=$true;sources=@{$sourcePath=(Get-CardReturningHash $sourceMarker)};artifacts=@{'object-fixture.txt'=(Get-CardReturningHash $objectMarker)}}
Write-Json (Join-Path $fixtureStack 'result.json') @{fixture_only=$true;status='pass';hardware_accessed=$false}
foreach ($pair in @(@('result.json','child-stack-audit-result.json'),@('manifest.json','child-stack-audit-manifest.json'))) { [IO.File]::Copy((Join-Path $fixtureStack $pair[0]),(Join-Path $fixtureCandidate $pair[1]),$false) }
. ([scriptblock]::Create((Assignment $ast 'inputs').Extent.Text))
foreach ($asset in $inputs | Where-Object { $_.Group -eq 'Candidate' -and $_.Name -notin @('candidate-manifest.json','payload-manifest.json','local-review.json') -or $_.Name -in @('stack-result.json','stack-manifest.json') }) {
    $path=Join-Path $root $asset.Path; $asset.Bytes=(Get-Item -LiteralPath $path).Length; $asset.Hash=Get-CardReturningHash $path
}
$fixtureHashes=@{}; foreach ($asset in $inputs) { $fixtureHashes[$asset.Name]=$asset.Hash }
Write-Json (Join-Path $fixtureCandidate 'manifest.json') @{fixture_only=$true;schema_version=1;status='built_review_required';image_kind='native_returning_pe_review';payload_kind='NativeReturning';hardware_accessed=$false;activation_performed=$false;physical_run_ready=$false;payload_sha256=$fixtureHashes['native-child.efi'];combined_sha256=$fixtureHashes['combined.bin'];combined_bytes=5242880;image_sha256=$fixtureHashes['configuration.bin'];slot_sha256=$fixtureHashes['payload-slot.bin'];pin_sha256=$fixtureHashes['pe-header.bin'];target_part='xc7a35tfgg484-2';stack_audit_result_sha256=$fixtureHashes['stack-result.json'];stack_audit_manifest_sha256=$fixtureHashes['stack-manifest.json'];stack_audit_verifier_sha256=$fixtureHashes['stack-verify.py']}
Write-Json (Join-Path $fixtureCandidate 'combined/payload-manifest.json') @{fixture_only=$true;schema_version=1;payload_format='SVMPE001';payload_bytes=51200;payload_sha256=$fixtureHashes['native-child.efi'];combined_sha256=$fixtureHashes['combined.bin'];combined_bytes=5242880;configuration_sha256=$fixtureHashes['configuration.bin'];slot_sha256=$fixtureHashes['payload-slot.bin'];header_sha256=$fixtureHashes['pe-header.bin'];payload_flash_offset=4194304;slot_bytes=1048576;required_parent_feature='card-returning-loader'}
Write-Json (Join-Path $fixtureCandidate 'local-review.json') @{fixture_only=$true;status='pass';scope='returning-programming-offline-review';candidate=$candidate;offline_only=$true;activation_performed=$false;child_sha256=$fixtureHashes['native-child.efi'];combined_sha256=$fixtureHashes['combined.bin'];configuration_sha256=$fixtureHashes['configuration.bin'];slot_sha256=$fixtureHashes['payload-slot.bin'];header_sha256=$fixtureHashes['pe-header.bin'];stack_result_sha256=$fixtureHashes['stack-result.json'];stack_manifest_sha256=$fixtureHashes['stack-manifest.json']}
foreach ($asset in $inputs | Where-Object { $_.Group -eq 'Candidate' }) { $path=Join-Path $root $asset.Path; $asset.Bytes=(Get-Item -LiteralPath $path).Length; $asset.Hash=Get-CardReturningHash $path }
$fixtureInputs=$inputs
$imageHash=$fixtureHashes['combined.bin']
if ($imageHash -ceq $known) { throw 'Fixture candidate must differ from rollback image.' }
$orchestration=@($ast.EndBlock.Statements | Where-Object { $_ -is [Management.Automation.Language.TryStatementAst] })
if ($orchestration.Count -ne 1) { throw 'Expected one orchestration block.' }
# Dynamic scriptblocks have no PSCommandPath; supply the actual production file
# for retained caller identity. No orchestration branches or checks are changed.
$orchestrationBlock=[scriptblock]::Create($orchestration[0].Extent.Text.Replace('$PSCommandPath','$entry'))
$fixtureSessions=Join-Path $testRoot 'orchestration-sessions-fixture'
function Invoke-OrchestrationFixture([string]$Mode,[string]$SourceSession='') {
    $Action=if($Mode -eq 'restore'){'Restore'}elseif($Mode -eq 'restore-check'){'CheckOnly'}else{'Program'}
    $RestoreSession=$SourceSession; $isRestore=[bool]$RestoreSession
    $sessionId=[guid]::NewGuid().ToString('N'); $sessionsRoot=$fixtureSessions
    $session=if($Action -eq 'CheckOnly'){Join-Path $testRoot ('offline-restore-check/'+$sessionId)}else{Join-Path $sessionsRoot $sessionId}
    $sessionTcl=ConvertTo-CardReturningTclPath $session; $knownWorking=$known
    $inputs=$fixtureInputs; . ([scriptblock]::Create((Assignment $ast 'auditTools').Extent.Text))
    $hashes=@{}; foreach($asset in $inputs){Assert-CardReturningPin $asset.Hash; $hashes[$asset.Name]=$asset.Hash}
    $selected=@($inputs | Where-Object { -not $isRestore -or $_.Group -eq 'Hardware' })
    foreach($asset in $selected){Assert-CardReturningFile (Join-Path $root $asset.Path) $asset.Bytes $asset.Hash}
    $prior=if($isRestore){Assert-CardReturningRestoreSource $sessionsRoot $RestoreSession $candidate $imageHash $known}else{$null}
    $locks=[Collections.Generic.List[IO.FileStream]]::new(); $operationLock=$null
    . ([scriptblock]::Create((Assignment $ast 'record').Extent.Text)); $record.fixture_only=$true
    foreach($name in @('Save-Record','Lock-CheckedInput')) {
        $definition=$ast.Find({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -ceq $name},$true)
        . ([scriptblock]::Create($definition.Extent.Text))
    }
    $stageCalls=[Collections.Generic.List[string]]::new()
    function Start-Process { throw 'OFFLINE FIXTURE: subprocess launch forbidden.' }
    function Invoke-BoundedProcess {
        param($Exe,$Arguments,$Name,$Seconds,$Markers)
        if ($isRestore -or $Name -cne 'stack-consumer') { throw 'Unexpected consumer in restore fixture.' }
        $stageCalls.Add('offline-stack-consumer-mock')
    }
    function Invoke-CardStage([string]$Config,[string[]]$Markers) {
        if ($Action -eq 'CheckOnly') { throw 'Offline restore check attempted a hardware stage.' }
        $record.hardware_accessed=$true # Simulated, explicitly fixture_only.
        Save-Record; $stageCalls.Add($Config)
        if($Config -eq 'backup.cfg') {
            $backupSource=if($Mode -eq 'wrong-c686'){$c686}elseif($Mode -eq 'wrong-2e9b'){$twoE9}elseif($Mode -eq 'restore'){$c686}else{$knownFile}
            [IO.File]::Copy($backupSource,(Join-Path $session 'before-a.bin'),$false)
            [IO.File]::Copy($backupSource,(Join-Path $session 'before-b.bin'),$false)
            [IO.File]::WriteAllText((Join-Path $session 'backup.cfg.log'),"OFFLINE FIXTURE ONLY; no hardware`nPASS card-target-id-and-geometry`nPASS card-double-backup`n")
        } elseif ($Config -in @('program.cfg','restore.cfg')) {
            if(-not $record.backup_complete){throw 'Write reached without backup admission.'}
            $stream=$null; $denied=$false
            try{$stream=[IO.File]::Open((Join-Path $session 'before-a.bin'),[IO.FileMode]::Open,[IO.FileAccess]::Write,[IO.FileShare]::ReadWrite)}catch{$denied=$true}finally{if($null -ne $stream){$stream.Dispose()}}
            if(-not $denied){throw 'Write reached without locked fresh backup.'}
            if($Mode -eq 'write-fail'){throw 'Simulated partial programming failure.'}
            $readbackSource=if($Mode -eq 'readback-fail'){$c686}elseif($isRestore){Join-Path $session 'rollback-a.bin'}else{Join-Path $session 'combined.bin'}
            [IO.File]::Copy($readbackSource,(Join-Path $session 'readback.bin'),$false)
        } else {throw 'Unexpected hardware stage.'}
    }
    $caught=$null
    try{. $orchestrationBlock | Out-Null}catch{$caught=$_.Exception.Message}
    $saved=Get-Content -Raw -LiteralPath (Join-Path $session 'result.json') | ConvertFrom-Json
    if($saved.activation_performed -ne $false){throw 'Fixture recorded activation.'}
    if($Mode -in @('wrong-c686','wrong-2e9b')) {
        if($stageCalls -contains 'program.cfg' -or $saved.phase -cne 'backing_up' -or $saved.backup_complete -or $saved.write_attempted){throw 'Wrong before-image reached write/admission.'}
        if($saved.status -cne 'failed' -or $caught -notmatch 'currently installed known-working'){throw 'Wrong before-image failure was not retained.'}
    } elseif($Mode -in @('write-fail','readback-fail')) {
        $wantedPhase=if($Mode -eq 'write-fail'){'programming'}else{'readback_verification'}
        if($saved.phase -cne $wantedPhase -or $saved.status -cne 'failed' -or -not $caught){throw 'Failure lost write/readback phase.'}
        $null=Assert-CardReturningRestoreSource $sessionsRoot $sessionId $candidate $imageHash $known
    } elseif($Mode -eq 'restore-check') {
        if($saved.status -cne 'offline_pass' -or $saved.hardware_accessed -or $stageCalls.Count -ne 0 -or $caught){throw "Offline Restore check failed or accessed a stage: $caught"}
    } else {
        if($saved.status -cne 'verified_not_activated' -or $caught){throw "Expected fixture success: $caught"}
    }
    if($isRestore) {
        if($Mode -eq 'restore' -and ($stageCalls -join ',') -cne 'backup.cfg,restore.cfg'){throw 'Restore invoked candidate consumer/program stage.'}
        $null=Assert-CardReturningBackups (Join-Path $session 'rollback-a.bin') (Join-Path $session 'rollback-b.bin') $known
        if(-not $saved.restore_source_provenance_sha256){throw 'Restore lost original backup provenance.'}
        foreach($name in @('prior-result.json','prior-backup-provenance.json','prior-backup.cfg.log')) { if(-not(Test-Path -LiteralPath (Join-Path $session $name))){throw "Restore lost retained provenance: $name"} }
        if($Mode -eq 'restore') {
            $null=Assert-CardReturningBackups (Join-Path $session 'before-a.bin') (Join-Path $session 'before-b.bin') (Get-CardReturningHash $c686)
            Assert-CardReturningFile (Join-Path $session 'readback.bin') 5242880 $known
        }
    }
    Write-Json (Join-Path $session 'fixture-execution.json') @{fixture_only=$true;actual_hardware_accessed=$false;stage_calls=@($stageCalls);mode=$Mode;error=$caught}
    return $sessionId
}
foreach($mode in @('wrong-c686','wrong-2e9b','readback-fail','success')) { $null=Invoke-OrchestrationFixture $mode; Pass "actual diagnostics orchestration: $mode" }
$partial=Invoke-OrchestrationFixture 'write-fail'; Pass 'actual diagnostics Program partial write retains dd24 rollback provenance'
# Remove only fixture sources from their expected paths. Production sources and
# historical candidates remain untouched. Verify absolute destinations first.
foreach($path in @($fixtureCandidate,$fixtureStack)) {
    $null=Assert-CardReturningPath $path $testRoot; $destination=$path+'-unavailable'; $null=Assert-CardReturningPath $destination $testRoot
    Move-Item -LiteralPath $path -Destination $destination
}
[IO.File]::AppendAllText($sourceMarker,' CHANGED SOURCE FIXTURE')
$null=Invoke-OrchestrationFixture 'restore-check' $partial; Pass 'actual offline Restore validates Program provenance without candidate or audit sources'
$null=Invoke-OrchestrationFixture 'restore' $partial; Pass 'actual Restore writes dd24 and retains fresh failed-state double backup without candidate sources'
$prior=Join-Path $fixtureSessions $partial
[IO.File]::AppendAllText((Join-Path $prior 'backup.cfg.log'),'tampered offline fixture')
Refuses 'offline Restore rejects altered prior Program transcript' { Invoke-OrchestrationFixture 'restore-check' $partial } 'Digest mismatch'
Write-Json (Join-Path $testRoot 'test-result.json') @{status='pass';tests=$script:passed;hardware_accessed=$false;activation_performed=$false;fixture_only=$true;entry_sha256=(Get-CardReturningHash $entry);unpopulated_pins=$hasEmptyPins}
Write-Output "PASS $script:passed focused offline diagnostics procedure tests. Evidence: $testRoot"
