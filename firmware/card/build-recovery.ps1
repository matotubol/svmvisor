[CmdletBinding()]
param([string] $VivadoRoot)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$workspaceRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..')).Path
$config = Import-PowerShellDataFile (Join-Path $PSScriptRoot 'config.psd1')
if (-not $VivadoRoot) { $VivadoRoot = $config.VivadoRoot }
$vivado = Join-Path $VivadoRoot 'Vivado\bin\vivado.bat'
if (-not (Test-Path -LiteralPath $vivado)) { throw "Vivado missing: $vivado" }
# Unique output prevents a failed build from reusing an earlier PASS or image.
$output = Join-Path $workspaceRoot ('target\firmware\card\recovery\' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $output | Out-Null
$sources = @('rtl/svmvisor_recovery.sv', 'rtl/svmvisor_recovery.xdc', 'vivado/recovery.tcl', 'build-recovery.ps1', 'config.psd1')
$hashes = [ordered] @{}
foreach ($source in $sources) { $hashes[$source] = (Get-FileHash -LiteralPath (Join-Path $PSScriptRoot $source) -Algorithm SHA256).Hash.ToLowerInvariant() }
& $vivado -mode batch -nojournal -log (Join-Path $output 'vivado.log') -source (Join-Path $PSScriptRoot 'vivado/recovery.tcl') -tclargs $PSScriptRoot $output
if ($LASTEXITCODE -ne 0) { throw "Recovery build failed ($LASTEXITCODE): $output" }
foreach ($source in $sources) {
    if ((Get-FileHash -LiteralPath (Join-Path $PSScriptRoot $source) -Algorithm SHA256).Hash.ToLowerInvariant() -cne $hashes[$source]) { throw "Source changed during build: $source" }
}
$policy = Get-Content -LiteralPath (Join-Path $output 'recovery-policy.txt') -Raw
if ($policy -notmatch '(?m)^non_enumerating_netlist_policy=PASS\r?$') { throw 'Recovery netlist policy did not pass.' }
$image = Get-Item -LiteralPath (Join-Path $output 'svmvisor-recovery.bin')
if ($image.Length -le 0) { throw 'Empty recovery image.' }
$manifest = [ordered] @{
    schema_version = 1
    image_kind = 'non_enumerating_recovery'
    target_part = 'xc7a35tfgg484-2'
    image_sha256 = (Get-FileHash -LiteralPath $image.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    image_size_bytes = $image.Length
    non_enumerating_netlist_policy = 'PASS'
    physical_fixture_test = 'NOT_RUN'
    source_sha256 = $hashes
    policy_report_sha256 = (Get-FileHash -LiteralPath (Join-Path $output 'recovery-policy.txt') -Algorithm SHA256).Hash.ToLowerInvariant()
    vivado_log_sha256 = (Get-FileHash -LiteralPath (Join-Path $output 'vivado.log') -Algorithm SHA256).Hash.ToLowerInvariant()
}
$manifest | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $output 'manifest.json') -Encoding UTF8
Write-Host "Recovery candidate built: $output"
Write-Host 'Physical programming/readback has not been performed.'
