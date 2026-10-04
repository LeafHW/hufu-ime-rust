# deploy-batch1c: final step - kill stale server (started by elevated admin run,
# cannot be killed from non-elevated session) and restart with new exe.
# Run this from an ELEVATED PowerShell. ASCII only.
$log = 'E:\DSH-KF\.deploy-batch1c-log.txt'
try {
    'START ' + (Get-Date -Format 'HH:mm:ss') | Out-File $log -Encoding utf8
    $ErrorActionPreference = 'Continue'

    # kill ALL hufu-server instances (elevated ones need admin)
    Get-Process hufu-server -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 800
    if (Get-Process hufu-server -ErrorAction SilentlyContinue) { throw 'still alive' }
    'all server instances killed' | Out-File $log -Append -Encoding utf8

    $root = (Get-ChildItem 'D:\HUFU' -Directory |
        Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'hufu-server.exe') } |
        Select-Object -First 1).FullName
    Start-Process -FilePath (Join-Path $root 'hufu-server.exe') -WorkingDirectory $root
    Start-Sleep -Seconds 2
    if (Get-Process hufu-server -ErrorAction SilentlyContinue) {
        'server restarted with new exe' | Out-File $log -Append -Encoding utf8
    } else { throw 'server did not start' }

    # verify: new config keys present over API
    $ok = $false
    try {
        $r = Invoke-WebRequest 'http://127.0.0.1:4390/api/config' -UseBasicParsing -TimeoutSec 3
        $j = $r.Content | ConvertFrom-Json
        if ($null -ne $j.general.esc_undo) { $ok = $true }
    } catch {}
    ("api verify esc_undo key: {0}" -f $ok) | Out-File $log -Append -Encoding utf8

    'DONE' | Out-File $log -Append -Encoding utf8
    Write-Output 'DEPLOY OK (restart done)'
} catch {
    ('ERROR: ' + $_.Exception.Message) | Out-File $log -Append -Encoding utf8
    Write-Output ('DEPLOY FAIL: ' + $_.Exception.Message)
}
