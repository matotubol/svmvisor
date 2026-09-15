[CmdletBinding()]
param(
    [ValidateSet('Success','RejectAdmission','InvalidEntry','BrokenRestore','GuestUd','GuestPageFault','HostUd','HostGp','HostFaultMismatch','ArmedHostUd','ArmedHostGp','ArmedHostFaultMismatch','LoadedHostUd','LoadedHostGp','LoadedHostFaultMismatch','XstateHostUd','XstateHostGp','XstateHostFaultMismatch','PostExitHostUd','PostExitHostGp','PostExitHostFaultMismatch')][string]$Profile='Success',
    [ValidateSet('Fx','Sse','Avx')][string]$XstateProfile='Fx'
)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$qemu=Join-Path $root 'target/synthetic-tools/qemu-10.1.0/qemu-system-x86_64.exe'
$qemuDirectory=Split-Path $qemu
$code=Join-Path $qemuDirectory 'share/edk2-x86_64-code.fd'
$varsTemplate=Join-Path $qemuDirectory 'share/edk2-i386-vars.fd'
foreach($path in @($qemu,$code,$varsTemplate)) {
    if(-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Missing existing emulator dependency: $path" }
}
$versionLines=@(& $qemu --version)
$version=$versionLines[0]
if($LASTEXITCODE -ne 0 -or $version -notmatch 'version 10\.1\.0\b') { throw 'Expected pinned QEMU 10.1.0.' }
$session=Join-Path $root ('target/returning-probe/runs/'+[Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $session | Out-Null
$record=[ordered]@{schema=1;profile=$Profile;xstateProfile=$XstateProfile;status='building';emulatorOnly=$true;hardwareAccessed=$false;session=$session;exitCode=$null;timedOut=$false;error=$null}
function Save-Result { $record | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $session 'result.json') }
try {
    $feature=switch($Profile) { 'RejectAdmission' {'refuse-admission'} 'InvalidEntry' {'invalid-entry'} 'BrokenRestore' {'missing-restore'} 'GuestUd' {'guest-ud'} 'GuestPageFault' {'guest-page-fault'} 'HostUd' {'host-ud'} 'HostGp' {'host-gp'} 'HostFaultMismatch' {'host-fault-mismatch'} 'ArmedHostUd' {'armed-host-ud'} 'ArmedHostGp' {'armed-host-gp'} 'ArmedHostFaultMismatch' {'armed-host-fault-mismatch'} 'LoadedHostUd' {'loaded-host-ud'} 'LoadedHostGp' {'loaded-host-gp'} 'LoadedHostFaultMismatch' {'loaded-host-fault-mismatch'} 'XstateHostUd' {'xstate-host-ud'} 'XstateHostGp' {'xstate-host-gp'} 'XstateHostFaultMismatch' {'xstate-host-fault-mismatch'} 'PostExitHostUd' {'post-exit-host-ud'} 'PostExitHostGp' {'post-exit-host-gp'} 'PostExitHostFaultMismatch' {'post-exit-host-fault-mismatch'} default {''} }
    $features=@()
    if($feature) { $features+=$feature }
    if($XstateProfile -eq 'Sse') { $features+='fixture-sse' }
    if($XstateProfile -eq 'Avx') { $features+='fixture-avx' }
    $buildArgs=@('build','--locked','--manifest-path',(Join-Path $PSScriptRoot 'Cargo.toml'),'--target','x86_64-unknown-uefi','--release','--target-dir',(Join-Path $session 'cargo'))
    if($features.Count) { $buildArgs+=@('--features',($features -join ',')) }
    & cargo @buildArgs
    if($LASTEXITCODE -ne 0) { throw 'Returning EFI application build failed.' }
    $image=Join-Path $session 'cargo/x86_64-unknown-uefi/release/RETURNPROBE.efi'
    $bytes=[IO.File]::ReadAllBytes($image)
    $pe=[BitConverter]::ToInt32($bytes,0x3c)
    if([BitConverter]::ToUInt16($bytes,$pe+92) -ne 10) { throw 'Expected EFI application subsystem.' }
    $boot=Join-Path $session 'esp/EFI/BOOT'
    New-Item -ItemType Directory -Path $boot -Force | Out-Null
    Copy-Item -LiteralPath $image -Destination (Join-Path $boot 'BOOTX64.EFI')
    & llvm-objdump --disassemble $image | Set-Content -LiteralPath (Join-Path $session 'disassembly.txt')
    if($LASTEXITCODE -ne 0) { throw 'Disassembly failed.' }
    $espImage=Join-Path $session 'esp.img'
    & (Join-Path $qemuDirectory 'qemu-img.exe') convert -f vvfat -O raw ('fat:'+(Join-Path $session 'esp')) $espImage
    if($LASTEXITCODE -ne 0) { throw 'Disposable ESP image creation failed.' }
    $vars=Join-Path $session 'vars.fd'
    Copy-Item -LiteralPath $varsTemplate -Destination $vars
    $debug=Join-Path $session 'debug.log'
    $stderr=Join-Path $session 'stderr.log'
    $stdout=Join-Path $session 'stdout.log'
    $cpu='max,svm=on,hypervisor=on'+$(switch($XstateProfile) {
        'Fx' { ',xsave=off,avx=off,avx2=off' }
        'Sse' { ',xsave=on,xsavec=off,xsaves=off,avx=off,avx2=off' }
        'Avx' { ',xsave=on,xsavec=off,xsaves=off,avx=on,avx2=off' }
    })
    # Only generated media and a private variable store; no network, host disk,
    # USB passthrough, hardware acceleration, or installed firmware changes.
    $arguments=@('-machine','q35,accel=tcg','-cpu',$cpu,'-m','256M','-smp','1',
        '-drive',('"if=pflash,format=raw,readonly=on,file='+$code+'"'),
        '-drive',('"if=pflash,format=raw,file='+$vars+'"'),
        '-drive',('"format=raw,snapshot=on,file='+$espImage+'"'),
        '-no-reboot','-display','none','-monitor','none','-serial','none','-nic','none',
        '-debugcon',('"file:'+$debug+'"'),'-device','isa-debug-exit,iobase=0xf4,iosize=0x04')
    $record.status='running'; $record.cpu=$cpu
    $record.imageSha256=(Get-FileHash -LiteralPath $image).Hash
    $record.sourceHashes=@(Get-ChildItem -LiteralPath $PSScriptRoot,(Join-Path $root 'crates/hypervisor') -Recurse -File |
        Where-Object { $_.Extension -in @('.rs','.S','.toml','.lock','.ps1') } |
        ForEach-Object { @{path=$_.FullName.Substring($root.Length+1);sha256=(Get-FileHash -LiteralPath $_.FullName).Hash} })
    $record.qemuSha256=(Get-FileHash -LiteralPath $qemu).Hash
    $record.firmwareSha256=(Get-FileHash -LiteralPath $code).Hash
    $record.varsTemplateSha256=(Get-FileHash -LiteralPath $varsTemplate).Hash
    Save-Result
    $process=Start-Process -FilePath $qemu -ArgumentList $arguments -PassThru -WindowStyle Hidden -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    $processHandle=$process.Handle
    if(-not $process.WaitForExit(45000)) { $record.timedOut=$true; $process.Kill(); $process.WaitForExit() }
    $process.Refresh(); $record.exitCode=$process.ExitCode
    $trace=if(Test-Path -LiteralPath $debug) { Get-Content -Raw -LiteralPath $debug } else { '' }
    $expected=switch($Profile) {
        'Success' {'PASS returning-probe sessions=16'}
        'RejectAdmission' {'PASS returning-admission-refused'}
        'InvalidEntry' {'PASS returning-invalid-entry-restored'}
        'BrokenRestore' {'PASS returning-missing-restore-detected'}
        'GuestUd' {'PASS returning-guest-ud-restored'}
        'GuestPageFault' {'PASS returning-guest-page-fault-restored'}
        'HostUd' {'PASS returning-host-ud-restored'}
        'HostGp' {'PASS returning-host-gp-restored'}
        'HostFaultMismatch' {'FAIL host-fault-unexpected'}
        'ArmedHostUd' {'PASS returning-armed-host-ud-restored'}
        'ArmedHostGp' {'PASS returning-armed-host-gp-restored'}
        'ArmedHostFaultMismatch' {'FAIL host-fault-unexpected'}
        'LoadedHostUd' {'PASS returning-loaded-host-ud-restored'}
        'LoadedHostGp' {'PASS returning-loaded-host-gp-restored'}
        'LoadedHostFaultMismatch' {'FAIL host-fault-unexpected'}
        'XstateHostUd' {'PASS returning-xstate-host-ud-restored'}
        'XstateHostGp' {'PASS returning-xstate-host-gp-restored'}
        'XstateHostFaultMismatch' {'FAIL host-fault-unexpected'}
        'PostExitHostUd' {'PASS returning-post-exit-host-ud-restored'}
        'PostExitHostGp' {'PASS returning-post-exit-host-gp-restored'}
        'PostExitHostFaultMismatch' {'FAIL post-exit-fault-mismatch'}
    }
    $record.expectedMarker=$expected
    $record.entryCount=[regex]::Matches($trace,'(?m)^returning-session=[0-9a-f]{16} begin\r?$').Count
    $record.exitCount=[regex]::Matches($trace,'(?m)^returning-exit=[0-9a-f]{16} abi=[0-9a-f]{16}\r?$').Count
    $record.hostFaultSessionCount=[regex]::Matches($trace,'(?m)^host-fault-session=[0-9a-f]{16} begin\r?$').Count
    $record.hostFaultReturnCount=[regex]::Matches($trace,'(?m)^host-fault-return=[0-9a-f]{16} abi=[0-9a-f]{16}\r?$').Count
    Write-Output $trace
    if($Profile -in @('HostFaultMismatch','ArmedHostFaultMismatch','LoadedHostFaultMismatch','XstateHostFaultMismatch','PostExitHostFaultMismatch')) {
        $failures=@([regex]::Matches($trace,'(?m)^FAIL[^\r\n]*'))
        if($record.timedOut -or $null -eq $record.exitCode -or $record.exitCode -ne 35 -or $failures.Count -ne 1 -or $failures[0].Value -ne $expected -or $trace -match '(?m)^PASS\b' -or $record.entryCount -ne 0 -or $record.exitCount -ne 0 -or $record.hostFaultSessionCount -ne 1 -or $record.hostFaultReturnCount -ne 0) {
            throw 'Mismatched host fault did not terminate through the expected rejection path.'
        }
        $record.status='passed'; $record.outcome='expected-terminal-rejection'; Save-Result
        Write-Output "Returning probe $Profile/$XstateProfile passed its terminal rejection check. Evidence: $session"
        return
    }
    if($record.timedOut -or $null -eq $record.exitCode -or $record.exitCode -ne 33 -or $trace -notmatch ('(?m)^'+[regex]::Escape($expected)+'\r?$') -or $trace -match '(?m)^FAIL\b') {
        throw "Returning probe failed expected outcome: $Profile"
    }
    if($trace -notmatch '(?m)^PASS boot-services-after-return\r?$') { throw 'Missing independent post-return Boot Services evidence.' }
    if($XstateProfile -ne 'Fx' -and $trace -notmatch '(?m)^PASS outer-fixture-restored\r?$') { throw 'Missing original OVMF fixture restoration evidence.' }
    if($Profile -ne 'RejectAdmission') {
        if($Profile -in @('PostExitHostUd','PostExitHostGp')) {
            if($record.entryCount -ne 0 -or $record.exitCount -ne 0 -or $record.hostFaultSessionCount -ne 16 -or $record.hostFaultReturnCount -ne 16) { throw 'Expected sixteen post-exit host recovery sessions.' }
            if([regex]::Matches($trace,'(?m)^host-fault-return=0000000000000081 abi=0000000000000000\r?$').Count -ne 16) { throw 'Expected successful guest exits and preserved caller ABI.' }
            $postVector=if($Profile -eq 'PostExitHostUd') { '0000000000000006' } else { '000000000000000d' }
            if([regex]::Matches($trace,'(?m)^host-fault-vector='+$postVector+' rip=[0-9a-f]{16} error=0000000000000000 stage=0000000000000004 vmrun-attempts=0000000000000001\r?$').Count -ne 16) { throw 'Missing exact post-exit fault or one-entry evidence.' }
            $guestRecords=[regex]::Matches($trace,'(?m)^post-exit-guest-rip=([0-9a-f]{16}) expected=([0-9a-f]{16}) capture-stage=0000000000000001 sentinel=51554d5552455455\r?$')
            if($guestRecords.Count -ne 16) { throw 'Missing independent completed guest capture and sentinel records.' }
            foreach($guestRecord in $guestRecords) { if($guestRecord.Groups[1].Value -ne $guestRecord.Groups[2].Value) { throw 'Wrong guest exit RIP.' } }
            $record.postExitGuestReturnCount=$guestRecords.Count
            $postCheckpoints=[regex]::Matches($trace,'(?m)^host-checkpoint-efer=([0-9a-f]{16}) original-efer=([0-9a-f]{16}) hsave=([0-9a-f]{16}) owned-hsave=([0-9a-f]{16}) original-hsave=([0-9a-f]{16}) mutation-stage=0000000000000008 checkpoint-rflags=([0-9a-f]{16})\r?$')
            if($postCheckpoints.Count -ne 16) { throw 'Missing post-exit control-state records.' }
            foreach($checkpoint in $postCheckpoints) {
                $values=@(1..6 | ForEach-Object { [Convert]::ToUInt64($checkpoint.Groups[$_].Value,16) })
                if($values[0] -ne ($values[1] -bor 4096) -or ($values[1] -band 4096) -ne 0 -or $values[2] -ne $values[3] -or $values[2] -eq 0 -or ($values[2] -band 4095) -ne 0 -or $values[2] -eq $values[4] -or ($values[5] -band 512) -ne 0) { throw 'Invalid post-exit control state.' }
            }
            $postExpected=@{ '0000000000000190'='0000000000000000'; '0000000000000198'='0000000000000000' }
            if($XstateProfile -eq 'Avx') {
                $avxRecords=[regex]::Matches($trace,'(?m)^returning-avx-offset=([0-9a-f]{16})\r?$')
                if($avxRecords.Count -ne 1) { throw 'Expected one post-exit AVX component offset.' }
                $avxBase=[Convert]::ToUInt64($avxRecords[0].Groups[1].Value,16)
                if($avxBase -lt 576 -or $avxBase -gt 3840) { throw 'Invalid AVX component bounds.' }
                $postExpected[($avxBase+240).ToString('x16')]='0000000000000000'
                $postExpected[($avxBase+248).ToString('x16')]='0000000000000000'
            }
            foreach($fieldKind in @('xstate','extra')) {
                $fieldExpected=if($fieldKind -eq 'xstate') { $postExpected } else { @{ '0000000000000608'='0000000012345000'; '0000000000000610'='0000000023456000' } }
                $fieldRecords=[regex]::Matches($trace,'(?m)^post-exit-'+$fieldKind+'-offset=([0-9a-f]{16}) original=([0-9a-f]{16}) checkpoint=([0-9a-f]{16}) expected=([0-9a-f]{16}) restored=([0-9a-f]{16})\r?$')
                if($fieldRecords.Count -ne 16*$fieldExpected.Count) { throw "Missing post-exit $fieldKind observations." }
                $fieldCounts=@{}
                foreach($fieldRecord in $fieldRecords) {
                    $offset=$fieldRecord.Groups[1].Value
                    if(-not $fieldExpected.ContainsKey($offset)) { throw 'Unexpected post-exit canary offset.' }
                    if($fieldRecord.Groups[3].Value -ne $fieldExpected[$offset] -or $fieldRecord.Groups[4].Value -ne $fieldExpected[$offset] -or $fieldRecord.Groups[2].Value -eq $fieldRecord.Groups[3].Value -or $fieldRecord.Groups[5].Value -ne $fieldRecord.Groups[2].Value) { throw 'Post-exit changed state or restoration not proven.' }
                    if(-not $fieldCounts.ContainsKey($offset)) { $fieldCounts[$offset]=0 }
                    $fieldCounts[$offset]++
                }
                foreach($offset in $fieldExpected.Keys) { if(-not $fieldCounts.ContainsKey($offset) -or $fieldCounts[$offset] -ne 16) { throw 'Expected sixteen observations for each post-exit field.' } }
                $record['postExit'+$fieldKind+'ObservationCount']=$fieldRecords.Count
            }
        } elseif($Profile -in @('HostUd','HostGp','ArmedHostUd','ArmedHostGp','LoadedHostUd','LoadedHostGp','XstateHostUd','XstateHostGp')) {
            if($record.entryCount -ne 0 -or $record.exitCount -ne 0 -or $record.hostFaultSessionCount -ne 16 -or $record.hostFaultReturnCount -ne 16) { throw 'Expected sixteen host fault recoveries and zero guest entry attempts.' }
            $vector=if($Profile -in @('HostUd','ArmedHostUd','LoadedHostUd','XstateHostUd')) { '0000000000000006' } else { '000000000000000d' }
            $faultEvidence='(?m)^host-fault-vector='+$vector+' rip=[0-9a-f]{16} error=0000000000000000 stage=0000000000000004 vmrun-attempts=0000000000000000\r?$'
            if([regex]::Matches($trace,$faultEvidence).Count -ne 16) { throw 'Missing exact vector, error-code, completed cleanup, or zero-VMRUN evidence.' }
            if($Profile -in @('ArmedHostUd','ArmedHostGp','LoadedHostUd','LoadedHostGp','XstateHostUd','XstateHostGp')) {
                $mutationStage=if($Profile -in @('XstateHostUd','XstateHostGp')) { '0000000000000006' } elseif($Profile -in @('LoadedHostUd','LoadedHostGp')) { '0000000000000004' } else { '0000000000000002' }
                $checkpointPattern='(?m)^host-checkpoint-efer=([0-9a-f]{16}) original-efer=([0-9a-f]{16}) hsave=([0-9a-f]{16}) owned-hsave=([0-9a-f]{16}) original-hsave=([0-9a-f]{16}) mutation-stage='+$mutationStage+' checkpoint-rflags=([0-9a-f]{16})\r?$'
                $checkpoints=[regex]::Matches($trace,$checkpointPattern)
                if($checkpoints.Count -ne 16) { throw 'Expected sixteen completed SVM setup and recovery records.' }
                foreach($checkpoint in $checkpoints) {
                    $values=@(1..6 | ForEach-Object { [Convert]::ToUInt64($checkpoint.Groups[$_].Value,16) })
                    if($values[0] -ne ($values[1] -bor 4096) -or ($values[1] -band 4096) -ne 0 -or $values[2] -ne $values[3] -or $values[2] -eq 0 -or ($values[2] -band 4095) -ne 0 -or $values[2] -eq $values[4] -or ($values[5] -band 512) -ne 0) { throw 'Invalid observed SVME, owned HSAVE binding, or checkpoint IF state.' }
                }
                $record.mutatedCheckpointCount=$checkpoints.Count
            }
            if($Profile -in @('LoadedHostUd','LoadedHostGp','XstateHostUd','XstateHostGp')) {
                $extraPattern='(?m)^host-loaded-extra-offset=([0-9a-f]{16}) original=([0-9a-f]{16}) checkpoint=([0-9a-f]{16}) expected=([0-9a-f]{16}) restored=([0-9a-f]{16})\r?$'
                $extraRecords=[regex]::Matches($trace,$extraPattern)
                if($extraRecords.Count -ne 32) { throw 'Expected two independently observed changed/restored extra fields per return.' }
                $extraCounts=@{ '0000000000000608'=0; '0000000000000610'=0 }
                foreach($extraRecord in $extraRecords) {
                    $offset=$extraRecord.Groups[1].Value
                    if(-not $extraCounts.ContainsKey($offset)) { throw 'Unexpected VMLOAD canary field.' }
                    $expectedExtra=if($offset -eq '0000000000000608') { '0000000012345000' } else { '0000000023456000' }
                    if($extraRecord.Groups[3].Value -ne $expectedExtra -or $extraRecord.Groups[4].Value -ne $expectedExtra -or $extraRecord.Groups[2].Value -eq $expectedExtra -or $extraRecord.Groups[5].Value -ne $extraRecord.Groups[2].Value) { throw 'Missing real guest extra-state change or exact host restoration.' }
                    $extraCounts[$offset]++
                }
                if($extraCounts['0000000000000608'] -ne 16 -or $extraCounts['0000000000000610'] -ne 16) { throw 'Expected sixteen observations of each extra-state canary.' }
                $record.loadedExtraObservationCount=$extraRecords.Count
            }
            if($Profile -in @('XstateHostUd','XstateHostGp')) {
                $vectorExpected=@{ '0000000000000190'='13579bdf2468ace0'; '0000000000000198'='0fedcba987654321' }
                if($XstateProfile -eq 'Avx') {
                    $avxRecords=[regex]::Matches($trace,'(?m)^returning-avx-offset=([0-9a-f]{16})\r?$')
                    if($avxRecords.Count -ne 1) { throw 'Expected one CPUID-derived AVX component offset.' }
                    $avxBase=[Convert]::ToUInt64($avxRecords[0].Groups[1].Value,16)
                    if($avxBase -lt 576 -or $avxBase -gt 3840) { throw 'Invalid AVX component bounds.' }
                    $vectorExpected[($avxBase+240).ToString('x16')]='1122334455667788'
                    $vectorExpected[($avxBase+248).ToString('x16')]='8877665544332211'
                }
                $vectorPattern='(?m)^host-loaded-xstate-offset=([0-9a-f]{16}) original=([0-9a-f]{16}) checkpoint=([0-9a-f]{16}) expected=([0-9a-f]{16}) restored=([0-9a-f]{16})\r?$'
                $vectorRecords=[regex]::Matches($trace,$vectorPattern)
                if($vectorRecords.Count -ne 16*$vectorExpected.Count) { throw 'Missing independent changed/restored vector observations.' }
                $vectorCounts=@{}
                foreach($vectorRecord in $vectorRecords) {
                    $offset=$vectorRecord.Groups[1].Value
                    if(-not $vectorExpected.ContainsKey($offset)) { throw 'Unexpected guest vector canary offset.' }
                    if($vectorRecord.Groups[3].Value -ne $vectorExpected[$offset] -or $vectorRecord.Groups[4].Value -ne $vectorExpected[$offset] -or $vectorRecord.Groups[2].Value -eq $vectorRecord.Groups[3].Value -or $vectorRecord.Groups[5].Value -ne $vectorRecord.Groups[2].Value) { throw 'Guest vector load or exact host restoration was not observed.' }
                    if(-not $vectorCounts.ContainsKey($offset)) { $vectorCounts[$offset]=0 }
                    $vectorCounts[$offset]++
                }
                foreach($offset in $vectorExpected.Keys) {
                    if(-not $vectorCounts.ContainsKey($offset) -or $vectorCounts[$offset] -ne 16) { throw 'Expected sixteen observations of each vector canary.' }
                }
                $record.loadedXstateObservationCount=$vectorRecords.Count
            }
        } else {
            if($record.entryCount -ne 16 -or $record.exitCount -ne 16 -or $record.hostFaultSessionCount -ne 0 -or $record.hostFaultReturnCount -ne 0) { throw 'Expected exactly sixteen independently logged guest entries and returns, without host fault injection.' }
        }
        $profileMarker=switch($XstateProfile) { 'Fx' {'fxsave'} 'Sse' {'xsave-sse'} 'Avx' {'xsave-avx'} }
        if($trace -notmatch ('(?m)^returning-xstate='+$profileMarker+'\r?$')) { throw 'Missing exact exercised state profile.' }
    } elseif($record.entryCount -ne 0 -or $record.exitCount -ne 0) {
        throw 'Refused admission must not attempt guest entry.'
    }
    $record.status='passed'; Save-Result
    Write-Output "Returning probe $Profile/$XstateProfile passed. Evidence: $session"
} catch {
    $record.status='failed'; $record.error=$_.Exception.Message; Save-Result
    throw "Returning probe stopped: $($_.Exception.Message) Evidence: $session"
}
