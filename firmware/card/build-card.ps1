[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$PayloadPath,
    [Parameter(Mandatory)][ValidatePattern('^[0-9a-f]{64}$')][string]$PayloadSha256,
    [ValidateSet('NativeResidentBoot')][string]$PayloadKind='NativeResidentBoot',
    [Parameter(Mandatory)][string]$PayloadEvidencePath,
    [Parameter(Mandatory)][string]$ResidentBuildPath,
    [switch]$BuildFpga,
    # Development ROM: build the loader with card-resident-dev-loader, which adopts
    # the header found in the payload slot instead of a compiled-in header pin.
    # The supplied payload only becomes the slot's initial content. Default: pinned.
    [switch]$DevLoader
)
# Offline construction only. This script contains no physical tool invocation.
# One job: build a NativeResidentBoot card candidate from an audited resident build.
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$payload=(Resolve-Path -LiteralPath $PayloadPath).Path
$evidence=(Resolve-Path -LiteralPath $PayloadEvidencePath).Path
if ((Get-FileHash -LiteralPath $payload).Hash.ToLowerInvariant() -cne $PayloadSha256) { throw 'Reviewed child digest mismatch.' }
$resident=$true
$feature=if($DevLoader){'card-resident-dev-loader'}else{'card-resident-loader'}
$deliveryProfile='native-resident'
[string[]]$packageArgs=@('--resident')
$residentVerifier=Join-Path $root 'firmware/card/verify-resident-build.py'
$residentBuild=(Resolve-Path -LiteralPath $ResidentBuildPath).Path
& python -B $residentVerifier --evidence $residentBuild --image $payload
if ($LASTEXITCODE -ne 0) { throw 'Exact production resident audit refused before packaging.' }
$session=Join-Path $root ('target/firmware/card/'+$deliveryProfile+'/'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $session | Out-Null
$record=[ordered]@{schema_version=1;status='building';image_kind=$(if($resident){'native_resident_pe_review'}else{'native_returning_pe_review'});payload_kind=$PayloadKind;physical_run_ready=$false;hardware_accessed=$false;activation_performed=$false;rom_bytes=32768;loader_mode=$(if($DevLoader){'dev'}else{'pinned'});loader_feature=$feature;payload_sha256=$PayloadSha256;payload_evidence_sha256=(Get-FileHash -LiteralPath $evidence).Hash.ToLowerInvariant();error=$null}
$priorPin=$env:SVMVISOR_CARD_PE_HEADER
function Save-Record { $record | ConvertTo-Json -Depth 9 | Set-Content -LiteralPath (Join-Path $session 'manifest.json') -Encoding utf8 }
function Check-Exit([string]$Operation) { if ($LASTEXITCODE -ne 0) { throw "$Operation failed ($LASTEXITCODE)." } }
function Save-Input([string]$Relative) {
    $source=Join-Path $root $Relative
    $dest=Join-Path $session ('reviewed-source/'+$Relative)
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dest) | Out-Null
    Copy-Item -LiteralPath $source -Destination $dest
    return @{path=$Relative;sha256=(Get-FileHash -LiteralPath $dest).Hash.ToLowerInvariant()}
}
function Get-ReturningSourceInputs([string]$SourceRoot) {
    $inputFiles=@('Cargo.toml','Cargo.lock','rust-toolchain.toml','.cargo/config.toml','firmware/card/build-card.ps1','firmware/card/package-payload.py','firmware/card/verify-resident-build.py','firmware/card/tests/test_returning_payload_package.py','firmware/card/tests/test_resident_build_evidence.py','firmware/card/config.psd1')
    # Capture the complete local dependency source checkpoint. In particular,
    # the loader's card-abi contract is outside crates/card-loader, and the child's
    # launcher and hypervisor sources are outside it too.
    $sourceDirectories=@('crates','firmware/card/rtl','firmware/card/vivado')
    foreach ($directory in $sourceDirectories) {
        $inputFiles+=@(Get-ChildItem -LiteralPath (Join-Path $SourceRoot $directory) -File -Recurse | Where-Object {$_.FullName -notmatch '[\\/](target|__pycache__)[\\/]'} | ForEach-Object {$_.FullName.Substring($SourceRoot.Length+1).Replace('\','/')})
    }
    return @($inputFiles | Sort-Object -Unique)
}
try {
    Save-Record
    $record.resident_build_path=$residentBuild
    $record.resident_summary_sha256=(Get-FileHash -LiteralPath (Join-Path $residentBuild 'summary.json')).Hash.ToLowerInvariant()
    $record.resident_source_manifest_sha256=(Get-FileHash -LiteralPath (Join-Path $residentBuild 'source-manifest.json')).Hash.ToLowerInvariant()
    Copy-Item -LiteralPath (Join-Path $residentBuild 'summary.json') -Destination (Join-Path $session 'child-resident-summary.json')
    Copy-Item -LiteralPath (Join-Path $residentBuild 'source-manifest.json') -Destination (Join-Path $session 'child-resident-source-manifest.json')
    $toolHashes=@()
    foreach ($toolName in @('rustc','cargo','python','llvm-objdump')) {
        $toolPath=if ($toolName -in @('rustc','cargo')) { (& rustup which $toolName).Trim() } elseif ($toolName -eq 'python') { (& python -c 'import sys; print(sys.executable)').Trim() } else { (Get-Command $toolName -CommandType Application -ErrorAction Stop).Source }
        if (-not (Test-Path -LiteralPath $toolPath -PathType Leaf)) {throw "Cannot retain actual tool identity: $toolName"}
        $toolHashes+=@{name=$toolName;path=$toolPath;bytes=(Get-Item -LiteralPath $toolPath).Length;sha256=(Get-FileHash -LiteralPath $toolPath).Hash.ToLowerInvariant()}
    }
    $record.tool_hashes=$toolHashes
    Copy-Item -LiteralPath $evidence -Destination (Join-Path $session 'child-build-evidence.bin')
    Copy-Item -LiteralPath $payload -Destination (Join-Path $session 'reviewed-child.efi')
    if ((Get-FileHash -LiteralPath (Join-Path $session 'reviewed-child.efi')).Hash.ToLowerInvariant() -cne $PayloadSha256) {throw 'Child changed while being copied.'}
    if ((Get-FileHash -LiteralPath (Join-Path $session 'child-build-evidence.bin')).Hash.ToLowerInvariant() -cne $record.payload_evidence_sha256) {throw 'Child evidence changed while being copied.'}
    $sources=@(Get-ReturningSourceInputs $root | ForEach-Object {Save-Input $_})
    $record.source_hashes=$sources
    $packager=Join-Path $session 'reviewed-source/firmware/card/package-payload.py'
    & python -B $packager @packageArgs --payload (Join-Path $session 'reviewed-child.efi') --output (Join-Path $session 'payload')
    Check-Exit 'PE packaging'
    # The dev loader compiles in no header; make sure a stale variable cannot matter.
    if ($DevLoader) { Remove-Item Env:SVMVISOR_CARD_PE_HEADER -ErrorAction SilentlyContinue }
    else { $env:SVMVISOR_CARD_PE_HEADER=Join-Path $session 'payload/pe-header.bin' }
    & cargo build --locked --manifest-path (Join-Path $root 'Cargo.toml') --package svmvisor-card-loader --profile rom --features $feature --target x86_64-unknown-uefi --target-dir (Join-Path $session 'loader-cargo')
    Check-Exit 'Loader build'
    Copy-Item -LiteralPath (Join-Path $session 'loader-cargo/x86_64-unknown-uefi/rom/svmvisor-card-loader.efi') -Destination (Join-Path $session 'svmvisor-dxe.efi')
    & cargo run --locked --quiet --release --manifest-path (Join-Path $root 'Cargo.toml') --package xtask --target-dir (Join-Path $session 'rompack-cargo') -- rompack --input (Join-Path $session 'svmvisor-dxe.efi') --output (Join-Path $session 'svmvisor-dxe.rom') --memory-output (Join-Path $session 'svmvisor-dxe.mem') --memory-size 32768 --vendor 0x10ee --device 0x0666 --class 0xff0000
    Check-Exit '32 KiB ROM packaging'
    Copy-Item -LiteralPath (Join-Path $session 'svmvisor-dxe.mem') -Destination (Join-Path $session 'svmvisor-dxe.mem.source')
    foreach ($input in $sources) {
        if ((Get-FileHash -LiteralPath (Join-Path $root $input.path)).Hash.ToLowerInvariant() -cne $input.sha256) {throw "Build input changed during compilation: $($input.path)"}
    }
    $record.loader_bytes=(Get-Item -LiteralPath (Join-Path $session 'svmvisor-dxe.efi')).Length
    $record.loader_sha256=(Get-FileHash -LiteralPath (Join-Path $session 'svmvisor-dxe.efi')).Hash.ToLowerInvariant()
    $record.rom_bytes_used=(Get-Item -LiteralPath (Join-Path $session 'svmvisor-dxe.rom')).Length
    $record.rom_sha256=(Get-FileHash -LiteralPath (Join-Path $session 'svmvisor-dxe.rom')).Hash.ToLowerInvariant()
    $record.slot_sha256=(Get-FileHash -LiteralPath (Join-Path $session 'payload/payload-slot.bin')).Hash.ToLowerInvariant()
    $record.pin_sha256=(Get-FileHash -LiteralPath (Join-Path $session 'payload/pe-header.bin')).Hash.ToLowerInvariant()
    (& rustc -Vv) | Set-Content -LiteralPath (Join-Path $session 'rustc-version.txt')
    Check-Exit 'Compiler identity'
    (& cargo -V) | Set-Content -LiteralPath (Join-Path $session 'cargo-version.txt')
    Check-Exit 'Cargo identity'
    & llvm-objdump --disassemble (Join-Path $session 'svmvisor-dxe.efi') | Set-Content -LiteralPath (Join-Path $session 'loader-disassembly.txt')
    Check-Exit 'Loader disassembly'
    $recipe="svmvisor-returning-pe-recipe-v1`nfeature=$feature`nrom_bytes=32768`npayload_flash_offset=4194304`npayload_sha256=$PayloadSha256`nrom_sha256=$($record.rom_sha256)`npin_sha256=$($record.pin_sha256)`n"
    foreach($input in $sources){$recipe+="$($input.path)=$($input.sha256)`n"}
    foreach($tool in $toolHashes){$recipe+="tool/$($tool.name)=$($tool.sha256)`n"}
    [IO.File]::WriteAllText((Join-Path $session 'build-recipe.txt'),$recipe,[Text.UTF8Encoding]::new($false))
    $record.build_recipe_sha256=(Get-FileHash -LiteralPath (Join-Path $session 'build-recipe.txt')).Hash.ToLowerInvariant()
    function Embedded-Id([string]$Hash) {$pairs=for($i=0;$i -lt 16;$i+=2){$Hash.Substring($i,2)};[array]::Reverse($pairs);return ($pairs -join '')}
    $record.fpga_build_id=Embedded-Id $record.build_recipe_sha256
    $record.rom_build_id=Embedded-Id $record.rom_sha256
    if ($BuildFpga) {
        $config=Import-PowerShellDataFile -LiteralPath (Join-Path $session 'reviewed-source/firmware/card/config.psd1')
        $sourceRoot=Join-Path $session 'reviewed-source/firmware/card'
        & (Join-Path $config.VivadoRoot 'Vivado/bin/vivado.bat') -mode batch -nojournal -log (Join-Path $session 'vivado.log') -source (Join-Path $sourceRoot 'vivado/endpoint.tcl') -tclargs $sourceRoot $session (Join-Path $session 'svmvisor-dxe.mem') $record.fpga_build_id $record.rom_build_id 1 32768
        Check-Exit 'Returning endpoint synthesis and routing'
        Copy-Item -LiteralPath (Join-Path $session 'svmvisor-dxe.mem.source') -Destination (Join-Path $session 'svmvisor-dxe.mem')
        & python -B $packager @packageArgs --payload (Join-Path $session 'reviewed-child.efi') --output (Join-Path $session 'combined') --configuration (Join-Path $session 'svmvisor-endpoint.bin')
        Check-Exit 'Combined 5 MiB layout'
        $record.image_sha256=(Get-FileHash -LiteralPath (Join-Path $session 'svmvisor-endpoint.bin')).Hash.ToLowerInvariant()
        $record.image_size_bytes=(Get-Item -LiteralPath (Join-Path $session 'svmvisor-endpoint.bin')).Length
        $record.combined_sha256=(Get-FileHash -LiteralPath (Join-Path $session 'combined/combined-review.bin')).Hash.ToLowerInvariant()
        $record.combined_bytes=(Get-Item -LiteralPath (Join-Path $session 'combined/combined-review.bin')).Length
        $record.target_part='xc7a35tfgg484-2'
        $record.completion_only_netlist_policy='PENDING_REVIEW'
    }
    & python -B $residentVerifier --evidence $residentBuild --image (Join-Path $session 'reviewed-child.efi')
    Check-Exit 'Final exact resident build audit'
    if ((Get-FileHash -LiteralPath (Join-Path $residentBuild 'summary.json')).Hash.ToLowerInvariant() -cne $record.resident_summary_sha256) { throw 'Resident evidence changed during packaging.' }
    $record.status='built_review_required'
    Write-Output "Offline returning PE delivery artifacts: $session"
} catch {$record.status='failed';$record.error=$_.Exception.Message;throw}
finally {$env:SVMVISOR_CARD_PE_HEADER=$priorPin;Save-Record}
