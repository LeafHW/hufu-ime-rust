param([switch]$Release)
$ErrorActionPreference = 'Stop'
$manifest = Join-Path $PSScriptRoot 'Cargo.toml'
$buildArgs = @('build', '--locked', '--manifest-path', $manifest, '--target', 'x86_64-pc-windows-gnu', '-p', 'gale-app', '--bin', 'hufu-trainer')
if ($Release) { $buildArgs += '--release' }
& cargo @buildArgs
if ($LASTEXITCODE -ne 0) { throw "跟打器构建失败：$LASTEXITCODE" }
