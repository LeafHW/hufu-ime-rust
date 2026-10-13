# qa-restore-state.ps1 — HuFu exe 安装器 QA · 状态对比/还原
# 与 qa-backup-state.ps1 配套：读取备份目录里的 backup-targets.txt（机读清单）工作。
# 模式：
#   -Compare   只对比当前态 vs 备份，输出差异报告（取证物），不改任何东西
#              （注册表逐键 export 后行级 diff；文件树+SHA256 diff；ACL/进程/PFRO 对照）
#   -Restore   真还原：
#              ① 杀 hufu-server → ② 导入备份 .reg
#                （mode=replace 的键先 reg delete 整键再导入=精确还原；
#                  mode=merge 的键 merge 导入，差异见 compare 报告）
#              ③ -PurgeExtras 时删除"备份后新增"的文件（SystemIME/ProgramData 需管理员）
#              ④ 重启 ctfmon
#              HKLM 导入需管理员：参考 uninstall.ps1 提权模式，非管理员时自动
#              Start-Process -Verb RunAs 重启自身跑 -ElevatedTail（HKLM 段）。
#   -SelfTest  与 backup 脚本的 -SelfTest 配套：跳过杀进程/重启 ctfmon/提权，
#              只按清单做 compare/restore（fixture 目标）。
# 退出码：Compare = 差异条数（封顶 255；0 = 与备份一致）；Restore = 0 成功 / 1 有失败项。
param(
    [Parameter(Mandatory = $true)][string]$BackupDir,
    [switch]$Compare,
    [switch]$Restore,
    [switch]$PurgeExtras,
    [switch]$SelfTest,
    [switch]$ElevatedTail    # 内部：提权子进程标记，只执行 HKLM/受保护文件段
)

$ErrorActionPreference = 'Continue'

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
           ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

if (-not (Test-Path $BackupDir)) { Write-Host "备份目录不存在: $BackupDir" -ForegroundColor Red; exit 2 }
$targetsFile = Join-Path $BackupDir 'backup-targets.txt'
if (-not (Test-Path $targetsFile)) { Write-Host "缺少 backup-targets.txt（请用 qa-backup-state.ps1 生成）" -ForegroundColor Red; exit 2 }

# ── 解析机读清单 ──
$regTargets = @()   # @{Tag;Key;File;Status;Mode}
$dirTargets = @()   # @{Tag;Dir;Tsv;Status}
foreach ($line in (Get-Content $targetsFile -Encoding UTF8)) {
    if ($line -notmatch '^\s*$') {
        $p = $line -split '\|'
        if ($p[0] -eq 'REG')  { $regTargets += @{ Tag = $p[1]; Key = $p[2]; File = $p[3]; Status = $p[4]; Mode = $p[5] } }
        if ($p[0] -eq 'DIR')  { $dirTargets += @{ Tag = $p[1]; Dir = $p[2]; Tsv = $p[3]; Status = $p[4] } }
    }
}

$report = New-Object System.Collections.Generic.List[string]
$diffCount = 0
function Write-Header([string]$mode, [string]$dir) {
    Write-Host ('===== HuFu QA {0}  备份: {1}  {2}  管理员: {3} =====' -f `
        $mode, $dir, (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'), $isAdmin) -ForegroundColor Cyan
}
function Add-Diff([string]$kind, [string]$detail) {
    $script:diffCount++
    $line = ('[{0}] {1}' -f $kind, $detail)
    $script:report.Add($line)
    Write-Host $line -ForegroundColor Yellow
}

# ── 注册表：当前态导出到临时 + 行级 diff ──
function Get-RegLines([string]$regFile) {
    # reg export 产物为 UTF-16LE；过滤空行
    try { return @((Get-Content $regFile -Encoding Unicode | Where-Object { $_ -notmatch '^\s*$' })) } catch { return $null }
}
function Compare-RegTarget($t) {
    $bkFile = Join-Path $BackupDir $t.File
    $tmp = Join-Path $env:TEMP ('qa-cmp-' + $t.Tag + '.reg')
    if (Test-Path $tmp) { Remove-Item $tmp -Force -ErrorAction SilentlyContinue }
    & reg.exe export $t.Key $tmp /y 2>$null | Out-Null
    $curOk = ($LASTEXITCODE -eq 0)

    if ($t.Status -eq 'MISSING') {
        if ($curOk) {
            $lines = Get-RegLines $tmp
            Add-Diff 'EXTRA-KEY' ("备份时不存在、现在存在: {0}（{1} 行）" -f $t.Key, @($lines).Count)
        } else { Write-Host ("[SAME] 两态均缺失: {0}" -f $t.Key) -ForegroundColor DarkGray }
        return
    }
    if (-not $curOk) { Add-Diff 'GONE-KEY' ("备份存在、现在消失: {0}" -f $t.Key); return }

    $bk = Get-RegLines $bkFile
    $cur = Get-RegLines $tmp
    $diff = Compare-Object -ReferenceObject $bk -DifferenceObject $cur
    if ($diff) {
        $sample = ($diff | Select-Object -First 8 | ForEach-Object { ('{0} {1}' -f $_.SideIndicator, $_.InputObject.Trim()) }) -join ' ; '
        Add-Diff 'REG-DIFF' ("{0}: {1} 行差异。样例: {2}" -f $t.Key, @($diff).Count, $sample)
    } else { Write-Host ("[SAME] 注册表一致: {0}" -f $t.Key) -ForegroundColor DarkGray }
    Remove-Item $tmp -Force -ErrorAction SilentlyContinue
}

# ── 文件清单：重算 manifest + 三分类 ──
function Get-CurrentManifest([string]$dir) {
    $h = @{}
    if (-not (Test-Path $dir)) { return $h }
    Get-ChildItem $dir -Recurse -Force -File -ErrorAction SilentlyContinue | ForEach-Object {
        $rel = $_.FullName.Substring($dir.Length).TrimStart('\')
        $sha = 'HASH_FAIL'
        try { $sha = (Get-FileHash $_.FullName -Algorithm SHA256 -ErrorAction Stop).Hash } catch {}
        $h[$rel] = @{ Size = $_.Length; Sha = $sha }
    }
    return $h
}
function Compare-DirTarget($d) {
    $bkTsv = Join-Path $BackupDir $d.Tsv
    if ($d.Status -eq 'MISSING') {
        if (Test-Path $d.Dir) { Add-Diff 'DIR-EXTRA' ("备份时目录不存在、现在存在: {0}" -f $d.Dir) }
        else { Write-Host ("[SAME] 目录两态均缺失: {0}" -f $d.Dir) -ForegroundColor DarkGray }
        return
    }
    if (-not (Test-Path $bkTsv)) { Add-Diff 'DIR-NOBK' ("备份清单文件丢失: {0}" -f $bkTsv); return }
    $before = $script:diffCount
    $bkFiles = @{}
    foreach ($l in (Get-Content $bkTsv -Encoding UTF8 | Select-Object -Skip 1)) {
        if ($l -match '^\s*$') { continue }
        $f = $l -split "`t"
        if ($f.Count -ge 3) { $bkFiles[$f[0]] = @{ Size = $f[1]; Sha = $f[2] } }
    }
    $cur = Get-CurrentManifest $d.Dir
    if (-not (Test-Path $d.Dir)) { Add-Diff 'DIR-GONE' ("备份时存在、现在整目录消失: {0}" -f $d.Dir); return }
    foreach ($k in $bkFiles.Keys) {
        if (-not $cur.Contains($k)) { Add-Diff 'FILE-REMOVED' ("{0} \ {1}" -f $d.Dir, $k) }
        elseif ($cur[$k].Sha -ne $bkFiles[$k].Sha -or [string]$cur[$k].Size -ne [string]$bkFiles[$k].Size) {
            Add-Diff 'FILE-CHANGED' ("{0} \ {1}（备份 {2}B/{3} → 现在 {4}B/{5}）" -f $d.Dir, $k, $bkFiles[$k].Size, $bkFiles[$k].Sha, $cur[$k].Size, $cur[$k].Sha)
        }
    }
    foreach ($k in $cur.Keys) {
        if (-not $bkFiles.Contains($k)) { Add-Diff 'FILE-ADDED' ("{0} \ {1}" -f $d.Dir, $k) }
    }
    if ($script:diffCount -eq $before) { Write-Host ("[SAME] 文件树一致: {0}" -f $d.Dir) -ForegroundColor DarkGray }
}

# ── PFRO / 进程对照（只展示）──
function Show-Context() {
    $tmp = Join-Path $env:TEMP 'qa-cmp-pfro.txt'
    & reg.exe query 'HKLM\SYSTEM\CurrentControlSet\Control\Session Manager' /v PendingFileRenameOperations 2>$null | Out-File $tmp -Encoding UTF8
    if ($LASTEXITCODE -ne 0) { Write-Host '[PFRO] 当前: （无 PFRO 值——正常态）' }
    else {
        Write-Host '[PFRO] 当前值（重点看是否残留虎符相关条目）:' -ForegroundColor Cyan
        Get-Content $tmp | Where-Object { $_ -match 'HuFu|hufu' } | ForEach-Object { Write-Host "  $_" -ForegroundColor Yellow }
        if (-not ((Get-Content $tmp | Out-String) -match 'HuFu|hufu')) { Write-Host '  （无 HuFu 相关条目）' }
    }
    Remove-Item $tmp -Force -ErrorAction SilentlyContinue
    Write-Host '[PROC] 当前 hufu-server/ctfmon:' -ForegroundColor Cyan
    $p = Get-Process hufu-server, ctfmon -ErrorAction SilentlyContinue | Select-Object Name, Id, Path | Format-Table -AutoSize | Out-String
    Write-Host $p
}

# ═══ Compare 模式 ═══
if ($Compare) {
    Write-Header 'Compare' ($BackupDir)
    foreach ($t in $regTargets) { Compare-RegTarget $t }
    foreach ($d in $dirTargets) { Compare-DirTarget $d }
    Show-Context
    $repFile = Join-Path $BackupDir 'compare-report.txt'
    $all = @()
    $all += ('=== HuFu QA Compare 报告  ' + (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'))
    $all += ''
    $all += $report
    $all += ''
    $all += ('差异总计: {0} 项' -f $diffCount)
    Set-Content $repFile $all -Encoding UTF8
    Write-Host ''
    Write-Host ('Compare 完成：差异 {0} 项，报告: {1}' -f $diffCount, $repFile) -ForegroundColor $(if ($diffCount -eq 0) { 'Green' } else { 'Yellow' })
    exit ([Math]::Min($diffCount, 255))
}

# ═══ Restore 模式 ═══
if ($Restore -and $ElevatedTail) {
    # 提权子进程段：只做 HKLM 导入 + 受保护目录的 extras 清理
    Write-Host '—— ElevatedTail：HKLM 还原（提权上下文）——'
    foreach ($t in $regTargets) {
        if ($t.Key -notlike 'HKLM\*') { continue }
        $bkFile = Join-Path $BackupDir $t.File
        if ($t.Status -ne 'OK' -or -not (Test-Path $bkFile)) { continue }
        if ($t.Mode -eq 'replace') {
            & reg.exe delete $t.Key /f 2>$null | Out-Null
        }
        & reg.exe import $bkFile 2>$null | Out-Null
        Write-Host ("  HKLM {0}: import rc={1}" -f $t.Key, $LASTEXITCODE)
    }
    if ($PurgeExtras) {
        foreach ($d in $dirTargets) {
            if ($d.Dir -notmatch 'SystemIME|ProgramData') { continue }
            # 备份时缺失的目录若现在存在=整体新增 → 删除；存在则在清单外文件即为新增
            if ($d.Status -eq 'MISSING') {
                if (Test-Path $d.Dir) {
                    Remove-Item $d.Dir -Recurse -Force -ErrorAction SilentlyContinue
                    Write-Host ("  已删除备份后新增目录: {0}" -f $d.Dir)
                }
                continue
            }
            $bkTsv = Join-Path $BackupDir $d.Tsv
            if (-not (Test-Path $bkTsv)) { continue }
            $bkRel = @{}
            foreach ($l in (Get-Content $bkTsv -Encoding UTF8 | Select-Object -Skip 1)) {
                if ($l -match '^\s*$') { continue }
                $f = $l -split "`t"; if ($f.Count -ge 3) { $bkRel[$f[0]] = $true }
            }
            Get-ChildItem $d.Dir -Recurse -Force -File -ErrorAction SilentlyContinue | ForEach-Object {
                $rel = $_.FullName.Substring($d.Dir.Length).TrimStart('\')
                if (-not $bkRel.Contains($rel)) {
                    Remove-Item $_.FullName -Force -ErrorAction SilentlyContinue
                    Write-Host ("  已删除备份后新增文件: {0}" -f $_.FullName)
                }
            }
        }
    }
    Write-Host '—— ElevatedTail 完成 ——'
    exit 0
}

if ($Restore) {
    Write-Header 'Restore' ($BackupDir)
    # ① 杀 server（SelfTest fixture 模式跳过真机进程操作）
    if (-not $SelfTest) {
        Get-Process hufu-server -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
        Write-Host '· hufu-server 已停止'
    }
    # ② HKCU 导入（普通权限可写）
    foreach ($t in $regTargets) {
        if ($t.Key -notlike 'HKCU\*') { continue }
        $bkFile = Join-Path $BackupDir $t.File
        if ($t.Status -ne 'OK' -or -not (Test-Path $bkFile)) { continue }
        if ($t.Mode -eq 'replace') {
            & reg.exe delete $t.Key /f 2>$null | Out-Null
        }
        & reg.exe import $bkFile 2>$null | Out-Null
        Write-Host ("  HKCU {0}: import rc={1}" -f $t.Key, $LASTEXITCODE)
    }
    # ③ HKLM 段：管理员直跑；非管理员且非 SelfTest → 提权重启自身（uninstall.ps1 模式）
    $needHklm = @($regTargets | Where-Object { $_.Key -like 'HKLM\*' -and $_.Status -eq 'OK' }).Count -gt 0
    $needFilePurge = $PurgeExtras -and (@($dirTargets | Where-Object { $_.Dir -match 'SystemIME|ProgramData' }).Count -gt 0)
    if ($needHklm -or $needFilePurge) {
        if ($isAdmin) {
            & $PSCommandPath -BackupDir $BackupDir -Restore -PurgeExtras:$PurgeExtras -ElevatedTail -SelfTest:$SelfTest
        } elseif ($SelfTest) {
            Write-Host '  （SelfTest 非管理员：HKLM fixture 段跳过）' -ForegroundColor Yellow
        } else {
            Write-Host '  （HKLM 还原需要管理员——弹 UAC，请在管理员终端或批准 UAC 后继续）' -ForegroundColor Yellow
            $ps = "$env:WINDIR\System32\WindowsPowerShell\v1.0\powershell.exe"
            $arg = "-NoProfile -ExecutionPolicy Bypass -File `"$PSCommandPath`" -BackupDir `"$BackupDir`" -Restore -PurgeExtras:$($PurgeExtras.IsPresent) -ElevatedTail"
            try {
                $p = Start-Process $ps -Verb RunAs -ArgumentList $arg -Wait -PassThru
                Write-Host ("  提权段退出码: {0}" -f $p.ExitCode)
            } catch {
                Write-Host '  ⚠ 提权被拒绝/失败——HKLM 未还原。请在管理员终端手动执行:' -ForegroundColor Red
                Write-Host ("     powershell -File `"{0}`" -BackupDir `"{1}`" -Restore -ElevatedTail" -f $PSCommandPath, $BackupDir)
            }
        }
    }
    # ④ 重启 ctfmon（真机模式；HKLM/受保护目录段已在上面 isAdmin 分支或提权子进程内完成）
    if (-not $SelfTest) {
        Start-Process ctfmon -ErrorAction SilentlyContinue
        Write-Host '· ctfmon 已拉起'
    }
    Write-Host ''
    Write-Host 'Restore 完成。注意：文件本体不回拷（清单级备份）；用 -Compare 复核剩余差异。' -ForegroundColor Cyan
    exit 0
}

# 无模式参数：打印用法
Write-Host '用法: qa-restore-state.ps1 -BackupDir <dir> [-Compare | -Restore [-PurgeExtras]] [-SelfTest]'
exit 2
