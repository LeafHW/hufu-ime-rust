# ============================================================================
# build-setup.ps1 — HuFu 虎符输入法 单 exe 安装包构建脚本
# ----------------------------------------------------------------------------
# 用法：
#   .\build-setup.ps1 -Version 1.8.2
# 可选：-PkgSource <打包源>（默认 E:\DSH-KF\hufu-发行\打包源）
#        -DistDir   <输出目录>（默认 E:\DSH-KF\hufu-发行）
#        -SkipCompile（只做 staging 与清单生成，供检查）
# 产物：DistDir\HuFuSetup-<版本>.exe（+ SHA256）
#
# 职责：镜像打包源 → 注入安装器支撑件（helper/stub/说明/version.txt）→
#       生成数据文件清单 hufu-files.lst 与 staging-files.inc → iscc 编译。
# 红线：不改 pack.ps1（zip 发行线原样保留，本脚本是并行的 exe 发行线）。
# ============================================================================
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [string]$PkgSource = 'E:\DSH-KF\hufu-发行\打包源',
    [string]$DistDir = 'E:\DSH-KF\hufu-发行',
    [switch]$SkipCompile
)

$ErrorActionPreference = 'Stop'
$setupRoot = $PSScriptRoot
$staging   = Join-Path $setupRoot 'staging'
$helper    = Join-Path $setupRoot 'helper'

function Fail([string]$msg) { Write-Host "✗ $msg" -ForegroundColor Red; exit 1 }
function Ok([string]$msg)   { Write-Host "OK $msg" -ForegroundColor Green }

# ── 1) 输入校验 ────────────────────────────────────────────────────────────
if ($Version -notmatch '^\d+\.\d+\.\d+$') { Fail("版本号格式应为 x.y.z，收到：$Version") }

$needFromPkg = @(
    'hufu-server.exe','hufu_tsf.dll','hufu_tsf32.dll','hufu-tsf-smoke.exe',
    'TigerClaw.Sentence.Native.dll','install.ps1','uninstall.ps1','设置.bat',
    '更新日志.txt','图标.ico'
)
$missing = @($needFromPkg | Where-Object { -not (Test-Path (Join-Path $PkgSource $_)) })
if ($missing.Count -gt 0) { Fail("打包源缺少：$($missing -join '、')") }
if (-not (Test-Path "$PkgSource\码表")) { Fail('打包源缺少 码表\ 目录') }
if (-not (Test-Path "$PkgSource\数据")) { Fail('打包源缺少 数据\ 目录') }

$needFromHelper = @('hufu-setup-tools.ps1','hufu-postboot.ps1','卸载-stub.bat')
$missingH = @($needFromHelper | Where-Object { -not (Test-Path (Join-Path $helper $_)) })
if ($missingH.Count -gt 0) { Fail("helper 目录缺少（等 task-1 交付）：$($missingH -join '、')") }
$readmeSetup = Get-ChildItem $helper -Filter '使用说明*' -File -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $readmeSetup) { Fail('helper 目录缺少 使用说明-setup版.txt') }

$iscc = "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
if (-not (Test-Path $iscc)) { $iscc = 'C:\Program Files (x86)\Inno Setup 6\ISCC.exe' }
if (-not (Test-Path $iscc)) { Fail('未找到 ISCC.exe（Inno Setup 6）——先安装 Inno Setup 6') }
if (-not (Test-Path (Join-Path $setupRoot 'lang\ChineseSimplified.isl'))) { Fail('缺少 lang\ChineseSimplified.isl') }

# ── 2) staging 重建 ───────────────────────────────────────────────────────
if (Test-Path $staging) { Remove-Item $staging -Recurse -Force }
New-Item -ItemType Directory -Path $staging | Out-Null

# 程序文件（进卸载日志）
foreach ($f in $needFromPkg) { Copy-Item (Join-Path $PkgSource $f) $staging -Force }
if (Test-Path "$PkgSource\licenses") { Copy-Item "$PkgSource\licenses" "$staging\licenses" -Recurse -Force }
if (Test-Path "$PkgSource\使用说明.txt") { Remove-Item "$staging\使用说明.txt" -Force -ErrorAction SilentlyContinue }
Copy-Item $readmeSetup.FullName "$staging\使用说明.txt" -Force

# 安装器支撑件（卸载 stub 随包；exe 体系不再安装 安装.bat——已装目录里
# 的遗留由 iss PrepareToInstall 清除，防 zip 时代双体系误用）
Copy-Item (Join-Path $helper '卸载-stub.bat') "$staging\卸载.bat" -Force
Copy-Item (Join-Path $helper 'hufu-setup-tools.ps1') $staging -Force
Copy-Item (Join-Path $helper 'hufu-postboot.ps1') $staging -Force

# ── 图标体系（2026-10-13 用户定稿：全套换虎符头图标，白底已抠透明）──
# 1) 安装/卸载器 + {app}\图标.ico ← icons\虎符.ico（多尺寸 256/64/48/32/16）
# 2) 程序 PE 内嵌图标 ← rcedit 打入（server/tsf 双架构/smoke；TigerClaw 纯计算库无图标面不碰）
$brandIco = Join-Path $setupRoot 'icons\虎符.ico'
if (-not (Test-Path $brandIco)) { Fail('缺少 icons\虎符.ico（先跑 icons\make-ico.ps1 生成）') }
Copy-Item $brandIco "$staging\图标.ico" -Force
$rcedit = Join-Path $setupRoot 'tools\rcedit-x64.exe'
if (-not (Test-Path $rcedit)) { Fail('缺少 tools\rcedit-x64.exe（github electron/rcedit releases）') }
foreach ($pe in @('hufu-server.exe','hufu_tsf.dll','hufu_tsf32.dll','hufu-tsf-smoke.exe')) {
    & $rcedit (Join-Path $staging $pe) --set-icon $brandIco 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) { Fail("rcedit 打图标失败：$pe（退出码 $LASTEXITCODE）") }
}
Ok('图标已应用：安装器/卸载器 + 图标.ico + 4 个 PE')

# stub bat 字节卫生：不得 UTF-8 BOM、必须 CRLF（血律：cmd 中文安全）
foreach ($bat in @("$staging\卸载.bat", "$staging\设置.bat")) {
    if (Test-Path $bat) {
        $bytes = [IO.File]::ReadAllBytes($bat)
        if ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF) {
            Fail("$bat 带 UTF-8 BOM（bat 必须 GBK/ANSI 无 BOM）")
        }
        $text = [Text.Encoding]::GetEncoding(936).GetString($bytes)
        if ($text -notmatch "`r`n") { Fail("$bat 缺少 CRLF 换行") }
    }
}

# ── 3) 数据文件（码表\ + 数据\ → staging\data\，清单 + inc） ──────────────
$dataRoot = Join-Path $staging 'data'
New-Item -ItemType Directory -Path $dataRoot | Out-Null
Copy-Item "$PkgSource\码表" "$dataRoot\码表" -Recurse -Force
Copy-Item "$PkgSource\数据" "$dataRoot\数据" -Recurse -Force

# 用户数据绝不入包（双保险：即便打包源被污染也剔掉）
$forbidden = @('用户调整.txt','adj-audit.log','user-adjust.log','导出 -*.txt')
foreach ($pat in $forbidden) {
    Get-ChildItem $dataRoot -Recurse -Filter $pat -File -ErrorAction SilentlyContinue | Remove-Item -Force
}

$dataFiles = Get-ChildItem $dataRoot -Recurse -File | Sort-Object FullName
if ($dataFiles.Count -lt 40) { Fail("数据文件仅 $($dataFiles.Count) 个（正常应 60+）——打包源镜像疑似异常") }

# 清单：相对 data 根的路径；'!' 前缀 = 目标缺失才拷（只补缺，绝不覆盖）
# 【铁律 2026-10-13 用户定稿】覆盖安装时用户财产零改动，仅补缺失文件：
#   · 码表\ 整目录（码表是用户自己的：个人自用/自定义方案/手工增改全在内）
#   · 数据\皮肤\（设置页可调参，导出含"调过参的皮肤"=用户可变文件）
#   · 数据\config.json（个人设置）
# 纯出厂词典（转换词典/注释/拼音反查/音效）保持"差异才更新+自动备份"。
$frozenPrefixes = @('码表\', '数据\皮肤\', '数据\config.json')
$lstLines = New-Object System.Collections.Generic.List[string]
$incLines = New-Object System.Collections.Generic.List[string]
$incLines.Add('; 由 build-setup.ps1 自动生成，勿手改')
foreach ($f in $dataFiles) {
    $rel = $f.FullName.Substring($dataRoot.Length + 1)
    $dir = Split-Path $rel -Parent
    $frozen = $false
    foreach ($pfx in $frozenPrefixes) { if ($rel -like ($pfx + '*')) { $frozen = $true; break } }
    if ($frozen) { $lstLines.Add('!' + $rel) }
    else { $lstLines.Add($rel) }
    $dest = if ($dir) { '{tmp}\hufu-pkg\' + $dir } else { '{tmp}\hufu-pkg' }
    $incLines.Add(('Source: "staging\data\{0}"; DestDir: "{1}"; Flags: ignoreversion deleteafterinstall' -f $rel, $dest))
}
[IO.File]::WriteAllText("$staging\hufu-files.lst", ($lstLines -join "`r`n") + "`r`n", (New-Object Text.UTF8Encoding($true)))
# 【升级清单】铁律定稿（2026-10-13 用户两轮收紧）：
#   · 码表\ 整目录零写入（不覆盖、不补缺、不复活已删）→ 升级清单直接剔除
#   · 数据\ 全部条目只补缺（'!'）：缺失补上，已存在一律不覆盖（含词典/皮肤/
#     音效/config——这些也是用户可自行维护的"表"，备份后覆盖也不做）
# 全新安装仍用全量清单（空目录，语义等价全量落盘）。
$upgLines = New-Object System.Collections.Generic.List[string]
foreach ($l in $lstLines) {
    if ($l.StartsWith('!码表\')) { continue }
    if ($l.StartsWith('!')) { $upgLines.Add($l) }
    else { $upgLines.Add('!' + $l) }
}
[IO.File]::WriteAllText("$staging\hufu-files-upgrade.lst", ($upgLines -join "`r`n") + "`r`n", (New-Object Text.UTF8Encoding($true)))
[IO.File]::WriteAllText((Join-Path $setupRoot 'staging-files.inc'), ($incLines -join "`r`n") + "`r`n", (New-Object Text.UTF8Encoding($true)))

# version.txt（安装器体系的版本信号）
[IO.File]::WriteAllText("$staging\version.txt", $Version + "`r`n", [Text.Encoding]::ASCII)

$stagedMB = [math]::Round(((Get-ChildItem $staging -Recurse -File | Measure-Object Length -Sum).Sum)/1MB,1)
Ok("staging 完成：$($dataFiles.Count) 个数据文件，共 ${stagedMB}MB（无模型档）")

# ── 4) 编译 ───────────────────────────────────────────────────────────────
if ($SkipCompile) { Ok('SkipCompile：staging 与 inc 已生成，未编译'); exit 0 }

& $iscc "/DSetupVersion=$Version" "/DDistDir=$DistDir" (Join-Path $setupRoot 'hufu-setup.iss') | Tee-Object -Variable isccOut
if ($LASTEXITCODE -ne 0) { Fail("iscc 编译失败（退出码 $LASTEXITCODE）") }

$outExe = Join-Path $DistDir ("HuFuSetup-{0}.exe" -f $Version)
if (-not (Test-Path $outExe)) { Fail("编译器报告成功但未找到产物：$outExe") }
$hash = (Get-FileHash $outExe -Algorithm SHA256).Hash
$sizeMB = [math]::Round((Get-Item $outExe).Length/1MB,2)
Ok("产物：$outExe（${sizeMB}MB）")
Ok("SHA256：$hash")
exit 0
