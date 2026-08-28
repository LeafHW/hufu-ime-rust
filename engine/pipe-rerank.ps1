# 重排端到端测试：管道模拟输入 → 停顿（重排完成）→ 空格提交对比
$ErrorActionPreference = 'Stop'

function Send-Hufu([object]$obj) {
  $pipe = New-Object System.IO.Pipes.NamedPipeClientStream('.', 'hufu-ime', [System.IO.Pipes.PipeDirection]::InOut)
  $pipe.Connect(3000)
  $json = [System.Text.Encoding]::UTF8.GetBytes(($obj | ConvertTo-Json -Compress -Depth 6))
  $len = [System.BitConverter]::GetBytes([Int32]$json.Length)
  $pipe.Write($len, 0, 4)
  $pipe.Write($json, 0, $json.Length)
  $pipe.Flush()
  $hdr = New-Object byte[] 4
  $read = 0
  while ($read -lt 4) { $read += $pipe.Read($hdr, $read, 4 - $read) }
  $n = [System.BitConverter]::ToInt32($hdr, 0)
  $buf = New-Object byte[] $n
  $read = 0
  while ($read -lt $n) { $read += $pipe.Read($buf, $read, $n - $read) }
  $pipe.Close()
  [System.Text.Encoding]::UTF8.GetString($buf) | ConvertFrom-Json
}

function Key([string]$k) { @{ op = 'key'; key = $k } }

Write-Host "== 重排端到端 =="
$null = Send-Hufu @{ op = 'reset' }

# 6 码整句：bwj=打 di=字 uvj=没 jm=有 → "打字没有"？随意一个 >4 码组合
# 用已验证的：mlwe=两次 + tm=他们 → mlwetm
$keys = 'm','l','w','e','t','m'
foreach ($k in $keys) {
  $r = Send-Hufu (Key $k)
}
$st = $r.state
$before = @($st.candidates | Select-Object -First 5 | ForEach-Object { $_.text })
Write-Host "输入 mlwetm 前5候选: [$($before -join ' ')]  raw=$($st.raw)"

Write-Host "等待重排（首含模型加载 7s + 去抖 + 打分）..."
Start-Sleep 14

# 空格提交（apply_rerank 在 process_key 入口应用）
$r2 = Send-Hufu (Key 'space')
$committed = $r2.outcome.commit
Write-Host "空格提交: [$committed]  （重排前首选是 [$($before[0])]）"
if ($committed -and $committed -ne $before[0]) {
  Write-Host "✓ 重排生效：提交 [$committed] ≠ 原首选 [$($before[0])]"
} else {
  Write-Host "⚠ 提交=原首选（可能重排未完成或顺序未变）"
}

# 第二轮：不重载模型，验证 ~2s 内出结果
$null = Send-Hufu @{ op = 'reset' }
foreach ($k in 'm','l','w','e','t','m') { $r = Send-Hufu (Key $k) }
$before2 = @($r.state.candidates | Select-Object -First 5 | ForEach-Object { $_.text })
Start-Sleep 4
$r3 = Send-Hufu (Key 'space')
Write-Host "第二轮提交: [$($r3.outcome.commit)] 原[$($before2[0])]"
exit 0
