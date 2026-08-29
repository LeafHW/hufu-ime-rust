# HuFu pipe protocol test (ASCII only; PS5.1-safe)
$ErrorActionPreference = 'Stop'
try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch {}

function Invoke-Pipe([string]$json) {
    $pipe = New-Object System.IO.Pipes.NamedPipeClientStream('.', 'hufu-ime', [System.IO.Pipes.PipeDirection]::InOut)
    $pipe.Connect(3000)
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($json)
    $len = [System.BitConverter]::GetBytes([UInt32]$bytes.Length)
    $pipe.Write($len, 0, 4)
    $pipe.Write($bytes, 0, $bytes.Length)
    $pipe.Flush()
    $lb = [byte[]]::new(4)
    $off = 0
    while ($off -lt 4) { $off += $pipe.Read($lb, $off, 4 - $off) }
    $n = [System.BitConverter]::ToUInt32($lb, 0)
    $rb = [byte[]]::new($n)
    $off = 0
    while ($off -lt $n) { $off += $pipe.Read($rb, $off, $n - $off) }
    $pipe.Close()
    [System.Text.Encoding]::UTF8.GetString($rb)
}

Write-Host '[1] ping:'
Write-Host ('  ' + (Invoke-Pipe '{"op":"ping"}'))

Write-Host '[2] reset:'
[void](Invoke-Pipe '{"op":"reset"}')

Write-Host '[3] per-key ueeyiahx over pipe:'
foreach ($ch in @('u','e','e','y','i','a','h','x')) {
    $r = Invoke-Pipe ('{"op":"key","key":"' + $ch + '"}')
    $o = $r | ConvertFrom-Json
    $cs = @(); foreach ($c in @($o.state.candidates | Select-Object -First 3)) { $cs += $c.text }
    Write-Host ('  key=' + $ch + ' raw=[' + $o.state.raw + '] cands=[' + ($cs -join ' ') + ']')
}

Write-Host '[4] state:'
$s = Invoke-Pipe '{"op":"state"}' | ConvertFrom-Json
Write-Host ('  schema=' + $s.current_schema + ' sentence_active=' + $s.sentence_active)

Write-Host '[5] focus clear:'
[void](Invoke-Pipe '{"op":"focus"}')
$s2 = Invoke-Pipe '{"op":"state"}' | ConvertFrom-Json
Write-Host ('  raw after focus=[' + $s2.state.raw + ']')
