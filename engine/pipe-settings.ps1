# 第二轮：设置生效性端到端
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
$pass = 0; $fail = 0
function Check([string]$name, [bool]$ok, [string]$detail) {
  if ($ok) { $script:pass++; Write-Host "PASS $name" } else { $script:fail++; Write-Host "FAIL $name  $detail" }
}

# ── 1. 皮肤改色 → pipe skin op 反映 ──
$skin = Invoke-RestMethod 'http://127.0.0.1:4390/api/skin' -TimeoutSec 5
$origBack = $skin.colors.back_color
$skin.colors.back_color = '#112233FF'
$b = [System.Text.Encoding]::UTF8.GetBytes(($skin | ConvertTo-Json -Depth 10))
Invoke-RestMethod 'http://127.0.0.1:4390/api/skin' -Method Post -Body $b -ContentType 'application/json; charset=utf-8' | Out-Null
$sk = Send-Hufu @{ op = 'skin' }
Check '皮肤改色热反映' ($sk.skin.colors.back_color -eq '#112233FF') "got $($sk.skin.colors.back_color)"
$skin.colors.back_color = $origBack
$b = [System.Text.Encoding]::UTF8.GetBytes(($skin | ConvertTo-Json -Depth 10))
Invoke-RestMethod 'http://127.0.0.1:4390/api/skin' -Method Post -Body $b -ContentType 'application/json; charset=utf-8' | Out-Null

# ── 2. 横排开关 → skin op 反映 ──
$skin = Invoke-RestMethod 'http://127.0.0.1:4390/api/skin' -TimeoutSec 5
$skin.layout.horizontal = $true
$b = [System.Text.Encoding]::UTF8.GetBytes(($skin | ConvertTo-Json -Depth 10))
Invoke-RestMethod 'http://127.0.0.1:4390/api/skin' -Method Post -Body $b -ContentType 'application/json; charset=utf-8' | Out-Null
$sk = Send-Hufu @{ op = 'skin' }
Check '横排开关热反映' ($sk.skin.layout.horizontal -eq $true) "got $($sk.skin.layout.horizontal)"
$skin.layout.horizontal = $false
$b = [System.Text.Encoding]::UTF8.GetBytes(($skin | ConvertTo-Json -Depth 10))
Invoke-RestMethod 'http://127.0.0.1:4390/api/skin' -Method Post -Body $b -ContentType 'application/json; charset=utf-8' | Out-Null

# ── 3. show_index / delay_show_ms → skin op 附带 ──
$cfg = Invoke-RestMethod 'http://127.0.0.1:4390/api/config' -TimeoutSec 5
$cfg.candidates.show_index = $false
$cfg.candidates.delay_show_ms = 250
$b = [System.Text.Encoding]::UTF8.GetBytes(($cfg | ConvertTo-Json -Depth 12))
Invoke-RestMethod 'http://127.0.0.1:4390/api/config' -Method Post -Body $b -ContentType 'application/json; charset=utf-8' | Out-Null
$sk = Send-Hufu @{ op = 'skin' }
Check 'show_index 附带' ($sk.show_index -eq $false) "got $($sk.show_index)"
Check 'delay_show_ms 附带' ($sk.delay_show_ms -eq 250) "got $($sk.delay_show_ms)"
# 还原
$cfg.candidates.show_index = $true
$cfg.candidates.delay_show_ms = 0
$b = [System.Text.Encoding]::UTF8.GetBytes(($cfg | ConvertTo-Json -Depth 12))
Invoke-RestMethod 'http://127.0.0.1:4390/api/config' -Method Post -Body $b -ContentType 'application/json; charset=utf-8' | Out-Null

# ── 4. 音效：立体声 KeyNormal 经 base64 下发 ──
$snd = Send-Hufu @{ op = 'sound'; tag = 'key' }
$bytes = [System.Convert]::FromBase64String($snd.data)
Check '音效 key=KeyNormal 立体声' ($bytes.Length -gt 28000 -and $snd.volume -ge 0) "len=$($bytes.Length)"
$snd2 = Send-Hufu @{ op = 'sound'; tag = 'commit' }
Check '音效 commit=KeySpace' ([System.Convert]::FromBase64String($snd2.data).Length -gt 28000) ''

# ── 5. log_adjust → user-adjust.log ──
$cfg = Invoke-RestMethod 'http://127.0.0.1:4390/api/config' -TimeoutSec 5
$cfg.user.log_adjust = $true
$b = [System.Text.Encoding]::UTF8.GetBytes(($cfg | ConvertTo-Json -Depth 12))
Invoke-RestMethod 'http://127.0.0.1:4390/api/config' -Method Post -Body $b -ContentType 'application/json; charset=utf-8' | Out-Null
$logPath = 'E:\DSH-KF\hufu\hufu-data\user-adjust.log'
if (Test-Path $logPath) { Remove-Item $logPath -Force }
$null = Send-Hufu @{ op = 'reset' }
$null = Send-Hufu (Key 'm'); $null = Send-Hufu (Key 'l')
$r = Send-Hufu (Key 'space')  # 提交「两/次」类 → learn 记日志
Start-Sleep 1
$logged = (Test-Path $logPath) -and ((Get-Content $logPath -Encoding UTF8 | Measure-Object).Count -ge 1)
Check 'log_adjust 用户调整日志' $logged "log exists=$(Test-Path $logPath)"
$cfg.user.log_adjust = $false
$b = [System.Text.Encoding]::UTF8.GetBytes(($cfg | ConvertTo-Json -Depth 12))
Invoke-RestMethod 'http://127.0.0.1:4390/api/config' -Method Post -Body $b -ContentType 'application/json; charset=utf-8' | Out-Null

Write-Host "── 结果: $pass PASS / $fail FAIL ──"
exit 0
