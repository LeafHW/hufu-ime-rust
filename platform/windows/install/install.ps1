# 【同步 2026-09-11】主=E:\DSH-KF\hufu-发行\打包源\install.ps1（本文件，真实使用）；
#   从=E:\DSH-KF\hufu\发行脚本\install.ps1（仓库副本）：以本文件为准整文件同步，改动一律改本文件后覆盖从文件。
# HuFu 虎符输入法 — 安装脚本（双阶段：普通权限主导，提权只做注册）
# - 文件/HKCU/语言列表/自启/server 永远普通权限执行（server 提权启动会锁管道 ACL，
#   导致所有普通应用连不上→只能打字母，2026-08-29 实测教训）。
# - HKLM 机器级键 + msctf 原生登记（本机实测需提权才 0x00000000）交给一次 UAC 的
#   提权子进程（-PhaseElevated），日志回流本窗口可见。
# - -NoHKLM：完全跳过提权（无管理员机器的每用户安装；msctf 登记尽力而为）。
# 【修复标签 2026-09-11】HKCU 注册表段（当前用户 IME 配置）拆为可独立执行 →
# 新增 -UserOnly 开关：只跑 Install-UserConfig（HKCU COM+TIP+InstallDir）；
# 不带开关时默认行为完全不变（任务3）。
param([switch]$NoHKLM, [switch]$PhaseElevated, [switch]$UserOnly)

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
# 【三十四修补 2026-09-13】$dll32/$sysdll32 原先只在提权段（$PhaseElevated
# 块内）定义——主流程 Install-UserConfig 的 32 位视图注册引用 $null →
# Test-Path 抛「参数绑定空值」（Continue 型：安装继续但 32 位 HKCU 视图
# 被跳过，v1.5.3 起存量 bug）。提到脚本头全路径定义。
$dll32     = Join-Path $inst 'hufu_tsf32.dll'
$sysdll32  = Join-Path "$env:SystemRoot\SysWOW64\SystemIME\HuFu" 'hufu_tsf32.dll'
# SystemIME 副本：开始菜单搜索/任务栏等 SystemApps 打包进程读不了用户目录
# （%LOCALAPPDATA% 无 ALL APPLICATION PACKAGES 权限），DLL 必须住在
# C:\Windows\SystemIME（系统输入法同款目录，打包进程可读）——2026-08-29
# 实测：SearchHost 不加载用户目录 DLL → 搜索框字母直通。
$sysdir = 'C:\Windows\SystemIME\HuFu'
$sysdll = Join-Path $sysdir 'hufu_tsf.dll'

# 【虎爪保护 2026-09-11】升级检测：本输入法 TIP 键已存在 = 升级安装。
# ctfmon 重启会触发 msctf 对 TIP 存储的一致性校验，注册结构非原生的
# 第三方输入法（虎爪）概率被判非法周期删除（用户复报「安装概率杀
# 死虎爪」）。升级场景：TIP 早已在列表，新文件随应用重开生效，无
# 需刷新——跳过 ctfmon 杀启；仅首次安装（键不存在）才刷新。
$tipAlready = (Test-Path "HKCU:\Software\Microsoft\CTF\TIP\$CLSID") -or (Test-Path "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$CLSID")

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

# 【三十四修补 2026-09-13】smoke 的 msctf 原生登记（COM 调
# ITfInputProcessorProfiles）在 CTF 服务未就绪的机器上会无限挂起
#（本机实测：清理过 explorer/ctfmon 后重装，安装窗口卡死在「OK DLL →
# SystemIME」之后）。同步调用（& exe）无超时手段——改 Start-Process
# + 60 秒等待，超时杀进程按「跳过原生登记」处理（与无管理员分支同
# 语义：文件与注册表已铺好，切换器列表可能要等 ctfmon 自愈）。
function Invoke-SmokeReg([string]$regArg) {
    $smokeExe = Join-Path $inst 'hufu-tsf-smoke.exe'
    if (-not (Test-Path $smokeExe)) { return 0 }
    $p = Start-Process $smokeExe -ArgumentList 'reg', $regArg -WindowStyle Hidden -PassThru
    if ($p.WaitForExit(60000)) { return $p.ExitCode }
    try { $p.Kill() } catch {}
    Write-Host '⚠ msctf 登记超时（60 秒）——已跳过原生登记（注册表已写好，重开应用或重启后一般自愈）' -ForegroundColor Yellow
    return 0
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
    # 【修复标签 2026-09-11】硬编码盘符路径 C:\ProgramData → $env:ProgramData 推导（任务5）
    $diag = Join-Path $env:ProgramData 'HuFu\diag'
    if (Test-Path $diag) {
        $n = @(Get-ChildItem $diag -Force -ErrorAction SilentlyContinue).Count
        Get-ChildItem $diag -Force -ErrorAction SilentlyContinue | ForEach-Object {
            try { Remove-Item $_.FullName -Force -Recurse -ErrorAction Stop } catch {}
        }
        if ($n -gt 0) { Write-Host "清理诊断日志: $n 个" }
    }
    # 3) 【早期版本残留 2026-09-07】%LOCALAPPDATA%\HuFu（绿色化前旧版
    #    数据目录——新版数据全部在安装目录，此目录属残留，安装时清除）
    $legacyLocal = Join-Path $env:LOCALAPPDATA 'HuFu'
    if (Test-Path $legacyLocal) {
        try {
            Get-ChildItem $legacyLocal -Recurse -Force -ErrorAction SilentlyContinue | ForEach-Object { $_.Attributes = 'Normal' }
            Remove-Item $legacyLocal -Recurse -Force -ErrorAction Stop
            Write-Host '清理早期版本数据: %LOCALAPPDATA%\HuFu'
        } catch { Write-Host '· %LOCALAPPDATA%\HuFu 被占用，下次安装再清' }
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
    # 【修复标签 2026-09-11】退出码未传播：smoke 登记失败原先仍以 0 退出 →
    # 捕获退出码，提权阶段按真实结果退出（正常退出与失败双路径，任务7/D7）。
    # 【三十四修补】同步 & 调用换 Invoke-SmokeReg（60 秒超时，COM 服务未
    # 就绪的机器不再无限挂死安装窗口）。
    $rcSmoke = Invoke-SmokeReg $sysdll
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
    # 【修复标签 2026-09-11】退出码传播：smoke 登记失败 → 非 0 退出；成功 → 0
    if ($rcSmoke -ne 0) { Write-Host "⚠ msctf 登记退出码 $rcSmoke"; exit $rcSmoke }
    exit 0
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

# 【修复标签 2026-09-11】原 1.5)+2) 两段（HKCU 注册表：当前用户 IME 配置）原样
# 拆为独立函数 Install-UserConfig → 可被 -UserOnly 单独执行；默认主流程照旧
# 依次调用，行为不变（任务3）。
function Install-UserConfig {
    # 1.5) 记录安装目录（DLL 自愈链 / 卸载器读取）
    Set-Reg 'HKCU:\Software\HuFu' 'InstallDir' $inst

    # 2) HKCU COM + TIP 键树注册（每用户；COM 解析 HKCU 优先）
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
    # 【下划线退役 2026-09-16】DisplayAttribute 注册表键全撤——对照本机
    # 微拼/Rime/虎爪：均无此键，下划线来自 TSF 默认组段渲染；自定义标记
    # 反而让宿主连默认下划线都不画（QQ/开始菜单/记事本实锤）。

    # 【32 位宿主注册 2026-09-12 v1.5.3】32 位进程（Pain 跟打器等）的
    # COM 解析读 HKCU 的 Wow6432Node 视图（InprocServer32 键重定向），
    # 缺键时回落 HKLM→SysWOW64——未提权安装（-NoHKLM/无 SystemIME 副本）
    # 时 32 位宿主将无 DLL 可载（v1.5.2 实测 Pain 跟打器全旧版）。
    # 提权安装指向 SystemIME 32 位副本；未提权指向安装目录。
    if (Test-Path $dll32) {
        $dllReg32 = if (Test-Path $sysdll32) { $sysdll32 } else { $dll32 }
        $wowU = "HKCU:\Software\Classes\Wow6432Node\CLSID\$CLSID"
        Set-Reg $wowU '(default)' 'HuFu TSF Service'
        Set-Reg "$wowU\InprocServer32" '(default)' $dllReg32
        Set-Reg "$wowU\InprocServer32" 'ThreadingModel' 'Apartment'
        Write-Host "OK HKCU 32 位视图已注册（DLL → $dllReg32）"
    }
    Write-Host "OK HKCU COM + TIP 键树已注册（DLL → $dllReg）"
    return $true
}

if ($UserOnly) {
    # 【修复标签 2026-09-11】-UserOnly：只写当前用户 HKCU IME 配置后退出——
    # 不动 HKLM/msctf/语言列表/自启/server（多用户机器：机器级底座已由首位
    # 用户装好，新用户免提权开通当前用户配置）。
    if (-not (Install-UserConfig)) { exit 1 }
    Write-Host 'OK -UserOnly 完成：仅当前用户 HKCU 配置已写入（HKLM/msctf/server 未动）'
    exit 0
}

if (-not (Install-UserConfig)) { exit 1 }   # 默认主流程：行为与拆分前一致

# ── 3) 提权注册（HKLM + msctf；一次 UAC，日志回流本窗口）──
if (-not $NoHKLM) {
    if ($isAdmin) {
        # 【三十四修 2026-09-13】& 调用 .ps1 不设置 $LASTEXITCODE（只有原生
        # exe 才设）——旧判定 `$LASTEXITCODE -ne 0` 对 $null 恒真，管理员
        # 直跑时代每次都打假告警「msctf 登记可能未完成」。改查 $?
        # （脚本 exit 1 → $? 为 false）。
        $null = & $PSCommandPath -PhaseElevated
        if (-not $?) { Write-Host '⚠ 提权阶段报告失败（msctf 登记可能未完成）' }
    } elseif ($inAdminGroup) {
        Write-Host '（弹出 UAC：机器级注册 + msctf 登记，请点「是」）'
        $elog = Join-Path $env:TEMP 'hufu-install-elevated.log'
        $ps = "$env:WINDIR\System32\WindowsPowerShell\v1.0\powershell.exe"
        $arg = "-NoProfile -ExecutionPolicy Bypass -Command `"[Console]::OutputEncoding=[Text.Encoding]::UTF8; & '$PSCommandPath' -PhaseElevated *> '$elog'`""
        # 【修复标签 2026-09-11】提权子进程退出码原先丢弃 → -PassThru 捕获并告警（不中断主流程）
        $evProc = Start-Process $ps -Verb RunAs -ArgumentList $arg -Wait -PassThru
        if ($evProc.ExitCode -ne 0) { Write-Host "⚠ 提权阶段退出码 $($evProc.ExitCode)（msctf 登记可能未完成）" }
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
        $null = Invoke-SmokeReg $icon
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
        $null = Invoke-SmokeReg $icon
}

# ── 4) 语言列表——【结构根治 2026-10-09：CTF 原生 API，对齐虎爪】──
# 【对齐虎爪 2026-10-09】虎爪（同为 TSF 输入法）安装卸载零事故的根
# 因：全程只做 regsvr32/TSF 原生注册，从不重写语言列表。虎符此前为
# 「装完自动进列表+默认首选」用了 Get/Set-WinUserLanguageList 整表
# 读改写——重写会把「读时没枚举到的输入法」固化删除（他机实锤：装
# 完别人的输入法被吞）。四层补救防线只救得回「快照看见过的」，救不
# 了快照本身没看见的——修多少次都会复发。
# 当晚再实锤：绕过 API 直接写注册表两个真存储（列表真存储 0804:* 值
# + 切换器装配表槽位）也不行——msctf 一致性校验只认走
# EnableLanguageProfile 激活过的成员条目，裸写条目数分钟内被整根拔
# 除（18:53 实测：注册完好、列表条目被清）。
# 根治：语言列表成员资格一律走 CTF 原生 API——smoke 新增 enable 命
# 令调 ITfInputProcessorProfiles::EnableLanguageProfile（设置页「添
# 加输入法」同款路径）。API 契约只启用自己的 profile，物理上碰不到
# 其他输入法；也不再需要知道机器装了什么输入法、装了多少个。
# 默认输入法不受影响：由 Set-WinDefaultInputMethodOverride 显式指定
# （下段），与列表位置无关。
# 注册表真存储在本段只读不写：装前快照/装后回读核验他人条目零变化
# 并写 install.log 留证。
$tipStr = "0804:$CLSID$PROFILE"
$asmBase = "HKCU:\Software\Microsoft\CTF\SortOrder\AssemblyItem\0x00000804\{34745C63-B2F0-4784-8B67-5E12C8701A31}"
$upRoot = 'HKCU:\Control Panel\International\User Profile'
# 快照：仅用于末态核验与日志留证——绝不作为任何重写的原料
function Get-HuFuUpOthers {
    $r = @()
    try {
        foreach ($lk in Get-ChildItem $upRoot -ErrorAction SilentlyContinue) {
            foreach ($n in (Get-Item $lk.PSPath -ErrorAction SilentlyContinue).Property) {
                if ($n -like '0804:*' -and $n -ne $tipStr) { $r += $n }
            }
        }
    } catch { }
    ,@($r | Select-Object -Unique)
}
function Get-HuFuAsmOthers {
    $r = @()
    if (Test-Path $asmBase) {
        foreach ($k in Get-ChildItem $asmBase -ErrorAction SilentlyContinue) {
            $p = Get-ItemProperty $k.PSPath -ErrorAction SilentlyContinue
            if ($p.CLSID -and $p.Profile -and $p.CLSID -ne $CLSID) { $r += "0804:$($p.CLSID)$($p.Profile)" }
        }
    }
    ,@($r | Select-Object -Unique)
}
$upBefore = Get-HuFuUpOthers
$asmBefore = Get-HuFuAsmOthers
# ① 加入语言列表：调 CTF 原生 API（smoke enable → EnableLanguageProfile）
# ——只启用自己的 profile，物理上碰不到其他输入法的任何条目。失败不
# 裸写注册表兜底（裸写条目会被 msctf 一致性校验判非法清除，18:53 实
# 锤）；失败=注册已就绪，提示手动到设置页添加一次（虎爪同款流程）。
$smokeExe = Join-Path $inst 'hufu-tsf-smoke.exe'
$enableOk = $false
$enableLog = Join-Path $PSScriptRoot 'install.log'
if (Test-Path $smokeExe) {
    $tmpOut = Join-Path $env:TEMP 'hufu-enable-out.txt'
    $p = Start-Process $smokeExe -ArgumentList 'enable' -WindowStyle Hidden -PassThru -RedirectStandardOutput $tmpOut
    $null = $p.WaitForExit(60000)
    Start-Sleep -Milliseconds 500
    $det = ''
    try { $det = (Get-Content $tmpOut -Encoding UTF8 -Raw -ErrorAction SilentlyContinue) } catch { }
    # PS5.1 重定向模式下 ExitCode 可能读空——以进程退出+成功输出为准
    if (($det -match 'OK 虎符已加入语言列表') -or (($p.HasExited) -and ($p.ExitCode -eq 0) -and $det -and ($det -notmatch '失败'))) {
        $enableOk = $true
        Write-Host 'OK 已加入语言列表（CTF 原生 EnableLanguageProfile：他人条目零触碰）'
    } else {
        try { Add-Content -Path $enableLog -Value ("[{0}] install enable 失败: exit={1} {2}" -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'), $p.ExitCode, $det) -Encoding UTF8 } catch { }
        Write-Host '⚠ 自动加入语言列表失败——请到 设置→时间和语言→语言→中文→选项 手动添加一次「HuFu 虎符输入法」（注册已就绪，添加一次即永久）' -ForegroundColor Yellow
    }
} else {
    Write-Host '⚠ 缺 hufu-tsf-smoke.exe——请到 设置→时间和语言→语言→中文→选项 手动添加一次「HuFu 虎符输入法」' -ForegroundColor Yellow
}
# ② ctfmon 若未运行则冷启动（CTF 从注册表真存储重建视图；本变量供
# 第 6 段刷新逻辑判断复用）
$ctfStartedEarly = $false
if (-not (Get-Process ctfmon -ErrorAction SilentlyContinue)) {
    Start-Process ctfmon -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 2
    $ctfStartedEarly = $true
}
# ③ 末态核验（注册表对注册表回读，不涉及枚举——枚举说不说谎都不影
# 响结论）+ 日志留证
$upAfter = Get-HuFuUpOthers
$asmAfter = Get-HuFuAsmOthers
$othersTotal = @((@($upBefore) + @($asmBefore)) | Select-Object -Unique)
$gone = @($othersTotal | Where-Object { ($upAfter -notcontains $_) -and ($asmAfter -notcontains $_) })
$meIn = $false
Start-Sleep -Milliseconds 800   # CTF 落注册表为异步，稍候再读
try {
    foreach ($lk in (Get-ChildItem $upRoot -ErrorAction SilentlyContinue | Where-Object { $_.PSChildName -like 'zh*' })) {
        if ((Get-Item $lk.PSPath -ErrorAction SilentlyContinue).Property -contains $tipStr) { $meIn = $true }
    }
} catch { }
try {
    $logLine = "[{0}] install CTF原生启用: enable={1} 他人快照={2}（真存储{3}+装配表{4}） 末态他人={5} 我们在场={6} 丢失=[{7}] ctfmon预热={8}" -f `
        (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'), $enableOk, @($othersTotal).Count, @($upBefore).Count, @($asmBefore).Count, @((@($upAfter) + @($asmAfter)) | Select-Object -Unique).Count, $meIn, ($gone -join ','), $ctfStartedEarly
    Add-Content -Path (Join-Path $PSScriptRoot 'install.log') -Value $logLine -Encoding UTF8
} catch { }
if ($gone.Count -gt 0) {
    # 原生启用按 API 契约只动自己的 profile；此告警只为极端并发修改留证
    Write-Host "⚠ 核验发现既有条目变化（原生启用下不应发生，详见 install.log）：$($gone -join ', ')" -ForegroundColor Yellow
} else {
    Write-Host "OK 语言列表处理完成（他人快照 $(@($othersTotal).Count) 条全数在场，0 缺失）"
}
# 【默认首选输入法 2026-09-16】显式设默认输入法覆盖=虎符（用户实锤：
# 仅靠列表首位，重启后默认输入法可能不是虎符）。覆盖优先于列表序，
# 重启/新会话/新宿主一律默认虎符；用户手动 Win+空格 切换不受影响。
try {
    Set-WinDefaultInputMethodOverride $tipStr -ErrorAction Stop
    Write-Host 'OK 默认输入法覆盖 = 虎符（重启/新会话默认首选）'
} catch { Write-Host '· 默认输入法覆盖写入失败（不影响列表首位默认）' }
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
# 【2026-09-11 虎爪保护二阶】ctfmon 重启本身仍会触发同款校验——升级
# 安装（$tipAlready）完全跳过刷新；仅首次安装执行。
# 【吃掉输入法·二阶根治 2026-10-06】第 4 段防线①已在本安装内冷启动
# 过 ctfmon（新登录会话场景）——刷新目的已达成，不再重复杀启（每次
# 重启都是一次 msctf 一致性校验风险）。
if ($ctfStartedEarly) {
    Write-Host 'OK ctfmon 已在本安装内冷启动（语言列表读取前），跳过刷新'
} elseif (-not $tipAlready) {
    Stop-Process -Name ctfmon -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 1
    Start-Process ctfmon -ErrorAction SilentlyContinue
} else {
    Write-Host 'OK 升级安装：跳过 ctfmon 刷新（虎爪保护；新文件随应用重开生效）'
}

Write-Host ''
Write-Host '=========================================='
Write-Host ' 安装完成！'
Write-Host '  · Win+空格 切到「HuFu 虎符输入法」'
Write-Host '  · 设置：Ctrl+Alt+H（已开则弹到最前）/ Ctrl+Shift+H 呼出到最前 / 开始菜单'
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
