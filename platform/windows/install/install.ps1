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
# 查找顺序：① 发行包自身目录（绿色分发：install.ps1 与 DLL 同级）
#          ② 仓库构建输出（开发机直装）
#          ③ 开发机绝对路径（历史兜底）
$dll = Join-Path $PSScriptRoot 'hufu_tsf.dll'
if (-not (Test-Path $dll)) { $dll = Join-Path $PSScriptRoot '..\target\release\hufu_tsf.dll' }
if (-not (Test-Path $dll)) { $dll = 'E:\DSH-KF\hufu\platform\windows\target\release\hufu_tsf.dll' }
$dll = [System.IO.Path]::GetFullPath($dll)
if (-not (Test-Path $dll)) { throw "找不到 hufu_tsf.dll：$dll（发行包内应与本脚本同级；开发机构建：cd platform/windows; cargo build --release）" }
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

# ── 发行包位置登记（server 自愈拉起的跨架构锚点）──────────────
# DLL 在 32 位宿主里注册于 SysWOW64（exe 旁无 server），靠
# HKCU\Software\HuFu\InstallDir 找回包内 hufu-server.exe（ipc.rs
# ensure_server 的候选 2，读 64 位视图，x86/x64 进程一致）。
# 仅当本脚本运行于发行包内（旁边就是 server）时写入。
# 【位置注意】必须在 Set-Reg 函数定义之后（曾在函数定义前调用，
# $ErrorActionPreference=Stop 下脚本静默早退——安装只跑了一行的教训）。
$pkgServer = Join-Path $PSScriptRoot 'hufu-server.exe'
if (Test-Path $pkgServer) {
    Set-Reg 'HKCU:\Software\HuFu' 'InstallDir' $PSScriptRoot
    Write-Host "✓ InstallDir 已登记: $PSScriptRoot"
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

# ── 32 位宿主支持（WoW64：Pain 打器等 32 位进程）─────────────
# 32 位进程无法加载 x64 COM DLL；照虎爪/小狼毫双视图模式在
# WOW6432Node 再挂一份 InprocServer32 指向 32 位 DLL。
# 32 位 DLL 定位：优先发行包同目录，其次 i686 构建输出。
$dll32src = Join-Path $PSScriptRoot 'hufu_tsf32.dll'
if (-not (Test-Path $dll32src)) { $dll32src = Join-Path $PSScriptRoot '..\target\i686-pc-windows-gnu\release\hufu_tsf.dll' }
$dll32src = [System.IO.Path]::GetFullPath($dll32src)
if (Test-Path $dll32src) {
    # 32 位进程的 System32 重定向视图：SysWOW64\SystemIME\HuFu\
    $dir32 = "$env:SystemRoot\SysWOW64\SystemIME\HuFu"
    New-Item -ItemType Directory -Force $dir32 | Out-Null
    $dll32 = Join-Path $dir32 'hufu_tsf32.dll'
    Copy-Item $dll32src $dll32 -Force
    $wow = "HKLM:\SOFTWARE\Classes\WOW6432Node\CLSID\$CLSID"
    Set-Reg $wow '(default)' 'HuFu TSF Service'
    Set-Reg "$wow\InProcServer32" '(default)' $dll32
    Set-Reg "$wow\InProcServer32" 'ThreadingModel' 'Apartment'
    Write-Host "✓ 32 位宿主支持已注册: $dll32"
} else {
    Write-Host '⚠ 未找到 32 位 DLL（hufu_tsf32.dll / i686 构建），跳过 WoW64 注册' -ForegroundColor Yellow
}

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
# 定位同 DLL：发行包目录优先（smoke exe 与脚本同级），其次构建输出。
$smoke = Join-Path $PSScriptRoot 'hufu-tsf-smoke.exe'
if (-not (Test-Path $smoke)) { $smoke = Join-Path $PSScriptRoot '..\target\release\hufu-tsf-smoke.exe' }
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
Write-Host '安装完成。' -ForegroundColor Green
Write-Host '  · 输入引擎（hufu-server）会在首次切换到虎符时自动拉起，无需手动启动'
Write-Host '  · Win+空格 切到「HuFu 虎符输入法」即可使用'
Write-Host '  · 设置页: 双击 安装目录里的 设置.bat（或托盘图标右键）'
Write-Host ''
Write-Host '卸载: .\uninstall.ps1'
Stop-Transcript | Out-Null
