# Keep this limit compatible with already released hufu-server downloaders.
function Assert-HufuUpdatePackage([string]$Path) {
    $item = Get-Item -LiteralPath $Path -ErrorAction Stop
    if ($item.PSIsContainer -or $item.Length -le 0 -or $item.Length -gt 67108864) {
        throw "The single EXE must be 1..67108864 bytes for existing online updaters: $Path ($($item.Length) bytes)"
    }
}
