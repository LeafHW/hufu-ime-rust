$ErrorActionPreference = 'Stop'
$exe = Join-Path $PSScriptRoot 'target\x86_64-pc-windows-gnu\release\hufu-settings.exe'
if (-not (Test-Path $exe)) {
    Write-Host '请先构建：cargo build --release --target x86_64-pc-windows-gnu'
    exit 1
}
try {
    $r = Invoke-WebRequest -UseBasicParsing -Uri 'http://127.0.0.1:4390/next' -TimeoutSec 2
    if ($r.StatusCode -ne 200) { throw "HTTP $($r.StatusCode)" }
} catch {
    Write-Host 'hufu-server 的 /next 未就绪；先启动/部署新版 hufu-server。'
    exit 1
}
Write-Host '启动虎符设置窗口（/next，关闭窗口即退出）…'
& $exe
