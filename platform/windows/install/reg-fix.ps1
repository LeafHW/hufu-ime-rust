# HuFu 分类注册修复（管理员运行一次）
# 作用：msctf 全局分类注册（caps 来源，切换器显示的关键）+ 垃圾键清理。

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
           ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    $ps = Join-Path $PSHOME 'powershell.exe'
    Start-Process $ps -Verb RunAs -ArgumentList "-ExecutionPolicy Bypass -File `"$PSCommandPath`""
    exit
}

$smoke = 'E:\DSH-KF\hufu\platform\windows\target\release\hufu-tsf-smoke.exe'
if (-not (Test-Path $smoke)) { throw "找不到 $smoke" }

Write-Host '── 1) msctf 档案 + 分类注册' -ForegroundColor Cyan
& $smoke reg

Write-Host '── 2) 清理垃圾注册表项' -ForegroundColor Cyan
$tip = 'HKLM:\SOFTWARE\Microsoft\CTF\TIP\{8F5C2A10-3E77-4B9C-A1D4-9E0B7C2F5A88}'
# 垃圾默认值（旧版单层写法残留）
$k1 = "$tip\Category\Category\{533C5E0E-5AC0-4ABD-B6F1-251B82B7BE7D}"
if (Test-Path $k1) {
    (Get-Item $k1).OpenSubKey('', $true) | Out-Null
    $hive = [Microsoft.Win32.Registry]::LocalMachine
    $key = $hive.OpenSubKey('SOFTWARE\Microsoft\CTF\TIP\{8F5C2A10-3E77-4B9C-A1D4-9E0B7C2F5A88}\Category\Category\{533C5E0E-5AC0-4ABD-B6F1-251B82B7BE7D}', $true)
    if ($key) { $key.DeleteValue(''); $key.Close(); Write-Host '  垃圾默认值已清' }
}
# 错层子键
Remove-Item "$tip\Category\Item\{533C5E0E-5AC0-4ABD-B6F1-251B82B7BE7D}" -Recurse -Force -ErrorAction SilentlyContinue
Write-Host '  错层子键已清'
# Description 乱码尾巴重写
$lpKey = $hive.OpenSubKey('SOFTWARE\Microsoft\CTF\TIP\{8F5C2A10-3E77-4B9C-A1D4-9E0B7C2F5A88}\LanguageProfile\0x00000804\{8F5C2A11-3E77-4B9C-A1D4-9E0B7C2F5A88}', $true)
if ($lpKey) {
    $lpKey.SetValue('Description', 'HuFu 虎符输入法', [Microsoft.Win32.RegistryValueKind]::String)
    $lpKey.Close()
    Write-Host '  Description 已修'
}

Write-Host '── 3) ctfmon 重读' -ForegroundColor Cyan
Stop-Process -Name ctfmon -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2
Start-Process ctfmon -ErrorAction SilentlyContinue

Write-Host ''
Write-Host '完成 —— 请按 Win+空格 检查第 4 项' -ForegroundColor Green
