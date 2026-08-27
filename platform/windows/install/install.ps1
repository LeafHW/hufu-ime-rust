# HuFu 虎符输入法 — Windows 安装脚本（需要管理员一次）
# 作用：把 hufu-tsf.dll 注册为系统 TSF 输入法（HKLM）+ 当前用户启用（HKCU）。
# 注册表布局对齐微软拼音实测：LanguageProfile 名值（Description=SZ /
# Enable=DWORD / IconFile+IconIndex），Category 两层子键纯存在性。

$ErrorActionPreference = 'Stop'
$log = Join-Path $PSScriptRoot 'install.log'
Start-Transcript -Path $log -Force | Out-Null

$CLSID        = '{8F5C2A10-3E77-4B9C-A1D4-9E0B7C2F5A88}'
$PROFILE      = '{8F5C2A11-3E77-4B9C-A1D4-9E0B7C2F5A88}'
$TFCAT_KBD    = '{533C5E0E-5AC0-4ABD-B6F1-251B82B7BE7D}'  # GUID_TFCAT_TIP_KEYBOARD

# ── 自动提权 ────────────────────────────────────────────────
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
           ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    Write-Host '需要管理员权限（TSF 系统注册写到 HKLM），触发 UAC…' -ForegroundColor Yellow
    $ps = Join-Path $PSHOME 'powershell.exe'
    Start-Process $ps -Verb RunAs -ArgumentList "-ExecutionPolicy Bypass -File `"$PSCommandPath`""
    exit
}

# ── 定位 DLL ────────────────────────────────────────────────
$dll = Join-Path $PSScriptRoot '..\target\release\hufu_tsf.dll'
if (-not (Test-Path $dll)) { $dll = 'E:\DSH-KF\hufu\platform\windows\target\release\hufu_tsf.dll' }
$dll = [System.IO.Path]::GetFullPath($dll)
if (-not (Test-Path $dll)) { throw "找不到 hufu_tsf.dll：$dll（先 cd platform/windows; cargo build --release）" }
Write-Host "DLL: $dll"

function Set-Reg([string]$Path, [string]$Name, [string]$Value) {
    if (-not (Test-Path $Path)) { New-Item -Path $Path -Force | Out-Null }
    if ($Name -eq '(default)') {
        # 注册表 API 直写默认值（Set-ItemProperty 对 '(default)' 在部分键上不生效）
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

# ── HKLM：COM 服务器 + CTF TIP 清单 ────────────────────────
$ips = "HKLM:\SOFTWARE\Classes\CLSID\$CLSID\InprocServer32"
Set-Reg "HKLM:\SOFTWARE\Classes\CLSID\$CLSID" '(default)' 'HuFu TSF Service'
Set-Reg $ips '(default)' $dll
Set-Reg $ips 'ThreadingModel' 'Apartment'

$tip = "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$CLSID"
Set-Reg "$tip\Description" '(default)' 'HuFu 虎符输入法（虎码）'
# Category 两层子键（对齐微拼：纯子键存在性，无值）
New-Item -Path "$tip\Category\Category\$TFCAT_KBD\$CLSID" -Force | Out-Null
New-Item -Path "$tip\Category\Item\$CLSID\$TFCAT_KBD" -Force | Out-Null
# 语言档案：名值布局（对齐微拼实测：Description=SZ / Enable=DWORD / Icon）
$lp = "$tip\LanguageProfile\0x00000804\$PROFILE"
Set-Reg $lp 'Description' 'HuFu 虎符输入法'
Set-Reg $lp 'Display Description' 'HuFu 虎符输入法'
Set-RegDWord $lp 'Enable' 1
Set-Reg $lp 'IconFile' "$env:SystemRoot\System32\shell32.dll"
Set-RegDWord $lp 'IconIndex' 70
Write-Host '✓ HKLM COM + CTF TIP 已注册'

# ── HKCU：当前用户可见（per-user 类注册兜底）────────────────
$ipsUser = "HKCU:\Software\Classes\CLSID\$CLSID\InprocServer32"
Set-Reg "HKCU:\Software\Classes\CLSID\$CLSID" '(default)' 'HuFu TSF Service'
Set-Reg $ipsUser '(default)' $dll
Set-Reg $ipsUser 'ThreadingModel' 'Apartment'
Set-RegDWord "HKCU:\Software\Microsoft\CTF\TIP\$CLSID\LanguageProfile\0x00000804\$PROFILE" 'Enable' 1
Write-Host '✓ HKCU 用户侧已启用'

# ── msctf 原生档案登记（提权环境直调，官方 IME 安装器同款）────
# ITfInputProcessorProfiles::Register + AddLanguageProfile + Enable。
# 非提权必 E_FAIL；缺这步 Win+空格 切换器不显示、官方 API 拒收。
$smoke = Join-Path $PSScriptRoot '..\target\release\hufu-tsf-smoke.exe'
$smoke = [System.IO.Path]::GetFullPath($smoke)
if (Test-Path $smoke) {
    & $smoke reg
    Write-Host '✓ msctf 原生档案已登记'
} else {
    Write-Host '⚠ 未找到 hufu-tsf-smoke.exe，跳过 msctf 登记' -ForegroundColor Yellow
}

# ── 语言列表 + 输入切换器装配 ────────────────────────────────
# Set-WinUserLanguageList 会静默丢弃未登记 TIP；msctf 登记后正常走官方 API。
$tipStr = "0804:$CLSID$PROFILE"
$list = Get-WinUserLanguageList
$zh = $list | Where-Object { $_.LanguageTag -like 'zh*' } | Select-Object -First 1
if (-not $zh) { $zh = $list[0] }
if ($zh.InputMethodTips -notcontains $tipStr) {
    $zh.InputMethodTips.Add($tipStr)
    Set-WinUserLanguageList $list -Force
}
# 切换器装配表（被系统清理时 Win+空格 不显示，兜底直写）
$asm = "HKCU:\Software\Microsoft\CTF\SortOrder\AssemblyItem\0x00000804\{34745C63-B2F0-4784-8B67-5E12C8701A31}\00000003"
New-Item -Path $asm -Force | Out-Null
Set-ItemProperty -Path $asm -Name 'CLSID' -Value $CLSID -Type String
Set-ItemProperty -Path $asm -Name 'KeyboardLayout' -Value '0' -Type String
Set-ItemProperty -Path $asm -Name 'Profile' -Value $PROFILE -Type String
Write-Host '✓ 语言列表 + 切换器装配已写入'

# ── ctfmon 重读 ─────────────────────────────────────────────
Stop-Process -Name ctfmon -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2
Start-Process ctfmon -ErrorAction SilentlyContinue
Write-Host '✓ ctfmon 已重启'

# ── 提醒 ───────────────────────────────────────────────────
Write-Host ''
Write-Host '安装完成。接下来：' -ForegroundColor Green
Write-Host '  1) 确保输入引擎在跑:  E:\DSH-KF\hufu\engine\target\release\hufu-server.exe'
Write-Host '     （它提供 \\.\pipe\hufu-ime 管道与 localhost 设置页）'
Write-Host '  2) Win+空格 切到「HuFu 虎符输入法」即可使用'
Write-Host ''
Write-Host '卸载: .\uninstall.ps1'
Stop-Transcript | Out-Null
