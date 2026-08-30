# HuFu 净室循环测试：一轮 = 卸载→删目录→新目录解压→绿色安装→全验收
# 用法：powershell -File roundtest.ps1 -Round N [-Zip <path>]
# 机器底座（HKLM/SystemIME/msctf 档案）不动；全程无 UAC。
param([int]$Round = 1, [string]$Zip = 'E:\DSH-KF\hufu-发行\HuFu虎符输入法-安装包.zip')

$ErrorActionPreference = 'Continue'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$CLSID = '{8F5C2A10-3E77-4B9C-A1D4-9E0B7C2F5A88}'
$PROFILE = '{8F5C2A11-3E77-4B9C-A1D4-9E0B7C2F5A88}'
$tipStr = "0804:$CLSID$PROFILE"
$pass = 0; $fail = 0; $failList = @()
function Check([string]$name, [bool]$ok) {
    if ($ok) { $script:pass++; Write-Host "    + $name" }
    else { $script:fail++; $script:failList += $name; Write-Host "    X $name" -ForegroundColor Red }
}
function Key([string]$k) {
    (Invoke-RestMethod -Uri 'http://127.0.0.1:4390/api/key' -Method Post -Body ([System.Text.Encoding]::UTF8.GetBytes("{`"key`":`"$k`"}")) -ContentType 'application/json')
}
function TypeIt([string[]]$keys, [int]$delayMs = 450, [int]$pauseSec = 2) {
    [void](Invoke-RestMethod -Uri 'http://127.0.0.1:4390/api/reset' -Method Post)
    $c = @()
    foreach ($k in $keys) {
        try { $r = Key $k } catch { return 'ERROR' }
        if ($r.outcome.commit) { $c += $r.outcome.commit }
        if ($delayMs -gt 0) { Start-Sleep -Milliseconds $delayMs }
    }
    if ($pauseSec -gt 0) { Start-Sleep -Seconds $pauseSec }
    return ($c -join '')
}

Write-Host "  ============ 第 $Round 轮 ============" -ForegroundColor Cyan
$dir = "$env:USERPROFILE\Downloads\HuFu测试-N$Round"

# ── A) 卸载（上一轮目录；首轮用现装目录）──
$prev = "$env:USERPROFILE\Downloads\HuFu安装包"
if ($Round -gt 1) { $prev = "$env:USERPROFILE\Downloads\HuFu测试-N$($Round-1)" }
if (Test-Path "$prev\uninstall.ps1") {
    Set-Location $prev
    powershell -NoProfile -ExecutionPolicy Bypass -File .\uninstall.ps1 -NoHKLM *> $null
    Start-Sleep -Seconds 1
    Check '[卸] 语言列表移除' (-not ((Get-WinUserLanguageList | ForEach-Object { $_.InputMethodTips }) -contains $tipStr))
    Check '[卸] HKCU CLSID 清' (-not (Test-Path "HKCU:\Software\Classes\CLSID\$CLSID"))
    Check '[卸] HKCU TIP 清' (-not (Test-Path "HKCU:\Software\Microsoft\CTF\TIP\$CLSID"))
    Check '[卸] Run 自启清' (-not ((Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -ErrorAction SilentlyContinue).HuFu))
    Check '[卸] 快捷方式清' (-not (Test-Path "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\HuFu 虎符输入法设置.lnk"))
    Check '[卸] InstallDir 清' (-not (Test-Path 'HKCU:\Software\HuFu'))
    # 重启 TSF 宿主：挂旧 DLL 的宿主会在窗口期用旧自愈链拉起 dev server
    Stop-Process -Name TextInputHost, ctfmon -Force -ErrorAction SilentlyContinue
    Get-Process hufu-server -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 2
    Start-Process ctfmon -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 1
    Check '[卸] server 停' (@(Get-Process hufu-server -ErrorAction SilentlyContinue).Count -eq 0)
    Check '[卸] msctf 机器级档案保留（每用户卸载不动）' (Test-Path "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$CLSID")
    foreach ($try in 1..3) {
        if (-not (Test-Path $prev)) { break }
        Get-Process hufu-server -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
        Start-Sleep -Seconds 1
        Remove-Item $prev -Recurse -Force -ErrorAction SilentlyContinue
    }
}

# ── B) 新目录解压（删不掉的占用文件腾位 .oldN）──
foreach ($try in 1..3) {
    if (-not (Test-Path $dir)) { break }
    Get-Process hufu-server -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 1
    Remove-Item $dir -Recurse -Force -ErrorAction SilentlyContinue
    Get-ChildItem $dir -Recurse -File -ErrorAction SilentlyContinue | ForEach-Object {
        $n = 1; while (Test-Path "$($_.FullName).old$n") { $n++ }
        Rename-Item $_.FullName "$($_.Name).old$n" -Force -ErrorAction SilentlyContinue
    }
}
Expand-Archive -Path $Zip -DestinationPath $dir -Force
# zip 自带顶层文件夹（防止解压散落）：定位实际包目录
if (-not (Test-Path "$dir\hufu-server.exe") -and (Test-Path "$dir\HuFu虎符输入法-安装包\hufu-server.exe")) {
    $dir = "$dir\HuFu虎符输入法-安装包"
}
Check '[解] 目录就位' (Test-Path "$dir\hufu-server.exe")

# ── C) 绿色安装（每用户，机器级已由底座承担）──
Set-Location $dir
powershell -NoProfile -ExecutionPolicy Bypass -File .\install.ps1 -NoHKLM *> $null
Start-Sleep -Seconds 2
Check '[装] InstallDir=当轮目录' ((Get-ItemProperty 'HKCU:\Software\HuFu' -ErrorAction SilentlyContinue).InstallDir -eq $dir)
$runV = (Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -ErrorAction SilentlyContinue).HuFu
Check '[装] Run 自启→当轮' ($runV -like "*$dir*")
Check '[装] 语言列表加入' (((Get-WinUserLanguageList | ForEach-Object { $_.InputMethodTips }) -contains $tipStr))
Check '[装] HKCU CLSID 注册' (Test-Path "HKCU:\Software\Classes\CLSID\$CLSID")

# 让当轮构建真实上机：HKCU CLSID 指向当轮 DLL（SystemIME 副本可能是
# 旧构建；TextInputHost/记事本等普通宿主可读 Downloads 目录）
reg add "HKCU\Software\Classes\CLSID\$CLSID\InprocServer32" /ve /t REG_SZ /d "$dir\hufu_tsf.dll" /f | Out-Null
$ipsVal = [string](Get-Item "HKCU:\Software\Classes\CLSID\$CLSID\InprocServer32").GetValue('')
Check '[装] HKCU CLSID→当轮 DLL' ($ipsVal -eq "$dir\hufu_tsf.dll")
Check '[装] 快捷方式建' (Test-Path "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\HuFu 虎符输入法设置.lnk")
$ps = Get-Process hufu-server -ErrorAction SilentlyContinue
$psPath = ''
foreach ($try in 1..3) {
    try { if ($ps) { $psPath = $ps.Path } } catch {}
    if (-not $psPath -and $ps) { $psPath = (Get-CimInstance Win32_Process -Filter "Name='hufu-server.exe'" | Select-Object -First 1).ExecutablePath }
    if ($psPath) { break }
    Start-Sleep -Seconds 2
    $ps = Get-Process hufu-server -ErrorAction SilentlyContinue
}
Check '[装] server 常驻@当轮' ($ps -and $psPath -eq "$dir\hufu-server.exe")
$cJunk = @(Get-ChildItem "$env:LOCALAPPDATA\HuFu" -ErrorAction SilentlyContinue | Where-Object { $_.Name -notlike '*.del' -and $_.Name -notlike '*.old*' })
Check '[装] C 盘零写入（忽略已知残留）' ($cJunk.Count -eq 0)
Check '[装] 设置页 200' ((Invoke-WebRequest 'http://127.0.0.1:4390/' -UseBasicParsing -TimeoutSec 6).StatusCode -eq 200)
$cfg = Invoke-RestMethod 'http://127.0.0.1:4390/api/config'
Check '[装] 出厂方案虎整句' ($cfg.schema.current -eq '虎整句')

# ── D) msctf 识别（当轮 DLL）──
$out = (& "$dir\hufu-tsf-smoke.exe" 2>&1 | Out-String)
Check '[msctf] 枚举识别 ours' ($out -match '8F5C2A10 <-- ours')
Check '[msctf] ActivateProfile' ($out -match 'ActivateProfile ✓')

# ── E) 的窒闷 三场景 + 词汇 ──
$t1 = TypeIt @('u','e','e','y','i','a','h','x','space') 0 0
Check "[打] 快打=$t1" ($t1 -eq '的窒闷')
$t2 = TypeIt @('u','e','e','y','i','a','h','x') 500 2
$r = Key 'space'; if ($r.outcome.commit) { $t2 += $r.outcome.commit }
Check "[打] 慢打停顿=$t2" ($t2 -eq '的窒闷')
$t3 = TypeIt @('u','e','e','y','i','a','h','x') 500 2
$r = Key 'space'; if ($r.outcome.commit) { $t3 += $r.outcome.commit }
Check "[打] 第二句=$t3" ($t3 -eq '的窒闷')
$w1 = TypeIt @('j','w','x','v','n') 450 2; $r = Key 'space'; if ($r.outcome.commit) { $w1 += $r.outcome.commit }
Check "[词] 作秀=$w1" ($w1 -eq '作秀')
$w2 = TypeIt @('b','e','m','a','v','y') 450 2; $r = Key 'space'; if ($r.outcome.commit) { $w2 += $r.outcome.commit }
Check "[词] 杀青=$w2" ($w2 -eq '杀青')

# ── F) 管道协议（帧协议：4B 小端长度 + JSON；ping 响应 {"ok":true,...}）──
$pipeOk = $false
try {
    $pipe = New-Object System.IO.Pipes.NamedPipeClientStream('.', 'hufu-ime', [System.IO.Pipes.PipeDirection]::InOut)
    $pipe.Connect(3000)
    $req = [System.Text.Encoding]::UTF8.GetBytes('{"op":"ping"}')
    $len = [BitConverter]::GetBytes([Int32]$req.Length)
    $pipe.Write($len, 0, 4); $pipe.Write($req, 0, $req.Length); $pipe.Flush()
    function ReadN($st, $cnt) {
        $b = New-Object byte[] $cnt; $got = 0
        while ($got -lt $cnt) {
            $r = $st.Read($b, $got, $cnt - $got)
            if ($r -le 0) { break }
            $got += $r
        }
        return ,$b
    }
    $bl = ReadN $pipe 4
    $n = [BitConverter]::ToInt32($bl, 0)
    if ($n -gt 0 -and $n -lt 1048576) {
        $buf = ReadN $pipe $n
        $pipeOk = ([System.Text.Encoding]::UTF8.GetString($buf)) -match '"ok"\s*:\s*true'
    }
    $pipe.Close()
} catch {}
Check '[管] ping 应答' $pipeOk

# ── G) 自愈链（InstallDir 拉起）──
Stop-Process -Name hufu-server -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1
& "$dir\hufu-tsf-smoke.exe" testkey *> $null
Start-Sleep -Seconds 5
$ps2 = Get-Process hufu-server -ErrorAction SilentlyContinue
$p2 = ''
try { $p2 = $ps2.Path } catch {}
if (-not $p2 -and $ps2) { $p2 = (Get-CimInstance Win32_Process -Filter "Name='hufu-server.exe'").ExecutablePath }
Check '[愈] server 从当轮拉起' ($ps2 -and $p2 -eq "$dir\hufu-server.exe")
try {
    $rt = Invoke-RestMethod -Uri 'http://127.0.0.1:4390/api/sentence_test' -Method Post -Body ([System.Text.Encoding]::UTF8.GetBytes('{"raw":"ueeyiahx"}')) -ContentType 'application/json' -TimeoutSec 6
    Check '[愈] 引擎存活' (@($rt.candidates)[0] -eq '的窒闷')
} catch { Check '[愈] 引擎存活' $false }

# TIP 键树（MASTER+Enable）：msctf 对 HKCU TIP 键有「删→回填」重整
# （触发点=语言列表变化），窗口可达 20s——断言挪至轮末+重试 5×3s。
$tipTree = "HKCU:\Software\Microsoft\CTF\TIP\$CLSID"
$tipOk = $false
foreach ($try in 1..5) {
    $t2 = Test-Path "$tipTree\Category\Category\{533C5E0E-5AC0-4ABD-B6F1-251B82B7BE7D}\$CLSID"
    $t3 = ((Get-ItemProperty "$tipTree\LanguageProfile\0x00000804\$PROFILE" -ErrorAction SilentlyContinue).Enable -eq 1)
    if ($t2 -and $t3) { $tipOk = $true; break }
    Start-Sleep -Seconds 3
}
Check '[装] HKCU TIP 树(MASTER+Enable 最终态)' $tipOk
# HKLM 分类全套（8 项，虎爪/主流 IME 同款）——切换器可用性依据
$catNeed = @('{046B8C80-1647-40F7-9B21-B93B81AABC1B}', '{13A016DF-560B-46CD-947A-4C3AF1E0E35D}', '{25504FB4-7BAB-4BC1-9C69-CF81890F0EF5}', '{34745C63-B2F0-4784-8B67-5E12C8701A31}', '{364215D9-75BC-11D7-A6EF-00065B84435C}', '{49D2F9CE-1F5E-11D7-A6D3-00065B84435C}', '{49D2F9CF-1F5E-11D7-A6D3-00065B84435C}', '{CCF05DD7-4A87-11D7-A6E2-00065B84435C}')
$catHit = @($catNeed | Where-Object { Test-Path "HKLM:\SOFTWARE\Microsoft\CTF\Category\Category\$_\$CLSID" }).Count
Check '[底] HKLM TIP 分类齐全(8)' ($catHit -eq 8)
# ── H) 中英状态牌（宿主挂载）+ DLL 加载画像 ──
Remove-Item "$env:TEMP\hufu-langbar.log" -Force -ErrorAction SilentlyContinue
& "$dir\hufu-tsf-smoke.exe" *> $null
Start-Sleep -Seconds 1
$lb = Get-Content "$env:TEMP\hufu-langbar.log" -Encoding UTF8 -ErrorAction SilentlyContinue | Out-String
Check '[牌] langbar install ok' ($lb -match 'install ok')
$loads = @(Get-ChildItem 'C:\ProgramData\HuFu\diag' -Filter 'load-*.txt' -ErrorAction SilentlyContinue)
Check '[载] DLL 加载画像留痕' ($loads.Count -gt 0)

# ── I) 新宿主默认输入法（记录；msctf 会在 ctfmon 重启时按 SortOrder
#     重排 tips——平台行为，Insert(0) 尽力写默认位，不作硬性断言）──
$tips0 = ((Get-WinUserLanguageList | Where-Object { $_.LanguageTag -like 'zh*' }).InputMethodTips)[0]
Write-Host "    · 语言列表[0]=$(if ($tips0 -like "*$CLSID*") {'虎符（默认位达成）'} else {'系统重排后非虎符（平台行为，Win+空格 可切）'})"

Write-Host "  ---- 第 $Round 轮: $pass OK / $fail FAIL ----" -ForegroundColor $(if ($fail -eq 0) { 'Green' } else { 'Red' })
if ($failList) { $failList | ForEach-Object { Write-Host "    FAIL> $_" -ForegroundColor Red } }
exit $(if ($fail -eq 0) { 0 } else { 1 })
