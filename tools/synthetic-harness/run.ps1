[CmdletBinding()]
param([Parameter(Mandatory)][string]$QemuPath, [switch]$RustCore, [switch]$GuestCr8Unblock, [switch]$CorrectedBackend, [switch]$RequireCr8Faults, [switch]$DisableRdtscp, [switch]$HostFault, [switch]$DoubleFault, [switch]$HostWriteProtect, [switch]$HostGuard, [switch]$StackOverflow, [switch]$DfGuard, [switch]$XstateBrokenHostRestore, [switch]$XstateBrokenRestore, [ValidateSet('Avx','AvxOnly','Sse','Fx')][string]$XstateProfile = 'Avx')
if ($GuestCr8Unblock -or $HostFault -or $DoubleFault -or $HostWriteProtect -or $HostGuard -or $StackOverflow -or $DfGuard -or $XstateBrokenRestore -or $XstateBrokenHostRestore) { $RustCore = $true }
if ((@($HostFault, $DoubleFault, $HostWriteProtect, $HostGuard, $StackOverflow, $DfGuard) | Where-Object { $_ }).Count -gt 1) { throw "Select only one fault profile." }
$ErrorActionPreference = 'Stop'
if ($CorrectedBackend -and (-not $GuestCr8Unblock -or $DisableRdtscp)) { throw 'CorrectedBackend requires GuestCr8Unblock with RDTSCP enabled.' }
if ($RequireCr8Faults -and -not $CorrectedBackend) { throw 'RequireCr8Faults requires CorrectedBackend.' }
$expectCr8Gap = $GuestCr8Unblock -and -not $CorrectedBackend
if ($GuestCr8Unblock -and ($HostFault -or $DoubleFault -or $HostWriteProtect -or $HostGuard -or $StackOverflow -or $DfGuard -or $XstateBrokenRestore -or $XstateBrokenHostRestore)) { throw 'Select the guest CR8 diagnostic without another fault profile.' }
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$imageName = if ($GuestCr8Unblock) { 'guest-cr8-unblock.bin' } elseif ($XstateBrokenHostRestore) { 'xstate-broken-host-restore.bin' } elseif ($XstateBrokenRestore) { 'xstate-broken-restore.bin' } elseif ($StackOverflow) { 'stack-overflow.bin' } elseif ($DfGuard) { 'df-guard.bin' } elseif ($HostGuard) { 'host-guard.bin' } elseif ($DoubleFault) { 'double-fault.bin' } elseif ($HostWriteProtect) { 'host-write-protect.bin' } elseif ($HostFault) { 'host-fault.bin' } elseif ($RustCore) { 'rust-core.bin' } else { 'smoke.bin' }
$image = Join-Path $root "target/synthetic-harness/$imageName"
$qemu = (Resolve-Path -LiteralPath $QemuPath).Path
if (-not (Test-Path -LiteralPath $image)) { throw 'Run build.ps1 first.' }
$session = Join-Path $root ('target/synthetic-harness/runs/' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $session -Force | Out-Null
$debug = Join-Path $session 'debug.log'
$stdout = Join-Path $session 'stdout.log'
$stderr = Join-Path $session 'stderr.log'
# Fixed software emulation, no disk, NIC, passthrough, KVM or WHPX.
$cpu = 'max,svm=on,hypervisor=off' + $(if ($DisableRdtscp) { ',rdtscp=off' } else { '' }) + $(if ($XstateProfile -eq 'Fx') { ',xsave=off,avx=off,avx2=off' } elseif ($XstateProfile -eq 'AvxOnly') { ',avx2=off' } elseif ($XstateProfile -eq 'Sse') { ',avx=off,avx2=off' } else { '' })
$arguments = @('-machine','pc,accel=tcg','-cpu',$cpu,'-m','64M','-smp','1',
    '-no-reboot','-display','none','-monitor','none','-serial','none','-nic','none',
    '-debugcon',('"file:' + $debug + '"'),'-device','isa-debug-exit,iobase=0xf4,iosize=0x04',
    '-kernel',('"' + $image + '"'))
$process = Start-Process -FilePath $qemu -ArgumentList $arguments -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput $stdout -RedirectStandardError $stderr
if (-not $process.WaitForExit(30000)) {
    $process.Kill()
    $process.WaitForExit()
    throw "QEMU timed out after 30 seconds. Evidence: $session"
}
$process.Refresh()
$exitCode = $process.ExitCode
$trace = if (Test-Path -LiteralPath $debug) { Get-Content -Raw -LiteralPath $debug } else { '' }
$errors = Get-Content -Raw -LiteralPath $stderr
$trace | Write-Output
if ($errors) { $errors | Write-Output }
$record = [ordered]@{ imageSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $image).Hash;
    qemuSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $qemu).Hash;
    cpu = $cpu; rdtscpDisabled = [bool]$DisableRdtscp; xstateBrokenRestore = [bool]$XstateBrokenRestore; xstateBrokenHostRestore=[bool]$XstateBrokenHostRestore; exitCode = $exitCode; trace = $trace; stderr = $errors }
. (Join-Path $PSScriptRoot 'timing-evidence.ps1')
$timing = Get-TimingEvidence $trace
$record['timing'] = $timing
. (Join-Path $PSScriptRoot 'interrupt-evidence.ps1')
$interrupts = Get-InterruptEvidence $trace
$record['interrupts'] = $interrupts
$record['guestCr8Probe'] = [bool]$GuestCr8Unblock
$record['cr8FaultCorrectionRequired'] = [bool]$RequireCr8Faults
$record['correctedBackendRequired'] = [bool]$CorrectedBackend
$record['backendGap'] = $(if ($expectCr8Gap -and $exitCode -eq 3 -and $errors -match 'cpu_interrupt: assertion failed: \(bql_locked\(\)\)') { 'qemu-cr8-unblock-locking' } else { $null })
$record | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $session 'result.json')
$expected = if ($expectCr8Gap) { '(?m)^PASS guest-interrupt-if-shadow=16 iretq-once\r?$' } elseif ($XstateBrokenHostRestore) { 'FAIL xstate-host-isolation' } elseif ($XstateBrokenRestore) { 'FAIL xstate-guest-isolation' } elseif ($StackOverflow) { 'host-stack-overflow-probe\r?\nHOST-FAULT vector=0000000000000008\r?\nDOUBLE-FAULT-IST-PASS' } elseif ($HostGuard -or $DfGuard) { 'HOST-FAULT vector=000000000000000e' } elseif ($DoubleFault) { 'HOST-FAULT vector=0000000000000008\r?\nDOUBLE-FAULT-IST-PASS' } elseif ($HostWriteProtect) { 'HOST-FAULT vector=000000000000000e' } elseif ($HostFault) { 'HOST-FAULT.*0000000000000006' } elseif ($RustCore) { 'rust-exit=0000000000000072\r?\nrust-exit=0000000000000081\r?\nrust-exit=0000000000000081\r?\nfault-exit=0000000000000400\r?\nfault-exit=0000000000000400\r?\nfault-exit=000000000000004e\r?\nPASS xstate-isolation=32 xsetbv-blocked x87-arithmetic\r?\nPASS checked-faults\r?\nPASS repeated-sessions=32\r?\nPASS guest-exceptions=16 ud-gp-pf iretq-continuation\r?\nPASS guest-nested-delivery-refused\r?\n[\s\S]*PASS timing-contract tcg-only\r?\n[\s\S]*PASS rust-dispatch' } else { 'vmexit=0000000000000078\r?\nPASS svm-npt-hlt' }
$expectedExit = if ($expectCr8Gap) { 3 } elseif ($XstateBrokenRestore -or $XstateBrokenHostRestore) { 35 } elseif ($DoubleFault -or $StackOverflow) { 37 } elseif ($HostFault -or $HostWriteProtect -or $HostGuard -or $DfGuard) { 35 } else { 33 }
$profileValid = $true
if ($RustCore -and -not ($XstateBrokenRestore -or $XstateBrokenHostRestore -or $expectCr8Gap)) {
    $profileValid = $interrupts.validFixture -and $timing.validFixture -and $trace -match '(?m)^PASS timing-contract tcg-only\r?$' -and $trace -match '(?m)^PASS timing-rdtsc-intercept-refusal\r?$' -and $trace -match '(?m)^PASS repeated-sessions=32\r?$' -and $trace -match '(?m)^host-mappings-restricted\r?$' -and $trace -match '(?m)^PASS checked-faults\r?$'
}
if ($RustCore) {
    $profileMarker = switch ($XstateProfile) { 'Avx' { 'xsave-avx' } 'AvxOnly' { 'xsave-avx' } 'Sse' { 'xsave-sse' } 'Fx' { 'fxsave' } }
    $profileValid = $profileValid -and $trace -match "(?m)^xstate-profile=$profileMarker\r?$"
    if (-not ($XstateBrokenRestore -or $XstateBrokenHostRestore -or $expectCr8Gap)) {
        $profileValid = $profileValid -and $trace -match '(?m)^PASS guest-exceptions=16 ud-gp-pf iretq-continuation\r?$' -and $trace -match '(?m)^PASS guest-nested-delivery-refused\r?$'
    }
}
if ($XstateBrokenRestore -or $XstateBrokenHostRestore) { $profileValid = $profileValid -and $trace -notmatch 'PASS xstate-isolation|PASS repeated-sessions|PASS rust-dispatch' }
if ($HostGuard -or $DfGuard) {
    $target = [regex]::Match($trace, 'host-guard-target=([0-9a-f]{16})')
    $fault = [regex]::Match($trace, 'HOST-PF error=0000000000000000 cr2=([0-9a-f]{16})')
    $profileValid = $profileValid -and $target.Success -and $fault.Success -and $target.Groups[1].Value -eq $fault.Groups[1].Value
}
if ($HostWriteProtect) {
    $target = [regex]::Match($trace, 'host-write-target=([0-9a-f]{16})')
    $fault = [regex]::Match($trace, 'HOST-PF error=0000000000000003 cr2=([0-9a-f]{16})')
    $profileValid = $profileValid -and $target.Success -and $fault.Success -and $target.Groups[1].Value -eq $fault.Groups[1].Value
}
if ($expectCr8Gap) { $profileValid = $profileValid -and $null -ne $record['backendGap'] -and $trace -notmatch 'PASS guest-external-interrupts|PASS rust-dispatch' }
if ($CorrectedBackend) {
    # Require all earlier backend corrections. The newly measured CR8 operand
    # ordering gap is retained as incomplete coverage, never a hardware #GP pass.
    $knownCr8Gaps = @('GAP apic-cr8-gp=32 intercept-before-operand-fault refused-unchanged', 'GAP apic-cr8-direct-gp=32 reserved-operand-truncated')
    $otherGaps = ($trace -split '\r?\n' | Where-Object { $_ -match '^GAP ' -and $knownCr8Gaps -cnotcontains $_ })
    $profileValid = $profileValid -and $interrupts.validFixture -and $interrupts.priorityEquality -eq 'blocked' -and $interrupts.nestedPendingBit -eq 'architectural-clear-observed' -and $interrupts.nestedEventType -eq 'external-interrupt-observed' -and $interrupts.guestCr8Unblock -eq 'guest-write-checked' -and $timing.rdtscpIntercept -eq 'checked' -and -not $otherGaps
}
# Direct bootstrap has no retained post-EBS ownership permitting phase discard.
if ($interrupts.runningGuestPreemptionBaseline -eq 'post-ebs-rebased-phase-discarded-nonreturning') { $profileValid = $false }
if ($RequireCr8Faults) {
    $profileValid = $profileValid -and $interrupts.coverage -eq 'bounded-virtual-delivery-only' -and $interrupts.apicCr8FaultRetries -eq 32 -and $interrupts.apicCr8DirectFaultRetries -eq 32 -and $trace -notmatch '(?m)^GAP '
}
if (-not $profileValid -or $exitCode -ne $expectedExit -or $trace -notmatch $expected -or ($trace -match 'FAIL' -and -not ($XstateBrokenRestore -or $XstateBrokenHostRestore))) {
    throw "Emulator backend smoke failed (exit $exitCode). Evidence: $session"
}
if ($expectCr8Gap) { Write-Output "QEMU guest-CR8 backend gap reproduced. Evidence: $session" } else { Write-Output "Emulator $imageName passed. Evidence: $session" }
