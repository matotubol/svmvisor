[CmdletBinding()]
param()
$ErrorActionPreference='Stop'
foreach($profile in @('Success','RejectAdmission','InvalidEntry','BrokenRestore','GuestUd','GuestPageFault')) {
    & (Join-Path $PSScriptRoot 'run.ps1') -Profile $profile -XstateProfile Fx
    if(-not $?) { throw "Returning probe suite failed: $profile" }
}
foreach($state in @('Sse','Avx')) {
    foreach($profile in @('Success','BrokenRestore')) {
        & (Join-Path $PSScriptRoot 'run.ps1') -Profile $profile -XstateProfile $state
    }
}
& (Join-Path $PSScriptRoot 'run.ps1') -Profile GuestPageFault -XstateProfile Avx
foreach($state in @('Fx','Sse','Avx')) {
    foreach($profile in @('HostUd','HostGp')) {
        & (Join-Path $PSScriptRoot 'run.ps1') -Profile $profile -XstateProfile $state
    }
}
& (Join-Path $PSScriptRoot 'run.ps1') -Profile HostFaultMismatch -XstateProfile Fx
foreach($state in @('Fx','Sse','Avx')) {
    foreach($profile in @('ArmedHostUd','ArmedHostGp')) {
        & (Join-Path $PSScriptRoot 'run.ps1') -Profile $profile -XstateProfile $state
    }
}
& (Join-Path $PSScriptRoot 'run.ps1') -Profile ArmedHostFaultMismatch -XstateProfile Fx
foreach($state in @('Fx','Sse','Avx')) {
    foreach($profile in @('LoadedHostUd','LoadedHostGp')) {
        & (Join-Path $PSScriptRoot 'run.ps1') -Profile $profile -XstateProfile $state
    }
}
& (Join-Path $PSScriptRoot 'run.ps1') -Profile LoadedHostFaultMismatch -XstateProfile Fx
foreach($state in @('Fx','Sse','Avx')) {
    foreach($profile in @('XstateHostUd','XstateHostGp')) {
        & (Join-Path $PSScriptRoot 'run.ps1') -Profile $profile -XstateProfile $state
    }
}
& (Join-Path $PSScriptRoot 'run.ps1') -Profile XstateHostFaultMismatch -XstateProfile Fx
foreach($state in @('Fx','Sse','Avx')) {
    foreach($profile in @('PostExitHostUd','PostExitHostGp')) {
        & (Join-Path $PSScriptRoot 'run.ps1') -Profile $profile -XstateProfile $state
    }
}
& (Join-Path $PSScriptRoot 'run.ps1') -Profile PostExitHostFaultMismatch -XstateProfile Fx
Write-Output 'PASS returning probe emulator suite (46 profiles)'
