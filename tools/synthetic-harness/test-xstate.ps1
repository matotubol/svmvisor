[CmdletBinding()]
param([string]$QemuPath, [string]$QemuImgPath)
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
if (-not $QemuPath) { $QemuPath = Join-Path $root 'target/synthetic-tools/qemu-10.1.0/qemu-system-x86_64.exe' }
$session = Join-Path $root ('target/synthetic-harness/xstate-regressions/' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $session -Force | Out-Null
$records = [Collections.Generic.List[object]]::new()
$aggregate = [ordered]@{ schemaVersion=1; suite='ExtendedState'; status='running'; profiles=$records }
& (Join-Path $PSScriptRoot 'build.ps1') -RustCore
foreach ($profile in @('Avx','AvxOnly','Sse','Fx','PciAvx','PciFx','BrokenAvx','BrokenFx','HostBrokenAvx','HostBrokenFx')) {
    $record = [ordered]@{ profile=$profile; status='running'; runResultPath=$null; result=$null; error=$null }
    $records.Add($record)
    try {
        $parameters = @{ QemuPath=$QemuPath; XstateProfile=$(if ($profile -like '*Fx') { 'Fx' } elseif ($profile -eq 'AvxOnly') { 'AvxOnly' } elseif ($profile -eq 'Sse') { 'Sse' } else { 'Avx' }) }
        if ($profile -like '*Broken*') {
            if ($profile -like 'Host*') {
                & (Join-Path $PSScriptRoot 'build.ps1') -XstateBrokenHostRestore
                $parameters.XstateBrokenHostRestore=$true
            } else {
                & (Join-Path $PSScriptRoot 'build.ps1') -XstateBrokenRestore
                $parameters.XstateBrokenRestore=$true
            }
        }
        if ($profile -like 'Pci*') {
            $parameters.PciRom=$true; $parameters.SkipPayloadBuild=$true; $parameters.ArenaBase=[UInt64]0x4000000
            if ($QemuImgPath) { $parameters.QemuImgPath=$QemuImgPath }
            $runner = 'run-uefi.ps1'
        } else { $parameters.RustCore=$true; $runner = 'run.ps1' }
        $lines = @(& (Join-Path $PSScriptRoot $runner) @parameters *>&1 | Out-String -Stream)
        $lines | Set-Content (Join-Path $session "$profile.log")
        $lines | Write-Output
        $evidence = @($lines | Where-Object { $_ -match '^.+ passed\. Evidence: (.+)$' })
        if ($evidence.Count -ne 1) { throw 'Missing exactly one successful run evidence path.' }
        $null = $evidence[0] -match '^.+ passed\. Evidence: (.+)$'
        $record.runResultPath = Join-Path $Matches[1].Trim() 'result.json'
        $record.result = Get-Content -Raw $record.runResultPath | ConvertFrom-Json
        $record.status='passed'
    } catch { $record.status='failed'; $record.error=$_.Exception.Message; $aggregate.status='failed'; throw }
    finally { $aggregate | ConvertTo-Json -Depth 12 | Set-Content (Join-Path $session 'result.json') }
}
$aggregate.status='passed'
$aggregate | ConvertTo-Json -Depth 12 | Set-Content (Join-Path $session 'result.json')
Write-Output "Extended-state regressions passed. Evidence: $session"
