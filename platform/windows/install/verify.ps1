# HuFu 安装后全链验证（安装完成后自动由 agent 触发）
$ErrorActionPreference = 'Continue'
$CLSID = '{8F5C2A10-3E77-4B9C-A1D4-9E0B7C2F5A88}'
$PROFILE_G = '{8F5C2A11-3E77-4B9C-A1D4-9E0B7C2F5A88}'

Write-Host '── 1) HKLM 注册键 ──'
$tip = "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$CLSID"
if (Test-Path $tip) {
    Write-Host "  TIP ✓"
    $lp = Get-ItemProperty "$tip\LanguageProfile\0x00000804\$PROFILE_G" -ErrorAction SilentlyContinue
    Write-Host "  LanguageProfile Enable = $($lp.Enable)"
} else { Write-Host '  TIP ✗（未安装）'; exit 1 }
$ips = Get-ItemProperty "HKLM:\SOFTWARE\Classes\CLSID\$CLSID\InprocServer32" -ErrorAction SilentlyContinue
Write-Host "  InprocServer32 = $($ips.'(default)')  ThreadingModel = $($ips.ThreadingModel)"

Write-Host '── 2) 输入法列表（EnumInputLanguages）──'
Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public class KL {
  [DllImport("user32.dll")] public static extern int GetKeyboardLayoutList(int nBuff, IntPtr[] list);
  [DllImport("user32.dll")] public static extern int GetKeyboardLayoutNameW(StringBuilder name);
}
'@
# TIP 型输入法不占 KL，改查 WinUserLanguageList
$langs = Get-WinUserLanguageList -ErrorAction SilentlyContinue
foreach ($l in $langs) {
  Write-Host "  $($l.LanguageTag): $($l.InputMethodTips -join ' ')"
}

Write-Host '── 3) server 进程 ──'
$pid_f = 'E:\DSH-KF\hufu\hufu-data\server.pid'
if (Test-Path $pid_f) {
  $sp = [int](Get-Content $pid_f -Raw)
  $proc = Get-Process -Id $sp -ErrorAction SilentlyContinue
  Write-Host "  pid=$sp 存活: $($null -ne $proc)"
}

Write-Host '── 4) HTTP 探活 ──'
$r = & curl.exe -s http://127.0.0.1:4390/api/state
Write-Host "  state: $($r.Substring(0,[Math]::Min(60,$r.Length)))"
