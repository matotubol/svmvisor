[CmdletBinding(SupportsShouldProcess, ConfirmImpact='Medium')]
param(
    [ValidateSet('CheckOnly','Program','Restore','CheckPayload','ProgramPayload','RestorePayload')][string]$Action='CheckOnly',
    [switch]$ConfirmFlash,
    [string]$RestoreSession,
    # Payload-slot-only development actions (card-resident-dev-loader), see card-payload.ps1.
    [string]$BuildPath,
    [ValidateRange(100,30000)][int]$AdapterKhz=1000
)
# Offline by default. No image/tool/path overrides, download, or activation.
# Target Windows configuration must be selected before activation; Hyper-V/VBS
# coexistence is unsupported. This wrapper never changes Windows protections.
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
if ($Action -notin @('CheckOnly','CheckPayload') -and -not $ConfirmFlash) { throw "$Action requires explicit -ConfirmFlash." }
if ($RestoreSession -and $RestoreSession -cnotmatch '^[0-9a-f]{32}$') { throw 'RestoreSession must be a lowercase 32-hex session ID, never a path.' }
$validationPin='9d5aae4137320c80f0b8544d36d2c46d8d2210dc4834ff20a497b64d0d52b25b'
$validation=Join-Path $PSScriptRoot 'card-validation.ps1'
if ($validationPin -cnotmatch '^[0-9a-f]{64}$') { throw 'Required exact returning validation pin is unpopulated; review is incomplete.' }
if ((Get-FileHash -LiteralPath $validation).Hash.ToLowerInvariant() -cne $validationPin) { throw 'Returning validation procedure changed.' }
. $validation
$isPayloadAction=$Action -in @('CheckPayload','ProgramPayload','RestorePayload')
if (-not $isPayloadAction) {
    if ($BuildPath -or $AdapterKhz -ne 1000) { throw '-BuildPath and -AdapterKhz apply only to the payload-slot actions.' }
    $Action=ConvertTo-CardReturningAction $Action
    Assert-CardReturningAction $Action ([bool]$ConfirmFlash) $RestoreSession
}
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
# Strictly validate the candidate-specific pins; reuse the validation helpers.
function Assert-CardCandidatePin($Pin, [string]$Root) {
    if ($null -eq $Pin -or $Pin -isnot [hashtable]) { throw 'candidate-pin.psd1 did not load as a data table.' }
    if ((@($Pin.Keys | Sort-Object) -join ',') -cne 'Candidate,Inputs,KnownWorking,ResidentBuild') { throw 'candidate-pin.psd1 has an unexpected key set.' }
    Assert-CardReturningPin $Pin.KnownWorking
    foreach ($relative in @($Pin.Candidate,$Pin.ResidentBuild)) {
        if ([string]::IsNullOrWhiteSpace($relative)) { throw 'candidate-pin.psd1 has an empty candidate path.' }
        $null=Assert-CardReturningPath (Join-Path $Root $relative) $Root
    }
    if ($Pin.Inputs -isnot [object[]] -or $Pin.Inputs.Count -lt 1) { throw 'candidate-pin.psd1 Inputs must be a non-empty list.' }
    foreach ($entry in $Pin.Inputs) {
        if ($entry -isnot [hashtable]) { throw 'candidate-pin.psd1 input is not a data table.' }
        if ((@($entry.Keys | Sort-Object) -join ',') -cne 'Bytes,Hash,Name,Path') { throw "candidate-pin.psd1 input has an unexpected key set: $($entry.Name)" }
        if ([string]::IsNullOrWhiteSpace($entry.Name) -or [string]::IsNullOrWhiteSpace($entry.Path)) { throw 'candidate-pin.psd1 input has an empty Name or Path.' }
        Assert-CardReturningPin $entry.Hash
        if (($entry.Bytes -isnot [int] -and $entry.Bytes -isnot [long]) -or [long]$entry.Bytes -lt 0) { throw "candidate-pin.psd1 input has an invalid byte count: $($entry.Name)" }
        $null=Assert-CardReturningPath (Join-Path $Root $entry.Path) $Root
    }
}
# Candidate-specific pins live in the data file; tool/hardware pins stay here.
$candidatePin=Import-PowerShellDataFile -LiteralPath (Join-Path $PSScriptRoot 'candidate-pin.psd1')
Assert-CardCandidatePin $candidatePin $root
$candidate=$candidatePin.Candidate
$resident=$candidatePin.ResidentBuild
$knownWorking=$candidatePin.KnownWorking
$sessionsRoot=Join-Path $root 'target/firmware/card/card-resident-sessions'
$isRestore=($Action -eq 'Restore' -or [bool]$RestoreSession)
# Exact candidate and independent offline review. Physical readiness is not implied.
# The card-returning-v1 transport schema is reused for the unchanged restore validator.
$candidateInputs=@($candidatePin.Inputs | ForEach-Object { @{Name=$_.Name;Path=$_.Path;Bytes=[long]$_.Bytes;Hash=$_.Hash;Group='Candidate'} })
# Fixed repository tool and hardware pins (recomputed from final file bytes).
$fixedInputs=@(
    @{Name='resident-verify.py';Path='firmware/card/verify-resident-build.py';Bytes=4069;Hash='80bd3f4d9cb9bd826ec41d6e7105fecf6e5f945af5becc29debdf85bda2d83c4';Group='Candidate'},
    @{Name='openocd.exe';Path='target/firmware/tools/openocd/bin/openocd.exe';Bytes=13664247;Hash='9732b05af7e0f6a05a0051371e49af42515662ad309ddcc87f86f9b434ce96d8';Group='Hardware'},
    @{Name='proxy.bit';Path='target/firmware/tools/lambda-squirrel/flash_screamer/bscan_spi_xc7a35t.bit';Bytes=261513;Hash='ef8af1e277a7fe556e1ed7ace4680d4993cfc4174616485e1c354793d784b7f6';Group='Hardware'},
    @{Name='transport.cfg';Path='firmware/card/openocd/card-transport.cfg';Bytes=-1;Hash='3a31fe2838566fc9653bd0306c7378da93c86beb2bde611f3038272b1c75e642';Group='Hardware'},
    @{Name='backup.cfg';Path='firmware/card/openocd/card-backup.cfg';Bytes=-1;Hash='fb376387fa53257dfcfe539a22fbeb0c8e12c0c403c6522b1cf21b65bbb059e2';Group='Hardware'},
    @{Name='program.cfg';Path='firmware/card/openocd/card-program.cfg';Bytes=-1;Hash='1127c815c2fcf611af2a7c6c5f1b4c1f85ee63d1caf870453afa50f50b8ff4b6';Group='Hardware'},
    @{Name='restore.cfg';Path='firmware/card/openocd/card-restore.cfg';Bytes=1215;Hash='633487c826e6860381765836414611d3caf8377cd03f3967ef95212aa049773a';Group='Hardware'},
    @{Name='validation.ps1';Path='firmware/card/card-validation.ps1';Bytes=9226;Hash=$validationPin;Group='Hardware'}
)
$inputs=@($candidateInputs + $fixedInputs)
$auditTools=@(
    @{Path='C:/Users/mato/AppData/Local/Programs/Python/Python313/python.exe';Bytes=105696;Hash='85b71d8c6ec1905935f74be0c9869aae198d00e98f39df699ec66f9c5a84cecd'}
)
# Payload-slot-only development path: same tool/hardware pins, its own reviewed
# procedure and OpenOCD stages, and no candidate pin (the payload changes every
# build; card-payload.ps1 re-verifies the supplied build directory instead).
if ($isPayloadAction) {
    $payloadProcedure=@{Name='card-payload.ps1';Path='firmware/card/card-payload.ps1';Bytes=-1;Hash='38e9baa70f95a780f0fef4843211520dc78f43dd285b8ac2539ae0b66085b4d1'}
    $payloadInputs=@($fixedInputs | Where-Object { $_.Name -in @('resident-verify.py','openocd.exe','proxy.bit','transport.cfg','validation.ps1') }) + @(
        @{Name='payload-backup.cfg';Path='firmware/card/openocd/card-payload-backup.cfg';Bytes=-1;Hash='578991380384a1894d0eff9d8c89b90a19c49d1d78f39a06e7443b091c74fbe4'},
        @{Name='payload-program.cfg';Path='firmware/card/openocd/card-payload-program.cfg';Bytes=-1;Hash='10ab95e25d6b9d02f3ec8b1a6292d2b366d8707e29b23dd13dff23df848d61e8'},
        $payloadProcedure
    )
    Assert-CardReturningFile (Join-Path $root $payloadProcedure.Path) $payloadProcedure.Bytes $payloadProcedure.Hash
    . (Join-Path $root $payloadProcedure.Path)
    $resolvedBuild=if ($BuildPath) { $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($BuildPath) } else { '' }
    Invoke-CardPayloadProcedure -Action $Action -Root $root -BuildPath $resolvedBuild -RestoreSession $RestoreSession -AdapterKhz $AdapterKhz -Inputs $payloadInputs -Python $auditTools[0] -CallerPath $PSCommandPath -Confirmed ([bool]$ConfirmFlash) -Cmdlet $PSCmdlet
    return
}
$hashes=@{}; foreach ($asset in $inputs) { Assert-CardReturningPin $asset.Hash; $hashes[$asset.Name]=$asset.Hash }
$selected=@($inputs | Where-Object { -not $isRestore -or $_.Group -eq 'Hardware' })
foreach ($asset in $selected) { Assert-CardReturningFile (Join-Path $root $asset.Path) $asset.Bytes $asset.Hash }
if (-not $isRestore) { foreach ($tool in $auditTools) { Assert-CardReturningFile $tool.Path $tool.Bytes $tool.Hash } }
$prior=$null
if ($isRestore) { $prior=Assert-CardReturningRestoreSource $sessionsRoot $RestoreSession $candidate $hashes['combined.bin'] $knownWorking }
if ($Action -ne 'CheckOnly' -and -not $PSCmdlet.ShouldProcess('Squirrel XC7A35T / IS25LP256D sectors 0..79', "$Action resident image: load BSCAN proxy, double-backup 5 MiB, verify current contents, exact 80-sector write and full readback; no activation")) { return }
$sessionId=[guid]::NewGuid().ToString('N')
$session=if ($Action -eq 'CheckOnly') { Join-Path $root ('work/card-resident-checks/'+$sessionId) } else { Join-Path $sessionsRoot $sessionId }
$null=Assert-CardReturningPath $session $root
$sessionTcl=ConvertTo-CardReturningTclPath $session
$locks=[Collections.Generic.List[IO.FileStream]]::new()
$operationLock=$null
$record=[ordered]@{schema_version=1;procedure='card-returning-v1';delivery_profile='native-resident-boot';session_id=$sessionId;action=$Action;candidate=$candidate;status='running';phase='staging';hardware_accessed=$false;activation_performed=$false;write_attempted=$false;backup_bytes=5242880;backup_complete=$false;backup_sha256=$null;backup_provenance_sha256=$null;image_sha256=$hashes['combined.bin'];restore_source_session=$RestoreSession;restore_source_provenance_sha256=$null;programmed_sha256=$(if($isRestore){$knownWorking}else{$hashes['combined.bin']});started_utc=[DateTime]::UtcNow.ToString('o');error=$null}
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
function Assert-CardResidentLayout([string]$Directory, [hashtable]$Hashes) {
    $combined=[IO.File]::ReadAllBytes((Join-Path $Directory 'combined.bin'))
    $configuration=[IO.File]::ReadAllBytes((Join-Path $Directory 'configuration.bin'))
    $slot=[IO.File]::ReadAllBytes((Join-Path $Directory 'payload-slot.bin'))
    $child=[IO.File]::ReadAllBytes((Join-Path $Directory 'native-child.efi'))
    $header=[IO.File]::ReadAllBytes((Join-Path $Directory 'pe-header.bin'))
    if ($combined.Length -ne 5242880 -or $configuration.Length -le 0 -or $configuration.Length -gt 4194304 -or
        $slot.Length -ne 1048576 -or $child.Length -ne 153088 -or $header.Length -ne 128) { throw 'Resident layout has wrong extent.' }
    $sha=[Security.Cryptography.SHA256]::Create()
    try {
        function Slice-Hash([byte[]]$Data,[int]$Offset,[int]$Count) {
            return [Convert]::ToHexString($sha.ComputeHash($Data,$Offset,$Count)).ToLowerInvariant()
        }
        if ((Slice-Hash $combined 0 $configuration.Length) -cne $Hashes['configuration.bin'] -or
            (Slice-Hash $combined 4194304 1048576) -cne $Hashes['payload-slot.bin'] -or
            (Slice-Hash $slot 0 128) -cne $Hashes['pe-header.bin'] -or
            (Slice-Hash $slot 128 $child.Length) -cne $Hashes['native-child.efi']) { throw 'Resident layout component mismatch.' }
        foreach ($part in @(@($combined,$configuration.Length,(4194304-$configuration.Length)), @($slot,(128+$child.Length),(1048576-128-$child.Length)))) {
            $padding=[byte[]]::new($part[2]); [Array]::Fill($padding,[byte]255)
            if ((Slice-Hash $part[0] $part[1] $part[2]) -cne (Slice-Hash $padding 0 $padding.Length)) { throw 'Resident layout padding is not erased 0xff.' }
        }
    } finally { $sha.Dispose() }
    if ([Text.Encoding]::ASCII.GetString($header,0,8) -cne 'SVMBPE01' -or
        [BitConverter]::ToUInt32($header,8) -ne 1 -or [BitConverter]::ToUInt32($header,12) -ne 128 -or
        [BitConverter]::ToUInt64($header,16) -ne 153088 -or [BitConverter]::ToUInt64($header,24) -ne 1048576 -or
        [BitConverter]::ToUInt64($header,32) -ne 128 -or [BitConverter]::ToUInt64($header,40) -ne 4 -or
        [Convert]::ToHexString($header,48,32).ToLowerInvariant() -cne $Hashes['native-child.efi']) { throw 'Resident PE header contract mismatch.' }
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
        # Single-writer lock on the resident session root.
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
        Assert-CardResidentLayout $session $hashes
        $manifest=Get-Content -Raw -LiteralPath (Join-Path $session 'candidate-manifest.json') | ConvertFrom-Json
        Assert-CardReturningFields $manifest @{schema_version=1;status='built_review_required';image_kind='native_resident_pe_review';payload_kind='NativeResidentBoot';hardware_accessed=$false;activation_performed=$false;physical_run_ready=$false;payload_sha256=$hashes['native-child.efi'];combined_sha256=$hashes['combined.bin'];combined_bytes=5242880;image_sha256=$hashes['configuration.bin'];slot_sha256=$hashes['payload-slot.bin'];pin_sha256=$hashes['pe-header.bin'];target_part='xc7a35tfgg484-2';resident_summary_sha256=$hashes['resident-summary.json'];resident_source_manifest_sha256=$hashes['resident-source-manifest.json'];completion_only_netlist_policy='PENDING_REVIEW'} 'Candidate manifest'
        $payload=Get-Content -Raw -LiteralPath (Join-Path $session 'payload-manifest.json') | ConvertFrom-Json
        Assert-CardReturningFields $payload @{schema_version=1;payload_format='SVMBPE01';payload_bytes=153088;payload_sha256=$hashes['native-child.efi'];combined_sha256=$hashes['combined.bin'];combined_bytes=5242880;configuration_sha256=$hashes['configuration.bin'];slot_sha256=$hashes['payload-slot.bin'];header_sha256=$hashes['pe-header.bin'];payload_flash_offset=4194304;slot_bytes=1048576;required_parent_feature='card-resident-loader'} 'Payload manifest'
        $review=Get-Content -Raw -LiteralPath (Join-Path $session 'local-review.json') | ConvertFrom-Json
        Assert-CardReturningFields $review @{status='pass';scope='resident-programming-offline-review';candidate=$candidate;offline_only=$true;activation_performed=$false;physical_run_ready=$false;child_sha256=$hashes['native-child.efi'];combined_sha256=$hashes['combined.bin'];configuration_sha256=$hashes['configuration.bin'];slot_sha256=$hashes['payload-slot.bin'];header_sha256=$hashes['pe-header.bin'];resident_summary_sha256=$hashes['resident-summary.json'];resident_source_manifest_sha256=$hashes['resident-source-manifest.json'];parent_sha256=$manifest.loader_sha256;firmware_fixture_status='pass'} 'Independent local review'
        Assert-CardReturningFields $review.routed_policy @{status='pass_for_exact_candidate'} 'Routed candidate review'
        Assert-CardReturningFields $review.predecessor @{combined_sha256=$knownWorking;full_readback_sha256=$knownWorking;full_readback_bytes=5242880} 'Last-known installed predecessor'
        # Review digest binds the routed reports, netlist, bitstream and physical predecessor evidence.
        foreach ($asset in $review.artifacts) {
            $path=Assert-CardReturningPath (Join-Path $root $asset.path) $root
            Lock-CheckedInput $path $asset.bytes $asset.sha256
        }
        # The pinned review binds complete result/log/image/source evidence, and
        # these checks bind successful firmware execution to this exact PE pair.
        if (@($review.firmware_fixtures).Count -ne 3) { throw 'Expected Positive, Header and Digest firmware evidence.' }
        $fixtureModes=@()
        foreach ($fixtureEntry in $review.firmware_fixtures) {
            $fixturePath=Assert-CardReturningPath (Join-Path $root $fixtureEntry.result_path) $root
            Lock-CheckedInput $fixturePath -1 $fixtureEntry.summary_sha256
            $fixture=Get-Content -Raw -LiteralPath $fixturePath | ConvertFrom-Json
            Assert-CardReturningFields $fixture @{schema=1;status='passed';emulatorOnly=$true;hardwareAccessed=$false;mode=$fixtureEntry.mode;processors=2;parentFeature='card-resident-loader';childFeature='native-resident-boot';testOutputChild=$false;windowsTested=$false;hyperVVBSSupported=$false} 'Actual resident firmware fixture'
            Assert-CardReturningFields $fixture.artifactHashes @{'reviewed-child.efi'=$hashes['native-child.efi'];'svmvisor-dxe.efi'=$manifest.loader_sha256} 'Executed parent/child identity'
            $expectedStages=if($fixture.mode -ceq 'Positive'){'1,2,3,4,5'}else{''}
            if ((@($fixture.childBootStages) -join ',') -cne $expectedStages) { throw 'Incorrect actual child boot stages.' }
            $fixtureModes += $fixture.mode
        }
        if ((($fixtureModes | Sort-Object) -join ',') -cne 'Digest,Header,Positive') { throw 'Missing or duplicate firmware fixture mode.' }
        # Lock both the current complete candidate source and its retained snapshot.
        foreach ($entry in $manifest.source_hashes) {
            foreach ($base in @($root,(Join-Path $root ($candidate+'/reviewed-source')))) {
                $path=Assert-CardReturningPath (Join-Path $base $entry.path) $base
                Lock-CheckedInput $path -1 $entry.sha256
            }
        }
        foreach ($tool in $auditTools) { Lock-CheckedInput $tool.Path $tool.Bytes $tool.Hash }
        $residentDirectory=Join-Path $root $resident
        $auditSummary=Get-Content -Raw -LiteralPath (Join-Path $session 'resident-summary.json') | ConvertFrom-Json
        $auditSources=Get-Content -Raw -LiteralPath (Join-Path $session 'resident-source-manifest.json') | ConvertFrom-Json
        foreach ($entry in $auditSummary.artifacts.PSObject.Properties) {
            $path=Assert-CardReturningPath (Join-Path $residentDirectory $entry.Name) $residentDirectory
            Lock-CheckedInput $path -1 $entry.Value
        }
        Lock-CheckedInput (Join-Path $residentDirectory 'summary.json') -1 $hashes['resident-summary.json']
        foreach ($entry in $auditSources.PSObject.Properties) {
            foreach ($base in @($root,(Join-Path $residentDirectory 'source'))) {
                $path=Assert-CardReturningPath (Join-Path $base $entry.Name) $base
                Lock-CheckedInput $path -1 $entry.Value
            }
        }
        $record.phase='resident_consumer'; Save-Record
        $oldPath=$env:PATH
        try {
            $env:PATH='C:\Windows\System32'
            # Use the pinned original location: verifier resolves repository ROOT from __file__.
            # --no-current-source-check: the verifier's own current-source check runs
            # `cargo xtask sources`, which needs cargo on PATH; this minimal-PATH consumer
            # instead locks every current source against the audited manifest above.
            $consumerArguments=@('-E','-s','-B',('"'+(Join-Path $root 'firmware/card/verify-resident-build.py')+'"'),'--evidence',('"'+$residentDirectory+'"'),'--image',('"'+(Join-Path $session 'native-child.efi')+'"'),'--no-current-source-check')
            Invoke-BoundedProcess $auditTools[0].Path $consumerArguments 'resident-consumer' 180 @('"status": "verified"',$hashes['native-child.efi'])
        } finally { $env:PATH=$oldPath }
        $record.resident_consumer_passed=$true
        # This technical gate is independent of physical-run readiness and target selection.
        # -ConfirmFlash authorizes programming only; activation remains a separate operator action.
        if ($Action -eq 'Program' -and $review.firmware_fixture_status -cne 'pass') {
            throw 'Actual resident parent/child firmware fixture has not passed; Program is not yet qualified.'
        }
    }
    if ($Action -eq 'CheckOnly') {
        $record.status='offline_pass'; $record.phase='complete'; Save-Record
        Write-Output "PASS resident offline check. No hardware accessed. Evidence: $session"
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
    $provenance=[ordered]@{schema_version=1;procedure='card-returning-v1';delivery_profile='native-resident-boot';session_id=$sessionId;action=$Action;candidate=$candidate;image_sha256=$hashes['combined.bin'];bytes=5242880;first_file='before-a.bin';second_file='before-b.bin';first_sha256=$backupHash;second_sha256=$backupHash;known_working_match=($backupHash -ceq $knownWorking);flash_verify_completed=$true;backup_config_sha256=$hashes['backup.cfg'];transport_sha256=$hashes['transport.cfg'];log_sha256=$logHash;recorded_utc=[DateTime]::UtcNow.ToString('o')}
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
    Write-Output "PASS resident $Action and independent full 5 MiB readback. No activation. Evidence: $session"
} catch {
    $failure=$_.Exception.Message
    $record.status='failed'; $record.error=$failure
    if (Test-Path -LiteralPath $session -PathType Container) { Save-Record }
    throw "Resident procedure stopped; no automatic restore or activation. Evidence: $session. $failure"
} finally {
    foreach ($handle in $locks) { $handle.Dispose() }
    if ($null -ne $operationLock) { $operationLock.Dispose() }
}
