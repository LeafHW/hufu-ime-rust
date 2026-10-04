# deploy-batch1b: server swap + ctfmon restart (ASCII ONLY - no Chinese in paths).
# DLLs (64+32) were already deployed by deploy-batch1.ps1 admin run.
# Install dir resolved via wildcard to avoid non-ASCII literals (PS5.1 reads
# BOM-less UTF-8 as GBK -> mojibake paths -> "illegal characters in path").
$log = 'E:\DSH-KF\.deploy-batch1b-log.txt'
try {
    'START ' + (Get-Date -Format 'HH:mm:ss') | Out-File $log -Encoding utf8
    $ErrorActionPreference = 'Stop'

    # 1) locate install dir: the D:\HUFU subdir that contains hufu-server.exe
    $root = Get-ChildItem 'D:\HUFU' -Directory |
        Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'hufu-server.exe') } |
        Select-Object -First 1
    if (-not $root) { throw 'install dir not found under D:\HUFU' }
    $dstSrv = Join-Path $root.FullName 'hufu-server.exe'
    "install dir: $($root.FullName)" | Out-File $log -Append -Encoding utf8

    # 2) stop running server (if any), swap exe with backup
    if (Get-Process hufu-server -ErrorAction SilentlyContinue) {
        Stop-Process -Name hufu-server -Force -ErrorAction SilentlyContinue
        Start-Sleep -Milliseconds 800
        'server stopped' | Out-File $log -Append -Encoding utf8
    }
    if (Test-Path -LiteralPath $dstSrv) {
        Rename-Item -LiteralPath $dstSrv ('hufu-server.old{0}' -f (Get-Random -Minimum 1000 -Maximum 9999)) -Force
        'renamed old server' | Out-File $log -Append -Encoding utf8
    }
    Copy-Item 'E:\DSH-KF\hufu\engine\target\release\hufu-server.exe' $dstSrv -Force
    ('copied OK: {0} ({1} bytes)' -f $dstSrv, (Get-Item -LiteralPath $dstSrv).Length) | Out-File $log -Append -Encoding utf8

    # 3) restart server (autostart equivalent)
    Start-Process -FilePath $dstSrv -WorkingDirectory $root.FullName
    Start-Sleep -Milliseconds 1200
    if (Get-Process hufu-server -ErrorAction SilentlyContinue) { 'server restarted' | Out-File $log -Append -Encoding utf8 }
    else { throw 'server did not start' }

    # 4) restart ctfmon so TSF reloads the new DLLs
    Stop-Process -Name ctfmon -Force -ErrorAction SilentlyContinue
    'ctfmon stopped (auto restart by system)' | Out-File $log -Append -Encoding utf8

    'DONE' | Out-File $log -Append -Encoding utf8
    Write-Output 'DEPLOY OK'
} catch {
    ('ERROR: ' + $_.Exception.Message) | Out-File $log -Append -Encoding utf8
    Write-Output ('DEPLOY FAIL: ' + $_.Exception.Message)
}
