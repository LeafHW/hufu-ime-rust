# HuFu 卸载（需要管理员）：删除全部注册链。
$ErrorActionPreference = 'Continue'

$CLSID   = '{8F5C2A10-3E77-4B9C-A1D4-9E0B7C2F5A88}'
$PROFILE = '{8F5C2A11-3E77-4B9C-A1D4-9E0B7C2F5A88}'
$tipStr  = "0804:$CLSID$PROFILE"

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
           ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    $ps = Join-Path $PSHOME 'powershell.exe'
    Start-Process $ps -Verb RunAs -ArgumentList "-ExecutionPolicy Bypass -File `"$PSCommandPath`""
    exit
}

# 1) 语言列表移除（官方 API 走起——登记已删前先移，避免残留提示）
$list = Get-WinUserLanguageList
$zh = $list | Where-Object { $_.LanguageTag -like 'zh*' } | Select-Object -First 1
if ($zh -and ($zh.InputMethodTips -contains $tipStr)) {
    $zh.InputMethodTips.Remove($tipStr) | Out-Null
    Set-WinUserLanguageList $list -Force
}

# 2) 切换器装配项
Remove-Item 'HKCU:\Software\Microsoft\CTF\SortOrder\AssemblyItem\0x00000804\{34745C63-B2F0-4784-8B67-5E12C8701A31}\00000003' -Force -ErrorAction SilentlyContinue

# 3) User Profile 直写值（兜底残留）
Remove-ItemProperty -Path 'HKCU:\Control Panel\International\User Profile\zh-Hans-CN' -Name $tipStr -ErrorAction SilentlyContinue

# 4) COM + CTF 注册树
Remove-Item "HKLM:\SOFTWARE\Classes\CLSID\$CLSID" -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$CLSID" -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item "HKCU:\Software\Classes\CLSID\$CLSID" -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item "HKCU:\Software\Microsoft\CTF\TIP\$CLSID" -Recurse -Force -ErrorAction SilentlyContinue

# 5) ctfmon 重读
Stop-Process -Name ctfmon -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2
Start-Process ctfmon -ErrorAction SilentlyContinue

Write-Host '✓ 已卸载（ctfmon 已重启，立即生效）'
