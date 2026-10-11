$ErrorActionPreference='Stop'
$root=Split-Path $PSScriptRoot -Parent
foreach($arch in @('x86','x64')) {
    $out=Join-Path $root ('evidence\'+$arch)
    & (Join-Path $out 'ComTests.exe') candidate_ui --nocapture | Tee-Object -FilePath (Join-Path $out 'com-final-tests.log')
    if($LASTEXITCODE -ne 0){throw "$arch COM tests failed"}
    & (Join-Path $PSScriptRoot 'test-load.ps1') -Arch $arch | Tee-Object -FilePath (Join-Path $out 'load-test.log')
    & (Join-Path $PSScriptRoot 'test-pain.ps1') -Arch $arch | Tee-Object -FilePath (Join-Path $out 'pain-test.log')
}
& (Join-Path $root 'evidence\x64\ServerTests.exe') --nocapture | Tee-Object -FilePath (Join-Path $root 'evidence\staged-server-tests.log')
if($LASTEXITCODE -ne 0){throw 'Server tests failed'}
Write-Output 'PASS all staged x86/x64 COM, DLL loading, Pain presentation, dual-window and server tests'
