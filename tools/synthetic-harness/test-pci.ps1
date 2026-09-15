[CmdletBinding()]
param([string]$QemuPath)

$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$session = Join-Path $root ('target/synthetic-harness/pci-regressions/' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $session -Force | Out-Null
$aggregatePath = Join-Path $session 'result.json'
$records = [Collections.Generic.List[object]]::new()
$aggregate = [ordered]@{ schemaVersion = 1; suite = 'FirmwarePCI'; status = 'running'; profiles = $records }

# Each invocation builds its own artifacts and enforces the runner's bounded
# emulator timeout. Sequential execution preserves the shared build outputs.
foreach ($profile in @('PciRom', 'Reconnect', 'NoAuthorization', 'RejectHandoff', 'OmitRom', 'RejectMmio')) {
    $parameters = @{ PciRom = $true }
    if ($profile -ne 'PciRom') { $parameters[$profile] = $true }
    if ($QemuPath) { $parameters.QemuPath = $QemuPath }
    $record = [ordered]@{
        profile = $profile
        status = 'running'
        output = Join-Path $session "$profile.log"
        runResultPath = $null
        hashes = [ordered]@{}
        result = $null
        error = $null
    }
    $records.Add($record)
    $lines = [Collections.Generic.List[string]]::new()
    try {
        Write-Output "Running FirmwarePCI regression profile $profile"
        $global:LASTEXITCODE = 0
        & (Join-Path $PSScriptRoot 'run-uefi.ps1') @parameters *>&1 |
            Out-String -Stream | ForEach-Object {
                $lines.Add($_)
                $_ | Add-Content -LiteralPath $record.output
                Write-Output $_
            }
        if ($LASTEXITCODE -ne 0) { throw "UEFI runner exited with code $LASTEXITCODE." }
        $evidence = @($lines | Where-Object { $_ -match '^.+ passed\. Evidence: (.+)$' })
        if ($evidence.Count -ne 1) { throw 'Runner did not emit exactly one successful evidence path.' }
        $null = $evidence[0] -match '^.+ passed\. Evidence: (.+)$'
        $record.runResultPath = Join-Path $Matches[1].Trim() 'result.json'
        $record.result = Get-Content -Raw -LiteralPath $record.runResultPath | ConvertFrom-Json
        foreach ($property in $record.result.PSObject.Properties) {
            if ($property.Name -like '*Sha256') { $record.hashes[$property.Name] = $property.Value }
        }
        foreach ($required in @('romSha256', 'payloadSha256', 'loaderSha256', 'driverSha256', 'qemuSha256', 'firmwareSha256', 'varsTemplateSha256')) {
            if ($record.hashes[$required] -notmatch '^[0-9a-fA-F]{64}$') {
                throw "Runner evidence has no valid $required."
            }
        }
        $record.status = 'passed'
    } catch {
        $record.status = 'failed'
        $record.error = $_.Exception.Message
        $record.error | Add-Content -LiteralPath $record.output
        # Recover only the exact session named by the failing runner. Never use
        # directory recency, which could select evidence from a different run.
        if ($record.error -match 'Evidence: ([^\r\n]+)') {
            $record.runResultPath = Join-Path $Matches[1].Trim() 'result.json'
            if (Test-Path -LiteralPath $record.runResultPath -PathType Leaf) {
                try {
                    $record.result = Get-Content -Raw -LiteralPath $record.runResultPath | ConvertFrom-Json
                    foreach ($property in $record.result.PSObject.Properties) {
                        if ($property.Name -like '*Sha256') { $record.hashes[$property.Name] = $property.Value }
                    }
                } catch {
                    $record.error += " Evidence could not be read: $($_.Exception.Message)"
                }
            }
        }
        $aggregate.status = 'failed'
        throw "FirmwarePCI regression $profile failed: $($record.error) Aggregate evidence: $aggregatePath"
    } finally {
        $aggregate | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $aggregatePath
    }
}
$aggregate.status = 'passed'
$aggregate | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $aggregatePath
Write-Output "All six FirmwarePCI regression profiles passed. Evidence: $aggregatePath"
