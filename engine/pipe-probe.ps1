# 探测：哪些码序列产生 >=2 个整句候选（重排可介入）
$ErrorActionPreference = 'Stop'
function Send-Hufu([object]$obj) {
  $pipe = New-Object System.IO.Pipes.NamedPipeClientStream('.', 'hufu-ime', [System.IO.Pipes.PipeDirection]::InOut)
  $pipe.Connect(3000)
  $json = [System.Text.Encoding]::UTF8.GetBytes(($obj | ConvertTo-Json -Compress -Depth 6))
  $len = [System.BitConverter]::GetBytes([Int32]$json.Length)
  $pipe.Write($len, 0, 4); $pipe.Write($json, 0, $json.Length); $pipe.Flush()
  $hdr = New-Object byte[] 4; $read = 0
  while ($read -lt 4) { $read += $pipe.Read($hdr, $read, 4 - $read) }
  $n = [System.BitConverter]::ToInt32($hdr, 0)
  $buf = New-Object byte[] $n; $read = 0
  while ($read -lt $n) { $read += $pipe.Read($buf, $read, $n - $read) }
  $pipe.Close()
  [System.Text.Encoding]::UTF8.GetString($buf) | ConvertFrom-Json
}
function Key([string]$k) { @{ op = 'key'; key = $k } }

$probes = @('nqbh', 'mlwetm', 'dskang', 'wqity', 'bwjdsk')
foreach ($code in $probes) {
  $null = Send-Hufu @{ op = 'reset' }
  $r = $null
  foreach ($ch in $code.ToCharArray()) { $r = Send-Hufu (Key ([string]$ch)) }
  if ($null -eq $r) { continue }
  $st = $r.state
  $cands = @($st.candidates | Select-Object -First 6 | ForEach-Object { $_.text })
  $kinds = @($st.candidates | Select-Object -First 6 | ForEach-Object { $_.source })
  Write-Host "[$code] raw=$($st.raw) cands=[$($cands -join ' ')] kinds=[$($kinds -join ',')] committed=$($st.committed_text)"
}
exit 0
