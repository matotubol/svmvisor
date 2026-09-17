[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $ImagePath,
    [Parameter(Mandatory)] [string] $ManifestPath,
    [switch] $Recovery
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$image = Get-Item -LiteralPath $ImagePath
if ($image.PSIsContainer -or $image.Extension -ne '.bin' -or $image.Length -le 0 -or $image.Length -gt 33554432) {
    throw 'Expected a non-empty .bin image no larger than the 32 MiB flash.'
}
$manifest = Get-Content -LiteralPath $ManifestPath -Raw | ConvertFrom-Json
if ($manifest.schema_version -ne 1 -or $manifest.target_part -cne 'xc7a35tfgg484-2') {
    throw 'Unsupported manifest schema or target part.'
}
if ($manifest.image_sha256 -cnotmatch '^[0-9a-f]{64}$' -or $manifest.image_size_bytes -ne $image.Length) {
    throw 'Invalid image digest or size in manifest.'
}
$digest = (Get-FileHash -LiteralPath $image.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
if ($digest -cne $manifest.image_sha256) { throw 'Image SHA-256 does not match manifest.' }
if ($Recovery) {
    if ($manifest.image_kind -cne 'non_enumerating_recovery' -or $manifest.non_enumerating_netlist_policy -cne 'PASS') {
        throw 'Recovery requires a passing non-enumerating netlist policy.'
    }
    # Separate reviewed pin; a newly built manifest alone cannot select recovery.
    $pin = Import-PowerShellDataFile -LiteralPath (Join-Path $PSScriptRoot 'recovery-pin.psd1')
    $manifestDigest = (Get-FileHash -LiteralPath $ManifestPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($pin.ImageSha256 -cne $digest -or $pin.ManifestSha256 -cne $manifestDigest) {
        throw 'Recovery image/manifest is not the pinned candidate.'
    }
} elseif ($manifest.image_kind -cne 'completion_only' -or $manifest.completion_only_netlist_policy -cne 'PASS') {
    throw 'Experimental flashing requires completion_only_netlist_policy = PASS.'
}
[pscustomobject] @{ ImagePath = $image.FullName; ImageSha256 = $digest; ImageSizeBytes = $image.Length; ImageKind = $manifest.image_kind }
