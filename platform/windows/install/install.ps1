# HuFu 虎符输入法 — 安装脚本（双阶段：普通权限主导，提权只做注册）
# - 文件/HKCU/语言列表/自启/server 永远普通权限执行（server 提权启动会锁管道 ACL，
#   导致所有普通应用连不上→只能打字母，2026-08-29 实测教训）。
# - HKLM 机器级键 + msctf 原生登记（本机实测需提权才 0x00000000）交给一次 UAC 的
#   提权子进程（-PhaseElevated），日志回流本窗口可见。
# - -NoHKLM：完全跳过提权（无管理员机器的每用户安装；msctf 登记尽力而为）。
param([switch]$NoHKLM, [switch]$PhaseElevated)

$ErrorActionPreference = 'Continue'

$CLSID   = '{8F5C2A10-3E77-4B9C-A1D4-9E0B7C2F5A88}'
$PROFILE = '{8F5C2A11-3E77-4B9C-A1D4-9E0B7C2F5A88}'
$TFCAT_KBD = '{533C5E0E-5AC0-4ABD-B6F1-251B82B7BE7D}'

$src  = Split-Path -Parent $MyInvocation.MyCommand.Path
# 【原地安装（绿色模式）】程序直接在安装目录运行，不拷贝到 %LOCALAPPDATA%：
# C 盘零数据占用；卸载 = 跑 卸载.bat 后整个删除本文件夹。
$inst = $src
$data = Join-Path $inst '数据'
$dll  = Join-Path $inst 'hufu_tsf.dll'
$exe  = Join-Path $inst 'hufu-server.exe'
$icon = Join-Path $inst '图标.ico'
# SystemIME 副本：开始菜单搜索/任务栏等 SystemApps 打包进程读不了用户目录
# （%LOCALAPPDATA% 无 ALL APPLICATION PACKAGES 权限），DLL 必须住在
# C:\Windows\SystemIME（系统输入法同款目录，打包进程可读）——2026-08-29
# 实测：SearchHost 不加载用户目录 DLL → 搜索框字母直通。
$sysdir = 'C:\Windows\SystemIME\HuFu'
$sysdll = Join-Path $sysdir 'hufu_tsf.dll'

function Set-Reg([string]$path, [string]$name, [string]$val) {
    if (-not (Test-Path $path)) { New-Item -Path $path -Force | Out-Null }
    if ($name -eq '(default)') {
        $k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey(($path -replace '^[^:\\]+:\\', ''), $true)
        if ($k) { $k.SetValue('', $val); $k.Close() }
        else { $ki = Get-Item $path; $ki.SetValue('', $val) }
    } else { Set-ItemProperty -Path $path -Name $name -Value $val -Type String }
}
function Set-RegDWord([string]$path, [string]$name, [int]$val) {
    if (-not (Test-Path $path)) { New-Item -Path $path -Force | Out-Null }
    Set-ItemProperty -Path $path -Name $name -Value $val -Type DWord
}
function Set-RegHKLM([string]$path, [string]$name, [string]$val) {
    if (-not (Test-Path $path)) { New-Item -Path $path -Force | Out-Null }
    if ($name -eq '(default)') {
        $k = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey(($path -replace '^HKLM:\\', ''), $true)
        if ($k) { $k.SetValue('', $val); $k.Close() }
    } else { Set-ItemProperty -Path $path -Name $name -Value $val -Type String }
}

# ═══ 提权子阶段：只做 HKLM 注册 + msctf 登记，绝不启动 server ═══
if ($PhaseElevated) {
    Write-Host '—— 提权阶段：SystemIME 副本（打包进程可读）——'
    New-Item -ItemType Directory -Path $sysdir -Force | Out-Null
    # DLL 可能被宿主进程加载（SearchHost 等长期占用）：改名腾位后拷贝。
    # （Windows 允许改名已加载的 DLL；旧副本留 .oldN，随系统清理。）
    try {
        Copy-Item $dll $sysdll -Force
    } catch {
        $n = 1
        while (Test-Path "$sysdll.old$n") { $n++ }
        Rename-Item $sysdll "hufu_tsf.dll.old$n" -Force
        Copy-Item $dll $sysdll -Force
        Write-Host "（旧 DLL 被占用，已腾位为 .old$n）"
    }
    if (-not (Test-Path $sysdll)) { throw "SystemIME DLL 拷贝失败：$sysdll" }
    icacls $sysdir /grant 'ALL APPLICATION PACKAGES:(OI)(CI)RX' | Out-Null
    icacls $sysdll /grant 'ALL APPLICATION PACKAGES:RX' | Out-Null
    Write-Host 'OK DLL → SystemIME'
    # ── 32 位宿主支持（WoW64：跟打器等 32 位进程）──
    # 32 位进程无法加载 x64 COM DLL；COM 查找顺序 HKCU（无重定向）
    # 失败后回退 HKLM 32 位视图（WOW6432Node）→ 命中 32 位 DLL。
    # 32 位 DLL 放 SysWOW64\SystemIME（32 位进程的 System32 视图）。
    $dll32 = Join-Path $PSScriptRoot 'hufu_tsf32.dll'
    if (Test-Path $dll32) {
        $dir32 = "$env:SystemRoot\SysWOW64\SystemIME\HuFu"
        New-Item -ItemType Directory -Path $dir32 -Force | Out-Null
        $sysdll32 = Join-Path $dir32 'hufu_tsf32.dll'
        try {
            Copy-Item $dll32 $sysdll32 -Force
        } catch {
            $n = 1
            while (Test-Path "$sysdll32.old$n") { $n++ }
            Rename-Item $sysdll32 "hufu_tsf32.dll.old$n" -Force
            Copy-Item $dll32 $sysdll32 -Force
        }
        icacls $dir32 /grant 'ALL APPLICATION PACKAGES:(OI)(CI)RX' | Out-Null
        icacls $sysdll32 /grant 'ALL APPLICATION PACKAGES:RX' | Out-Null
        $wow = "HKLM:\SOFTWARE\Classes\WOW6432Node\CLSID\$CLSID"
        Set-RegHKLM $wow '(default)' 'HuFu TSF Service'
        Set-RegHKLM "$wow\InprocServer32" '(default)' $sysdll32
        Set-RegHKLM "$wow\InprocServer32" 'ThreadingModel' 'Apartment'
        Write-Host 'OK 32 位 DLL → SysWOW64（跟打器等 32 位宿主可用）'
    } else {
        Write-Host '（未找到 hufu_tsf32.dll，跳过 32 位支持）' -ForegroundColor Yellow
    }
    # ── 升级清理（2026-09-06）：腾位残留与诊断日志 ──
    # 1) 历史腾位目录/文件（HuFu.oldN 目录、hufu_tsf.dll.oldN——此前
    #    「随系统清理」实际永不清，升级一次攒一份）。本次 DLL 已就位，
    #    旧的若仍被运行中的应用占用则跳过（下次安装/重启后再清）。
    foreach ($base in @("$env:SystemRoot\SystemIME", "$env:SystemRoot\SysWOW64\SystemIME")) {
        Get-ChildItem $base -Force -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -like 'HuFu.old*' } |
            ForEach-Object {
                try {
                    Get-ChildItem $_.FullName -Recurse -Force -ErrorAction SilentlyContinue | ForEach-Object { $_.Attributes = 'Normal' }
                    Remove-Item $_.FullName -Recurse -Force -ErrorAction Stop
                    Write-Host "清理腾位残留: $($_.FullName)"
                } catch {}
            }
        $hu = Join-Path $base 'HuFu'
        if (Test-Path $hu) {
            Get-ChildItem $hu -Filter '*.old*' -Force -ErrorAction SilentlyContinue | ForEach-Object {
                try { Remove-Item $_.FullName -Force -ErrorAction Stop; Write-Host "清理旧副本: $($_.Name)" } catch {}
            }
        }
    }
    # 2) 诊断日志（ProgramData\HuFu\diag——升级即清，日志无需跨版本保留）
    $diag = 'C:\ProgramData\HuFu\diag'
    if (Test-Path $diag) {
        $n = @(Get-ChildItem $diag -Force -ErrorAction SilentlyContinue).Count
        Get-ChildItem $diag -Force -ErrorAction SilentlyContinue | ForEach-Object {
            try { Remove-Item $_.FullName -Force -Recurse -ErrorAction Stop } catch {}
        }
        if ($n -gt 0) { Write-Host "清理诊断日志: $n 个" }
    }
    Write-Host '—— 提权阶段：HKLM 机器级注册 ——'
    $ips = "HKLM:\SOFTWARE\Classes\CLSID\$CLSID\InprocServer32"
    Set-RegHKLM "HKLM:\SOFTWARE\Classes\CLSID\$CLSID" '(default)' 'HuFu TSF Service'
    Set-RegHKLM $ips '(default)' $sysdll
    Set-RegHKLM $ips 'ThreadingModel' 'Apartment'
    $tip = "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$CLSID"
    Set-RegHKLM "$tip\Description" '(default)' 'HuFu 虎符输入法（虎码）'
    # 【TIP 全套分类（8 个，虎爪/主流 IME 同款）】切换器（尤其搜索框等
    # 打包宿主会话）按分类集合判定 TIP 可用性——只注册键盘一两个分类
    # 时 Win+空格 会跳过本输入法（用户实测「只能前 3 个来回切」）。
    # 双层写入：全局分类库 + TIP 树，两层均需齐全。
    $cats = @(
        '{046B8C80-1647-40F7-9B21-B93B81AABC1B}',
        '{13A016DF-560B-46CD-947A-4C3AF1E0E35D}',
        '{25504FB4-7BAB-4BC1-9C69-CF81890F0EF5}',
        '{34745C63-B2F0-4784-8B67-5E12C8701A31}',
        '{364215D9-75BC-11D7-A6EF-00065B84435C}',
        '{49D2F9CE-1F5E-11D7-A6D3-00065B84435C}',
        '{49D2F9CF-1F5E-11D7-A6D3-00065B84435C}',
        '{CCF05DD7-4A87-11D7-A6E2-00065B84435C}'
    )
    $lmCat = 'HKLM:\SOFTWARE\Microsoft\CTF\Category'
    foreach ($c in $cats) {
        New-Item -Path "$tip\Category\Category\$c\$CLSID" -Force | Out-Null
        New-Item -Path "$tip\Category\Item\$CLSID\$c" -Force | Out-Null
        New-Item -Path "$lmCat\Category\$c\$CLSID" -Force | Out-Null
        New-Item -Path "$lmCat\Item\$CLSID\$c" -Force | Out-Null
    }
    Write-Host 'OK TIP 分类 8 项已齐全（全局库 + TIP 树双层）'
    $lp = "$tip\LanguageProfile\0x00000804\$PROFILE"
    Set-RegHKLM $lp 'Description' 'HuFu 虎符输入法'
    Set-RegHKLM $lp 'Display Description' 'HuFu 虎符输入法'
    Set-ItemProperty -Path $lp -Name 'Enable' -Value 1 -Type DWord
    Set-RegHKLM $lp 'IconFile' $sysdll
    Set-ItemProperty -Path $lp -Name 'IconIndex' -Value 0 -Type DWord
    Write-Host 'OK HKLM 机器级已注册（指向 SystemIME）'
    Write-Host '—— 提权阶段：msctf 原生登记 ——'
    & (Join-Path $inst 'hufu-tsf-smoke.exe') reg $sysdll
    # 【顺序铁律·回写半边】完整安装下每用户段先写了 HKCU→安装目录 DLL
    # （当时 SystemIME 尚未建立）。此刻 SystemIME 已就位：HKCU COM 必须
    # 回写为 SystemIME 路径——否则打包进程（开始菜单/UWP）按 HKCU 优先
    # 解析到用户目录 DLL（无 ALL APPLICATION PACKAGES 读权限）→ 加载
    # 失败，开始菜单/UWP 打不了字（实测回归位）。
    $ipsCU = "HKCU:\Software\Classes\CLSID\$CLSID\InprocServer32"
    Set-ItemProperty -Path $ipsCU -Name '(default)' -Value $sysdll -EA SilentlyContinue
    (Get-Item $ipsCU).OpenSubKey('', $true).SetValue('', $sysdll)
    $chkCU = (Get-Item $ipsCU).GetValue('')
    if ($chkCU -ne $sysdll) { Write-Host "⚠ HKCU 回写校验失败：$chkCU" }
    else { Write-Host 'OK HKCU COM 已回写 → SystemIME（打包进程可读）' }
    Write-Host '提权阶段完成。'
    exit
}

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
           ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
$inAdminGroup = $false
try {
    $inAdminGroup = (whoami /groups /fo csv | Select-String 'S-1-5-32-544').Count -gt 0
} catch {}

Write-Host ''
Write-Host 'HuFu 虎符输入法 安装' -ForegroundColor Cyan
Write-Host "安装目录（原地运行）: $inst"

# ── 0) 安装位置安全性检查 + 旧布局检测 ──
if ($inst -like "$env:TEMP*") {
    Write-Host '⚠ 当前位于临时目录（会被系统清理，输入法将失效）！'
    Write-Host '  请把整个文件夹移到稳定位置（如 D:\HuFu）后重新运行本安装。'
    exit 1
}
$legacy = Join-Path $env:LOCALAPPDATA 'HuFu'
if (Test-Path (Join-Path $legacy 'hufu-server.exe')) {
    $mb = [math]::Round((Get-ChildItem $legacy -Recurse -File -ErrorAction SilentlyContinue | Measure-Object Length -Sum).Sum / 1MB)
    Write-Host "· 检测到旧版安装目录：$legacy（约 ${mb}MB）"
    Write-Host '  本次安装完成后确认输入法正常，即可删除该目录释放空间。'
}

# ── 1) 文件检查（原地运行：不拷贝；数据/模型直接在本目录使用）──
if (-not (Test-Path $dll)) { Write-Host "✗ 缺少 $dll，请完整解压安装包后重试"; exit 1 }
if (-not (Test-Path $exe)) { Write-Host "✗ 缺少 $exe，请完整解压安装包后重试"; exit 1 }
Write-Host 'OK 文件就位（原地运行，不占用 C 盘额外空间）'

# ── 1.5) 记录安装目录（DLL 自愈链 / 卸载器读取）──
Set-Reg 'HKCU:\Software\HuFu' 'InstallDir' $inst

# ── 2) HKCU COM + TIP 键树注册（每用户；COM 解析 HKCU 优先）──
# DLL 路径：优先 SystemIME 副本（打包进程可读，开始菜单搜索/UWP 可用）；
# 未提权安装（无 SystemIME 副本）时退回用户目录——此时开始菜单搜索
# 框不可用（SystemApps 进程读不了用户目录），记事本等普通应用不受影响。
$dllReg = if (Test-Path $sysdll) { $sysdll } else { $dll }
$ipsUser = "HKCU:\Software\Classes\CLSID\$CLSID\InprocServer32"
# 【顺序铁律】regsvr32 必须先跑：DllRegisterServer 会把 HKCU CLSID
# 默认值覆盖为 DLL 自身路径（安装目录，AppContainer 宿主读不了——
# 开始菜单/UWP 因此打不了字）。之后我们重写为 $dllReg（SystemIME
# 副本，打包进程可读），最终值必须落 SystemIME。
regsvr32 /s $dll
Set-Reg "HKCU:\Software\Classes\CLSID\$CLSID" '(default)' 'HuFu TSF Service'
Set-Reg $ipsUser '(default)' $dllReg
Set-Reg $ipsUser 'ThreadingModel' 'Apartment'
$finalDll = [string](Get-Item "HKCU:\Software\Classes\CLSID\$CLSID\InprocServer32").GetValue('')
if ($finalDll -ne $dllReg) { Set-Reg $ipsUser '(default)' $dllReg }  # 双保险

# 【HKCU TIP 键树——切换器/搜索框的生命线，不可省】（dae3baf/23978b3
# 历史定案，afc2dfc 曾误删致 Win+空格 切不到虎符，实测回归后恢复）：
# Win+空格切换器只列「带键盘分类」的 TIP（Category 两层键）；语言档案
# 启用状态（LanguageProfile Enable）与切换器图标（IconFile）也在此树。
# regsvr32 写的副本 IconFile 指向安装目录 DLL（打包进程读不了），故
# 安装器直写一遍并统一指向 $dllReg——与 regsvr32 双保险，缺一不可。
$tipU = "HKCU:\Software\Microsoft\CTF\TIP\$CLSID"
Set-Reg $tipU '(default)' 'HuFu 输入法'
Set-Reg "$tipU\Description" '(default)' 'HuFu 虎符输入法（虎码）'
New-Item -Path "$tipU\Category\Category\$TFCAT_KBD\$CLSID" -Force | Out-Null
New-Item -Path "$tipU\Category\Item\$CLSID\$TFCAT_KBD" -Force | Out-Null
# 【键盘分类（34745C63）只写 HKLM 全局库】（dae3baf 定案：切换器按
# HKLM CTF\Category 识别键盘 TIP；提权段 RegisterCategory / oneshot
# 补全）——HKCU TIP 树里写键盘分类键会被 msctf 判非法周期删除
# （净室实测稳定复现：MASTER 键存活、34745C63 键必消失），勿双写。
$lpU = "$tipU\LanguageProfile\0x00000804\$PROFILE"
Set-Reg $lpU '(default)' 'HuFu 虎符输入法'
Set-RegDWord $lpU 'Enable' 1          # DWORD（msctf 标准）
Set-RegDWord $lpU 'IconIndex' 0
Set-Reg $lpU 'IconFile' $dllReg       # SystemIME 副本（打包进程可读）
Set-Reg $lpU 'Icon' "$dllReg,0"       # 图标双写（3544b61：Index+字符串两制式）
Write-Host "OK HKCU COM + TIP 键树已注册（DLL → $dllReg）"

# ── 3) 提权注册（HKLM + msctf；一次 UAC，日志回流本窗口）──
if (-not $NoHKLM) {
    if ($isAdmin) {
        & $PSCommandPath -PhaseElevated
    } elseif ($inAdminGroup) {
        Write-Host '（弹出 UAC：机器级注册 + msctf 登记，请点「是」）'
        $elog = Join-Path $env:TEMP 'hufu-install-elevated.log'
        $ps = "$env:WINDIR\System32\WindowsPowerShell\v1.0\powershell.exe"
        $arg = "-NoProfile -ExecutionPolicy Bypass -Command `"[Console]::OutputEncoding=[Text.Encoding]::UTF8; & '$PSCommandPath' -PhaseElevated *> '$elog'`""
        Start-Process $ps -Verb RunAs -ArgumentList $arg -Wait
        if (Test-Path $elog) {
            # smoke 输出为 UTF-8 字节：按 UTF-8 读回（默认 ANSI 会乱码）
            Get-Content $elog -Encoding UTF8 | ForEach-Object { Write-Host "  $_" }
        }
        # 【顺序铁律·校验半边】提权段已回写 HKCU→SystemIME；此处回读双保险
        $ipsCU = "HKCU:\Software\Classes\CLSID\$CLSID\InprocServer32"
        $sysdllChk = 'C:\Windows\SystemIME\HuFu\hufu_tsf.dll'
        if ((Test-Path $ipsCU) -and ((Get-Item $ipsCU).GetValue('') -ne $sysdllChk)) {
            (Get-Item $ipsCU).OpenSubKey('', $true).SetValue('', $sysdllChk)
            Write-Host "  · HKCU COM 校正 → SystemIME（原值 $((Get-Item $ipsCU).GetValue(''))）"
        }
    } else {
        Write-Host '⚠ 无管理员权限：msctf 输入法注册需机器级写入（TSF 平台限制，'
        Write-Host '  同类输入法如虎爪同样要求管理员）。文件与语言列表已铺好，'
        Write-Host '  但输入法要能用，需以管理员身份重跑本安装器完成登记。'
        & (Join-Path $inst 'hufu-tsf-smoke.exe') reg $icon
    }
} else {
        # 本机已有机器级底座（SystemIME/HKLM 档案/8 分类）时，每用户装
        # 即全功能（含开始菜单/UWP）；全新机器首次安装仍需提权一次。
        $hasBase = (Test-Path $sysdll) -and (Test-Path "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$CLSID")
        if ($hasBase) {
            Write-Host '· 每用户安装（本机已有机器级底座）：功能完整可用。'
        } else {
            Write-Host '· -NoHKLM 且本机无机器级底座：输入法将不可用。全新机器'
            Write-Host '  首次安装请不带 -NoHKLM 运行（一次 UAC 完成机器级注册）。'
        }
        & (Join-Path $inst 'hufu-tsf-smoke.exe') reg $icon
}

# ── 4) 语言列表（虎符插第 0 位=默认输入法）+ 切换器装配 ──
# 输入法按应用各自记忆选择：新开宿主（开始菜单搜索框、UWP 应用、
# 新窗口）默认用列表第一项。追加到尾部会让这些宿主落回微软拼音，
# 用户体验即「开始菜单/UWP 打不了中文」。插入第 0 位后新宿主默认
# 虎符；已开应用/用户手动切过的选择不受影响（Win+空格随时可切回）。
$tipStr = "0804:$CLSID$PROFILE"
$list = Get-WinUserLanguageList
$zh = $list | Where-Object { $_.LanguageTag -like 'zh*' } | Select-Object -First 1
if (-not $zh) { $zh = $list[0] }
if ($zh.InputMethodTips -notcontains $tipStr) {
    $zh.InputMethodTips.Insert(0, $tipStr)
    Set-WinUserLanguageList $list -Force -WarningAction SilentlyContinue
} elseif ($zh.InputMethodTips[0] -ne $tipStr) {
    # 已安装但不在首位（升级/用户调整过）：调到首位，保证新宿主默认虎符
    $zh.InputMethodTips.Remove($tipStr) | Out-Null
    $zh.InputMethodTips.Insert(0, $tipStr)
    Set-WinUserLanguageList $list -Force -WarningAction SilentlyContinue
}
$asm = "HKCU:\Software\Microsoft\CTF\SortOrder\AssemblyItem\0x00000804\{34745C63-B2F0-4784-8B67-5E12C8701A31}\00000003"
New-Item -Path $asm -Force | Out-Null
Set-ItemProperty -Path $asm -Name 'CLSID' -Value $CLSID -Type String
Set-ItemProperty -Path $asm -Name 'KeyboardLayout' -Value '0' -Type String
Set-ItemProperty -Path $asm -Name 'Profile' -Value $PROFILE -Type String
Write-Host 'OK 语言列表 + 切换器装配已写入'

# ── 5) 开机自启（server 常驻 = 托盘 + 设置页 + 管道）+ 开始菜单快捷方式 ──
$run = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
Set-Reg $run 'HuFu' ('"{0}"' -f $exe)
try {
    $sm = [Environment]::GetFolderPath('Programs')
    $ws = New-Object -ComObject WScript.Shell
    $lnk = $ws.CreateShortcut((Join-Path $sm 'HuFu 虎符输入法设置.lnk'))
    $lnk.TargetPath = 'http://127.0.0.1:4390/'
    $lnk.IconLocation = $icon
    $lnk.Save()
    Write-Host 'OK 开机自启 + 开始菜单快捷方式「HuFu 虎符输入法设置」'
} catch {
    Write-Host 'OK 开机自启已设置（开始菜单快捷方式失败，可忽略）'
}

# ── 6) 启动 server ——【铁律】必须普通权限：提权启动的管道会拒绝普通应用】──
Get-Process hufu-server -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500
if ($isAdmin) {
    # 本脚本自身被提权运行（如右键管理员）：经 explorer 中转降权启动
    explorer.exe $exe
} else {
    Start-Process $exe -WindowStyle Hidden
}
Start-Sleep -Seconds 2
# 【2026-09-06 虎爪误伤修复】刷新宿主只杀 ctfmon（输入法框架标准刷新，
# 主流输入法安装器通用）。此前杀 TextInputHost/ShellExperienceHost：
# shell 宿主重启会触发 msctf 对 TIP 存储的一致性校验，把注册结构
# 非原生/不完整的第三方输入法（如虎爪）判非法周期删除——「装完
# HuFu 虎爪从列表消失」的概率性根因。ctfmon 重载即可让新 TIP 进
# Win+空格列表，无需动 shell 组件。
Stop-Process -Name ctfmon -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1
Start-Process ctfmon -ErrorAction SilentlyContinue

Write-Host ''
Write-Host '=========================================='
Write-Host ' 安装完成！'
Write-Host '  · Win+空格 切到「HuFu 虎符输入法」'
Write-Host '  · 设置：Ctrl+Alt+H（全局热键）/ 开始菜单'
Write-Host '     搜「HuFu」或双击「设置.bat」'
Write-Host '  · 绿色模式：程序在本目录原地运行，不占 C 盘；'
Write-Host '    不要移动/删除本文件夹（输入法依赖它）'
Write-Host '  · 卸载：运行「卸载.bat」后把本文件夹整个删除即可'
Write-Host '=========================================='
Write-Host '  · 无需重启/注销；正在运行的应用重开后才加载新输入法'
if ($legacy -and (Test-Path (Join-Path $legacy 'hufu-server.exe'))) {
    Write-Host "  · 记得删除旧版目录释放 ${mb}MB：$legacy"
}
$perUserFull = $NoHKLM -and (Test-Path $sysdll) -and (Test-Path "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$CLSID")
if (-not (Test-Path $sysdll)) { Write-Host '  · 本机未做机器级注册：开始菜单/UWP 暂不可用（下次以管理员运行安装器一次即可）' }
