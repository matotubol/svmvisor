[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$DeliveryDirectory,
    [ValidateSet('Fx','Sse','Avx')][string]$Profile='Fx',
    [ValidateSet(1,4)][int]$Processors=1,
    [ValidateSet('Positive','Header','Digest','PristineRefused','StructuredRefused')][string]$Mode='Positive',
    [ValidateSet(6,7)][int]$JournalDetail=6,
    [switch]$TerminalExitBootServices
)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
if($Mode -in @('PristineRefused','StructuredRefused') -and $JournalDetail -ne 7){throw 'Refusal scenarios require explicit JournalDetail 7.'}
if($Mode -in @('PristineRefused','StructuredRefused') -and ($Profile -ne 'Fx' -or $Processors -ne 1)){throw 'Bounded refusal scenarios require Fx and one processor.'}
if($TerminalExitBootServices -and ($JournalDetail -ne 7 -or $Mode -ne 'StructuredRefused')){throw 'Terminal ExitBootServices requires StructuredRefused detail 7.'}
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$delivery=(Resolve-Path -LiteralPath $DeliveryDirectory).Path
$qemuDir=Join-Path $root 'target/synthetic-tools/qemu-10.1.0'
$qemu=Join-Path $qemuDir 'qemu-system-x86_64.exe'
$qemuHash=(Get-FileHash -LiteralPath $qemu).Hash.ToLowerInvariant()
if ($qemuHash -cne '57448131c0fbaed74e059ab0f12b97d6ec278c0215330e585004102859a2be71') {throw 'Exact QEMU executable pin required.'}
$session=Join-Path $root ('target/native-returning-card-test/'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $session | Out-Null
$record=[ordered]@{schema=1;status='building';emulatorOnly=$true;hardwareAccessed=$false;secureBootPolicyTested=$false;profile=$Profile;processors=$Processors;mode=$Mode;session=$session;delivery=$delivery;parentFeature='card-returning-loader';childFeature='native-transition-test';qemuSha256=$qemuHash}
$record.schema=if($JournalDetail -eq 7){2}else{1}
$record.journalDetail=$JournalDetail
$record.terminalExitBootServices=[bool]$TerminalExitBootServices
$record.stopAndPostReturnBootServicesTested=-not [bool]$TerminalExitBootServices
$oldParent=$env:SVMVISOR_RETURNING_PARENT
$oldSlot=$env:SVMVISOR_RETURNING_SLOT
$oldDetail=$env:SVMVISOR_JOURNAL_DETAIL
$oldMode=$env:SVMVISOR_RETURNING_MODE
$oldTerminal=$env:SVMVISOR_TERMINAL_EBS
function Check-Exit([string]$Operation){if($LASTEXITCODE -ne 0){throw "$Operation failed ($LASTEXITCODE)"}}
function Hash([string]$Path){(Get-FileHash -LiteralPath $Path).Hash.ToLowerInvariant()}
try {
    $manifest=Get-Content -Raw -LiteralPath (Join-Path $delivery 'manifest.json') | ConvertFrom-Json
    if($manifest.status -cne 'built_review_required' -or $manifest.payload_kind -cne 'EmulatorFixture'){throw 'Reviewed offline EmulatorFixture delivery required.'}
    foreach($pair in @(@('svmvisor-dxe.efi','loader_sha256'),@('payload/payload-slot.bin','slot_sha256'),@('payload/pe-header.bin','pin_sha256'),@('reviewed-child.efi','payload_sha256'))){
        $source=Join-Path $delivery $pair[0]
        if((Hash $source) -cne $manifest.($pair[1])){throw "Delivery artifact hash mismatch: $($pair[0])"}
        $dest=Join-Path $session $pair[0]
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dest) | Out-Null
        Copy-Item -LiteralPath $source -Destination $dest
    }
    Copy-Item -LiteralPath (Join-Path $delivery 'manifest.json') -Destination (Join-Path $session 'delivery-manifest.json')
    Copy-Item -LiteralPath (Join-Path $delivery 'child-build-evidence.bin') -Destination (Join-Path $session 'child-build-evidence.bin')
    $record.deliveryManifestSha256=Hash (Join-Path $session 'delivery-manifest.json')
    $record.parentSha256=Hash (Join-Path $session 'svmvisor-dxe.efi')
    $record.childSha256=Hash (Join-Path $session 'reviewed-child.efi')
    $record.slotSha256=Hash (Join-Path $session 'payload/payload-slot.bin')
    $record.pinSha256=Hash (Join-Path $session 'payload/pe-header.bin')
    $record.parentBytes=(Get-Item -LiteralPath (Join-Path $session 'svmvisor-dxe.efi')).Length
    $record.childBytes=(Get-Item -LiteralPath (Join-Path $session 'reviewed-child.efi')).Length
    # Verify archived source against each build manifest. Runtime never depends
    # on the mutable production checkout after the two PE images were built.
    foreach($input in $manifest.source_hashes){
        $source=Join-Path $delivery ('reviewed-source/'+$input.path)
        if((Hash $source) -cne $input.sha256){throw "Parent source checkpoint mismatch: $($input.path)"}
        $dest=Join-Path $session ('parent-source/'+$input.path)
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dest)|Out-Null
        Copy-Item -LiteralPath $source -Destination $dest
    }
    $childEvidence=Get-Content -Raw -LiteralPath (Join-Path $session 'child-build-evidence.bin')|ConvertFrom-Json
    if($childEvidence.status -cne 'built' -or $childEvidence.feature -cne 'native-transition-test' -or $childEvidence.child_sha256 -cne $record.childSha256){throw 'Current mailbox child build evidence required'}
    foreach($input in $childEvidence.source_hashes){
        $source=Join-Path $childEvidence.source_root $input.path
        if((Hash $source) -cne $input.sha256){throw "Child source checkpoint mismatch: $($input.path)"}
        $dest=Join-Path $session ('child-source/'+$input.path)
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dest)|Out-Null
        Copy-Item -LiteralPath $source -Destination $dest
    }
    $record.childEvidenceSha256=Hash (Join-Path $session 'child-build-evidence.bin')
    $files=@()
    foreach($directory in @('tools/native-returning-card-test')){
        $files+=@(Get-ChildItem -LiteralPath (Join-Path $root $directory) -File -Recurse | Where-Object {$_.FullName -notmatch '[\\/]target[\\/]|[\\/]__pycache__[\\/]'} | ForEach-Object {$_.FullName.Substring($root.Length+1).Replace('\','/')})
    }
    $hashes=@()
    foreach($file in ($files | Sort-Object -Unique)){
        $dest=Join-Path $session ('reviewed-source/'+$file)
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dest) | Out-Null
        Copy-Item -LiteralPath (Join-Path $root $file) -Destination $dest
        $hashes+=@{path=$file;sha256=(Hash $dest)}
    }
    $record.sourceHashes=$hashes
    $toolHashes=@()
    foreach($name in @('rustc','cargo','clang','llvm-objdump','python')){
        $path=if($name -in @('rustc','cargo')){(& rustup which $name).Trim()}elseif($name -eq 'python'){(& python -c 'import sys; print(sys.executable)').Trim()}else{(Get-Command $name -CommandType Application -ErrorAction Stop).Source}
        $toolHashes+=@{name=$name;path=$path;bytes=(Get-Item -LiteralPath $path).Length;sha256=(Hash $path)}
    }
    $record.toolHashes=$toolHashes
    $env:SVMVISOR_RETURNING_PARENT=Join-Path $session 'svmvisor-dxe.efi'
    $env:SVMVISOR_RETURNING_SLOT=Join-Path $session 'payload/payload-slot.bin'
    $env:SVMVISOR_JOURNAL_DETAIL=[string]$JournalDetail
    $env:SVMVISOR_RETURNING_MODE=$Mode
    $env:SVMVISOR_TERMINAL_EBS=if($TerminalExitBootServices){'1'}else{'0'}
    $features=@()
    if($Profile -eq 'Sse'){$features+='boundary-sse'}
    if($Profile -eq 'Avx'){$features+='boundary-avx'}
    if($Mode -eq 'Header'){$features+='header-negative'}
    if($Mode -eq 'Digest'){$features+='digest-negative'}
    $featureArgs=@()
    if($features.Count -gt 0){$featureArgs=@('--features',($features -join ','))}
    $launcherManifest=Join-Path $session 'reviewed-source/tools/native-returning-card-test/Cargo.toml'
    $record.launcherBuild=@('cargo','build','--offline','--locked','--manifest-path',$launcherManifest,'--release','--target','x86_64-unknown-uefi','--target-dir',(Join-Path $session 'launcher-cargo'))+$featureArgs
    & cargo build --offline --locked --manifest-path $launcherManifest --release --target x86_64-unknown-uefi --target-dir (Join-Path $session 'launcher-cargo') @featureArgs 2>&1 | Tee-Object -FilePath (Join-Path $session 'launcher-build.log')
    Check-Exit 'Launcher build'
    $launcher=Join-Path $session 'launcher-cargo/x86_64-unknown-uefi/release/svmvisor-native-returning-card-test.efi'
    Copy-Item -LiteralPath $launcher -Destination (Join-Path $session 'launcher.efi')
    $record.launcherSha256=Hash (Join-Path $session 'launcher.efi')
    $record.launcherBuildLogSha256=Hash (Join-Path $session 'launcher-build.log')
    foreach($input in $hashes){if((Hash (Join-Path $root $input.path)) -cne $input.sha256){throw "Input changed while compiling: $($input.path)"}}
    foreach($input in $toolHashes){if((Hash $input.path) -cne $input.sha256){throw "Tool changed while compiling: $($input.name)"}}
    $boot=Join-Path $session 'esp/EFI/BOOT'
    New-Item -ItemType Directory -Force -Path $boot | Out-Null
    Copy-Item -LiteralPath $launcher -Destination (Join-Path $boot 'BOOTX64.EFI')
    & (Join-Path $qemuDir 'qemu-img.exe') convert -f vvfat -O raw ('fat:'+(Join-Path $session 'esp')) (Join-Path $session 'esp.img')
    Check-Exit 'ESP creation'
    $record.espImageSha256=Hash (Join-Path $session 'esp.img')
    Copy-Item -LiteralPath (Join-Path $qemuDir 'share/edk2-i386-vars.fd') -Destination (Join-Path $session 'vars.fd')
    $record.firmwareInputs=@('edk2-x86_64-code.fd','edk2-i386-vars.fd')|ForEach-Object{@{path=$_;sha256=(Hash (Join-Path $qemuDir "share/$_"))}}
    & llvm-objdump --disassemble (Join-Path $session 'svmvisor-dxe.efi') | Set-Content -LiteralPath (Join-Path $session 'parent-disassembly.txt')
    Check-Exit 'Parent disassembly'
    & llvm-objdump --disassemble (Join-Path $session 'reviewed-child.efi') | Set-Content -LiteralPath (Join-Path $session 'child-disassembly.txt')
    Check-Exit 'Child disassembly'
    $terminalArgs=@()
    if($TerminalExitBootServices){$terminalArgs=@('--terminal-ebs')}
    & python -B (Join-Path $session 'reviewed-source/tools/native-returning-card-test/journal_model.py') --qemu $qemu --firmware (Join-Path $qemuDir 'share/edk2-x86_64-code.fd') --vars (Join-Path $session 'vars.fd') --disk (Join-Path $session 'esp.img') --session $session --processors $Processors --journal-detail $JournalDetail --mode $Mode @terminalArgs
    Check-Exit 'Exact card integration'
    $trace=Get-Content -Raw -LiteralPath (Join-Path $session 'debug.log')
    $passes=@('firmware-journal-uc-qualified','external-journal-model-attached','real-parent-load-start-owner','reentrant-supported-start-stop-refused','parent-start-abi-canaries','observed-host-state-unchanged','real-loaded-image-count-restored','exact-parent-result-journal')
    if($TerminalExitBootServices){$passes+='genuine-exit-boot-services-final-journal'}else{$passes+=@('permanent-one-attempt-latch-after-stop','stop-cleanup-decode-events','real-pci-ownership-released','fixture-protocols-removed','post-card-boot-services')}
    foreach($line in $passes){
        if([regex]::Matches($trace,[regex]::Escape("PASS $line")+"`n").Count -ne 1){throw "Missing or duplicate pass: $line"}
    }
    if($trace -cmatch '(?m)^FAIL '){throw 'Guest failure marker'}
    $model=Get-Content -Raw -LiteralPath (Join-Path $session 'journal-model.json')|ConvertFrom-Json
    $expectedCommits=if($TerminalExitBootServices){6}elseif($Mode -in @('Header','Digest')){3}else{5}
    if($model.status -cne 'passed' -or $model.exitCode -ne 33 -or $model.commits.Count -ne $expectedCommits){throw 'External journal model evidence invalid'}
    if($Mode -notin @('Header','Digest') -and $trace -notmatch 'PASS ready-after-lifecycle-journal'){throw 'Missing lifecycle evidence'}
    $console=Get-Content -Raw -LiteralPath (Join-Path $session 'serial.log')
    if($Mode -eq 'Positive'){
        if($trace -notmatch 'PASS ready-after-lifecycle-journal'){throw 'Missing lifecycle evidence'}
        $expectedProfile=if($Profile -eq 'Avx'){7}elseif($Profile -eq 'Sse'){3}else{0}
        $expected=[ordered]@{'entry-profile'=$expectedProfile;'cpu-total'=$Processors;'cpu-enabled'=$Processors;'cpu-ap-completed'=($Processors-1);'cpu-scoped-complete'=1;'transition-outcome'=2;'transition-refusal'=0;'transition-progress'=14;'transition-vmruns'=1;'transition-exits'=1;'transition-events-released'=1;'transition-restored'=1;'transition-guest-exit'=0x81;'transition-guest-rip'=0x1096;'transition-guest-rax'=0x53564d4e41544956;'transition-guest-captured'=15;'transition-adapter-checks'=15;'transition-canary-failures'=0;'transition-canary-observed'=1;'transition-canary-called'=1;'transition-canary-changed'=$(if($Profile -eq 'Avx'){7}else{3})}
        $observed=@{}
        foreach($field in $expected.Keys){
            $matches=[regex]::Matches($console,'SVMVISOR snapshot '+[regex]::Escape($field)+'=([0-9a-f]{16})')
            if($matches.Count -ne 1){throw "Missing/duplicate child field $field"}
            $value=[Convert]::ToUInt64($matches[0].Groups[1].Value,16)
            if($value -ne $expected[$field]){throw "Child field mismatch $field=$value"}
            $observed[$field]=$value
        }
        $record.childObservation=$observed
    }elseif($Mode -eq 'StructuredRefused'){
        $profileField=[regex]::Matches($console,'SVMVISOR snapshot entry-profile=([0-9a-f]{16})')
        if($profileField.Count -ne 1 -or [Convert]::ToUInt64($profileField[0].Groups[1].Value,16) -ne 0){throw 'Structured refusal must enter Rust under FX profile'}
        if([regex]::Matches($console,'SVMVISOR native-preflight CPUID refused code=00000005').Count -ne 1){throw 'Missing actual SvmUnsupported console observation'}
        if($console -match 'SVMVISOR snapshot transition-'){throw 'Structured refusal unexpectedly reached transition fixture'}
        $record.childObservation=@{'entry-profile'=0;'cpuid-refusal'=5}
    }elseif($console -match 'SVMVISOR snapshot '){throw 'Pre-Rust refusal unexpectedly entered native child'}
    $record.journalModelSha256=Hash (Join-Path $session 'journal-model.json')
    $record.debugSha256=Hash (Join-Path $session 'debug.log')
    $record.serialSha256=Hash (Join-Path $session 'serial.log')
    $record.status='passed'
    Write-Output "PASS returning-card $Mode $Profile $Processors CPUs: $session"
} catch {$record.status='failed';$record.error=$_.Exception.Message;throw}
finally{
    $env:SVMVISOR_RETURNING_PARENT=$oldParent;$env:SVMVISOR_RETURNING_SLOT=$oldSlot
    $env:SVMVISOR_JOURNAL_DETAIL=$oldDetail;$env:SVMVISOR_RETURNING_MODE=$oldMode;$env:SVMVISOR_TERMINAL_EBS=$oldTerminal
    $record.artifactHashes=@('launcher-build.log','launcher.efi','parent-disassembly.txt','child-disassembly.txt','journal-model.json','debug.log','serial.log','stdout.log','stderr.log','vars.fd')|ForEach-Object{
        $path=Join-Path $session $_
        if(Test-Path -LiteralPath $path -PathType Leaf){@{path=$_;bytes=(Get-Item -LiteralPath $path).Length;sha256=(Hash $path)}}
    }
    $record|ConvertTo-Json -Depth 9|Set-Content -LiteralPath (Join-Path $session 'result.json') -Encoding utf8
}
