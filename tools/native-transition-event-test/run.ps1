[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('Nmi','Init','Smi')][string]$Event,
    [ValidateRange(5,60)][int]$EventTimeoutSeconds=15
)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$qemuDir=Join-Path $root 'target/synthetic-tools/qemu-10.1.0'
$qemu=Join-Path $qemuDir 'qemu-system-x86_64.exe'
$expectedQemuHash='57448131C0FBAED74E059AB0F12B97D6EC278C0215330E585004102859A2BE71'
$version=@(& $qemu --version)[0]
if ($LASTEXITCODE -ne 0 -or $version -notmatch 'version 10\.1\.0\b' -or (Get-FileHash -LiteralPath $qemu).Hash -ne $expectedQemuHash) { throw 'The pinned QEMU 10.1.0 TCG executable is required' }
$session=Join-Path $root ('target/native-transition-event-test/'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $session -Force | Out-Null
$record=[ordered]@{event=$Event;status='building';emulatorOnly=$true;hardwareAccessed=$false;session=$session;preStgiCleanupObserved=$false;firmwareEventDeliveryObserved=$false;returningRestorationPassed=$false}
$oldDriver=$env:SVMVISOR_PREFLIGHT_DRIVER
$process=$null
$script:qmp=$null
$script:qtest=$null
$script:gdb=$null
$script:qmpId=0
$script:gdbStop=$null
$script:metadata=$null

function Hex64([uint64]$Value) { return ('0x{0:x16}' -f $Value) }
function U64([byte[]]$Bytes,[int]$Offset) { return [BitConverter]::ToUInt64($Bytes,$Offset) }
function U32([byte[]]$Bytes,[int]$Offset) { return [BitConverter]::ToUInt32($Bytes,$Offset) }
function U16([byte[]]$Bytes,[int]$Offset) { return [BitConverter]::ToUInt16($Bytes,$Offset) }
function Require([bool]$Condition,[string]$Message) { if (-not $Condition) { throw $Message } }
function Read-SharedText([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return '' }
    $file=[IO.FileStream]::new($Path,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::ReadWrite)
    $reader=[IO.StreamReader]::new($file)
    try { return $reader.ReadToEnd() } finally { $reader.Dispose() }
}
function Log-Protocol([string]$Name,[string]$Direction,[string]$Text) {
    Add-Content -LiteralPath (Join-Path $session ($Name+'.log')) -Value (([DateTime]::UtcNow.ToString('O'))+' '+$Direction+' '+$Text)
}
function New-LocalPort {
    $listener=[Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback,0)
    $listener.Start()
    try { return $listener.LocalEndpoint.Port } finally { $listener.Stop() }
}
function Connect-Channel([int]$Port,[string]$Name) {
    $deadline=[DateTime]::UtcNow.AddSeconds(8)
    do {
        $client=[Net.Sockets.TcpClient]::new()
        try { $client.Connect([Net.IPAddress]::Loopback,$Port); break }
        catch { $client.Dispose(); if ([DateTime]::UtcNow -ge $deadline -or $process.HasExited) { throw }; Start-Sleep -Milliseconds 50 }
    } while ($true)
    $client.NoDelay=$true
    $stream=$client.GetStream()
    $stream.ReadTimeout=4000
    $stream.WriteTimeout=4000
    if ($Name -eq 'gdb') { return @{Client=$client;Stream=$stream;Name=$Name} }
    $reader=[IO.StreamReader]::new($stream,[Text.Encoding]::ASCII,$false,65536,$true)
    $writer=[IO.StreamWriter]::new($stream,[Text.Encoding]::ASCII,65536,$true)
    $writer.NewLine="`n"; $writer.AutoFlush=$true
    return @{Client=$client;Stream=$stream;Reader=$reader;Writer=$writer;Name=$Name}
}
function Read-ChannelLine($Channel) {
    $line=$Channel.Reader.ReadLine()
    if ($null -eq $line) { throw ($Channel.Name+' disconnected') }
    Log-Protocol $Channel.Name '<' $line
    return $line
}
function Invoke-Qmp([string]$Command,[hashtable]$Arguments=@{}) {
    $script:qmpId++
    $request=@{execute=$Command;id=$script:qmpId}
    if ($Arguments.Count) { $request.arguments=$Arguments }
    $line=ConvertTo-Json -InputObject $request -Compress -Depth 8
    Log-Protocol 'qmp' '>' $line
    $script:qmp.Writer.WriteLine($line)
    do {
        $reply=ConvertFrom-Json -InputObject (Read-ChannelLine $script:qmp) -AsHashtable
        if ($reply.ContainsKey('id') -and $reply.id -eq $script:qmpId) {
            if ($reply.ContainsKey('error')) { throw ('QMP '+$Command+': '+($reply.error | ConvertTo-Json -Compress)) }
            return $reply['return']
        }
    } while ($true)
}
function Invoke-Qtest([string]$Command) {
    Log-Protocol 'qtest' '>' $Command
    $script:qtest.Writer.WriteLine($Command)
    $reply=Read-ChannelLine $script:qtest
    if (-not $reply.StartsWith('OK')) { throw ('qtest command failed: '+$reply) }
    return $reply
}
function Read-PhysicalBytes([uint64]$Address,[int]$Length) {
    Require ($Length -gt 0 -and $Length -le (33*4096) -and $Address -ge 0x100000 -and $Address -le (0x10000000-$Length)) 'Physical observation is outside the fixed TCG RAM bounds'
    $reply=Invoke-Qtest ('read 0x{0:x} 0x{1:x}' -f $Address,$Length)
    Require ($reply.StartsWith('OK 0x')) 'Malformed physical-memory prefix'
    $hex=$reply.Substring(5)
    Require ($hex.Length -eq (2*$Length) -and $hex -match '^[0-9a-fA-F]+$') 'Malformed physical-memory extent'
    return ,([Convert]::FromHexString($hex))
}
function Read-GdbPacket {
    do {
        $byte=$script:gdb.Stream.ReadByte()
        if ($byte -lt 0) { throw 'GDB disconnected' }
        if ($byte -ne 36) { continue }
        $body=[Text.StringBuilder]::new()
        $sum=0
        do {
            $byte=$script:gdb.Stream.ReadByte()
            if ($byte -lt 0) { throw 'GDB disconnected inside packet' }
            if ($byte -eq 35) { break }
            [void]$body.Append([char]$byte)
            $sum=($sum+$byte) -band 255
            Require ($body.Length -le 65536) 'GDB reply exceeds bounded packet size'
        } while ($true)
        $a=$script:gdb.Stream.ReadByte(); $b=$script:gdb.Stream.ReadByte()
        Require ($a -ge 0 -and $b -ge 0) 'GDB packet checksum missing'
        $expected=[Convert]::ToInt32(([string][char]$a+[char]$b),16)
        Require ($sum -eq $expected) 'GDB packet checksum mismatch'
        $script:gdb.Stream.WriteByte(43)
        $result=$body.ToString()
        Log-Protocol 'gdb' '<' $result
        return $result
    } while ($true)
}
function Invoke-Gdb([string]$Command) {
    $sum=0; foreach ($byte in [Text.Encoding]::ASCII.GetBytes($Command)) { $sum=($sum+$byte) -band 255 }
    $packet='$'+$Command+('#{0:x2}' -f $sum)
    Log-Protocol 'gdb' '>' $Command
    $bytes=[Text.Encoding]::ASCII.GetBytes($packet)
    $script:gdb.Stream.Write($bytes,0,$bytes.Length)
    do {
        $reply=Read-GdbPacket
        if ($reply -match '^[STWX][0-9a-fA-F]{2}') {
            $script:gdbStop=$reply
            if ($Command -eq '?') { return $reply }
            continue
        }
        return $reply
    } while ($true)
}
function Set-Breakpoint([uint64]$Address,[bool]$Enabled) {
    Require ($Address -ge 0x100000 -and $Address -lt 0x10000000) 'Debugger breakpoint must remain in emulated firmware RAM'
    $verb=if ($Enabled) {'Z0'} else {'z0'}
    Require ((Invoke-Gdb ('{0},{1:x},1' -f $verb,$Address)) -eq 'OK') 'QEMU internal debugger breakpoint rejected'
}
function Get-Registers { return [string](Invoke-Qmp 'human-monitor-command' @{'command-line'='info registers'}) }
function Register-Value([string]$Text,[string]$Name) {
    $match=[regex]::Match($Text,'(?m)(?:^|\s)'+[regex]::Escape($Name)+'\s*=\s*([0-9a-fA-F]+)')
    Require $match.Success ('Missing actual emulator register '+$Name)
    return [Convert]::ToUInt64($match.Groups[1].Value,16)
}
function Current-Pc([string]$Text) {
    if ($Text -match '(?:^|\s)RIP=') { return Register-Value $Text 'RIP' }
    return Register-Value $Text 'EIP'
}
function Save-Observation([string]$Name,[bool]$OwnershipEstablished=$false) {
    $directory=Join-Path $session $Name
    New-Item -ItemType Directory -Path $directory -Force | Out-Null
    $registers=Get-Registers
    Set-Content -LiteralPath (Join-Path $directory 'registers.txt') -Value $registers
    $arena=Read-PhysicalBytes $script:metadata['event-arena'] (33*4096)
    [IO.File]::WriteAllBytes((Join-Path $directory 'arena.bin'),$arena)
    $context=[byte[]]$arena[(30*4096)..(30*4096+1087)]
    [IO.File]::WriteAllBytes((Join-Path $directory 'context.bin'),$context)
    $summary=[ordered]@{ownershipEstablished=$OwnershipEstablished;interpretation=$(if ($OwnershipEstablished) {'live paused fixture'} else {'forensic emulated physical bytes; allocation lifetime is not established'});pc=(Hex64 (Current-Pc $registers));progress=(U64 $context 960);outcome=(U64 $context 968);refusal=(U64 $context 976);vmruns=(U64 $context 1024);exits=(U64 $context 1032);eventsReleased=(U64 $context 1040);restored=(U64 $context 1048);gdtRepairs=(U64 $context 1056);guestExit=(Hex64 (U64 $context 704));guestRip=(Hex64 (U64 $context 736));guestRax=(Hex64 (U64 $context 760));guestCaptured=(U64 $context 880);vmcbExit=(Hex64 (U64 $arena 0x70));directory=$directory}
    $summary | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $directory 'observation.json')
    return @{Context=$context;Arena=$arena;Registers=$registers;Summary=$summary;Directory=$directory}
}
function In-Spin([uint64]$Pc) { return $Pc -eq $script:metadata['event-spin-rip'] -or $Pc -eq ($script:metadata['event-spin-end-rip']-2) }
function Check-PreStgi($Observation) {
    $c=$Observation.Context; $s=$Observation.Summary
    Require ((Current-Pc $Observation.Registers) -eq $script:metadata['event-stgi-va']) 'Stopped PC is not the exported STGI site'
    Require ($s.progress -eq 10 -and $s.refusal -eq 0 -and $s.vmruns -eq 1 -and $s.exits -eq 1 -and $s.eventsReleased -eq 0 -and $s.restored -eq 0 -and $s.gdtRepairs -le 4) 'Pre-STGI journal does not describe completed cleanup before event release'
    Require ((U64 $c 880) -eq 15) 'Actual guest capture is incomplete'
    $expectedExit=switch ($Event) {'Nmi' {0x61} 'Init' {0x63} 'Smi' {0x81}}
    $expectedOutcome=switch ($Event) {'Nmi' {3} 'Init' {4} 'Smi' {2}}
    Require ((U64 $c 704) -eq $expectedExit -and (U64 $Observation.Arena 0x70) -eq $expectedExit -and $s.outcome -eq $expectedOutcome) 'Actual event exit or classification differs from the requested event'
    $guestRip=U64 $c 736
    if ($Event -eq 'Smi') { Require ($guestRip -eq $script:metadata['event-vmmcall-rip']) 'SMI round trip did not reach the fixed VMMCALL' }
    else { Require (In-Spin $guestRip) 'Intercepted guest RIP was outside the observed waiting loop' }
    Require ((U64 $c 760) -eq 0x53564d4e41544956 -and (U64 $c 744) -eq 0x9000) 'Guest RAX or stack was not the fixed guest state'
    $seeds=@(0x301,0x302,0x303,0x304,0x305,0x306,0x308,0x309,0x30a,0x30b,0x30c,0x30d,0x30e,0x30f)
    for ($i=0;$i -lt $seeds.Count;$i++) { Require ((U64 $c (768+8*$i)) -eq $seeds[$i]) ('Guest register capture mismatch at index '+$i) }
    for ($i=0;$i -lt 256;$i+=8) {
        $expected=U64 $c (192+$i)
        if ($i -eq 48) { $expected=$expected -bor 0x1000 }
        Require ((U64 $c (448+$i)) -eq $expected) ('Retained scalar comparison differs at offset '+$i)
    }
    foreach ($item in @(@('CR0',8),@('CR2',16),@('CR3',24),@('CR4',32),@('DR0',72),@('DR1',80),@('DR2',88),@('DR3',96),@('DR6',104),@('DR7',112))) {
        Require ((Register-Value $Observation.Registers $item[0]) -eq (U64 $c (192+$item[1]))) ('Actual pre-STGI register differs: '+$item[0])
    }
    Require ((Register-Value $Observation.Registers 'EFER') -eq ((U64 $c 240) -bor 0x1000)) 'Actual pre-STGI EFER is not original plus SVME'
    Require (((Register-Value $Observation.Registers 'RFL') -band 0x44700) -eq 0) 'Unexpected pre-STGI IF/DF/TF/NT/AC flags'
    # The canary records RSP before CALL; CALL(8), PUSHFQ+15 GPRs(128), locals(136).
    $canary=32*4096
    $callerRsp=U64 $Observation.Arena ($canary+48)
    Require ((U64 $Observation.Arena ($canary+184)) -eq $script:metadata['event-context-pa'] -and (U64 $Observation.Arena ($canary+240)) -eq 0 -and (U64 $Observation.Arena ($canary+248)) -eq 1) 'Immediate-call canary is not suspended at its one transition call'
    Require ($callerRsp -ge 272 -and (Register-Value $Observation.Registers 'RSP') -eq ($callerRsp-272)) 'Actual pre-STGI RSP differs from the exact transition frame'
    $gdtBase=U64 $c 394; $gdtBytes=U64 $c 176; $copyBase=U64 $c 168
    Require ($gdtBytes -ge 8 -and $gdtBytes -le 65536 -and $gdtBytes -eq ((U16 $c 392)+1)) 'GDT extent does not match original GDTR'
    $live=Read-PhysicalBytes $gdtBase ([int]$gdtBytes)
    $copy=Read-PhysicalBytes $copyBase ([int]$gdtBytes)
    [IO.File]::WriteAllBytes((Join-Path $Observation.Directory 'gdt-live.bin'),$live)
    [IO.File]::WriteAllBytes((Join-Path $Observation.Directory 'gdt-original.bin'),$copy)
    Require ([Convert]::ToHexString($live) -ceq [Convert]::ToHexString($copy)) 'Actual live GDT differs before STGI'
    $gdtMatch=[regex]::Match($Observation.Registers,'(?m)GDT=\s*([0-9a-fA-F]+)\s+([0-9a-fA-F]+)')
    $idtMatch=[regex]::Match($Observation.Registers,'(?m)IDT=\s*([0-9a-fA-F]+)\s+([0-9a-fA-F]+)')
    Require ($gdtMatch.Success -and $idtMatch.Success) 'Actual emulator descriptor-table registers missing'
    Require ([Convert]::ToUInt64($gdtMatch.Groups[1].Value,16) -eq $gdtBase -and [Convert]::ToUInt64($gdtMatch.Groups[2].Value,16) -eq ($gdtBytes-1)) 'Actual GDTR differs from original'
    Require ([Convert]::ToUInt64($idtMatch.Groups[1].Value,16) -eq (U64 $c 410) -and [Convert]::ToUInt64($idtMatch.Groups[2].Value,16) -eq (U16 $c 408)) 'Actual IDTR differs from original'
    foreach ($item in @(@('CS',0),@('SS',1),@('DS',2),@('ES',3),@('FS',4),@('GS',5),@('LDT',6),@('TR',7))) {
        Require ((Register-Value $Observation.Registers $item[0]) -eq (U16 $c (424+2*$item[1]))) ('Actual segment selector differs: '+$item[0])
    }
    $record.preStgiCleanupObserved=$true
    $record.preStgi=$s
}
function Breakpoint-Reached {
    $status=Invoke-Qmp 'query-status'
    return (-not $status.running -and $status.status -eq 'debug')
}
function Read-TransitionFields([string]$Console) {
    $fields=@{}
    foreach ($field in @('outcome','refusal','progress','vmruns','exits','events-released','restored','gdt-accessed-restores','guest-exit','guest-rip','guest-rax','guest-captured','adapter-checks','canary-failures','canary-observed','canary-called','canary-changed')) {
        $matches=[regex]::Matches($Console,'SVMVISOR snapshot transition-'+$field+'=([0-9a-f]{16})')
        Require ($matches.Count -eq 1) ('Missing or duplicate final transition observation: '+$field)
        $fields[$field]=[Convert]::ToUInt64($matches[0].Groups[1].Value,16)
    }
    return $fields
}

try {
    Write-Output "Building the exact event fixture for $Event. $session"
    & cargo build --locked --manifest-path (Join-Path $root 'Cargo.toml') -p svmvisor-dxe --profile dxe --target x86_64-unknown-uefi --features native-transition-event-test --target-dir (Join-Path $session 'driver-cargo')
    if ($LASTEXITCODE -ne 0) { throw 'Event DXE build failed' }
    $driver=Join-Path $session 'driver-cargo/x86_64-unknown-uefi/dxe/svmvisor-dxe.efi'
    $env:SVMVISOR_PREFLIGHT_DRIVER=$driver
    $launcherDirectory=Join-Path $root 'tools/native-preflight-test'
    & cargo build --locked --manifest-path (Join-Path $launcherDirectory 'Cargo.toml') --release --target x86_64-unknown-uefi --features boundary-call --target-dir (Join-Path $session 'launcher-cargo')
    if ($LASTEXITCODE -ne 0) { throw 'Actual-image launcher build failed' }
    $launcher=Join-Path $session 'launcher-cargo/x86_64-unknown-uefi/release/svmvisor-native-preflight-test.efi'
    $boot=Join-Path $session 'esp/EFI/BOOT'
    New-Item -ItemType Directory -Force -Path $boot | Out-Null
    Copy-Item -LiteralPath $launcher -Destination (Join-Path $boot 'BOOTX64.EFI')
    & (Join-Path $qemuDir 'qemu-img.exe') convert -f vvfat -O raw ('fat:'+(Join-Path $session 'esp')) (Join-Path $session 'esp.img')
    if ($LASTEXITCODE -ne 0) { throw 'ESP creation failed' }
    Copy-Item -LiteralPath (Join-Path $qemuDir 'share/edk2-i386-vars.fd') -Destination (Join-Path $session 'vars.fd')
    $debug=Join-Path $session 'debug.log'; $serial=Join-Path $session 'serial.log'; $cpuLog=Join-Path $session 'cpu.log'
    $ports=@(); while ($ports.Count -lt 3) { $candidate=New-LocalPort; if ($candidate -notin $ports) { $ports+= $candidate } }
    $arguments=@('-machine','q35,accel=tcg','-cpu','max,svm=on,hypervisor=off','-m','256M','-smp','1','-S',
        '-drive',('"if=pflash,format=raw,readonly=on,file='+(Join-Path $qemuDir 'share/edk2-x86_64-code.fd')+'"'),
        '-drive',('"if=pflash,format=raw,file='+(Join-Path $session 'vars.fd')+'"'),
        '-drive',('"format=raw,snapshot=on,file='+(Join-Path $session 'esp.img')+'"'),
        '-qmp',('tcp:127.0.0.1:'+$ports[0]+',server=on,wait=off'),'-qtest',('tcp:127.0.0.1:'+$ports[1]+',server=on,wait=off'),'-qtest-log',('"'+(Join-Path $session 'qtest-server.log')+'"'),
        '-gdb',('tcp:127.0.0.1:'+$ports[2]),'-d','int,cpu_reset,guest_errors','-D',('"'+$cpuLog+'"'),'-trace','enable=apic_deliver_irq',
        '-display','none','-serial',('"file:'+$serial+'"'),'-monitor','none','-nic','none','-no-reboot',
        '-debugcon',('"file:'+$debug+'"'),'-device','isa-debug-exit,iobase=0xf4,iosize=0x04')
    $record.arguments=$arguments; $record.qemuVersion=$version; $record.qemuSha256=$expectedQemuHash
    $record.driverSha256=(Get-FileHash -LiteralPath $driver).Hash; $record.launcherSha256=(Get-FileHash -LiteralPath $launcher).Hash
    $record.buildInputs=@('Cargo.toml','Cargo.lock','rust-toolchain.toml','.cargo/config.toml') | ForEach-Object { @{path=$_;sha256=(Get-FileHash -LiteralPath (Join-Path $root $_)).Hash} }
    $record.firmwareInputs=@('edk2-x86_64-code.fd','edk2-i386-vars.fd') | ForEach-Object { @{path=$_;sha256=(Get-FileHash -LiteralPath (Join-Path $qemuDir ('share/'+$_))).Hash} }
    $record.sourceHashes=@(Get-ChildItem -LiteralPath (Join-Path $root 'crates/dxe'),(Join-Path $root 'crates/hypervisor'),$launcherDirectory,$PSScriptRoot -File -Recurse | Where-Object { $_.FullName -notmatch '[\\/]target[\\/]' } | ForEach-Object { @{path=$_.FullName;sha256=(Get-FileHash -LiteralPath $_.FullName).Hash} })
    & llvm-objdump --disassemble $driver | Set-Content -LiteralPath (Join-Path $session 'driver-disassembly.txt')
    if ($LASTEXITCODE -ne 0) { throw 'Driver disassembly failed' }
    $process=Start-Process -FilePath $qemu -ArgumentList $arguments -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $session 'stdout.log') -RedirectStandardError (Join-Path $session 'stderr.log')
    $processHandle=$process.Handle
    $script:qmp=Connect-Channel $ports[0] 'qmp'
    $greeting=ConvertFrom-Json -InputObject (Read-ChannelLine $script:qmp) -AsHashtable
    Require ($greeting.ContainsKey('QMP')) 'Missing QMP greeting'
    [void](Invoke-Qmp 'qmp_capabilities')
    $script:qtest=Connect-Channel $ports[1] 'qtest'
    $script:gdb=Connect-Channel $ports[2] 'gdb'
    [void](Invoke-Gdb 'qSupported')
    [void](Invoke-Qmp 'cont')
    $record.status='waiting-for-guest'
    $deadline=[DateTime]::UtcNow.AddSeconds(60)
    $required=@('event-arena','event-ready-pa','event-context-pa','event-stgi-va','event-spin-rip','event-spin-end-rip','event-vmmcall-rip')
    do {
        Require (-not $process.HasExited) 'Emulator exited before event guest readiness'
        $console=Read-SharedText $serial
        $script:metadata=@{}
        foreach ($field in $required) {
            $items=[regex]::Matches($console,'SVMVISOR snapshot '+$field+'=([0-9a-f]{16})')
            Require ($items.Count -le 1) ('Duplicate event metadata '+$field)
            if ($items.Count -eq 1) { $script:metadata[$field]=[Convert]::ToUInt64($items[0].Groups[1].Value,16) }
        }
        if ($script:metadata.Count -eq $required.Count) { break }
        Require ($console -notmatch 'transition-fixture-refused=') 'Event resource adapter refused before readiness'
        Require ([DateTime]::UtcNow -lt $deadline) 'Timed out waiting for actual event metadata'
        Start-Sleep -Milliseconds 100
    } while ($true)
    $arena=$script:metadata['event-arena']
    Require ($arena -ge 0x100000 -and $arena -le (0x10000000-33*4096) -and ($arena -band 4095) -eq 0) 'Unexpected event arena'
    Require ($script:metadata['event-ready-pa'] -eq ($arena+8*4096) -and $script:metadata['event-context-pa'] -eq ($arena+30*4096)) 'Event metadata does not match the reviewed owned arena layout'
    Require ($script:metadata['event-spin-rip'] -eq 0x10b4 -and $script:metadata['event-spin-end-rip'] -eq 0x10be -and $script:metadata['event-vmmcall-rip'] -eq 0x10be) 'Event guest code bounds differ from the reviewed fixed instruction layout'
    $record.metadata=$script:metadata
    do {
        $ready=Read-PhysicalBytes $script:metadata['event-ready-pa'] 16
        if ((U64 $ready 0) -eq 0x53564d4556454e54) {
            [void](Invoke-Qmp 'stop')
            $registers=Get-Registers
            if (In-Spin (Current-Pc $registers)) { break }
            [void](Invoke-Qmp 'cont')
        }
        Require (-not $process.HasExited -and [DateTime]::UtcNow -lt $deadline) 'Guest READY/spin observation timed out'
        Start-Sleep -Milliseconds 50
    } while ($true)
    Require ((U64 $ready 8) -eq 0) 'Controller release field was already set'
    $initial=Save-Observation 'ready' $true
    Require ((U64 $initial.Context 0) -eq 1 -and (U64 $initial.Context 8) -eq 1088 -and $initial.Summary.progress -eq 7 -and $initial.Summary.vmruns -eq 1 -and $initial.Summary.exits -eq 0) 'Readiness did not occur inside the one real VMRUN'
    $record.ready=$initial.Summary
    Set-Breakpoint $script:metadata['event-stgi-va'] $true
    $apicId=U32 $initial.Context 440
    Require ($apicId -eq (U64 $initial.Context 152) -and $apicId -eq 0) 'Actual captured BSP APIC ID differs from the one-CPU TCG fixture'
    $cpuBefore=Read-SharedText $cpuLog
    $smmBefore=[regex]::Matches($cpuBefore,'SMM: enter').Count
    $rsmBefore=[regex]::Matches($cpuBefore,'SMM: after RSM').Count
    $resetBefore=[regex]::Matches($cpuBefore,'CPU Reset').Count
    $data=switch ($Event) {'Nmi' {0x400} 'Init' {0x500} 'Smi' {0x200}}
    $address=[uint64]0xfee00000L -bor ([uint64]$apicId -shl 12)
    $record.injection=@{address=(Hex64 $address);data=(Hex64 $data);apicId=$apicId;utc=[DateTime]::UtcNow.ToString('O');readyObserved=$true;spinObserved=$true;smmEntriesBefore=$smmBefore;rsmBefore=$rsmBefore;cpuResetsBefore=$resetBefore}
    Write-Output "Observed the real guest waiting; injecting $Event through QEMU's APIC MSI model."
    [void](Invoke-Qtest ('writel 0x{0:x} 0x{1:x}' -f $address,$data))
    [void](Invoke-Qmp 'cont')
    $deadline=[DateTime]::UtcNow.AddSeconds($EventTimeoutSeconds)
    $releaseWritten=$false
    $reached=$false
    do {
        Require (-not $process.HasExited) 'Emulator exited before a pre-STGI observation'
        if (Breakpoint-Reached) { $reached=$true; break }
        $cpuText=Read-SharedText $cpuLog
        if ($Event -eq 'Smi' -and -not $releaseWritten -and [regex]::Matches($cpuText,'SMM: enter').Count -gt $smmBefore -and [regex]::Matches($cpuText,'SMM: after RSM').Count -gt $rsmBefore) {
            [void](Invoke-Qmp 'stop')
            $registers=Get-Registers
            if (In-Spin (Current-Pc $registers)) {
                $resumed=Save-Observation 'smm-resumed-guest' $true
                Require ($resumed.Summary.progress -eq 7 -and $resumed.Summary.exits -eq 0) 'SMI resumed observation is not the original guest entry'
                $record.firmwareEventDeliveryObserved=$true
                $record.smmResumed=$resumed.Summary
                [void](Invoke-Qtest ('writeb 0x{0:x} 0x1' -f ($script:metadata['event-ready-pa']+8)))
                $releaseWritten=$true
                $record.releaseByteWritten=$true
            }
            [void](Invoke-Qmp 'cont')
        }
        if ($Event -eq 'Init' -and [regex]::Matches($cpuText,'CPU Reset').Count -gt $resetBefore) { break }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    if (-not $reached) {
        [void](Invoke-Qmp 'stop')
        $observed=Save-Observation 'event-did-not-reach-stgi'
        $record.lastObservation=$observed.Summary
        $record.forensicInputsMatchReady=[Convert]::ToHexString([byte[]]$observed.Context[0..191]) -ceq [Convert]::ToHexString([byte[]]$initial.Context[0..191])
        $record.cpuResetsAfter=[regex]::Matches((Read-SharedText $cpuLog),'CPU Reset').Count
        $record.smmEntriesAfter=[regex]::Matches((Read-SharedText $cpuLog),'SMM: enter').Count
        $record.rsmAfter=[regex]::Matches((Read-SharedText $cpuLog),'SMM: after RSM').Count
        if ($Event -eq 'Init' -and $record.cpuResetsAfter -gt $resetBefore -and $record.forensicInputsMatchReady -and (U64 $observed.Arena 0x70) -eq 0x63 -and $observed.Summary.progress -lt 10) {
            $record.status='unsupported-init-reset-before-cleanup'
            $record.reason='Actual INIT VMEXIT was followed by CPU reset before the exported STGI cleanup checkpoint.'
        } elseif ($Event -eq 'Smi' -and $record.smmEntriesAfter -gt $smmBefore -and -not $record.firmwareEventDeliveryObserved) {
            $record.status='unsupported-smm-no-guest-resumption'
            $record.reason='A real SMM entry was observed, but normal RSM and return to the original guest were not established.'
        } else { throw 'Injected event did not establish the required pre-STGI cleanup checkpoint' }
    } else {
        $checkpoint=Save-Observation 'pre-stgi' $true
        if ($Event -eq 'Smi' -and -not $releaseWritten) {
            $record.lastObservation=$checkpoint.Summary
            Require ([regex]::Matches((Read-SharedText $cpuLog),'SMM: enter').Count -gt $smmBefore) 'Early SMI exit has no observed SMM entry'
            $record.status='unsupported-smm-exited-before-resumption'
            $record.reason='Actual SMM entry led to guest exit before normal RSM/resumption and the controller release byte.'
        } else {
            Check-PreStgi $checkpoint
            Write-Output 'Captured actual guest exit, restored host registers, and exact GDT bytes before STGI.'
            Set-Breakpoint $script:metadata['event-stgi-va'] $false
            $nmiHandler=[uint64]0
            if ($Event -eq 'Nmi') {
                $idt=U64 $checkpoint.Context 410
                Require ((U16 $checkpoint.Context 408) -ge 47) 'Original IDT has no full NMI gate'
                $gate=Read-PhysicalBytes ($idt+32) 16
                [IO.File]::WriteAllBytes((Join-Path $checkpoint.Directory 'original-nmi-gate.bin'),$gate)
                Require (($gate[5] -band 0x8f) -eq 0x8e -or ($gate[5] -band 0x8f) -eq 0x8f) 'Original NMI gate is not a present interrupt/trap gate'
                $nmiHandler=[uint64](U16 $gate 0) -bor ([uint64](U16 $gate 6) -shl 16) -bor ([uint64](U32 $gate 8) -shl 32)
                $record.originalNmiHandler=Hex64 $nmiHandler
                Set-Breakpoint $nmiHandler $true
            }
            [void](Invoke-Qmp 'cont')
            $deadline=[DateTime]::UtcNow.AddSeconds($EventTimeoutSeconds)
            do {
                if ($process.HasExited) { break }
                if ($Event -eq 'Nmi' -and -not $record.firmwareEventDeliveryObserved -and (Breakpoint-Reached)) {
                    $delivered=Save-Observation 'original-nmi-handler-entry' $true
                    Require ((Current-Pc $delivered.Registers) -eq $nmiHandler) 'Unexpected breakpoint after event release'
                    $record.firmwareEventDeliveryObserved=$true
                    $record.nmiHandlerEntry=$delivered.Summary
                    Set-Breakpoint $nmiHandler $false
                    [void](Invoke-Qmp 'cont')
                }
                Start-Sleep -Milliseconds 100
            } while ([DateTime]::UtcNow -lt $deadline)
            if ($process.HasExited) {
                $process.WaitForExit(); $process.Refresh(); $record.exitCode=$process.ExitCode
                $trace=Read-SharedText $debug; $console=Read-SharedText $serial
                Require ($process.ExitCode -eq 33 -and $trace -notmatch 'FAIL') 'Launcher did not report an intact actual driver return'
                foreach ($marker in @('PASS actual-dxe-returned-unsupported','PASS post-dxe-boot-services','PASS observed-host-state-unchanged','PASS exact-driver-entry-gpr-flags-xmm','PASS exact-driver-entry-x87-payload','PASS directly-invoked-driver-unloaded','PASS memory-attribute-fixture-removed')) { Require ($trace.Contains($marker)) ('Missing independent launcher evidence: '+$marker) }
                Require ($console -notmatch 'transition-fixture-refused=') 'Returning event fixture reported a refusal'
                $fields=Read-TransitionFields $console
                $record.finalTransition=$fields
                Require ($Event -ne 'Init') 'Unexpected returning INIT is not a successful architectural INIT test'
                $expectedOutcome=if ($Event -eq 'Nmi') {3} else {2}
                Require ($fields.outcome -eq $expectedOutcome -and $fields.refusal -eq 0 -and $fields.progress -eq 14 -and $fields.vmruns -eq 1 -and $fields.exits -eq 1 -and $fields['events-released'] -eq 1 -and $fields.restored -eq 1 -and $fields['gdt-accessed-restores'] -le 4 -and $fields['guest-captured'] -eq 15 -and $fields['adapter-checks'] -eq 15 -and $fields['canary-failures'] -eq 0 -and $fields['canary-observed'] -eq 1 -and $fields['canary-called'] -eq 1 -and $fields['canary-changed'] -eq 3) 'Returning transition observations or immediate-call canary checks failed'
                Require $record.firmwareEventDeliveryObserved 'Returning test did not observe actual firmware event delivery'
                $record.returningRestorationPassed=$true
                $record.status='passed-returning-event'
            } else {
                [void](Invoke-Qmp 'stop')
                Set-Content -LiteralPath (Join-Path $session 'nonreturn-registers.txt') -Value (Get-Registers)
                # Ownership after arbitrary firmware execution is not assumed: no late arena dereference.
                if ($Event -eq 'Nmi' -and $record.firmwareEventDeliveryObserved) {
                    $record.status='observed-cleanup-and-original-nmi-delivery-nonreturn'
                    $record.reason='Cleanup was observed before STGI and execution reached the original firmware NMI handler. The firmware did not return within the bounded interval.'
                } elseif ($Event -eq 'Init' -and [regex]::Matches((Read-SharedText $cpuLog),'CPU Reset').Count -gt $resetBefore) {
                    $record.firmwareEventDeliveryObserved=$true
                    $record.status='observed-cleanup-and-init-reset-nonreturn'
                } else { throw 'No verified firmware event completion after the cleanup checkpoint' }
            }
        }
    }
    Write-Output ($record.status+': '+$session)
} catch {
    $record.status='failed'; $record.error=$_.Exception.Message
    throw
} finally {
    if ($null -ne $process) {
        if (-not $process.HasExited) { $record.controllerTerminatedEmulator=$true; $process.Kill(); $process.WaitForExit() }
        $process.Refresh(); $record.exitCode=$process.ExitCode
    }
    foreach ($channel in @($script:gdb,$script:qtest,$script:qmp)) { if ($null -ne $channel) { $channel.Client.Dispose() } }
    $env:SVMVISOR_PREFLIGHT_DRIVER=$oldDriver
    $record.finishedUtc=[DateTime]::UtcNow.ToString('O')
    $record | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath (Join-Path $session 'result.json')
}
