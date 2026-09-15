[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$CheckpointDirectory,
    [string]$SourceRoot=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
)
# Offline build. Keep the checkpoint path short for the Windows host linker.
# No hardware, flash, proxy, guest boot or physical service is invoked here.
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$source=(Resolve-Path -LiteralPath $SourceRoot).Path
$checkpoint=[IO.Path]::GetFullPath($CheckpointDirectory)
if(Test-Path -LiteralPath $checkpoint){throw 'Checkpoint must be a new directory; existing evidence is immutable.'}
if($checkpoint.Length -gt 80){throw 'Use a short checkpoint path (at most80 characters) for Windows linker paths.'}
function Hash([string]$Path){(Get-FileHash -LiteralPath $Path).Hash.ToLowerInvariant()}
function Check-Exit([string]$What){if($LASTEXITCODE -ne 0){throw "$What failed ($LASTEXITCODE)."}}
New-Item -ItemType Directory -Path $checkpoint|Out-Null
$record=[ordered]@{schema=1;status='capturing';emulatorOnly=$true;feature='native-transition-test';source_root=$checkpoint;original_source_root=$source;panic_guard='unchanged unresolved svmvisor_dxe_must_not_panic link gate'}
function Save-Record{$record|ConvertTo-Json -Depth 9|Set-Content -LiteralPath (Join-Path $checkpoint 'child-build-evidence.json') -Encoding utf8}
try{
    $files=@('Cargo.toml','Cargo.lock','rust-toolchain.toml','.cargo/config.toml')
    foreach($directory in @('crates','tools/rompack','tools/synthetic-harness/firmware-handoff','firmware/squirrel')){
        $files+=@(Get-ChildItem -LiteralPath (Join-Path $source $directory) -File -Recurse|Where-Object{$_.FullName -notmatch '[\\/](target|__pycache__)[\\/]'}|ForEach-Object{$_.FullName.Substring($source.Length+1).Replace('\','/')})
    }
    $hashes=@()
    foreach($file in ($files|Sort-Object -Unique)){
        $original=Join-Path $source $file;$expected=Hash $original;$dest=Join-Path $checkpoint $file
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dest)|Out-Null
        Copy-Item -LiteralPath $original -Destination $dest
        if((Hash $dest) -cne $expected){throw "Snapshot copy raced: $file"}
        $hashes+=@{path=$file;sha256=$expected}
    }
    foreach($item in $hashes){if((Hash (Join-Path $source $item.path)) -cne $item.sha256){throw "Source mutated during snapshot: $($item.path)"}}
    $record.source_hashes=$hashes
    $hashes|ConvertTo-Json -Depth 5|Set-Content -LiteralPath (Join-Path $checkpoint 'source-checkpoint.json') -Encoding utf8
    $record.snapshot_manifest_sha256=Hash (Join-Path $checkpoint 'source-checkpoint.json')
    $toolHashes=@()
    foreach($name in @('rustc','cargo','clang','llvm-objdump','python')){
        $path=if($name -in @('rustc','cargo')){(& rustup which $name).Trim()}elseif($name -eq 'python'){(& python -c 'import sys; print(sys.executable)').Trim()}else{(Get-Command $name -CommandType Application -ErrorAction Stop).Source}
        $toolHashes+=@{name=$name;path=$path;bytes=(Get-Item -LiteralPath $path).Length;sha256=(Hash $path)}
    }
    $sysroot=(& rustc --print sysroot).Trim()
    foreach($file in @(Get-ChildItem -LiteralPath (Join-Path $sysroot 'lib/rustlib/x86_64-unknown-uefi/lib') -File) + @(Get-Item -LiteralPath (Join-Path $sysroot 'lib/rustlib/x86_64-pc-windows-msvc/bin/rust-lld.exe'))){
        $toolHashes+=@{name=$file.Name;path=$file.FullName;bytes=$file.Length;sha256=(Hash $file.FullName)}
    }
    $record.tool_hashes=$toolHashes
    $record.rustc=@(& rustc -Vv)
    $record.clang=@(& clang --version)
    $record.build_command='cargo build --locked --package svmvisor-dxe --profile dxe --features native-transition-test --target x86_64-unknown-uefi'
    $record.status='building';Save-Record
    # Explicit manifest/target directory: the mutable live checkout is unused.
    & cargo build --locked --manifest-path (Join-Path $checkpoint 'Cargo.toml') --package svmvisor-dxe --profile dxe --features native-transition-test --target x86_64-unknown-uefi --target-dir (Join-Path $checkpoint 'child-cargo') 2>&1|Tee-Object -FilePath (Join-Path $checkpoint 'child-build.log')
    Check-Exit 'Frozen child build'
    foreach($item in $hashes){if((Hash (Join-Path $checkpoint $item.path)) -cne $item.sha256){throw "Frozen source changed: $($item.path)"}}
    foreach($item in $toolHashes){if((Hash $item.path) -cne $item.sha256){throw "Tool changed during child build: $($item.name)"}}
    $child=Join-Path $checkpoint 'child-cargo/x86_64-unknown-uefi/dxe/svmvisor-dxe.efi'
    $record.child_sha256=Hash $child;$record.child_bytes=(Get-Item -LiteralPath $child).Length
    $record.status='built';Save-Record
    & (Join-Path $checkpoint 'firmware/squirrel/build-returning-card.ps1') -PayloadPath $child -PayloadSha256 $record.child_sha256 -PayloadKind EmulatorFixture -PayloadEvidencePath (Join-Path $checkpoint 'child-build-evidence.json')
    Check-Exit 'Frozen parent packaging'
    Write-Output "Frozen returning-card build checkpoint: $checkpoint"
}catch{$record.status='failed';$record.error=$_.Exception.Message;Save-Record;throw}
