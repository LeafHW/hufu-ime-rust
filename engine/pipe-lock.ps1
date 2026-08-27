# 整句「写入编码选重」真码表验证 v2（动态期望 + HTTP 切方案）
$enc = [System.Text.Encoding]::UTF8
function PipeCall([string]$json) {
  $pipe = [System.IO.Pipes.NamedPipeClientStream]::new('.', 'hufu-ime', [System.IO.Pipes.PipeDirection]::InOut)
  $pipe.Connect(3000)
  $body = $enc.GetBytes($json)
  $pipe.Write([BitConverter]::GetBytes([UInt32]$body.Length), 0, 4)
  $pipe.Write($body, 0, $body.Length); $pipe.Flush()
  $head = New-Object byte[] 4; [void]$pipe.Read($head, 0, 4)
  $n = [BitConverter]::ToUInt32($head, 0); $buf = New-Object byte[] $n
  $read = 0; while ($read -lt $n) { $read += $pipe.Read($buf, $read, $n - $read) }
  $pipe.Close(); ($enc.GetString($buf) | ConvertFrom-Json)
}
function TypeCh([char[]]$chs) {
  foreach ($ch in $chs) { $script:last = PipeCall ('{"op":"key","key":"' + $ch + '"}') }
}
$pass = 0; $fail = 0
function Check([string]$name, [bool]$cond, [string]$detail) {
  if ($cond) { $script:pass++; Write-Host "  [PASS] $name" }
  else { $script:fail++; Write-Host "  [FAIL] $name  $detail" }
}

# 确保在 虎整句
$cfg = (Invoke-RestMethod http://127.0.0.1:4390/api/config)
if ($cfg.schema.current -ne '虎整句') {
  $cfg.schema.current = '虎整句'
  $body = $cfg | ConvertTo-Json -Depth 10
  Invoke-RestMethod -Method Post -Uri http://127.0.0.1:4390/api/config -Body $body -ContentType 'application/json; charset=utf-8' | Out-Null
  Start-Sleep -Milliseconds 500
}

# ── 1) jd + 2：锁候选框第 2 名，不上屏
[void](PipeCall '{"op":"reset"}')
TypeCh @('j','d')
$disp = @($script:last.state.candidates | ForEach-Object { $_.text })
$want2 = $disp[1]
TypeCh @('2')
$r = $script:last
Check 'jd+2 不上屏' ($null -eq $r.outcome.commit) "commit=$($r.outcome.commit)"
Check 'jd+2 raw 含锁' ($r.state.raw -eq 'jd2') "raw=$($r.state.raw)"
Check "jd+2 锁到候选框第2名($want2)" ($r.state.candidates[0].text -eq $want2) "首选=$($r.state.candidates[0].text) 框序: $($disp -join ' ')"

# ── 2) 续打：jd2 + tu → 锁保留组句；空格上屏
TypeCh @('t','u')
$r = $script:last
Check 'jd2tu 续打 raw' ($r.state.raw -eq 'jd2tu') "raw=$($r.state.raw)"
$head2 = $r.state.candidates[0].text
Check "jd2tu 组句以$want2 开头" ($head2.StartsWith($want2)) "首选=$head2"
TypeCh @(' ')
$r = $script:last
Check '空格上屏整句' ($r.outcome.commit -eq $head2) "commit=$($r.outcome.commit)"

# ── 3) 分号锁第 2 / 引号锁第 3（同基准）
[void](PipeCall '{"op":"reset"}')
TypeCh @('j','d',';')
$r = $script:last
Check 'jd; 锁2=候选框第2' ($null -eq $r.outcome.commit -and $r.state.candidates[0].text -eq $want2) "commit=$($r.outcome.commit) 首选=$($r.state.candidates[0].text)"
[void](PipeCall '{"op":"reset"}')
TypeCh @('j','d')
$want3 = @($script:last.state.candidates)[2].text
TypeCh @("'")
$r = $script:last
Check "jd' 锁3=候选框第3($want3)" ($null -eq $r.outcome.commit -and $r.state.candidates[0].text -eq $want3) "首选=$($r.state.candidates[0].text)"

# ── 4) 整句流不断：tujatuja 提前上屏
[void](PipeCall '{"op":"reset"}')
TypeCh @('t','u','j','a','t','u','j','a')
$r = $script:last
Check 'tujatuja 组句在' ($r.state.candidates.Count -gt 0) "候选数=$($r.state.candidates.Count)"
TypeCh @(' ')
$r = $script:last
Check 'tujatuja+空格上屏' ($null -ne $r.outcome.commit -and $r.outcome.commit.Length -ge 3) "commit=$($r.outcome.commit)"

# ── 5) 非整句回归：HTTP 切 虎码单字 → 数字选重立即上屏 → 切回
[void](PipeCall '{"op":"reset"}')
$cfg = (Invoke-RestMethod http://127.0.0.1:4390/api/config)
$cfg.schema.current = '虎码单字'
$body = $cfg | ConvertTo-Json -Depth 10
Invoke-RestMethod -Method Post -Uri http://127.0.0.1:4390/api/config -Body $body -ContentType 'application/json; charset=utf-8' | Out-Null
Start-Sleep -Milliseconds 600
TypeCh @('j','d')
$disp2 = @($script:last.state.candidates | ForEach-Object { $_.text })
TypeCh @('2')
$r = $script:last
Check "非整句 jd+2 立即上屏($($disp2[1]))" ($r.outcome.commit -eq $disp2[1]) "commit=$($r.outcome.commit)"
# 切回 虎整句
$cfg = (Invoke-RestMethod http://127.0.0.1:4390/api/config)
$cfg.schema.current = '虎整句'
$body = $cfg | ConvertTo-Json -Depth 10
Invoke-RestMethod -Method Post -Uri http://127.0.0.1:4390/api/config -Body $body -ContentType 'application/json; charset=utf-8' | Out-Null

Write-Host ""
Write-Host "结果: $pass PASS / $fail FAIL"
exit 0
