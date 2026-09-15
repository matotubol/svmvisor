[CmdletBinding()]
param([switch]$UefiSmp, [ValidateRange(1,3)][int]$HostCpuCount=2, [string]$SipiPage='', [switch]$DisableRdtscp,
    [ValidateSet('None','Topology','LowPageRequest','LowPageAllocation','MissingRecord')][string]$ExpectedSmpRefusal='None',
    [string]$QemuPath, [string]$QemuImgPath, [switch]$RejectHandoff, [switch]$ProductionDxe, [switch]$NoAuthorization, [switch]$PciRom, [switch]$OmitRom, [switch]$Reconnect, [switch]$RejectMmio,
    [UInt64]$ArenaBase = 0, [switch]$RejectOwnership, [switch]$RejectRelocation, [switch]$RejectResidentOwnership, [switch]$ExpectArenaRejection, [switch]$ExpectAllocationRejection, [switch]$SkipPayloadBuild, [ValidateSet('Avx','AvxOnly','Sse','Fx')][string]$XstateProfile = 'Avx')
if ((@($RejectHandoff, $NoAuthorization, $OmitRom, $RejectMmio, $RejectOwnership, $RejectRelocation, $RejectResidentOwnership, $ExpectArenaRejection, $ExpectAllocationRejection) | Where-Object { $_ }).Count -gt 1) { throw 'Select one rejection test.' }
if ($Reconnect -or $RejectMmio) { $PciRom = $true }
if ($RejectMmio -and ($RejectHandoff -or $NoAuthorization -or $OmitRom -or $Reconnect)) { throw "Select one rejection test." }
if ($PciRom -or $OmitRom) { $ProductionDxe = $true; $PciRom = $true }
if ($OmitRom -and ($RejectHandoff -or $NoAuthorization)) { throw 'Select one rejection test.' }
if ($RejectHandoff -and $NoAuthorization) { throw 'Select one rejection test.' }
$ErrorActionPreference = 'Stop'
if($UefiSmp -and ($ProductionDxe -or $PciRom -or $NoAuthorization -or $OmitRom -or $Reconnect -or $RejectMmio -or $RejectOwnership -or $RejectRelocation)){throw 'UEFI SMP uses the direct benign driver handoff profile'}
if(-not $UefiSmp -and ($SipiPage -ne '' -or $DisableRdtscp -or $ExpectedSmpRefusal -ne 'None')){throw 'SMP options require UefiSmp'}
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
if (-not $QemuPath) { $QemuPath = Join-Path $root 'target/synthetic-tools/qemu-10.1.0/qemu-system-x86_64.exe' }
$qemu = (Resolve-Path -LiteralPath $QemuPath).Path
if (-not $QemuImgPath) { $QemuImgPath = Join-Path (Split-Path $qemu) 'qemu-img.exe' }
$qemuImg = (Resolve-Path -LiteralPath $QemuImgPath).Path
$qemuImgVersionLines = @(& $qemuImg --version)
if ($LASTEXITCODE -ne 0) { throw 'Image preparation tool version check failed.' }
$qemuImgVersion = $qemuImgVersionLines[0]
$share = Join-Path (Split-Path $qemu) 'share'
$firmware = (Resolve-Path -LiteralPath (Join-Path $share 'edk2-x86_64-code.fd')).Path
$varsTemplate = (Resolve-Path -LiteralPath (Join-Path $share 'edk2-i386-vars.fd')).Path
if($UefiSmp){
    foreach($pin in @(@($qemu,'c867dd99822400be12f1aa3fbf9995ea5f103486fd3affede6076cd444cb3047'),
        @($firmware,'33090cc07675baa5190d9f1e84bf5176b33bcbfa9bacac522961150cdb6dbb2a'),
        @($varsTemplate,'5d2ac383371b408398accee7ec27c8c09ea5b74a0de0ceea6513388b15be5d1e'))){
        if((Get-FileHash $pin[0]).Hash -ne $pin[1]){throw 'UEFI SMP backend/firmware pin mismatch'}
    }
}
if (-not $SkipPayloadBuild) {
    if($UefiSmp){& (Join-Path $PSScriptRoot 'build.ps1') -UefiSmp}
    else{& (Join-Path $PSScriptRoot 'build.ps1') -RustCore}
}
$out = Join-Path $root 'target/synthetic-harness'
$imageName=if($UefiSmp){'uefi-smp'}else{'rust-core'}
$rawPayload = Join-Path $out "$imageName.bin"
$payload = Join-Path $out "$imageName.reloc"
$originalPayloadHash = (Get-FileHash $payload).Hash
$session = Join-Path $out ('uefi-runs/' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $session -Force | Out-Null
if ($RejectOwnership -or $RejectRelocation) {
    $bytes = [IO.File]::ReadAllBytes($payload)
    if ($RejectOwnership) {
        # Declared BSS crosses into the dedicated handoff page.
        [BitConverter]::GetBytes([UInt64]0x100000).CopyTo($bytes, 32)
    } else {
        # First relocation claims an offset beyond initialized owned bytes.
        $imageBytes = [BitConverter]::ToUInt64($bytes, 24)
        [BitConverter]::GetBytes($imageBytes).CopyTo($bytes, [int](64 + $imageBytes))
    }
    $payload = Join-Path $session 'rejected.reloc'
    [IO.File]::WriteAllBytes($payload, $bytes)
}
$symbols = & llvm-nm -n (Join-Path $out "$imageName.elf")
if ($LASTEXITCODE -ne 0) { throw 'Cannot inspect payload symbols.' }
$entry = @($symbols | Where-Object { $_ -match '^([0-9a-fA-F]+)\s+\w\s+entry_uefi$' })
if ($entry.Count -ne 1) { throw 'Missing unique UEFI payload entry.' }
$null = $entry[0] -match '^([0-9a-fA-F]+)'
$address = [Convert]::ToUInt64($Matches[1], 16)
$savedPayload = $env:SVMVISOR_PAYLOAD
$savedEntry = $env:SVMVISOR_ENTRY
$savedArenaBase = $env:SVMVISOR_ARENA_BASE
$savedRejectResidentOwnership = $env:SVMVISOR_REJECT_RESIDENT_OWNERSHIP
$savedSmp=$env:SVMVISOR_UEFI_SMP
$savedSipi=$env:SVMVISOR_SIPI_PAGE
$savedMissingSmp=$env:SVMVISOR_REJECT_SMP_RECORD
try {
    $env:SVMVISOR_PAYLOAD = $payload
    $env:SVMVISOR_ENTRY = [string]($address - 0x100000)
    $env:SVMVISOR_ARENA_BASE = if ($ArenaBase) { [string]$ArenaBase } else { $null }
    $env:SVMVISOR_REJECT_RESIDENT_OWNERSHIP = if ($RejectResidentOwnership) { '1' } else { $null }
    $env:SVMVISOR_UEFI_SMP=if($UefiSmp){'1'}else{$null}
    $env:SVMVISOR_SIPI_PAGE=if($SipiPage -ne ''){$SipiPage}else{$null}
    $env:SVMVISOR_REJECT_SMP_RECORD=if($ExpectedSmpRefusal -eq 'MissingRecord'){'1'}else{$null}
    if ($ProductionDxe) {
        $features = if ($RejectMmio) { 'emulator-reject-mmio' } elseif ($PciRom) { if ($RejectHandoff) { 'emulator-pci-handoff,emulator-reject-handoff' } else { 'emulator-pci-handoff' } } elseif ($RejectHandoff) { 'emulator-reject-handoff' } else { 'emulator-handoff' }
        & cargo build --manifest-path (Join-Path $root 'Cargo.toml') -p svmvisor-dxe --features $features --target x86_64-unknown-uefi --target-dir (Join-Path $out 'production-dxe-cargo') --release
    } else {
        $features = if ($RejectHandoff) { 'driver,bad-handoff' } else { 'driver' }
        & cargo build --manifest-path (Join-Path $PSScriptRoot 'uefi-loader/Cargo.toml') --features $features --target x86_64-unknown-uefi --target-dir (Join-Path $out 'uefi-cargo') --release
    }
    if ($LASTEXITCODE -ne 0) { throw 'UEFI loader build failed.' }
} finally {
    $env:SVMVISOR_PAYLOAD = $savedPayload
    $env:SVMVISOR_ENTRY = $savedEntry
    $env:SVMVISOR_ARENA_BASE = $savedArenaBase
    $env:SVMVISOR_REJECT_RESIDENT_OWNERSHIP = $savedRejectResidentOwnership
    $env:SVMVISOR_UEFI_SMP=$savedSmp
    $env:SVMVISOR_SIPI_PAGE=$savedSipi
    $env:SVMVISOR_REJECT_SMP_RECORD=$savedMissingSmp
}
$driver = if ($ProductionDxe) { Join-Path $out 'production-dxe-cargo/x86_64-unknown-uefi/release/svmvisor-dxe.efi' } else { Join-Path $out 'uefi-cargo/x86_64-unknown-uefi/release/BOOTX64.efi' }
$savedDriver = $env:SVMVISOR_DRIVER
try {
    $env:SVMVISOR_DRIVER = $driver
    $launcherFeatureNames = @()
    if ($NoAuthorization) { $launcherFeatureNames += 'no-authorization' }
    if ($Reconnect) { $launcherFeatureNames += 'reconnect' }
    $launcherFeatures = if ($launcherFeatureNames.Count) { @('--features',($launcherFeatureNames -join ',')) } else { @() }
    $launcherManifest = if ($PciRom) { 'pci-launcher/Cargo.toml' } else { 'uefi-loader/launcher/Cargo.toml' }
    & cargo build --manifest-path (Join-Path $PSScriptRoot $launcherManifest) @launcherFeatures --target x86_64-unknown-uefi --target-dir (Join-Path $out 'launcher-cargo') --release
    if ($LASTEXITCODE -ne 0) { throw 'UEFI launcher build failed.' }
} finally { $env:SVMVISOR_DRIVER = $savedDriver }
function Assert-Subsystem([string]$Path, [int]$Expected) {
    $bytes = [IO.File]::ReadAllBytes($Path)
    $pe = [BitConverter]::ToInt32($bytes, 0x3c)
    if ([BitConverter]::ToUInt16($bytes, $pe + 92) -ne $Expected) { throw "Unexpected PE subsystem: $Path" }
}
Assert-Subsystem $driver 11
$launcher = Join-Path $out 'launcher-cargo/x86_64-unknown-uefi/release/BOOTX64.efi'
Assert-Subsystem $launcher 10
$esp = Join-Path $session 'esp'
$boot = Join-Path $esp 'EFI/BOOT'
New-Item -ItemType Directory -Path $boot -Force | Out-Null
$loader = Join-Path $boot 'BOOTX64.EFI'
Copy-Item -LiteralPath $launcher -Destination $loader
Copy-Item -LiteralPath $driver -Destination (Join-Path $session 'driver.efi')
$rom = Join-Path $session 'option.rom'
if ($PciRom) {
    & cargo run --manifest-path (Join-Path $root 'Cargo.toml') -p svmvisor-rompack --release -- --input $driver --output $rom --vendor 0x1b36 --device 0x0005 --class 0x00ff00
    if ($LASTEXITCODE -ne 0) { throw 'Option ROM packaging failed.' }
}
$espImage = Join-Path $session 'esp.img'
& $qemuImg convert -f vvfat -O raw ("fat:" + $esp) $espImage
if ($LASTEXITCODE -ne 0) { throw 'Generated ESP image conversion failed.' }
$vars = Join-Path $session 'vars.fd'
Copy-Item -LiteralPath $varsTemplate -Destination $vars
$debug = Join-Path $session 'debug.log'
$stderr = Join-Path $session 'stderr.log'
$stdout = Join-Path $session 'stdout.log'
# Only a generated ESP disk is attached, with disposable writes; no user disks/network.
$cpu = 'max,svm=on,hypervisor=off' + $(if ($XstateProfile -eq 'Fx') { ',xsave=off,avx=off,avx2=off' } elseif ($XstateProfile -eq 'AvxOnly') { ',avx2=off' } elseif ($XstateProfile -eq 'Sse') { ',avx=off,avx2=off' } else { '' })
if($DisableRdtscp){$cpu+=',rdtscp=off'}
$machine=if($UefiSmp){'pc-q35-10.1'}else{'q35,accel=tcg'}
$topology=if($UefiSmp){"$HostCpuCount,sockets=1,cores=$HostCpuCount,threads=1"}else{'1'}
$arguments = @('-machine',$machine,'-cpu',$cpu,'-m','256M','-smp',$topology,
    '-drive',('"if=pflash,format=raw,readonly=on,file=' + $firmware + '"'),
    '-drive',('"if=pflash,format=raw,file=' + $vars + '"'),
    '-drive',('"format=raw,snapshot=on,file=' + $espImage + '"'),
    '-no-reboot','-display','none','-monitor','none','-serial','none','-nic','none',
    '-debugcon',('"file:' + $debug + '"'),'-device','isa-debug-exit,iobase=0xf4,iosize=0x04')
if($UefiSmp){$arguments+=@('-accel','tcg,thread=multi')}
if ($PciRom) {
    $device = if ($OmitRom) { 'pci-testdev' } else { '"pci-testdev,romfile=' + $rom + '"' }
    $arguments += @('-device', $device)
}
$process = Start-Process -FilePath $qemu -ArgumentList $arguments -PassThru -WindowStyle Hidden -RedirectStandardOutput $stdout -RedirectStandardError $stderr
$timedOut = -not $process.WaitForExit(30000)
if ($timedOut) { $process.Kill(); $process.WaitForExit() }
$process.Refresh()
$trace = if (Test-Path -LiteralPath $debug) { Get-Content -Raw -LiteralPath $debug } else { '' }
$errors = Get-Content -Raw -LiteralPath $stderr
$baseMarkers = [regex]::Matches($trace, '(?m)^uefi-payload-base=0x([0-9a-f]{16})\r?$')
$actualBase = if ($baseMarkers.Count -eq 1) { [Convert]::ToUInt64($baseMarkers[0].Groups[1].Value, 16) } else { $null }
$record = [ordered]@{ cpu=$cpu; rejectMmio=[bool]$RejectMmio; reconnect=[bool]$Reconnect; pciRom=[bool]$PciRom; omittedRom=[bool]$OmitRom; romSha256=$(if ($PciRom) { (Get-FileHash $rom).Hash } else { $null }); productionDxe=[bool]$ProductionDxe; noAuthorization=[bool]$NoAuthorization; rejectHandoff=[bool]$RejectHandoff; exitCode=$process.ExitCode; timedOut=$timedOut; trace=$trace; stderr=$errors;
    payloadSha256=(Get-FileHash $rawPayload).Hash; relocationPackageSha256=(Get-FileHash $payload).Hash; originalPackageSha256=$originalPayloadHash; loaderSha256=(Get-FileHash $loader).Hash; driverSha256=(Get-FileHash $driver).Hash;
    qemuSha256=(Get-FileHash $qemu).Hash; qemuImgPath=$qemuImg; qemuImgSha256=(Get-FileHash $qemuImg).Hash; qemuImgVersion=$qemuImgVersion; firmwareSha256=(Get-FileHash $firmware).Hash;
    varsTemplateSha256=(Get-FileHash $varsTemplate).Hash; linkedEntryAddress=$address;
    requestedArenaBase=$ArenaBase; actualArenaBase=$actualBase; entryAddress=$(if ($null -ne $actualBase) { $actualBase + $address - 0x100000 } else { $null });
    rejectOwnership=[bool]$RejectOwnership; rejectRelocation=[bool]$RejectRelocation; rejectResidentOwnership=[bool]$RejectResidentOwnership; expectArenaRejection=[bool]$ExpectArenaRejection; expectAllocationRejection=[bool]$ExpectAllocationRejection }
$record['uefiSmp']=[bool]$UefiSmp;$record['hostCpuCount']=if($UefiSmp){$HostCpuCount}else{1}
$record['sipiPageRequest']=$SipiPage;$record['expectedSmpRefusal']=$ExpectedSmpRefusal
$record['rdtscpDisabled']=[bool]$DisableRdtscp;$record['xstateProfile']=$XstateProfile
$record['arguments']=$arguments;$record['machine']=$machine
. (Join-Path $PSScriptRoot 'timing-evidence.ps1')
$timing = Get-TimingEvidence $trace
$record['timing'] = $timing
. (Join-Path $PSScriptRoot 'interrupt-evidence.ps1')
$interrupts = Get-InterruptEvidence $trace
$record['interrupts'] = $interrupts
if($UefiSmp){. (Join-Path $PSScriptRoot 'concurrent-evidence.ps1');$record['concurrent']=Get-ConcurrentEvidence $trace}
if($UefiSmp){. (Join-Path $PSScriptRoot 'uefi-smp-evidence.ps1');$record['uefiOwnership']=Get-UefiSmpEvidence -Record $record}
Write-Output $trace
if ($errors) { Write-Output $errors }
$driverMarker = if ($PciRom) { 'pci-rom-entry' } elseif ($ProductionDxe) { 'production-dxe-emulator-entry' } else { 'uefi-driver-entry' }
$valid = -not $timedOut -and $trace -match $driverMarker
if($UefiSmp){$valid=$valid -and -not $errors}
if ($PciRom -and -not $OmitRom -and -not $RejectMmio) {
    $valid = $valid -and $trace -match 'PASS pci-mmio-count=16 reset=0' -and $trace -match '(?s)pci-rom-entry\r?\n.*pci-binding-supported\r?\n.*pci-binding-start\r?\n.*pci-launcher-entry'
}
if ($Reconnect) { $valid = $valid -and $trace -match 'pci-binding-stop' -and $trace -match 'pci-binding-disconnected' -and $trace -match 'pci-binding-reconnected' }
if ($RejectOwnership -or $RejectRelocation -or $ExpectArenaRejection -or $ExpectAllocationRejection) {
    $rejection = if ($ExpectAllocationRejection) { 'FAIL uefi-payload-allocation' } elseif ($ExpectArenaRejection) { 'FAIL uefi-arena-layout' } else { 'FAIL uefi-payload-layout' }
    $valid = $valid -and $process.ExitCode -eq 35 -and $trace -match $rejection -and $trace -notmatch 'uefi-payload-reserved|uefi-boot-services-exited|uefi-private-entry|PASS repeated-sessions|PASS rust-dispatch'
} elseif ($RejectMmio) {
    $valid = -not $timedOut -and $process.ExitCode -eq 35 -and $trace -match 'pci-rom-entry' -and $trace -match 'pci-binding-supported' -and $trace -match 'pci-mmio-rejected-for-test' -and $trace -match 'pci-start-failure-cleaned' -and $trace -match 'FAIL pci-binding-not-found' -and $trace -notmatch 'pci-binding-start|uefi-payload-reserved|uefi-boot-services-exited'
} elseif ($OmitRom) {
    $valid = -not $timedOut -and $process.ExitCode -eq 35 -and $trace -match 'FAIL pci-binding-not-found' -and $trace -notmatch 'pci-rom-entry|pci-binding-start|uefi-payload-reserved'
} elseif ($PciRom -and $NoAuthorization) {
    $valid = $valid -and $process.ExitCode -eq 33 -and $trace -match 'handoff-authorization-rejected' -and $trace -match 'PASS pci-authorization-rejected' -and $trace -notmatch 'FAIL|uefi-payload-reserved|uefi-boot-services-exited'
} elseif ($NoAuthorization) {
    $valid = $valid -and $process.ExitCode -eq 33 -and $trace -match '(?m)^handoff-not-authorized\r?$' -and $trace -match 'PASS handoff-not-authorized' -and $trace -notmatch 'FAIL|uefi-payload-reserved|uefi-boot-services-exited|uefi-private-entry'
} elseif ($UefiSmp -and $ExpectedSmpRefusal -ne 'None') {
    $reason=switch($ExpectedSmpRefusal){'Topology'{'REFUSE uefi-smp-topology'} 'LowPageRequest'{'FAIL uefi-smp-low-page-request'} 'LowPageAllocation'{'REFUSE uefi-smp-low-page-allocation'} 'MissingRecord'{'FAIL resident-ownership'}}
    $valid=$valid -and $process.ExitCode -eq 35 -and ([regex]::Matches($trace,'(?m)^'+[regex]::Escape($reason)+'\r?$').Count -eq 1) -and $trace -notmatch 'SMP-STARTUP |CONCURRENT |PASS concurrent-|PASS rust-dispatch'
    if($ExpectedSmpRefusal -eq 'MissingRecord'){$valid=$valid -and $trace -match 'uefi-smp-record-omitted-for-test' -and $trace -match 'uefi-boot-services-exited'}
    else{$valid=$valid -and $trace -notmatch 'uefi-boot-services-exited|uefi-private-entry'}
} elseif ($RejectResidentOwnership) {
    $valid = $valid -and $process.ExitCode -eq 35 -and $trace -match '(?m)^uefi-boot-services-exited\r?$' -and $trace -match '(?m)^uefi-resident-ownership-corrupted-for-test\r?$' -and $trace -match '(?m)^uefi-private-entry\r?$' -and $trace -match '(?m)^FAIL resident-ownership\r?$' -and $trace -notmatch 'PASS resident-ownership|PASS resident-guest-exclusion|host-mappings-restricted|PASS repeated-sessions|PASS captured-loader|PASS guest-map|PASS rust-dispatch'
} elseif ($RejectHandoff) {
    $valid = $valid -and $trace -match 'uefi-boot-services-exited' -and $trace -match 'uefi-private-entry'
    $valid = $valid -and $process.ExitCode -eq 35 -and $trace -match 'uefi-handoff-corrupted-for-test' -and $trace -match 'FAIL rust-bootstrap' -and $trace -notmatch 'rust-core|PASS rust-dispatch|PASS repeated-sessions'
} elseif ($UefiSmp) {
    $valid=$valid -and -not $errors -and $process.ExitCode -eq 33 -and $record.uefiOwnership.validFixture
    foreach($marker in @('uefi-smp-mp-callbacks-returned=1','uefi-smp-pre-ebs-admitted','uefi-boot-services-exited',
        'uefi-smp-post-ebs-owned','uefi-private-entry','PASS resident-ownership-retained','PASS resident-ownership-private',
        'PASS resident-smp-low-reservation','PASS resident-guest-exclusion','SMP-STARTUP resident=0000000000000001')){
        $valid=$valid -and ([regex]::Matches($trace,'(?m)^'+[regex]::Escape($marker)+'\r?$').Count -eq 1)
    }
} else {
    $valid = $valid -and $interrupts.validFixture -and $timing.validFixture -and $trace -match 'uefi-boot-services-exited' -and $trace -match 'uefi-private-entry' -and $process.ExitCode -eq 33 -and $trace -notmatch 'FAIL' -and $trace -match 'PASS repeated-sessions=32' -and $trace -match 'PASS timing-contract tcg-only' -and $trace -match 'PASS timing-rdtsc=16 monotonic' -and $trace -match 'PASS rust-dispatch' -and $trace -match 'PASS xstate-isolation=32 xsetbv-blocked x87-arithmetic' -and $trace -match '(?m)^PASS guest-exceptions=16 ud-gp-pf iretq-continuation\r?$' -and $trace -match '(?m)^PASS guest-nested-delivery-refused\r?$'
    $profileMarker = switch ($XstateProfile) { 'Avx' { 'xsave-avx' } 'AvxOnly' { 'xsave-avx' } 'Sse' { 'xsave-sse' } 'Fx' { 'fxsave' } }
    $valid = $valid -and $trace -match '(?m)^PASS resident-ownership-retained\r?$' -and $trace -match '(?m)^PASS resident-ownership-private\r?$' -and $trace -match '(?m)^PASS resident-guest-exclusion\r?$' -and $trace -match '(?m)^PASS captured-loader-continuation=16 same-rip-rsp-flags\r?$' -and $trace -match '(?m)^PASS guest-map-allocation reserved-refusal\r?$' -and $trace -match '(?m)^PASS captured-state-negative-controls\r?$'
    $valid = $valid -and $trace -match "(?m)^xstate-profile=$profileMarker\r?$"
}
if ($trace -match 'uefi-payload-reserved') {
    $valid = $valid -and $null -ne $actualBase
    if ($ArenaBase) { $valid = $valid -and $actualBase -eq $ArenaBase }
}
$record['status']=if($valid){'passed'}else{'failed'}
$record | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $session 'result.json')
if (-not $valid) {
    throw "UEFI handoff failed. Evidence: $session"
}
$testName = if ($RejectResidentOwnership) { 'UEFI resident-ownership rejection' } elseif ($RejectMmio) { 'PCI MMIO failure cleanup' } elseif ($OmitRom) { 'PCI ROM absence' } elseif ($PciRom) { 'PCI ROM binding handoff' } elseif ($NoAuthorization) { 'UEFI missing-token rejection' } elseif ($RejectHandoff) { 'UEFI malformed-handoff rejection' } else { 'UEFI driver handoff' }
Write-Output "$testName passed. Evidence: $session"




