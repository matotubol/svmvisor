[CmdletBinding()]
param([switch]$CardLoadOnly, [string]$PayloadPath, [int]$RomSizeBytes = 32768)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$workspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$config = Import-PowerShellDataFile (Join-Path $PSScriptRoot 'config.psd1')
$output = Join-Path $workspaceRoot ('target/firmware/card/endpoint/' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $output | Out-Null
$romParameters = @{}
if ($CardLoadOnly) {
    if (-not $PayloadPath) { throw 'Card load-only requires an explicit reviewed SVMRELO1 PayloadPath.' }
    & python (Join-Path $PSScriptRoot 'package-card-payload.py') --payload $PayloadPath --output (Join-Path $output 'payload')
    if ($LASTEXITCODE -ne 0) { throw 'Card payload packaging failed.' }
    $payloadManifest = Get-Content -Raw -LiteralPath (Join-Path $output 'payload/payload-manifest.json') | ConvertFrom-Json
    $romParameters = @{ CardLoadOnly=$true; CardPayloadSha256=$payloadManifest.package_sha256; RomSizeBytes=$RomSizeBytes }
} elseif ($PayloadPath) { throw 'PayloadPath requires CardLoadOnly.' }
& (Join-Path $PSScriptRoot 'package-rom.ps1') @romParameters -OutputPath (Join-Path $output 'svmvisor-dxe.rom') -MemoryPath (Join-Path $output 'svmvisor-dxe.mem')
# Vivado's in-memory project cleanup can remove an imported .mem source.
# Retain the exact packaged bytes outside that imported file's name.
Copy-Item -LiteralPath (Join-Path $output 'svmvisor-dxe.mem') -Destination (Join-Path $output 'svmvisor-dxe.mem.source')
$efiRelative = if ($CardLoadOnly) { 'target/card-load-only-cargo/x86_64-unknown-uefi/dxe/svmvisor-dxe.efi' } else { 'target/x86_64-unknown-uefi/dxe/svmvisor-dxe.efi' }
Copy-Item -LiteralPath (Join-Path $workspaceRoot $efiRelative) -Destination (Join-Path $output 'svmvisor-dxe.efi')
$sourceFiles = @('rtl/svmvisor_pcie_pkg.sv','rtl/svmvisor_bypass.sv','rtl/svmvisor_journal.sv','rtl/svmvisor_snapshot.sv','rtl/svmvisor_percpu_snapshot.sv','rtl/svmvisor_completer.sv',
    'rtl/svmvisor_tx_guard.sv','rtl/svmvisor_endpoint.sv','rtl/svmvisor_endpoint.xdc','vivado/endpoint.tcl',
    'build-endpoint.ps1','package-rom.ps1','package-card-payload.py','config.psd1','rtl/svmvisor_payload_spi.sv')
$hashes = [ordered] @{}
if ($CardLoadOnly) {
    $hashes['payload/payload.reloc'] = $payloadManifest.package_sha256
    $hashes['payload/payload-slot.bin'] = $payloadManifest.slot_sha256
}
foreach ($source in $sourceFiles) {
    $destination = Join-Path "$output/sources" $source
    New-Item -ItemType Directory -Force -Path (Split-Path $destination -Parent) | Out-Null
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot $source) -Destination $destination
    $hashes[$source] = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLowerInvariant()
}
# Retain the exact Rust/packager inputs as well as the resulting EFI/ROM.
$softwareFiles = @('Cargo.toml','Cargo.lock','rust-toolchain.toml','.cargo/config.toml') +
    @(Get-ChildItem (Join-Path $workspaceRoot 'crates/dxe'),(Join-Path $workspaceRoot 'tools/rompack'),(Join-Path $workspaceRoot 'tools/synthetic-harness/firmware-handoff') -File -Recurse |
        Where-Object { $_.FullName -notmatch '[\\/]target[\\/]' } |
        ForEach-Object { $_.FullName.Substring($workspaceRoot.Length + 1).Replace('\','/') })
foreach ($source in $softwareFiles) {
    $destination = Join-Path "$output/software" $source
    New-Item -ItemType Directory -Force -Path (Split-Path $destination -Parent) | Out-Null
    Copy-Item -LiteralPath (Join-Path $workspaceRoot $source) -Destination $destination
    $hashes["software/$source"] = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLowerInvariant()
}
(& rustc -Vv) | Set-Content -LiteralPath (Join-Path $output 'rustc-version.txt')
$hashes['rustc-version.txt'] = (Get-FileHash (Join-Path $output 'rustc-version.txt') -Algorithm SHA256).Hash.ToLowerInvariant()
# An embedded ID cannot be the hash of the final bitstream containing that ID.
# Hash an explicitly retained canonical source/build recipe instead; retain the
# independent final bitstream hash in the manifest as before.
$romHash=(Get-FileHash (Join-Path $output 'svmvisor-dxe.rom') -Algorithm SHA256).Hash.ToLowerInvariant()
$recipe = "svmvisor-build-recipe-v1`nvivado=2026.1`nrom_sha256=$romHash`n"
if ($CardLoadOnly) { $recipe += "feature=card-load-only`nrom_bytes=$RomSizeBytes`npayload_flash_offset=4194304`npayload_sha256=$($payloadManifest.package_sha256)`n" }
foreach ($key in ($hashes.Keys | Sort-Object)) { $recipe += "$key=$($hashes[$key])`n" }
[IO.File]::WriteAllText((Join-Path $output 'build-recipe.txt'),$recipe,[Text.UTF8Encoding]::new($false))
$recipeHash=(Get-FileHash (Join-Path $output 'build-recipe.txt') -Algorithm SHA256).Hash.ToLowerInvariant()
function Get-EmbeddedId([string] $digest) {
    $pairs=for($i=0;$i -lt 16;$i+=2) { $digest.Substring($i,2) }
    [array]::Reverse($pairs)
    return ($pairs -join '')
}
$fpgaId=Get-EmbeddedId $recipeHash
$romId=Get-EmbeddedId $romHash
$candidateArguments = if ($CardLoadOnly) { @('1', [string]$RomSizeBytes) } else { @() }
& (Join-Path $config.VivadoRoot 'Vivado/bin/vivado.bat') -mode batch -nojournal -log (Join-Path $output 'vivado.log') `
    -source (Join-Path $output 'sources/vivado/endpoint.tcl') -tclargs (Join-Path $output 'sources') $output (Join-Path $output 'svmvisor-dxe.mem') $fpgaId $romId @candidateArguments
if ($LASTEXITCODE -ne 0) { throw "Endpoint build failed ($LASTEXITCODE): $output" }
Copy-Item -LiteralPath (Join-Path $output 'svmvisor-dxe.mem.source') -Destination (Join-Path $output 'svmvisor-dxe.mem')
$image = Get-Item -LiteralPath (Join-Path $output 'svmvisor-endpoint.bin')
if ($CardLoadOnly) {
    & python (Join-Path $output 'sources/package-card-payload.py') --payload (Join-Path $output 'payload/payload.reloc') --output (Join-Path $output 'payload') --configuration $image.FullName
    if ($LASTEXITCODE -ne 0) { throw 'Combined card review layout failed.' }
}
[ordered] @{
    schema_version=1; target_part='xc7a35tfgg484-2'; image_kind=$(if ($CardLoadOnly) { 'card_load_only_review' } else { 'completion_only' })
    image_sha256=(Get-FileHash -LiteralPath $image.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    image_size_bytes=$image.Length
    # Routing alone does not pass the complete policy/physical-run gates.
    completion_only_netlist_policy='PENDING_REVIEW'
    source_sha256=$hashes; physical_fixture_test='NOT_RUN'; first_light_qualification='NOT_RUN'
    physical_run_ready=$false; journal_jtag_qualification='INCOMPLETE'
    fpga_build_id=$fpgaId; rom_build_id=$romId; build_recipe_sha256=$recipeHash; rom_sha256=$romHash
    card_load_only=[bool]$CardLoadOnly; payload_manifest=$(if ($CardLoadOnly) { 'payload/payload-manifest.json' } else { $null })
    payload_package_sha256=$(if ($CardLoadOnly) { $payloadManifest.package_sha256 } else { $null })
    payload_slot_sha256=$(if ($CardLoadOnly) { $payloadManifest.slot_sha256 } else { $null })
    payload_provenance=$(if ($CardLoadOnly) { 'explicit prebuilt SVMRELO1 input; bound by retained package digest' } else { $null })
} | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $output 'manifest.json') -Encoding UTF8
Write-Host "Endpoint build candidate: $output"


