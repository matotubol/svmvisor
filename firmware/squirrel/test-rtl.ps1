[CmdletBinding()]
param(
    [string] $VivadoRoot
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$workspaceRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..')).Path
$config = Import-PowerShellDataFile -LiteralPath (Join-Path $PSScriptRoot 'config.psd1')

if ([string]::IsNullOrWhiteSpace($VivadoRoot)) {
    $VivadoRoot = $config.VivadoRoot
}

$vivadoBin = Join-Path $VivadoRoot 'Vivado\bin'
$xvlog = Join-Path $vivadoBin 'xvlog.bat'
$xelab = Join-Path $vivadoBin 'xelab.bat'
$xsim = Join-Path $vivadoBin 'xsim.bat'
foreach ($tool in @($xvlog, $xelab, $xsim)) {
    if (-not (Test-Path -LiteralPath $tool)) {
        throw "Vivado simulation tool was not found at '$tool'."
    }
}

$upstreamHeaderDirectory = Join-Path $workspaceRoot 'target\fpga\PCIeSquirrel\src'
$rtl = Join-Path $PSScriptRoot 'rtl\svmvisor_option_rom.sv'
$completionRtl = Join-Path $PSScriptRoot 'rtl\svmvisor_tlps128_bar_rdengine.sv'
$fifoStubs = Join-Path $PSScriptRoot 'rtl\tb\fifo_stubs.sv'
$optionRomTestbench = Join-Path $PSScriptRoot 'rtl\tb\svmvisor_option_rom_tb.sv'
$completionTestbench = Join-Path $PSScriptRoot 'rtl\tb\svmvisor_tlps128_bar_rdengine_tb.sv'
foreach ($source in @(
    (Join-Path $upstreamHeaderDirectory 'pcileech_header.svh'),
    $rtl,
    $completionRtl,
    $fifoStubs,
    $optionRomTestbench,
    $completionTestbench
)) {
    if (-not (Test-Path -LiteralPath $source)) {
        throw "Required RTL simulation source was not found at '$source'. Run bootstrap.ps1 first if the pinned upstream checkout is absent."
    }
}

$simulationRoot = Join-Path $workspaceRoot 'target\firmware\squirrel\sim\option-rom'
New-Item -ItemType Directory -Force -Path $simulationRoot | Out-Null

Push-Location $simulationRoot
try {
    & $xvlog -sv -i $upstreamHeaderDirectory $fifoStubs $rtl $completionRtl $optionRomTestbench $completionTestbench
    if ($LASTEXITCODE -ne 0) {
        throw "xvlog failed with exit code $LASTEXITCODE."
    }

    foreach ($test in @(
        @{ Top = 'svmvisor_option_rom_tb'; Snapshot = 'svmvisor_option_rom_tb_snapshot' },
        @{ Top = 'svmvisor_tlps128_bar_rdengine_tb'; Snapshot = 'svmvisor_tlps128_bar_rdengine_tb_snapshot' }
    )) {
        & $xelab $test.Top -s $test.Snapshot
        if ($LASTEXITCODE -ne 0) {
            throw "xelab failed for $($test.Top) with exit code $LASTEXITCODE."
        }

        & $xsim $test.Snapshot -runall
        if ($LASTEXITCODE -ne 0) {
            throw "xsim failed for $($test.Top) with exit code $LASTEXITCODE."
        }
    }
}
finally {
    Pop-Location
}
