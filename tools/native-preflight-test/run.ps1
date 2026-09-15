[CmdletBinding()]
param(
    [ValidateSet('ReportedHypervisor','BoundaryUnavailable','AttributeUnavailable','AttributeReadDenied','BoundaryCanary','BoundarySse','BoundaryAvx','BoundaryXcr0Refused')][string]$Profile='ReportedHypervisor',
    [ValidateSet(1,4)][int]$Processors=1,
    [ValidateSet('None','Bind','Guest','Detector')][string]$Transition='None'
)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
if ($Transition -ne 'None' -and $Profile -notin @('BoundaryUnavailable','BoundaryCanary','BoundarySse','BoundaryAvx')) { throw 'Transition tests require a positive emulator fixture profile' }
$driverFeature=switch ($Transition) { 'Bind' {'native-transition-roundtrip'} 'Guest' {'native-transition-test'} 'Detector' {'native-transition-canary-negative'} default {'native-preflight'} }
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$qemuDir=Join-Path $root 'target/synthetic-tools/qemu-10.1.0'
$qemu=Join-Path $qemuDir 'qemu-system-x86_64.exe'
$version=@(& $qemu --version)[0]
if ($LASTEXITCODE -ne 0 -or $version -notmatch 'version 10\.1\.0\b') { throw 'QEMU 10.1.0 required' }
$session=Join-Path $root ('target/native-preflight-test/'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $session | Out-Null
$record=[ordered]@{profile=$Profile; transition=$Transition; driverFeature=$driverFeature; status='building'; emulatorOnly=$true; hardwareAccessed=$false; session=$session}
$oldDriver=$env:SVMVISOR_PREFLIGHT_DRIVER
try {
    & cargo build --locked --manifest-path (Join-Path $root 'Cargo.toml') -p svmvisor-dxe --profile dxe --target x86_64-unknown-uefi --features $driverFeature --target-dir (Join-Path $session 'driver-cargo')
    if ($LASTEXITCODE -ne 0) { throw 'DXE build failed' }
    $driver=Join-Path $session 'driver-cargo/x86_64-unknown-uefi/dxe/svmvisor-dxe.efi'
    $env:SVMVISOR_PREFLIGHT_DRIVER=$driver
    $launcherFeatures=@()
    if ($Profile -eq 'BoundaryUnavailable') { $launcherFeatures=@('--features','attribute-fixture') }
    if ($Profile -eq 'BoundaryCanary') { $launcherFeatures=@('--features','boundary-call') }
    if ($Profile -eq 'BoundarySse') { $launcherFeatures=@('--features','boundary-sse') }
    if ($Profile -eq 'BoundaryAvx') { $launcherFeatures=@('--features','boundary-avx') }
    if ($Profile -eq 'BoundaryXcr0Refused') { $launcherFeatures=@('--features','boundary-xcr0-refused') }
    if ($Profile -eq 'AttributeReadDenied') { $launcherFeatures=@('--features','attribute-read-denied') }
    & cargo build --locked --manifest-path (Join-Path $PSScriptRoot 'Cargo.toml') --release --target x86_64-unknown-uefi --target-dir (Join-Path $session 'launcher-cargo') @launcherFeatures
    if ($LASTEXITCODE -ne 0) { throw 'Launcher build failed' }
    $launcher=Join-Path $session 'launcher-cargo/x86_64-unknown-uefi/release/svmvisor-native-preflight-test.efi'
    $boot=Join-Path $session 'esp/EFI/BOOT'
    New-Item -ItemType Directory -Force -Path $boot | Out-Null
    Copy-Item -LiteralPath $launcher -Destination (Join-Path $boot 'BOOTX64.EFI')
    & (Join-Path $qemuDir 'qemu-img.exe') convert -f vvfat -O raw ('fat:'+(Join-Path $session 'esp')) (Join-Path $session 'esp.img')
    if ($LASTEXITCODE -ne 0) { throw 'ESP creation failed' }
    Copy-Item -LiteralPath (Join-Path $qemuDir 'share/edk2-i386-vars.fd') -Destination (Join-Path $session 'vars.fd')
    $debug=Join-Path $session 'debug.log'
    $serial=Join-Path $session 'serial.log'
    $cpu='max,svm=on,hypervisor='+$(if ($Profile -eq 'ReportedHypervisor') {'on'} else {'off'})
    $arguments=@('-machine','q35,accel=tcg','-cpu',$cpu,'-m','256M','-smp',$Processors.ToString(),
        '-drive',('"if=pflash,format=raw,readonly=on,file='+(Join-Path $qemuDir 'share/edk2-x86_64-code.fd')+'"'),
        '-drive',('"if=pflash,format=raw,file='+(Join-Path $session 'vars.fd')+'"'),
        '-drive',('"format=raw,snapshot=on,file='+(Join-Path $session 'esp.img')+'"'),
        '-display','none','-serial',('"file:'+$serial+'"'),'-monitor','none','-nic','none','-no-reboot',
        '-debugcon',('"file:'+$debug+'"'),'-device','isa-debug-exit,iobase=0xf4,iosize=0x04')
    $record.cpu=$cpu
    $record.processors=$Processors
    $record.driverSha256=(Get-FileHash -LiteralPath $driver).Hash
    $record.launcherSha256=(Get-FileHash -LiteralPath $launcher).Hash
    $record.qemuSha256=(Get-FileHash -LiteralPath $qemu).Hash
    $record.buildInputs=@('Cargo.toml','Cargo.lock','rust-toolchain.toml','.cargo/config.toml') | ForEach-Object { @{path=$_;sha256=(Get-FileHash -LiteralPath (Join-Path $root $_)).Hash} }
    $record.firmwareInputs=@('edk2-x86_64-code.fd','edk2-i386-vars.fd') | ForEach-Object { @{path=$_;sha256=(Get-FileHash -LiteralPath (Join-Path $qemuDir "share/$_")).Hash} }
    $record.sourceHashes=@(Get-ChildItem -LiteralPath (Join-Path $root 'crates/dxe'),(Join-Path $root 'crates/hypervisor'),$PSScriptRoot -File -Recurse | Where-Object { $_.FullName -notmatch '[\\/]target[\\/]' } | ForEach-Object { @{path=$_.FullName; sha256=(Get-FileHash -LiteralPath $_.FullName).Hash} })
    & llvm-objdump --disassemble $driver | Set-Content -LiteralPath (Join-Path $session 'driver-disassembly.txt')
    if ($LASTEXITCODE -ne 0) { throw 'Driver disassembly failed' }
    $process=Start-Process -FilePath $qemu -ArgumentList $arguments -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $session 'stdout.log') -RedirectStandardError (Join-Path $session 'stderr.log')
    $processHandle=$process.Handle
    if (-not $process.WaitForExit(45000)) { $process.Kill(); $process.WaitForExit(); throw 'Emulator timeout' }
    $process.Refresh()
    $record.exitCode=$process.ExitCode
    $trace=Get-Content -Raw -LiteralPath $debug
    if ($process.ExitCode -ne 33 -or $trace -notmatch 'PASS actual-dxe-returned-unsupported' -or $trace -notmatch 'PASS post-dxe-boot-services' -or $trace -match 'FAIL') { throw 'DXE return verification failed' }
    if ($trace -notmatch 'PASS observed-host-state-unchanged') { throw 'Missing independent host preservation check' }
    $console=Get-Content -Raw -LiteralPath $serial
    $expected=if ($Profile -eq 'ReportedHypervisor') { 'SVMVISOR native-preflight CPUID refused code=00000003' } else { 'SVMVISOR native-preflight native boundary unavailable code=00000100' }
    if ($Profile -in @('ReportedHypervisor','BoundaryXcr0Refused')) { $expected='assembly refused before Rust'; if ($console -match 'SVMVISOR snapshot ') { throw 'Early boundary refusal reached Rust' } }
    elseif ($console -notmatch [regex]::Escape($expected)) { throw 'Expected actual driver diagnostic missing from firmware console' }
    if ($Profile -notin @('ReportedHypervisor','BoundaryXcr0Refused')) {
        $cpuCounts=@{}
        foreach ($field in @('cpu-total','cpu-enabled','cpu-ap-completed','cpu-probe-processors','cpu-scoped-rflags','cpu-scoped-complete','entry-profile','entry-xstate-captured','entry-xstate-bytes')) {
            $items=[regex]::Matches($console,('SVMVISOR snapshot '+$field+'=([0-9a-f]{16})'))
            if ($items.Count -ne 1) { throw "Missing or duplicate entry/ownership field: $field" }
            $cpuCounts[$field]=[Convert]::ToUInt64($items[0].Groups[1].Value,16)
        }
        $expectedProfile=if ($Profile -eq 'BoundaryAvx') {7} elseif ($Profile -eq 'BoundarySse') {3} else {0}
        if ($cpuCounts['cpu-total'] -ne $Processors -or $cpuCounts['cpu-enabled'] -ne $Processors -or $cpuCounts['cpu-ap-completed'] -ne ($Processors-1) -or $cpuCounts['cpu-probe-processors'] -ne 1 -or $cpuCounts['cpu-scoped-complete'] -ne 1 -or ($cpuCounts['cpu-scoped-rflags'] -band 0x600) -ne 0) { throw 'CPU ownership interval evidence invalid' }
        if ($cpuCounts['entry-profile'] -ne $expectedProfile -or $cpuCounts['entry-xstate-captured'] -ne 1 -or $cpuCounts['entry-xstate-bytes'] -lt 512 -or $cpuCounts['entry-xstate-bytes'] -gt 1024) { throw 'Entry original xstate capture evidence invalid' }
        $record.entryAndCpuObservation=$cpuCounts
        $compared=0
        foreach ($field in @('cr0','cr3','cr4','gdtr-base','gdtr-limit','idtr-base','idtr-limit','cs','ss','ds','es','flags-if-df')) {
            $driverField=if ($field -eq 'flags-if-df') {'rflags'} else {$field}
            $observed=[regex]::Matches($console,('SVMVISOR snapshot '+[regex]::Escape($driverField)+'=([0-9a-f]{16})'))
            $baseline=[regex]::Matches($trace,('HOST '+[regex]::Escape($field)+'=([0-9a-f]{16})'))
            if ($observed.Count -ne 1 -or $baseline.Count -ne 1) { throw "Missing or duplicate snapshot field: $field" }
            $actual=[Convert]::ToUInt64($observed[0].Groups[1].Value,16)
            $prior=[Convert]::ToUInt64($baseline[0].Groups[1].Value,16)
            if ($field -eq 'flags-if-df') { $actual=$actual -band 0x600 }
            if ($field -eq 'cr4' -and $Profile -in @('BoundarySse','BoundaryAvx')) { $prior=$prior -bor 0x40000 }
            if ($actual -ne $prior) { throw "Driver/launcher snapshot mismatch: $field" }
            $compared++
        }
        $record.snapshotFieldsCompared=$compared
        $refusal=[regex]::Match($console,'SVMVISOR snapshot tables-refused=([0-9a-f]{16})')
        if ($Profile -in @('AttributeUnavailable','AttributeReadDenied')) {
            $expectedRefusal=if ($Profile -eq 'AttributeUnavailable') {1} else {7}
            if (-not $refusal.Success -or [Convert]::ToUInt64($refusal.Groups[1].Value,16) -ne $expectedRefusal -or $console -match 'tables-complete=') { throw 'Expected permission refusal missing or contradictory' }
            $record.tablesRefusal=$refusal.Groups[1].Value
        } elseif ($refusal.Success) {
            $record.tablesRefusal=$refusal.Groups[1].Value
            throw "Live table observation refused: $($record.tablesRefusal)"
        }
        if ($Profile -in @('BoundaryUnavailable','BoundaryCanary','BoundarySse','BoundaryAvx')) {
        $counts=@{}
        foreach ($field in @('tables-descriptors','tables-pages','tables-reads','tables-gdt-bytes','tables-retained-entries','tables-retained-pages','tables-scoped-complete','tables-complete')) {
            $items=[regex]::Matches($console,('SVMVISOR snapshot '+$field+'=([0-9a-f]{16})'))
            if ($items.Count -ne 1) { throw "Missing or duplicate table field: $field" }
            $counts[$field]=[Convert]::ToUInt64($items[0].Groups[1].Value,16)
        }
        $gdtLimit=[Convert]::ToUInt64([regex]::Match($trace,'HOST gdtr-limit=([0-9a-f]{16})').Groups[1].Value,16)
        $gdtBase=[Convert]::ToUInt64([regex]::Match($trace,'HOST gdtr-base=([0-9a-f]{16})').Groups[1].Value,16)
        $expectedPages=[uint64]([Math]::Floor((($gdtBase -band 4095)+$gdtLimit)/4096)+1)
        if ($counts['tables-complete'] -ne 1 -or $counts['tables-descriptors'] -lt 1 -or $counts['tables-descriptors'] -gt 4096 -or $counts['tables-gdt-bytes'] -ne ($gdtLimit+1) -or $counts['tables-pages'] -ne $expectedPages -or $counts['tables-reads'] -lt (2*$expectedPages) -or $counts['tables-reads'] -gt (4*$expectedPages)) { throw 'Invalid live table observation counts' }
        if ($counts['tables-scoped-complete'] -ne 1 -or $counts['tables-retained-entries'] -lt $counts['tables-reads'] -or $counts['tables-retained-entries'] -gt 256 -or $counts['tables-retained-pages'] -lt 1 -or $counts['tables-retained-pages'] -gt 128) { throw 'Retained table scope evidence invalid' }
        $record.tableObservation=$counts
        }
        if ($Profile -in @('BoundaryUnavailable','BoundaryCanary','BoundarySse','BoundaryAvx','AttributeReadDenied')) {
            if ($trace -notmatch 'FIXTURE memory-attributes installed' -or $trace -notmatch 'PASS memory-attribute-fixture-removed') { throw 'Explicit fixture lifecycle missing' }
            $record.permissionProvider='explicit-emulator-fixture'
        } else { $record.permissionProvider='firmware-protocol-unavailable' }
    } elseif ($console -match 'SVMVISOR snapshot ') { throw 'CPUID-rejected driver unexpectedly entered snapshot path' }
    if ($Profile -in @('BoundaryCanary','BoundarySse','BoundaryAvx') -and ($trace -notmatch 'PASS exact-driver-entry-gpr-flags-xmm' -or $trace -notmatch 'PASS directly-invoked-driver-unloaded')) { throw 'Missing exact entry canary/unload evidence' }
    if ($Profile -in @('BoundaryCanary','BoundarySse','BoundaryAvx','BoundaryXcr0Refused') -and $trace -notmatch 'PASS exact-driver-entry-x87-payload') { throw 'Missing x87 payload comparison' }
    if ($Profile -eq 'BoundaryAvx' -and $trace -notmatch 'PASS exact-driver-entry-upper-ymm') { throw 'Missing upper YMM comparison' }
    if ($Profile -eq 'BoundaryXcr0Refused' -and $trace -notmatch 'PASS exact-driver-entry-xcr0-refusal') { throw 'Missing original XCR0 refusal check' }
    if ($Transition -ne 'None') {
        if ($console -match 'transition-fixture-refused=') { throw 'Transition adapter refused; inspect serial log' }
        $transitionFields=@{}
        foreach ($field in @('outcome','refusal','progress','vmruns','exits','events-released','restored','gdt-accessed-restores','guest-exit','guest-rip','guest-rax','guest-captured','adapter-checks','canary-failures','canary-observed','canary-called','canary-changed')) {
            $items=[regex]::Matches($console,('SVMVISOR snapshot transition-'+$field+'=([0-9a-f]{16})'))
            if ($items.Count -ne 1) { throw "Missing or duplicate transition field: $field" }
            $transitionFields[$field]=[Convert]::ToUInt64($items[0].Groups[1].Value,16)
        }
        $record.transitionObservation=$transitionFields
        $expectedChanged=if ($Profile -eq 'BoundaryAvx') {7} else {3}
        if ($transitionFields['canary-changed'] -ne $expectedChanged) { throw 'Immediate canary seeds did not differ from actual incoming xstate' }
        if ($Transition -eq 'Detector') {
            $expectedFailure=if ($Profile -eq 'BoundaryAvx') {0xfb} else {0x7b}
            if ($transitionFields['canary-failures'] -ne $expectedFailure -or $transitionFields['canary-called'] -ne 1 -or $transitionFields['canary-observed'] -ne 1) { throw 'Deliberately broken callee was not detected as expected' }
            foreach ($field in @('outcome','refusal','progress','vmruns','exits','events-released','restored','gdt-accessed-restores','guest-exit','guest-rip','guest-rax','guest-captured','adapter-checks')) {
                if ($transitionFields[$field] -ne 0) { throw 'Detector test unexpectedly claimed transition execution' }
            }
            $record.detectorOnly=$true
        } else {
        $expectedOutcome=if ($Transition -eq 'Bind') {9} else {2}
        $expectedRuns=if ($Transition -eq 'Bind') {0} else {1}
        if ($transitionFields['outcome'] -ne $expectedOutcome -or $transitionFields['refusal'] -ne 0 -or $transitionFields['progress'] -ne 14 -or $transitionFields['vmruns'] -ne $expectedRuns -or $transitionFields['exits'] -ne $expectedRuns -or $transitionFields['events-released'] -ne 1 -or $transitionFields['restored'] -ne 1) { throw 'Actual transition completion evidence invalid' }
        if ($transitionFields['gdt-accessed-restores'] -gt 4 -or ($Transition -eq 'Bind' -and $transitionFields['gdt-accessed-restores'] -ne 0)) { throw 'Invalid bounded GDT accessed-bit repair count' }
        if ($transitionFields['canary-failures'] -ne 0 -or $transitionFields['canary-observed'] -ne 1 -or $transitionFields['canary-called'] -ne 1) { throw 'Immediate transition-call hardware canary evidence invalid' }
        if ($Transition -eq 'Bind') {
            if ($transitionFields['adapter-checks'] -ne 7 -or $transitionFields['guest-captured'] -ne 0) { throw 'Bind-only capture evidence invalid' }
        } elseif ($transitionFields['adapter-checks'] -ne 15 -or $transitionFields['guest-captured'] -ne 15 -or $transitionFields['guest-exit'] -ne 0x81 -or $transitionFields['guest-rip'] -ne 0x1096 -or $transitionFields['guest-rax'] -ne 0x53564d4e41544956) { throw 'Actual changed guest-register evidence invalid' }
        }
    }
    $record.diagnostic=$expected
    $record.status='passed'
    Write-Output "PASS $Profile actual DXE entry returned; Boot Services verified. $session"
} catch {
    $record.status='failed'; $record.error=$_.Exception.Message
    throw
} finally {
    $env:SVMVISOR_PREFLIGHT_DRIVER=$oldDriver
    $record | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $session 'result.json')
}




