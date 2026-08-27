$ErrorActionPreference = 'Continue'
Get-Process hufu-server -ErrorAction SilentlyContinue | ForEach-Object {
  Write-Host ("found PID {0}" -f $_.Id)
  Stop-Process -Id $_.Id -Force
}
Start-Sleep -Seconds 2
$left = Get-Process hufu-server -ErrorAction SilentlyContinue
if ($left) { Write-Host "still alive: $($left.Id -join ',')" } else { Write-Host "no hufu-server process" }
$exe = 'E:\DSH-KF\hufu\engine\target\release\hufu-server.exe'
try {
  $h = [System.IO.File]::Open($exe, 'Open', 'ReadWrite', 'None')
  $h.Close()
  Write-Host "exe 可写（锁已释放）"
} catch {
  Write-Host ("exe 仍被锁: {0}" -f $_.Exception.Message)
}
exit 0
