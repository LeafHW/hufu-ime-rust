# 重排端到端 v2：bwjdsk（2 个整句候选）→ 等待 → 验证 worker 完成与应用
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

$null = Send-Hufu @{ op = 'reset' }
foreach ($ch in 'b','w','j','d','s','k') { $r = Send-Hufu (Key ([string]$ch)) }
$before = @($r.state.candidates | ForEach-Object { $_.text })
Write-Host "bwjdsk 候选: [$($before -join ' ')] raw=$($r.state.raw)"

Write-Host '等待重排...'
Start-Sleep 6
# down 查看顺序（apply 在 process_key 入口）
$r2 = Send-Hufu (Key 'down')
$after = @($r2.state.candidates | ForEach-Object { $_.text })
Write-Host "重排后顺序: [$($after -join ' ')]（原 [$($before -join ' ')]）"
$null = Send-Hufu @{ op = 'reset' }
exit 0
