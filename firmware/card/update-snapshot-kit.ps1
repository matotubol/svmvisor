[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$KitPath,
    [string]$CandidatePath
)
# Refresh the portable read-only snapshot kit (second PC + card UPDATE USB port)
# from this checkout: current decoder, its tests, the USER2/USER3 JTAG config and
# the build IDs of the candidate that is on the card. The kit keeps its own
# offline Python runtimes, OpenOCD and Zadig; this script never touches hardware.
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$kit=[IO.Path]::GetFullPath($KitPath)
foreach ($required in 'runtime/amd64/python.exe','driver/Recover-USB.ps1','target/firmware/tools/openocd/bin/openocd.exe') {
    if (-not (Test-Path -LiteralPath (Join-Path $kit $required))) { throw "Not a snapshot kit (missing $required): $kit" }
}
if (-not $CandidatePath) {
    $pin=Import-PowerShellDataFile -LiteralPath (Join-Path $PSScriptRoot 'candidate-pin.psd1')
    $CandidatePath=Join-Path $root $pin.Candidate
}
$manifest=Get-Content -LiteralPath (Join-Path $CandidatePath 'manifest.json') -Raw | ConvertFrom-Json
foreach ($id in $manifest.fpga_build_id,$manifest.rom_build_id) { if ($id -cnotmatch '^[0-9a-f]{16}$') { throw 'Candidate manifest has no build IDs.' } }

# The pre-rename decoder is kept, not deleted, so old captures stay decodable.
$old=Join-Path $kit 'firmware/squirrel'
if (Test-Path -LiteralPath $old) {
    $archive=Join-Path $kit ('archive/firmware-squirrel-'+(Get-Date -Format 'yyyyMMdd-HHmmss'))
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $archive) | Out-Null
    Move-Item -LiteralPath $old -Destination $archive
}
$card=Join-Path $kit 'firmware/card'
if (Test-Path -LiteralPath $card) { Remove-Item -LiteralPath $card -Recurse -Force }
New-Item -ItemType Directory -Force -Path (Join-Path $card 'openocd'),(Join-Path $card 'tests') | Out-Null
$files=@('read_snapshot.py','openocd/read_snapshot.cfg','tests/percpu-record-v1.hex')+
    @(Get-ChildItem -LiteralPath $PSScriptRoot -Filter 'test_*.py' -File | ForEach-Object Name)
foreach ($file in $files) { Copy-Item -LiteralPath (Join-Path $PSScriptRoot $file) -Destination (Join-Path $card $file) }

$utf8=[Text.UTF8Encoding]::new($false)
[IO.File]::WriteAllText((Join-Path $kit 'expected-build.json'),
    ([ordered]@{rom_build_id=$manifest.rom_build_id;fpga_build_id=$manifest.fpga_build_id} | ConvertTo-Json)+"`n",$utf8)
$commit=(& git -C $root rev-parse --short HEAD 2>$null)
$templates=Join-Path $PSScriptRoot 'snapshot-kit'
foreach ($name in 'Run.ps1','START-HERE.txt','READ-SNAPSHOT.cmd','CHECK-SETUP.cmd') {
    $text=[IO.File]::ReadAllText((Join-Path $templates $name))
    $text=$text.Replace('@FPGA_BUILD_ID@',$manifest.fpga_build_id).Replace('@ROM_BUILD_ID@',$manifest.rom_build_id).Replace('@LOADER_MODE@',[string]$manifest.loader_mode).Replace('@SOURCE_COMMIT@',[string]$commit).Replace('@KIT_DATE@',(Get-Date -Format 'yyyy-MM-dd'))
    [IO.File]::WriteAllText((Join-Path $kit $name),$text,$utf8)
}

# Pin every kit file the launcher executes or reads; results, old captures and
# unrelated folders are not part of the kit.
$skip='^(RESULTS|archive|outputs)[\/]|[\/]snapshots[\/]|__pycache__|^SHA256\.json$'
$pins=@(Get-ChildItem -LiteralPath $kit -File -Recurse | ForEach-Object {
    $relative=$_.FullName.Substring($kit.Length+1).Replace('\','/')
    if ($relative -notmatch $skip) { [ordered]@{path=$relative;sha256=(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()} }
})
[IO.File]::WriteAllText((Join-Path $kit 'SHA256.json'),($pins | ConvertTo-Json)+"`n",$utf8)
Write-Output "Kit updated: $kit"
Write-Output "  decoder from commit $commit; expects FPGA $($manifest.fpga_build_id) ROM $($manifest.rom_build_id) ($($manifest.loader_mode) loader); $($pins.Count) files pinned"
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $kit 'Run.ps1') -CheckOnly
if ($LASTEXITCODE -ne 0) { throw 'Kit self-check failed.' }
