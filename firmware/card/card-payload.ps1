# Payload-slot-only programming for the development loader (card-resident-dev-loader).
# Dot-sourced by flash-card.ps1 after card-validation.ps1; pinned there by SHA-256.
# Touches nothing outside flash [0x400000,0x500000) = 64 KiB sectors 64..79. The
# FPGA configuration below 0x400000 is never erased or written by this procedure.
# Runs under Windows PowerShell 5.1 and PowerShell 7: no .NET-Core-only APIs here.
Set-StrictMode -Version Latest

$script:CardPayloadSlotOffset=0x400000
$script:CardPayloadSlotBytes=0x100000
$script:CardPayloadSectorBytes=0x10000
$script:CardPayloadFirstSector=64
$script:CardPayloadLastSector=79

function Assert-CardPayloadAction([string]$Action,[bool]$Confirmed,[string]$BuildPath,[string]$RestoreSession) {
    if ($Action -cnotin @('CheckPayload','ProgramPayload','RestorePayload')) { throw 'Unknown payload action.' }
    if ($Action -cne 'CheckPayload' -and -not $Confirmed) { throw "$Action requires explicit -ConfirmFlash." }
    if ($RestoreSession -and $RestoreSession -cnotmatch '^[0-9a-f]{32}$') { throw 'RestoreSession must be a lowercase 32-hex session ID, never a path.' }
    if ($Action -ceq 'RestorePayload') {
        if (-not $RestoreSession) { throw 'RestorePayload requires a prior payload session ID.' }
        if ($BuildPath) { throw 'RestorePayload restores a session backup; it takes no -BuildPath.' }
    } else {
        if ($RestoreSession) { throw "$Action cannot select a restore session." }
        if ([string]::IsNullOrWhiteSpace($BuildPath)) { throw "$Action requires -BuildPath <cargo xtask card-dev output directory>." }
    }
}

function Assert-CardPayloadAdapterKhz($Value) {
    # Numbers only: this value is placed on the OpenOCD command line.
    if ($Value -isnot [int] -and $Value -isnot [long]) { throw 'AdapterKhz must be an integer.' }
    if ([long]$Value -lt 100 -or [long]$Value -gt 30000) { throw 'AdapterKhz must be within 100..30000 (FT4232H limit).' }
    return [int]$Value
}

function Get-CardPayloadHex([byte[]]$Hash) { return [BitConverter]::ToString($Hash).Replace('-','').ToLowerInvariant() }

function Get-CardPayloadSliceHash([byte[]]$Data,[int]$Offset,[int]$Count) {
    if ($Offset -lt 0 -or $Count -lt 0 -or $Offset+$Count -gt $Data.Length) { throw 'Slice escapes its buffer.' }
    $sha=[Security.Cryptography.SHA256]::Create()
    try { return Get-CardPayloadHex $sha.ComputeHash($Data,$Offset,$Count) } finally { $sha.Dispose() }
}

function Get-CardPayloadErasedHash([int]$Count) {
    $erased=New-Object byte[] $Count
    for ($i=0; $i -lt $Count; $i++) { $erased[$i]=255 }
    return Get-CardPayloadSliceHash $erased 0 $Count
}

# The slot part of Assert-CardResidentLayout, without a candidate-pinned child
# size: header + child + 0xff, and a header that describes exactly that child.
function Assert-CardPayloadSlot([byte[]]$Slot,[byte[]]$Child,[byte[]]$Header) {
    if ($Slot.Length -ne $script:CardPayloadSlotBytes -or $Header.Length -ne 128 -or
        $Child.Length -lt 512 -or $Child.Length -gt $script:CardPayloadSlotBytes-128) { throw 'Payload slot layout has wrong extent.' }
    $childHash=Get-CardPayloadSliceHash $Child 0 $Child.Length
    if ((Get-CardPayloadSliceHash $Slot 0 128) -cne (Get-CardPayloadSliceHash $Header 0 128) -or
        (Get-CardPayloadSliceHash $Slot 128 $Child.Length) -cne $childHash) { throw 'Payload slot component mismatch.' }
    $tail=$script:CardPayloadSlotBytes-128-$Child.Length
    if ((Get-CardPayloadSliceHash $Slot (128+$Child.Length) $tail) -cne (Get-CardPayloadErasedHash $tail)) { throw 'Payload slot padding is not erased 0xff.' }
    if ([Text.Encoding]::ASCII.GetString($Header,0,8) -cne 'SVMBPE01' -or
        [BitConverter]::ToUInt32($Header,8) -ne 1 -or [BitConverter]::ToUInt32($Header,12) -ne 128 -or
        [BitConverter]::ToUInt64($Header,16) -ne [uint64]$Child.Length -or [BitConverter]::ToUInt64($Header,24) -ne 1048576 -or
        [BitConverter]::ToUInt64($Header,32) -ne 128 -or [BitConverter]::ToUInt64($Header,40) -ne 4 -or
        (Get-CardPayloadHex $Header[48..79]) -cne $childHash) { throw 'Payload PE header contract mismatch.' }
    return $childHash
}

# Sector numbers are absolute flash sector indices. Only 64..79 can be named.
function Assert-CardPayloadSectors($Sectors,[string]$Name) {
    $previous=$script:CardPayloadFirstSector-1
    foreach ($sector in @($Sectors)) {
        if ($sector -isnot [int]) { throw "$Name contains a non-integer sector." }
        if ($sector -lt $script:CardPayloadFirstSector -or $sector -gt $script:CardPayloadLastSector) { throw "$Name sector $sector is outside the payload slot sectors 64..79." }
        if ($sector -le $previous) { throw "$Name sectors must be strictly ascending." }
        $offset=[long]$sector*$script:CardPayloadSectorBytes
        if ($offset -lt $script:CardPayloadSlotOffset -or $offset+$script:CardPayloadSectorBytes -gt $script:CardPayloadSlotOffset+$script:CardPayloadSlotBytes) { throw "$Name sector $sector escapes [0x400000,0x500000)." }
        $previous=$sector
    }
}

# Injection-safe: the only text that reaches Tcl is decimal digits and spaces.
function ConvertTo-CardPayloadTclList($Sectors,[string]$Name) {
    Assert-CardPayloadSectors $Sectors $Name
    $text=(@($Sectors) | ForEach-Object { ([int]$_).ToString([Globalization.CultureInfo]::InvariantCulture) }) -join ' '
    if ($text -cnotmatch '^(|[1-9][0-9]( [1-9][0-9])*)$') { throw "$Name did not serialize to decimal sector numbers." }
    return $text
}

# Minimal sector plan to turn the slot's Current bytes into Target bytes.
#   Touch = every sector whose current bytes differ from the target. For a new
#           payload that is the sectors covering header+child (the header digest
#           changes every build) plus every sector beyond the new extent that is
#           not already erased (stale bytes of a longer previous payload).
#   Erase = touched sectors that are not already all 0xff.
#   Write = touched sectors whose target is not all 0xff.
function Get-CardPayloadPlan([byte[]]$Current,[byte[]]$Target) {
    if ($Current.Length -ne $script:CardPayloadSlotBytes -or $Target.Length -ne $script:CardPayloadSlotBytes) { throw 'Sector plan needs two exact 1 MiB slot images.' }
    $erased=Get-CardPayloadErasedHash $script:CardPayloadSectorBytes
    $touch=@(); $erase=@(); $write=@()
    for ($index=0; $index -lt 16; $index++) {
        $offset=$index*$script:CardPayloadSectorBytes
        $now=Get-CardPayloadSliceHash $Current $offset $script:CardPayloadSectorBytes
        $want=Get-CardPayloadSliceHash $Target $offset $script:CardPayloadSectorBytes
        if ($now -ceq $want) { continue }
        $sector=[int]($script:CardPayloadFirstSector+$index)
        $touch+=$sector
        if ($now -cne $erased) { $erase+=$sector }
        if ($want -cne $erased) { $write+=$sector }
    }
    foreach ($pair in @(@($touch,'Touch'),@($erase,'Erase'),@($write,'Write'))) { Assert-CardPayloadSectors $pair[0] $pair[1] }
    return @{Touch=[int[]]$touch;Erase=[int[]]$erase;Write=[int[]]$write}
}

# Sectors [first,last] that hold the first ExtentBytes of the slot.
function Get-CardPayloadCoveringSectors([long]$ExtentBytes) {
    if ($ExtentBytes -lt 1 -or $ExtentBytes -gt $script:CardPayloadSlotBytes) { throw 'Payload extent escapes the slot.' }
    $count=[int][Math]::Ceiling($ExtentBytes/[double]$script:CardPayloadSectorBytes)
    $sectors=[int[]]@($script:CardPayloadFirstSector..($script:CardPayloadFirstSector+$count-1))
    Assert-CardPayloadSectors $sectors 'Covering'
    return $sectors
}

function Assert-CardPayloadRestoreSource([string]$SessionsRoot,[string]$Id) {
    if ($Id -cnotmatch '^[0-9a-f]{32}$') { throw 'Missing or malformed payload restore session ID.' }
    $prior=Assert-CardReturningPath (Join-Path $SessionsRoot $Id) $SessionsRoot
    foreach ($name in @('result.json','slot-before-a.bin','slot-before-b.bin','payload-backup.cfg.log')) { $null=Assert-CardReturningPath (Join-Path $prior $name) $prior }
    $result=Get-Content -Raw -LiteralPath (Join-Path $prior 'result.json') | ConvertFrom-Json
    Assert-CardReturningFields $result @{schema_version=1;procedure='card-payload-v1';session_id=$Id;activation_performed=$false;backup_complete=$true;backup_bytes=1048576;hardware_accessed=$true;slot_flash_offset=4194304} 'Prior payload result'
    if ($result.action -cnotin @('ProgramPayload','RestorePayload')) { throw 'Prior payload session is not a programming session.' }
    Assert-CardReturningPin $result.backup_sha256
    Assert-CardReturningFile (Join-Path $prior 'slot-before-a.bin') 1048576 $result.backup_sha256
    Assert-CardReturningFile (Join-Path $prior 'slot-before-b.bin') 1048576 $result.backup_sha256
    $log=Get-Content -Raw -LiteralPath (Join-Path $prior 'payload-backup.cfg.log')
    if ($log -notmatch 'PASS card-payload-double-backup' -or $log -notmatch 'PASS card-target-id-and-geometry') { throw 'Prior payload backup transcript is incomplete.' }
    return $prior
}

function Invoke-CardPayloadProcedure {
    param(
        [Parameter(Mandatory)][string]$Action,
        [Parameter(Mandatory)][string]$Root,
        [string]$BuildPath,
        [string]$RestoreSession,
        [Parameter(Mandatory)][int]$AdapterKhz,
        [Parameter(Mandatory)][object[]]$Inputs,      # pinned tools/cfgs copied into the session: Name,Path,Bytes,Hash
        [Parameter(Mandatory)][hashtable]$Python,     # pinned interpreter: Path,Bytes,Hash
        [Parameter(Mandatory)][string]$CallerPath,
        [bool]$Confirmed,
        $Cmdlet
    )
    Assert-CardPayloadAction $Action $Confirmed $BuildPath $RestoreSession
    $AdapterKhz=Assert-CardPayloadAdapterKhz $AdapterKhz
    $isCheck=$Action -ceq 'CheckPayload'; $isRestore=$Action -ceq 'RestorePayload'
    $sessionsRoot=Join-Path $Root 'target/firmware/card/card-payload-sessions'
    $hashes=@{}; foreach ($asset in $Inputs) { Assert-CardReturningPin $asset.Hash; $hashes[$asset.Name]=$asset.Hash }
    foreach ($name in @('openocd.exe','proxy.bit','transport.cfg','payload-backup.cfg','payload-program.cfg','resident-verify.py')) { if (-not $hashes.ContainsKey($name)) { throw "Missing pinned payload input: $name" } }
    foreach ($asset in $Inputs) { Assert-CardReturningFile (Join-Path $Root $asset.Path) $asset.Bytes $asset.Hash }
    if (-not $isRestore) { Assert-CardReturningFile $Python.Path $Python.Bytes $Python.Hash }
    $build=$null; $prior=$null
    if ($isRestore) { $prior=Assert-CardPayloadRestoreSource $sessionsRoot $RestoreSession }
    else {
        $build=Assert-CardReturningPath $BuildPath (Join-Path $Root 'target/card-dev')
        if (-not (Test-Path -LiteralPath $build -PathType Container)) { throw "BuildPath is not a cargo xtask card-dev output directory: $build" }
    }
    if (-not $isCheck -and $null -ne $Cmdlet -and -not $Cmdlet.ShouldProcess('Squirrel XC7A35T / IS25LP256D payload slot, sectors 64..79 only',"${Action}: load BSCAN proxy, double-backup the 1 MiB slot, erase/write only the differing slot sectors, full 1 MiB slot readback; no activation")) { return }
    $sessionId=[guid]::NewGuid().ToString('N')
    $session=if ($isCheck) { Join-Path $Root ('target/firmware/card/card-payload-checks/'+$sessionId) } else { Join-Path $sessionsRoot $sessionId }
    $null=Assert-CardReturningPath $session $Root
    $sessionTcl=ConvertTo-CardReturningTclPath $session
    $locks=New-Object 'Collections.Generic.List[IO.FileStream]'
    $operationLock=$null
    $clock=[Diagnostics.Stopwatch]::StartNew()
    $record=[ordered]@{schema_version=1;procedure='card-payload-v1';loader_mode='dev';session_id=$sessionId;action=$Action;build_path=$(if($build){$build.Substring($Root.TrimEnd('\','/').Length+1).Replace('\','/')}else{$null});status='running';phase='staging';hardware_accessed=$false;activation_performed=$false;write_attempted=$false;adapter_khz=$AdapterKhz;slot_flash_offset=4194304;slot_bytes=1048576;backup_bytes=1048576;backup_complete=$false;backup_sha256=$null;expected_slot_sha256=$null;header_sha256=$null;child_sha256=$null;child_bytes=$null;covering_sectors=$null;touch_sectors=$null;erase_sectors=$null;write_sectors=$null;bytes_erased=$null;bytes_written=$null;restore_source_session=$(if($RestoreSession){$RestoreSession}else{$null});timings_seconds=[ordered]@{};started_utc=[DateTime]::UtcNow.ToString('o');error=$null}
    $saveRecord={
        $record.updated_utc=[DateTime]::UtcNow.ToString('o')
        $final=Join-Path $session 'result.json'; $temp=$final+'.tmp'
        [IO.File]::WriteAllText($temp,($record | ConvertTo-Json -Depth 8),(New-Object Text.UTF8Encoding $false))
        if (Test-Path -LiteralPath $final) { [IO.File]::Replace($temp,$final,[NullString]::Value) } else { [IO.File]::Move($temp,$final) }
    }
    $lockInput={ param([string]$Path,[long]$Bytes,[string]$Hash)
        $null=Assert-CardReturningPath $Path ''
        $locks.Add([IO.File]::Open($Path,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read))
        Assert-CardReturningFile $Path $Bytes $Hash
    }
    $bounded={ param([string]$Exe,[string[]]$Arguments,[string]$Name,[int]$Seconds,[string[]]$Markers)
        $stdout=Join-Path $session ($Name+'.stdout.log'); $stderr=Join-Path $session ($Name+'.stderr.log')
        $process=$null; $trace=''
        try {
            $process=Start-Process -FilePath $Exe -ArgumentList $Arguments -WorkingDirectory $session -PassThru -WindowStyle Hidden -RedirectStandardOutput $stdout -RedirectStandardError $stderr
            $null=$process.Handle
            if (-not $process.WaitForExit($Seconds*1000)) { throw "Stage timed out after $Seconds seconds: $Name" }
            $process.WaitForExit(); $process.Refresh(); $exitCode=$process.ExitCode
            if ($null -eq $exitCode -or $exitCode -ne 0) { throw "Stage failed: $Name; exit=$exitCode" }
        } finally {
            if ($null -ne $process) {
                if (-not $process.HasExited) { try { $process.Kill($true) } catch { $process.Kill() }; $process.WaitForExit() }
                $process.Dispose()
            }
            foreach($path in @($stdout,$stderr)) { if(Test-Path -LiteralPath $path){$trace += [IO.File]::ReadAllText($path)+"`n"} }
            [IO.File]::WriteAllText((Join-Path $session ($Name+'.log')),$trace,(New-Object Text.UTF8Encoding $false))
        }
        foreach ($marker in $Markers) { if ($trace -notmatch [regex]::Escape($marker)) { throw "Missing stage success marker: $Name / $marker" } }
    }
    $stage={ param([string]$Config,[string[]]$Settings,[string[]]$Markers)
        $record.hardware_accessed=$true; & $saveRecord
        $arguments=@('-c','"gdb_port disabled"','-c','"tcl_port disabled"','-c','"telnet_port disabled"','-c',('"set SESSION {'+$sessionTcl+'}"'))
        foreach ($setting in $Settings) {
            # Name and value were produced from validated integers only.
            if ($setting -cnotmatch '^set [A-Z_]+ \{[0-9 ]*\}$') { throw 'Refusing a non-numeric OpenOCD setting.' }
            $arguments+=@('-c',('"'+$setting+'"'))
        }
        $arguments+=@('-f',('"'+(Join-Path $session $Config)+'"'))
        & $bounded (Join-Path $session 'openocd.exe') $arguments $Config 900 (@('PASS card-target-id-and-geometry',"PASS card-payload-adapter-khz $AdapterKhz")+$Markers)
    }
    try {
        if (-not $isCheck) {
            # One writer per card: share the full-image procedure's operation lock.
            $fullRoot=Assert-CardReturningPath (Join-Path $Root 'target/firmware/card/card-resident-sessions') $Root
            New-Item -ItemType Directory -Force -Path $fullRoot | Out-Null
            New-Item -ItemType Directory -Force -Path $sessionsRoot | Out-Null
            $operationLock=[IO.File]::Open((Assert-CardReturningPath (Join-Path $fullRoot 'operation.lock') $fullRoot),[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
        }
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $session) | Out-Null
        New-Item -ItemType Directory -Path $session | Out-Null
        & $saveRecord
        foreach ($asset in $Inputs) {
            $source=Join-Path $Root $asset.Path
            & $lockInput $source $asset.Bytes $asset.Hash
            $dest=Join-Path $session $asset.Name
            [IO.File]::Copy($source,$dest,$false)
            & $lockInput $dest $asset.Bytes $asset.Hash
        }
        $callerHash=Get-CardReturningHash $CallerPath
        & $lockInput $CallerPath -1 $callerHash
        [IO.File]::Copy($CallerPath,(Join-Path $session 'procedure.ps1'),$false)
        $record.procedure_sha256=$callerHash
        $expectedPath=Join-Path $session 'slot-expected.bin'
        if ($isRestore) {
            foreach ($name in @('result.json','slot-before-a.bin','slot-before-b.bin','payload-backup.cfg.log')) { $path=Join-Path $prior $name; & $lockInput $path -1 (Get-CardReturningHash $path) }
            $null=Assert-CardPayloadRestoreSource $sessionsRoot $RestoreSession
            [IO.File]::Copy((Join-Path $prior 'slot-before-a.bin'),$expectedPath,$false)
            [IO.File]::Copy((Join-Path $prior 'result.json'),(Join-Path $session 'prior-result.json'),$false)
            $record.expected_slot_sha256=Get-CardReturningHash (Join-Path $prior 'slot-before-b.bin')
            & $lockInput $expectedPath 1048576 $record.expected_slot_sha256
        } else {
            # Nothing about the build is pinned: it changes every iteration. Copy
            # it, lock the copies, then prove the copies are self-consistent and
            # are the audited resident build's driver.efi.
            foreach ($pair in @(@('payload/payload-slot.bin','slot-expected.bin'),@('payload/native-child.efi','native-child.efi'),@('payload/pe-header.bin','pe-header.bin'),@('payload/payload-manifest.json','payload-manifest.json'))) {
                $source=Assert-CardReturningPath (Join-Path $build $pair[0]) $build
                $dest=Join-Path $session $pair[1]
                [IO.File]::Copy($source,$dest,$false)
                $copied=Get-CardReturningHash $dest
                & $lockInput $dest -1 $copied
                & $lockInput $source -1 $copied
            }
            $slot=[IO.File]::ReadAllBytes($expectedPath)
            $child=[IO.File]::ReadAllBytes((Join-Path $session 'native-child.efi'))
            $header=[IO.File]::ReadAllBytes((Join-Path $session 'pe-header.bin'))
            $record.child_sha256=Assert-CardPayloadSlot $slot $child $header
            $record.child_bytes=$child.Length
            $record.header_sha256=Get-CardReturningHash (Join-Path $session 'pe-header.bin')
            $record.expected_slot_sha256=Get-CardReturningHash $expectedPath
            $record.covering_sectors=@(Get-CardPayloadCoveringSectors (128+$child.Length))
            $manifest=Get-Content -Raw -LiteralPath (Join-Path $session 'payload-manifest.json') | ConvertFrom-Json
            Assert-CardReturningFields $manifest @{schema_version=1;payload_format='SVMBPE01';payload_bytes=$child.Length;payload_sha256=$record.child_sha256;header_bytes=128;header_sha256=$record.header_sha256;slot_bytes=1048576;slot_sha256=$record.expected_slot_sha256;payload_flash_offset=4194304;hardware_accessed=$false} 'Payload manifest'
            $residentDirectory=Assert-CardReturningPath (Join-Path $build 'resident') $build
            $summaryPath=Join-Path $residentDirectory 'summary.json'
            & $lockInput $summaryPath -1 (Get-CardReturningHash $summaryPath)
            $summary=Get-Content -Raw -LiteralPath $summaryPath | ConvertFrom-Json
            foreach ($entry in $summary.artifacts.PSObject.Properties) {
                $path=Assert-CardReturningPath (Join-Path $residentDirectory $entry.Name) $residentDirectory
                & $lockInput $path -1 $entry.Value
            }
            if ($summary.artifacts.'driver.efi' -cne $record.child_sha256) { throw 'Packaged child is not the audited resident driver.efi.' }
            & $lockInput $Python.Path $Python.Bytes $Python.Hash
            $record.phase='resident_consumer'; & $saveRecord
            $oldPath=$env:PATH
            try {
                $env:PATH='C:\Windows\System32'
                # Pinned verifier from its original location (it resolves the repository
                # root from __file__). The audited build's own retained sources are
                # checked; whether the working tree has moved on since is not a flashing
                # concern in development mode, so no current-source comparison.
                $consumer=@('-E','-s','-B',('"'+(Join-Path $Root 'firmware/card/verify-resident-build.py')+'"'),'--evidence',('"'+$residentDirectory+'"'),'--image',('"'+(Join-Path $session 'native-child.efi')+'"'),'--no-current-source-check')
                & $bounded $Python.Path $consumer 'resident-consumer' 180 @('"status": "verified"',$record.child_sha256)
            } finally { $env:PATH=$oldPath }
            $record.resident_consumer_passed=$true
        }
        if ($isCheck) {
            $record.status='offline_pass'; $record.phase='complete'; $record.timings_seconds.total=[Math]::Round($clock.Elapsed.TotalSeconds,1); & $saveRecord
            Write-Output "PASS payload offline check. No hardware accessed. child=$($record.child_bytes) bytes sha256=$($record.child_sha256); slot sha256=$($record.expected_slot_sha256); covering sectors $($record.covering_sectors -join ','). Evidence: $session"
            return
        }
        $speed=@("set ADAPTER_KHZ {$AdapterKhz}")
        $record.phase='backing_up'; & $saveRecord
        $timer=[Diagnostics.Stopwatch]::StartNew()
        & $stage 'payload-backup.cfg' $speed @('PASS card-payload-double-backup')
        $record.timings_seconds.backup=[Math]::Round($timer.Elapsed.TotalSeconds,1)
        $first=Join-Path $session 'slot-before-a.bin'; $second=Join-Path $session 'slot-before-b.bin'
        $backupHash=Get-CardReturningHash $first
        & $lockInput $first 1048576 $backupHash; & $lockInput $second 1048576 $backupHash
        $logPath=Join-Path $session 'payload-backup.cfg.log'; & $lockInput $logPath -1 (Get-CardReturningHash $logPath)
        $record.backup_complete=$true; $record.backup_sha256=$backupHash; $record.phase='backup_admitted'; & $saveRecord
        $target=[IO.File]::ReadAllBytes($expectedPath)
        $plan=Get-CardPayloadPlan ([IO.File]::ReadAllBytes($first)) $target
        $record.touch_sectors=@($plan.Touch); $record.erase_sectors=@($plan.Erase); $record.write_sectors=@($plan.Write)
        $record.bytes_erased=$plan.Erase.Count*65536; $record.bytes_written=$plan.Write.Count*65536
        if ($plan.Touch.Count -eq 0) {
            # Two identical fresh reads already equal the expected slot image.
            $record.status='verified_already_current'; $record.phase='complete'; $record.timings_seconds.total=[Math]::Round($clock.Elapsed.TotalSeconds,1); & $saveRecord
            Write-Output "PASS payload slot already holds the expected image ($backupHash); nothing erased or written. Evidence: $session"
            return
        }
        foreach ($sector in $plan.Write) {
            $bytes=New-Object byte[] 65536
            [Array]::Copy($target,($sector-64)*65536,$bytes,0,65536)
            $path=Join-Path $session "sector-$sector.bin"
            [IO.File]::WriteAllBytes($path,$bytes)
            & $lockInput $path 65536 (Get-CardPayloadSliceHash $target (($sector-64)*65536) 65536)
        }
        Assert-CardReturningFile $expectedPath 1048576 $record.expected_slot_sha256
        Assert-CardReturningFile $first 1048576 $backupHash
        $settings=$speed+@(('set PAYLOAD_ERASE_SECTORS {'+(ConvertTo-CardPayloadTclList $plan.Erase 'Erase')+'}'),('set PAYLOAD_WRITE_SECTORS {'+(ConvertTo-CardPayloadTclList $plan.Write 'Write')+'}'))
        $record.phase='programming'; $record.write_attempted=$true; & $saveRecord
        $timer=[Diagnostics.Stopwatch]::StartNew()
        & $stage 'payload-program.cfg' $settings @('PASS card-payload-range-confined','PASS card-payload-prewrite-backup-verified','PASS card-payload-program-readback')
        $record.timings_seconds.program=[Math]::Round($timer.Elapsed.TotalSeconds,1)
        $record.phase='readback_verification'; & $saveRecord
        & $lockInput (Join-Path $session 'slot-readback.bin') 1048576 $record.expected_slot_sha256
        $record.status='verified_not_activated'; $record.phase='complete'; $record.timings_seconds.total=[Math]::Round($clock.Elapsed.TotalSeconds,1); & $saveRecord
        Write-Output "PASS $Action and full 1 MiB slot readback ($($record.expected_slot_sha256)). Erased sectors: $($plan.Erase -join ','); written: $($plan.Write -join ','). $($record.timings_seconds.total) s at $AdapterKhz kHz. No activation: power-cycle the target to use it. Backup/evidence (RestorePayload -RestoreSession $sessionId): $session"
    } catch {
        $failure=$_.Exception.Message
        $record.status='failed'; $record.error=$failure
        if (Test-Path -LiteralPath $session -PathType Container) { & $saveRecord }
        throw "Payload procedure stopped; no automatic restore or activation. The FPGA configuration below 0x400000 was not addressed. Evidence: $session. $failure"
    } finally {
        foreach ($handle in $locks) { $handle.Dispose() }
        if ($null -ne $operationLock) { $operationLock.Dispose() }
    }
}
