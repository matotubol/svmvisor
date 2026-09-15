[CmdletBinding()]
param(
    [ValidateSet('bypass','tx_guard','completer','journal','snapshot','percpu_snapshot','payload_spi','payload_completer')]
    [string[]] $Suite = @('bypass','tx_guard','completer','journal','snapshot','percpu_snapshot','payload_spi','payload_completer')
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$workspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$config = Import-PowerShellDataFile (Join-Path $PSScriptRoot 'config.psd1')
$bin = Join-Path $config.VivadoRoot 'Vivado/bin'
$sim = Join-Path $workspaceRoot ('target/firmware/squirrel/sim/endpoint-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $sim | Out-Null
$sources = @('svmvisor_pcie_pkg.sv','svmvisor_bypass.sv','svmvisor_tx_guard.sv','svmvisor_journal.sv','svmvisor_snapshot.sv','svmvisor_percpu_snapshot.sv','svmvisor_payload_spi.sv','svmvisor_completer.sv',
    'tb/svmvisor_bypass_tb.sv','tb/svmvisor_tx_guard_tb.sv','tb/svmvisor_completer_tb.sv','tb/svmvisor_journal_tb.sv','tb/svmvisor_snapshot_tb.sv','tb/svmvisor_percpu_snapshot_tb.sv','tb/svmvisor_spi_flash_model.sv','tb/svmvisor_payload_spi_tb.sv','tb/svmvisor_payload_completer_tb.sv') | ForEach-Object { Join-Path $PSScriptRoot "rtl/$_" }
Push-Location $sim
try {
    & (Join-Path $bin 'xvlog.bat') -sv @sources (Join-Path $config.VivadoRoot 'Vivado/data/verilog/src/glbl.v')
    if ($LASTEXITCODE -ne 0) { throw 'Endpoint RTL compile failed.' }
    foreach ($name in $Suite) {
        $top = "svmvisor_${name}_tb"
        & (Join-Path $bin 'xelab.bat') -L xpm $top glbl -s $top
        if ($LASTEXITCODE -ne 0) { throw "Elaboration failed: $top" }
        $output = & (Join-Path $bin 'xsim.bat') $top -runall 2>&1
        $output | Set-Content -LiteralPath "$top.log"
        $output | Write-Host
        if ($LASTEXITCODE -ne 0 -or ($output -join "`n") -notmatch 'PASS:' -or ($output -join "`n") -match 'Fatal:') {
            throw "Simulation failed: $top (logs: $sim)"
        }
    }
} finally { Pop-Location }
Write-Host "Endpoint simulations passed: $sim"

