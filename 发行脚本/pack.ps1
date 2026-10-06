param([string]$Version = '1.4.7', [string]$ElevMark = '', [switch]$NoModel)
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
# HuFu 虎符输入法 · 固化打包脚本（唯一合法打包入口）
# ────────────────────────────────────────────────────────────
# 【铁律】本脚本与「打包源」「基线包」同在 E:\DSH-KF\hufu-发行，
# 打包产物只落在本目录。禁止再出现临时脚本/漂移路径打包。
#
# 流程（顺序固定）：
#   0) 前置断言：打包源在位、关键脚本特征在位
#   1) 同步最新二进制（构建输出 → 打包源）
#   2) 停服窗口（ctfmon+server）压缩
#   3) 自动核验 A：存在性（文件清单逐项）
#   4) 自动核验 B：语义特征（双阶段安装/32 位段/BOM/默认皮肤）
#   5) 自动核验 C：与基线包 diff（文件清单差集+关键文件内容对比）
#      —— 基线=上一版「用户验证过」的 zip，任何差异必须人工解释
#   6) 输出核验报告到 日志\，全绿才复制为正式 zip
#
# 用法：powershell -ExecutionPolicy Bypass -File pack.ps1 [-Version 1.4.2]
$ErrorActionPreference = 'Stop'
$REL  = 'E:\DSH-KF\hufu-发行'
$SRC  = "$REL\打包源"
$BASE = "$REL\基线包"
$LOGD = "$REL\日志"
# 【2026-09-11 无模型唯一路径（用户拍板）】不再打带模型的全量包——
# -NoModel 为正式分发形态：打包源排除 模型\ 目录、核验要求模型 0 个、
# 产物名固定 HuFu虎符输入法-v<版本>-无模型.zip。今后所有修改都落在这
# 条路径上；全量包能力保留仅作 -NoModel 缺省内部兜底，不再对外。
$zipName = if ($NoModel) { "HuFu虎符输入法-v$Version-无模型.zip" } else { "HuFu虎符输入法-v$Version.zip" }
$zip = "$REL\$zipName"
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$report = @()
function Check($ok, [string]$what) {
    $script:report += "$(if ($ok) {'[PASS]'} else {'[FAIL]'}) $what"
    if (-not $ok) { Write-Host "[FAIL] $what" -ForegroundColor Red } else { Write-Host "[PASS] $what" -ForegroundColor Green }
    $ok
}

# ── 0) 前置断言 ──
if (-not (Test-Path "$SRC\install.ps1")) { throw '打包源缺失（从上一版 zip 解压重建到 打包源\）' }
# 【T9·单一事实源 2026-10-06】安装/卸载脚本以仓内 platform\windows\install\
# 为准——打包时自动同步进打包源。此前是反向的手工拉平（打包源为主、仓内
# 为镜像），出过 6 天/23 天漂移事故：修了仓库忘了打包源=修复进不了包。
# 仓库侧缺失（他机打包）沿用打包源现有版并黄字提示；同步后下方语义特征
# 断言照常把关（防仓库侧回退/漂移）。
$repoInst = 'E:\DSH-KF\hufu\platform\windows\install'
if (Test-Path "$repoInst\install.ps1") {
    foreach ($s in @('install.ps1','uninstall.ps1')) {
        if (Test-Path "$repoInst\$s") {
            $rv = [IO.File]::ReadAllText("$repoInst\$s", [Text.Encoding]::UTF8)
            $sv = [IO.File]::ReadAllText("$SRC\$s", [Text.Encoding]::UTF8)
            if ($rv -cne $sv) {
                Copy-Item "$repoInst\$s" "$SRC\$s" -Force
                Write-Host "  [同步] $s ← 仓内 platform\windows\install（单一事实源）" -ForegroundColor Cyan
            }
        }
    }
} else {
    Write-Host '  [警告] 仓内 install 目录缺失——沿用打包源现有脚本（他机打包回退）' -ForegroundColor Yellow
}
$t = [System.IO.File]::ReadAllText("$SRC\install.ps1", [System.Text.Encoding]::UTF8)
$ok0 = $true
$ok0 = $ok0 -and (Check ($t -match 'PhaseElevated') 'install.ps1 双阶段版特征（用户可感知界面）')
$ok0 = $ok0 -and (Check ($t -match 'SysWOW64') 'install.ps1 含 32 位部署段')
# 【三十四修补 2026-09-13】语法+作用域双门禁：$dll32 空值 bug（v1.5.3
# 存量，主流程引用了只在提权分支定义的变量）两轮「本机验证全绿」都没
# 拦住——根因是验证输出被 Select-String 关键词过滤、红字报错被滤掉。
# 此后 pack 侧加硬门禁：
#   a) PowerShell Parser 语法零错误（必须 ParseInput+显式 UTF-8 文本——
#      ParseFile 路径版对 BOM 文件按 ANSI 误读中文会产生假阳性，禁用）；
#   b) 核心安装变量必须在脚本头部（首个 if 分支前）全部有定义——
#      拦「分支内定义、别处使用」的空值地雷。
$pErrs = $null; $pToks = $null
[System.Management.Automation.Language.Parser]::ParseInput($t, [ref]$pToks, [ref]$pErrs) | Out-Null
$ok0 = $ok0 -and (Check (@($pErrs).Count -eq 0) "install.ps1 语法零错误（实际 $(@($pErrs).Count)）")
$pErrs | Select-Object -First 3 | ForEach-Object { Write-Host ("    L" + $_.Extent.StartLineNumber + ': ' + $_.Message) -ForegroundColor Red }
$tLines = $t -split "`r?`n"
$firstIfHit = ($tLines | Select-String -Pattern '^if \(' | Select-Object -First 1)
if (-not $firstIfHit) { throw 'install.ps1 结构异常：找不到首个顶层 if（作用域门禁无法划界）' }
$firstIf = $firstIfHit.LineNumber
$headZone = ($tLines[0..([Math]::Max($firstIf - 2, 0))] -join "`n")
$missVars = @()
foreach ($v in @('dll','dll32','exe','icon','sysdir','sysdll','sysdll32','inst','data')) {
    if ($headZone -notmatch ('\$' + $v + '\s*=')) { $missVars += $v }
}
$ok0 = $ok0 -and (Check ($missVars.Count -eq 0) "install.ps1 核心变量头部定义齐全（缺失: $(if ($missVars) { $missVars -join ',' } else { '无' })）")
$u = [System.IO.File]::ReadAllText("$SRC\uninstall.ps1", [System.Text.Encoding]::UTF8)
$uErrs = $null; $uToks = $null
[System.Management.Automation.Language.Parser]::ParseInput($u, [ref]$uToks, [ref]$uErrs) | Out-Null
$ok0 = $ok0 -and (Check (@($uErrs).Count -eq 0) "uninstall.ps1 语法零错误（实际 $(@($uErrs).Count)）")
$ok0 = $ok0 -and (Check ($u -match 'NoHKLM') 'uninstall.ps1 双阶段版特征')
$ok0 = $ok0 -and (Check ($u -match 'SysWOW64') 'uninstall.ps1 含 32 位清理')
# 【T9·语义特征门禁 2026-10-06】防吃输入法二阶根治特征（回读校验+装配表
# 快照）必须在位——脚本被回退/漂移时 pack 直接 FAIL，不再依赖逐字节基线
# 碰运气（10-02 槽位修复就是这样被旧脚本挡在包外、10-06 晚实锤复发）。
$ok0 = $ok0 -and (Check ($t -match '回读') 'install.ps1 含「回读校验」防线（防吃输入法）')
$ok0 = $ok0 -and (Check ($t -match '快照') 'install.ps1 含「快照」防线（装配表地面真值）')
$ok0 = $ok0 -and (Check ($u -match '回读') 'uninstall.ps1 含「回读校验」防线（防吃输入法）')
if (-not $ok0) { throw '脚本特征断言失败：脚本版本不对，禁止打包（这就是货不对版的根源防线）' }
# 【bat 编码门禁 2026-09-07】cmd 按 ANSI/GBK 解析 bat：UTF-8 中文会碎成
# 乱码命令（卸载.bat 实测事故）。判定：含非 ASCII 字节且严格 UTF-8 解码
# 成功 = UTF-8 → FAIL；纯 ASCII（GBK 子集）或 GBK 编码 → PASS。
$strictUtf8 = New-Object System.Text.UTF8Encoding($false, $true)
foreach ($bf in @('卸载.bat', '安装.bat', '设置.bat')) {
    $bp = Join-Path $SRC $bf
    if (-not (Test-Path $bp)) { continue }
    $bb = [System.IO.File]::ReadAllBytes($bp)
    if ($bb.Length -ge 3 -and $bb[0] -eq 239 -and $bb[1] -eq 187 -and $bb[2] -eq 191) {
        # BOM 对 cmd 无害但会干扰判定——剥后再判
        if ($bb.Length -gt 3) { $bb = $bb[3..($bb.Length - 1)] } else { $bb = @() }
    }
    $hasNonAscii = $false
    foreach ($x in $bb) { if ($x -gt 127) { $hasNonAscii = $true; break } }
    $isUtf8 = $false
    if ($hasNonAscii) { try { [void]$strictUtf8.GetString($bb); $isUtf8 = $true } catch {} }
    [void](Check (-not $isUtf8) "$bf 非 UTF-8（GBK 兼容，cmd 中文安全）")
    if ($isUtf8) { throw "$bf 是 UTF-8：cmd 会乱码碎行（先转 ANSI/GBK）" }
    # 【CRLF 门禁】cmd 对 LF-only 的 bat 在中文长行处解析错位碎行（实测）。
    $lfOnly = 0
    for ($i = 0; $i -lt $bb.Length; $i++) {
        if ($bb[$i] -eq 10 -and ($i -eq 0 -or $bb[$i - 1] -ne 13)) { $lfOnly++ }
    }
    [void](Check ($lfOnly -eq 0) "$bf CRLF 行尾（LF-only=$lfOnly）")
    if ($lfOnly -gt 0) { throw "$bf 有 $lfOnly 个 LF-only 行尾：cmd 会解析碎行（转 CRLF）" }
}

# ── 1) 同步最新二进制 ──
# 【2026-09-07 发布瘦身】打包专用瘦身构建：独立 target-slim 目录 +
# profile 环境变量覆盖（DEBUG=0/STRIP=symbols——DLL 11.8→0.84MB，
# debuginfo/符号不进发布包；平时 target 目录保持 debug=true 供冻结
# 取证不受影响）。增量编译：无源码变化时秒级跳过。
Write-Host '—— 瘦身构建（target-slim）——'
$env:CARGO_PROFILE_RELEASE_DEBUG = '0'
$env:CARGO_PROFILE_RELEASE_STRIP = 'symbols'
$env:CARGO_TARGET_DIR = 'E:\DSH-KF\hufu\platform\windows\target-slim'
$tsfDir = 'E:\DSH-KF\hufu\platform\windows'
$builds = @(
    @{ dir = $tsfDir; args = @('--release', '--target', 'x86_64-pc-windows-gnu', '-p', 'hufu-tsf') },
    @{ dir = $tsfDir; args = @('--release', '--target', 'i686-pc-windows-gnu', '-p', 'hufu-tsf') },
    @{ dir = $tsfDir; args = @('--release', '-p', 'hufu-tsf-smoke') }
)
foreach ($b in $builds) {
    Push-Location $b.dir
    cmd /c "cargo build $($b.args -join ' ') 2>&1" | ForEach-Object { Write-Host "  $_" }
    $code = $LASTEXITCODE
    Pop-Location
    if ($code -ne 0) { throw "瘦身构建失败: cargo build $($b.args -join ' ')" }
}
$env:CARGO_TARGET_DIR = 'E:\DSH-KF\hufu\engine\target-slim'
Push-Location 'E:\DSH-KF\hufu\engine'
cmd /c 'cargo build --release -p hufu-server 2>&1' | ForEach-Object { Write-Host "  $_" }
$code = $LASTEXITCODE
Pop-Location
Remove-Item Env:CARGO_TARGET_DIR, Env:CARGO_PROFILE_RELEASE_DEBUG, Env:CARGO_PROFILE_RELEASE_STRIP
if ($code -ne 0) { throw '瘦身构建失败: hufu-server' }
Write-Host '  [OK] 瘦身构建完成'
$copies = @(
    @('E:\DSH-KF\hufu\platform\windows\target-slim\x86_64-pc-windows-gnu\release\hufu_tsf.dll', (Join-Path $SRC 'hufu_tsf.dll')),
    @('E:\DSH-KF\hufu\platform\windows\target-slim\i686-pc-windows-gnu\release\hufu_tsf.dll', (Join-Path $SRC 'hufu_tsf32.dll')),
    @('E:\DSH-KF\hufu\engine\target-slim\release\hufu-server.exe', (Join-Path $SRC 'hufu-server.exe')),
    @('E:\DSH-KF\hufu\platform\windows\target-slim\release\hufu-tsf-smoke.exe', (Join-Path $SRC 'hufu-tsf-smoke.exe'))
)
foreach ($c in $copies) {
    if (Test-Path $c[0]) { Copy-Item $c[0] $c[1] -Force; [void](Check $true "同步 $($c[1].Split([char]92)[-1])") }
    else { [void](Check $false "源缺失 $($c[0])"); throw "构建产物缺失：$($c[0])" }
}

# 清理垃圾
Get-ChildItem $SRC -Filter '*.old*' -EA SilentlyContinue | ForEach-Object { $_.Attributes = 'Normal'; Remove-Item $_.FullName -Force -EA SilentlyContinue }
Remove-Item "$SRC\install.log" -Force -EA SilentlyContinue

# ── 1b) 同步真机码表（用户 2026-09-06 拍板：以后打包都用真机当前码表）──
# 真机 码表 整目录镜像到打包源（方案文件+快符+必看说明+补充语料
# 全部以真机为准）。个人运行时数据（用户调整.txt / adj-audit.log /
# user-adjust.log）剔除——它们是安装者自己操作产生的，干净包从空开始。
# 【2026-09-06b】安装目录随版本号升级（v1.4.6→v1.4.7→…）——动态跟随
# D:\HUFU 下最新的 HuFu虎符输入法-v* 目录。
# 【一级目录布局 2026-09-05】码表/模型已从 数据\ 提升到安装根。
$liveMa = Get-ChildItem 'D:\HUFU' -Directory -Filter 'HuFu虎符输入法-v*' -EA SilentlyContinue |
    Sort-Object { try { [version]($_.Name -replace '^.*-v','') } catch { [version]'0.0' } } -Descending |
    Select-Object -First 1 | ForEach-Object { Join-Path $_.FullName '码表' }
if (-not $liveMa -or -not (Test-Path $liveMa)) {
    # 【2026-09-07 回退】真机未安装（已卸载/他机打包）：沿用打包源现有
    # 码表镜像（上次镜像+瘦身成果），不中断打包。
    if (Test-Path "$SRC\码表") {
        Write-Host '  [跳过] 真机未安装——沿用打包源现有码表镜像' -ForegroundColor Yellow
    } else {
        [void](Check $false '真机码表目录不存在（D:\HUFU\HuFu虎符输入法-v*\码表）')
        throw '真机码表目录缺失且打包源无镜像，无法打包'
    }
}
if ($liveMa -and (Test-Path $liveMa)) {
    if (Test-Path "$SRC\码表") { Remove-Item "$SRC\码表" -Recurse -Force }
    Copy-Item $liveMa "$SRC\码表" -Recurse -Force
    Get-ChildItem "$SRC\码表" -Recurse -Include '用户调整.txt','adj-audit.log','user-adjust.log' -File -EA SilentlyContinue | ForEach-Object {
        Remove-Item $_.FullName -Force
        Write-Host "  [剔] 个人数据: $($_.FullName.Substring($SRC.Length+1))"
    }
    # 【2026-09-07 大统一瘦身】反查/注释/拆分已全局化（数据\拼音反查\ 等），
    # 方案目录里的旧副本不再入包（各方案一份 → 全局一份，包体积 -25MB+）。
    Get-ChildItem "$SRC\码表" -Directory | ForEach-Object {
        Get-ChildItem $_.FullName -File | Where-Object { $_.Name -match '反查|注释|拆分' } | ForEach-Object {
            Remove-Item $_.FullName -Force
            Write-Host "  [剔] 冗余资源: $($_.Directory.Name)\$($_.Name)"
        }
    }
    # 【2026-09-07 用户拍板】微软双拼反查表删除（无人使用）；全拼/自然码
    # 已按小鹤蓝本过滤瘦身（82万→36.4万条）。镜像若带回微软表/超重表，
    # 打包时剔除，防回潮。
    Remove-Item "$SRC\数据\拼音反查\微软双拼.txt" -Force -EA SilentlyContinue
    # 【2026-09-07 用户拍板】反查表剔除特别生僻单字（不在 Jun Da 9900 字
    # 频表；hanzi_db.csv 在本脚本同目录）：三表每次打包重筛（幂等防回潮）。
    {
        $freq = New-Object 'System.Collections.Generic.HashSet[string]'
        $csv = Join-Path $PSScriptRoot 'hanzi_db.csv'
        # 【2026-09-11 回退】字频表是打包依赖（曾在大清理中被当杂项误删致打包
        # 中止）——本地缺失时自动回退 语料\ 副本并复原，双处皆无才报错。
        if (-not (Test-Path $csv)) {
            $bak = 'E:\DSH-KF\语料\hanzi_db.csv'
            if (Test-Path $bak) { Copy-Item $bak $csv -Force; Write-Host '  [复原] hanzi_db.csv ← 语料\ 副本' }
        }
        if (-not (Test-Path $csv)) { throw "pack: hanzi_db.csv 缺失（Jun Da 字频白名单，反查防回潮必需）" }
        $sr = New-Object System.IO.StreamReader($csv, [System.Text.UTF8Encoding]::new($false))
        [void]$sr.ReadLine()
        while ($null -ne ($ln = $sr.ReadLine())) {
            $ch = ($ln -split ',')[1]
            if ($ch) { [void]$freq.Add($ch.Trim()) }
        }
        $sr.Close()
        if ($freq.Count -lt 9000) { throw "pack: 字频白名单异常 $($freq.Count)——中止防误删" }
        foreach ($t in @('小鹤双拼', '全拼', '自然码')) {
            $tbl = "$SRC\数据\拼音反查\$t.txt"
            if (-not (Test-Path $tbl)) { continue }
            $in = New-Object System.IO.StreamReader($tbl, [System.Text.UTF8Encoding]::new($false))
            $keepLines = New-Object System.Collections.Generic.List[string]
            $cutN = 0
            while ($null -ne ($ln = $in.ReadLine())) {
                $word = ($ln -split "`t")[0]
                $cs = @($word.ToCharArray())
                if ($cs.Count -eq 1 -and -not $freq.Contains([string]$cs[0])) { $cutN++; continue }
                $keepLines.Add($ln)
            }
            $in.Close()
            if ($cutN -gt 0) {
                [System.IO.File]::WriteAllLines($tbl, $keepLines, [System.Text.UTF8Encoding]::new($false))
                Write-Host "  [剔] 反查生僻单字: $t.txt ×$cutN"
            }
        }
    }.Invoke()
    # 【2026-09-07 用户拍板】这两个码表方案不再入包（以后打包也不放）。
    foreach ($s in @('虎字-繁体优先', 'B定制-常用')) {
        if (Test-Path "$SRC\码表\$s") {
            Remove-Item "$SRC\码表\$s" -Recurse -Force
            Write-Host "  [剔] 码表方案: $s"
        }
    }
}

# 每个方案文件夹必须有功能两件套（必看说明+快符——{} 功能词载体）
# 【2026-09-07】核验移出镜像块：真机回退模式（沿用打包源）同样核验
$maDirs = Get-ChildItem "$SRC\码表" -Directory
foreach ($d in $maDirs) {
    [void](Check (Test-Path "$($d.FullName)\必看！功能模块说明.txt") "码表\$($d.Name)\必看！功能模块说明.txt")
    [void](Check (Test-Path "$($d.FullName)\快符.txt") "码表\$($d.Name)\快符.txt")
}
[void](Check (Test-Path "$SRC\码表\虎整句\补充语料.txt") '虎整句\补充语料.txt（整句提权词表）')
if ($liveMa -and (Test-Path $liveMa)) { Write-Host "  [OK] 真机码表已镜像（$($maDirs.Count) 个方案）" }
else { Write-Host "  [OK] 沿用打包源码表镜像（$($maDirs.Count) 个方案）" }
# config.json：/ 直出档默认关闭（命名空间档= /jc /jq 等 / 前缀功能开箱即用）
$cfgPath = "$SRC\数据\config.json"
$j2 = Get-Content $cfgPath -Raw -Encoding UTF8 | ConvertFrom-Json
if ($j2.input.slash_dunhao -ne $false) {
    $j2.input.slash_dunhao = $false
    $j2 | ConvertTo-Json -Depth 20 | Out-File $cfgPath -Encoding utf8 -NoNewline
    Write-Host '  [FIX] config.json slash_dunhao 已置 false（命名空间档）'
}

# 【外发包不带学习数据】（用户 2026-09-05 约定）：user-adjust.log 是
# 安装者的个人选词记录（含打字痕迹），发给别人必须剔除；干净包安装
# 后从空学习状态开始。真机自己的日志不受影响。
# 【2026-09-05b 全目录通配】tbench/真机切方案会把学习日志写到
# 码表\<方案>\user-adjust.log（引擎按方案目录落盘），单点剔除不够——
# 通配全部位置，任何 user-adjust.log 都不得进包。
Get-ChildItem "$SRC" -Recurse -Filter 'user-adjust.log' -File -EA SilentlyContinue | ForEach-Object {
    Remove-Item $_.FullName -Force
    Write-Host "  [剔] 学习数据: $($_.FullName.Substring($SRC.Length+1))"
}
# 【2026-10-02】用户从设置页导出的码表（真机镜像会带回）：个人数据，
# 干净包不带——「导出 - *.txt」通配剔除（导入导出功能命名约定）。
Get-ChildItem "$SRC\码表" -Recurse -File -EA SilentlyContinue | Where-Object { $_.Name -like '导出 -*' } | ForEach-Object {
    Remove-Item $_.FullName -Force
    Write-Host "  [剔] 用户导出: $($_.FullName.Substring($SRC.Length+1))"
}
# 【出厂数据门禁 2026-10-02】config.json 是「出厂默认」，不是真机镜像：
# 皮肤/码表有真机镜像段，config 没有——但历史上曾被真机个人值污染
# （page_size=3 / show_index=false / sound=true / recent_pair 顺序全是
# 设置页个人改动，随包流出）。四个哨兵键=出厂共识值，任一偏离即 FAIL。
$cfgChk = Get-Content "$SRC\数据\config.json" -Raw -Encoding UTF8 | ConvertFrom-Json
[void](Check ($cfgChk.candidates.page_size -eq 4) 'config 出厂哨兵: candidates.page_size=4')
[void](Check ($cfgChk.candidates.show_index -eq $true) 'config 出厂哨兵: candidates.show_index=true')
[void](Check ($cfgChk.sound.enabled -eq $false) 'config 出厂哨兵: sound.enabled=false')
[void](Check (@($cfgChk.schema.recent_pair)[0] -eq '个人自用') 'config 出厂哨兵: recent_pair[0]=个人自用')
# 【用户学习默认关 2026-10-05】auto_frequency/log_adjust 默认改关（2026-11
# 用户拍板注释）——出厂 config 若显式写值必须与代码默认一致，防旧值回流。
[void](Check ($cfgChk.user.auto_frequency -eq $false) 'config 出厂哨兵: user.auto_frequency=false')
[void](Check ($cfgChk.user.log_adjust -eq $false) 'config 出厂哨兵: user.log_adjust=false')
# 【皮肤出厂哨兵 2026-10-02b】默认皮肤 hilite_on 必须为 true（用户
# 拍板：高亮默认开）。曾随 config 同路泄漏——live 验证「关」效果时
# 设置页写回 chenwu/moyan 皮肤，真机镜像又带回包。
$skinChk = Get-Content "$SRC\数据\皮肤\hufu-chenwu.json" -Raw -Encoding UTF8 | ConvertFrom-Json
[void](Check ($skinChk.material.hilite_on -eq $true) '皮肤出厂哨兵: 默认皮肤 hufu-chenwu hilite_on=true')
$left = @(Get-ChildItem "$SRC" -Recurse -Filter 'user-adjust.log' -File -EA SilentlyContinue)
if ($left.Count -gt 0) { throw "学习数据剔除失败：仍存在 $($left.Count) 个 user-adjust.log" }

# ── 1c) 同步真机皮肤（2026-09-19 用户拍板：以后打包都带当前皮肤集）──
# 真机 数据\皮肤 整目录镜像到打包源（用户在设置页做/改的皮肤自动入包，
# 默认皮肤=数据\config.json 的 appearance.skin）。真机未安装时沿用打
# 包源现有皮肤镜像（他机打包回退，同码表语义）。
$liveSkin = Get-ChildItem 'D:\HUFU' -Directory -Filter 'HuFu虎符输入法-v*' -EA SilentlyContinue |
    Sort-Object { try { [version]($_.Name -replace '^.*-v','') } catch { [version]'0.0' } } -Descending |
    Select-Object -First 1 | ForEach-Object { Join-Path $_.FullName '数据\皮肤' }
if ($liveSkin -and (Test-Path $liveSkin)) {
    if (Test-Path "$SRC\数据\皮肤") { Remove-Item "$SRC\数据\皮肤" -Recurse -Force }
    Copy-Item $liveSkin "$SRC\数据\皮肤" -Recurse -Force
    $skN = @(Get-ChildItem "$SRC\数据\皮肤" -Filter 'hufu-*.json').Count
    Write-Host "  [OK] 真机皮肤镜像 $skN 款"
    # 【挂件图自动清空 2026-10-06】真机默认皮肤可能带着用户正在用的挂件
    # 图（v1.7.0 二包实录：chenwu 带图被下方哨兵拒 pack）。出厂包必须为
    # 空——只在打包镜像里清（真机数据不动）；无 BOM UTF-8 直写
    #（serde_json 拒 BOM）。
    $stripN = 0
    Get-ChildItem "$SRC\数据\皮肤" -Filter 'hufu-*.json' | ForEach-Object {
        $sj = Get-Content $_.FullName -Raw -Encoding UTF8 | ConvertFrom-Json
        $ch = $false
        if ($sj.overlay) {
            if ($sj.overlay.image) { $sj.overlay.image = ''; $ch = $true }
            foreach ($p in @($sj.overlay.presets)) { if ($p.image) { $p.image = ''; $ch = $true } }
        }
        if ($ch) {
            [System.IO.File]::WriteAllText($_.FullName, ($sj | ConvertTo-Json -Depth 32), (New-Object System.Text.UTF8Encoding($false)))
            $stripN++
        }
    }
    if ($stripN) { Write-Host "  [清] 出厂挂件图置空: $stripN 款（真机数据不动）" }
    # 【挂件防污染 2026-10-05】真机皮肤会把测试期挂件图（base64 data:
    # URL / 本机路径）原样镜像进包——「候选挂件」页一开就有东西（v1.7.0
    # 首包实测）。出厂皮肤 overlay.image 与 presets[].image 必须全空串
    # （命名配置可保留，图必须清——首修只查主配置漏了 presets，同一张
    # GIF 经命名配置二度入包 +691KB 实录）：非空一律拒绝打包。
    $dirty = @()
    Get-ChildItem "$SRC\数据\皮肤" -Filter 'hufu-*.json' | ForEach-Object {
        $sj = Get-Content $_.FullName -Raw -Encoding UTF8 | ConvertFrom-Json
        if ($sj.overlay) {
            if ($sj.overlay.image) { $dirty += "$($_.Name):image" }
            $pi = @($sj.overlay.presets | Where-Object { $_.image })
            if ($pi.Count) { $dirty += "$($_.Name):presets x$($pi.Count)" }
        }
    }
    if ($dirty) { throw "皮肤挂件图未清空（overlay.image/presets 非空）: $($dirty -join ', ')——清掉后再打包" }
    Write-Host '  [OK] 皮肤挂件图全空（overlay.image + presets 出厂为空）'
} else {
    Write-Host '  [跳过] 真机未安装——沿用打包源现有皮肤镜像' -ForegroundColor Yellow
}

# ── 2) 停服压缩 ──
# 【2026-09-07 腾位断链法】此前「杀 6 轮」不可靠：ctfmon 被系统秒拉、
# 新 ctfmon 里的 DLL 守护 1-2 秒内复活 server，六轮耗尽仍失败。
# 改用：腾位 SystemIME\HuFu（DLL 加载不了→ctfmon 重启后无守护）→
# 杀一次即净 → 压缩 → 恢复目录 → 【关键】重挂语言列表（ctfmon 对加载
# 失败的 TIP 不重试，仅杀重启修不回来——2026-09-07 打包把真机输入法
# 打失效的实测教训）→ 启 ctfmon → 验证守护复活。
# 需要 C:\Windows 写权限：非管理员时自提权重跑（elev 防循环）。
# 【2026-09-12 无 UAC 打包】用户环境弹不出 UAC（看不到确认框）——
# 非管理员时不再走提权重跑（必悬死），改判守护链是否已由调用方
# 杀净：server 已死即无 DLL 守护在跑（腾位的唯一目的是杀得净
# server；压缩对象=打包源，与 ctfmon 进程无文件冲突）——跳过腾位
# 与重挂语言列表直接压缩。server 活着才中止（此时腾位不可行但
# 又必要，报错让人工处理）。
$isAdminPack = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
              ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
$skipTeng = $false
if (-not $isAdminPack) {
    if (Get-Process hufu-server -EA SilentlyContinue) {
        throw '非管理员且 hufu-server 未杀净（无 UAC 腾位不可行）——先停 server 再打包'
    }
    $skipTeng = $true
    Write-Host '  [无 UAC 模式] server 已死——跳过腾位直接压缩（不触碰 SystemIME/语言列表）' -ForegroundColor Yellow
}
if (-not $skipTeng -and -not $isAdminPack -and $ElevMark -ne 'elev') {
    Write-Host '需要管理员（腾位 SystemIME 断 DLL 守护链），请求提权重跑…'
    # 【2026-09-11 -Wait 悬死修复】Start-Process -Wait 在非交互后台宿主里
    # 对 RunAs 提权子进程偶发永不返回（子进程已退出甚至从未创建，句柄不
    # signal——本日两次一小时悬死实录）。改 PassThru + 存活性轮询，15 分
    # 钟超时兜底；完成与否以最终 zip 产物为准，不依赖此处等待。
    $ep = Start-Process powershell -Verb RunAs -ArgumentList "-ExecutionPolicy Bypass -File `"$PSCommandPath`" -Version $Version elev $(if ($NoModel) {'-NoModel'})" -PassThru
    $t0 = Get-Date
    while (((Get-Date) - $t0).TotalMinutes -lt 15) {
        $alive = $true
        try { $alive = -not $ep.HasExited } catch { $alive = [bool](Get-Process -Id $ep.Id -ErrorAction SilentlyContinue) }
        if (-not $alive) { break }
        Start-Sleep -Seconds 2
    }
    exit
}
$sysIme = "$env:SystemRoot\SystemIME\HuFu"
$sysBak = $null
if (-not $skipTeng -and (Test-Path $sysIme)) {
    $n = 1; while (Test-Path "$env:SystemRoot\SystemIME\HuFu.pack$n") { $n++ }
    $sysBak = "HuFu.pack$n"
    Rename-Item $sysIme $sysBak -Force
}
Stop-Process -Name ctfmon -Force -EA SilentlyContinue
Start-Sleep -Seconds 1
# 【三十四修 2026-09-13】旧版单杀单查被 DLL 守护链击败：杀 server 后
# ctfmon/SearchHost 里仍活的 DLL 实例会按 InstallDir 立即拉起新 server
#（本机实锤：拉起 v1.5.2 旧目录的 exe，pid 秒换）→「腾位后仍未退出」
# 假异常。改循环击杀 3 轮（每轮杀完 800ms 复查），SystemIME 已改名
# 隔断的进程中 DLL 拉活有限；3 轮后仍活才是真异常。
$killRounds = 0
while ($killRounds -lt 3) {
    Get-Process hufu-server -EA SilentlyContinue | Stop-Process -Force -EA SilentlyContinue
    Start-Sleep -Milliseconds 800
    if (-not (Get-Process hufu-server -EA SilentlyContinue)) { break }
    Stop-Process -Name ctfmon -Force -EA SilentlyContinue
    $killRounds++
}
if (Get-Process hufu-server -EA SilentlyContinue) { throw "腾位后 server 仍未退出（$killRounds 轮击杀后仍复活，真异常）" }
Write-Host "  [OK] server 已停（$killRounds 轮内）"
$tmpZip = "$REL\.tmp-$zipName"
if (Test-Path $tmpZip) { Remove-Item $tmpZip -Force }
$items = Get-ChildItem $SRC | Where-Object { $_.Name -notmatch '\.old' -and $_.Name -ne 'install.log' -and -not ($NoModel -and $_.Name -eq '模型') }
# 【2026-10-05 分隔符归一】PS5.1 Compress-Archive 写 '\' 条目名（zip
# 规范为 '/'，且历版发布 zip 均为 '/'——v1.7.0 首打全量 91/91 误报实
# 录）。改 .NET 直写：条目名一律 '/' 分隔（PS5.1/PS7 行为一致）；空
# 目录补显式目录条目（与 Compress-Archive 布局同构）。
Add-Type -AssemblyName System.IO.Compression.FileSystem
$za = [System.IO.Compression.ZipFile]::Open($tmpZip, 'Create')
foreach ($it in $items) {
    if ($it.PSIsContainer) {
        Get-ChildItem $it.FullName -Recurse -File | ForEach-Object {
            $rel = $_.FullName.Substring($it.FullName.Length + 1).Replace([char]92, '/')
            [void][System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($za, $_.FullName, "$($it.Name)/$rel", [System.IO.Compression.CompressionLevel]::Optimal)
        }
        Get-ChildItem $it.FullName -Recurse -Directory | Where-Object { -not (Get-ChildItem $_.FullName -Recurse -File) } | ForEach-Object {
            $rel = $_.FullName.Substring($it.FullName.Length + 1).Replace([char]92, '/')
            [void]$za.CreateEntry("$($it.Name)/$rel/")
        }
        if (-not (Get-ChildItem $it.FullName -Recurse -File)) { [void]$za.CreateEntry("$($it.Name)/") }
    } else {
        [void][System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($za, $it.FullName, $it.Name, [System.IO.Compression.CompressionLevel]::Optimal)
    }
}
$za.Dispose()
# 恢复 SystemIME + 重挂语言列表 + 启 ctfmon（勿在压缩中途做——mmap 释放后才安全）
if ($sysBak) {
    if (Test-Path $sysIme) { Remove-Item $sysIme -Recurse -Force -EA SilentlyContinue }
    Rename-Item "$env:SystemRoot\SystemIME\$sysBak" 'HuFu' -Force
}
# 【无 UAC 模式】未腾位（SystemIME 全程在位、ctfmon 由系统自动拉起）
# ——DLL 加载链从未断过，无需重挂语言列表；腾位过才需要。
if (-not $skipTeng) {
    $tipStr = "0804:{8F5C2A10-3E77-4B9C-A1D4-9E0B7C2F5A88}{8F5C2A11-3E77-4B9C-A1D4-9E0B7C2F5A88}"
    # 【吃掉其他输入法·二阶根治 2026-10-06】本块在 ctfmon 停止窗口内
    # 读写语言列表——Get 少报 + Set -Flush 会把第三方输入法抹掉（与
    # install/uninstall 同病）。装配表快照做地面真值 + Set 后回读并回。
    $asmChk = 'HKCU:\Software\Microsoft\CTF\SortOrder\AssemblyItem\0x00000804\{34745C63-B2F0-4784-8B67-5E12C8701A31}'
    $assemblyTips = @()
    if (Test-Path $asmChk) {
        foreach ($k in Get-ChildItem $asmChk -ErrorAction SilentlyContinue) {
            $p = Get-ItemProperty $k.PSPath -ErrorAction SilentlyContinue
            if ($p.CLSID -and $p.Profile -and $p.CLSID -ne '{8F5C2A10-3E77-4B9C-A1D4-9E0B7C2F5A88}') {
                $assemblyTips += "0804:$($p.CLSID)$($p.Profile)"
            }
        }
    }
    $list = Get-WinUserLanguageList
    foreach ($l in $list) {
        if ($l.InputMethodTips -contains $tipStr) {
            $keep = @($l.InputMethodTips | Where-Object { $_ -ne $tipStr })
            $l.InputMethodTips.Remove($tipStr) | Out-Null
            Set-WinUserLanguageList $list -Force -WarningAction SilentlyContinue
            Start-Sleep -Seconds 1
            $l.InputMethodTips.Add($tipStr) | Out-Null
            Set-WinUserLanguageList $list -Force -WarningAction SilentlyContinue
            # 回读校验：列表若丢了别人（枚举少报被固化），用快照并回重写
            $list2 = Get-WinUserLanguageList
            $l2 = $list2 | Where-Object { $_.LanguageTag -eq $l.LanguageTag } | Select-Object -First 1
            if ($l2) {
                $known = @((@($keep) + @($assemblyTips)) | Select-Object -Unique)
                $dropped = @($known | Where-Object { $l2.InputMethodTips -notcontains $_ })
                if ($dropped.Count -gt 0) {
                    Write-Host "⚠ 重挂丢失 $($dropped.Count) 个既有输入法（枚举少报），已检出并并回" -ForegroundColor Yellow
                    foreach ($d in $dropped) {
                        if ($l2.InputMethodTips -notcontains $d) { $l2.InputMethodTips.Add($d) }
                    }
                    Set-WinUserLanguageList $list2 -Force -WarningAction SilentlyContinue
                    Write-Host "OK 已并回 $($dropped.Count) 个被丢输入法（打包防丢生效）"
                }
            }
            break
        }
    }
}
Start-Process ctfmon -EA SilentlyContinue

Add-Type -AssemblyName System.IO.Compression.FileSystem
# ── 3) 核验 A：存在性 ──
$arc = [System.IO.Compression.ZipFile]::OpenRead($tmpZip)
$all = $true
foreach ($n in @('hufu_tsf.dll','hufu_tsf32.dll','hufu-server.exe','hufu-tsf-smoke.exe','install.ps1','uninstall.ps1','安装.bat','卸载.bat','设置.bat','使用说明.txt','更新日志.txt','图标.ico','TigerClaw.Sentence.Native.dll')) {
    $all = (Check ($arc.Entries.FullName -contains $n) "包内 $n") -and $all
}
$all = (Check ($null -ne ($arc.Entries | Where-Object { $_.FullName -match '数据[/\\]config\.json$' })) '包内 数据\config.json') -and $all
# 【2026-09-07 大统一】全局资源：拼音反查 3 表（小鹤蓝本过滤版）+ 拆分 + 注释 2 表
$all = (Check (@($arc.Entries | Where-Object { $_.FullName -match '拼音反查[/\\](小鹤双拼|全拼|自然码)\.txt$' }).Count -eq 3) '拼音反查 3 表（小鹤/全拼/自然码，微软已删）') -and $all
$all = (Check (@($arc.Entries | Where-Object { $_.FullName -match '拼音反查[/\\]微软双拼\.txt$' }).Count -eq 0) '微软双拼已删') -and $all
$all = (Check (@($arc.Entries | Where-Object { $_.FullName -match '码表[/\\](虎字-繁体优先|B定制-常用)' }).Count -eq 0) '两码表方案已删') -and $all
$all = (Check ($null -ne ($arc.Entries | Where-Object { $_.FullName -match '拆分[/\\]虎码\.拆分$' })) '拆分表 虎码.拆分') -and $all
$all = (Check (@($arc.Entries | Where-Object { $_.FullName -match '注释[/\\](拼音|unicode)\.注释$' }).Count -eq 2) '注释 2 表（拼音+unicode）') -and $all
$skins = @($arc.Entries | Where-Object { $_.FullName -match '皮肤[/\\]hufu-.*\.json$' })
$srcSkins = @(Get-ChildItem "$SRC\数据\皮肤" -Filter 'hufu-*.json' -EA SilentlyContinue).Count
$all = (Check ($skins.Count -eq $srcSkins -and $skins.Count -ge 9) "皮肤 $($skins.Count) 款（=打包源 $srcSkins 款，下限 9）") -and $all
if ($NoModel) {
    $all = (Check (@($arc.Entries | Where-Object { $_.FullName -match '模型[/\\]' }).Count -eq 0) '模型 0 个（无模型正式版）') -and $all
} else {
    $all = (Check (@($arc.Entries | Where-Object { $_.FullName -match '模型[/\\]' }).Count -eq 2) '模型 2 个') -and $all
}
$all = (Check (@($arc.Entries | Where-Object { $_.FullName -match '\.old' }).Count -eq 0) '零 .old 垃圾') -and $all
$all = (Check (@($arc.Entries | Where-Object { $_.Name -eq 'install.log' }).Count -eq 0) '零 install.log') -and $all
# ── 4) 核验 B：语义特征 ──
$e = $arc.Entries | Where-Object { $_.Name -eq 'install.ps1' }
$st = $e.Open(); $b = [byte[]]::new(3); [void]$st.Read($b, 0, 3); $st.Close()
$all = (Check ($b[0] -eq 0xEF) 'install.ps1 UTF8 BOM（PS5.1 中文必需）') -and $all
$sr = New-Object System.IO.StreamReader($e.Open()); $zi = $sr.ReadToEnd(); $sr.Close()
$all = (Check ($zi -match 'PhaseElevated') 'zip 内 install.ps1 双阶段特征') -and $all
$all = (Check ($zi -match 'SysWOW64') 'zip 内 install.ps1 32 位段') -and $all
$all = (Check ($zi -match '回读') 'zip 内 install.ps1 防吃防线（回读校验）') -and $all
$cfgE = $arc.Entries | Where-Object { $_.Name -eq 'config.json' }
$sr2 = New-Object System.IO.StreamReader($cfgE.Open()); $j = $sr2.ReadToEnd() | ConvertFrom-Json; $sr2.Close()
$all = (Check ($j.appearance.skin -eq 'hufu-chenwu') "默认皮肤=$($j.appearance.skin)（晨雾，2026-09-19 用户定稿）") -and $all
# 【2026-09-06】/ 直出档默认关（命名空间档——/jc /jq /rq 等 / 前缀功能开箱即用）
$all = (Check ($j.input.slash_dunhao -eq $false) 'config slash_dunhao=false（/ 命名空间档）') -and $all
$arc.Dispose()
if (-not $all) { Remove-Item $tmpZip -Force; $report | Out-File "$LOGD\pack-$stamp-FAIL.txt" -Encoding UTF8; throw '核验 A/B 未全绿，产物已销毁，报告在 日志\' }

# ── 5) 核验 C：与基线 diff（无模型版基线=最近的无模型 zip）──
$baseZip = Get-ChildItem $BASE -Filter $(if ($NoModel) { '*无模型.zip' } else { '*.zip' }) -EA SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 1
if ($baseZip) {
    $ba = [System.IO.Compression.ZipFile]::OpenRead($baseZip.FullName)
    $newNames = (Get-ChildItem $tmpZip | Out-Null) # noop
    $na = [System.IO.Compression.ZipFile]::OpenRead($tmpZip)
    $newSet = @($na.Entries.FullName | Sort-Object)
    $oldSet = @($ba.Entries.FullName | Sort-Object)
    $added = Compare-Object $oldSet $newSet | Where-Object SideIndicator -eq '=>' | ForEach-Object InputObject
    $removed = Compare-Object $oldSet $newSet | Where-Object SideIndicator -eq '<=' | ForEach-Object InputObject
    Check $true "基线对比（基线=$($baseZip.Name)）：新增 $($added.Count) 项 / 移除 $($removed.Count) 项"
    if ($added) { $report += "  新增: $($added -join ', ')" }
    if ($removed) { $report += "  移除: $($removed -join ', ')" }
    # 关键脚本逐字节对比（install/uninstall 与基线一致或差异有解释）
    foreach ($n in @('install.ps1','uninstall.ps1')) {
        $nE = $na.Entries | Where-Object { $_.Name -eq $n }; $bE = $ba.Entries | Where-Object { $_.Name -eq $n }
        $r1 = New-Object System.IO.StreamReader($nE.Open()); $t1 = $r1.ReadToEnd(); $r1.Close()
        $r2 = New-Object System.IO.StreamReader($bE.Open()); $t2 = $r2.ReadToEnd(); $r2.Close()
        if ($t1 -ceq $t2) { Check $true "$n 与基线逐字节一致" }
        else {
            $same32 = ($t1 -match 'SysWOW64') -and ($t2 -match 'SysWOW64')
            Check $same32 "$n 与基线有差异（双方均含 32 位段——变更需在报告中说明原因）"
            $report += "  [$n 变更说明]（打包时人工补注）"
        }
    }
    $na.Dispose(); $ba.Dispose()
}

# ── 6) 落盘正式 zip + 报告 ──
Move-Item $tmpZip $zip -Force
$z = Get-Item $zip
$report += "产物: $zipName $([math]::Round($z.Length/1MB,2))MB @ $($z.LastWriteTime.ToString('HH:mm:ss'))"

# ── 6b) 【无模型正式版专属】config 默认方案切「个人自用」──
# 与旧第 7 段小包同语义：无模型依赖、装完即用；recent_pair 保
# 虎整句（Ctrl+M 可切 / 拖入模型后即整句）。在 zip 内原位改写。
if ($NoModel) {
    $na2 = [System.IO.Compression.ZipFile]::Open($zip, 'Update')
    # 【2026-10-05 分隔符归一】条目名已改 '/'——兼容两种分隔符匹配。
    $cfgE2 = $na2.Entries | Where-Object { $_.FullName -eq '数据/config.json' -or $_.FullName -eq '数据\config.json' } | Select-Object -First 1
    if ($cfgE2) {
        $cr2 = New-Object System.IO.StreamReader($cfgE2.Open()); $ct2 = $cr2.ReadToEnd(); $cr2.Close()
        $cj2 = $ct2 | ConvertFrom-Json
        $cj2.schema.current = '个人自用'
        $nc2 = $cj2 | ConvertTo-Json -Depth 32
        $cfgE2.Delete()
        $ne2 = $na2.CreateEntry('数据/config.json', [System.IO.Compression.CompressionLevel]::Optimal)
        $nw2 = $ne2.Open(); $nb2 = [System.Text.Encoding]::UTF8.GetBytes($nc2); $nw2.Write($nb2, 0, $nb2.Length); $nw2.Close()
    }
    $na2.Dispose()
    # 换配核验
    $sv2 = [System.IO.Compression.ZipFile]::OpenRead($zip)
    $cfgV2 = $sv2.Entries | Where-Object { $_.FullName -eq '数据/config.json' -or $_.FullName -eq '数据\config.json' } | Select-Object -First 1
    if ($cfgV2) {
        $vr2 = New-Object System.IO.StreamReader($cfgV2.Open()); $vt2 = $vr2.ReadToEnd(); $vr2.Close()
        $vj2 = $vt2 | ConvertFrom-Json
        [void](Check ($vj2.schema.current -eq '个人自用') '无模型正式版 默认方案=个人自用（零模型即用）')
        [void](Check ($vj2.schema.recent_pair -contains '虎整句') '无模型正式版 recent_pair 含虎整句（Ctrl+M 可切）')
    }
    $sv2.Dispose()
}

# ── 7) 无模型小包（旧双包路径，仅全量模式执行；-NoModel 时正式包
#    即无模型版，本段跳过——曾经在此发生 $szip==$zip 自删事故）──
if (-not $NoModel) {
$szipName = "HuFu虎符输入法-v$Version-无模型.zip"
$szip = Join-Path $PSScriptRoot $szipName
$sall = $true
if (Test-Path $szip) { Remove-Item $szip -Force }
Copy-Item $zip $szip -Force
$sa = [System.IO.Compression.ZipFile]::Open($szip, 'Update')
# 剔除模型条目
$sm = @($sa.Entries | Where-Object { $_.FullName -match '模型' })
foreach ($m in $sm) { $m.Delete() }
# 换 config：默认方案=个人自用（recent_pair 保留 [个人自用,虎整句]——
# Ctrl+M 一键切整句）
$cfgE = $sa.Entries | Where-Object { $_.FullName -eq '数据/config.json' -or $_.FullName -eq '数据\config.json' } | Select-Object -First 1
$cfgText = ''
if ($cfgE) {
    $cr = New-Object System.IO.StreamReader($cfgE.Open()); $cfgText = $cr.ReadToEnd(); $cr.Close()
    $cfgJson = $cfgText | ConvertFrom-Json
    $cfgJson.schema.current = '个人自用'
    $newCfg = $cfgJson | ConvertTo-Json -Depth 32
    $cfgE.Delete()
    $ne = $sa.CreateEntry('数据/config.json', [System.IO.Compression.CompressionLevel]::Optimal)
    $nw = $ne.Open(); $nb = [System.Text.Encoding]::UTF8.GetBytes($newCfg); $nw.Write($nb, 0, $nb.Length); $nw.Close()
}
$sa.Dispose()
# 小包核验
$sv = [System.IO.Compression.ZipFile]::OpenRead($szip)
$svAll = $all
$left = @($sv.Entries | Where-Object { $_.FullName -match '模型' })
$svAll = (Check ($left.Count -eq 0) "小包 模型残留 0（剔除 $($sm.Count) 条）") -and $svAll
$cfgV = $sv.Entries | Where-Object { $_.FullName -eq '数据/config.json' -or $_.FullName -eq '数据\config.json' } | Select-Object -First 1
if ($cfgV) {
    $vr = New-Object System.IO.StreamReader($cfgV.Open()); $vt = $vr.ReadToEnd(); $vr.Close()
    $vj = $vt | ConvertFrom-Json
    $svAll = (Check ($vj.schema.current -eq '个人自用') '小包 默认方案=个人自用') -and $svAll
    $svAll = (Check ($vj.schema.recent_pair -contains '虎整句') '小包 recent_pair 含虎整句（Ctrl+M 可切）') -and $svAll
}
foreach ($n in @('hufu_tsf.dll','hufu-server.exe')) {
    $e1 = $sv.Entries | Where-Object { $_.Name -eq $n } | Select-Object -First 1
    if ($e1) { $svAll = (Check ($e1.Length -gt 500000) "小包 $n 在位") -and $svAll }
}
$sv.Dispose()
$sz = Get-Item $szip
$svAll = (Check ($sz.Length -lt 60MB) "小包体积 $([math]::Round($sz.Length/1MB,2))MB < 60MB") -and $svAll
if (-not $svAll) {
    Write-Host "✗ 无模型小包核验未全绿（产物保留供人工检查）: $szip" -ForegroundColor Yellow
} else {
    Write-Host "★ 无模型小包: $szip（$([math]::Round($sz.Length/1MB,2))MB）" -ForegroundColor Cyan
}
} # end if (-not $NoModel)

$report | Out-File "$LOGD\pack-$stamp-OK.txt" -Encoding UTF8

# 【2026-10-06 打包后复活 live server】无 UAC 模式的前提就是调用方把
# server 杀净了（上面 382 行门禁），而 DLL 侧守护并不可靠（实测
# pipe=fail perr=2 只报错不拉活——本机用户「打不了字」事故：打包杀
# server 后无人复活，打字全断直到人工拉起）。打包链杀了谁就该负责
# 拉回谁：收尾时从 live 安装重启 server。管理员模式自带 ctfmon/守护
# 复活链，不经此路径。
if ($skipTeng) {
    $liveSrv = Get-ChildItem 'D:\HUFU' -Directory -Filter 'HuFu虎符输入法-v*' -ErrorAction SilentlyContinue |
        Sort-Object Name -Descending | Select-Object -First 1
    if ($liveSrv -and (Test-Path (Join-Path $liveSrv.FullName 'hufu-server.exe'))) {
        try {
            Start-Process -FilePath (Join-Path $liveSrv.FullName 'hufu-server.exe') -WindowStyle Hidden
            Start-Sleep -Seconds 2
            if (Get-Process hufu-server -ErrorAction SilentlyContinue) {
                Write-Host '★ 已重启 live hufu-server（停服腾位的善后）' -ForegroundColor Cyan
            } else {
                Write-Host '⚠ live server 重启失败——请手动启动或重新登录' -ForegroundColor Yellow
            }
        } catch { Write-Host "⚠ 重启 live server 异常: $_" -ForegroundColor Yellow }
    }
}

Write-Host ''
Write-Host "★ 打包完成（核验全绿）: $zip" -ForegroundColor Cyan
Write-Host '  报告见 日志\；发布后（用户验证过）把 zip 复制进 基线包\ 作为下一版基线'