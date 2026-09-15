# Positive execution evidence for the deliberately pinned two-CPU UEFI profile.
# This validates retained observations; it does not execute or repair a guest.
. (Join-Path $PSScriptRoot 'concurrent-evidence.ps1')

function Get-UefiSmpEvidence {
    param([Parameter(Mandatory)]$Record)
    try { Get-UefiSmpEvidenceCore -Record $Record }
    catch {
        [ordered]@{validFixture=$false;errors=@('malformed-evidence: ' + $_.Exception.Message)
            coverage='not-completed'}
    }
}

function Get-UefiSmpEvidenceCore {
    param([Parameter(Mandatory)]$Record)
    $violations = [System.Collections.Generic.List[string]]::new()
    $trace = [string]$Record.trace
    $positions = @{}
    $values = @{}
    $unique = {
        param([string]$Name, [string]$Pattern)
        $found = [regex]::Matches($trace, '(?m)^' + $Pattern + '\r?$')
        if ($found.Count -ne 1) { $violations.Add("unique:$Name"); return $null }
        $positions[$Name] = $found[0].Index
        return $found[0]
    }
    $number = {
        param([string]$Name, [string]$Prefix)
        if ([regex]::Matches($trace, '(?m)^' + [regex]::Escape($Prefix)).Count -ne 1) {
            $violations.Add("numeric-marker-count:$Name")
        }
        $found = & $unique $Name ([regex]::Escape($Prefix) + '([0-9a-f]{16})')
        if ($null -eq $found) { $values[$Name] = [UInt64]0; return }
        $values[$Name] = [Convert]::ToUInt64($found.Groups[1].Value, 16)
    }
    $ordered = {
        param([string[]]$Names)
        $prior = -1
        foreach ($name in $Names) {
            if (-not $positions.ContainsKey($name)) { continue }
            if ($positions[$name] -le $prior) { $violations.Add("order:$name") }
            $prior = $positions[$name]
        }
    }
    $required = @('trace','uefiSmp','hostCpuCount','expectedSmpRefusal','exitCode','timedOut',
        'stderr','cpu','machine','arguments','xstateProfile','rdtscpDisabled','sipiPageRequest',
        'requestedArenaBase','actualArenaBase','linkedEntryAddress','entryAddress',
        'qemuSha256','firmwareSha256','varsTemplateSha256','payloadSha256',
        'relocationPackageSha256','originalPackageSha256','loaderSha256','driverSha256')
    foreach ($name in $required) {
        $present = if ($Record -is [System.Collections.IDictionary]) {
            $Record.Contains($name)
        } else { $null -ne $Record.PSObject.Properties[$name] }
        if (-not $present) { $violations.Add("field:$name") }
    }
    if ($Record.uefiSmp -ne $true -or $Record.hostCpuCount -ne 2 -or
        $Record.expectedSmpRefusal -ne 'None' -or $Record.exitCode -ne 33 -or
        $Record.timedOut -ne $false -or -not [string]::IsNullOrEmpty([string]$Record.stderr)) {
        $violations.Add('execution-status')
    }
    foreach ($name in @('uefiSmp','timedOut','rdtscpDisabled')) {
        if ($Record.$name -isnot [bool]) { $violations.Add("boolean:$name") }
    }
    foreach ($name in @('rejectMmio','reconnect','pciRom','omittedRom','productionDxe',
        'noAuthorization','rejectHandoff','rejectOwnership','rejectRelocation',
        'rejectResidentOwnership','expectArenaRejection','expectAllocationRejection')) {
        if ($Record.$name) { $violations.Add("positive-profile:$name") }
    }
    $pins = @{
        qemuSha256 = 'c867dd99822400be12f1aa3fbf9995ea5f103486fd3affede6076cd444cb3047'
        firmwareSha256 = '33090cc07675baa5190d9f1e84bf5176b33bcbfa9bacac522961150cdb6dbb2a'
        varsTemplateSha256 = '5d2ac383371b408398accee7ec27c8c09ea5b74a0de0ceea6513388b15be5d1e'
    }
    foreach ($name in $pins.Keys) {
        if ([string]$Record.$name -ine $pins[$name]) { $violations.Add("pin:$name") }
    }
    foreach ($name in @('payloadSha256','relocationPackageSha256','originalPackageSha256',
        'loaderSha256','driverSha256')) {
        if ([string]$Record.$name -notmatch '^[0-9a-fA-F]{64}$') { $violations.Add("hash:$name") }
    }
    if ($Record.relocationPackageSha256 -ine $Record.originalPackageSha256) {
        $violations.Add('changed-package')
    }
    $cpu = 'max,svm=on,hypervisor=off'
    $profile = switch ([string]$Record.xstateProfile) {
        'Avx' { 'xsave-avx' }
        'AvxOnly' { $cpu += ',avx2=off'; 'xsave-avx' }
        'Sse' { $cpu += ',avx=off,avx2=off'; 'xsave-sse' }
        'Fx' { $cpu += ',xsave=off,avx=off,avx2=off'; 'fxsave' }
        default { $violations.Add('xstate-profile'); 'invalid' }
    }
    if ($Record.rdtscpDisabled -eq $true) { $cpu += ',rdtscp=off' }
    if ($Record.cpu -cne $cpu -or $Record.machine -cne 'pc-q35-10.1') {
        $violations.Add('machine-cpu-profile')
    }
    $arguments = @($Record.arguments)
    foreach ($pair in @(@('-machine','pc-q35-10.1'),@('-cpu',$cpu),@('-m','256M'),
        @('-smp','2,sockets=1,cores=2,threads=1'),@('-accel','tcg,thread=multi'),
        @('-nic','none'),@('-display','none'),@('-monitor','none'),@('-serial','none'))) {
        $indices = @(for ($i=0; $i -lt $arguments.Count; $i++) {
            if ($arguments[$i] -ceq $pair[0]) { $i }
        })
        if ($indices.Count -ne 1 -or $indices[0]+1 -ge $arguments.Count -or
            $arguments[$indices[0]+1] -cne $pair[1]) { $violations.Add("argument:$($pair[0])") }
    }
    $lifecycle = @('uefi-launcher-entry','uefi-driver-loaded','uefi-driver-entry',
        'uefi-smp-mp-callbacks-returned=1','uefi-smp-pre-ebs-admitted','uefi-payload-reserved',
        'uefi-boot-services-exited','uefi-smp-post-ebs-owned','uefi-private-entry','rust-core',
        'PASS resident-ownership-retained','host-idt-installed','host-memory-rx-ro-rw',
        'host-mappings-restricted','PASS resident-smp-low-reservation','PASS resident-ownership-private',
        "xstate-profile=$profile",'PASS resident-guest-exclusion','PASS rust-dispatch')
    foreach ($marker in $lifecycle) { $null = & $unique $marker ([regex]::Escape($marker)) }
    & $ordered $lifecycle
    if ([regex]::Matches($trace,'(?m)^xstate-profile=').Count -ne 1) {
        $violations.Add('xstate-marker-count')
    }
    $count = & $unique 'cpu-count' 'uefi-smp-firmware-count total=0x([0-9a-f]{16}) enabled=0x([0-9a-f]{16}) bsp=0x([0-9a-f]{16})'
    if ($null -ne $count -and ($count.Groups[1].Value -ne '0000000000000002' -or
        $count.Groups[2].Value -ne '0000000000000002' -or $count.Groups[3].Value -ne '0000000000000000')) {
        $violations.Add('firmware-topology')
    }
    $identities = [regex]::Matches($trace,'(?m)^uefi-smp-cpu processor=0x([0-9a-f]{16}) apic=0x([0-9a-f]{16}) signature=0x([0-9a-f]{16}) vendor=AuthenticAMD\r?$')
    $cpus = @()
    if ($identities.Count -ne 2 -or [regex]::Matches($trace,'(?m)^uefi-smp-cpu ').Count -ne 2) {
        $violations.Add('cpu-identities')
    } else {
        foreach ($id in 0..1) {
            $row = $identities[$id]
            $processor = [Convert]::ToUInt64($row.Groups[1].Value,16)
            $apic = [Convert]::ToUInt64($row.Groups[2].Value,16)
            $signature = [Convert]::ToUInt64($row.Groups[3].Value,16)
            $positions["cpu$id"] = $row.Index
            if ($processor -ne $id -or $apic -ne $id -or $signature -eq 0 -or
                $signature -gt [UInt32]::MaxValue -or ($id -eq 1 -and $signature -ne $cpus[0].signature)) {
                $violations.Add("cpu-identity:$id")
            }
            $cpus += [ordered]@{processorId=$processor;apicId=$apic;signature=$signature;vendor='AuthenticAMD'}
        }
    }
    & $number 'low-page' 'uefi-smp-low-page=0x'
    & $number 'arena' 'uefi-payload-base=0x'
    foreach ($name in @('patched','rdtscp','page','vector','root','resident')) {
        & $number $name "SMP-STARTUP $name="
    }
    if ([regex]::Matches($trace,'(?m)^SMP-STARTUP ').Count -ne 6) { $violations.Add('startup-marker-count') }
    & $ordered @('uefi-driver-entry','cpu-count','uefi-smp-mp-callbacks-returned=1','low-page',
        'cpu0','cpu1','uefi-smp-pre-ebs-admitted','arena','uefi-payload-reserved')
    & $ordered @("xstate-profile=$profile",'patched','rdtscp','page','vector','root','resident',
        'PASS resident-guest-exclusion','PASS rust-dispatch')
    $page = $values['page']; $arena = $values['arena']; $root = $values['root']
    if ($page -lt 4096 -or $page -ge 0x100000 -or ($page % 4096) -ne 0 -or
        $page -ne $values['low-page'] -or $values['vector'] -ne ($page -shr 12) -or
        $values['resident'] -ne 1 -or $values['patched'] -ne 3) { $violations.Add('startup-page-vector') }
    if ($values['rdtscp'] -ne $(if($Record.rdtscpDisabled -eq $true){0}else{1})) {
        $violations.Add('rdtscp-observation')
    }
    if ($arena -lt 0x100000 -or $arena -gt 0x3ff00000 -or ($arena % 4096) -ne 0 -or
        ($arena % 0x200000) -gt 0x100000 -or $Record.actualArenaBase -ne $arena -or
        ($Record.requestedArenaBase -ne 0 -and $Record.requestedArenaBase -ne $arena)) {
        $violations.Add('arena')
    }
    if ($root -lt $arena -or $root -gt ($arena + 0x100000 - 4*4096) -or
        ($root % 4096) -ne 0 -or $root -gt [UInt32]::MaxValue) { $violations.Add('startup-root') }
    try {
        $linked = [UInt64]$Record.linkedEntryAddress
        if ($linked -lt 0x100000 -or $linked -ge 0x1ff000 -or
            [UInt64]$Record.entryAddress -ne ($arena + $linked - 0x100000)) { $violations.Add('entry-address') }
        if ([string]$Record.sipiPageRequest -ne '') {
            $request = [string]$Record.sipiPageRequest
            $requested = if ($request -match '^0[xX][0-9a-fA-F]+$') {
                [Convert]::ToUInt64($request.Substring(2),16)
            } elseif ($request -match '^[0-9]+$') { [UInt64]::Parse($request) }
            else { throw 'invalid request' }
            if ($requested -ne $page) { $violations.Add('requested-sipi-page') }
        }
    } catch { $violations.Add('numeric-metadata') }
    $owned = [ordered]@{}
    $rangeNames = @('host-rsp','hsave','vmcb','guest-xstate','gpr','controller','host-xstate','clock')
    $rangeOrder = @('PASS resident-guest-exclusion')
    foreach ($id in 0..1) {
        $owned["cpu$id"] = [ordered]@{}
        foreach ($name in $rangeNames) {
            $key = "cpu$id-$name"
            & $number $key "UEFI-SMP $key="
            $address = $values[$key]
            $owned["cpu$id"][$name] = $address
            if ($address -lt $arena -or $address -ge ($arena + 0x100000)) {
                $violations.Add("owned-arena:$key")
            }
            if ($name -in @('hsave','vmcb') -and ($address % 4096) -ne 0) {
                $violations.Add("owned-alignment:$key")
            }
            $rangeOrder += $key
        }
    }
    foreach ($left in $owned.cpu0.Values) {
        if ($owned.cpu1.Values -contains $left) { $violations.Add('cross-cpu-owned-alias') }
    }
    if ([regex]::Matches($trace,'(?m)^UEFI-SMP ').Count -ne 16) { $violations.Add('owned-marker-count') }
    $ownedPass = 'PASS uefi-smp-owned-ranges=16 arena-contained-cross-cpu-disjoint'
    $null = & $unique $ownedPass ([regex]::Escape($ownedPass))
    $null = & $unique 'metrics-start' 'CONCURRENT cpu0-entries=[0-9a-f]{16}'
    & $ordered ($rangeOrder + @($ownedPass,'metrics-start','PASS rust-dispatch'))
    $concurrent = Get-ConcurrentEvidence $trace
    if (-not $concurrent.validFixture) { $violations.Add('concurrent-fixture') }
    [ordered]@{
        validFixture=($violations.Count -eq 0); errors=@($violations.ToArray())
        coverage=$(if($violations.Count -eq 0){'two-cpu-uefi-resident-owned'}else{'not-completed'})
        cpus=$cpus; startup=$values; ownedAddresses=$owned; concurrent=$concurrent
        rdtscpEvidence='native-bsp-cpuid-and-requested-profile'; timing='raw-mttcg-tsc-not-native-latency'
    }
}
