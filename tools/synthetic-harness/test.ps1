[CmdletBinding()]
param([string]$QemuPath)

$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
if (-not $QemuPath) {
    $QemuPath = Join-Path $root 'target/synthetic-tools/qemu-10.1.0/qemu-system-x86_64.exe'
}
if (-not (Test-Path -LiteralPath $QemuPath -PathType Leaf)) {
    throw "QEMU executable missing: $QemuPath. Run tools/synthetic-harness/bootstrap-tools.ps1 first, or supply -QemuPath."
}
$qemu = (Resolve-Path -LiteralPath $QemuPath).Path
$session = Join-Path $root ('target/synthetic-harness/regressions/' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $session -Force | Out-Null
$aggregatePath = Join-Path $session 'result.json'
$records = [Collections.Generic.List[object]]::new()
$aggregate = [ordered]@{
    schemaVersion = 1
    status = 'running'
    qemuPath = $qemu
    qemuSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $qemu).Hash
    profiles = $records
}

# These seven profiles run sequentially because the builder shares intermediate
# artifacts. Each run.ps1 invocation enforces its own 30-second emulator limit.
foreach ($profile in @('RustCore', 'HostFault', 'DoubleFault', 'HostWriteProtect', 'HostGuard', 'StackOverflow', 'DfGuard')) {
    $record = [ordered]@{
        profile = $profile
        status = 'running'
        stage = 'build'
        buildOutput = Join-Path $session "$profile-build.log"
        runOutput = Join-Path $session "$profile-run.log"
        runResultPath = $null
        imageSha256 = $null
        qemuSha256 = $null
        result = $null
        error = $null
    }
    $records.Add($record)
    $parameters = @{ $profile = $true }
    $runLines = @()
    try {
        Write-Output "Building regression profile $profile"
        $global:LASTEXITCODE = 0
        & (Join-Path $PSScriptRoot 'build.ps1') @parameters *>&1 |
            Out-String -Stream | Tee-Object -FilePath $record.buildOutput | Write-Output
        if ($LASTEXITCODE -ne 0) { throw "Build exited with code $LASTEXITCODE." }

        $record.stage = 'run'
        $global:LASTEXITCODE = 0
        & (Join-Path $PSScriptRoot 'run.ps1') -QemuPath $qemu @parameters *>&1 |
            Out-String -Stream | ForEach-Object {
                $runLines += $_
                $_ | Add-Content -LiteralPath $record.runOutput
                Write-Output $_
            }
        if ($LASTEXITCODE -ne 0) { throw "Runner exited with code $LASTEXITCODE." }
        $evidence = @($runLines | Where-Object { $_ -match '^Emulator .* passed\. Evidence: (.+)$' })
        if ($evidence.Count -ne 1) { throw 'Runner did not emit exactly one successful evidence path.' }
        $null = $evidence[0] -match '^Emulator .* passed\. Evidence: (.+)$'
        $record.runResultPath = Join-Path $Matches[1].Trim() 'result.json'
        $record.result = Get-Content -Raw -LiteralPath $record.runResultPath | ConvertFrom-Json
        $record.imageSha256 = $record.result.imageSha256
        $record.qemuSha256 = $record.result.qemuSha256
        if (-not $record.imageSha256 -or $record.qemuSha256 -ne $aggregate.qemuSha256) {
            throw 'Runner evidence is missing its image hash or has a different QEMU hash.'
        }
        $record.status = 'passed'
        $record.stage = 'complete'
    } catch {
        $record.status = 'failed'
        $record.error = $_.Exception.Message
        $failureLog = if ($record.stage -eq 'build') { $record.buildOutput } else { $record.runOutput }
        $record.error | Add-Content -LiteralPath $failureLog
        # A failing runner names its exact session in the exception. Never select
        # a directory by recency: another invocation may be running concurrently.
        if ($record.stage -eq 'run' -and $record.error -match 'Evidence: ([^\r\n]+)') {
            $record.runResultPath = Join-Path $Matches[1].Trim() 'result.json'
            if (Test-Path -LiteralPath $record.runResultPath -PathType Leaf) {
                try {
                    $record.result = Get-Content -Raw -LiteralPath $record.runResultPath | ConvertFrom-Json
                    $record.imageSha256 = $record.result.imageSha256
                    $record.qemuSha256 = $record.result.qemuSha256
                } catch {
                    $record.error += " Evidence could not be read: $($_.Exception.Message)"
                }
            }
        }
        $aggregate.status = 'failed'
        throw "Regression $profile failed at $($record.stage): $($record.error) Aggregate evidence: $aggregatePath"
    } finally {
        $aggregate | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $aggregatePath
    }
}
$aggregate.status = 'passed'
$aggregate | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $aggregatePath
Write-Output "All seven synthetic regression profiles passed. Evidence: $aggregatePath"
