# Shared trace accounting for the Multiboot and UEFI runners. A known emulator
# gap is retained as incomplete coverage, never upgraded to intercept success.
function Get-TimingEvidence([string]$Trace) {
    $rdtsc = [regex]::Matches($Trace, '(?m)^timing-sample=rdtsc\r?$').Count
    $rdtscp = [regex]::Matches($Trace, '(?m)^timing-sample=rdtscp\r?$').Count
    $gap = $Trace -match '(?m)^GAP timing-rdtscp-intercept not-observed tcg-only\r?$'
    $skip = $Trace -match '(?m)^SKIP timing-rdtscp unsupported\r?$'
    $intercept = $Trace -match '(?m)^PASS timing-rdtscp-intercept-refusal\r?$'
    $rdtscpValid = (($rdtscp -eq 16) -and -not $skip -and ($gap -xor $intercept) -and
        ($Trace -match '(?m)^PASS timing-rdtscp=16 monotonic\r?$')) -or
        ($skip -and $rdtscp -eq 0 -and -not $gap -and -not $intercept)
    $valid = $rdtsc -eq 16 -and $rdtscpValid -and
        ($Trace -match '(?m)^PASS clock-msr-refusal=6 stopped-state-preserved\r?$') -and
        ($Trace -match '(?m)^PASS clock-boundary host-restored\r?$') -and
        ($Trace -match '(?m)^PASS timing-rdtsc=16 monotonic\r?$') -and
        ($Trace -match '(?m)^PASS timing-rdtsc-intercept-refusal\r?$') -and
        ($Trace -match '(?m)^PASS timing-contract tcg-only\r?$')
    [ordered]@{
        validFixture = $valid
        coverage = $(if (-not $valid) { 'not-completed' } elseif ($gap -or $skip) { 'incomplete' } else { 'bounded-fixture-only' })
        rdtscSamples = $rdtsc
        rdtscpSamples = $rdtscp
        rdtscpIntercept = $(if ($gap) { 'emulator-gap' } elseif ($skip) { 'unsupported' } elseif ($intercept) { 'checked' } else { 'not-observed' })
        physicalLatencyMeasured = $false
        clockDriftMeasured = $false
        clockBoundary = $(if ($valid) { 'host-restoration-checked' } else { 'not-completed' })
        clockMsrPolicy = $(if ($valid) { 'all-accesses-refused' } else { 'not-completed' })
        ratioExecution = $(if ($Trace -match '(?m)^clock-ratio=identity-owned\r?$') { 'identity-installed' } elseif ($Trace -match '(?m)^SKIP clock-ratio unsupported\r?$') { 'unsupported-not-accessed' } else { 'not-observed' })
    }
}
