[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$cache = Join-Path $root 'target/synthetic-tools'
$downloads = Join-Path $cache 'downloads'
New-Item -ItemType Directory -Path $downloads -Force | Out-Null

function Get-VerifiedDownload([string]$Url, [string]$Name, [string]$Algorithm, [string]$Digest) {
    $path = Join-Path $downloads $Name
    if (-not (Test-Path -LiteralPath $path)) {
        Invoke-WebRequest -Uri $Url -OutFile $path
    }
    if ((Get-FileHash -LiteralPath $path -Algorithm $Algorithm).Hash -ne $Digest) {
        throw "Checksum mismatch: $path. Remove this cached download before retrying."
    }
    return $path
}

# 7-Zip hashes pin the downloaded official release artifacts; QEMU's SHA-512
# is also published at the same distributor URL with the .sha512 suffix.
$sevenStub = Get-VerifiedDownload 'https://github.com/ip7z/7zip/releases/download/26.03/7zr.exe' '7zr.exe' SHA256 'AD4C82FADCBDF93C03B4FC440F300509C7D60C5C2F4D183E35D9D70D6957037D'
$sevenArchive = Get-VerifiedDownload 'https://github.com/ip7z/7zip/releases/download/26.03/7z2603-x64.exe' '7z2603-x64.exe' SHA256 '0859C524B8A63551848F0C246ABDDCB1D0B7B656B0FBFE879F8D85E61A9E6EDD'
$qemuArchive = Get-VerifiedDownload 'https://qemu.weilnetz.de/w64/2025/qemu-w64-setup-20250826.exe' 'qemu-w64-setup-20250826.exe' SHA512 '5e6b88318ab1233e6d9cea187f657a8d69c5007fbaaee18c57383abbadf080c2731ccdcfe5c12b021b344b285b4bf2d69f6c80124714a2ac7d5f6a46b7297ccc'
$sevenDir = Join-Path $cache '7zip'
& $sevenStub x $sevenArchive "-o$sevenDir" -y | Out-Null
if ($LASTEXITCODE -ne 0) { throw '7-Zip extraction failed.' }
$qemuDir = Join-Path $cache 'qemu-10.1.0'
& (Join-Path $sevenDir '7z.exe') x $qemuArchive "-o$qemuDir" -y | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'QEMU extraction failed.' }
$qemu = Join-Path $qemuDir 'qemu-system-x86_64.exe'
if (-not (Test-Path -LiteralPath $qemu)) { throw 'QEMU executable missing after extraction.' }
& $qemu --version
if ($LASTEXITCODE -ne 0) { throw 'QEMU version check failed.' }
Write-Output "Portable QEMU: $qemu"
