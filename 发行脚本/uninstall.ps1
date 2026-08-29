# HuFu 虎符输入法 — 卸载脚本（管理员可选；卸载.bat 直接调用）
# 每用户部分（HKCU/语言列表/自启/server）无管理员亦可完整卸载；
# HKLM 机器级键：管理员组则提权清一次，普通用户跳过（无害残留）。
param([switch]$NoHKLM)   # 测试用：强制每用户模式（跳过 HKLM 与提权）

$ErrorActionPreference = 'Continue'

$CLSID   = '{8F5C2A10-3E77-4B9C-A1D4-9E0B7C2F5A88}'
$PROFILE = '{8F5C2A11-3E77-4B9C-A1D4-9E0B7C2F5A88}'

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
           ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
$inAdminGroup = $false
try {
    $inAdminGroup = (whoami /groups /fo csv | Select-String 'S-1-5-32-544').Count -gt 0
} catch {}
$hklm = $isAdmin
if (-not $isAdmin -and -not $NoHKLM -and $inAdminGroup) {
    $ps = Join-Path $PSHOME 'powershell.exe'
    Start-Process $ps -Verb RunAs -ArgumentList "-ExecutionPolicy Bypass -File `"$PSCommandPath`"" -Wait
    exit
}

$inst = Join-Path $env:LOCALAPPDATA 'HuFu'

# 1) 语言列表移除
$tipStr = "0804:$CLSID$PROFILE"
$list = Get-WinUserLanguageList
foreach ($l in $list) {
    if ($l.InputMethodTips -contains $tipStr) {
        $l.InputMethodTips.Remove($tipStr) | Out-Null
        Set-WinUserLanguageList $list -Force
        break
    }
}

# 2) msctf 原生档案注销
$smoke = Join-Path $inst 'hufu-tsf-smoke.exe'
if (Test-Path $smoke) { & $smoke unreg }

# 3) 停 server、删自启
Get-Process hufu-server -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Remove-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' 'HuFu' -ErrorAction SilentlyContinue

# 4) HKCU 注册表清理
Remove-Item "HKCU:\Software\Microsoft\CTF\TIP\$CLSID" -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item "HKCU:\Software\Classes\CLSID\$CLSID" -Recurse -Force -ErrorAction SilentlyContinue
$asm = 'HKCU:\Software\Microsoft\CTF\SortOrder\AssemblyItem\0x00000804\{34745C63-B2F0-4784-8B67-5E12C8701A31}'
Get-ChildItem $asm -ErrorAction SilentlyContinue | Where-Object {
    (Get-ItemProperty $_.PSPath -ErrorAction SilentlyContinue).CLSID -eq $CLSID
} | Remove-Item -Recurse -Force -ErrorAction SilentlyContinue

# 5) HKLM 清理（提权时；普通用户跳过——无害残留）
if ($hklm) {
    Remove-Item "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$CLSID" -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item "HKLM:\SOFTWARE\Classes\CLSID\$CLSID" -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item "HKLM:\SOFTWARE\WOW6432Node\Microsoft\CTF\TIP\$CLSID" -Recurse -Force -ErrorAction SilentlyContinue
}

# 6) 刷新宿主
Stop-Process -Name TextInputHost, ShellExperienceHost, ctfmon -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1
Start-Process ctfmon -ErrorAction SilentlyContinue

Write-Host ''
Write-Host 'OK 卸载完成（注册表已清）' -ForegroundColor Green
Write-Host "  程序与数据目录保留在：$inst"
Write-Host "  确认不再使用可手动删除整个文件夹。"
if (-not $hklm) { Write-Host '  （每用户卸载；HKLM 无写入，无残留）' }
Write-Host '  · 无需重启/注销；已开应用里的输入法随应用关闭即消失'
