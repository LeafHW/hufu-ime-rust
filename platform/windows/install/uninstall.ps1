# HuFu 虎符输入法 — 卸载脚本（卸载.bat 提权调用）
# 【2026-09-07 残留清除强化】一次跑完全清：
#   每用户注册+语言列表+自启+快捷方式 + 机器级注册+分类库
#   + SystemIME（含 .oldN 腾位链，x64/SysWOW64 双位）
#   + ProgramData + %LOCALAPPDATA% 早期版本数据 + 安装目录本体。
# 关键顺序：先杀 ctfmon 再清注册（后杀会在重启时用缓存档案重建
# HKCU TIP——实测残留根因）；全部清完最后再拉起 ctfmon。
param([switch]$NoHKLM)   # 测试用：仅每用户清理（不提权、不动 HKLM）

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

# 0) 【顺序关键】先杀 ctfmon/server 再清注册（防档案重建）
Get-Process hufu-server -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Stop-Process -Name ctfmon -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1

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

# 3) 删自启
Remove-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' 'HuFu' -ErrorAction SilentlyContinue

# 4) HKCU 注册表清理
Remove-Item "HKCU:\Software\Microsoft\CTF\TIP\$CLSID" -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item "HKCU:\Software\Classes\CLSID\$CLSID" -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item 'HKCU:\Software\HuFu' -Recurse -Force -ErrorAction SilentlyContinue   # InstallDir 记录（原地安装模式）
$asm = 'HKCU:\Software\Microsoft\CTF\SortOrder\AssemblyItem\0x00000804\{34745C63-B2F0-4784-8B67-5E12C8701A31}'
Get-ChildItem $asm -ErrorAction SilentlyContinue | Where-Object {
    (Get-ItemProperty $_.PSPath -ErrorAction SilentlyContinue).CLSID -eq $CLSID
} | Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
# 开始菜单快捷方式（通配双保险：精确名+*HuFu* 扫描）
Remove-Item "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\HuFu 虎符输入法设置.lnk" -Force -ErrorAction SilentlyContinue
Get-ChildItem "$env:APPDATA\Microsoft\Windows\Start Menu\Programs" -Filter '*HuFu*' -ErrorAction SilentlyContinue |
    Remove-Item -Force -Recurse -ErrorAction SilentlyContinue
# 【早期版本残留】%LOCALAPPDATA%\HuFu（旧版数据目录——绿色化前版本使用）
$legacyLocal = Join-Path $env:LOCALAPPDATA 'HuFu'
if (Test-Path $legacyLocal) {
    Remove-Item $legacyLocal -Recurse -Force -ErrorAction SilentlyContinue
    if (Test-Path $legacyLocal) { Write-Host '· %LOCALAPPDATA%\HuFu 部分被占用，重启后再跑一次卸载可清' }
    else { Write-Host '已清早期版本数据: %LOCALAPPDATA%\HuFu' }
}

# 5) HKLM 清理（提权时；普通用户跳过——无害残留）
if ($hklm) {
    Remove-Item "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$CLSID" -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item "HKLM:\SOFTWARE\Classes\CLSID\$CLSID" -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item "HKLM:\SOFTWARE\WOW6432Node\Microsoft\CTF\TIP\$CLSID" -Recurse -Force -ErrorAction SilentlyContinue
    # 32 位 COM 双视图 + SysWOW64 副本（32 位宿主支持）
    Remove-Item "HKLM:\SOFTWARE\Classes\WOW6432Node\CLSID\$CLSID" -Recurse -Force -ErrorAction SilentlyContinue
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
    # 【腾位链清除】SystemIME 当前目录 + 历史 .oldN 腾位（升级一次攒
    # 一份的根源；卸载语义=全清，占用项改名腾位、重启后系统回收）
    foreach ($base in @("$env:SystemRoot\SystemIME", "$env:SystemRoot\SysWOW64\SystemIME")) {
        Get-ChildItem $base -Force -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -like 'HuFu*' } |
            ForEach-Object {
                Get-ChildItem $_.FullName -Recurse -Force -ErrorAction SilentlyContinue | ForEach-Object { $_.Attributes = 'Normal' }
                Remove-Item $_.FullName -Recurse -Force -ErrorAction SilentlyContinue
            }
        $sysdir = Join-Path $base 'HuFu'
        if (Test-Path $sysdir) {
            $n = 1; while (Test-Path "$sysdir.old$n") { $n++ }
            Rename-Item $sysdir "HuFu.old$n" -Force -ErrorAction SilentlyContinue
            Write-Host "· DLL 被占用，SystemIME 已腾位 HuFu.old$n（重启后自动可删）"
        }
    }
    # 诊断画像（load-*/act-*.txt）+ 删后复查（实测偶发删除后又被写回）
    Remove-Item 'C:\ProgramData\HuFu' -Recurse -Force -ErrorAction SilentlyContinue
    if (Test-Path 'C:\ProgramData\HuFu') {
        Remove-Item 'C:\ProgramData\HuFu' -Recurse -Force -ErrorAction SilentlyContinue
    }
}
# 临时安装/提权日志（%TEMP%\hufu-*.log）
Get-ChildItem "$env:TEMP\hufu-*.log" -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue

# 6) 【HKCU 二次复查】ctfmon 已在步骤 0 关闭，此处防外部因素重建
if (Test-Path "HKCU:\Software\Microsoft\CTF\TIP\$CLSID") {
    Remove-Item "HKCU:\Software\Microsoft\CTF\TIP\$CLSID" -Recurse -Force -ErrorAction SilentlyContinue
}

# 7) 【数据保留 2026-09-07 用户拍板】卸载=解除注册+停进程，安装目录
#    与其中数据（配置/用户调整/码表导出）一概不删——去留由用户决定。
#    卸载完成输出里给出数据位置与手动清理指引。

# 8) 拉起 ctfmon（系统输入法框架恢复）
Start-Process ctfmon -ErrorAction SilentlyContinue

Write-Host ''
Write-Host 'OK 卸载完成（注册表+SystemIME+ProgramData+早期残留已清；输入法已移除）' -ForegroundColor Green
if (-not $hklm) {
    Write-Host '  · 本次为每用户模式（-NoHKLM）：机器级注册未动。' -ForegroundColor Yellow
    Write-Host '    完整卸载请直接运行 卸载.bat（自动提权）。'
}
Write-Host '  · 腾位项（.oldN）重启后自动可删；下次安装也会自动清理'
Write-Host '  · 无需重启/注销；已开应用里的输入法随应用关闭即消失'
# 【数据保留指引】安装目录不删——用户数据去留由用户决定
if ($inst -and (Test-Path $inst)) {
    Write-Host ''
    Write-Host "  数据已保留（未删除任何文件），安装目录：$inst"
    $dataTips = @()
    if (Test-Path "$inst\数据\config.json") { $dataTips += '数据\config.json（配置：方案/皮肤/习惯设置）' }
    $adjN = @(Get-ChildItem "$inst\码表" -Recurse -Filter '用户调整.txt' -File -ErrorAction SilentlyContinue).Count
    if ($adjN -gt 0) { $dataTips += "码表\*\用户调整.txt（词频学习记录，$adjN 个方案）" }
    if (Test-Path "$inst\码表导出") {
        $expN = @(Get-ChildItem "$inst\码表导出" -Recurse -File -ErrorAction SilentlyContinue).Count
        if ($expN -gt 0) { $dataTips += "码表导出\（导出的码表快照，$expN 个文件）" }
    }
    if ($dataTips.Count -gt 0) {
        Write-Host '  可存档的数据：'
        $dataTips | ForEach-Object { Write-Host "    · $_" }
        Write-Host '  （重装同版后再放回原位即可恢复）'
    }
    Write-Host "  确认不要了，手动删整个文件夹即彻底清理：$inst"
}
