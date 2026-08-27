# HuFu 管道批量测试：模拟 DLL 逐键发送，断言预期
param([string]$Script = 'basic')
$ErrorActionPreference = 'Stop'
$enc = [System.Text.Encoding]::UTF8

function PipeCall([string]$json) {
  $pipe = [System.IO.Pipes.NamedPipeClientStream]::new('.', 'hufu-ime', [System.IO.Pipes.PipeDirection]::InOut)
  $pipe.Connect(2000)
  $body = $enc.GetBytes($json)
  $pipe.Write([BitConverter]::GetBytes([UInt32]$body.Length), 0, 4)
  $pipe.Write($body, 0, $body.Length)
  $pipe.Flush()
  $head = New-Object byte[] 4
  [void]$pipe.Read($head, 0, 4)
  $n = [BitConverter]::ToUInt32($head, 0)
  $buf = New-Object byte[] $n
  $read = 0
  while ($read -lt $n) { $read += $pipe.Read($buf, $read, $n - $read) }
  $pipe.Close()
  $enc.GetString($buf) | ConvertFrom-Json
}

function SendKey([string]$k) {
  $r = PipeCall ('{"op":"key","key":"' + $k + '"}')
  [pscustomobject]@{
    key    = $k
    raw    = $r.state.raw
    pre    = $r.state.preedit
    cand   = ($r.state.candidates | ForEach-Object { $_.text }) -join ' '
    commit = $r.commit
  }
}

function Reset { [void](PipeCall '{"op":"reset"}') }

# ── 测试组 ──────────────────────────────────────────────
$results = @()
function T([string]$name, [scriptblock]$body) {
  Reset
  try {
    $out = & $body
    $script:results += [pscustomobject]@{ Test = $name; Result = 'RUN'; Detail = ($out | Out-String).Trim() }
  } catch {
    $script:results += [pscustomobject]@{ Test = $name; Result = 'ERR'; Detail = $_.Exception.Message }
  }
}

# 1. 标点：中文态逗号/句号/反引号
T 'punct-comma' { (SendKey ',').cand + ' | commit=' + (SendKey ',').commit }
T 'punct-period' { (SendKey '.').commit }
T 'punct-backtick-first' { (SendKey '`').raw + '|' + (SendKey '`').cand }
# 2. 反查：` 引导后打字母
T 'reverse-lookup' {
  $a = SendKey '`'
  $b = SendKey 'j'
  $c = SendKey 'd'
  "after-tick raw=$($a.raw) cand=$($a.cand); jd raw=$($c.raw) cand=$($c.cand)"
}
# 3. csps 顶功链
T 'csps-auto-commit' {
  $c = ''; $s = ''
  foreach ($k in 'c','s','p','s') { $s += "$k→$((SendKey $k).cand | Select-Object -First 1); " }
  $s
}
# 4. 整句：连续编码
T 'sentence-tujatuja' {
  $out = ''
  foreach ($k in 't','u','j','a') { $r = SendKey $k; $out += ('{0}: [{1}] ' -f $k, $r.preedit) }
  $out
}
# 5. 选重稳定性：同码两次完整查询
T 'ordering-stability' {
  Reset; $a = (SendKey 'jd').cand
  Reset; $b = (SendKey 'jd').cand
  Reset; $c2 = (SendKey 'jd').cand
  "1=$a`n2=$b`n3=$c2"
}
# 6. 数字选重 + 翻页
T 'digit-select' { $r = SendKey 'jd'; $s = SendKey '2'; "cand=[$($r.cand)] sel2-commit=$($s.commit)" }
T 'page' { $r1 = SendKey 'a'; $p = SendKey '='; "a:[$($r1.cand)] page:[$($p.cand)]" }

$results | ForEach-Object {
  Write-Host "── $($_.Test) [$($_.Result)]" -ForegroundColor $(if ($_.Result -eq 'ERR') { 'Red' } else { 'Gray' })
  Write-Host $_.Detail
}
