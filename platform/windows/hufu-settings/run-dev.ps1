param(
    [switch]$NoDevServer
)

$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$appDir = (Resolve-Path (Join-Path $root '..\..\..\settings-app')).Path
$exe = Join-Path $root 'target\x86_64-pc-windows-gnu\release\hufu-settings.exe'

if (-not (Test-Path $exe)) {
    Write-Host "未找到 $exe"
    Write-Host '请先构建：'
    Write-Host "  cd `"$root`""
    Write-Host '  cargo build --release --target x86_64-pc-windows-gnu'
    exit 1
}

$vite = 'http://127.0.0.1:5173/'
$up = $false
try {
    $r = Invoke-WebRequest -UseBasicParsing -TimeoutSec 1 -Uri $vite
    if ($r.StatusCode -eq 200) { $up = $true }
} catch {}

if (-not $up -and -not $NoDevServer) {
    if (-not (Test-Path (Join-Path $appDir 'node_modules'))) {
        Write-Host "前端依赖未安装：cd `"$appDir`"; npm install"
        exit 1
    }
    Write-Host '启动 Vite 开发服务器…'
    Start-Process -FilePath 'powershell.exe' -ArgumentList @(
        '-NoExit', '-Command', "Set-Location '$appDir'; npm run dev"
    ) | Out-Null
    for ($i = 0; $i -lt 40; $i++) {
        Start-Sleep -Milliseconds 500
        try {
            $r = Invoke-WebRequest -UseBasicParsing -TimeoutSec 1 -Uri $vite
            if ($r.StatusCode -eq 200) { $up = $true; break }
        } catch {}
    }
}

if (-not $up) {
    Write-Host "Vite 未就绪：$vite"
    exit 1
}

Write-Host '启动虎符设置窗口（关闭窗口即退出）…'
& $exe --url 'http://127.0.0.1:5173/?shell=1'
