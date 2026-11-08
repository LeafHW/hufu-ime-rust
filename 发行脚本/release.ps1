# release.ps1 — 虎符唯一发行入口（2026-10-18 立此存照）
# ============================================================================
# 用法：powershell -ExecutionPolicy Bypass -File release.ps1 -Version x.y.z
# 可选：-VerifyOnly（只校验该版双产物，不重新打包）——校验内容不只看「在不在」：
#       版本号（zip 内 version.txt = exe 版本资源 = 本版号）、关键指纹
#       （zip 内 install.ps1 与仓内单一事实源 SHA256 一致）、体积、SHA256。
# 可选：-SkipChangelogCheck（应急：允许更新日志顶部不是本版本；默认=失败）
#
# 【为什么存在】v1.8.7 事故：打包只跑 pack.ps1 出了 zip、漏了 exe 发行
# 线。「完整发行=zip+exe 缺一不可」从此不再依赖执行者记忆——本脚本一次
# 跑全两条线，任何一步失败=整体失败，绝不出现「只出一半」。
# 铁律全文：E:\DSH-KF\hufu\发行脚本\发行铁律.md
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [switch]$VerifyOnly,
    # 发完包后自动同步到 Gitee（附件 + 更新清单 + 站点）；失败只告警不阻断发行
    [switch]$NoGitee,
    # 【P2-27】应急开关：日志顶部不含本版本时默认直接失败（护栏，不是提示）
    [switch]$SkipChangelogCheck,
    [string]$ReleaseRoot = 'E:\DSH-KF\hufu-发行'
)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$ScriptDir = if ($PSScriptRoot) { $PSScriptRoot } elseif ($PSCommandPath) { Split-Path -Parent $PSCommandPath } else { throw '无法确定脚本路径，请使用 -File 运行当前仓库的 release.ps1' }
$RepoRoot = [IO.Path]::GetFullPath((Join-Path $ScriptDir '..'))
$rel  = [IO.Path]::GetFullPath($ReleaseRoot)
$zip  = Join-Path $rel "HuFu虎符输入法-v$Version-无模型.zip"
$exe  = Join-Path $rel "HuFuSetup-$Version.exe"
$pack = Join-Path $ScriptDir 'pack.ps1'
$setup= Join-Path $RepoRoot 'platform\windows\setup\build-setup.ps1'

# 【P2-27 2026-11-04】核验升级：原来 -VerifyOnly 只查「文件在不在 + 哈希」，
# 版本号写错、zip 里装的是上一版、装了个空壳 exe 都验不出来（护栏只是提示）。
# 现在逐项判死：任何一项不过 → exit 1（非零退出，CI/人工都挡得住）。
Add-Type -AssemblyName System.IO.Compression.FileSystem
$script:verifyFail = 0
function VCheck($ok, [string]$what) {
    if ($ok) { Write-Host ("  [PASS] " + $what) -ForegroundColor Green }
    else { $script:verifyFail++; Write-Host ("  [FAIL] " + $what) -ForegroundColor Red }
    $ok
}

function Verify([string]$tag) {
    Write-Host "`n—— 双产物核验（$tag）——" -ForegroundColor Cyan
    $script:verifyFail = 0
    $zipVer = ''
    # ── 1) zip 线 ──
    if (-not (Test-Path $zip)) {
        [void](VCheck $false "缺产物：$zip")
    } else {
        $zi = Get-Item $zip
        $zh = (Get-FileHash $zip -Algorithm SHA256).Hash
        [void](VCheck ($zi.Length -gt 10MB) ("{0}  {1:N1} MB  SHA256 {2}" -f $zi.Name, ($zi.Length/1MB), $zh))
        try {
            $za = [System.IO.Compression.ZipFile]::OpenRead($zip)
            $names = @($za.Entries | ForEach-Object { $_.FullName })
            # 【发行链补口·深挖 P2 2026-11-08】licenses/trainer 核 zhouzhou 两件
            # （LXGW 字体两件随字体策略移除后仅剩此二 + drills 的 NOTICE.md）；
            # 语料 5 件是 gale-core 编译期 include_str! 源（不随包发行）——产本
            # 包的仓库必须五件齐全，防"语料被裁的仓库照发版"。
            foreach ($need in @('hufu-server.exe','hufu-trainer.exe','hufu_tsf.dll','install.ps1','hufu-render-helper.exe',
                    'licenses/trainer/zhouzhou-LICENSE.txt','licenses/trainer/zhouzhou-NOTICE.txt')) {
                [void](VCheck ($names -contains $need) "zip 内 $need")
            }
            foreach ($dz in @('前1500单字.txt','常用单字前五百.txt','常用单字中五百.txt','常用单字后五百.txt','常用词组前三百.txt')) {
                [void](VCheck (Test-Path (Join-Path $RepoRoot ("trainer\drills\" + $dz))) "语料源 trainer\drills\$dz")
            }
            $ve = $za.Entries | Where-Object { $_.FullName -eq 'version.txt' } | Select-Object -First 1
            if ($ve) {
                $sr = New-Object System.IO.StreamReader($ve.Open()); $zipVer = $sr.ReadToEnd().Trim(); $sr.Close()
            }
            [void](VCheck ($zipVer -eq $Version) "zip 内 version.txt = '$zipVer'（应为 $Version）")
            # 关键指纹：zip 内 install.ps1 必须与仓内单一事实源逐字节一致（漂移检出）
            $repoInst = Join-Path $RepoRoot 'platform\windows\install\install.ps1'
            $ie = $za.Entries | Where-Object { $_.FullName -eq 'install.ps1' } | Select-Object -First 1
            if ((Test-Path $repoInst) -and $ie) {
                $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("hufu-verify-" + [guid]::NewGuid().ToString('N') + ".ps1")
                [System.IO.Compression.ZipFileExtensions]::ExtractToFile($ie, $tmp, $true)
                $h1 = (Get-FileHash $tmp -Algorithm SHA256).Hash
                $h2 = (Get-FileHash $repoInst -Algorithm SHA256).Hash
                Remove-Item $tmp -Force -EA SilentlyContinue
                [void](VCheck ($h1 -eq $h2) "zip 内 install.ps1 = 仓内单一事实源（SHA256 $($h1.Substring(0,12))…）")
            } else {
                [void](VCheck $false '当前仓库或zip缺少 install.ps1，无法核实同源脚本指纹')
            }
            $za.Dispose()
        } catch {
            [void](VCheck $false "zip 打不开/条目异常：$($_.Exception.Message)")
        }
    }
    # ── 2) exe 线 ──
    if (-not (Test-Path $exe)) {
        [void](VCheck $false "缺产物：$exe")
    } else {
        $ei = Get-Item $exe
        $eh = (Get-FileHash $exe -Algorithm SHA256).Hash
        [void](VCheck ($ei.Length -gt 10MB) ("{0}  {1:N1} MB  SHA256 {2}" -f $ei.Name, ($ei.Length/1MB), $eh))
        $pv = ([string]$ei.VersionInfo.ProductVersion).Trim()
        [void](VCheck ($pv -eq $Version -or $pv -eq "$Version.0") "安装包版本资源 = '$pv'（应为 $Version）")
        if ($zipVer) { [void](VCheck ($pv -eq $zipVer -or $pv -eq "$zipVer.0") "双产物同版：zip version.txt='$zipVer' / exe='$pv'") }
        # 侧车哈希（打包机若写了 *.sha256，必须与实测一致）
        $sc = "$exe.sha256"
        if (Test-Path $sc) {
            $want = (((Get-Content $sc -TotalCount 1) -split '\s+')[0]).Trim().ToUpper()
            [void](VCheck ($want -eq $eh) "SHA256 侧车一致（$(Split-Path $sc -Leaf)）")
        }
    }
    if ($script:verifyFail -gt 0) {
        Write-Host ("════ 发行未完成：$($script:verifyFail) 项核验未过（zip + exe 缺一不可）════") -ForegroundColor Red
        exit 1
    }
    Write-Host '════ 核验全绿：双产物齐 + 版本号/指纹一致 ════' -ForegroundColor Green
}

if ($Version -notmatch '^\d+\.\d+\.\d+(\.\d+)?$') {
    Write-Host "✗ 版本号格式应为 x.y.z 或 x.y.z.w，收到：$Version" -ForegroundColor Red
    exit 1
}
if ($VerifyOnly) { Verify '仅校验'; exit 0 }

# ── 1) zip 线（pack.ps1 红线不改，原样调用） ──
Write-Host '—— [0/2] 同步更新日志（仓内主副本 → 打包源）——' -ForegroundColor Cyan
# 【防漂移 2026-10-18】更新日志唯一事实源在仓内（发行脚本\更新日志.txt，
# git 跟踪）；打包前自动覆盖打包源副本——此前日志只活在打包源、无 git
# 记账，v1.8.7 曾整版漏写随包发行。
$changelog = Join-Path $ScriptDir '更新日志.txt'
if (Test-Path $changelog) {
    Copy-Item $changelog (Join-Path $rel '打包源\更新日志.txt') -Force
    $first = (Get-Content $changelog -TotalCount 1 -Encoding UTF8)
    Write-Host "  已同步，当前最新条目：$first"
    if ($first -notmatch [regex]::Escape($Version)) {
        # 【P2-27 2026-11-04】护栏从「提示」改成「失败」：v1.8.7 曾整版漏写随包
        # 更新日志，黄字警告没人看见就发出去了。默认直接非零退出。
        if (-not $SkipChangelogCheck) {
            Write-Host "  [FAIL] 日志顶部条目不含本版本 $Version——先写更新日志再发行（v1.8.7 事故防线）" -ForegroundColor Red
            Write-Host "         修法：在 $changelog 顶部加一行「$(Get-Date -Format 'yyyy-MM-dd')  v$Version …」，或应急加 -SkipChangelogCheck" -ForegroundColor Yellow
            exit 1
        }
        Write-Host "  [警告] 日志顶部条目不含本版本 $Version——已按 -SkipChangelogCheck 放行（应急，发行报告需说明）" -ForegroundColor Yellow
    }
} else {
    throw "当前仓库缺少更新日志主副本，禁止沿用旧打包源日志：$changelog"
}

Write-Host '—— [1/2] zip 发行线（pack.ps1）——' -ForegroundColor Cyan
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File $pack -Version $Version -NoModel -ReleaseRoot $rel
if ($LASTEXITCODE -ne 0) { Write-Host '✗ zip 线失败，发行中止' -ForegroundColor Red; exit 1 }

# ── 2) exe 线（必须 rust 轨——ps1 轨是回滚轨，装时弹蓝窗） ──
Write-Host '—— [2/2] exe 发行线（build-setup.ps1 -Engine rust）——' -ForegroundColor Cyan
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File $setup -Version $Version -Engine rust -ReleaseRoot $rel
if ($LASTEXITCODE -ne 0) { Write-Host '✗ exe 线失败，发行中止' -ForegroundColor Red; exit 1 }

Verify '打包后'

# ── 3) 附加：同步到 Gitee（附件 + 更新清单 + 站点）──────────────────────────
# 发行结果已经由 Verify 决定；这一段失败只告警，绝不影响「发行是否成功」。
# 2026-11-03：国内直连 GitHub 只有几十 KB/s，Gitee 实测 2 MB/s。
if (-not $NoGitee) {
    Write-Host '—— [附加] 同步到 Gitee：Release 附件 + 更新清单 + 站点 ——' -ForegroundColor Cyan
    $pub = Join-Path $ScriptDir 'publish-gitee.ps1'
    if (Test-Path $pub) {
        try {
            & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $pub -Version $Version -ExePath $exe -PublishSite
            if ($LASTEXITCODE -ne 0) {
                Write-Host '  [警告] Gitee 同步非零退出；发行本身已完成，可稍后单独重跑 publish-gitee.ps1' -ForegroundColor Yellow
            }
        } catch {
            Write-Host "  [警告] Gitee 同步异常（不影响发行）：$($_.Exception.Message)" -ForegroundColor Yellow
        }
    } else {
        Write-Host '  [警告] 找不到 publish-gitee.ps1，跳过 Gitee 同步' -ForegroundColor Yellow
    }
}
