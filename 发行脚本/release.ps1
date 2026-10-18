# release.ps1 — 虎符唯一发行入口（2026-10-18 立此存照）
# ============================================================================
# 用法：powershell -ExecutionPolicy Bypass -File release.ps1 -Version x.y.z
# 可选：-VerifyOnly x.y.z（只校验该版双产物+哈希，不重新打包）
#
# 【为什么存在】v1.8.7 事故：打包只跑 pack.ps1 出了 zip、漏了 exe 发行
# 线。「完整发行=zip+exe 缺一不可」从此不再依赖执行者记忆——本脚本一次
# 跑全两条线，任何一步失败=整体失败，绝不出现「只出一半」。
# 铁律全文：E:\DSH-KF\hufu\发行脚本\发行铁律.md
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [switch]$VerifyOnly
)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$rel  = 'E:\DSH-KF\hufu-发行'
$zip  = Join-Path $rel "HuFu虎符输入法-v$Version-无模型.zip"
$exe  = Join-Path $rel "HuFuSetup-$Version.exe"
$pack = 'E:\DSH-KF\hufu\发行脚本\pack.ps1'
$setup= 'E:\DSH-KF\hufu\platform\windows\setup\build-setup.ps1'

function Verify([string]$tag) {
    Write-Host "`n—— 双产物核验（$tag）——" -ForegroundColor Cyan
    $ok = $true
    foreach ($f in @($zip, $exe)) {
        if (Test-Path $f) {
            $h = (Get-FileHash $f -Algorithm SHA256).Hash
            Write-Host ("  [PASS] {0}  {1:N1} MB  SHA256 {2}" -f (Split-Path $f -Leaf), ((Get-Item $f).Length/1MB), $h)
        } else {
            Write-Host ("  [FAIL] 缺产物：{0}" -f $f) -ForegroundColor Red
            $ok = $false
        }
    }
    if (-not $ok) {
        Write-Host '════ 发行未完成（zip + exe 缺一不可）════' -ForegroundColor Red
        exit 1
    }
    Write-Host '════ 发行完成：双产物齐 ════' -ForegroundColor Green
}

if ($VerifyOnly) { Verify '仅校验'; exit 0 }

# ── 1) zip 线（pack.ps1 红线不改，原样调用） ──
Write-Host '—— [0/2] 同步更新日志（仓内主副本 → 打包源）——' -ForegroundColor Cyan
# 【防漂移 2026-10-18】更新日志唯一事实源在仓内（发行脚本\更新日志.txt，
# git 跟踪）；打包前自动覆盖打包源副本——此前日志只活在打包源、无 git
# 记账，v1.8.7 曾整版漏写随包发行。
$changelog = 'E:\DSH-KF\hufu\发行脚本\更新日志.txt'
if (Test-Path $changelog) {
    Copy-Item $changelog 'E:\DSH-KF\hufu-发行\打包源\更新日志.txt' -Force
    $first = (Get-Content $changelog -TotalCount 1 -Encoding UTF8)
    Write-Host "  已同步，当前最新条目：$first"
    if ($first -notmatch [regex]::Escape($Version)) {
        Write-Host "  [警告] 日志顶部条目不含本版本 $Version——是否忘了写更新日志？" -ForegroundColor Yellow
    }
} else {
    Write-Host '  [警告] 仓内无更新日志主副本，沿用打包源现有版' -ForegroundColor Yellow
}

Write-Host '—— [1/2] zip 发行线（pack.ps1）——' -ForegroundColor Cyan
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File $pack -Version $Version -NoModel
if ($LASTEXITCODE -ne 0) { Write-Host '✗ zip 线失败，发行中止' -ForegroundColor Red; exit 1 }

# ── 2) exe 线（必须 rust 轨——ps1 轨是回滚轨，装时弹蓝窗） ──
Write-Host '—— [2/2] exe 发行线（build-setup.ps1 -Engine rust）——' -ForegroundColor Cyan
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File $setup -Version $Version -Engine rust
if ($LASTEXITCODE -ne 0) { Write-Host '✗ exe 线失败，发行中止' -ForegroundColor Red; exit 1 }

Verify '打包后'
