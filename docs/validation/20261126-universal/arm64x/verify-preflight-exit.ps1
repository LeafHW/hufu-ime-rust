
$ErrorActionPreference = 'Continue'
$exe = 'C:\Workspace\hufu-ime-rust\_tmp\arm64\build\payload-95b54d2fba2246e98c97fe7f937d54e8\arm64\hufu-setup-engine.exe'
$bad = 'C:\Workspace\hufu-ime-rust\_tmp\arm64\build\payload-1143f8c8c6af4147916c975a9f920a40\arm64'
$log = 'C:\Workspace\hufu-ime-rust\docs\validation\20261126-universal\arm64x\bad-payload-preflight.log'
& $exe preflight --src $bad --log $log | Out-Host
$code = $LASTEXITCODE
Write-Output "pipeline_exit_code=$code"
if ($code -ne 1) { exit 2 }
