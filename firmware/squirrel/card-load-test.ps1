[CmdletBinding(SupportsShouldProcess, ConfirmImpact='Medium')]
param([ValidateSet('CheckOnly','Program')][string]$Action='CheckOnly', [switch]$ConfirmFlash)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'card-load-validation.ps1')
Assert-CardLoadAction $Action ([bool]$ConfirmFlash)
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$candidate='target/firmware/squirrel/endpoint/aee684bd8ead4ec3aa8de384b3f7f5f8'
$recovery='target/firmware/squirrel/recovery/a94c8fd7dc2441008d89bc52cf034b1b'
# Exact reviewed inputs only. No arbitrary image/manifest/tool override.
$inputs = @(
    @{ Name='combined.bin'; Path="$candidate/payload/combined-review.bin"; Bytes=5242880; Hash='c686655e362313bb32e5590077dc58a5ed1e6ff549db27cf1f4f55395615d885' },
    @{ Name='configuration.bin'; Path="$candidate/svmvisor-endpoint.bin"; Bytes=1161428; Hash='aa7b372175c6a806311197b3916082980c9fd7edc87c7e03f1813c3c9c250123' },
    @{ Name='payload-slot.bin'; Path="$candidate/payload/payload-slot.bin"; Bytes=1048576; Hash='a7c686cfc1b92e2e08e1e6e6cc376398f58f085072165e02dd3ba06aa3058cde' },
    @{ Name='payload.reloc'; Path="$candidate/payload/payload.reloc"; Bytes=250112; Hash='6d8b7e8c245223a90e105dd74b65e6cadd7666f66f7f73ba0219d7a3a92e9d4d' },
    @{ Name='candidate-manifest.json'; Path="$candidate/manifest.json"; Bytes=-1; Hash='01ca4a3aa7149a4b18d05b8f55cd2ccb35a88857f0db86e017d08afbb28ca957' },
    @{ Name='local-review.json'; Path="$candidate/local-review.json"; Bytes=895; Hash='f87a02e660211839bbfc605de5e5b148bf8f08a7bd4e73dc4eabc43d96210817' },
    @{ Name='payload-manifest.json'; Path="$candidate/payload/payload-manifest.json"; Bytes=-1; Hash='95d86c5e95d708577ecf44e9cfd4523bef5bcdaa12540f0755bd26f48c8e84e1' },
    @{ Name='recovery.bin'; Path="$recovery/svmvisor-recovery.bin"; Bytes=199580; Hash='c731a42b0f0d309661f767de3807d671c1935677def5cb10153ceab257f05d9a' },
    @{ Name='recovery-manifest.json'; Path="$recovery/manifest.json"; Bytes=1177; Hash='7a768d9add688514297c662f1dcc6625807b8cf3b11213b4d32e740971b282b0' },
    @{ Name='openocd.exe'; Path='target/firmware/tools/openocd/bin/openocd.exe'; Bytes=13664247; Hash='9732b05af7e0f6a05a0051371e49af42515662ad309ddcc87f86f9b434ce96d8' },
    @{ Name='proxy.bit'; Path='target/firmware/tools/lambda-squirrel/flash_screamer/bscan_spi_xc7a35t.bit'; Bytes=261513; Hash='ef8af1e277a7fe556e1ed7ace4680d4993cfc4174616485e1c354793d784b7f6' },
    @{ Name='transport.cfg'; Path='firmware/squirrel/openocd/card-load-transport.cfg'; Bytes=-1; Hash='dd2176b5cb7652aceb2f58ea1aed2d8312e8535ef91939fe8fbf5216b7ef3112' },
    @{ Name='backup.cfg'; Path='firmware/squirrel/openocd/card-load-backup.cfg'; Bytes=-1; Hash='45adafe304102dfbdf4f468d9ca857b2d2b134f299025c18307d7b3f6cfc75cf' },
    @{ Name='program.cfg'; Path='firmware/squirrel/openocd/card-load-program.cfg'; Bytes=-1; Hash='ce1a5bc3674c9208387ada7bc5c4149c9588e3e6463e425330824d7f045cdb3c' }
)
foreach ($asset in $inputs) { Assert-CardLoadFile (Join-Path $root $asset.Path) $asset.Bytes $asset.Hash }
if ($Action -eq 'CheckOnly') {
    Write-Output 'PASS card-load offline preparation: exact candidate, recovery, tools, and procedure pins match. No hardware accessed; fresh 5 MiB backup still required during an authorized Program action.'
    return
}
if (-not $PSCmdlet.ShouldProcess('Squirrel XC7A35T / IS25LP256D sectors 0..79', 'Load BSCAN proxy, double-backup 5 MiB, verify backup, program pinned load-only candidate, verify full readback; no activation')) { return }
$session = Join-Path $root ('target/firmware/squirrel/card-load-sessions/' + [Guid]::NewGuid().ToString('N'))
$sessionTcl = ConvertTo-CardLoadTclPath $session
New-Item -ItemType Directory -Path $session | Out-Null
$locks=[Collections.Generic.List[IO.FileStream]]::new()
$record=[ordered]@{ schema_version=1; candidate=$candidate; status='staging'; hardware_accessed=$false; activation_performed=$false; backup_bytes=5242880; backup_sha256=$null; image_sha256=$inputs[0].Hash; error=$null }
function Save-Record { $record | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $session 'result.json') }
function Lock-CheckedInput([string]$Path, [long]$Bytes, [string]$Hash) {
    # Hold read-only handles which deny writes/deletion through both sessions.
    $locks.Add([IO.File]::Open($Path,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read))
    Assert-CardLoadFile $Path $Bytes $Hash
}
function Invoke-CardSession([string]$Config, [string]$Marker) {
    $exe=Join-Path $session 'openocd.exe'
    $log=Join-Path $session ($Config + '.log')
    $record.hardware_accessed=$true
    Save-Record
    # Explicit config; no upstream Tcl sources, automatic downloads, or server.
    $stdout=Join-Path $session ($Config + '.stdout.log')
    $stderr=Join-Path $session ($Config + '.stderr.log')
    $arguments=@('-c','"gdb_port disabled"','-c','"tcl_port disabled"','-c','"telnet_port disabled"','-c',('"set SESSION {' + $sessionTcl + '}"'),'-f',('"' + (Join-Path $session $Config) + '"'))
    $process=Start-Process -FilePath $exe -ArgumentList $arguments -PassThru -WindowStyle Hidden -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    $processHandle=$process.Handle
    if (-not $process.WaitForExit(1800000)) {
        $process.Kill(); $process.WaitForExit()
        throw "OpenOCD stage timed out after 30 minutes: $Config"
    }
    $process.Refresh(); $exitCode=$process.ExitCode
    ((Get-Content -Raw -LiteralPath $stdout) + (Get-Content -Raw -LiteralPath $stderr)) | Set-Content -LiteralPath $log
    $trace=Get-Content -Raw -LiteralPath $log
    if ($null -eq $exitCode -or $exitCode -ne 0 -or $trace -notmatch [regex]::Escape($Marker) -or $trace -notmatch 'PASS card-target-id-and-geometry') { throw "OpenOCD stage failed: $Config; exit=$exitCode. Retained log: $log" }
}
try {
    foreach ($asset in $inputs) {
        $dest=Join-Path $session $asset.Name
        Copy-Item -LiteralPath (Join-Path $root $asset.Path) -Destination $dest
        Lock-CheckedInput $dest $asset.Bytes $asset.Hash
    }
    $record.status='backing_up'; Save-Record
    Invoke-CardSession 'backup.cfg' 'PASS card-double-backup'
    $first=Join-Path $session 'before-a.bin'; $second=Join-Path $session 'before-b.bin'
    $backupHash=Assert-CardLoadBackups $first $second
    Lock-CheckedInput $first 5242880 $backupHash
    Lock-CheckedInput $second 5242880 $backupHash
    $record.backup_sha256=$backupHash; $record.status='programming'; Save-Record
    Invoke-CardSession 'program.cfg' 'PASS card-program-readback'
    Assert-CardLoadFile (Join-Path $session 'readback.bin') 5242880 $inputs[0].Hash
    $record.status='verified_not_activated'; Save-Record
    Write-Output "PASS pinned card-load programming and full 5 MiB readback. No activation. Evidence: $session"
} catch {
    $record.status='failed'; $record.error=$_.Exception.Message; Save-Record
    throw "Card-load procedure stopped; no activation. Evidence: $session. $($_.Exception.Message)"
} finally {
    foreach ($handle in $locks) { $handle.Dispose() }
}
