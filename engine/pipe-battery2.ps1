# HuFu 管道回归电池 v3（与 TigerClaw 语义对齐）
# 断言语义：
#   - csps 非码表编码：4 码死路留在缓冲区，第 5 键进整句解码器（虎整句方案）
#   - 整句「提前上屏」默认开：缓冲可能中途轮转（属设计）
#   - 反查用真实小鹤音节（de/ni）
$enc = [System.Text.Encoding]::UTF8
function PipeCall([string]$json) {
  $pipe = [System.IO.Pipes.NamedPipeClientStream]::new('.', 'hufu-ime', [System.IO.Pipes.PipeDirection]::InOut)
  $pipe.Connect(2000)
  $body = $enc.GetBytes($json)
  $pipe.Write([BitConverter]::GetBytes([UInt32]$body.Length), 0, 4)
  $pipe.Write($body, 0, $body.Length); $pipe.Flush()
  $head = New-Object byte[] 4; [void]$pipe.Read($head, 0, 4)
  $n = [BitConverter]::ToUInt32($head, 0); $buf = New-Object byte[] $n
  $read = 0; while ($read -lt $n) { $read += $pipe.Read($buf, $read, $n - $read) }
  $pipe.Close(); ($enc.GetString($buf) | ConvertFrom-Json)
}
function SK([string]$k) {
  $r = PipeCall ('{"op":"key","key":"' + $k + '"}')
  [pscustomobject]@{
    key = $k; raw = $r.state.raw
    cand = (($r.state.candidates | ForEach-Object { $_.text }) -join ' ')
    commit = $r.outcome.commit; page = $r.state.page; pc = $r.state.page_count
    mode = $r.state.mode
  }
}
function Reset { [void](PipeCall '{"op":"reset"}') }
$pass = 0; $fail = 0
function Check([string]$n, [bool]$c, [string]$d) {
  if ($c) { $script:pass++; Write-Host "  PASS $n  ($d)" -ForegroundColor Green }
  else { $script:fail++; Write-Host "  FAIL $n  ($d)" -ForegroundColor Red }
}

Write-Host '══ 1 标点（开头/编码后）'
Reset; $c1 = SK ','; $c2 = SK '.'
Check '开头逗号→，' ($c1.commit -eq '，') "'$($c1.commit)'"
Check '开头句号→。' ($c2.commit -eq '。') "'$($c2.commit)'"
Reset; [void](SK 'a'); $ap = SK ','
Check 'a+逗号→来，' ($ap.commit -eq '来，') "'$($ap.commit)'"

Write-Host '══ 2 反查（`+小鹤双拼，数字选重）'
Reset; $t1 = SK '`'; [void](SK 'n'); $ni = SK 'i'; $sel = SK '1'
Check '进入反查' ($t1.mode -eq 'Reverse') $t1.mode
Check 'ni 出字' ($ni.cand -match '你') "'$($ni.cand)'"
Check '选1→你' ($sel.commit -eq '你') "'$($sel.commit)'"

Write-Host '══ 3 整句（tujatuja）'
Reset; $last = $null
foreach ($k in 't','u','j','a','t','u','j','a') { $last = SK $k }
$sp = SK 'space'
Check '候选含我们' ($last.cand -match '我们') "'$($last.cand)'"
Check '空格上屏' ($sp.commit.Length -gt 0) "'$($sp.commit)'"

Write-Host '══ 4 选重稳定（调频关，5 查同序）'
$order = $null; $stable = $true
for ($i = 0; $i -lt 5; $i++) { Reset; $r = SK 'jd'; if ($null -eq $order) { $order = $r.cand } elseif ($r.cand -ne $order) { $stable = $false } }
Check 'jd 同序' $stable "'$order'"

Write-Host '══ 5 csps（死码缓冲→解码器）'
Reset; $s1 = $null
foreach ($k in 'c','s','p','s') { $s1 = SK $k }
Check 'csps 缓冲保留' ($s1.raw -eq 'csps') "'$($s1.raw)'"
$n5 = SK 'a'
Check '第5键进解码器' ($n5.cand.Length -gt 0) "'$($n5.cand)'"

Write-Host '══ 6 选重键与翻页'
Reset; [void](SK 'jd'); $sel2 = SK '2'; Check '数字2选' ($sel2.commit.Length -gt 0) "'$($sel2.commit)'"
Reset; [void](SK 'jd'); $semi = SK ';'; Check '分号次选' ($semi.commit.Length -gt 0) "'$($semi.commit)'"
Reset; foreach ($k in 't','u','j','a','t','u') { $last = SK $k }
$pg = SK '='; Check '解码器态翻页' ($pg.pc -ge 2 -and $pg.page -eq 1) "page=$($pg.page)/$($pg.pc)"
$bk = SK '-'; Check '减号回页' ($bk.page -eq 0) "page=$($bk.page)"

Write-Host ''
Write-Host "结果: $pass PASS / $fail FAIL" -ForegroundColor $(if ($fail -eq 0) { 'Green' } else { 'Yellow' })
if ($fail -gt 0) { exit 1 }
