# HuFu 虎符输入法 · 开机后清理薄壳（RunOnce 入口，指向本文件）
# ─────────────────────────────────────────────────────────────
# 职责：重启后清理安装目录（=本文件所在目录）与 SystemIME 的 .old* 腾位残留。
# 铁律：恒静默、恒退出 0——绝不打扰开机；一切细节由 hufu-setup-tools.ps1 -PostBoot 完成。
$ErrorActionPreference = 'Continue'
$instDir = $PSScriptRoot
if (-not $instDir) { $instDir = Split-Path -Parent $MyInvocation.MyCommand.Path }
$tools = Join-Path $instDir 'hufu-setup-tools.ps1'
try {
    if (Test-Path -LiteralPath $tools) {
        & $tools -PostBoot -InstDir $instDir -Log (Join-Path $instDir 'setup-tools.log')
    }
} catch { }
exit 0
