[CmdletBinding()]
param([string]$QemuPath, [string]$QemuImgPath)
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$session = Join-Path $root ('target/synthetic-harness/relocation-regressions/' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $session -Force | Out-Null
$aggregatePath = Join-Path $session 'result.json'
$records = [Collections.Generic.List[object]]::new()
$aggregate = [ordered]@{ schemaVersion=1; suite='RuntimeRelocation'; status='running'; profiles=$records }
# Link once. Every run embeds the exact same package, except deliberate corruptions.
& (Join-Path $PSScriptRoot 'build.ps1') -RustCore
$payloadHash = (Get-FileHash (Join-Path $root 'target/synthetic-harness/rust-core.bin')).Hash
$packageHash = (Get-FileHash (Join-Path $root 'target/synthetic-harness/rust-core.reloc')).Hash
$profiles = @(
    @{ Name='Automatic'; Base=0 },
    @{ Name='Legacy1MiB'; Base=0x100000 },
    @{ Name='Interior3MiB'; Base=0x300000 },
    @{ Name='Relocated4MiB'; Base=0x400000 },
    @{ Name='Relocated64MiB'; Base=0x4000000 },
    @{ Name='OwnershipOverlap'; Base=0x400000; Rejection='RejectOwnership' },
    @{ Name='RelocationOutsideImage'; Base=0x400000; Rejection='RejectRelocation' },
    @{ Name='ArenaCrossesWindow'; Base=0x380000; Rejection='ExpectArenaRejection' },
    @{ Name='Unowned512MiB'; Base=0x20000000; Rejection='ExpectAllocationRejection' },
    @{ Name='CorruptRelocatedHandoff'; Base=0x400000; Rejection='RejectHandoff' }
)
foreach ($profile in $profiles) {
    $parameters = @{ PciRom=$true; SkipPayloadBuild=$true; ArenaBase=[UInt64]$profile.Base }
    if ($QemuPath) { $parameters.QemuPath = $QemuPath }
    if ($QemuImgPath) { $parameters.QemuImgPath = $QemuImgPath }
    if ($profile.Rejection) { $parameters[$profile.Rejection] = $true }
    $record = [ordered]@{ profile=$profile.Name; status='running'; output=(Join-Path $session ($profile.Name + '.log')); runResultPath=$null; result=$null; error=$null }
    $records.Add($record)
    $lines = [Collections.Generic.List[string]]::new()
    try {
        Write-Output "Running relocation regression $($profile.Name)"
        & (Join-Path $PSScriptRoot 'run-uefi.ps1') @parameters *>&1 |
            Out-String -Stream | ForEach-Object {
                $lines.Add($_)
                $_ | Add-Content -LiteralPath $record.output
                Write-Output $_
            }
        $evidence = @($lines | Where-Object { $_ -match '^.+ passed\. Evidence: (.+)$' })
        if ($evidence.Count -ne 1) { throw 'Runner did not emit exactly one evidence path.' }
        $null = $evidence[0] -match '^.+ passed\. Evidence: (.+)$'
        $record.runResultPath = Join-Path $Matches[1].Trim() 'result.json'
        $result = Get-Content -Raw -LiteralPath $record.runResultPath | ConvertFrom-Json
        $record.result = $result
        if ($result.payloadSha256 -ne $payloadHash -or $result.originalPackageSha256 -ne $packageHash) {
            throw 'Payload or linked relocation package changed between load addresses.'
        }
        if ($profile.Rejection -notin @('RejectOwnership', 'RejectRelocation') -and $result.relocationPackageSha256 -ne $packageHash) {
            throw 'A non-corruption profile did not embed the original package.'
        }
        if (-not $profile.Rejection -and ($null -eq $result.actualArenaBase -or
            ($profile.Base -ne 0 -and $result.actualArenaBase -ne $profile.Base) -or
            ($profile.Base -eq 0 -and $result.actualArenaBase -lt 0x200000))) {
            throw 'Actual owned load address did not match selected allocation policy.'
        }
        $record.status = 'passed'
    } catch {
        $record.status = 'failed'
        $record.error = $_.Exception.Message
        if ($record.error -match 'Evidence: ([^\r\n]+)') {
            $record.runResultPath = Join-Path $Matches[1].Trim() 'result.json'
            if (Test-Path -LiteralPath $record.runResultPath) {
                $record.result = Get-Content -Raw -LiteralPath $record.runResultPath | ConvertFrom-Json
            }
        }
        $aggregate.status = 'failed'
        throw "Relocation regression $($profile.Name) failed: $($record.error) Aggregate evidence: $aggregatePath"
    } finally {
        $aggregate | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $aggregatePath
    }
}
$aggregate.status = 'passed'
$aggregate | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $aggregatePath
Write-Output "All ten runtime relocation regression profiles passed. Evidence: $aggregatePath"
