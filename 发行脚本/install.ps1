# HuFu 虎符输入法 — 安装脚本（须管理员一次；安装.bat 已自动提权）
# 布局：复制到 %LOCALAPPDATA%\HuFu（程序+数据同处，用户可写），
#       HKLM 登记 COM 服务器，msctf 原生档案带虎符图标登记，
#       HKCU 自启动 + 语言列表装配。
$ErrorActionPreference = 'Stop'

$CLSID   = '{8F5C2A10-3E77-4B9C-A1D4-9E0B7C2F5A88}'
$PROFILE = '{8F5C2A11-3E77-4B9C-A1D4-9E0B7C2F5A88}'
$TFCAT_KBD = '{533C5E0E-5AC0-4ABD-B6F1-251B82B7BE7D}'

# ── 自动提权 ──
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
           ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    $ps = Join-Path $PSHOME 'powershell.exe'
    Start-Process $ps -Verb RunAs -ArgumentList "-ExecutionPolicy Bypass -File `"$PSCommandPath`"" -Wait
    exit
}

$src = $PSScriptRoot
$inst = Join-Path $env:LOCALAPPDATA 'HuFu'
$dll  = Join-Path $inst 'hufu_tsf.dll'
$exe  = Join-Path $inst 'hufu-server.exe'
$icon = Join-Path $inst '图标.ico'
$data = Join-Path $inst '数据'

Write-Host "安装到: $inst"

# ── 1) 复制文件（升级时保留用户 config.json 与词库日志）──
New-Item -ItemType Directory -Force -Path $inst | Out-Null
robocopy $src $inst /E /XF config.json /R:1 /W:1 /NFL /NDL /NP | Out-Null
$userCfg = Join-Path $data 'config.json'
if (-not (Test-Path $userCfg)) {
    New-Item -ItemType Directory -Force -Path $data | Out-Null
    Copy-Item (Join-Path $src '数据\config.json') $userCfg -Force
}
Write-Host '✓ 文件就位'

function Set-Reg([string]$Path, [string]$Name, [string]$Value) {
    if (-not (Test-Path $Path)) { New-Item -Path $Path -Force | Out-Null }
    if ($Name -eq '(default)') {
        $hive = if ($Path.StartsWith('HKLM:')) { [Microsoft.Win32.Registry]::LocalMachine } else { [Microsoft.Win32.Registry]::CurrentUser }
        $sub = $Path -replace '^[A-Z]+:\\', ''
        $key = $hive.OpenSubKey($sub, $true)
        if ($key) { $key.SetValue('', $Value); $key.Close() }
    } else {
        Set-ItemProperty -Path $Path -Name $Name -Value $Value -Type String
    }
}
function Set-RegDWord([string]$Path, [string]$Name, [int]$Value) {
    if (-not (Test-Path $Path)) { New-Item -Path $Path -Force | Out-Null }
    Set-ItemProperty -Path $Path -Name $Name -Value $Value -Type DWord
}

# ── 2) HKLM COM 服务器 + CTF TIP 清单 ──
$ips = "HKLM:\SOFTWARE\Classes\CLSID\$CLSID\InprocServer32"
Set-Reg "HKLM:\SOFTWARE\Classes\CLSID\$CLSID" '(default)' 'HuFu TSF Service'
Set-Reg $ips '(default)' $dll
Set-Reg $ips 'ThreadingModel' 'Apartment'
$tip = "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$CLSID"
Set-Reg "$tip\Description" '(default)' 'HuFu 虎符输入法（虎码）'
New-Item -Path "$tip\Category\Category\$TFCAT_KBD\$CLSID" -Force | Out-Null
New-Item -Path "$tip\Category\Item\$CLSID\$TFCAT_KBD" -Force | Out-Null
$lp = "$tip\LanguageProfile\0x00000804\$PROFILE"
Set-Reg $lp 'Description' 'HuFu 虎符输入法'
Set-Reg $lp 'Display Description' 'HuFu 虎符输入法'
Set-RegDWord $lp 'Enable' 1
Set-Reg $lp 'IconFile' $dll
Set-RegDWord $lp 'IconIndex' 0
Write-Host '✓ HKLM COM + CTF TIP 已注册'

# ── 3) HKCU 用户侧（DllRegisterServer 会写全套 CTF 键）──
$ipsUser = "HKCU:\Software\Classes\CLSID\$CLSID\InprocServer32"
Set-Reg "HKCU:\Software\Classes\CLSID\$CLSID" '(default)' 'HuFu TSF Service'
Set-Reg $ipsUser '(default)' $dll
Set-Reg $ipsUser 'ThreadingModel' 'Apartment'
regsvr32 /s $dll
Write-Host '✓ HKCU 用户侧已启用'

# ── 4) msctf 原生档案登记（带图标；Win+空格 浮层唯一数据源）──
& (Join-Path $inst 'hufu-tsf-smoke.exe') reg $icon
Write-Host '✓ msctf 原生档案已登记'

# ── 5) 语言列表 + 切换器装配 ──
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
Write-Host '✓ 语言列表 + 切换器装配已写入'

# ── 6) 开机自启（server 常驻 = 托盘 + 设置页 + 管道）──
$run = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
Set-Reg $run 'HuFu' ('"{0}" --data "{1}"' -f $exe, $data)
Write-Host '✓ 开机自启已设置'

# ── 7) 启动 server + 刷新输入浮层宿主 ──
Get-Process hufu-server -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Process $exe -ArgumentList '--data', $data -WindowStyle Hidden
Stop-Process -Name TextInputHost, ShellExperienceHost -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1
Start-Process ctfmon -ErrorAction SilentlyContinue

Write-Host ''
Write-Host '════════════════════════════════════' -ForegroundColor Green
Write-Host ' 安装完成！' -ForegroundColor Green
Write-Host '  · Win+空格 切到「HuFu 虎符输入法」'
Write-Host '  · 双击「设置.bat」打开设置窗口'
Write-Host '  · 托盘（输入法区）双击同样打开设置'
Write-Host '════════════════════════════════════' -ForegroundColor Green
Write-Host '  · 托盘小虎图标只在「切到虎符输入法」时出现；'
Write-Host '    想让它常驻任务栏（不进隐藏折叠区）：'
Write-Host '    右键任务栏 - 任务栏设置 - 其他系统托盘图标 - 把「hufu-server」打开（只需设一次）。'
Write-Host '  · 音效默认关闭，想开请到 设置 - 音效 打开。'
