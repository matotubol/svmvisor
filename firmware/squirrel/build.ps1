[CmdletBinding()]
param(
    [string] $VivadoRoot,
    [switch] $CheckOnly,
    [switch] $GenerateOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if ($CheckOnly -and $GenerateOnly) {
    throw '-CheckOnly and -GenerateOnly cannot be used together.'
}

$workspaceRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..')).Path
$config = Import-PowerShellDataFile -LiteralPath (Join-Path $PSScriptRoot 'config.psd1')

function Get-NormalizedPciHex {
    param(
        [Parameter(Mandatory)] [object] $Value,
        [Parameter(Mandatory)] [int] $Digits,
        [Parameter(Mandatory)] [string] $Name
    )

    $text = [string] $Value
    if ($text -notmatch "(?i)^0x[0-9a-f]{$Digits}$") {
        throw "$Name must be written as 0x followed by exactly $Digits hexadecimal digits; got '$text'."
    }
    return $text.Substring(2).ToUpperInvariant()
}

$romSizeBytes = [int] $config.Pci.ExpansionRomSizeBytes
if ($romSizeBytes -ne 4096) {
    throw "The Squirrel ROM leaf is fixed at 4096 bytes; config.psd1 specifies $romSizeBytes."
}
$romSizeKilobytes = $romSizeBytes / 1024
$vendorId = Get-NormalizedPciHex -Value $config.Pci.VendorId -Digits 4 -Name 'Pci.VendorId'
$deviceId = Get-NormalizedPciHex -Value $config.Pci.DeviceId -Digits 4 -Name 'Pci.DeviceId'
$classCode = Get-NormalizedPciHex -Value $config.Pci.ClassCode -Digits 6 -Name 'Pci.ClassCode'

if ([string]::IsNullOrWhiteSpace($VivadoRoot)) {
    $VivadoRoot = $config.VivadoRoot
}

$vivadoBat = Join-Path $VivadoRoot 'Vivado\bin\vivado.bat'
if (-not (Test-Path -LiteralPath $vivadoBat)) {
    throw "Vivado was not found at '$vivadoBat'. Pass -VivadoRoot to override it."
}

& (Join-Path $PSScriptRoot 'bootstrap.ps1') -SkipOpenOcd
if ($LASTEXITCODE -ne 0) {
    throw 'Squirrel gateware bootstrap failed.'
}

$upstreamRoot = Join-Path $workspaceRoot $config.Upstream.CheckoutPath
$sourceRoot = Join-Path $upstreamRoot 'PCIeSquirrel'
$buildTcl = Join-Path $PSScriptRoot 'vivado\build.tcl'
$optionRomRtl = Join-Path $PSScriptRoot 'rtl\svmvisor_option_rom.sv'
$completionRtl = Join-Path $PSScriptRoot 'rtl\svmvisor_tlps128_bar_rdengine.sv'

if (-not (Test-Path -LiteralPath (Join-Path $sourceRoot 'vivado_generate_project.tcl'))) {
    throw "The pinned PCIeSquirrel source is incomplete at '$sourceRoot'."
}
foreach ($requiredRtl in @($optionRomRtl, $completionRtl)) {
    if (-not (Test-Path -LiteralPath $requiredRtl)) {
        throw "Required svmvisor Expansion ROM RTL is missing at '$requiredRtl'."
    }
}

Write-Host "Vivado: $vivadoBat"
Write-Host "Squirrel source: $sourceRoot"

if ($CheckOnly) {
    Write-Host 'Squirrel build prerequisites are present.'
    return
}

$romRoot = Join-Path $workspaceRoot 'target\firmware\squirrel\rom'
$romPath = Join-Path $romRoot 'svmvisor-dxe.rom'
$romMemoryPath = Join-Path $romRoot 'svmvisor-dxe.mem'
& (Join-Path $PSScriptRoot 'package-rom.ps1') -OutputPath $romPath -MemoryPath $romMemoryPath
if ($LASTEXITCODE -ne 0) {
    throw 'Squirrel option-ROM packaging failed.'
}

$generatedSourceRoot = Join-Path $workspaceRoot 'target\firmware\squirrel\generated'
$upstreamBarController = Join-Path $sourceRoot 'src\pcileech_tlps128_bar_controller.sv'
$patchedBarController = Join-Path $generatedSourceRoot 'pcileech_tlps128_bar_controller.sv'
$barControllerSource = [System.IO.File]::ReadAllText($upstreamBarController)
$controllerReplacements = [ordered] @{
    'pcileech_tlps128_bar_rdengine i_pcileech_tlps128_bar_rdengine(' = 'svmvisor_tlps128_bar_rdengine i_pcileech_tlps128_bar_rdengine('
    'pcileech_bar_impl_none i_bar6_optrom(' = 'svmvisor_option_rom i_bar6_optrom('
}
$patchedBarControllerSource = $barControllerSource
foreach ($entry in $controllerReplacements.GetEnumerator()) {
    $matchCount = ([regex]::Matches($patchedBarControllerSource, [regex]::Escape($entry.Key))).Count
    if ($matchCount -ne 1) {
        throw "Expected one pinned controller match '$($entry.Key)' in '$upstreamBarController'; found $matchCount."
    }
    $patchedBarControllerSource = $patchedBarControllerSource.Replace($entry.Key, $entry.Value)
}
New-Item -ItemType Directory -Force -Path $generatedSourceRoot | Out-Null
[System.IO.File]::WriteAllText(
    $patchedBarController,
    $patchedBarControllerSource,
    [System.Text.UTF8Encoding]::new($false)
)

$resolvedSourceRoot = (Resolve-Path -LiteralPath $sourceRoot).Path.TrimEnd('\')
$projectDirectory = [System.IO.Path]::GetFullPath((Join-Path $resolvedSourceRoot 'pcileech_squirrel'))
$expectedProjectDirectory = "$resolvedSourceRoot\pcileech_squirrel"
if (-not [string]::Equals($projectDirectory, $expectedProjectDirectory, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to clean unexpected Vivado project path '$projectDirectory'."
}
if (Test-Path -LiteralPath $projectDirectory) {
    Remove-Item -LiteralPath $projectDirectory -Recurse -Force
}

$mode = if ($GenerateOnly) { 'generate' } else { 'build' }
$vivadoSourceRoot = $sourceRoot.Replace('\', '/')
$vivadoBuildTcl = $buildTcl.Replace('\', '/')
$vivadoPatchedBarController = $patchedBarController.Replace('\', '/')
$vivadoCompletionRtl = $completionRtl.Replace('\', '/')
$vivadoOptionRomRtl = $optionRomRtl.Replace('\', '/')
$vivadoRomMemoryPath = $romMemoryPath.Replace('\', '/')
$logRoot = Join-Path $workspaceRoot 'target\firmware\squirrel\logs'
New-Item -ItemType Directory -Force -Path $logRoot | Out-Null
$vivadoLog = Join-Path $logRoot "vivado-$mode.log"
$vivadoJournal = Join-Path $logRoot "vivado-$mode.jou"
foreach ($oldLog in @($vivadoLog, $vivadoJournal)) {
    if (Test-Path -LiteralPath $oldLog) {
        Remove-Item -LiteralPath $oldLog -Force
    }
}

$outputRoot = Join-Path $workspaceRoot 'target\firmware\squirrel'
$outputImage = Join-Path $outputRoot 'svmvisor-squirrel.bin'
$upstreamImage = Join-Path $sourceRoot 'pcileech_squirrel.bin'
New-Item -ItemType Directory -Force -Path $outputRoot | Out-Null
if (-not $GenerateOnly) {
    foreach ($oldImage in @($upstreamImage, $outputImage)) {
        if (Test-Path -LiteralPath $oldImage) {
            Remove-Item -LiteralPath $oldImage -Force
        }
    }
}
$buildStartedUtc = [DateTime]::UtcNow

Push-Location $logRoot
try {
    & $vivadoBat `
        -mode batch `
        -journal $vivadoJournal `
        -log $vivadoLog `
        -source $vivadoBuildTcl `
        -tclargs `
        $vivadoSourceRoot `
        $mode `
        $vivadoPatchedBarController `
        $vivadoCompletionRtl `
        $vivadoOptionRomRtl `
        $vivadoRomMemoryPath `
        $romSizeKilobytes `
        $vendorId `
        $deviceId `
        $classCode

    if ($LASTEXITCODE -ne 0) {
        throw "Vivado failed with exit code $LASTEXITCODE."
    }
}
finally {
    Pop-Location
}

if ($GenerateOnly) {
    Write-Host 'Vivado project generation completed.'
    return
}

if (-not (Test-Path -LiteralPath $upstreamImage)) {
    throw "Vivado completed without producing '$upstreamImage'."
}
$upstreamImageInfo = Get-Item -LiteralPath $upstreamImage
if ($upstreamImageInfo.Length -eq 0 -or $upstreamImageInfo.LastWriteTimeUtc -lt $buildStartedUtc.AddSeconds(-2)) {
    throw "Vivado produced an empty or stale gateware image at '$upstreamImage'."
}

Copy-Item -LiteralPath $upstreamImage -Destination $outputImage -Force

$sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $outputImage).Hash.ToLowerInvariant()
$sourceSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $upstreamImage).Hash.ToLowerInvariant()
if ($sha256 -ne $sourceSha256) {
    throw 'The copied gateware image does not match the Vivado implementation output.'
}
Write-Host "Gateware image: $outputImage"
Write-Host "SHA-256: $sha256"
Write-Host "Expansion ROM: $romPath ($romSizeBytes-byte BAR)"
