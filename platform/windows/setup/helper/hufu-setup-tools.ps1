# HuFu 虎符输入法 · 安装器助手工具集（Inno 安装器 [Code] 调用，契约见下）
# ─────────────────────────────────────────────────────────────────────
# 形态：UTF-8 带 BOM / PowerShell 5.1 兼容 / 零交互（不读键盘输入、无确认弹窗）。
# 日志：每次运行追加「[时间] 命令行 + 结果摘要」到 -Log 指定文件；
#       默认 DstRoot\setup-tools.log（无 DstRoot 用 InstDir\setup-tools.log，
#       再无则 %TEMP%\hufu-setup-tools.log）。
#
# 命令与退出码（契约，安装器依赖，勿改语义）：
#   -BackupDiff -Manifest <file> -SrcRoot <dir> -DstRoot <dir> -BackupRoot <dir>
#       manifest 每行一个相对路径（如 码表\虎整句\码表.txt）；'!' 前缀 = 目标
#       缺失才拷（仅 数据\config.json 用）。目标无→拷；有→SHA256 相同→跳过
#       （保 mtime）；不同→目标按相对路径拷入 BackupRoot 后再覆盖。
#       退出：0 正常 / 2 manifest 缺失 / 5 拷贝失败（日志注明哪个文件）。
#   -KillServer [-Rounds 12]
#       循环杀 hufu-server（Stop-Process -Force），700ms 间隔，连续 2 轮无进程
#       即成。退出：0 已死 / 3 杀不净。
#   -ClearOverride
#       Remove-ItemProperty 'HKCU:\Control Panel\International\User Profile'
#       -Name InputMethodOverride（幂等）。恒 0。
#   -ElevatedUninstallTail -InstDir <dir>   【仅管理员上下文；否则退出 6】
#       (a) SystemIME 两处（System32/SysWOW64）HuFu.old* 目录内文件与
#           HuFu\*.old* 文件做 PFRO 延迟删除登记：MoveFileEx(…, DELAY_UNTIL_
#           REBOOT, NULL) P/Invoke；PFRO 注册表值读-改-写保既有条目并去重，
#           每次写前快照进日志；目录只登记其中文件。
#       (b) 校验 HKLM CTF\TIP\{CLSID} 与 Classes\CLSID\{CLSID}（含 WOW6432Node
#           副本）已不存在；残留→日志+退出 4。
#       (c) 删 C:\ProgramData\HuFu（重试 2 次）。
#   -PostBoot -InstDir <dir>
#       删 InstDir\*.old*（5×1s 重试），尽力删 SystemIME 残留（普通权限失败
#       静默），日志一行报告。恒 0（不得打扰开机）。
#   -PurgeData -InstDir <dir>
#       删 码表\ 数据\ 模型\ 码表导出\ 升级备份 与 install.log / setup-*.log
#       （活动日志自身除外，避免边写边删），输出统计。恒 0。
#
# 其它退出码：1 = 未给命令 / 必需参数缺失（用法错误，日志有说明）。
# 库模式：设环境变量 HUFU_SETUP_TOOLS_LIB=1 后 dot-source，只定义函数不执行
#        （自测 / QA 用）。
param(
    [switch]$BackupDiff,
    [switch]$KillServer,
    [switch]$ClearOverride,
    [switch]$ElevatedUninstallTail,
    [switch]$PostBoot,
    [switch]$PurgeData,
    [string]$Manifest = '',
    [string]$SrcRoot = '',
    [string]$DstRoot = '',
    [string]$BackupRoot = '',
    [int]$Rounds = 12,
    [string]$InstDir = '',
    [string]$Log = '',
    # 测试注入口：KillServer 的目标进程名（默认 hufu-server；安装器不传此参）。
    # 自测用替身进程名以免误伤真机 live server。
    [string]$ProcName = 'hufu-server'
)

$ErrorActionPreference = 'Continue'

$script:HuFuClSID = '{8F5C2A10-3E77-4B9C-A1D4-9E0B7C2F5A88}'
$script:PfroKey   = 'SYSTEM\CurrentControlSet\Control\Session Manager'
$script:PfroValue = 'PendingFileRenameOperations'

# ── 日志（UTF-8 BOM 起头；失败绝不影响主流程） ──
if (-not $Log) {
    if ($DstRoot)       { $Log = Join-Path $DstRoot 'setup-tools.log' }
    elseif ($InstDir)   { $Log = Join-Path $InstDir 'setup-tools.log' }
    else                { $Log = Join-Path $env:TEMP 'hufu-setup-tools.log' }
}
$script:LogPath = $Log

function Write-ToolLog {
    param([string]$Message = '', [string]$Prefix = '')
    try {
        $dir = Split-Path -Parent $script:LogPath
        if ($dir -and -not (Test-Path -LiteralPath $dir)) {
            New-Item -ItemType Directory -Path $dir -Force | Out-Null
        }
        $line = if ($Prefix) { "[$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')] $Prefix $Message" } else { $Message }
        $enc = New-Object System.Text.UTF8Encoding($true)
        if (-not (Test-Path -LiteralPath $script:LogPath)) {
            [System.IO.File]::WriteAllText($script:LogPath, $line + "`r`n", $enc)
        } else {
            [System.IO.File]::AppendAllText($script:LogPath, $line + "`r`n", $enc)
        }
    } catch { }
}

# ── 结果输出：stdout（供安装器 [Code] 捕获）+ 日志双写 ──
function Out-Result {
    param([string]$Message = '', [string]$Mark = '')
    try { Write-Host $Message } catch { }
    Write-ToolLog $Message $Mark
}

# ── 通用小件 ──
function Test-IsAdmin {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    return ([Security.Principal.WindowsPrincipal]$id).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Ensure-DirForFile {
    param([string]$FilePath)
    $dir = Split-Path -Parent $FilePath
    if ($dir -and -not (Test-Path -LiteralPath $dir)) {
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
    }
}

# ═══════════════════ PFRO（PendingFileRenameOperations）═══════════════════
# Hive/KeyPath 参数是测试注入口（默认=真实 Session Manager）；生产行为不受影响。

function Get-PfroRaw {
    param([string]$Hive = 'LM', [string]$KeyPath = $script:PfroKey, [string]$ValueName = $script:PfroValue)
    $root = if ($Hive -eq 'CU') { [Microsoft.Win32.Registry]::CurrentUser } else { [Microsoft.Win32.Registry]::LocalMachine }
    $k = $root.OpenSubKey($KeyPath)
    if (-not $k) { return $null }
    try {
        $v = $k.GetValue($ValueName)
        if ($v -is [string[]]) { return ,@($v) }
        return $null
    } finally { $k.Close() }
}

function Set-PfroRaw {
    # $Value=$null ⇒ 删除该值（用于还原到"原本不存在"状态）
    param([AllowNull()][string[]]$Value, [string]$Hive = 'LM', [string]$KeyPath = $script:PfroKey, [string]$ValueName = $script:PfroValue)
    $root = if ($Hive -eq 'CU') { [Microsoft.Win32.Registry]::CurrentUser } else { [Microsoft.Win32.Registry]::LocalMachine }
    $k = $root.OpenSubKey($KeyPath, $true)
    if (-not $k) { return $false }
    try {
        if ($null -eq $Value) {
            if ($k.GetValueNames() -contains $ValueName) { $k.DeleteValue($ValueName, $false) }
        } else {
            $k.SetValue($ValueName, [string[]]$Value, [Microsoft.Win32.RegistryValueKind]::MultiString)
        }
        return $true
    } finally { $k.Close() }
}

function Merge-PfroEntries {
    # 纯函数：既有条目 + 待登记删除路径（成对 (src,'')）→ 去重保序。
    # 返回合并后数组；无新增（已全部在表）返回 $null（调用方免写）。
    param([AllowNull()][string[]]$Existing, [string[]]$ToAddSrcPaths)
    $pairs = New-Object System.Collections.Generic.List[string]
    $seen  = New-Object 'System.Collections.Generic.HashSet[string]' -ArgumentList ([System.StringComparer]::OrdinalIgnoreCase)
    if ($Existing) {
        $i = 0
        while ($i -lt $Existing.Count) {
            $src = [string]$Existing[$i]
            $dst = ''
            if (($i + 1) -lt $Existing.Count) { $dst = [string]$Existing[$i + 1] }
            $key = "$src|$dst"
            if ($seen.Add($key)) { $pairs.Add($src); $pairs.Add($dst) }
            $i += 2
        }
    }
    $added = 0
    foreach ($p in @($ToAddSrcPaths)) {
        if (-not $p) { continue }
        $src = if ($p.StartsWith('\??\')) { $p } else { '\??\' + $p }
        if ($seen.Add("$src|")) { $pairs.Add($src); $pairs.Add(''); $added++ }
    }
    if ($added -eq 0) { return $null }
    return ,@($pairs.ToArray())
}

function Test-PfroContains {
    # PFRO 是否已含 (路径, '') 删除对（\??\ 前缀、忽略大小写、容忍尾反斜杠）
    param([AllowNull()][string[]]$Pfro, [string]$Path)
    if (-not $Pfro) { return $false }
    $want = ('\??\' + $Path).TrimEnd('\')
    $i = 0
    while (($i + 1) -lt $Pfro.Count) {
        $src = [string]$Pfro[$i]
        if ($src -and $src.TrimEnd('\').Equals($want, [System.StringComparison]::OrdinalIgnoreCase) -and [string]$Pfro[$i + 1] -eq '') {
            return $true
        }
        $i += 2
    }
    return $false
}

function Format-Pfro {
    param([AllowNull()][string[]]$V)
    if ($null -eq $V) { return '<无值>' }
    $shown = @($V | ForEach-Object { if ([string]$_ -eq '') { '∅' } else { [string]$_ } })
    return "[$(@($V).Count) 项] " + ($shown -join ' | ')
}

function Ensure-MoveFileExType {
    if (-not ([System.Management.Automation.PSTypeName]'HuFuSetup.Native').Type) {
        Add-Type -Namespace HuFuSetup -Name Native -MemberDefinition @'
[DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)]
public static extern bool MoveFileEx(string lpExistingFileName, string lpNewFileName, int dwFlags);
'@
    }
}

function Register-PfroDelete {
    # 主路径：MoveFileEx(DELAY_UNTIL_REBOOT, NULL)。返回失败清单（空=全成）。
    param([string[]]$Paths)
    $failed = @()
    Ensure-MoveFileExType
    foreach ($p in @($Paths)) {
        if (-not $p) { continue }
        if (-not (Test-Path -LiteralPath $p)) {
            Write-ToolLog "PFRO: 跳过（文件已不在）: $p" '·'
            continue
        }
        # MOVEFILE_DELAY_UNTIL_REBOOT = 0x4；目的=NULL = 重启后删除
        # 【M5】PS5.1 的 $null 传 P/Invoke string 会变空串""（被当目标路径→
        # err=3 PATH_NOT_FOUND）；[NullString]::Value 才是真 NULL 指针。
        $ok = [HuFuSetup.Native]::MoveFileEx($p, [NullString]::Value, 4)
        if ($ok) {
            Write-ToolLog "PFRO: 已登记重启删除: $p" '·'
        } else {
            $err = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
            Write-ToolLog "PFRO: MoveFileEx 失败 err=$err : $p" '⚠'
            $failed += $p
        }
    }
    return ,@($failed)
}

# ═══════════════════ 命令实现 ═══════════════════

function Invoke-BackupDiffCmd {
    if (-not $Manifest -or -not (Test-Path -LiteralPath $Manifest)) {
        Out-Result "RESULT: manifest 缺失: $Manifest (exit=2)" '✗'
        return 2
    }
    if (-not $SrcRoot -or -not (Test-Path -LiteralPath $SrcRoot)) {
        Out-Result "RESULT: SrcRoot 无效: $SrcRoot (exit=5)" '✗'
        return 5
    }
    if (-not $DstRoot -or -not $BackupRoot) {
        Out-Result 'RESULT: 需要 -DstRoot 与 -BackupRoot (exit=1)' '✗'
        return 1
    }
    foreach ($d in @($DstRoot, $BackupRoot)) {
        if (-not (Test-Path -LiteralPath $d)) {
            New-Item -ItemType Directory -Path $d -Force | Out-Null
        }
    }

    $copied = 0; $skipSame = 0; $skipExists = 0; $backed = 0; $failed = 0
    $failList = New-Object System.Collections.Generic.List[string]
    $lines = @()
    try { $lines = Get-Content -LiteralPath $Manifest -Encoding UTF8 } catch {
        Out-Result "RESULT: manifest 读取失败: $Manifest ($_)" '✗'
        return 2
    }

    foreach ($raw in @($lines)) {
        $line = ([string]$raw).Trim()
        if (-not $line -or $line.StartsWith('#')) { continue }
        $onlyMissing = $false
        if ($line.StartsWith('!')) { $onlyMissing = $true; $line = $line.Substring(1).Trim() }
        if (-not $line) { continue }
        $rel  = $line -replace '/', '\'
        $src  = Join-Path $SrcRoot  $rel
        $dst  = Join-Path $DstRoot  $rel

        if (Test-Path -LiteralPath $dst) {
            if ($onlyMissing) {
                $skipExists++
                Write-ToolLog "BackupDiff: 跳过（! 目标已在）: $rel" '·'
                continue
            }
            if (-not (Test-Path -LiteralPath $src)) {
                $failed++; $failList.Add("$rel（源缺失）")
                Write-ToolLog "BackupDiff: 源缺失: $src" '⚠'
                continue
            }
            $same = $false
            try {
                $same = ((Get-FileHash -LiteralPath $src -Algorithm SHA256).Hash -eq
                         (Get-FileHash -LiteralPath $dst -Algorithm SHA256).Hash)
            } catch { $same = $false }   # 哈希异常按"不同"处理（走备份，保守）
            if ($same) {
                $skipSame++
                Write-ToolLog "BackupDiff: 跳过（SHA256 相同，保 mtime）: $rel" '·'
                continue
            }
            $bdst = Join-Path $BackupRoot $rel
            try {
                Ensure-DirForFile $bdst
                Copy-Item -LiteralPath $dst -Destination $bdst -Force -ErrorAction Stop
                $backed++
                Copy-Item -LiteralPath $src -Destination $dst -Force -ErrorAction Stop
                $copied++
                Write-ToolLog "BackupDiff: 已备份+覆盖: $rel → 备份 $bdst" '+'
            } catch {
                $failed++; $failList.Add("$rel（备份/覆盖失败）")
                Write-ToolLog "BackupDiff: 备份/覆盖失败: $rel : $($_.Exception.Message)" '⚠'
            }
        } else {
            if (-not (Test-Path -LiteralPath $src)) {
                $failed++; $failList.Add("$rel（源缺失）")
                Write-ToolLog "BackupDiff: 源缺失: $src" '⚠'
                continue
            }
            try {
                Ensure-DirForFile $dst
                Copy-Item -LiteralPath $src -Destination $dst -Force -ErrorAction Stop
                $copied++
                Write-ToolLog "BackupDiff: 拷入: $rel" '+'
            } catch {
                $failed++; $failList.Add("$rel（拷入失败）")
                Write-ToolLog "BackupDiff: 拷入失败: $rel : $($_.Exception.Message)" '⚠'
            }
        }
    }

    $rc = 0
    if ($failed -gt 0) { $rc = 5 }
    $summary = "BackupDiff 拷入=$copied 跳过=$($skipSame + $skipExists)（相同$skipSame/已存在$skipExists）已备份=$backed 失败=$failed exit=$rc"
    if ($failList.Count -gt 0) { $summary += " 失败清单: $($failList -join '; ')" }
    Out-Result "RESULT: $summary"
    return $rc
}

function Invoke-KillServerCmd {
    $name = $ProcName
    if (-not $name) { $name = 'hufu-server' }
    $consecNone = 0
    $round = 0
    $killedTotal = 0
    while ($round -lt $Rounds) {
        $round++
        $procs = @(Get-Process -Name $name -ErrorAction SilentlyContinue)
        if ($procs.Count -gt 0) {
            $consecNone = 0
            $killedTotal += $procs.Count
            Write-ToolLog "KillServer: 第${round}轮 杀 $($procs.Count) 个 $name (pid=$(($procs | ForEach-Object Id) -join ','))" '·'
            $procs | Stop-Process -Force -ErrorAction SilentlyContinue
        } else {
            $consecNone++
            if ($consecNone -ge 2) { break }
        }
        Start-Sleep -Milliseconds 700
    }
    $left = @(Get-Process -Name $name -ErrorAction SilentlyContinue)
    if ($left.Count -gt 0) {
        Out-Result "RESULT: KillServer 杀不净（${Rounds} 轮后仍剩 $($left.Count) 个）exit=3" '✗'
        return 3
    }
    Out-Result "RESULT: KillServer 已净（${round} 轮，累计杀 $killedTotal 个）exit=0"
    return 0
}

function Invoke-ClearOverrideCmd {
    $key = 'HKCU:\Control Panel\International\User Profile'
    $had = $false
    try {
        $v = (Get-ItemProperty -Path $key -Name 'InputMethodOverride' -ErrorAction SilentlyContinue).InputMethodOverride
        if ($null -ne $v) { $had = $true }
    } catch { }
    try { Remove-ItemProperty -Path $key -Name 'InputMethodOverride' -Force -ErrorAction SilentlyContinue } catch { }
    # 回读确认（幂等：不存在也算成功）
    $still = $false
    try {
        $v2 = (Get-ItemProperty -Path $key -Name 'InputMethodOverride' -ErrorAction SilentlyContinue).InputMethodOverride
        if ($null -ne $v2) { $still = $true }
    } catch { }
    if ($still) {
        Out-Result 'RESULT: ClearOverride 清除后回读仍在（异常，但不报错——幂等契约恒 0）' '⚠'
    } else {
        Out-Result "RESULT: ClearOverride 完成（原有值=$had）exit=0"
    }
    return 0
}

function Invoke-ElevatedUninstallTailCmd {
    if (-not (Test-IsAdmin)) {
        Out-Result 'RESULT: ElevatedUninstallTail 需要管理员上下文 (exit=6)' '✗'
        return 6
    }
    if (-not $InstDir) {
        Out-Result 'RESULT: ElevatedUninstallTail 需要 -InstDir (exit=1)' '✗'
        return 1
    }

    # (0)【M2 契约对齐 2026-10-13】先以子进程运行原版 uninstall.ps1：
    #     管理员上下文=完整模式（HKCU 全套+语言列表减法+HKLM/SystemIME/
    #     ProgramData+ctfmon 复位），其内部 isAdmin 成立故零自提权零交互。
    #     子进程隔离：防其任何显式 exit 直接终结本 helper，并拿到真实退出码。
    $un_ps1 = Join-Path $InstDir 'uninstall.ps1'
    $unRc = 0
    if (Test-Path -LiteralPath $un_ps1) {
        Write-ToolLog 'ElevatedUninstallTail: 子进程运行原版 uninstall.ps1（完整模式）'
        $unOut = "$Log.uninstall.txt"
        try {
            $p = Start-Process -FilePath "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" `
                -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-File', $un_ps1) `
                -WorkingDirectory $InstDir -NoNewWindow -Wait -PassThru `
                -RedirectStandardOutput $unOut -RedirectStandardError "$unOut.err"
            $unRc = $p.ExitCode
        } catch {
            $unRc = 1
            Write-ToolLog "uninstall.ps1 启动异常: $($_.Exception.Message)" '⚠'
        }
        if (Test-Path -LiteralPath $unOut) {
            foreach ($l in (Get-Content -LiteralPath $unOut -Encoding UTF8 -ErrorAction SilentlyContinue)) {
                Write-ToolLog ("uninstall.ps1| " + $l)
            }
            Remove-Item -LiteralPath $unOut, "$unOut.err" -Force -ErrorAction SilentlyContinue
        }
        Write-ToolLog "uninstall.ps1 退出码=$unRc"
        if ($unRc -ne 0) {
            Out-Result "RESULT: ElevatedUninstallTail: uninstall.ps1 失败($unRc)，中止尾部清理 exit=1" '✗'
            return 1
        }
    } else {
        Write-ToolLog "未找到 $un_ps1（旧目录形态？仅执行尾部清理）" '⚠'
    }

    # (a) PFRO 延迟删除登记：SystemIME 两处 HuFu.old* 目录内文件 + HuFu\*.old* 文件
    $targets = New-Object System.Collections.Generic.List[string]
    foreach ($base in @((Join-Path $env:SystemRoot 'SystemIME'), (Join-Path $env:SystemRoot 'SysWOW64\SystemIME'))) {
        if (-not (Test-Path -LiteralPath $base)) { continue }
        $oldDirs = @(Get-ChildItem -LiteralPath $base -Force -ErrorAction SilentlyContinue |
            Where-Object { $_.PSIsContainer -and $_.Name -like 'HuFu.old*' })
        foreach ($d in $oldDirs) {
            # 目录只能登记其中文件（PFRO 不删目录）
            Get-ChildItem -LiteralPath $d.FullName -Recurse -File -Force -ErrorAction SilentlyContinue |
                ForEach-Object { $targets.Add($_.FullName) }
        }
        $hu = Join-Path $base 'HuFu'
        if (Test-Path -LiteralPath $hu) {
            Get-ChildItem -LiteralPath $hu -Filter '*.old*' -File -Force -ErrorAction SilentlyContinue |
                ForEach-Object { $targets.Add($_.FullName) }
        }
    }
    $targets = @($targets | Select-Object -Unique)
    Write-ToolLog "PFRO: 收集待登记 $($targets.Count) 个文件（HuFu.old* 目录内文件 + HuFu\*.old*）"

    $before = Get-PfroRaw
    Write-ToolLog ("PFRO 写前快照: " + (Format-Pfro $before))
    if ($targets.Count -gt 0) {
        $mfeFailed = Register-PfroDelete -Paths $targets
        # 校验 + 手动读-改-写兜底（读的是"当前最新值"，不用陈旧快照）
        $after = Get-PfroRaw
        $missing = @()
        foreach ($t in $targets) {
            if (-not (Test-PfroContains -Pfro $after -Path $t)) { $missing += $t }
        }
        if ($missing.Count -gt 0) {
            Write-ToolLog "PFRO: MoveFileEx 后仍缺 $($missing.Count) 项，转手动读-改-写" '⚠'
            $cur = Get-PfroRaw
            Write-ToolLog ("PFRO 手写前快照: " + (Format-Pfro $cur))
            $merged = Merge-PfroEntries -Existing $cur -ToAddSrcPaths $missing
            if ($merged) {
                $wok = Set-PfroRaw -Value $merged
                Write-ToolLog "PFRO: 手动写入 $($merged.Count) 项 ok=$wok"
            } else {
                Write-ToolLog 'PFRO: 手写判定无需（已全部在表）'
            }
        } else {
            Write-ToolLog "PFRO: MoveFileEx 全部登记在表（含既有条目保全）"
        }
    } else {
        Write-ToolLog 'PFRO: 无待登记目标（SystemIME 已净）'
    }

    # (b) HKLM 残留校验
    $clsid = $script:HuFuClSID
    $checks = @(
        "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$clsid",
        "HKLM:\SOFTWARE\Classes\CLSID\$clsid",
        "HKLM:\SOFTWARE\Classes\WOW6432Node\CLSID\$clsid"
    )
    $leftover = @()
    foreach ($c in $checks) {
        if (Test-Path $c) { $leftover += $c }
    }
    foreach ($c in $checks) {
        $has = Test-Path $c
        $state = '已净'; $mark = '·'
        if ($has) { $state = '残留!'; $mark = '⚠' }
        Write-ToolLog "HKLM 校验: $state  $c" $mark
    }

    # (c) ProgramData\HuFu（重试 2 次）
    $pdf = Join-Path $env:ProgramData 'HuFu'
    $pdfDone = -not (Test-Path -LiteralPath $pdf)
    if (-not $pdfDone) {
        foreach ($attempt in 1..3) {
            try {
                Get-ChildItem -LiteralPath $pdf -Recurse -Force -ErrorAction SilentlyContinue |
                    ForEach-Object { $_.Attributes = 'Normal' }
                Remove-Item -LiteralPath $pdf -Recurse -Force -ErrorAction Stop
                $pdfDone = $true
                break
            } catch {
                Write-ToolLog "ProgramData 删除第 ${attempt} 次失败: $($_.Exception.Message)" '⚠'
                Start-Sleep -Milliseconds 500
            }
        }
    }

    $rc = 0
    if ($leftover.Count -gt 0) {
        $rc = 4
        Out-Result "RESULT: ElevatedUninstallTail HKLM 残留 $($leftover.Count) 处（见上）; ProgramData 清理=$pdfDone; exit=4" '✗'
    } else {
        Out-Result "RESULT: ElevatedUninstallTail 完成; PFRO 目标 $($targets.Count); ProgramData 清理=$pdfDone; exit=0"
    }
    return $rc
}

function Invoke-PostBootCmd {
    if (-not $InstDir) {
        Out-Result 'RESULT: PostBoot 缺 -InstDir（按契约仍恒 0）' '⚠'
        return 0
    }
    $deleted = 0; $remain = 0
    foreach ($attempt in 1..5) {
        $olds = @(Get-ChildItem -LiteralPath $InstDir -Filter '*.old*' -File -Force -ErrorAction SilentlyContinue)
        if ($olds.Count -eq 0) { break }
        foreach ($f in $olds) {
            try {
                Remove-Item -LiteralPath $f.FullName -Force -ErrorAction Stop
                $deleted++
            } catch { }
        }
        $remain = @(Get-ChildItem -LiteralPath $InstDir -Filter '*.old*' -File -Force -ErrorAction SilentlyContinue).Count
        if ($remain -eq 0) { break }
        Start-Sleep -Seconds 1
    }
    # SystemIME 尽力删（普通权限失败静默；不打扰开机）
    $sysDel = 0; $sysRemain = 0
    foreach ($base in @((Join-Path $env:SystemRoot 'SystemIME'), (Join-Path $env:SystemRoot 'SysWOW64\SystemIME'))) {
        if (-not (Test-Path -LiteralPath $base)) { continue }
        $items = @(Get-ChildItem -LiteralPath $base -Force -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -like 'HuFu.old*' })
        $items += @(Get-ChildItem -LiteralPath (Join-Path $base 'HuFu') -Filter '*.old*' -File -Force -ErrorAction SilentlyContinue)
        foreach ($it in $items) {
            try {
                if ($it.PSIsContainer) {
                    Get-ChildItem -LiteralPath $it.FullName -Recurse -Force -ErrorAction SilentlyContinue |
                        ForEach-Object { $_.Attributes = 'Normal' }
                }
                Remove-Item -LiteralPath $it.FullName -Recurse -Force -ErrorAction Stop
                $sysDel++
            } catch { $sysRemain++ }
        }
    }
    Out-Result "RESULT: PostBoot 安装目录 .old* 删 $deleted 剩 $remain；SystemIME 尽力删 $sysDel 剩 $sysRemain；exit=0"
    return 0
}

function Invoke-PurgeDataCmd {
    if (-not $InstDir) {
        Out-Result 'RESULT: PurgeData 缺 -InstDir（按契约仍恒 0）' '⚠'
        return 0
    }
    $dirNames = @('码表', '数据', '模型', '码表导出', '升级备份')
    $dirsGone = 0; $dirsFail = 0; $filesGone = 0; $filesFail = 0; $selfSkipped = $false

    foreach ($n in $dirNames) {
        $p = Join-Path $InstDir $n
        if (-not (Test-Path -LiteralPath $p)) { continue }
        $ok = $false
        foreach ($attempt in 1..2) {
            try {
                Get-ChildItem -LiteralPath $p -Recurse -Force -ErrorAction SilentlyContinue |
                    ForEach-Object { $_.Attributes = 'Normal' }
                Remove-Item -LiteralPath $p -Recurse -Force -ErrorAction Stop
                $ok = $true
                break
            } catch { Start-Sleep -Milliseconds 400 }
        }
        if ($ok) { $dirsGone++ } else { $dirsFail++; Write-ToolLog "PurgeData: 目录未能删除: $p" '⚠' }
    }

    $logTargets = @(Get-ChildItem -LiteralPath $InstDir -File -Force -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -eq 'install.log' -or $_.Name -like 'setup-*.log' })
    foreach ($f in $logTargets) {
        # 活动日志自身不删（此刻正被追加写入；其余 setup-*.log 全删）
        if ($f.FullName -ieq $script:LogPath) { $selfSkipped = $true; continue }
        try {
            Remove-Item -LiteralPath $f.FullName -Force -ErrorAction Stop
            $filesGone++
        } catch { $filesFail++ }
    }
    $note = ''
    if ($selfSkipped) { $note = '；活动日志自身保留（iss 可传 -Log 指向它处以全删）' }
    Out-Result "RESULT: PurgeData 目录删 $dirsGone 剩 $dirsFail；日志删 $filesGone 剩 $filesFail$note；exit=0"
    return 0
}

# ═══════════════════ 入口 ═══════════════════

# 库模式（自测/QA）：只定义函数，不执行命令
if ($env:HUFU_SETUP_TOOLS_LIB -eq '1') { return }

# 命令行回显进日志（每次运行必记）
$argEcho = New-Object System.Collections.Generic.List[string]
foreach ($c in @('BackupDiff', 'KillServer', 'ClearOverride', 'ElevatedUninstallTail', 'PostBoot', 'PurgeData')) {
    if ((Get-Variable -Name $c -ValueOnly).IsPresent) { $argEcho.Add("-$c") }
}
foreach ($p in @('Manifest', 'SrcRoot', 'DstRoot', 'BackupRoot', 'InstDir', 'Log')) {
    $v = [string](Get-Variable -Name $p -ValueOnly)
    if ($v) { $argEcho.Add("-$p `"$v`"") }
}
if ($Rounds -ne 12) { $argEcho.Add("-Rounds $Rounds") }
if ($ProcName -ne 'hufu-server') { $argEcho.Add("-ProcName `"$ProcName`"") }
Write-ToolLog ($argEcho -join ' ') 'CMD'

$rc = 1
$sw = [System.Diagnostics.Stopwatch]::StartNew()
if ($BackupDiff)            { $rc = Invoke-BackupDiffCmd }
elseif ($KillServer)        { $rc = Invoke-KillServerCmd }
elseif ($ClearOverride)     { $rc = Invoke-ClearOverrideCmd }
elseif ($ElevatedUninstallTail) { $rc = Invoke-ElevatedUninstallTailCmd }
elseif ($PostBoot)          { $rc = Invoke-PostBootCmd }
elseif ($PurgeData)         { $rc = Invoke-PurgeDataCmd }
else {
    Out-Result 'RESULT: 未指定命令（-BackupDiff/-KillServer/-ClearOverride/-ElevatedUninstallTail/-PostBoot/-PurgeData）exit=1' '✗'
    exit 1
}
$sw.Stop()
Write-ToolLog ("  耗时 $([math]::Round($sw.Elapsed.TotalMilliseconds))ms exit=$rc")
exit $rc
