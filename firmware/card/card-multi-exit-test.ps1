[CmdletBinding(SupportsShouldProcess, ConfirmImpact='Medium')]
param(
    [ValidateSet('CheckOnly','Program','Restore')][string]$Action='CheckOnly',
    [switch]$ConfirmFlash,
    [string]$RestoreSession
)
# Offline by default. No image/tool/path overrides, download, or activation.
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
if ($Action -ne 'CheckOnly' -and -not $ConfirmFlash) { throw "$Action requires explicit -ConfirmFlash." }
if ($RestoreSession -and $RestoreSession -cnotmatch '^[0-9a-f]{32}$') { throw 'RestoreSession must be a lowercase 32-hex session ID, never a path.' }
$validationPin='1e311fe8a650c63bb2ddddd4a2003bb70a005ea76714ed741d1edb5be71e4005'
$validation=Join-Path $PSScriptRoot 'card-multi-exit-validation.ps1'
if ($validationPin -cnotmatch '^[0-9a-f]{64}$') { throw 'Required exact returning validation pin is unpopulated; review is incomplete.' }
if ((Get-FileHash -LiteralPath $validation).Hash.ToLowerInvariant() -cne $validationPin) { throw 'Returning validation procedure changed.' }
. $validation
$Action=ConvertTo-CardReturningAction $Action
Assert-CardReturningAction $Action ([bool]$ConfirmFlash) $RestoreSession
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$candidate='target/firmware/card/native-returning/c8773e334da54b40aa2acab15beee1e5'
$stack='work/native-stack-audit-multi-exit-20260912-b'
$knownWorking='0d59fd3fd684adb95bf69e6918883cb55111a5248b89bbd60f9dd762f3297c2f'
$sessionsRoot=Join-Path $root 'target/firmware/card/card-multi-exit-sessions'
$isRestore=($Action -eq 'Restore' -or [bool]$RestoreSession)
# Final candidate is exact; local-review pin remains empty until independent review.
$inputs=@(
    @{Name='combined.bin';Path="$candidate/combined/combined-review.bin";Bytes=5242880;Hash='4d3d738898a4e49b098a4ae7f940894c7a7c9c4a38e1607345c3551ce1e3efc7';Group='Candidate'},
    @{Name='configuration.bin';Path="$candidate/svmvisor-endpoint.bin";Bytes=1171404;Hash='642bd59a55997010e6ecda0d1cdef420892c656d55a6475591089e34e2fce1e3';Group='Candidate'},
    @{Name='payload-slot.bin';Path="$candidate/combined/payload-slot.bin";Bytes=1048576;Hash='686650a101a273e05a3639b5e08481d45e60cb871cef3f8c836826b985435245';Group='Candidate'},
    @{Name='pe-header.bin';Path="$candidate/combined/pe-header.bin";Bytes=128;Hash='f876d5a0d13d065b91e1b327aca06193b11766c9a100cfe1c9d43852f2f06947';Group='Candidate'},
    @{Name='native-child.efi';Path="$candidate/reviewed-child.efi";Bytes=59904;Hash='287c29c9c2f2832a45b05b7c24bdf4f9e48fa5f48846fbbebb3d6b00d459b0dd';Group='Candidate'},
    @{Name='candidate-manifest.json';Path="$candidate/manifest.json";Bytes=34100;Hash='62c2bda32252da9bba0ba44aa64484f049a4771194e8899e5575b25d4566ccbc';Group='Candidate'},
    @{Name='payload-manifest.json';Path="$candidate/combined/payload-manifest.json";Bytes=1285;Hash='7a114ec0f122edce4154261aba964b98579321025734a48448625d6bc98c80f7';Group='Candidate'},
    @{Name='local-review.json';Path="$candidate/local-review.json";Bytes=9111;Hash='4dad838c4b95c1421b5f5c62a36c99e359f7eec8868618387c3b1ae1b911d94a';Group='Candidate'},
    @{Name='child-stack-result.json';Path="$candidate/child-stack-audit-result.json";Bytes=48794;Hash='181e41c7b49147077e4fc382da65f58655913625fe069e26a53da2865b80b124';Group='Candidate'},
    @{Name='child-stack-manifest.json';Path="$candidate/child-stack-audit-manifest.json";Bytes=164925;Hash='75e9675360bf08feebe9855195c5b6e8ce9a77da78d65f3f4ff1bd7f4b24935c';Group='Candidate'},
    @{Name='openocd.exe';Path='target/firmware/tools/openocd/bin/openocd.exe';Bytes=13664247;Hash='9732b05af7e0f6a05a0051371e49af42515662ad309ddcc87f86f9b434ce96d8';Group='Hardware'},
    @{Name='proxy.bit';Path='target/firmware/tools/lambda-squirrel/flash_screamer/bscan_spi_xc7a35t.bit';Bytes=261513;Hash='ef8af1e277a7fe556e1ed7ace4680d4993cfc4174616485e1c354793d784b7f6';Group='Hardware'},
    @{Name='transport.cfg';Path='firmware/card/openocd/card-load-transport.cfg';Bytes=-1;Hash='dd2176b5cb7652aceb2f58ea1aed2d8312e8535ef91939fe8fbf5216b7ef3112';Group='Hardware'},
    @{Name='backup.cfg';Path='firmware/card/openocd/card-load-backup.cfg';Bytes=-1;Hash='45adafe304102dfbdf4f468d9ca857b2d2b134f299025c18307d7b3f6cfc75cf';Group='Hardware'},
    @{Name='program.cfg';Path='firmware/card/openocd/card-load-program.cfg';Bytes=-1;Hash='ce1a5bc3674c9208387ada7bc5c4149c9588e3e6463e425330824d7f045cdb3c';Group='Hardware'},
    @{Name='restore.cfg';Path='firmware/card/openocd/card-returning-restore.cfg';Bytes=1195;Hash='a38b0aabe7635810bbe19a0910abd9ffdf1c70b2e0959ee95ee91a0446f86ba1';Group='Hardware'},
    @{Name='validation.ps1';Path='firmware/card/card-multi-exit-validation.ps1';Bytes=9091;Hash=$validationPin;Group='Hardware'},
    @{Name='stack-verify.py';Path='tools/native-stack-audit/verify.py';Bytes=4012;Hash='1d1a7956995ea64a0f06b27dc234a1d498804be08504358f21fa3c609f65a6cf';Group='Audit'},
    @{Name='stack-run.py';Path='tools/native-stack-audit/run.py';Bytes=12499;Hash='463ad74d6921c66dc486bb5e6e4bca8316291f49f22de7c83507b54731ee0cf1';Group='Audit'},
    @{Name='stack-audit.py';Path='tools/native-stack-audit/audit.py';Bytes=32112;Hash='5bd320651ae27e40d89c87eaabc010c5e514ab164801e85bae6a449ce7d0cfd6';Group='Audit'},
    @{Name='stack-result.json';Path="$stack/result.json";Bytes=48794;Hash='181e41c7b49147077e4fc382da65f58655913625fe069e26a53da2865b80b124';Group='Audit'},
    @{Name='stack-manifest.json';Path="$stack/manifest.json";Bytes=164925;Hash='75e9675360bf08feebe9855195c5b6e8ce9a77da78d65f3f4ff1bd7f4b24935c';Group='Audit'}
)
$auditTools=@(
    @{Path='C:/Users/mato/AppData/Local/Programs/Python/Python313/python.exe';Bytes=105696;Hash='85b71d8c6ec1905935f74be0c9869aae198d00e98f39df699ec66f9c5a84cecd'},
    @{Path='C:/Program Files/LLVM/bin/llvm-objdump.exe';Bytes=21094400;Hash='bfdc5de7c990d201a1393dea95a9dae6dac2dd42f50f5f367b215ec73ae1f5aa'},
    @{Path='C:/Program Files/LLVM/bin/llvm-ar.exe';Bytes=18453504;Hash='80934e8f208a0cc2a87a6057f871d0f492461952b8672464749a6c3dff34109c'}
)
$hashes=@{}; foreach ($asset in $inputs) { Assert-CardReturningPin $asset.Hash; $hashes[$asset.Name]=$asset.Hash }
$selected=@($inputs | Where-Object { -not $isRestore -or $_.Group -eq 'Hardware' })
foreach ($asset in $selected) { Assert-CardReturningFile (Join-Path $root $asset.Path) $asset.Bytes $asset.Hash }
if (-not $isRestore) { foreach ($tool in $auditTools) { Assert-CardReturningFile $tool.Path $tool.Bytes $tool.Hash } }
$prior=$null
if ($isRestore) { $prior=Assert-CardReturningRestoreSource $sessionsRoot $RestoreSession $candidate $hashes['combined.bin'] $knownWorking }
if ($Action -ne 'CheckOnly' -and -not $PSCmdlet.ShouldProcess('Squirrel XC7A35T / IS25LP256D sectors 0..79', "$Action returning image: load BSCAN proxy, double-backup 5 MiB, verify current contents, exact 80-sector write and full readback; no activation")) { return }
$sessionId=[guid]::NewGuid().ToString('N')
$session=if ($Action -eq 'CheckOnly') { Join-Path $root ('work/card-multi-exit-checks/'+$sessionId) } else { Join-Path $sessionsRoot $sessionId }
$null=Assert-CardReturningPath $session $root
$sessionTcl=ConvertTo-CardReturningTclPath $session
$locks=[Collections.Generic.List[IO.FileStream]]::new()
$operationLock=$null
$record=[ordered]@{schema_version=1;procedure='card-returning-v1';session_id=$sessionId;action=$Action;candidate=$candidate;status='running';phase='staging';hardware_accessed=$false;activation_performed=$false;write_attempted=$false;backup_bytes=5242880;backup_complete=$false;backup_sha256=$null;backup_provenance_sha256=$null;image_sha256=$hashes['combined.bin'];restore_source_session=$RestoreSession;restore_source_provenance_sha256=$null;programmed_sha256=$(if($isRestore){$knownWorking}else{$hashes['combined.bin']});started_utc=[DateTime]::UtcNow.ToString('o');error=$null}
function Save-Record {
    $record.updated_utc=[DateTime]::UtcNow.ToString('o')
    $temp=Join-Path $session 'result.json.tmp'
    [IO.File]::WriteAllText($temp,($record | ConvertTo-Json -Depth 8),[Text.UTF8Encoding]::new($false))
    [IO.File]::Move($temp,(Join-Path $session 'result.json'),$true)
}
function Lock-CheckedInput([string]$Path,[long]$Bytes,[string]$Hash) {
    $null=Assert-CardReturningPath $Path ''
    $locks.Add([IO.File]::Open($Path,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read))
    Assert-CardReturningFile $Path $Bytes $Hash
}
function Invoke-BoundedProcess([string]$Exe,[string[]]$Arguments,[string]$Name,[int]$Seconds,[string[]]$Markers) {
    $stdout=Join-Path $session ($Name+'.stdout.log'); $stderr=Join-Path $session ($Name+'.stderr.log')
    $process=$null
    try {
        $process=Start-Process -FilePath $Exe -ArgumentList $Arguments -WorkingDirectory $session -PassThru -WindowStyle Hidden -RedirectStandardOutput $stdout -RedirectStandardError $stderr
        $null=$process.Handle
        if (-not $process.WaitForExit($Seconds*1000)) { $process.Kill($true); $process.WaitForExit(); throw "Stage timed out after $Seconds seconds: $Name" }
        $process.Refresh(); $exitCode=$process.ExitCode
        if ($null -eq $exitCode -or $exitCode -ne 0) { throw "Stage failed: $Name; exit=$exitCode" }
    } finally {
        if ($null -ne $process) {
            if (-not $process.HasExited) { $process.Kill($true); $process.WaitForExit() }
            $process.Dispose()
        }
        $trace=''; foreach($path in @($stdout,$stderr)) { if(Test-Path -LiteralPath $path){$trace += [IO.File]::ReadAllText($path)+"`n"} }
        [IO.File]::WriteAllText((Join-Path $session ($Name+'.log')),$trace,[Text.UTF8Encoding]::new($false))
    }
    foreach ($marker in $Markers) { if ($trace -notmatch [regex]::Escape($marker)) { throw "Missing stage success marker: $Name / $marker" } }
}
function Invoke-CardStage([string]$Config,[string[]]$Markers) {
    $record.hardware_accessed=$true; Save-Record
    $arguments=@('-c','"gdb_port disabled"','-c','"tcl_port disabled"','-c','"telnet_port disabled"','-c',('"set SESSION {'+$sessionTcl+'}"'),'-f',('"'+(Join-Path $session $Config)+'"'))
    Invoke-BoundedProcess (Join-Path $session 'openocd.exe') $arguments $Config 1800 (@('PASS card-target-id-and-geometry')+$Markers)
}
try {
    if ($Action -ne 'CheckOnly') {
        $null=Assert-CardReturningPath $sessionsRoot $root
        New-Item -ItemType Directory -Force -Path $sessionsRoot | Out-Null
        $lockPath=Assert-CardReturningPath (Join-Path $sessionsRoot 'operation.lock') $sessionsRoot
        $operationLock=[IO.File]::Open($lockPath,[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
    }
    New-Item -ItemType Directory -Path $session | Out-Null
    Save-Record
    foreach ($asset in $selected) {
        $source=Join-Path $root $asset.Path
        Lock-CheckedInput $source $asset.Bytes $asset.Hash
        $dest=Join-Path $session $asset.Name
        [IO.File]::Copy($source,$dest,$false)
        Lock-CheckedInput $dest $asset.Bytes $asset.Hash
    }
    # Retain the actual caller for session provenance; authority is its reviewed
    # checked-in pins, not a self-referential digest embedded in that caller.
    $callerHash=Get-CardReturningHash $PSCommandPath
    Lock-CheckedInput $PSCommandPath -1 $callerHash
    [IO.File]::Copy($PSCommandPath,(Join-Path $session 'procedure.ps1'),$false)
    Lock-CheckedInput (Join-Path $session 'procedure.ps1') -1 $callerHash
    $record.procedure_sha256=$callerHash
    if ($isRestore) {
        foreach ($name in @('result.json','backup-provenance.json','backup.cfg.log','before-a.bin','before-b.bin')) {
            $path=Join-Path $prior $name
            Lock-CheckedInput $path -1 (Get-CardReturningHash $path)
        }
        $null=Assert-CardReturningRestoreSource $sessionsRoot $RestoreSession $candidate $hashes['combined.bin'] $knownWorking
        foreach ($pair in @(@('before-a.bin','rollback-a.bin'),@('before-b.bin','rollback-b.bin'),@('result.json','prior-result.json'),@('backup-provenance.json','prior-backup-provenance.json'),@('backup.cfg.log','prior-backup.cfg.log'))) {
            $source=Join-Path $prior $pair[0]; $dest=Join-Path $session $pair[1]
            [IO.File]::Copy($source,$dest,$false)
            Lock-CheckedInput $dest -1 (Get-CardReturningHash $source)
        }
        $null=Assert-CardReturningBackups (Join-Path $session 'rollback-a.bin') (Join-Path $session 'rollback-b.bin') $knownWorking
        $record.restore_source_provenance_sha256=Get-CardReturningHash (Join-Path $session 'prior-backup-provenance.json')
    } else {
        Assert-CardReturningLayout $session $hashes
        $manifest=Get-Content -Raw -LiteralPath (Join-Path $session 'candidate-manifest.json') | ConvertFrom-Json
        Assert-CardReturningFields $manifest @{schema_version=1;status='built_review_required';image_kind='native_returning_pe_review';payload_kind='NativeReturning';hardware_accessed=$false;activation_performed=$false;physical_run_ready=$false;payload_sha256=$hashes['native-child.efi'];combined_sha256=$hashes['combined.bin'];combined_bytes=5242880;image_sha256=$hashes['configuration.bin'];slot_sha256=$hashes['payload-slot.bin'];pin_sha256=$hashes['pe-header.bin'];target_part='xc7a35tfgg484-2';stack_audit_result_sha256=$hashes['stack-result.json'];stack_audit_manifest_sha256=$hashes['stack-manifest.json'];stack_audit_verifier_sha256=$hashes['stack-verify.py']} 'Candidate manifest'
        $payload=Get-Content -Raw -LiteralPath (Join-Path $session 'payload-manifest.json') | ConvertFrom-Json
        Assert-CardReturningFields $payload @{schema_version=1;payload_format='SVMPE001';payload_bytes=59904;payload_sha256=$hashes['native-child.efi'];combined_sha256=$hashes['combined.bin'];combined_bytes=5242880;configuration_sha256=$hashes['configuration.bin'];slot_sha256=$hashes['payload-slot.bin'];header_sha256=$hashes['pe-header.bin'];payload_flash_offset=4194304;slot_bytes=1048576;required_parent_feature='card-returning-loader'} 'Payload manifest'
        $review=Get-Content -Raw -LiteralPath (Join-Path $session 'local-review.json') | ConvertFrom-Json
        Assert-CardReturningFields $review @{status='pass';scope='returning-programming-offline-review';candidate=$candidate;offline_only=$true;activation_performed=$false;child_sha256=$hashes['native-child.efi'];combined_sha256=$hashes['combined.bin'];configuration_sha256=$hashes['configuration.bin'];slot_sha256=$hashes['payload-slot.bin'];header_sha256=$hashes['pe-header.bin'];stack_result_sha256=$hashes['stack-result.json'];stack_manifest_sha256=$hashes['stack-manifest.json']} 'Independent local review'
        foreach ($tool in $auditTools) { Lock-CheckedInput $tool.Path $tool.Bytes $tool.Hash }
        $stackDirectory=Join-Path $root $stack
        $auditManifest=Get-Content -Raw -LiteralPath (Join-Path $stackDirectory 'manifest.json') | ConvertFrom-Json
        foreach ($section in @(@($auditManifest.sources,$root),@($auditManifest.artifacts,$stackDirectory))) {
            foreach ($entry in $section[0].PSObject.Properties) {
                $path=Assert-CardReturningPath (Join-Path $section[1] $entry.Name) $section[1]
                Lock-CheckedInput $path -1 $entry.Value
            }
        }
        $record.phase='stack_consumer'; Save-Record
        $oldPath=$env:PATH
        try {
            # Consumer's llvm-ar/llvm-objdump resolve only to pinned LLVM tools.
            $env:PATH='C:\Program Files\LLVM\bin;C:\Windows\System32'
            $consumerArguments=@('-E','-s','-B',('"'+(Join-Path $root 'tools/native-stack-audit/verify.py')+'"'),'--evidence',('"'+$stackDirectory+'"'),'--image',('"'+(Join-Path $session 'native-child.efi')+'"'))
            Invoke-BoundedProcess $auditTools[0].Path $consumerArguments 'stack-consumer' 180 @('"status": "pass"',$hashes['native-child.efi'])
        } finally { $env:PATH=$oldPath }
        $record.stack_consumer_passed=$true
    }
    if ($Action -eq 'CheckOnly') {
        $record.status='offline_pass'; $record.phase='complete'; Save-Record
        Write-Output "PASS returning offline check. No hardware accessed. Evidence: $session"
        return
    }
    $record.phase='backing_up'; Save-Record
    Invoke-CardStage 'backup.cfg' @('PASS card-double-backup')
    $first=Join-Path $session 'before-a.bin'; $second=Join-Path $session 'before-b.bin'
    $expectedBefore=if($isRestore){''}else{$knownWorking}
    $backupHash=Assert-CardReturningBackups $first $second $expectedBefore
    Lock-CheckedInput $first 5242880 $backupHash; Lock-CheckedInput $second 5242880 $backupHash
    $log=Join-Path $session 'backup.cfg.log'; $logHash=Get-CardReturningHash $log
    Lock-CheckedInput $log -1 $logHash
    $provenance=[ordered]@{schema_version=1;procedure='card-returning-v1';session_id=$sessionId;action=$Action;candidate=$candidate;image_sha256=$hashes['combined.bin'];bytes=5242880;first_file='before-a.bin';second_file='before-b.bin';first_sha256=$backupHash;second_sha256=$backupHash;known_working_match=($backupHash -ceq $knownWorking);flash_verify_completed=$true;backup_config_sha256=$hashes['backup.cfg'];transport_sha256=$hashes['transport.cfg'];log_sha256=$logHash;recorded_utc=[DateTime]::UtcNow.ToString('o')}
    $provenancePath=Join-Path $session 'backup-provenance.json'
    [IO.File]::WriteAllText($provenancePath,($provenance | ConvertTo-Json),[Text.UTF8Encoding]::new($false))
    $record.backup_provenance_sha256=Get-CardReturningHash $provenancePath
    Lock-CheckedInput $provenancePath -1 $record.backup_provenance_sha256
    $record.backup_complete=$true; $record.backup_sha256=$backupHash; $record.phase='backup_admitted'; Save-Record
    # Rehash locked inputs once more; the Tcl stage then verifies the selected
    # card against before-a immediately before its sole erase/write command.
    $null=Assert-CardReturningBackups $first $second $expectedBefore
    if ($isRestore) { $null=Assert-CardReturningBackups (Join-Path $session 'rollback-a.bin') (Join-Path $session 'rollback-b.bin') $knownWorking }
    else { Assert-CardReturningFile (Join-Path $session 'combined.bin') 5242880 $hashes['combined.bin'] }
    $record.phase='programming'; $record.write_attempted=$true; Save-Record
    if ($isRestore) { Invoke-CardStage 'restore.cfg' @('PASS card-restore-prewrite-backup-verified','PASS card-returning-restore-readback') }
    else { Invoke-CardStage 'program.cfg' @('PASS card-prewrite-backup-verified','PASS card-program-readback') }
    $record.phase='readback_verification'; Save-Record
    Lock-CheckedInput (Join-Path $session 'readback.bin') 5242880 $record.programmed_sha256
    $record.status='verified_not_activated'; $record.phase='complete'; Save-Record
    Write-Output "PASS returning $Action and independent full 5 MiB readback. No activation. Evidence: $session"
} catch {
    $failure=$_.Exception.Message
    $record.status='failed'; $record.error=$failure
    if (Test-Path -LiteralPath $session -PathType Container) { Save-Record }
    throw "Returning procedure stopped; no automatic restore or activation. Evidence: $session. $failure"
} finally {
    foreach ($handle in $locks) { $handle.Dispose() }
    if ($null -ne $operationLock) { $operationLock.Dispose() }
}
