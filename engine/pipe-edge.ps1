# HuFu 边界电池：TigerClaw 语义对齐后的边界行为
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
  [pscustomobject]@{ key=$k; raw=$r.state.raw; cand=(($r.state.candidates | ForEach-Object { $_.text }) -join ' '); commit=$r.outcome.commit; consumed=$r.outcome.consumed; mode=$r.state.mode }
}
function Reset { [void](PipeCall '{"op":"reset"}') }
$pass=0; $fail=0
function Check([string]$n, [bool]$c, [string]$d) {
  if ($c) { $script:pass++; Write-Host "  PASS $n  ($d)" -ForegroundColor Green }
  else { $script:fail++; Write-Host "  FAIL $n  ($d)" -ForegroundColor Red }
}

Write-Host '══ 直通类（raw 空时）'
Reset; $d = SK '5'
Check '数字直通' ($d.consumed -eq $false) "consumed=$($d.consumed)"
Reset; $u = SK 'A'
Check '大写直通(混输关)' ($u.consumed -eq $false) "consumed=$($u.consumed)"

Write-Host '══ 编码态功能键'
Reset; [void](SK 'a'); $bk = SK 'backspace'
Check '退格删码' ($bk.raw -eq '') "raw='$($bk.raw)'"
Reset; [void](SK 'a'); [void](SK 'j'); $esc = SK 'escape'
Check 'ESC清屏' ($esc.raw -eq '' -and $esc.consumed) "raw='$($esc.raw)'"
Reset; [void](SK 'a'); $tb = SK 'tab'
Check 'TAB清屏' ($tb.raw -eq '') "raw='$($tb.raw)' consumed=$($tb.consumed)"
Reset; $en = SK 'enter'
Check '空raw回车直通' ($en.consumed -eq $false) "consumed=$($en.consumed)"

Write-Host '══ 死码缓冲（空码不清屏=否）'
Reset; foreach ($k in 'c','s','p','s') { $s = SK $k }
Check 'csps 死码保持' ($s.raw -eq 'csps') "raw='$($s.raw)'"
$bk2 = SK 'backspace'
Check '死码退格' ($bk2.raw -eq 'csp') "raw='$($bk2.raw)'"

Write-Host '══ 符号命名空间'
Reset; $sl = SK '/'
Check '/首选顿号' ($sl.cand.Length -gt 0) "cand='$($sl.cand)'"
Reset; $q = SK ';'
Check ';快符进码' ($q.raw -eq ';' -and $q.consumed) "raw='$($q.raw)'"

Write-Host '══ 引号配对'
Reset; $q1 = SK "\u0027"
Check '单引号→' ($q1.commit.Length -gt 0) "'$($q1.commit)'"
Reset; $q2 = SK "\u0022"
Check '双引号→' ($q2.commit.Length -gt 0) "'$($q2.commit)'"

Write-Host '══ 反查退出'
Reset; [void](SK "``"); $ex = SK 'escape'
Check 'ESC退出反查' ($ex.mode -eq 'Normal') "mode=$($ex.mode)"
Reset; [void](SK "``"); [void](SK 'n'); [void](SK 'i'); $sp2 = SK 'space'
Check '反查空格首选' ($sp2.commit -eq '你') "'$($sp2.commit)'"

Write-Host '══ 中英切换'
Reset; $sh = SK 'shift'
Check 'Shift切换中英' ($sh.consumed) "consumed=$($sh.consumed)"
$en2 = SK 'a'
Check '英文态字母直通' ($en2.consumed -eq $false) "consumed=$($en2.consumed)"
$sh2 = SK 'shift'
$ch2 = SK 'a'
Check '切回中文态' ($ch2.consumed -and $ch2.raw -eq 'a') "raw='$($ch2.raw)'"

Write-Host ''
Write-Host ("结果: {0} PASS / {1} FAIL" -f $pass, $fail)
if ($fail -gt 0) { exit 1 }
