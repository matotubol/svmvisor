<#
.SYNOPSIS
Performs fail-closed PE/COFF and instruction-safety checks on svmvisor EFI images.

.DESCRIPTION
Strictly validates the probe and DXE images as AMD64 PE32+ EFI binaries with
non-empty AMD64 base relocations. The probe's .text section is then disassembled
with llvm-objdump and inspected as parsed instructions. Exactly the reviewed
read-only RDMSR site set is permitted: the AMD VM_CR (C001_0114h) site plus the
41 allowlisted system-register sites from docs/m0b-msr-read-policy.md (42
sites), each immediately preceded by a literal 'mov ecx, 0x<reviewed address>'.
The per-processor slices may execute each site at most once on each measured
processor.
State-changing SVM, MSR, control-register,
port-I/O, and other privileged instructions are rejected. HLT is intentionally
allowed because the UEFI panic handler uses it after reporting a fatal error.
This establishes the application instruction boundary only; it does not prove
that opaque UEFI MP Services dispatch preserves an AP's pre-dispatch state.

The llvm-objdump "Intel syntax" option controls assembly notation only. It does
not select, identify, or assume an Intel processor; this checker targets an AMD
platform and the AMD VM_CR MSR.

.PARAMETER ProbeEfiPath
Path to the svmvisor M0b EFI application.

.PARAMETER DxeEfiPath
Path to the svmvisor DXE boot-service driver.

.PARAMETER LlvmObjdumpPath
Optional explicit path to llvm-objdump. When omitted, the executable is located
on PATH or in the standard LLVM installation directory.
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string] $ProbeEfiPath,

    [Parameter(Mandatory)]
    [string] $DxeEfiPath,

    [Alias('ObjdumpPath')]
    [string] $LlvmObjdumpPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Assert-Condition {
    param(
        [Parameter(Mandatory)] $Condition,
        [Parameter(Mandatory)] [string] $Message
    )

    if (-not $Condition) {
        throw $Message
    }
}

function Get-ReviewedMsrSiteAllowlist {
    # The 42 reviewed read-only RDMSR sites: AMD VM_CR plus the 41
    # system-register sites reviewed in docs/m0b-msr-read-policy.md.
    $allowlist = [ordered]@{}
    foreach ($entry in @(
        @(0x000000fe, 'MTRRcap'),
        @(0x00000200, 'MTRRphysBase0'),
        @(0x00000201, 'MTRRphysMask0'),
        @(0x00000202, 'MTRRphysBase1'),
        @(0x00000203, 'MTRRphysMask1'),
        @(0x00000204, 'MTRRphysBase2'),
        @(0x00000205, 'MTRRphysMask2'),
        @(0x00000206, 'MTRRphysBase3'),
        @(0x00000207, 'MTRRphysMask3'),
        @(0x00000208, 'MTRRphysBase4'),
        @(0x00000209, 'MTRRphysMask4'),
        @(0x0000020a, 'MTRRphysBase5'),
        @(0x0000020b, 'MTRRphysMask5'),
        @(0x0000020c, 'MTRRphysBase6'),
        @(0x0000020d, 'MTRRphysMask6'),
        @(0x0000020e, 'MTRRphysBase7'),
        @(0x0000020f, 'MTRRphysMask7'),
        @(0x00000250, 'MTRRfix64K_00000'),
        @(0x00000258, 'MTRRfix16K_80000'),
        @(0x00000259, 'MTRRfix16K_A0000'),
        @(0x00000268, 'MTRRfix4K_00000'),
        @(0x00000269, 'MTRRfix4K_10000'),
        @(0x0000026a, 'MTRRfix4K_20000'),
        @(0x0000026b, 'MTRRfix4K_30000'),
        @(0x0000026c, 'MTRRfix4K_40000'),
        @(0x0000026d, 'MTRRfix4K_50000'),
        @(0x0000026e, 'MTRRfix4K_60000'),
        @(0x0000026f, 'MTRRfix4K_70000'),
        @(0x00000277, 'PAT'),
        @(0x000002ff, 'MTRRdefType'),
        @(0xc0010010, 'SYS_CFG'),
        @(0xc0010015, 'HWCR'),
        @(0xc0010016, 'IORRBase0'),
        @(0xc0010017, 'IORRMask0'),
        @(0xc0010018, 'IORRBase1'),
        @(0xc0010019, 'IORRMask1'),
        @(0xc001001a, 'TOP_MEM'),
        @(0xc001001d, 'TOM2'),
        @(0xc0010111, 'SMM_BASE'),
        @(0xc0010112, 'SMMAddr'),
        @(0xc0010113, 'SMMMask'),
        @(0xc0010114, 'VM_CR')
    )) {
        # PowerShell parses 0x8......./0xc....... literals as negative Int32;
        # mask back to the unsigned 32-bit MSR address (decimal 4294967295 =
        # 0xFFFFFFFF; the hex literal would parse as Int32 -1).
        $allowlist[[uint32]([int64] $entry[0] -band 4294967295)] = [string] $entry[1]
    }
    return $allowlist
}

function Get-MsrSelectionLoad {
    param(
        [Parameter(Mandatory)] [System.Collections.IList] $Instructions,
        [Parameter(Mandatory)] [int] $RdmsrIndex
    )

    Assert-Condition `
        ($RdmsrIndex -gt 0 -and $RdmsrIndex -lt $Instructions.Count) `
        'An RDMSR site has no preceding instruction to select its MSR.'

    $loadIndex = $RdmsrIndex - 1
    while ($loadIndex -ge 0 -and $Instructions[$loadIndex].Mnemonic -eq 'nop') {
        $loadIndex--
    }

    Assert-Condition `
        ($loadIndex -ge 0) `
        'An RDMSR site is preceded only by NOPs and has no MSR-selection load.'

    $load = $Instructions[$loadIndex]
    Assert-Condition `
        ($load.Mnemonic -eq 'mov' -and
            $load.Operands -match '^ecx\s*,\s*0x([0-9a-fA-F]{1,8})(?:\s*(?:#|;).*)?$') `
        "Every RDMSR site must be immediately preceded, except for explicit NOPs, by 'mov ecx, 0x<reviewed address>'; found '$($load.Assembly)'."

    return [pscustomobject]@{
        LoadIndex = $loadIndex
        MsrAddress = [uint32]::Parse(
            $Matches[1],
            [System.Globalization.NumberStyles]::HexNumber,
            [System.Globalization.CultureInfo]::InvariantCulture
        )
    }
}

function Get-ExistingLeafFile {
    param(
        [Parameter(Mandatory)] [string] $LiteralPath,
        [Parameter(Mandatory)] [string] $Description
    )

    Assert-Condition `
        (Test-Path -LiteralPath $LiteralPath -PathType Leaf) `
        "$Description does not exist or is not a file: '$LiteralPath'."
    $item = Get-Item -Force -LiteralPath $LiteralPath
    Assert-Condition `
        (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -eq 0) `
        "$Description must not be a reparse point: '$LiteralPath'."
    return $item.FullName
}

function Assert-ByteRange {
    param(
        [Parameter(Mandatory)] [byte[]] $Bytes,
        [Parameter(Mandatory)] [long] $Offset,
        [Parameter(Mandatory)] [long] $Length,
        [Parameter(Mandatory)] [string] $Context
    )

    Assert-Condition `
        ($Offset -ge 0 -and $Length -ge 0 -and $Offset -le $Bytes.LongLength -and
            $Length -le ($Bytes.LongLength - $Offset)) `
        "$Context is outside the file (offset $Offset, length $Length, file length $($Bytes.LongLength))."
}

function Get-U16Le {
    param([byte[]] $Bytes, [long] $Offset, [string] $Context)
    Assert-ByteRange -Bytes $Bytes -Offset $Offset -Length 2 -Context $Context
    return [System.BitConverter]::ToUInt16($Bytes, [int] $Offset)
}

function Get-U32Le {
    param([byte[]] $Bytes, [long] $Offset, [string] $Context)
    Assert-ByteRange -Bytes $Bytes -Offset $Offset -Length 4 -Context $Context
    return [System.BitConverter]::ToUInt32($Bytes, [int] $Offset)
}

function Get-U64Le {
    param([byte[]] $Bytes, [long] $Offset, [string] $Context)
    Assert-ByteRange -Bytes $Bytes -Offset $Offset -Length 8 -Context $Context
    return [System.BitConverter]::ToUInt64($Bytes, [int] $Offset)
}

function Test-PowerOfTwo {
    param([long] $Value)
    return ($Value -gt 0 -and (($Value -band ($Value - 1)) -eq 0))
}

function Get-SectionName {
    param([byte[]] $Bytes, [long] $Offset, [string] $ImageDescription)

    Assert-ByteRange -Bytes $Bytes -Offset $Offset -Length 8 -Context "$ImageDescription section name"
    $length = 0
    while ($length -lt 8 -and $Bytes[[int] ($Offset + $length)] -ne 0) {
        $value = $Bytes[[int] ($Offset + $length)]
        Assert-Condition `
            ($value -ge 0x20 -and $value -le 0x7e) `
            "$ImageDescription has a non-ASCII PE section name byte at offset $($Offset + $length)."
        $length++
    }
    Assert-Condition ($length -gt 0) "$ImageDescription has an empty PE section name."
    return [System.Text.Encoding]::ASCII.GetString($Bytes, [int] $Offset, $length)
}

function Assert-BaseRelocations {
    param(
        [byte[]] $Bytes,
        [object[]] $Sections,
        [long] $RelocationRva,
        [long] $RelocationSize,
        [long] $SizeOfImage,
        [string] $ImageDescription
    )

    $relocationEnd = $RelocationRva + $RelocationSize
    Assert-Condition `
        ($relocationEnd -gt $RelocationRva -and $relocationEnd -le $SizeOfImage) `
        "$ImageDescription has an out-of-image base-relocation directory."

    $mappingSections = @()
    foreach ($section in $Sections) {
        $mappedLength = [long] $section.VirtualSize
        if ([long] $section.RawSize -gt $mappedLength) {
            $mappedLength = [long] $section.RawSize
        }
        $sectionEnd = [long] $section.VirtualAddress + $mappedLength
        if ($RelocationRva -ge [long] $section.VirtualAddress -and
            $relocationEnd -le $sectionEnd) {
            $delta = $RelocationRva - [long] $section.VirtualAddress
            if ($delta -le [long] $section.RawSize -and
                $RelocationSize -le ([long] $section.RawSize - $delta)) {
                $mappingSections += $section
            }
        }
    }
    Assert-Condition `
        ($mappingSections.Count -eq 1) `
        "$ImageDescription base-relocation directory must map to exactly one raw-backed PE section; found $($mappingSections.Count)."

    $mapping = $mappingSections[0]
    $directoryOffset = [long] $mapping.RawPointer +
        ($RelocationRva - [long] $mapping.VirtualAddress)
    Assert-ByteRange `
        -Bytes $Bytes `
        -Offset $directoryOffset `
        -Length $RelocationSize `
        -Context "$ImageDescription base-relocation directory"

    $cursor = $directoryOffset
    $directoryEnd = $directoryOffset + $RelocationSize
    $blockCount = 0
    $actualRelocationCount = 0
    while ($cursor -lt $directoryEnd) {
        Assert-Condition `
            (($directoryEnd - $cursor) -ge 8) `
            "$ImageDescription has a truncated base-relocation block header."
        $pageRva = [long] (Get-U32Le $Bytes $cursor "$ImageDescription relocation page RVA")
        $blockSize = [long] (Get-U32Le $Bytes ($cursor + 4) "$ImageDescription relocation block size")
        Assert-Condition `
            ($blockSize -ge 8 -and (($blockSize - 8) % 2) -eq 0) `
            "$ImageDescription has an invalid base-relocation block size $blockSize."
        Assert-Condition `
            ($blockSize -le ($directoryEnd - $cursor)) `
            "$ImageDescription has a truncated base-relocation block."
        Assert-Condition `
            (($pageRva % 0x1000) -eq 0) `
            "$ImageDescription has an unaligned base-relocation page RVA 0x$($pageRva.ToString('x'))."

        $entryOffset = $cursor + 8
        $entryEnd = $cursor + $blockSize
        while ($entryOffset -lt $entryEnd) {
            $entry = [long] (Get-U16Le $Bytes $entryOffset "$ImageDescription relocation entry")
            $type = $entry -shr 12
            $offsetInPage = $entry -band 0x0fff
            if ($type -ne 0) {
                Assert-Condition `
                    ($type -eq 10) `
                    "$ImageDescription contains unsupported AMD64 base-relocation type $type."
                $targetRva = $pageRva + $offsetInPage
                Assert-Condition `
                    ($targetRva -lt $SizeOfImage) `
                    "$ImageDescription contains an out-of-image AMD64 base relocation at RVA 0x$($targetRva.ToString('x'))."
                $actualRelocationCount++
            }
            $entryOffset += 2
        }
        $blockCount++
        $cursor += $blockSize
    }

    Assert-Condition `
        ($cursor -eq $directoryEnd) `
        "$ImageDescription base-relocation blocks do not exactly fill the directory."
    Assert-Condition `
        ($blockCount -gt 0 -and $actualRelocationCount -gt 0) `
        "$ImageDescription has no non-padding AMD64 base relocations."

    return [pscustomobject]@{
        BlockCount = $blockCount
        RelocationCount = $actualRelocationCount
    }
}

function Read-PeImage {
    param(
        [Parameter(Mandatory)] [string] $LiteralPath,
        [Parameter(Mandatory)] [string] $ImageDescription,
        [Parameter(Mandatory)] [int] $ExpectedSubsystem,
        [Parameter(Mandatory)] [string] $ExpectedSubsystemName
    )

    $path = Get-ExistingLeafFile -LiteralPath $LiteralPath -Description $ImageDescription
    $fileInfo = Get-Item -Force -LiteralPath $path
    Assert-Condition `
        ($fileInfo.Length -ge 64 -and $fileInfo.Length -le 268435456) `
        "$ImageDescription has an unreasonable file length $($fileInfo.Length); expected 64 bytes through 256 MiB."
    $bytes = [System.IO.File]::ReadAllBytes($path)
    Assert-Condition ([System.BitConverter]::IsLittleEndian) 'This checker requires a little-endian host.'

    Assert-Condition `
        ($bytes[0] -eq 0x4d -and $bytes[1] -eq 0x5a) `
        "$ImageDescription does not have an MZ DOS header."
    $peOffset = [long] (Get-U32Le $bytes 0x3c "$ImageDescription PE header offset")
    Assert-Condition `
        ($peOffset -ge 64) `
        "$ImageDescription has an invalid PE header offset $peOffset."
    Assert-ByteRange -Bytes $bytes -Offset $peOffset -Length 24 -Context "$ImageDescription PE/COFF header"
    Assert-Condition `
        ((Get-U32Le $bytes $peOffset "$ImageDescription PE signature") -eq 0x00004550) `
        "$ImageDescription does not have a PE signature."

    $coffOffset = $peOffset + 4
    $machine = Get-U16Le $bytes $coffOffset "$ImageDescription COFF machine"
    Assert-Condition `
        ($machine -eq 0x8664) `
        "$ImageDescription is not an AMD64 PE image (machine 0x$($machine.ToString('x4')))."
    $sectionCount = [int] (Get-U16Le $bytes ($coffOffset + 2) "$ImageDescription COFF section count")
    Assert-Condition `
        ($sectionCount -ge 1 -and $sectionCount -le 96) `
        "$ImageDescription has an invalid PE section count $sectionCount."
    $optionalSize = [long] (Get-U16Le $bytes ($coffOffset + 16) "$ImageDescription optional-header size")
    $characteristics = Get-U16Le $bytes ($coffOffset + 18) "$ImageDescription COFF characteristics"
    Assert-Condition `
        (($characteristics -band 0x0002) -ne 0) `
        "$ImageDescription is not marked as an executable image."
    Assert-Condition `
        (($characteristics -band 0x0001) -eq 0) `
        "$ImageDescription is marked as having stripped base relocations."

    $optionalOffset = $peOffset + 24
    Assert-Condition `
        ($optionalSize -ge 160) `
        "$ImageDescription PE32+ optional header is too short for the base-relocation directory."
    Assert-ByteRange `
        -Bytes $bytes `
        -Offset $optionalOffset `
        -Length $optionalSize `
        -Context "$ImageDescription PE32+ optional header"
    $magic = Get-U16Le $bytes $optionalOffset "$ImageDescription optional-header magic"
    Assert-Condition `
        ($magic -eq 0x020b) `
        "$ImageDescription is not PE32+ (optional-header magic 0x$($magic.ToString('x4')))."

    $sizeOfCode = [long] (Get-U32Le $bytes ($optionalOffset + 4) "$ImageDescription size of code")
    $entryPoint = [long] (Get-U32Le $bytes ($optionalOffset + 16) "$ImageDescription entry point")
    $imageBase = Get-U64Le $bytes ($optionalOffset + 24) "$ImageDescription image base"
    $sectionAlignment = [long] (Get-U32Le $bytes ($optionalOffset + 32) "$ImageDescription section alignment")
    $fileAlignment = [long] (Get-U32Le $bytes ($optionalOffset + 36) "$ImageDescription file alignment")
    $sizeOfImage = [long] (Get-U32Le $bytes ($optionalOffset + 56) "$ImageDescription size of image")
    $sizeOfHeaders = [long] (Get-U32Le $bytes ($optionalOffset + 60) "$ImageDescription size of headers")
    $subsystem = Get-U16Le $bytes ($optionalOffset + 68) "$ImageDescription subsystem"
    Assert-Condition `
        ($subsystem -eq $ExpectedSubsystem) `
        "$ImageDescription has PE subsystem $subsystem; expected $ExpectedSubsystemName ($ExpectedSubsystem)."
    Assert-Condition ($sizeOfCode -gt 0) "$ImageDescription has an empty code-size field."
    Assert-Condition ($entryPoint -gt 0) "$ImageDescription has a zero entry-point RVA."
    Assert-Condition `
        (Test-PowerOfTwo $fileAlignment) `
        "$ImageDescription has a non-power-of-two file alignment $fileAlignment."
    Assert-Condition `
        ($fileAlignment -ge 512 -and $fileAlignment -le 65536) `
        "$ImageDescription file alignment $fileAlignment is outside the PE range 512 through 65536."
    Assert-Condition `
        (Test-PowerOfTwo $sectionAlignment) `
        "$ImageDescription has a non-power-of-two section alignment $sectionAlignment."
    Assert-Condition `
        ($sectionAlignment -ge $fileAlignment) `
        "$ImageDescription section alignment is smaller than its file alignment."
    Assert-Condition `
        ($sizeOfImage -gt 0 -and ($sizeOfImage % $sectionAlignment) -eq 0) `
        "$ImageDescription has an invalid or unaligned SizeOfImage $sizeOfImage."
    Assert-Condition `
        ($sizeOfHeaders -gt 0 -and $sizeOfHeaders -le $bytes.LongLength -and
            ($sizeOfHeaders % $fileAlignment) -eq 0) `
        "$ImageDescription has an invalid or unaligned SizeOfHeaders $sizeOfHeaders."
    Assert-Condition `
        ($entryPoint -lt $sizeOfImage) `
        "$ImageDescription entry-point RVA is outside SizeOfImage."

    $directoryCount = [long] (Get-U32Le $bytes ($optionalOffset + 108) "$ImageDescription data-directory count")
    $availableDirectoryCount = [Math]::Floor(($optionalSize - 112) / 8)
    Assert-Condition `
        ($directoryCount -ge 6 -and $directoryCount -le $availableDirectoryCount) `
        "$ImageDescription has an invalid data-directory count $directoryCount for optional-header size $optionalSize."
    $relocationDirectoryOffset = $optionalOffset + 112 + (5 * 8)
    $relocationRva = [long] (Get-U32Le $bytes $relocationDirectoryOffset "$ImageDescription base-relocation RVA")
    $relocationSize = [long] (Get-U32Le $bytes ($relocationDirectoryOffset + 4) "$ImageDescription base-relocation size")
    Assert-Condition `
        ($relocationRva -ne 0 -and $relocationSize -ne 0) `
        "$ImageDescription has an empty base-relocation directory."

    $sectionTableOffset = $optionalOffset + $optionalSize
    $sectionTableLength = [long] $sectionCount * 40
    Assert-ByteRange `
        -Bytes $bytes `
        -Offset $sectionTableOffset `
        -Length $sectionTableLength `
        -Context "$ImageDescription PE section table"
    Assert-Condition `
        (($sectionTableOffset + $sectionTableLength) -le $sizeOfHeaders) `
        "$ImageDescription PE section table extends past SizeOfHeaders."

    $sections = @()
    for ($index = 0; $index -lt $sectionCount; $index++) {
        $offset = $sectionTableOffset + ([long] $index * 40)
        $name = Get-SectionName $bytes $offset $ImageDescription
        $virtualSize = [long] (Get-U32Le $bytes ($offset + 8) "$ImageDescription section '$name' virtual size")
        $virtualAddress = [long] (Get-U32Le $bytes ($offset + 12) "$ImageDescription section '$name' RVA")
        $rawSize = [long] (Get-U32Le $bytes ($offset + 16) "$ImageDescription section '$name' raw size")
        $rawPointer = [long] (Get-U32Le $bytes ($offset + 20) "$ImageDescription section '$name' raw pointer")
        $sectionCharacteristics = Get-U32Le $bytes ($offset + 36) "$ImageDescription section '$name' characteristics"

        Assert-Condition `
            (($virtualAddress % $sectionAlignment) -eq 0) `
            "$ImageDescription section '$name' has an unaligned RVA."
        $mappedLength = $virtualSize
        if ($rawSize -gt $mappedLength) {
            $mappedLength = $rawSize
        }
        Assert-Condition `
            ($mappedLength -gt 0 -and ($virtualAddress + $mappedLength) -le $sizeOfImage) `
            "$ImageDescription section '$name' has an empty or out-of-image virtual range."
        if ($rawSize -gt 0) {
            Assert-Condition `
                (($rawPointer % $fileAlignment) -eq 0 -and ($rawSize % $fileAlignment) -eq 0) `
                "$ImageDescription section '$name' has unaligned raw data."
            Assert-Condition `
                ($rawPointer -ge $sizeOfHeaders) `
                "$ImageDescription section '$name' raw data overlaps the PE headers."
            Assert-ByteRange `
                -Bytes $bytes `
                -Offset $rawPointer `
                -Length $rawSize `
                -Context "$ImageDescription section '$name' raw data"
        }

        $sections += [pscustomobject]@{
            Name = $name
            VirtualSize = $virtualSize
            VirtualAddress = $virtualAddress
            RawSize = $rawSize
            RawPointer = $rawPointer
            Characteristics = [long] $sectionCharacteristics
            IsCode = (($sectionCharacteristics -band 0x00000020) -ne 0)
            IsExecutable = (($sectionCharacteristics -band 0x20000000) -ne 0)
        }
    }

    for ($leftIndex = 0; $leftIndex -lt $sections.Count; $leftIndex++) {
        $left = $sections[$leftIndex]
        $leftVirtualLength = [long] $left.VirtualSize
        if ([long] $left.RawSize -gt $leftVirtualLength) {
            $leftVirtualLength = [long] $left.RawSize
        }
        $leftVirtualEnd = [long] $left.VirtualAddress + $leftVirtualLength
        $leftRawEnd = [long] $left.RawPointer + [long] $left.RawSize
        for ($rightIndex = $leftIndex + 1; $rightIndex -lt $sections.Count; $rightIndex++) {
            $right = $sections[$rightIndex]
            $rightVirtualLength = [long] $right.VirtualSize
            if ([long] $right.RawSize -gt $rightVirtualLength) {
                $rightVirtualLength = [long] $right.RawSize
            }
            $rightVirtualEnd = [long] $right.VirtualAddress + $rightVirtualLength
            $virtualOverlap = (
                [long] $left.VirtualAddress -lt $rightVirtualEnd -and
                [long] $right.VirtualAddress -lt $leftVirtualEnd
            )
            Assert-Condition `
                (-not $virtualOverlap) `
                "$ImageDescription sections '$($left.Name)' and '$($right.Name)' overlap in virtual address space."

            if ([long] $left.RawSize -gt 0 -and [long] $right.RawSize -gt 0) {
                $rightRawEnd = [long] $right.RawPointer + [long] $right.RawSize
                $rawOverlap = (
                    [long] $left.RawPointer -lt $rightRawEnd -and
                    [long] $right.RawPointer -lt $leftRawEnd
                )
                Assert-Condition `
                    (-not $rawOverlap) `
                    "$ImageDescription sections '$($left.Name)' and '$($right.Name)' overlap in the file."
            }
        }
    }

    $textSections = @($sections | Where-Object { $_.Name -ceq '.text' })
    Assert-Condition `
        ($textSections.Count -eq 1) `
        "$ImageDescription must contain exactly one .text section; found $($textSections.Count)."
    $textSection = $textSections[0]
    Assert-Condition `
        ([long] $textSection.RawSize -gt 0 -and $textSection.IsCode -and $textSection.IsExecutable) `
        "$ImageDescription .text section must be non-empty, code, and executable."

    $entryPointSections = @()
    foreach ($section in $sections) {
        $mappedLength = [long] $section.VirtualSize
        if ([long] $section.RawSize -gt $mappedLength) {
            $mappedLength = [long] $section.RawSize
        }
        if ($entryPoint -ge [long] $section.VirtualAddress -and
            $entryPoint -lt ([long] $section.VirtualAddress + $mappedLength)) {
            $entryPointSections += $section
        }
    }
    Assert-Condition `
        ($entryPointSections.Count -eq 1 -and $entryPointSections[0].IsCode -and
            $entryPointSections[0].IsExecutable) `
        "$ImageDescription entry point does not map uniquely to an executable code section."

    $relocationInfo = Assert-BaseRelocations `
        -Bytes $bytes `
        -Sections $sections `
        -RelocationRva $relocationRva `
        -RelocationSize $relocationSize `
        -SizeOfImage $sizeOfImage `
        -ImageDescription $ImageDescription

    return [pscustomobject]@{
        Path = $path
        Bytes = $bytes
        ImageBase = $imageBase
        SizeOfImage = $sizeOfImage
        Subsystem = $subsystem
        SubsystemName = $ExpectedSubsystemName
        SectionCount = $sectionCount
        TextSection = $textSection
        RelocationBlockCount = $relocationInfo.BlockCount
        RelocationCount = $relocationInfo.RelocationCount
    }
}

function Resolve-LlvmObjdump {
    param([string] $ExplicitPath)

    if (-not [string]::IsNullOrWhiteSpace($ExplicitPath)) {
        return Get-ExistingLeafFile `
            -LiteralPath $ExplicitPath `
            -Description 'Explicit llvm-objdump executable'
    }

    $command = Get-Command -Name 'llvm-objdump.exe' -CommandType Application -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if ($null -ne $command) {
        return $command.Source
    }

    $candidates = @()
    $programFiles = [Environment]::GetFolderPath([Environment+SpecialFolder]::ProgramFiles)
    if (-not [string]::IsNullOrWhiteSpace($programFiles)) {
        $candidates += (Join-Path $programFiles 'LLVM\bin\llvm-objdump.exe')
    }
    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return Get-ExistingLeafFile `
                -LiteralPath $candidate `
                -Description 'llvm-objdump executable'
        }
    }
    throw 'Could not locate llvm-objdump. Install LLVM, add llvm-objdump.exe to PATH, or pass -LlvmObjdumpPath.'
}

function Get-ProbeInstructions {
    param(
        [Parameter(Mandatory)] [string] $ObjdumpPath,
        [Parameter(Mandatory)] [string] $ProbePath
    )

    $versionOutput = @(& $ObjdumpPath '--version' 2>&1)
    $versionExitCode = $LASTEXITCODE
    Assert-Condition `
        ($versionExitCode -eq 0) `
        "llvm-objdump --version failed with exit code ${versionExitCode}: $($versionOutput -join ' ')"
    $versionText = $versionOutput -join "`n"
    Assert-Condition `
        ($versionText -match '(?im)\bLLVM version\b') `
        "The selected executable does not identify itself as llvm-objdump: '$ObjdumpPath'."

    # Intel syntax is notation only. The parsed image and policy remain AMD64/AMD.
    $output = @(& $ObjdumpPath `
        '--disassemble' `
        '--section=.text' `
        '--x86-asm-syntax=intel' `
        '--no-show-raw-insn' `
        $ProbePath 2>&1)
    $exitCode = $LASTEXITCODE
    Assert-Condition `
        ($exitCode -eq 0) `
        "llvm-objdump failed with exit code $exitCode while disassembling probe .text: $($output -join ' ')"

    $prefixes = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::OrdinalIgnoreCase
    )
    foreach ($prefix in @('lock', 'rep', 'repe', 'repz', 'repne', 'repnz', 'bnd', 'notrack', 'data16', 'addr16', 'rex64')) {
        [void] $prefixes.Add($prefix)
    }

    $instructions = @()
    $lineNumber = 0
    $pendingPrefix = $null
    foreach ($outputLine in $output) {
        $lineNumber++
        $line = [string] $outputLine
        if ($line -notmatch '^\s*(?<Address>[0-9A-Fa-f]+):\s+(?<Assembly>\S.*)$') {
            continue
        }

        $addressText = $Matches['Address']
        $assembly = $Matches['Assembly'].Trim()
        Assert-Condition `
            ($assembly -notmatch '^<unknown>') `
            "llvm-objdump could not decode an instruction at 0x$addressText."

        $prefixList = @()
        $remaining = $assembly
        while ($true) {
            $parts = $remaining -split '\s+', 2
            $token = $parts[0].ToLowerInvariant()
            if (-not $prefixes.Contains($token)) {
                break
            }
            $prefixList += $token
            if ($parts.Count -eq 1) {
                $remaining = ''
                break
            }
            $remaining = $parts[1].Trim()
        }

        # llvm-objdump may render an x86 prefix (e.g. f0 lock) on its own line,
        # split from the instruction it belongs to. The prefix belongs to the
        # following line's instruction; merge them so each parsed instruction
        # reflects what the CPU actually decodes. A dangling prefix fails.
        if ([string]::IsNullOrWhiteSpace($remaining)) {
            Assert-Condition `
                ($null -eq $pendingPrefix) `
                "llvm-objdump emitted consecutive prefix-only lines before 0x$addressText."
            Assert-Condition `
                ($prefixList.Count -gt 0) `
                "llvm-objdump emitted an instruction prefix without a mnemonic at 0x$addressText."
            $pendingPrefix = [pscustomobject]@{
                Address = [Convert]::ToUInt64($addressText, 16)
                AddressText = $addressText.ToLowerInvariant()
                Prefixes = $prefixList
                Assembly = $assembly
            }
            continue
        }

        $instructionParts = $remaining -split '\s+', 2
        $mnemonic = $instructionParts[0].ToLowerInvariant()
        Assert-Condition `
            ($mnemonic -match '^[a-z][a-z0-9_.]*$') `
            "llvm-objdump emitted an unparseable instruction mnemonic '$mnemonic' at 0x$addressText."
        $operands = ''
        if ($instructionParts.Count -eq 2) {
            $operands = $instructionParts[1].Trim().ToLowerInvariant()
        }

        $instructionAddress = [Convert]::ToUInt64($addressText, 16)
        $instructionAddressText = $addressText.ToLowerInvariant()
        if ($null -ne $pendingPrefix) {
            $instructionAddress = $pendingPrefix.Address
            $instructionAddressText = $pendingPrefix.AddressText
            $prefixList = @($pendingPrefix.Prefixes) + $prefixList
            $assembly = "$($pendingPrefix.Assembly) $assembly"
            $pendingPrefix = $null
        }

        $instructions += [pscustomobject]@{
            Address = $instructionAddress
            AddressText = $instructionAddressText
            Prefixes = $prefixList
            Mnemonic = $mnemonic
            Operands = $operands
            Assembly = $assembly
            OutputLine = $lineNumber
        }
    }

    Assert-Condition `
        ($null -eq $pendingPrefix) `
        'llvm-objdump emitted a trailing instruction prefix with no following instruction.'

    Assert-Condition `
        ($instructions.Count -gt 0) `
        'llvm-objdump produced no parsed instructions for the probe .text section.'
    return [pscustomobject]@{
        Instructions = $instructions
        VersionLine = [string] ($versionOutput | Where-Object { [string] $_ -match 'LLVM version' } | Select-Object -First 1)
    }
}

function Assert-ProbeInstructionSafety {
    param([Parameter(Mandatory)] [object[]] $Instructions)

    $forbidden = @{
        'wrmsr' = 'MSR write'
        'wrmsrns' = 'non-serializing MSR write'
        'vmrun' = 'AMD SVM guest entry'
        'vmload' = 'AMD SVM state load'
        'vmsave' = 'AMD SVM state save'
        'vmmcall' = 'AMD SVM hypervisor call'
        'stgi' = 'AMD SVM global-interrupt enable'
        'clgi' = 'AMD SVM global-interrupt disable'
        'skinit' = 'AMD secure-loader state change'
        'invlpga' = 'AMD guest TLB invalidation'
        'invlpgb' = 'AMD broadcast TLB invalidation'
        'tlbsync' = 'AMD broadcast TLB synchronization'
        'pvalidate' = 'AMD SEV-SNP page-validation change'
        'rmpadjust' = 'AMD SEV-SNP reverse-map change'
        'rmpupdate' = 'AMD SEV-SNP reverse-map change'
        'psmash' = 'AMD SEV-SNP page-state change'
        'in' = 'port input'
        'inb' = 'port input'
        'inw' = 'port input'
        'ind' = 'port input'
        'ins' = 'string port input'
        'insb' = 'string port input'
        'insw' = 'string port input'
        'insd' = 'string port input'
        'out' = 'port output'
        'outb' = 'port output'
        'outw' = 'port output'
        'outd' = 'port output'
        'outs' = 'string port output'
        'outsb' = 'string port output'
        'outsw' = 'string port output'
        'outsd' = 'string port output'
        'cli' = 'interrupt-disable state change'
        'sti' = 'interrupt-enable state change'
        'clac' = 'SMAP access-control state change'
        'stac' = 'SMAP access-control state change'
        'lgdt' = 'descriptor-table load'
        'lidt' = 'descriptor-table load'
        'lldt' = 'descriptor-table load'
        'ltr' = 'task-register load'
        'lmsw' = 'control-register state change'
        'smsw' = 'control-register access'
        'clts' = 'control-register state change'
        'xsetbv' = 'extended-control-register state change'
        'invlpg' = 'TLB invalidation'
        'invpcid' = 'PCID/TLB invalidation'
        'wbinvd' = 'cache write-back and invalidation'
        'wbnoinvd' = 'cache write-back'
        'invd' = 'cache invalidation'
        'swapgs' = 'GS-base state change'
        'wrfsbase' = 'FS-base state change'
        'wrgsbase' = 'GS-base state change'
        'monitor' = 'monitored-address state change'
        'monitorx' = 'AMD monitored-address state change'
        'mwait' = 'monitor wait-state entry'
        'mwaitx' = 'AMD monitor wait-state entry'
        'rsm' = 'system-management-mode return'
        'syscall' = 'privilege-level transfer'
        'sysenter' = 'privilege-level transfer'
        'sysexit' = 'privilege-level transfer'
        'sysret' = 'privilege-level transfer'
        'sysretq' = 'privilege-level transfer'
        'iret' = 'interrupt/privilege-state return'
        'iretd' = 'interrupt/privilege-state return'
        'iretq' = 'interrupt/privilege-state return'
        'lcall' = 'far control transfer'
        'ljmp' = 'far control transfer'
        'xrstors' = 'supervisor extended-state restore'
        'xrstors64' = 'supervisor extended-state restore'
        'wrpkru' = 'protection-key rights state change'
        'pconfig' = 'platform-configuration state change'
    }

    $violations = @()
    foreach ($instruction in $Instructions) {
        if ($forbidden.ContainsKey($instruction.Mnemonic)) {
            $violations += "0x$($instruction.AddressText): $($instruction.Assembly) [$($forbidden[$instruction.Mnemonic])]"
        }
        if ($instruction.Mnemonic -eq 'mov' -and
            $instruction.Operands -match '(?i)(?:^|[^a-z0-9_])cr[0-9]+(?:[^a-z0-9_]|$)') {
            $violations += "0x$($instruction.AddressText): $($instruction.Assembly) [control-register access]"
        }
        if ($instruction.Mnemonic -eq 'mov' -and
            $instruction.Operands -match '(?i)(?:^|[^a-z0-9_])dr[0-9]+(?:[^a-z0-9_]|$)') {
            $violations += "0x$($instruction.AddressText): $($instruction.Assembly) [debug-register access]"
        }
    }
    Assert-Condition `
        ($violations.Count -eq 0) `
        "Probe .text contains forbidden privileged/state-changing instructions:`n  $($violations -join "`n  ")"

    $reviewedSites = Get-ReviewedMsrSiteAllowlist
    $rdmsrIndexes = @()
    for ($index = 0; $index -lt $Instructions.Count; $index++) {
        if ($Instructions[$index].Mnemonic -eq 'rdmsr') {
            $rdmsrIndexes += $index
        }
    }
    Assert-Condition `
        ($rdmsrIndexes.Count -eq $reviewedSites.Count) `
        "Probe .text must contain exactly $($reviewedSites.Count) reviewed read-only RDMSR sites; found $($rdmsrIndexes.Count)."

    $observedAddresses = @{}
    foreach ($rdmsrIndex in $rdmsrIndexes) {
        $selection = Get-MsrSelectionLoad -Instructions $Instructions -RdmsrIndex $rdmsrIndex
        $address = $selection.MsrAddress
        Assert-Condition `
            ($reviewedSites.Contains($address)) `
            ("RDMSR site at 0x{0} selects unreviewed MSR address 0x{1:X8}." -f $Instructions[$rdmsrIndex].AddressText, $address)
        Assert-Condition `
            (-not $observedAddresses.ContainsKey($address)) `
            ("Probe .text contains a duplicate RDMSR site for MSR 0x{0:X8} ({1})." -f $address, $reviewedSites[$address])
        $observedAddresses[$address] = $true
    }
    foreach ($address in $reviewedSites.Keys) {
        Assert-Condition `
            ($observedAddresses.ContainsKey($address)) `
            ("Reviewed RDMSR site for MSR 0x{0:X8} ({1}) is missing from the probe image." -f $address, $reviewedSites[$address])
    }

    return [pscustomobject]@{
        RdmsrSiteCount = $rdmsrIndexes.Count
        HltCount = @($Instructions | Where-Object { $_.Mnemonic -eq 'hlt' }).Count
    }
}

$probe = Read-PeImage `
    -LiteralPath $ProbeEfiPath `
    -ImageDescription 'Probe EFI' `
    -ExpectedSubsystem 10 `
    -ExpectedSubsystemName 'EFI_APPLICATION'
$dxe = Read-PeImage `
    -LiteralPath $DxeEfiPath `
    -ImageDescription 'DXE EFI' `
    -ExpectedSubsystem 11 `
    -ExpectedSubsystemName 'EFI_BOOT_SERVICE_DRIVER'
$objdump = Resolve-LlvmObjdump -ExplicitPath $LlvmObjdumpPath
$disassembly = Get-ProbeInstructions -ObjdumpPath $objdump -ProbePath $probe.Path
$instructionSafety = Assert-ProbeInstructionSafety -Instructions $disassembly.Instructions

Write-Output 'PASS: AMD-platform EFI safety checks passed.'
Write-Output "  Probe: AMD64 PE32+ $($probe.SubsystemName), $($probe.SectionCount) sections, $($probe.RelocationCount) relocations."
Write-Output "  DXE: AMD64 PE32+ $($dxe.SubsystemName), $($dxe.SectionCount) sections, $($dxe.RelocationCount) relocations."
Write-Output "  Probe .text: $($disassembly.Instructions.Count) parsed instructions; $($instructionSafety.RdmsrSiteCount) reviewed read-only RDMSR sites (AMD VM_CR plus the 41 system-register sites of docs/m0b-msr-read-policy.md), each pinned to its literal MSR address; HLT count $($instructionSafety.HltCount)."
Write-Output "  Disassembler: $($disassembly.VersionLine.Trim())"
Write-Output '  Note: Intel syntax is assembly notation only; no Intel-platform assumption is made.'
