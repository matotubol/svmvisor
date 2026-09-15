# Both runners retain backend gaps separately from bounded fixture completion.
function Get-InterruptEvidence([string]$Trace) {
    $stoppedDelivery = $Trace -match '(?m)^PASS guest-external-interrupts=32 if-shadow-stopped-tpr iretq-once\r?$'
    $guestDelivery = $Trace -match '(?m)^PASS guest-external-interrupts=32 if-shadow-guest-cr8 iretq-once\r?$'
    $delivery = $stoppedDelivery -xor $guestDelivery
    $window = $Trace -match '(?m)^PASS guest-vintr-window=32 pending-preserved\r?$'
    $conflicts = $Trace -match '(?m)^PASS guest-interrupt-conflicts-refused\r?$'
    $nested = $Trace -match '(?m)^PASS guest-interrupt-nested-refused\r?$'
    $equalityGap = $Trace -match '(?m)^GAP guest-interrupt-tpr-equality tcg-greater-or-equal\r?$'
    $equalityBlocked = $Trace -match '(?m)^PASS guest-interrupt-tpr-equality blocked\r?$'
    $nestedPendingGap = $Trace -match '(?m)^GAP guest-interrupt-nested-pending tcg-clears-after-idt\r?$'
    $nestedTypeGap = $Trace -match '(?m)^GAP guest-interrupt-nested-type tcg-exception-type\r?$'
    $cr8Skipped = $Trace -match '(?m)^SKIP guest-cr8-unblock backend-locking-gap\r?$'
    $cr8Written = $Trace -match '(?m)^PASS guest-cr8-unblock guest-write\r?$'
    $priorityValid = ($stoppedDelivery -and $cr8Skipped -and -not $cr8Written) -or ($guestDelivery -and $cr8Written -and -not $cr8Skipped)
    $controller = $Trace -match '(?m)^PASS local-apic=16 timer-eoi-priority iretq-once\r?$' -and $Trace -match '(?m)^PASS local-apic-eoi-adapter-refusal\r?$'
    $registers = $Trace -match '(?m)^PASS x2apic-msr=16 tpr-eoi-bitmaps\r?$' -and $Trace -match '(?m)^PASS x2apic-refusals=1 no-state-change\r?$'
    $faults = $Trace -match '(?m)^PASS msr-gp=160 repaired-operand-iretq-retry\r?$' -and $Trace -match '(?m)^PASS msr-gp-nested-delivery-refused\r?$'
    $modes = $Trace -match '(?m)^PASS apic-modes=16 disabled-xapic-x2apic\r?$'
    $cr8Writes = $Trace -match '(?m)^PASS apic-cr8-writes=32 same-class-priority-iretq\r?$' -and $Trace -match '(?m)^PASS apic-cr8-sources=16 gpr-vmcb-preserved\r?$'
    $cr8Faults = $Trace -match '(?m)^PASS apic-cr8-gp=32 hardware-fault-iretq-retry\r?$'
    $cr8FaultGap = $Trace -match '(?m)^GAP apic-cr8-gp=32 intercept-before-operand-fault refused-unchanged\r?$'
    $cr8DirectWrites = $Trace -match '(?m)^PASS apic-cr8-direct-writes=16 full-priority-range\r?$'
    $cr8DirectFaults = $Trace -match '(?m)^PASS apic-cr8-direct-gp=32 hardware-fault-iretq-retry\r?$'
    $cr8DirectGap = $Trace -match '(?m)^GAP apic-cr8-direct-gp=32 reserved-operand-truncated\r?$'
    # These separate markers make skipped MMIO, mode, delivery and refusal paths
    # fail both runners even when every historical interrupt marker is present.
    $mmio = [regex]::Matches($Trace, '(?m)^PASS xapic-mmio=16 id-tpr-ppr-irr-isr-eoi\r?$').Count -eq 1
    $crossMode = [regex]::Matches($Trace, '(?m)^PASS xapic-cross-mode=16 retained-owner\r?$').Count -eq 1
    $mmioIretq = [regex]::Matches($Trace, '(?m)^PASS xapic-iretq=16 exactly-once\r?$').Count -eq 1
    $mmioRefusals = [regex]::Matches($Trace, '(?m)^PASS xapic-refusals=13 actual-npf-unchanged\r?$').Count -eq 1
    $idFaults = [regex]::Matches($Trace, '(?m)^PASS x2apic-id-gp=16 repaired-tpr-iretq\r?$').Count -eq 1
    $svr = [regex]::Matches($Trace, '(?m)^PASS apic-svr=32 disabled-retained-irr-isr-iretq\r?$').Count -eq 1
    $lvt = [regex]::Matches($Trace, '(?m)^PASS apic-lvt=32 forced-mask-no-restore-version\r?$').Count -eq 1
    $timerRegisters = [regex]::Matches($Trace, '(?m)^PASS apic-timer-registers=32 oneshot-periodic-divided-source-ticks\r?$').Count -eq 1
    $timerGates = [regex]::Matches($Trace, '(?m)^PASS apic-timer-gates=32 software-and-base-disabled\r?$').Count -eq 1
    $registerFaults = [regex]::Matches($Trace, '(?m)^PASS apic-register-gp=112 repaired-operand-iretq-retry\r?$').Count -eq 1
    $registerRefusals = [regex]::Matches($Trace, '(?m)^PASS apic-register-refusals=13 actual-exit-unchanged\r?$').Count -eq 1
    $scheduled = [regex]::Matches($Trace, '(?m)^PASS apic-scheduled=64 clock-driven-hlt-eoi-iretq\r?$').Count -eq 1
    $scheduledOneshot = [regex]::Matches($Trace, '(?m)^PASS apic-scheduled-oneshot=32 completed-hlt-once\r?$').Count -eq 1
    $scheduledPeriodic = [regex]::Matches($Trace, '(?m)^PASS apic-scheduled-periodic=32 two-hlt-wakes\r?$').Count -eq 1
    $scheduledGates = [regex]::Matches($Trace, '(?m)^PASS apic-scheduled-gates=12 masked-if0-priority-svr-base-cancel\r?$').Count -eq 1
    $scheduledClock = [regex]::Matches($Trace, '(?m)^PASS apic-scheduled-clock=64 guest-host-monotonic-identity\r?$').Count -eq 1
    $scheduledRatio = [regex]::Matches($Trace, '(?m)^SCHEDULED-TIMER ratio-apic=1 ratio-tsc=1024 initial=4096\r?$').Count -eq 1
    $scheduledCounts = [regex]::Matches($Trace, '(?m)^SCHEDULED-TIMER delivered=96 halted=108 bounded-stops=12\r?$').Count -eq 1
    $scheduledMetrics = [ordered]@{}
    $scheduledMetricsValid = $true
    foreach ($name in @('samples', 'poll-samples', 'entries', 'before-deadline', 'sti-shadows', 'min-wake-lateness-tsc', 'max-wake-lateness-tsc')) {
        $matches = [regex]::Matches($Trace, ('(?m)^SCHEDULED-TIMER ' + [regex]::Escape($name) + '=([0-9a-f]{16})\r?$'))
        if ($matches.Count -eq 1) { $scheduledMetrics[$name] = [Convert]::ToUInt64($matches[0].Groups[1].Value, 16) }
        else { $scheduledMetrics[$name] = $null; $scheduledMetricsValid = $false }
    }
    if ($scheduledMetricsValid) {
        $scheduledMetricsValid = [decimal]$scheduledMetrics['samples'] -eq ([decimal]$scheduledMetrics['poll-samples'] + 644) -and $scheduledMetrics['poll-samples'] -ge 864 -and $scheduledMetrics['entries'] -eq 568 -and $scheduledMetrics['before-deadline'] -le 108 -and $scheduledMetrics['sti-shadows'] -eq 106 -and $scheduledMetrics['min-wake-lateness-tsc'] -le $scheduledMetrics['max-wake-lateness-tsc']
    }
    $preemptionValid = $true
    foreach ($marker in @(
        'PASS apic-preemption=32 running-loop-eoi-iretq',
        'PASS apic-preemption-oneshot=16 completed-once',
        'PASS apic-preemption-periodic=16 completed-twice',
        'PASS apic-preemption-gates=12 if0-priority-mask-svr-cancel-base',
        'PASS apic-preemption-restored timer-lvt-divide-tpr-svr-pic-idt-map-if',
        'PREEMPTION source=lapic-oneshot vector=f0 count=100000 divide=1',
        'PREEMPTION ratio-apic=1 ratio-tsc=1024 initial=4096 max-entries=256 negative-exits=64',
        'PREEMPTION delivered=48 armed=48 bounded-stops=12'
    )) {
        if ([regex]::Matches($Trace, ('(?m)^' + [regex]::Escape($marker) + '\r?$')).Count -ne 1) { $preemptionValid = $false }
    }
    $preemptionMetrics = [ordered]@{}
    $preemptionMetricsValid = $true
    foreach ($name in @('entries', 'intr-exits', 'spin-exits', 'no-progress-spin-exits', 'spin-progress', 'host-acks', 'voluntary-acks', 'min-lateness-tsc', 'max-lateness-tsc', 'min-entry-exit-tsc', 'max-entry-exit-tsc')) {
        $matches = [regex]::Matches($Trace, ('(?m)^PREEMPTION ' + [regex]::Escape($name) + '=([0-9a-f]{16})\r?$'))
        if ($matches.Count -eq 1) { $preemptionMetrics[$name] = [Convert]::ToUInt64($matches[0].Groups[1].Value, 16) }
        else { $preemptionMetrics[$name] = $null; $preemptionMetricsValid = $false }
    }
    if ($preemptionMetricsValid) {
        $p = $preemptionMetrics
        $preemptionMetricsValid = $p['entries'] -le 11264 -and [decimal]$p['entries'] -eq ([decimal]$p['intr-exits'] + 252) -and $p['intr-exits'] -ge 816 -and $p['spin-exits'] -ge 60 -and $p['spin-exits'] -le $p['intr-exits'] -and $p['no-progress-spin-exits'] -le $p['spin-exits'] -and ([decimal]$p['spin-exits'] - [decimal]$p['no-progress-spin-exits']) -ge 44 -and [decimal]$p['spin-progress'] -ge ([decimal]$p['spin-exits'] - [decimal]$p['no-progress-spin-exits']) -and [decimal]$p['host-acks'] -eq ([decimal]$p['intr-exits'] + [decimal]$p['voluntary-acks']) -and $p['voluntary-acks'] -le 252 -and $p['min-lateness-tsc'] -le $p['max-lateness-tsc'] -and $p['min-entry-exit-tsc'] -gt 0 -and $p['min-entry-exit-tsc'] -le $p['max-entry-exit-tsc']
    }
    $idleBaseline = [regex]::Matches($Trace, '(?m)^PASS apic-preemption-baseline idle-exact\r?$').Count
    $rebasedBaseline = [regex]::Matches($Trace, '(?m)^PASS apic-preemption-baseline post-ebs-rebased-phase-discarded-nonreturning\r?$').Count
    $hostTimerAdmission = [ordered]@{}
    $hostTimerValid = ($idleBaseline + $rebasedBaseline) -eq 1
    foreach ($name in @('initial', 'current', 'lvt', 'svr', 'tpr', 'divide')) {
        $matches = [regex]::Matches($Trace, ('(?m)^HOST-TIMER-ADMISSION ' + $name + '=([0-9a-f]{16})\r?$'))
        if ($matches.Count -eq 1) {
            $hostTimerAdmission[$name] = [Convert]::ToUInt64($matches[0].Groups[1].Value, 16)
            if ($hostTimerAdmission[$name] -gt [uint32]::MaxValue) { $hostTimerValid = $false }
        } else { $hostTimerAdmission[$name] = $null; $hostTimerValid = $false }
    }
    if ($hostTimerValid) {
        $h = $hostTimerAdmission
        $hostTimerValid = ($h['lvt'] -band 0x41000) -eq 0 -and $h['current'] -le $h['initial']
        if ($idleBaseline -eq 1) { $hostTimerValid = $hostTimerValid -and $h['initial'] -eq 0 -and $h['current'] -eq 0 }
        else { $hostTimerValid = $hostTimerValid -and $h['initial'] -gt 0 -and ($h['lvt'] -band 0x10000) -ne 0 -and [regex]::Matches($Trace, '(?m)^uefi-boot-services-exited\r?$').Count -eq 1 -and [regex]::Matches($Trace, '(?m)^PASS resident-ownership-retained\r?$').Count -eq 1 }
    }
    $preemptionValid = $preemptionValid -and $preemptionMetricsValid -and $hostTimerValid
    $interruptedValid = $true
    if ([regex]::Matches($Trace, '(?m)^IRQFAULT ').Count -ne 6 -or [regex]::Matches($Trace, '(?m)^DELIVERY ').Count -ne 7 -or [regex]::Matches($Trace, '(?m)^PASS (?:irq-handler-|interrupted-idt=|guest-shutdown=)').Count -ne 4) { $interruptedValid = $false }
    foreach ($marker in @(
        'PASS irq-handler-fault=24 ud-gp-pf-second-timer-xapic-x2apic',
        'PASS irq-handler-order=24 nested-frame-fault-iretq-two-eoi-two-iretq-once',
        'PASS interrupted-idt=24 ud-np-iretq-gp-pf-iretq-gp-pf-df-terminal-df-shutdown',
        'PASS guest-shutdown=4 actual-intercept-terminal'
    )) {
        if ([regex]::Matches($Trace, ('(?m)^' + [regex]::Escape($marker) + '\r?$')).Count -ne 1) { $interruptedValid = $false }
    }
    $irqFaultMetrics = [ordered]@{}
    $deliveryMetrics = [ordered]@{}
    foreach ($group in @(
        @{ Prefix = 'IRQFAULT'; Values = $irqFaultMetrics; Expected = @{ entries=312; faults=24; delivered=48; eois=48; 'min-entry-exit-tsc'=$null; 'max-entry-exit-tsc'=$null } },
        @{ Prefix = 'DELIVERY'; Values = $deliveryMetrics; Expected = @{ entries=84; 'secondary-faults'=28; 'np-iretq'=4; 'pf-iretq'=4; 'df-terminal'=12; 'resolved-shutdown'=4; 'intercepted-shutdown'=4 } }
    )) {
        foreach ($name in $group.Expected.Keys) {
            $matches = [regex]::Matches($Trace, ('(?m)^' + $group.Prefix + ' ' + [regex]::Escape($name) + '=([0-9a-f]{16})\r?$'))
            if ($matches.Count -ne 1) { $group.Values[$name] = $null; $interruptedValid = $false; continue }
            $value = [Convert]::ToUInt64($matches[0].Groups[1].Value, 16)
            $group.Values[$name] = $value
            if ($null -ne $group.Expected[$name] -and $value -ne $group.Expected[$name]) { $interruptedValid = $false }
        }
    }
    if ($irqFaultMetrics['min-entry-exit-tsc'] -le 0 -or $irqFaultMetrics['max-entry-exit-tsc'] -lt $irqFaultMetrics['min-entry-exit-tsc']) { $interruptedValid = $false }
    $multicoreValid = $true
    if ([regex]::Matches($Trace, '(?m)^MULTICORE ').Count -ne 9 -or [regex]::Matches($Trace, '(?m)^PASS multicore-').Count -ne 4) { $multicoreValid = $false }
    foreach ($marker in @(
        'PASS multicore-startup=8 real16-protected32-long64-two-guests-one-host',
        'PASS multicore-ipi=8 xapic-x2apic-bidirectional-coalesced-eoi-iretq',
        'PASS multicore-ownership=8 vmcb-gpr-xstate-auxiliary-fsbase-clock-stack',
        'PASS multicore-refusals=16 actual-icr-destination-running-init-unchanged'
    )) {
        if ([regex]::Matches($Trace, ('(?m)^' + [regex]::Escape($marker) + '\r?$')).Count -ne 1) { $multicoreValid = $false }
    }
    $multicoreMetrics = [ordered]@{}
    $multicoreExpected = @{ entries=296; 'real-starts'=8; 'long-starts'=8; checkpoints=80; delivered=16; eois=16; refused=16; 'min-entry-exit-tsc'=$null; 'max-entry-exit-tsc'=$null }
    foreach ($name in $multicoreExpected.Keys) {
        $matches = [regex]::Matches($Trace, ('(?m)^MULTICORE ' + [regex]::Escape($name) + '=([0-9a-f]{16})\r?$'))
        if ($matches.Count -ne 1) { $multicoreMetrics[$name]=$null; $multicoreValid=$false; continue }
        $value = [Convert]::ToUInt64($matches[0].Groups[1].Value,16)
        $multicoreMetrics[$name] = $value
        if ($null -ne $multicoreExpected[$name] -and $value -ne $multicoreExpected[$name]) { $multicoreValid=$false }
    }
    if ($multicoreMetrics['min-entry-exit-tsc'] -le 0 -or $multicoreMetrics['max-entry-exit-tsc'] -lt $multicoreMetrics['min-entry-exit-tsc']) { $multicoreValid=$false }
    $overlapValid = $true
    foreach ($marker in @(
        'PASS event-overlap=48 ud-gp-pf-if0-tpr-xapic-x2apic',
        'PASS event-overlap-order=48 fault-handler-iretq-sti-shadow-irq-eoi-iretq-once'
    )) {
        if ([regex]::Matches($Trace, ('(?m)^' + [regex]::Escape($marker) + '\r?$')).Count -ne 1) { $overlapValid = $false }
    }
    $overlapMetrics = [ordered]@{}
    foreach ($name in @('entries', 'deferred', 'faults', 'delivered', 'eois', 'min-entry-exit-tsc', 'max-entry-exit-tsc')) {
        $matches = [regex]::Matches($Trace, ('(?m)^OVERLAP ' + [regex]::Escape($name) + '=([0-9a-f]{16})\r?$'))
        if ($matches.Count -eq 1) { $overlapMetrics[$name] = [Convert]::ToUInt64($matches[0].Groups[1].Value, 16) }
        else { $overlapMetrics[$name] = $null; $overlapValid = $false }
    }
    if ($overlapValid) {
        $o = $overlapMetrics
        $overlapValid = $o['entries'] -eq 552 -and $o['deferred'] -eq 72 -and $o['faults'] -eq 48 -and $o['delivered'] -eq 48 -and $o['eois'] -eq 48 -and $o['min-entry-exit-tsc'] -gt 0 -and $o['min-entry-exit-tsc'] -le $o['max-entry-exit-tsc']
    }
    $valid = $overlapValid -and $preemptionValid -and $cr8Writes -and $scheduledMetricsValid -and $scheduled -and $scheduledOneshot -and $scheduledPeriodic -and $scheduledGates -and $scheduledClock -and $scheduledRatio -and $scheduledCounts -and $registerFaults -and $registerRefusals -and $svr -and $lvt -and $timerRegisters -and $timerGates -and $mmio -and $crossMode -and $mmioIretq -and $mmioRefusals -and $idFaults -and $cr8DirectWrites -and ($cr8DirectFaults -xor $cr8DirectGap) -and ($cr8Faults -xor $cr8FaultGap) -and $faults -and $modes -and $registers -and $controller -and $priorityValid -and $delivery -and $window -and $conflicts -and $nested -and ($equalityGap -xor $equalityBlocked)
    [ordered]@{
        validFixture = ($valid -and $overlapValid -and $interruptedValid -and $multicoreValid)
        multicore = $(if ($multicoreValid) { 'two-guest-vcpus-one-host-cooperative' } else { 'not-completed' })
        multicoreMetrics = $multicoreMetrics
        multicoreTiming = 'raw-emulator-tsc-monotonic-cooperative-order-not-physical-cross-cpu-latency'
        interruptedDelivery = $(if ($interruptedValid) { 'bounded-exception-combination-and-terminal-shutdown' } else { 'not-completed' })
        irqHandlerFaultSessions = $(if ($interruptedValid) { 24 } else { 0 })
        irqHandlerFaultMetrics = $irqFaultMetrics
        interruptedIdtSessions = $(if ($interruptedValid) { 24 } else { 0 })
        interruptedDeliveryMetrics = $deliveryMetrics
        coverage = $(if (-not $valid -or -not $interruptedValid -or -not $multicoreValid) { 'not-completed' } elseif ($cr8DirectGap -or $cr8FaultGap -or $cr8Skipped -or $equalityGap -or $nestedPendingGap -or $nestedTypeGap) { 'incomplete' } else { 'bounded-virtual-delivery-only' })
        deliveredSessions = $(if ($delivery) { 32 } else { 0 })
        readinessWindows = $(if ($window) { 32 } else { 0 })
        priorityEquality = $(if ($equalityGap) { 'emulator-gap' } elseif ($equalityBlocked) { 'blocked' } else { 'not-observed' })
        nestedDelivery = $(if ($nested) { 'refused' } else { 'not-observed' })
        nestedPendingBit = $(if ($nestedPendingGap) { 'emulator-gap' } elseif ($nested) { 'architectural-clear-observed' } else { 'not-observed' })
        nestedEventType = $(if ($nestedTypeGap) { 'emulator-gap' } elseif ($nested) { 'external-interrupt-observed' } else { 'not-observed' })
        runningGuestPreemptionBaseline = $(if (-not $hostTimerValid) { 'not-completed' } elseif ($idleBaseline -eq 1) { 'idle-exact' } else { 'post-ebs-rebased-phase-discarded-nonreturning' })
        eventOverlap = $(if ($overlapValid) { 'bounded-timer-fault-if-tpr-shadow-ordering' } else { 'not-completed' })
        eventOverlapSessions = $(if ($overlapValid) { 48 } else { 0 })
        eventOverlapMetrics = $overlapMetrics
        eventOverlapTiming = 'raw-emulator-tsc-entry-exit-span-not-physical-latency'
        hostTimerAdmission = $hostTimerAdmission
        runningGuestPreemption = $(if ($preemptionValid) { 'emulator-owned-lapic-intr' } else { 'not-completed' })
        runningGuestPreemptionSessions = $(if ($preemptionValid) { 32 } else { 0 })
        runningGuestPreemptionDeliveries = $(if ($preemptionValid) { 48 } else { 0 })
        runningGuestPreemptionGates = $(if ($preemptionValid) { 12 } else { 0 })
        runningGuestPreemptionMetrics = $preemptionMetrics
        runningGuestPreemptionTiming = 'raw-emulator-tsc-arm-entry-exit-cancel-ack-span-not-isolated-exit-cost'
        physicalInterruptRouting = 'not-tested'
        apicController = $(if ($controller) { 'bounded-internal-irr-isr-ppr-eoi' } else { 'not-completed' })
        apicRegisterInterface = $(if ($registers -and $mmio) { 'partial-fixed-xapic-mmio-x2apic-msr-fixture' } else { 'not-completed' })
        apicMmioSessions = $(if ($mmio) { 16 } else { 0 })
        apicCrossBusSessions = $(if ($crossMode) { 16 } else { 0 })
        apicMmioIretqSessions = $(if ($mmioIretq) { 16 } else { 0 })
        apicMmioRefusals = $(if ($mmioRefusals) { 13 } else { 0 })
        apicIdFaultRetries = $(if ($idFaults) { 16 } else { 0 })
        apicMmioCacheability = 'not-established-trapped-synthetic-access'
        apicSoftwareEnableSessions = $(if ($svr) { 32 } else { 0 })
        apicLvtSessions = $(if ($lvt) { 32 } else { 0 })
        apicLvtInventory = $(if ($lvt) { 'one-functional-timer-lvt' } else { 'not-completed' })
        apicTimerRegisterSessions = $(if ($timerRegisters) { 32 } else { 0 })
        apicTimerGateSessions = $(if ($timerGates) { 32 } else { 0 })
        apicTimerClock = 'supplied-source-ticks-no-physical-frequency'
        apicRegisterFaultRetries = $(if ($registerFaults) { 112 } else { 0 })
        apicRegisterPolicyRefusals = $(if ($registerRefusals) { 13 } else { 0 })
        apicScheduledSessions = $(if ($scheduled) { 64 } else { 0 })
        apicScheduledOneshotSessions = $(if ($scheduledOneshot) { 32 } else { 0 })
        apicScheduledPeriodicSessions = $(if ($scheduledPeriodic) { 32 } else { 0 })
        apicScheduledNoWakeSessions = $(if ($scheduledGates) { 12 } else { 0 })
        apicScheduledInterrupts = $(if ($scheduledCounts) { 96 } else { 0 })
        apicScheduledClock = $(if ($scheduledClock -and $scheduledRatio) { 'serialized-tsc-div1024-explicit-fixture' } else { 'not-completed' })
        apicSchedulingScope = 'stopped-guest-polling-no-running-guest-preemption'
        apicSchedulingMetrics = $scheduledMetrics
        apicSchedulingMetricUnits = 'emulator-tsc-counts-and-event-counts-not-physical-latency'
        apicModeTransitions = $(if ($modes) { 'bounded-disabled-xapic-x2apic' } else { 'not-completed' })
        apicModeSessions = $(if ($modes) { 16 } else { 0 })
        apicFaultInjection = $(if ($faults) { 'gp-zero-repaired-operand-iretq-retry' } else { 'not-completed' })
        apicFaultSessions = $(if ($faults) { 160 } else { 0 })
        apicCr8Writes = $(if ($cr8Writes) { 'bounded-gpr-tpr-synchronization' } else { 'not-completed' })
        apicCr8FaultOrdering = $(if ($cr8FaultGap) { 'emulator-gap-refused-unchanged' } elseif ($cr8Faults) { 'hardware-gp-iretq-retry' } else { 'not-observed' })
        apicCr8DirectFaultRetries = $(if ($cr8DirectFaults) { 32 } else { 0 })
        apicCr8DirectFaultOrdering = $(if ($cr8DirectGap) { 'emulator-gap-truncated' } elseif ($cr8DirectFaults) { 'hardware-gp-iretq-retry' } else { 'not-observed' })
        apicCr8FaultRetries = $(if ($cr8Faults) { 32 } else { 0 })
        apicCr8InvalidOperandRefusals = $(if ($cr8FaultGap) { 32 } else { 0 })
        apicRegisterSessions = $(if ($registers) { 16 } else { 0 })
        apicTimer = $(if ($timerRegisters -and $timerGates) { 'one-shot-periodic-programmable-divider-supplied-ticks' } else { 'not-completed' })
        apicControllerSessions = $(if ($controller) { 16 } else { 0 })
        nmiDelivery = 'not-implemented'
        guestCr8Unblock = $(if ($cr8Skipped) { 'skipped-backend-locking-gap' } elseif ($cr8Written) { 'guest-write-checked' } else { 'not-observed' })
        priorityUpdate = $(if (-not $valid) { 'not-completed' } elseif ($guestDelivery) { 'guest-cr8-write' } else { 'monitor-stopped-vmcb' })
    }
}
