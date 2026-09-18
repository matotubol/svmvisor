param([switch]$CheckOnly, [int]$WaitSeconds = 600)
$ErrorActionPreference = 'Stop'
Set-Location -LiteralPath $PSScriptRoot

function Write-Step([string]$Text) { Write-Host ''; Write-Host "== $Text" -ForegroundColor Cyan }

# The card's UPDATE port is an FTDI FT4232H; JTAG is its interface 0.
function Find-CardInterface {
    @(Get-PnpDevice -PresentOnly -ErrorAction SilentlyContinue |
        Where-Object { $_.InstanceId -like 'USB\VID_0403&PID_6011&MI_00\*' })
}
function Get-InterfaceDriver($Device) {
    try { (Get-PnpDeviceProperty -InstanceId $Device.InstanceId -KeyName 'DEVPKEY_Device_Service' -ErrorAction Stop).Data } catch { $null }
}
function Wait-Card([int]$Seconds) {
    $deadline = (Get-Date).AddSeconds($Seconds); $told = $false
    while ($true) {
        $found = @(Find-CardInterface)
        if ($found.Count -eq 1) { return $found[0] }
        if ($found.Count -gt 1) { throw 'More than one card programming interface is connected. Leave only one plugged in.' }
        if (-not $told) {
            Write-Host 'Card not found on USB yet.' -ForegroundColor Yellow
            Write-Host '  - Plug the card UPDATE / programming USB port into THIS computer (data cable).'
            Write-Host '  - Keep the hung desktop powered: the snapshot is lost on a power cycle.'
            Write-Host "  Waiting up to $Seconds s (Ctrl+C to stop) ..."
            $told = $true
        }
        if ((Get-Date) -gt $deadline) { return $null }
        Start-Sleep -Seconds 2; Write-Host '.' -NoNewline
    }
}
function Show-Value([string]$Name, $Value) {
    if ($null -eq $Value) { return }
    if ($Value -is [long] -or $Value -is [int] -or $Value -is [decimal] -or $Value -is [double]) {
        $n = [uint64]$Value; Write-Host ('  {0,-32} {1}  (0x{2:x})' -f $Name, $n, $n)
    } else { Write-Host ('  {0,-32} {1}' -f $Name, $Value) }
}
function Show-Result([string]$JsonPath) {
    $s = Get-Content -LiteralPath $JsonPath -Raw | ConvertFrom-Json
    Write-Step 'RESULT'
    Show-Value 'fpga_build_id' $s.fpga_build_id; Show-Value 'rom_build_id' $s.rom_build_id
    Show-Value 'boot_id' $s.boot_id; Show-Value 'phase' $s.phase; Show-Value 'detail' $s.detail
    Show-Value 'rom_reads' $s.rom_reads; Show-Value 'bar_writes' $s.bar_writes
    if ($s.boot_id -eq 0 -and $s.rom_reads -eq 0) {
        Write-Host '  The card recorded nothing this power session: the firmware never read the' -ForegroundColor Yellow
        Write-Host '  option ROM (ROM disabled in BIOS, or the desktop was power-cycled since).' -ForegroundColor Yellow
    }
    foreach ($section in 'native_resident_observation', 'processor_admission_failure', 'native_returning_observation') {
        $value = $s.$section
        if ($null -eq $value) { continue }
        Write-Host ''; Write-Host " [$section]" -ForegroundColor Green
        foreach ($p in $value.PSObject.Properties) {
            if ($p.Name -eq 'raw_words' -or $null -eq $p.Value) { continue }
            Show-Value $p.Name $p.Value
        }
    }
    $all = @($s.percpu_diagnostics.frames)
    $frames = @($all | Where-Object { $_.record_valid })
    Write-Host ''; Write-Host " [per-CPU records] valid: $($frames.Count) of $($all.Count)" -ForegroundColor Green
    foreach ($f in $frames | Select-Object -First 12) {
        $r = $f.record; $name = $r.event_name; if (-not $name) { $name = "event $($r.event)" }
        Write-Host ('  slot {0,-2} {1,-13} {2}' -f $f.processor_slot, $f.kind, $name)
    }
}

try {
    Write-Step 'Checking kit files'
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
    Write-Host 'Expected image: FPGA @FPGA_BUILD_ID@  ROM @ROM_BUILD_ID@  (@LOADER_MODE@ loader, decoder @SOURCE_COMMIT@, kit @KIT_DATE@)'
    if ($CheckOnly) {
        $ErrorActionPreference = 'Continue'
        & $python -B -c "import sys,unittest; sys.path.insert(0,'firmware/card'); r=unittest.TextTestRunner(stream=sys.stdout).run(unittest.defaultTestLoader.discover('firmware/card',pattern='test_*.py')); sys.exit(not r.wasSuccessful())"
        $testsExit = $LASTEXITCODE
        & './target/firmware/tools/openocd/bin/openocd.exe' --version 2>&1 | Select-Object -First 1 | ForEach-Object { "$_" }
        $ErrorActionPreference = 'Stop'
        if ($testsExit -ne 0) { throw 'Offline decoder tests failed.' }
        $present = @(Find-CardInterface)
        if ($present.Count) { Write-Host "Card interface present; driver: $(Get-InterfaceDriver $present[0])" }
        else { Write-Host 'Card interface not connected right now (not required for this check).' }
        Write-Host 'PASS: files, runtime and offline decoder checked. No JTAG access performed.' -ForegroundColor Green
        exit 0
    }

    New-Item -ItemType Directory -Force 'RESULTS' | Out-Null
    Write-Step 'Looking for the card on USB'
    $card = Wait-Card $WaitSeconds
    if (-not $card) {
        Write-Host ''; Write-Host 'Still not found.' -ForegroundColor Yellow
        Write-Host 'Try another cable/port. As a last resort RECOVER-USB.cmd restarts this computer''s USB controllers (admin).'
        throw 'Card programming interface (USB 0403:6011 interface 0) not found.'
    }
    Write-Host ''; Write-Host "Found: $($card.FriendlyName)  [$($card.InstanceId)]" -ForegroundColor Green
    $driver = Get-InterfaceDriver $card
    Write-Host "Driver: $driver"
    if ($driver -ne 'WinUSB') {
        Write-Host 'Interface 0 must use the WinUSB driver (one-time setup):' -ForegroundColor Yellow
        Write-Host '  run driver\zadig-2.9.exe > Options > List All Devices > "Quad RS232-HS (Interface 0)"'
        Write-Host '  (USB ID 0403 6011 00) > WinUSB > Replace Driver. Details: driver\MANUAL-SETUP.txt'
        $answer = Read-Host 'Open Zadig now? (y/n)'
        if ($answer -match '^[yY]') {
            Start-Process -FilePath (Join-Path $PSScriptRoot 'driver/zadig-2.9.exe') -Wait
            $card = Wait-Card 30
            if ($card) { $driver = Get-InterfaceDriver $card; Write-Host "Driver now: $driver" }
        }
        if ($driver -ne 'WinUSB') { throw 'Interface 0 is not bound to WinUSB; OpenOCD cannot open it.' }
    }

    Write-Step 'Reading the snapshot over JTAG (read-only, a few seconds)'
    $runId = (Get-Date -Format 'yyyyMMdd-HHmmss') + '-' + [guid]::NewGuid().ToString('N').Substring(0,8)
    $capture = "RESULTS/snapshot-$runId"
    $ErrorActionPreference = 'Continue'
    & $python -B 'firmware/card/read_snapshot.py' --live --manifest 'expected-build.json' --output-dir $capture --summary 2>&1 |
        ForEach-Object { "$_" } | Out-File -FilePath "RESULTS/console-$runId.txt" -Encoding utf8
    $captureExit = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    $json = Join-Path $capture 'snapshot.json'
    if (Test-Path -LiteralPath $json) { Show-Result $json }
    else { Write-Host 'No decoded snapshot. Reader output:' -ForegroundColor Yellow; Get-Content "RESULTS/console-$runId.txt" | Select-Object -Last 25 }
    $parts = @("RESULTS/console-$runId.txt"); if (Test-Path -LiteralPath $capture) { $parts += $capture }
    Compress-Archive -Path $parts -DestinationPath "RESULTS/capture-$runId.zip"
    Write-Step 'Saved'
    Write-Host "  $PSScriptRoot\RESULTS\capture-$runId.zip   <- keep / send this"
    Write-Host "  full JSON: $PSScriptRoot\$($capture -replace '/','\')\snapshot.json"
    Write-Host "Reader exit code: $captureExit"
    if ($captureExit -ne 0) { Write-Host 'Capture did not validate. Keep the raw logs; do not flash or reset to fix this.' -ForegroundColor Yellow; exit $captureExit }
} catch {
    Write-Host ''; Write-Host "ERROR: $_" -ForegroundColor Red
    Write-Host 'Keep this message. Do not reset or program the card.'
    exit 1
}
