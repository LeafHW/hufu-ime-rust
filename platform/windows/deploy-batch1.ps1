# deploy-batch1: 2026-11 batch-1 features (Esc-undo, custom select keys, history stack,
# func-word punct push, backtick gate). ASCII only. Installs BOTH engine server + TSF DLLs.
$log = 'E:\DSH-KF\.deploy-batch1-log.txt'
try {
    'START ' + (Get-Date -Format 'HH:mm:ss') | Out-File $log -Encoding utf8
    $ErrorActionPreference = 'Continue'
    $src64 = 'E:\DSH-KF\hufu\platform\windows\target-slim\x86_64-pc-windows-gnu\release\hufu_tsf.dll'
    $src32 = 'E:\DSH-KF\hufu\platform\windows\target-fix\i686-pc-windows-gnu\release\hufu_tsf.dll'
    $dst64 = 'C:\Windows\SystemIME\HuFu\hufu_tsf.dll'
    $dst32 = 'C:\Windows\SysWOW64\SystemIME\HuFu\hufu_tsf32.dll'
    foreach ($pair in @(@($src64, $dst64), @($src32, $dst32))) {
        $d = $pair[1]
        if (Test-Path $d) {
            try {
                Rename-Item $d ("hufu_tsf.old{0}" -f (Get-Random -Minimum 1000 -Maximum 9999)) -Force
                "renamed old: $d" | Out-File $log -Append -Encoding utf8
            } catch {
                "rename failed: $d - $($_.Exception.Message)" | Out-File $log -Append -Encoding utf8
            }
        }
        Copy-Item $pair[0] $d -Force
        if (Test-Path $d) { "copied OK: $d ($(Get-Item $d).Length bytes)" | Out-File $log -Append -Encoding utf8 }
        else { throw "copy failed: $d" }
    }
    # swap engine server (old one may be running: stop, rename, copy, keep .old for rollback)
    $srcSrv = 'E:\DSH-KF\hufu\engine\target\release\hufu-server.exe'
    $dstSrv = 'D:\HUFU\HuFu虎符输入法-v1.6.9-无模型\hufu-server.exe'
    if (Get-Process hufu-server -ErrorAction SilentlyContinue) {
        Stop-Process -Name hufu-server -Force -ErrorAction SilentlyContinue
        Start-Sleep -Milliseconds 800
        "server stopped" | Out-File $log -Append -Encoding utf8
    }
    if (Test-Path $dstSrv) {
        Rename-Item $dstSrv ("hufu-server.old{0}" -f (Get-Random -Minimum 1000 -Maximum 9999)) -Force
        "renamed old server" | Out-File $log -Append -Encoding utf8
    }
    Copy-Item $srcSrv $dstSrv -Force
    "copied OK: $dstSrv" | Out-File $log -Append -Encoding utf8
    # restart server (same as HKCU Run entry)
    Start-Process $dstSrv
    "server restarted" | Out-File $log -Append -Encoding utf8
    Stop-Process -Name ctfmon -Force -ErrorAction SilentlyContinue
    'ctfmon stopped (auto restart by system)' | Out-File $log -Append -Encoding utf8
    'DONE' | Out-File $log -Append -Encoding utf8
    Write-Output 'DEPLOY OK'
} catch {
    ('ERROR: ' + $_.Exception.Message) | Out-File $log -Append -Encoding utf8
    Write-Output ('DEPLOY FAIL: ' + $_.Exception.Message)
}
