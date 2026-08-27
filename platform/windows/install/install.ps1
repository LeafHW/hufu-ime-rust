# HuFu 虎符输入法 — Windows 安装脚本（需要管理员一次）
# 作用：把 hufu-tsf.dll 注册为系统 TSF 输入法（HKLM）+ 当前用户启用（HKCU）。
# 用法：右键「使用 PowerShell 运行」，或:  powershell -ExecutionPolicy Bypass -File .\install.ps1

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
    Start-Process pwsh -Verb RunAs -ArgumentList "-ExecutionPolicy Bypass -File `"$PSCommandPath`""
    exit
}

# ── 定位 DLL ────────────────────────────────────────────────
$dll = Join-Path $PSScriptRoot '..\target\release\hufu_tsf.dll'
if (-not (Test-Path $dll)) { $dll = 'E:\DSH-KF\hufu\platform\windows\target\release\hufu_tsf.dll' }
$dll = [System.IO.Path]::GetFullPath($dll)
if (-not (Test-Path $dll)) { throw "找不到 hufu_tsf.dll：$dll（先 cd platform/windows; cargo build --release）" }
Write-Host "DLL: $dll"

function Set-Reg([string]$Path, [string]$Name, [string]$Value) {
    New-Item -Path $Path -Force | Out-Null
    Set-ItemProperty -Path $Path -Name $Name -Value $Value -Type String
}

# ── HKLM：COM 服务器 + CTF TIP 清单 ────────────────────────
$ips = "HKLM:\SOFTWARE\Classes\CLSID\$CLSID\InprocServer32"
Set-Reg "HKLM:\SOFTWARE\Classes\CLSID\$CLSID" '(default)' 'HuFu TSF Service'
Set-Reg $ips '(default)' $dll
Set-Reg $ips 'ThreadingModel' 'Apartment'

$tip = "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$CLSID"
Set-Reg $tip '(default)' 'HuFu 虎符输入法'
Set-Reg "$tip\Description" '(default)' 'HuFu 虎符输入法（虎码）'
Set-Reg "$tip\Category\Category\$TFCAT_KBD" '(default)' $CLSID
Set-Reg "$tip\Category\Item\$TFCAT_KBD" '(default)' $CLSID
$lp = "$tip\LanguageProfile\0x00000804\$PROFILE"
Set-Reg $lp '(default)' 'HuFu 虎符输入法'
Set-Reg $lp 'Enable' '1'
Write-Host '✓ HKLM COM + CTF TIP 已注册'

# ── HKCU：当前用户可见（per-user 类注册兜底）────────────────
$ipsUser = "HKCU:\Software\Classes\CLSID\$CLSID\InprocServer32"
Set-Reg "HKCU:\Software\Classes\CLSID\$CLSID" '(default)' 'HuFu TSF Service'
Set-Reg $ipsUser '(default)' $dll
Set-Reg $ipsUser 'ThreadingModel' 'Apartment'
Set-Reg "HKCU:\Software\Microsoft\CTF\TIP\$CLSID\LanguageProfile\0x00000804\$PROFILE" 'Enable' '1'
Write-Host '✓ HKCU 用户侧已启用'

# ── 提醒 ───────────────────────────────────────────────────
Write-Host ''
Write-Host '安装完成。接下来：' -ForegroundColor Green
Write-Host '  1) 确保输入引擎在跑:  E:\DSH-KF\hufu\engine\target\release\hufu-server.exe'
Write-Host '     （它提供 \\.\pipe\hufu-ime 管道与 localhost 设置页）'
Write-Host '  2) 注销重登（或任务管理器结束 ctfmon.exe 后重开资源管理器）'
Write-Host '  3) Win+空格 切到「HuFu 虎符输入法」即可使用'
Write-Host ''
Write-Host '卸载: .\uninstall.ps1'
Stop-Transcript | Out-Null
