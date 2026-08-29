# HuFu 虎符输入法 — 安装脚本（双阶段：普通权限主导，提权只做注册）
# - 文件/HKCU/语言列表/自启/server 永远普通权限执行（server 提权启动会锁管道 ACL，
#   导致所有普通应用连不上→只能打字母，2026-08-29 实测教训）。
# - HKLM 机器级键 + msctf 原生登记（本机实测需提权才 0x00000000）交给一次 UAC 的
#   提权子进程（-PhaseElevated），日志回流本窗口可见。
# - -NoHKLM：完全跳过提权（无管理员机器的每用户安装；msctf 登记尽力而为）。
param([switch]$NoHKLM, [switch]$PhaseElevated)

$ErrorActionPreference = 'Continue'

$CLSID   = '{8F5C2A10-3E77-4B9C-A1D4-9E0B7C2F5A88}'
$PROFILE = '{8F5C2A11-3E77-4B9C-A1D4-9E0B7C2F5A88}'
$TFCAT_KBD = '{533C5E0E-5AC0-4ABD-B6F1-251B82B7BE7D}'

$src  = Split-Path -Parent $MyInvocation.MyCommand.Path
$inst = Join-Path $env:LOCALAPPDATA 'HuFu'
$data = Join-Path $inst '数据'
$dll  = Join-Path $inst 'hufu_tsf.dll'
$exe  = Join-Path $inst 'hufu-server.exe'
$icon = Join-Path $inst '图标.ico'

function Set-Reg([string]$path, [string]$name, [string]$val) {
    if (-not (Test-Path $path)) { New-Item -Path $path -Force | Out-Null }
    if ($name -eq '(default)') {
        $k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey(($path -replace '^[^:\\]+:\\', ''), $true)
        if ($k) { $k.SetValue('', $val); $k.Close() }
        else { $ki = Get-Item $path; $ki.SetValue('', $val) }
    } else { Set-ItemProperty -Path $path -Name $name -Value $val -Type String }
}
function Set-RegDWord([string]$path, [string]$name, [int]$val) {
    if (-not (Test-Path $path)) { New-Item -Path $path -Force | Out-Null }
    Set-ItemProperty -Path $path -Name $name -Value $val -Type DWord
}
function Set-RegHKLM([string]$path, [string]$name, [string]$val) {
    if (-not (Test-Path $path)) { New-Item -Path $path -Force | Out-Null }
    if ($name -eq '(default)') {
        $k = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey(($path -replace '^HKLM:\\', ''), $true)
        if ($k) { $k.SetValue('', $val); $k.Close() }
    } else { Set-ItemProperty -Path $path -Name $name -Value $val -Type String }
}

# ═══ 提权子阶段：只做 HKLM 注册 + msctf 登记，绝不启动 server ═══
if ($PhaseElevated) {
    Write-Host '—— 提权阶段：HKLM 机器级注册 ——'
    $ips = "HKLM:\SOFTWARE\Classes\CLSID\$CLSID\InprocServer32"
    Set-RegHKLM "HKLM:\SOFTWARE\Classes\CLSID\$CLSID" '(default)' 'HuFu TSF Service'
    Set-RegHKLM $ips '(default)' $dll
    Set-RegHKLM $ips 'ThreadingModel' 'Apartment'
    $tip = "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$CLSID"
    Set-RegHKLM "$tip\Description" '(default)' 'HuFu 虎符输入法（虎码）'
    New-Item -Path "$tip\Category\Category\$TFCAT_KBD\$CLSID" -Force | Out-Null
    New-Item -Path "$tip\Category\Item\$CLSID\$TFCAT_KBD" -Force | Out-Null
    $lp = "$tip\LanguageProfile\0x00000804\$PROFILE"
    Set-RegHKLM $lp 'Description' 'HuFu 虎符输入法'
    Set-RegHKLM $lp 'Display Description' 'HuFu 虎符输入法'
    Set-ItemProperty -Path $lp -Name 'Enable' -Value 1 -Type DWord
    Set-RegHKLM $lp 'IconFile' $dll
    Set-ItemProperty -Path $lp -Name 'IconIndex' -Value 0 -Type DWord
    Write-Host 'OK HKLM 机器级已注册'
    Write-Host '—— 提权阶段：msctf 原生登记 ——'
    & (Join-Path $inst 'hufu-tsf-smoke.exe') reg $icon
    Write-Host '提权阶段完成。'
    exit
}

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
           ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
$inAdminGroup = $false
try {
    $inAdminGroup = (whoami /groups /fo csv | Select-String 'S-1-5-32-544').Count -gt 0
} catch {}

Write-Host ''
Write-Host 'HuFu 虎符输入法 安装' -ForegroundColor Cyan
Write-Host "安装到: $inst"

# ── 1) 文件就位（保留用户 config.json）──
robocopy $src $inst /E /XF config.json install.ps1 uninstall.ps1 *.bat > $null
Write-Host 'OK 文件就位'

# ── 2) HKCU COM + CTF TIP 键树（每用户；msctf/COM 解析 HKCU 优先）──
$ipsUser = "HKCU:\Software\Classes\CLSID\$CLSID\InprocServer32"
Set-Reg "HKCU:\Software\Classes\CLSID\$CLSID" '(default)' 'HuFu TSF Service'
Set-Reg $ipsUser '(default)' $dll
Set-Reg $ipsUser 'ThreadingModel' 'Apartment'
regsvr32 /s $dll
$tipU = "HKCU:\Software\Microsoft\CTF\TIP\$CLSID"
Set-Reg $tipU '(default)' 'HuFu 输入法'
Set-Reg "$tipU\Description" '(default)' 'HuFu 虎符输入法（虎码）'
New-Item -Path "$tipU\Category\Category\$TFCAT_KBD\$CLSID" -Force | Out-Null
New-Item -Path "$tipU\Category\Item\$CLSID\$TFCAT_KBD" -Force | Out-Null
$lpU = "$tipU\LanguageProfile\0x00000804\$PROFILE"
Set-Reg $lpU '(default)' 'HuFu 虎符输入法'
Set-Reg $lpU 'Enable' '1'
Set-RegDWord $lpU 'IconIndex' 0
Set-Reg $lpU 'IconFile' $dll
Write-Host 'OK HKCU COM + TIP 键树已注册'

# ── 3) 提权注册（HKLM + msctf；一次 UAC，日志回流本窗口）──
if (-not $NoHKLM) {
    if ($isAdmin) {
        & $PSCommandPath -PhaseElevated
    } elseif ($inAdminGroup) {
        Write-Host '（弹出 UAC：机器级注册 + msctf 登记，请点「是」）'
        $elog = Join-Path $env:TEMP 'hufu-install-elevated.log'
        $ps = "$env:WINDIR\System32\WindowsPowerShell\v1.0\powershell.exe"
        $arg = "-NoProfile -ExecutionPolicy Bypass -Command `"& '$PSCommandPath' -PhaseElevated *> '$elog'`""
        Start-Process $ps -Verb RunAs -ArgumentList $arg -Wait
        if (Test-Path $elog) { Get-Content $elog | ForEach-Object { Write-Host "  $_" } }
    } else {
        Write-Host '· 无管理员权限：跳过 HKLM/msctf 提权登记，尝试每用户登记'
        & (Join-Path $inst 'hufu-tsf-smoke.exe') reg $icon
    }
} else {
    Write-Host '· -NoHKLM：跳过提权，尝试每用户 msctf 登记'
    & (Join-Path $inst 'hufu-tsf-smoke.exe') reg $icon
}

# ── 4) 语言列表 + 切换器装配（每用户）──
$tipStr = "0804:$CLSID$PROFILE"
$list = Get-WinUserLanguageList
$zh = $list | Where-Object { $_.LanguageTag -like 'zh*' } | Select-Object -First 1
if (-not $zh) { $zh = $list[0] }
if ($zh.InputMethodTips -notcontains $tipStr) {
    $zh.InputMethodTips.Add($tipStr)
    Set-WinUserLanguageList $list -Force
}
$asm = "HKCU:\Software\Microsoft\CTF\SortOrder\AssemblyItem\0x00000804\{34745C63-B2F0-4784-8B67-5E12C8701A31}\00000003"
New-Item -Path $asm -Force | Out-Null
Set-ItemProperty -Path $asm -Name 'CLSID' -Value $CLSID -Type String
Set-ItemProperty -Path $asm -Name 'KeyboardLayout' -Value '0' -Type String
Set-ItemProperty -Path $asm -Name 'Profile' -Value $PROFILE -Type String
Write-Host 'OK 语言列表 + 切换器装配已写入'

# ── 5) 开机自启（server 常驻 = 托盘 + 设置页 + 管道）──
$run = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
Set-Reg $run 'HuFu' ('"{0}"' -f $exe)
Write-Host 'OK 开机自启已设置（数据目录默认 exe 同目录）'

# ── 6) 启动 server ——【铁律】必须普通权限：提权启动的管道会拒绝普通应用】──
Get-Process hufu-server -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500
if ($isAdmin) {
    # 本脚本自身被提权运行（如右键管理员）：经 explorer 中转降权启动
    explorer.exe $exe
} else {
    Start-Process $exe -WindowStyle Hidden
}
Start-Sleep -Seconds 2
Stop-Process -Name TextInputHost, ShellExperienceHost -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1
Start-Process ctfmon -ErrorAction SilentlyContinue

Write-Host ''
Write-Host '=========================================='
Write-Host ' 安装完成！'
Write-Host '  · Win+空格 切到「HuFu 虎符输入法」'
Write-Host '  · 双击「设置.bat」打开设置窗口'
Write-Host '  · 托盘（输入法区）双击同样打开设置'
Write-Host '=========================================='
Write-Host '  · 无需重启/注销；正在运行的应用重开后才加载新输入法'
if ($NoHKLM) { Write-Host '  · 本次为每用户安装（-NoHKLM，未写 HKLM）' }
Write-Host '  · 托盘小虎图标只在「切到虎符输入法」时出现；想常驻任务栏：'
Write-Host '    右键任务栏 - 任务栏设置 - 其他系统托盘图标 - 打开「hufu-server」。'
