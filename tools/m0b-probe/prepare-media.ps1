<#
.SYNOPSIS
Prepares an existing removable volume for one svmvisor M0b probe run.

.DESCRIPTION
Validates an immutable five-file M0a target-profile bundle with both the
current repository verifier and the verifier copied into that bundle. It then
copies the explicitly supplied probe EFI to EFI\BOOT\BOOTX64.EFI and writes the
exact root binding marker consumed by the probe. Existing destination files are
never overwritten. This script never formats media.

.PARAMETER TargetProfileBundle
Existing five-file M0a target-profile evidence bundle.

.PARAMETER ProbeEfiPath
Existing svmvisor M0b probe EFI image to copy.

.PARAMETER RemovableMediaRoot
Existing filesystem root of the removable volume, for example E:\.
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string] $TargetProfileBundle,

    [Parameter(Mandatory)]
    [string] $ProbeEfiPath,

    [Parameter(Mandatory)]
    [string] $RemovableMediaRoot
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

function Get-ExistingDirectory {
    param(
        [Parameter(Mandatory)] [string] $LiteralPath,
        [Parameter(Mandatory)] [string] $Description
    )

    $item = Get-Item -Force -LiteralPath $LiteralPath
    Assert-Condition ([bool] $item.PSIsContainer) "$Description is not a directory: '$LiteralPath'."
    Assert-Condition `
        (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -eq 0) `
        "$Description must not be a reparse point: '$LiteralPath'."
    return $item.FullName
}

function Get-ExistingLeafFile {
    param(
        [Parameter(Mandatory)] [string] $LiteralPath,
        [Parameter(Mandatory)] [string] $Description
    )

    $item = Get-Item -Force -LiteralPath $LiteralPath
    Assert-Condition (-not [bool] $item.PSIsContainer) "$Description is not a file: '$LiteralPath'."
    Assert-Condition `
        (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -eq 0) `
        "$Description must not be a reparse point: '$LiteralPath'."
    return $item.FullName
}

function Assert-ProbeEfiApplication {
    param([Parameter(Mandatory)] [string] $LiteralPath)

    $bytes = [System.IO.File]::ReadAllBytes($LiteralPath)
    Assert-Condition ($bytes.Length -ge 64) 'Probe EFI is too short to contain a DOS/PE header.'
    Assert-Condition `
        ($bytes[0] -eq 0x4d -and $bytes[1] -eq 0x5a) `
        'Probe EFI does not have an MZ header.'
    $peOffset = [long] [System.BitConverter]::ToUInt32($bytes, 0x3c)
    Assert-Condition `
        ($peOffset -ge 64 -and $peOffset -le ($bytes.Length - 24)) `
        'Probe EFI has an out-of-range PE header offset.'
    Assert-Condition `
        ([System.BitConverter]::ToUInt32($bytes, [int] $peOffset) -eq 0x00004550) `
        'Probe EFI does not have a PE signature.'
    Assert-Condition `
        ([System.BitConverter]::ToUInt16($bytes, [int] ($peOffset + 4)) -eq 0x8664) `
        'Probe EFI is not an AMD64 PE image.'
    Assert-Condition `
        ([System.BitConverter]::ToUInt16($bytes, [int] ($peOffset + 6)) -gt 0) `
        'Probe EFI has no PE sections.'
    $characteristics = [System.BitConverter]::ToUInt16($bytes, [int] ($peOffset + 22))
    Assert-Condition `
        (($characteristics -band 0x0002) -ne 0) `
        'Probe EFI is not marked as an executable image.'

    $optionalSize = [long] [System.BitConverter]::ToUInt16($bytes, [int] ($peOffset + 20))
    $optionalOffset = $peOffset + 24
    Assert-Condition `
        ($optionalSize -ge 160 -and ($optionalOffset + $optionalSize) -le $bytes.Length) `
        'Probe EFI has a truncated PE32+ optional header.'
    Assert-Condition `
        ([System.BitConverter]::ToUInt16($bytes, [int] $optionalOffset) -eq 0x020b) `
        'Probe EFI is not PE32+.'
    Assert-Condition `
        ([System.BitConverter]::ToUInt16($bytes, [int] ($optionalOffset + 68)) -eq 10) `
        'Probe EFI is not an EFI_APPLICATION image.'
    $directoryCount = [System.BitConverter]::ToUInt32($bytes, [int] ($optionalOffset + 108))
    Assert-Condition ($directoryCount -ge 6) 'Probe EFI has no base-relocation directory slot.'
    $relocationOffset = $optionalOffset + 112 + (5 * 8)
    $relocationRva = [System.BitConverter]::ToUInt32($bytes, [int] $relocationOffset)
    $relocationSize = [System.BitConverter]::ToUInt32($bytes, [int] ($relocationOffset + 4))
    Assert-Condition `
        ($relocationRva -ne 0 -and $relocationSize -ne 0) `
        'Probe EFI has no non-empty base-relocation directory.'
}

function Get-ContainedPath {
    param(
        [Parameter(Mandatory)] [string] $Root,
        [Parameter(Mandatory)] [string] $RelativePath
    )

    Assert-Condition `
        (-not [System.IO.Path]::IsPathRooted($RelativePath)) `
        "A media-relative path unexpectedly became rooted: '$RelativePath'."
    $candidate = [System.IO.Path]::GetFullPath((Join-Path $Root $RelativePath))
    $rootPrefix = $Root.TrimEnd(
        [System.IO.Path]::DirectorySeparatorChar,
        [System.IO.Path]::AltDirectorySeparatorChar
    ) + [System.IO.Path]::DirectorySeparatorChar
    Assert-Condition `
        ($candidate.StartsWith($rootPrefix, [System.StringComparison]::OrdinalIgnoreCase)) `
        "Media path escaped the explicitly supplied root: '$candidate'."
    return $candidate
}

function Assert-ExactM0aFileSet {
    param([Parameter(Mandatory)] [string] $Bundle)

    $expected = @(
        'collect-windows.ps1',
        'manifest.json',
        'target-profile-v1.schema.json',
        'target-profile.json',
        'verify-bundle.ps1'
    )
    $expectedSet = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($name in $expected) {
        [void] $expectedSet.Add($name)
    }

    $items = @(Get-ChildItem -Force -LiteralPath $Bundle)
    Assert-Condition ($items.Count -eq $expected.Count) 'The M0a bundle must contain exactly five files.'
    foreach ($item in $items) {
        Assert-Condition (-not [bool] $item.PSIsContainer) "The M0a bundle contains a directory: '$($item.Name)'."
        Assert-Condition `
            (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -eq 0) `
            "The M0a bundle contains a reparse point: '$($item.Name)'."
        Assert-Condition ($expectedSet.Contains($item.Name)) "Unexpected M0a bundle file: '$($item.Name)'."
    }
}

function Invoke-M0aVerification {
    param([Parameter(Mandatory)] [string] $Bundle)

    Assert-ExactM0aFileSet -Bundle $Bundle
    $toolsRoot = Split-Path -Parent $PSScriptRoot
    $currentVerifier = Get-ExistingLeafFile `
        -LiteralPath (Join-Path $toolsRoot 'target-profile\verify-bundle.ps1') `
        -Description 'Current repository M0a verifier'
    $copiedVerifier = Get-ExistingLeafFile `
        -LiteralPath (Join-Path $Bundle 'verify-bundle.ps1') `
        -Description 'Copied M0a verifier'

    & $currentVerifier -BundleDirectory $Bundle
    & $copiedVerifier -BundleDirectory $Bundle
}

$m0aBundle = Get-ExistingDirectory `
    -LiteralPath $TargetProfileBundle `
    -Description 'M0a target-profile bundle'
$probeEfi = Get-ExistingLeafFile `
    -LiteralPath $ProbeEfiPath `
    -Description 'Probe EFI'
Assert-Condition `
    ([System.StringComparer]::OrdinalIgnoreCase.Equals([System.IO.Path]::GetExtension($probeEfi), '.efi')) `
    "ProbeEfiPath must identify an .efi file: '$probeEfi'."
Assert-ProbeEfiApplication -LiteralPath $probeEfi

Invoke-M0aVerification -Bundle $m0aBundle

$mediaRoot = Get-ExistingDirectory `
    -LiteralPath $RemovableMediaRoot `
    -Description 'Removable-media root'
$pathRoot = [System.IO.Path]::GetPathRoot($mediaRoot)
Assert-Condition `
    ([System.StringComparer]::OrdinalIgnoreCase.Equals(
        $mediaRoot.TrimEnd('\'),
        $pathRoot.TrimEnd('\')
    )) `
    "RemovableMediaRoot must be a filesystem root, not a subdirectory: '$mediaRoot'."
$drive = New-Object System.IO.DriveInfo($pathRoot)
Assert-Condition ($drive.IsReady) "The removable-media volume is not ready: '$mediaRoot'."
Assert-Condition `
    ($drive.DriveType -eq [System.IO.DriveType]::Removable) `
    "The supplied volume is not reported as removable media: '$mediaRoot'."

$efiDirectory = Get-ContainedPath -Root $mediaRoot -RelativePath 'EFI'
$bootDirectory = Get-ContainedPath -Root $mediaRoot -RelativePath 'EFI\BOOT'
$destinationEfi = Get-ContainedPath -Root $mediaRoot -RelativePath 'EFI\BOOT\BOOTX64.EFI'
$markerPath = Get-ContainedPath -Root $mediaRoot -RelativePath 'svmvisor-m0b-target.txt'

foreach ($directory in @($efiDirectory, $bootDirectory)) {
    if (Test-Path -LiteralPath $directory) {
        [void] (Get-ExistingDirectory -LiteralPath $directory -Description 'Existing media directory')
    }
}
Assert-Condition `
    (-not (Test-Path -LiteralPath $destinationEfi)) `
    "Refusing to overwrite an existing boot image: '$destinationEfi'."
Assert-Condition `
    (-not (Test-Path -LiteralPath $markerPath)) `
    "Refusing to overwrite an existing profile binding marker: '$markerPath'."

$m0aManifestPath = Get-ExistingLeafFile `
    -LiteralPath (Join-Path $m0aBundle 'manifest.json') `
    -Description 'M0a manifest'
$m0aManifestSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $m0aManifestPath).Hash.ToLowerInvariant()
Assert-Condition `
    ($m0aManifestSha256 -cmatch '^[0-9a-f]{64}$') `
    'The computed M0a manifest hash is not canonical lowercase SHA-256.'
$markerText = "svmvisor-m0b-target-v1`n$m0aManifestSha256`n"
$markerBytes = [System.Text.Encoding]::ASCII.GetBytes($markerText)
Assert-Condition ($markerBytes.Length -eq 88) 'Internal error: the binding marker is not exactly 88 bytes.'

$efiWasCreated = $false
$markerWasCreated = $false
try {
    if (-not (Test-Path -LiteralPath $efiDirectory)) {
        [void] [System.IO.Directory]::CreateDirectory($efiDirectory)
    }
    if (-not (Test-Path -LiteralPath $bootDirectory)) {
        [void] [System.IO.Directory]::CreateDirectory($bootDirectory)
    }
    [void] (Get-ExistingDirectory -LiteralPath $efiDirectory -Description 'Media EFI directory')
    [void] (Get-ExistingDirectory -LiteralPath $bootDirectory -Description 'Media boot directory')

    [System.IO.File]::Copy($probeEfi, $destinationEfi, $false)
    $efiWasCreated = $true

    $stream = New-Object System.IO.FileStream(
        $markerPath,
        [System.IO.FileMode]::CreateNew,
        [System.IO.FileAccess]::Write,
        [System.IO.FileShare]::None
    )
    try {
        $stream.Write($markerBytes, 0, $markerBytes.Length)
        $stream.Flush()
    }
    finally {
        $stream.Dispose()
    }
    $markerWasCreated = $true

    $copiedEfiHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $destinationEfi).Hash.ToLowerInvariant()
    $sourceEfiHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $probeEfi).Hash.ToLowerInvariant()
    Assert-Condition `
        ([System.StringComparer]::Ordinal.Equals($copiedEfiHash, $sourceEfiHash)) `
        'The copied EFI hash does not match the supplied probe EFI.'
    $writtenMarker = [System.IO.File]::ReadAllBytes($markerPath)
    Assert-Condition `
        ([System.StringComparer]::Ordinal.Equals(
            [System.Convert]::ToBase64String($markerBytes),
            [System.Convert]::ToBase64String($writtenMarker)
        )) `
        'The written binding marker bytes are not canonical.'
}
catch {
    if ($markerWasCreated -and (Test-Path -LiteralPath $markerPath -PathType Leaf)) {
        [System.IO.File]::Delete($markerPath)
    }
    if ($efiWasCreated -and (Test-Path -LiteralPath $destinationEfi -PathType Leaf)) {
        [System.IO.File]::Delete($destinationEfi)
    }
    throw
}

Write-Host "M0b removable media preparation: PASS ($mediaRoot)"
Write-Host "M0a manifest SHA-256: $m0aManifestSha256"
Write-Host "Probe EFI SHA-256: $sourceEfiHash"
Write-Host "Boot image: $destinationEfi"
Write-Host "Binding marker: $markerPath"
