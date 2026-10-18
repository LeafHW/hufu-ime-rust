# qa-backup-state.ps1 — HuFu exe 安装器 QA · 真机状态备份（纯只读取证）
# 用途：安装/卸载/升级测试前对"虎符在系统里的一切痕迹"做快照，
#       测试后用 qa-restore-state.ps1 -Compare 出差异报告（取证物）。
# 备份内容：
#   1) 注册表（reg export /y，UTF-16 .reg）：
#      HKCU\Software\HuFu、HKCU\Software\Microsoft\CTF 整树、
#      HKCU\Control Panel\International\User Profile、HKCU Run / RunOnce、
#      HKLM CTF\TIP\{CLSID}、HKLM CTF\Category、HKLM Classes\CLSID\{CLSID}、
#      HKLM Classes\WOW6432Node\CLSID\{CLSID}、
#      HKLM(HKCU) Uninstall 中 DisplayName 含 HuFu 的条目（枚举筛选逐个导出）
#   2) 文件清单+SHA256+icacls /save：C:\Windows\SystemIME、
#      C:\Windows\SysWOW64\SystemIME、C:\ProgramData\HuFu（递归；清单级，不拷贝文件本体）
#   3) 进程快照：hufu-server / ctfmon（Name/PID/Path）
#   4) PFRO（PendingFileRenameOperations）当前值留档
#   5) backup-index.txt（人读索引）+ backup-targets.txt（机读清单，restore 据此工作）
# 自测：-SelfTest 在 .testtmp 造 fixture 假键/假目录全流程演练（HKLM 假键自清理）。
# 红线：本脚本只读真机，不写任何真实键/文件（SelfTest fixture 除外）。
param(
    [string]$BackupDir = '',   # 省略 = E:\DSH-KF\.testtmp\setupqa\backup-<yyyyMMdd-HHmmss>
    [switch]$SelfTest          # fixture 自测（配套调用 qa-restore-state.ps1）
)

$ErrorActionPreference = 'Continue'

# ── 常量（与 install.ps1 / uninstall.ps1 对齐）──
$CLSID   = '{8F5C2A10-3E77-4B9C-A1D4-9E0B7C2F5A88}'
$PROFILE = '{8F5C2A11-3E77-4B9C-A1D4-9E0B7C2F5A88}'
$QA_ROOT = 'E:\DSH-KF\.testtmp\setupqa'
$script:RestoreScript = Join-Path $PSScriptRoot 'qa-restore-state.ps1'

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
           ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

# ── 日志辅助 ──
$script:Index = New-Object System.Collections.Generic.List[string]
function Add-Idx([string]$status, [string]$src, [string]$dst) {
    $line = '{0}  {1}  ->  {2}' -f $status, $src, $dst
    $script:Index.Add($line)
    Write-Host $line
}

# ── 注册表目标（mode: replace=restore 时先删后导（精确还原）；merge=merge 导入+差异报告）──
function Get-RegTargets([bool]$fixture) {
    if ($fixture) {
        return @(
            @{ Key = 'HKCU\Software\HuFuQAFixture';                                   Tag = 'fx-hkcu';    Mode = 'replace' }
            @{ Key = 'HKLM\SOFTWARE\HuFuQAFixture';                                   Tag = 'fx-hklm';    Mode = 'replace' }
        )
    }
    return @(
        @{ Key = 'HKCU\Software\HuFu';                                                Tag = 'hkcu-hufu';      Mode = 'replace' }
        @{ Key = 'HKCU\Software\Microsoft\CTF';                                       Tag = 'hkcu-ctf';       Mode = 'merge'   }
        @{ Key = 'HKCU\Control Panel\International\User Profile';                    Tag = 'hkcu-uprofile';  Mode = 'merge'   }
        @{ Key = 'HKCU\Software\Microsoft\Windows\CurrentVersion\Run';               Tag = 'hkcu-run';       Mode = 'merge'   }
        @{ Key = 'HKCU\Software\Microsoft\Windows\CurrentVersion\RunOnce';           Tag = 'hkcu-runonce';   Mode = 'merge'   }
        @{ Key = "HKLM\SOFTWARE\Microsoft\CTF\TIP\$CLSID";                           Tag = 'hklm-tip';       Mode = 'replace' }
        @{ Key = 'HKLM\SOFTWARE\Microsoft\CTF\Category';                             Tag = 'hklm-cat';       Mode = 'merge'   }
        @{ Key = "HKLM\SOFTWARE\Classes\CLSID\$CLSID";                               Tag = 'hklm-clsid';     Mode = 'replace' }
        @{ Key = "HKLM\SOFTWARE\Classes\WOW6432Node\CLSID\$CLSID";                   Tag = 'hklm-clsid32';   Mode = 'replace' }
    )
}

function Get-DirTargets([bool]$fixture, [string]$fixtureRoot) {
    if ($fixture) {
        return @( @{ Dir = Join-Path $fixtureRoot 'SystemIME'; Tag = 'fx-systemime' } )
    }
    return @(
        @{ Dir = "$env:SystemRoot\SystemIME";        Tag = 'systemime64' }
        @{ Dir = "$env:SystemRoot\SysWOW64\SystemIME"; Tag = 'systemime32' }
        @{ Dir = (Join-Path $env:ProgramData 'HuFu'); Tag = 'programdata' }
    )
}

# ── 注册表导出（返回 OK / MISSING）──
function Export-RegKey([string]$key, [string]$file) {
    & reg.exe export $key $file /y 2>$null | Out-Null
    if ($LASTEXITCODE -eq 0 -and (Test-Path $file)) { return 'OK' }
    if (Test-Path $file) { Remove-Item $file -Force -ErrorAction SilentlyContinue }
    return 'MISSING'
}

# ── Uninstall 条目枚举（DisplayName 含 HuFu）──
function Get-UninstallTargets([bool]$fixture) {
    if ($fixture) { return @() }
    $roots = @(
        'HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall',
        'HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall',
        'HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall'
    )
    $hits = @()
    foreach ($r in $roots) {
        Get-ChildItem $r -ErrorAction SilentlyContinue | ForEach-Object {
            $dn = ''
            try { $dn = [string](Get-ItemProperty $_.PSPath -ErrorAction Stop).DisplayName } catch {}
            if ($dn -match 'HuFu') { $hits += $_.PSPath }
        }
    }
    return $hits
}

# ── 文件清单 + ACL 快照 ──
function Save-DirManifest([string]$dir, [string]$tsv, [string]$acl) {
    if (-not (Test-Path $dir)) { return 'MISSING' }
    $lines = New-Object System.Collections.Generic.List[string]
    $lines.Add("relpath`tsize`tsha256")
    Get-ChildItem $dir -Recurse -Force -File -ErrorAction SilentlyContinue | ForEach-Object {
        $rel = $_.FullName.Substring($dir.Length).TrimStart('\')
        $sha = 'HASH_FAIL'
        try { $sha = (Get-FileHash $_.FullName -Algorithm SHA256 -ErrorAction Stop).Hash } catch {}
        $lines.Add($rel + "`t" + $_.Length + "`t" + $sha)
    }
    Set-Content -Path $tsv -Value $lines -Encoding UTF8
    & icacls.exe $dir /save $acl /t /c 2>$null | Out-Null
    if (Test-Path $acl) { return 'OK' }
    return 'ACL_FAIL'
}

# ── 主备份流程 ──
function Invoke-Backup([string]$bkDir, [bool]$fixture, [string]$fixtureRoot) {
    New-Item -ItemType Directory -Path $bkDir -Force | Out-Null
    $targets = @()
    $fail = 0

    # 1) 固定注册表键
    foreach ($t in (Get-RegTargets $fixture)) {
        $file = Join-Path $bkDir ('reg-{0}.reg' -f $t.Tag)
        $st = Export-RegKey $t.Key $file
        Add-Idx $st $t.Key $file
        if ($st -ne 'OK' -and $st -ne 'MISSING') { $fail++ }
        $targets += ('REG|{0}|{1}|reg-{0}.reg|{2}|{3}' -f $t.Tag, $t.Key, $st, $t.Mode)
    }

    # 2) Uninstall 条目（枚举筛选）
    $i = 0
    foreach ($uk in (Get-UninstallTargets $fixture)) {
        $i++
        $keyPlain = $uk -replace '^Microsoft\.PowerShell\.Core\\Registry::', ''
        $file = Join-Path $bkDir ('reg-uninstall-{0:d2}.reg' -f $i)
        $st = Export-RegKey $keyPlain $file
        Add-Idx $st $keyPlain $file
        if ($st -ne 'OK') { $fail++ }
        $targets += ('REG|uninstall-{0:d2}|{1}|reg-uninstall-{0:d2}.reg|OK|replace' -f $i, $keyPlain)
    }

    # 3) 文件三处
    foreach ($d in (Get-DirTargets $fixture $fixtureRoot)) {
        $tsv = Join-Path $bkDir ('files-{0}.tsv' -f $d.Tag)
        $acl = Join-Path $bkDir ('acl-{0}.txt' -f $d.Tag)
        $st = Save-DirManifest $d.Dir $tsv $acl
        Add-Idx $st $d.Dir $tsv
        if ($st -eq 'ACL_FAIL') { $fail++ }
        $targets += ('DIR|{0}|{1}|files-{0}.tsv|{2}' -f $d.Tag, $d.Dir, $st)
    }

    # 4) PFRO 留档
    $pfro = Join-Path $bkDir 'pfro.txt'
    & reg.exe query 'HKLM\SYSTEM\CurrentControlSet\Control\Session Manager' /v PendingFileRenameOperations 2>$null | Out-File $pfro -Encoding UTF8
    if ($LASTEXITCODE -ne 0) {
        Set-Content $pfro 'MISSING: PendingFileRenameOperations 值不存在（正常态）' -Encoding UTF8
        Add-Idx 'MISSING' 'PFRO (Session Manager)' $pfro
    } else {
        Add-Idx 'OK' 'PFRO (Session Manager)' $pfro
    }

    # 5) 进程快照
    $procFile = Join-Path $bkDir 'processes.txt'
    $procs = Get-Process hufu-server, ctfmon -ErrorAction SilentlyContinue |
        Select-Object Name, Id, Path | Format-Table -AutoSize | Out-String
    Set-Content $procFile $procs -Encoding UTF8
    Add-Idx 'OK' 'processes(hufu-server/ctfmon)' $procFile

    # 6) 机读清单 + 人读索引
    Set-Content (Join-Path $bkDir 'backup-targets.txt') $targets -Encoding UTF8
    $idxHeader = @(
        '# HuFu QA 状态备份索引',
        ('# 时间: ' + (Get-Date -Format 'yyyy-MM-dd HH:mm:ss')),
        ('# 模式: ' + $(if ($fixture) { 'SELFTEST-FIXTURE' } else { 'REAL' })),
        ('# 管理员: ' + $isAdmin),
        ('# 说明: MISSING=键/目录不存在（正常，如实记录）；OK=已导出'),
        ''
    )
    Set-Content (Join-Path $bkDir 'backup-index.txt') ($idxHeader + $script:Index) -Encoding UTF8
    Write-Host ''
    Write-Host ('备份完成: {0}（失败 {1} 项）' -f $bkDir, $fail) -ForegroundColor Cyan
    return $fail
}

# ── SelfTest：fixture 全流程演练 ──
function Invoke-SelfTest {
    $ts = Get-Date -Format 'yyyyMMdd-HHmmss'
    $stRoot = Join-Path $QA_ROOT "selftest-$ts"
    $fxRoot = Join-Path $stRoot 'fixture'
    $fxSys = Join-Path $fxRoot 'SystemIME'
    $fxKey = 'HKCU:\Software\HuFuQAFixture'
    $fxKeyLm = 'HKLM:\SOFTWARE\HuFuQAFixture'
    $results = New-Object System.Collections.Generic.List[string]
    function Assert([string]$name, [bool]$cond) {
        $results.Add(('{0}  {1}' -f $(if ($cond) { 'PASS' } else { 'FAIL' }), $name))
        if (-not $cond) { $script:SelfFail = $true }
        Write-Host ('  [{0}] {1}' -f $(if ($cond) { 'PASS' } else { 'FAIL' }), $name)
    }
    $script:SelfFail = $false

    Write-Host "== HuFu QA SelfTest（fixture: $stRoot）==" -ForegroundColor Cyan

    # 造 fixture：HKCU 假键 + 目录结构（模拟 SystemIME：HuFu/ + HuFu.old1/ 腾位链）
    New-Item -Path "$fxKey\$CLSID" -Force | Out-Null
    Set-ItemProperty "$fxKey" -Name 'InstallDir' -Value 'E:\fixture-fake' -Type String
    Set-ItemProperty "$fxKey\$CLSID" -Name '(default)' -Value 'QA Fixture' -Type String
    New-Item -Path "$fxKey\Sub" -Force | Out-Null
    Set-ItemProperty "$fxKey\Sub" -Name 'V1' -Value 'alpha' -Type String
    if ($isAdmin) {
        New-Item -Path $fxKeyLm -Force | Out-Null
        Set-ItemProperty $fxKeyLm -Name '(default)' -Value 'QA Fixture LM' -Type String
        New-Item -Path "$fxKeyLm\InprocServer32" -Force | Out-Null
        Set-ItemProperty "$fxKeyLm\InprocServer32" -Name '(default)' -Value 'C:\fake\qa.dll' -Type String
    } else {
        Write-Host '  （非管理员：HKLM fixture 部分跳过）' -ForegroundColor Yellow
    }
    $hu = Join-Path $fxSys 'HuFu'
    $old = Join-Path $fxSys 'HuFu.old1'
    New-Item -ItemType Directory -Path $hu, $old -Force | Out-Null
    [System.IO.File]::WriteAllBytes((Join-Path $hu 'hufu_tsf.dll'), [byte[]](1,2,3,4,5,6,7,8))
    [System.IO.File]::WriteAllBytes((Join-Path $hu 'hufu_tsf32.dll'), [byte[]](9,9,9))
    [System.IO.File]::WriteAllBytes((Join-Path $old 'hufu_tsf.dll'), [byte[]](1,1))
    Assert 'fixture 假键/假目录已建立' ($?)

    # ① 备份
    $bk = Join-Path $stRoot 'backup'
    $fail = Invoke-Backup $bk $true $fxRoot
    Assert 'backup 执行返回 0 失败项' ($fail -eq 0)
    Assert 'backup-index.txt 已生成' (Test-Path (Join-Path $bk 'backup-index.txt'))
    Assert 'backup-targets.txt 已生成' (Test-Path (Join-Path $bk 'backup-targets.txt'))

    # ② 破坏现场（模拟安装器动作）：改值 / 删子键 / 加子键 / 改 DLL / 加新文件 / （管理员）删 HKLM 子键
    Set-ItemProperty $fxKey -Name 'InstallDir' -Value 'CHANGED' -Type String
    Remove-Item "$fxKey\Sub" -Recurse -Force
    New-Item -Path "$fxKey\ExtraAfterBackup" -Force | Out-Null
    [System.IO.File]::WriteAllBytes((Join-Path $hu 'hufu_tsf.dll'), [byte[]](8,8,8,8))
    [System.IO.File]::WriteAllBytes((Join-Path $hu 'new-after-backup.txt'), [byte[]](1))
    if ($isAdmin) { Remove-Item "$fxKeyLm\InprocServer32" -Recurse -Force }

    # ③ Compare 应报出差异（断言读报告文件——Write-Host 流无法被管道捕获）
    & $script:RestoreScript -BackupDir $bk -Compare -SelfTest | Out-Null
    $cmpTxt = ''
    $repFile = Join-Path $bk 'compare-report.txt'
    if (Test-Path $repFile) { $cmpTxt = [System.IO.File]::ReadAllText($repFile) }
    Assert 'Compare 报告已生成' ($cmpTxt -ne '')
    Assert 'Compare 检出注册表差异' ($cmpTxt -match 'REG-DIFF|EXTRA-KEY|GONE-KEY')
    Assert 'Compare 检出文件差异' ($cmpTxt -match 'FILE-(CHANGED|ADDED|REMOVED)')

    # ④ Restore（fixture 模式：不动真机进程、不提权）
    & $script:RestoreScript -BackupDir $bk -Restore -SelfTest | Out-Null
    $v = ''
    try { $v = [string](Get-ItemProperty $fxKey -ErrorAction Stop).InstallDir } catch {}
    Assert 'Restore 后 InstallDir 恢复原值' ($v -eq 'E:\fixture-fake')
    Assert 'Restore 后被删子键 Sub\V1 恢复' ((Get-ItemProperty "$fxKey\Sub" -ErrorAction SilentlyContinue).V1 -eq 'alpha')
    if ($isAdmin) {
        Assert 'Restore 后 HKLM InprocServer32 恢复' ((Get-Item "$fxKeyLm\InprocServer32" -EA SilentlyContinue) -ne $null)
    }

    # ⑤ 备份后新增子键（ExtraAfterBackup）仍在 —— replace 模式键 restore 时应先删后导已清掉
    #    （fx-hkcu 为 replace 模式：整键删除重导，ExtraAfterBackup 应消失）
    $extraGone = (Get-Item "$fxKey\ExtraAfterBackup" -ErrorAction SilentlyContinue) -eq $null
    Assert 'replace 模式还原清除了备份后新增子键' $extraGone

    # ⑥ 清理 fixture（红线：HKLM 假键必须可自清理）
    Remove-Item $fxKey -Recurse -Force -ErrorAction SilentlyContinue
    if ($isAdmin) { Remove-Item $fxKeyLm -Recurse -Force -ErrorAction SilentlyContinue }
    Remove-Item $fxRoot -Recurse -Force -ErrorAction SilentlyContinue
    Assert 'HKCU fixture 假键已清理' (-not (Test-Path $fxKey))
    if ($isAdmin) { Assert 'HKLM fixture 假键已清理' (-not (Test-Path $fxKeyLm)) }
    Assert 'fixture 目录已清理' (-not (Test-Path $fxRoot))

    # 结果汇总
    $report = Join-Path $stRoot 'selftest-result.txt'
    Set-Content $report $results -Encoding UTF8
    Write-Host ''
    if ($script:SelfFail) {
        Write-Host "SELFTEST: FAIL（明细 $report）" -ForegroundColor Red
        exit 1
    }
    Write-Host "SELFTEST: PASS（明细 $report）" -ForegroundColor Green
    # PASS 后清掉本次 selftest 目录（保留失败现场）
    Remove-Item $stRoot -Recurse -Force -ErrorAction SilentlyContinue
    exit 0
}

# ── 入口 ──
if ($SelfTest) { Invoke-SelfTest; exit }

$dir = $BackupDir
if (-not $dir) {
    $dir = Join-Path $QA_ROOT ('backup-' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
}
if (Test-Path (Join-Path $dir 'backup-index.txt')) {
    Write-Host "目标目录已有备份索引，拒绝覆盖: $dir" -ForegroundColor Red
    exit 1
}
$failCnt = Invoke-Backup $dir $false ''
exit $(if ($failCnt -gt 0) { 1 } else { 0 })
