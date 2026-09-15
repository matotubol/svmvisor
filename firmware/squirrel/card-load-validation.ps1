Set-StrictMode -Version Latest
function Assert-CardLoadFile([string]$Path, [long]$Bytes, [string]$Sha256) {
    $file = Get-Item -LiteralPath $Path -ErrorAction Stop
    if ($file.PSIsContainer -or ($Bytes -ge 0 -and $file.Length -ne $Bytes)) { throw "Invalid file extent: $Path" }
    if ((Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() -cne $Sha256) { throw "Digest mismatch: $Path" }
}
function Assert-CardLoadBackups([string]$First, [string]$Second) {
    foreach ($path in @($First,$Second)) {
        if ((Get-Item -LiteralPath $path -ErrorAction Stop).Length -ne 5242880) { throw "Backup must contain exactly5MiB: $path" }
    }
    $hash = (Get-FileHash -LiteralPath $First).Hash.ToLowerInvariant()
    Assert-CardLoadFile $Second 5242880 $hash
    return $hash
}
function ConvertTo-CardLoadTclPath([string]$Path) {
    if ($Path -match '[{}"\r\n]') { throw 'Unsupported Tcl path characters.' }
    return $Path.Replace('\','/')
}
function Assert-CardLoadAction([string]$Action, [bool]$Confirmed) {
    if ($Action -notin @('CheckOnly','Program')) { throw 'Unknown action.' }
    if ($Action -eq 'Program' -and -not $Confirmed) { throw 'Program requires explicit -ConfirmFlash after separate user authorization.' }
}
