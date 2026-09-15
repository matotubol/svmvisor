[CmdletBinding()]
param([switch]$UefiSmp, [switch]$IoIntercept, [switch]$IoInterceptBypass, [switch]$AmdCpuModel, [switch]$RustCore, [switch]$ConcurrentSmp, [switch]$GuestCr8Unblock, [switch]$HostFault, [switch]$DoubleFault, [switch]$HostWriteProtect, [switch]$HostGuard, [switch]$StackOverflow, [switch]$DfGuard, [switch]$XstateBrokenHostRestore, [switch]$XstateBrokenRestore)
if ($UefiSmp) { $ConcurrentSmp = $true; if ($AmdCpuModel) { throw 'UefiSmp and AmdCpuModel are separate profiles' } }
if ($IoInterceptBypass) { $IoIntercept = $true }
if ($IoIntercept) { $RustCore = $true; if ($AmdCpuModel -or $ConcurrentSmp -or $GuestCr8Unblock -or $HostFault -or $DoubleFault -or $HostWriteProtect -or $HostGuard -or $StackOverflow -or $DfGuard -or $XstateBrokenHostRestore -or $XstateBrokenRestore) { throw 'IoIntercept is a separate profile' } }
if ($AmdCpuModel) { $ConcurrentSmp = $true }
if ($ConcurrentSmp -or $GuestCr8Unblock -or $HostFault -or $DoubleFault -or $HostWriteProtect -or $HostGuard -or $StackOverflow -or $DfGuard -or $XstateBrokenRestore -or $XstateBrokenHostRestore) { $RustCore = $true }
if ((@($HostFault, $DoubleFault, $HostWriteProtect, $HostGuard, $StackOverflow, $DfGuard) | Where-Object { $_ }).Count -gt 1) { throw "Select only one fault profile." }
$ErrorActionPreference = 'Stop'
if ($ConcurrentSmp -and ($GuestCr8Unblock -or $HostFault -or $DoubleFault -or $HostWriteProtect -or $HostGuard -or $StackOverflow -or $DfGuard -or $XstateBrokenRestore -or $XstateBrokenHostRestore)) { throw 'ConcurrentSmp is a separate profile.' }
if ($GuestCr8Unblock -and ($HostFault -or $DoubleFault -or $HostWriteProtect -or $HostGuard -or $StackOverflow -or $DfGuard -or $XstateBrokenRestore -or $XstateBrokenHostRestore)) { throw 'Select the guest CR8 diagnostic without another fault profile.' }
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$out = Join-Path $root 'target/synthetic-harness'
New-Item -ItemType Directory -Force -Path $out | Out-Null
$name = if ($UefiSmp) { 'uefi-smp' } elseif ($IoInterceptBypass) { 'io-intercept-bypass' } elseif ($IoIntercept) { 'io-intercept' } elseif ($AmdCpuModel) { 'amd-cpu-model' } elseif ($ConcurrentSmp) { 'concurrent-smp' } elseif ($GuestCr8Unblock) { 'guest-cr8-unblock' } elseif ($XstateBrokenHostRestore) { 'xstate-broken-host-restore' } elseif ($XstateBrokenRestore) { 'xstate-broken-restore' } elseif ($StackOverflow) { 'stack-overflow' } elseif ($DfGuard) { 'df-guard' } elseif ($HostGuard) { 'host-guard' } elseif ($DoubleFault) { 'double-fault' } elseif ($HostWriteProtect) { 'host-write-protect' } elseif ($HostFault) { 'host-fault' } elseif ($RustCore) { 'rust-core' } else { 'smoke' }
$object = Join-Path $out "$name.o"
$elf = Join-Path $out "$name.elf"
$binary = Join-Path $out "$name.bin"
$source = if ($RustCore) { 'rust-bootstrap.S' } else { 'bootstrap.S' }
$libraries = @()
if ($RustCore) {
    $rustTarget = Join-Path $out 'cargo'
    $features = if ($UefiSmp) { @('--features','uefi-smp') } elseif ($IoInterceptBypass) { @('--features','io-intercept-bypass') } elseif ($IoIntercept) { @('--features','io-intercept') } elseif ($AmdCpuModel) { @('--features','amd-cpu-model') } elseif ($ConcurrentSmp) { @('--features','concurrent-smp') } elseif ($GuestCr8Unblock) { @('--features','guest-cr8-unblock') } elseif ($XstateBrokenHostRestore) { @('--features','xstate-broken-host-restore') } elseif ($XstateBrokenRestore) { @('--features','xstate-broken-restore') } elseif ($StackOverflow) { @('--features','stack-overflow') } elseif ($DfGuard) { @('--features','df-guard') } elseif ($HostGuard) { @('--features','host-guard') } elseif ($DoubleFault) { @('--features','double-fault') } elseif ($HostWriteProtect) { @('--features','host-write-protect') } elseif ($HostFault) { @('--features','host-fault') } else { @() }
    & cargo rustc --manifest-path (Join-Path $PSScriptRoot 'rust/Cargo.toml') --target x86_64-unknown-none --target-dir $rustTarget --release @features -- -C relocation-model=static -C code-model=small
    if ($LASTEXITCODE -ne 0) { throw 'Rust harness compilation failed.' }
    $libraries = @((Join-Path $rustTarget 'x86_64-unknown-none/release/libsvmvisor_emulator_harness.a'))
    $faultObject = Join-Path $out 'host-faults.o'
    & clang --target=x86_64-unknown-none -c (Join-Path $PSScriptRoot 'host-faults.S') -o $faultObject
    if ($LASTEXITCODE -ne 0) { throw 'Host fault assembly failed.' }
    $xstateObject = Join-Path $out 'xstate-switch.o'
    & clang --target=x86_64-unknown-none -c (Join-Path $PSScriptRoot 'xstate-switch.S') -o $xstateObject
    if ($LASTEXITCODE -ne 0) { throw 'Extended-state assembly failed.' }
    $continuationObject = Join-Path $out 'continuation.o'
    & clang --target=x86_64-unknown-none -c (Join-Path $PSScriptRoot 'continuation.S') -o $continuationObject
    if ($LASTEXITCODE -ne 0) { throw 'Loader continuation assembly failed.' }
    $multicoreObject = Join-Path $out 'multicore.o'
    & clang --target=x86_64-unknown-none -c (Join-Path $PSScriptRoot 'multicore.S') -o $multicoreObject
    if ($LASTEXITCODE -ne 0) { throw 'Multicore guest assembly failed.' }
    $hostSmpObject = Join-Path $out 'host-smp.o'
    [string[]]$hostSmpDefines = if ($ConcurrentSmp) { @('-DCONCURRENT_SMP') } else { @() }
    & clang --target=x86_64-unknown-none @hostSmpDefines -c (Join-Path $PSScriptRoot 'host-smp.S') -o $hostSmpObject
    if ($LASTEXITCODE -ne 0) { throw 'Host SMP assembly failed.' }
    $concurrentObjects = @()
    if ($ConcurrentSmp) {
        $concurrentObject = Join-Path $out 'concurrent.o'
        & clang --target=x86_64-unknown-none -c (Join-Path $PSScriptRoot 'concurrent.S') -o $concurrentObject
        if ($LASTEXITCODE -ne 0) { throw 'Concurrent guest assembly failed.' }
        $concurrentObjects = @($concurrentObject)
    }
    $libraries = @($faultObject, $xstateObject, $continuationObject, $multicoreObject, $hostSmpObject) + $concurrentObjects + $libraries
}
& clang --target=x86_64-unknown-none -c (Join-Path $PSScriptRoot $source) -o $object
if ($LASTEXITCODE -ne 0) { throw 'Assembly failed.' }
$linker = if ($RustCore) { 'rust-linker.ld' } else { 'linker.ld' }
& ld.lld -m elf_x86_64 --emit-relocs -T (Join-Path $PSScriptRoot $linker) $object @libraries -o $elf
if ($LASTEXITCODE -ne 0) { throw 'Link failed.' }
& llvm-objcopy -O binary $elf $binary
if ($LASTEXITCODE -ne 0) { throw 'Flat image conversion failed.' }
if ($RustCore) {
    & python (Join-Path $PSScriptRoot 'package-relocations.py') --elf $elf --image $binary --output (Join-Path $out "$name.reloc")
    if ($LASTEXITCODE -ne 0) { throw 'Runtime relocation packaging failed.' }
}
Get-FileHash -Algorithm SHA256 -LiteralPath $binary
Write-Output "Built emulator-only image: $binary"
