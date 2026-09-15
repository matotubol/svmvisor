function Get-ConcurrentEvidence([string]$Trace) {
    $valid=$true
    if([regex]::Matches($Trace,'(?m)^PASS rust-dispatch\r?$').Count -ne 1 -or $Trace -match '(?m)^(FAIL |REFUSE |AUXILIARY-MISMATCH )'){$valid=$false}
    $markers=@(
        'PASS concurrent-smp=8 two-host-two-guest-xapic-x2apic',
        'PASS concurrent-ipi=64 handler-eoi-iretq-coalescing-running-simultaneous-hlt',
        'PASS concurrent-startup=1 target-owned-init-sipi-real16-protected32-long64',
        'PASS concurrent-hlt=16 bidirectional-before-park-after-park-after-empty-poll',
        'PASS concurrent-entry-race=1 publish-after-drain-before-vmrun',
        'PASS concurrent-ownership=2 vmcb-gpr-xstate-fsbase-clock-stack-host-restored',
        'PASS concurrent-clock=8 per-cpu-monotonic-ordered-shared-handoff',
        'PASS concurrent-timer=32 two-cpu-xapic-x2apic-spin-hlt-eoi-iretq',
        'PASS concurrent-idle=32 guest-hlt-sessions-actual-host-sti-hlt-cli-source-witness',
        'PASS concurrent-watchdog=4 host-wake-without-guest-delivery'
    )
    foreach($marker in $markers) {
        if([regex]::Matches($Trace,('(?m)^'+[regex]::Escape($marker)+'\r?$')).Count -ne 1){$valid=$false}
    }
    if([regex]::Matches($Trace,'(?m)^PASS concurrent-').Count -ne $markers.Count){$valid=$false}
    $names=@('entries','intrs','running-intrs','acks','sent','delivered','eois','queries','entry-races',
        'startup-sent','startup-applied','real-starts','long-starts','hlts','wakes',
        'before-park-races','after-park-races','after-poll-races',
        'min-entry-exit-tsc','max-entry-exit-tsc','last-guest-tsc',
        'timer-acks','timer-intrs','ipi-intrs','both-source-intrs','voluntary-timer-acks',
        'timer-delivered','timer-eois','timer-programs','timer-hlts','timer-wakes',
        'spin-intrs','spin-progress','spin-no-progress','spin-sessions',
        'idle-returns','idle-timer-acks','idle-ipi-acks','idle-timer-witnesses','idle-ipi-witnesses',
        'watchdog-only-wakes','min-deadline-lateness-tsc','max-deadline-lateness-tsc',
        'min-host-idle-tsc','max-host-idle-tsc','timer-cancel-races','timer-cancel-active-races')
    if([regex]::Matches($Trace,'(?m)^CONCURRENT ').Count -ne (2*$names.Count)){$valid=$false}
    $metrics=[ordered]@{}
    foreach($cpu in 0..1) {
        $v=[ordered]@{}
        foreach($name in $names) {
            $matches=[regex]::Matches($Trace,"(?m)^CONCURRENT cpu$cpu-$name=([0-9a-f]{16})\r?$")
            if($matches.Count -ne 1){$v[$name]=$null;$valid=$false;continue}
            $v[$name]=[Convert]::ToUInt64($matches[0].Groups[1].Value,16)
        }
        $expected=@{sent=40;delivered=32;eois=32;queries=64;'entry-races'=$cpu;
            'startup-sent'=(3*(1-$cpu));'startup-applied'=(2*$cpu);'real-starts'=$cpu;'long-starts'=$cpu;
            hlts=8;wakes=8;'before-park-races'=3;'after-park-races'=3;'after-poll-races'=2;
            'timer-delivered'=16;'timer-eois'=16;'timer-programs'=16;'timer-hlts'=8;'timer-wakes'=8;
            'spin-sessions'=8;'watchdog-only-wakes'=2}
        foreach($name in $expected.Keys){if($v[$name] -ne $expected[$name]){$valid=$false}}
        if($v.entries -gt 4096 -or $v.entries -ne (251-2*$cpu+$v.intrs) -or $v.intrs -lt 8 -or $v['running-intrs'] -lt 1 -or $v['running-intrs'] -gt $v['ipi-intrs']){$valid=$false}
        if($v.acks -lt 1 -or $v.acks -gt (40+3*$cpu) -or $v['last-guest-tsc'] -lt 1){$valid=$false}
        foreach($pair in @(@('min-entry-exit-tsc','max-entry-exit-tsc'),@('min-host-idle-tsc','max-host-idle-tsc'))){
            if($v[$pair[0]] -lt 1 -or $v[$pair[1]] -lt $v[$pair[0]]){$valid=$false}
        }
        if($v['max-deadline-lateness-tsc'] -lt $v['min-deadline-lateness-tsc']){$valid=$false}
        # F0 and F1 may be acknowledged at the same INTR boundary. These are
        # source-attributed exit counts; IPI acknowledgements can coalesce.
        if($v.intrs -ne ($v['timer-intrs']+$v['ipi-intrs']-$v['both-source-intrs']) -or
            $v['timer-intrs'] -gt $v.intrs -or $v['ipi-intrs'] -gt $v.intrs -or
            $v['both-source-intrs'] -gt [Math]::Min($v['timer-intrs'],$v['ipi-intrs']) -or
            $v['ipi-intrs']+$v['idle-ipi-acks'] -gt $v.acks){$valid=$false}
        if($v['timer-acks'] -ne ($v['timer-intrs']+$v['voluntary-timer-acks']+$v['idle-timer-acks']) -or
            $v['voluntary-timer-acks'] -gt (251-2*$cpu) -or $v['idle-timer-acks'] -gt $v['idle-returns']){$valid=$false}
        if($v['spin-intrs'] -lt 8 -or $v['spin-intrs'] -gt $v['timer-intrs'] -or
            $v['spin-no-progress'] -gt ($v['spin-intrs']-$v['spin-sessions']) -or $v['spin-progress'] -lt 8){$valid=$false}
        # 16 guest HLTs plus two deliberately watchdog-only host returns per CPU.
        if($v['idle-returns'] -lt 18 -or $v['idle-returns'] -gt (18*1024) -or $v['idle-timer-acks'] -lt 10 -or
            $v['idle-ipi-witnesses'] -lt 1 -or $v['idle-timer-witnesses'] -lt 2 -or
            $v['idle-timer-witnesses'] -gt $v['idle-timer-acks'] -or $v['idle-ipi-witnesses'] -gt $v['idle-ipi-acks'] -or
            $v['idle-timer-witnesses']+$v['idle-ipi-witnesses'] -lt $v['idle-returns']){$valid=$false}
        if($v['timer-cancel-active-races'] -gt $v['timer-cancel-races'] -or $v['timer-cancel-races'] -gt $v['timer-acks']){$valid=$false}
        $metrics["cpu$cpu"]=$v
    }
    [ordered]@{validFixture=$valid;coverage=$(if($valid){'two-cpu-timer-preemption-host-idle-bounded'}else{'not-completed'});metrics=$metrics;timing='raw-mttcg-tsc-not-native-latency'}
}

