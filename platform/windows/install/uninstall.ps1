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

$inst = Split-Path -Parent $MyInvocation.MyCommand.Path   # 安装目录（脚本所在处）

# 1) 语言列表移除
$tipStr = "0804:$CLSID$PROFILE"
$list = Get-WinUserLanguageList
foreach ($l in $list) {
    if ($l.InputMethodTips -contains $tipStr) {
        $l.InputMethodTips.Remove($tipStr) | Out-Null
        Set-WinUserLanguageList $list -Force -WarningAction SilentlyContinue
        break
    }
}

# 2) msctf 原生档案注销（机器级；仅在完整卸载（管理员）时做——
#    每用户卸载（-NoHKLM 或普通权限）不动 msctf 档案：它注册时
#    需要管理员重建，清了会导致无提权重装后输入法失忆）
$smoke = Join-Path $inst 'hufu-tsf-smoke.exe'
if ($hklm -and (Test-Path $smoke)) { & $smoke unreg }

# 3) 停 server、删自启
Get-Process hufu-server -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Remove-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' 'HuFu' -ErrorAction SilentlyContinue

# 4) HKCU 注册表清理
Remove-Item "HKCU:\Software\Microsoft\CTF\TIP\$CLSID" -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item "HKCU:\Software\Classes\CLSID\$CLSID" -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item 'HKCU:\Software\HuFu' -Recurse -Force -ErrorAction SilentlyContinue   # InstallDir 记录（原地安装模式）
$asm = 'HKCU:\Software\Microsoft\CTF\SortOrder\AssemblyItem\0x00000804\{34745C63-B2F0-4784-8B67-5E12C8701A31}'
Get-ChildItem $asm -ErrorAction SilentlyContinue | Where-Object {
    (Get-ItemProperty $_.PSPath -ErrorAction SilentlyContinue).CLSID -eq $CLSID
} | Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
# 开始菜单快捷方式（install.ps1 创建的「HuFu 虎符输入法设置.lnk」）
Remove-Item "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\HuFu 虎符输入法设置.lnk" -Force -ErrorAction SilentlyContinue

# 5) HKLM 清理（提权时；普通用户跳过——无害残留）
if ($hklm) {
    Remove-Item "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$CLSID" -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item "HKLM:\SOFTWARE\Classes\CLSID\$CLSID" -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item "HKLM:\SOFTWARE\WOW6432Node\Microsoft\CTF\TIP\$CLSID" -Recurse -Force -ErrorAction SilentlyContinue
    # 32 位 COM 双视图 + SysWOW64 副本（32 位宿主支持）
    Remove-Item "HKLM:\SOFTWARE\Classes\WOW6432Node\CLSID\$CLSID" -Recurse -Force -ErrorAction SilentlyContinue
    $sysdir32 = "$env:SystemRoot\SysWOW64\SystemIME\HuFu"
    if (Test-Path $sysdir32) {
        Remove-Item $sysdir32 -Recurse -Force -ErrorAction SilentlyContinue
        if (Test-Path $sysdir32) {
            $m = 1; while (Test-Path "$sysdir32.old$m") { $m++ }
            Rename-Item $sysdir32 "HuFu.old$m" -Force -ErrorAction SilentlyContinue
        }
    }
    # 全局分类库条目（8 项 TIP 分类 + MASTER，安装器双写对应清理）
    $lmCat = 'HKLM:\SOFTWARE\Microsoft\CTF\Category'
    $catAll = @(
        '{046B8C80-1647-40F7-9B21-B93B81AABC1B}', '{13A016DF-560B-46CD-947A-4C3AF1E0E35D}',
        '{25504FB4-7BAB-4BC1-9C69-CF81890F0EF5}', '{34745C63-B2F0-4784-8B67-5E12C8701A31}',
        '{364215D9-75BC-11D7-A6EF-00065B84435C}', '{49D2F9CE-1F5E-11D7-A6D3-00065B84435C}',
        '{49D2F9CF-1F5E-11D7-A6D3-00065B84435C}', '{CCF05DD7-4A87-11D7-A6E2-00065B84435C}',
        '{533C5E0E-5AC0-4ABD-B6F1-251B82B7BE7D}'
    )
    foreach ($c in $catAll) {
        Remove-Item "$lmCat\Category\$c\$CLSID" -Recurse -Force -ErrorAction SilentlyContinue
    }
    Remove-Item "$lmCat\Item\$CLSID" -Recurse -Force -ErrorAction SilentlyContinue
    # SystemIME 副本；被运行中宿主占用时腾位改名（下次系统清理/重启后消失）
    $sysdir = 'C:\Windows\SystemIME\HuFu'
    if (Test-Path $sysdir) {
        Remove-Item $sysdir -Recurse -Force -ErrorAction SilentlyContinue
        if (Test-Path $sysdir) {
            $n = 1; while (Test-Path "$sysdir.old$n") { $n++ }
            Rename-Item $sysdir "HuFu.old$n" -Force -ErrorAction SilentlyContinue
        }
    }
    # 诊断画像（load-*/act-*.txt，几十 KB）
    Remove-Item 'C:\ProgramData\HuFu' -Recurse -Force -ErrorAction SilentlyContinue
}
# 临时安装/提权日志（%TEMP%\hufu-*.log）
Get-ChildItem "$env:TEMP\hufu-*.log" -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue

# 6) 刷新宿主
Stop-Process -Name TextInputHost, ShellExperienceHost, ctfmon -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1
Start-Process ctfmon -ErrorAction SilentlyContinue

Write-Host ''
Write-Host 'OK 卸载完成（注册表已清）' -ForegroundColor Green
Write-Host "  现在把整个文件夹删除即完成卸载：$inst"
Write-Host "  （绿色模式：程序原地在安装目录运行，删目录即彻底卸载，"
Write-Host "    C 盘无数据残留）"
$legacy = Join-Path $env:LOCALAPPDATA 'HuFu'
if (Test-Path $legacy) {
    Write-Host "  · 旧版目录仍在：$legacy（可一并删除）"
}
# 机器级残留说明（普通权限卸载清不掉 HKLM/SystemIME——机器级注册
# 会让系统重新激活虎符：切换器可见、可打字。彻底卸载必须提权一次）
$lmLeft = (Test-Path "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$CLSID") -or (Test-Path 'C:\Windows\SystemIME\HuFu')
if ($lmLeft) {
    Write-Host '  ⚠ 本机仍有机器级注册（HKLM 档案 + SystemIME DLL）——虎符会继续'
    Write-Host '    可用（切换器可见、可打字）！彻底卸载请右键「卸载.bat」选'
    Write-Host '    「以管理员身份运行」再跑一次（清机器级注册）。'
}
# 文件夹删除受阻提示（宿主占用 → 注销/重启后删除）
Write-Host '  · 若删除文件夹时提示「文件被占用」：注销或重启电脑后再删一次'
Write-Host '    即可完全删净（个别 DLL 会被系统输入宿主短暂占用）。'
Write-Host '  · 无需重启/注销；已开应用里的输入法随应用关闭即消失'
