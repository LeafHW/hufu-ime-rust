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

Write-Host '── 2) 分类全集对齐（caps 位来源；照抄微软五笔的 8 分类）' -ForegroundColor Cyan
$tip = 'HKLM:\SOFTWARE\Microsoft\CTF\TIP\{8F5C2A10-3E77-4B9C-A1D4-9E0B7C2F5A88}'
$hive = [Microsoft.Win32.Registry]::LocalMachine
$clsid = '{8F5C2A10-3E77-4B9C-A1D4-9E0B7C2F5A88}'
$cats = @(
    '{046B8C80-1647-40F7-9B21-B93B81AABC1B}',
    '{13A016DF-560B-46CD-947A-4C3AF1E0E35D}',
    '{25504FB4-7BAB-4BC1-9C69-CF81890F0EF5}',
    '{34745C63-B2F0-4784-8B67-5E12C8701A31}',  # TFCAT_TIP_KEYBOARD（键盘，必需）
    '{364215D9-75BC-11D7-A6EF-00065B84435C}',
    '{49D2F9CE-1F5E-11D7-A6D3-00065B84435C}',
    '{49D2F9CF-1F5E-11D7-A6D3-00065B84435C}',
    '{CCF05DD7-4A87-11D7-A6E2-00065B84435C}'
)
# 清掉伪 GUID（533C5E0E 非键盘分类，当初误写）
foreach ($side in 'Category\Category', 'Category\Item') {
    Remove-Item "$tip\$side\{533C5E0E-5AC0-4ABD-B6F1-251B82B7BE7D}" -Recurse -Force -ErrorAction SilentlyContinue
}
# 8 分类两层写入：Category\Category\{cat}\{clsid} + Category\Item\{clsid}\{cat}
foreach ($cat in $cats) {
    $k1 = $hive.CreateSubKey("SOFTWARE\Microsoft\CTF\TIP\$clsid\Category\Category\$cat")
    [void]$k1.CreateSubKey($clsid)
    $k1.Close()
    $k2 = $hive.CreateSubKey("SOFTWARE\Microsoft\CTF\TIP\$clsid\Category\Item\$clsid")
    [void]$k2.CreateSubKey($cat)
    $k2.Close()
}
Write-Host "  已写 $($cats.Count) 个分类（两层子键）+ 清除伪 533C5E0E"

# Description 乱码尾巴重写
$lpKey = $hive.OpenSubKey("SOFTWARE\Microsoft\CTF\TIP\$clsid\LanguageProfile\0x00000804\{8F5C2A11-3E77-4B9C-A1D4-9E0B7C2F5A88}", $true)
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
