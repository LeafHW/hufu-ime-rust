# 一次性清场（提权）：杀 dev server / SystemIME 换最新 / 清 C 盘残留
$ErrorActionPreference = 'Continue'
Write-Output '== 杀 hufu-server（含提权 dev 残留）=='
Get-Process hufu-server -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1
Get-Process hufu-server -ErrorAction SilentlyContinue | ForEach-Object { taskkill /F /PID $_.Id 2>&1 | Out-Null }
Write-Output "server 剩余: $(@(Get-Process hufu-server -ErrorAction SilentlyContinue).Count)"

Write-Output '== SystemIME DLL 更新为最新门控版 =='
$sysdir = 'C:\Windows\SystemIME\HuFu'
$sysdll = "$sysdir\hufu_tsf.dll"
$src = 'E:\DSH-KF\hufu-发行\HuFu虎符输入法-安装包\hufu_tsf.dll'
New-Item -ItemType Directory -Path $sysdir -Force | Out-Null
try {
    Copy-Item $src $sysdll -Force
} catch {
    $n = 1; while (Test-Path "$sysdll.old$n") { $n++ }
    Rename-Item $sysdll "hufu_tsf.dll.old$n" -Force
    Copy-Item $src $sysdll -Force
    Write-Output "（旧 DLL 占用，腾位 .old$n）"
}
icacls $sysdir /grant 'ALL APPLICATION PACKAGES:(OI)(CI)RX' | Out-Null
icacls $sysdll /grant 'ALL APPLICATION PACKAGES:RX' | Out-Null
$h1 = (Get-FileHash $sysdll -Algorithm MD5).Hash
$h2 = (Get-FileHash $src -Algorithm MD5).Hash
Write-Output "SystemIME = 最新: $($h1 -eq $h2)"

Write-Output '== 清 C 盘旧残留 =='
Remove-Item "$env:LOCALAPPDATA\HuFu" -Recurse -Force -ErrorAction SilentlyContinue
Get-ChildItem "$env:LOCALAPPDATA\HuFu" -ErrorAction SilentlyContinue | ForEach-Object {
    $n = 1; while (Test-Path "$($_.FullName).old$n") { $n++ }
    Rename-Item $_.FullName "$($_.Name).old$n" -Force
}
Write-Output "LOCALAPPDATA HuFu 残留: $(@(Get-ChildItem "$env:LOCALAPPDATA\HuFu" -ErrorAction SilentlyContinue).Count) 项"

Write-Output '== 清 ProgramData 旧诊断 =='
Remove-Item 'C:\ProgramData\HuFu\diag' -Recurse -Force -ErrorAction SilentlyContinue
Write-Output '== 清场完成 =='
