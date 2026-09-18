param([switch]$CheckOnly)
$ErrorActionPreference = 'Stop'
Set-Location -LiteralPath $PSScriptRoot
try {
    $pins = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'SHA256.json') -Raw | ConvertFrom-Json
    foreach ($pin in $pins) {
        $actual = (Get-FileHash -LiteralPath (Join-Path $PSScriptRoot $pin.path) -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($actual -ne $pin.sha256) { throw "File damaged or changed: $($pin.path). Copy the kit again." }
    }
    $arch = 'amd64'
    if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64' -or $env:PROCESSOR_ARCHITEW6432 -eq 'ARM64') { $arch = 'arm64' }
    $python = Join-Path $PSScriptRoot "runtime/$arch/python.exe"
    & $python --version
    if ($LASTEXITCODE -ne 0) { throw 'Portable Python could not start.' }
    if ($CheckOnly) {
        & $python -B -c "import sys,unittest; sys.path.insert(0,'firmware/card'); r=unittest.TextTestRunner().run(unittest.defaultTestLoader.discover('firmware/card',pattern='test_*.py')); sys.exit(not r.wasSuccessful())"
        if ($LASTEXITCODE -ne 0) { throw 'Offline decoder tests failed.' }
        & './target/firmware/tools/openocd/bin/openocd.exe' --version
        if ($LASTEXITCODE -ne 0) { throw 'OpenOCD could not start.' }
        Write-Host 'PASS: files, runtime and offline decoder checked. No USB access performed.'
        Write-Host 'Expected image: FPGA @FPGA_BUILD_ID@  ROM @ROM_BUILD_ID@  (@LOADER_MODE@ loader, decoder @SOURCE_COMMIT@, kit @KIT_DATE@)'
        Write-Host 'USB driver and live capture still require the connected card.'
    } else {
        New-Item -ItemType Directory -Force 'RESULTS' | Out-Null
        Write-Host 'Checking USB before capture. Keep the card powered independently by the desktop.'
        & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'driver/Recover-USB.ps1') -DesktopPowered
        if ($LASTEXITCODE -ne 0) { throw 'USB recovery failed. See the latest RESULTS/usb-recovery log.' }
        $runId = (Get-Date -Format 'yyyyMMdd-HHmmss') + '-' + [guid]::NewGuid().ToString('N').Substring(0,8)
        $capture = "RESULTS/snapshot-$runId"
        $ErrorActionPreference = 'Continue'
        # --summary prints the decoded stage/stop fields; the full snapshot.json
        # and the raw openocd.log are kept in the capture directory.
        & $python -B 'firmware/card/read_snapshot.py' --live --manifest 'expected-build.json' --output-dir $capture --summary 2>&1 | Tee-Object -FilePath "RESULTS/console-$runId.txt"
        $captureExit = $LASTEXITCODE
        $ErrorActionPreference = 'Stop'
        Write-Host "Reader exit code: $captureExit"
        $parts = @("RESULTS/console-$runId.txt")
        if (Test-Path -LiteralPath $capture) { $parts += $capture }
        Compress-Archive -Path $parts -DestinationPath "RESULTS/capture-$runId.zip"
        Write-Host "Send this file back: $PSScriptRoot\RESULTS\capture-$runId.zip"
        if ($captureExit -ne 0) { Write-Host 'Capture did not validate. Keep the raw logs; do not flash or reset to fix this.'; exit $captureExit }
    }
} catch {
    Write-Host "ERROR: $_"
    Write-Host 'Keep this message. Do not reset or program the card.'
    exit 1
}
