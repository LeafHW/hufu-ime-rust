$ErrorActionPreference = 'SilentlyContinue'
Get-ChildItem 'E:\DSH-KF\hufu\platform\windows\target\release\hufu_tsf.dll.lock*' | ForEach-Object {
  Write-Host ("占用副本: {0}  {1}" -f $_.Name, $_.LastWriteTime.ToString('HH:mm:ss'))
}
foreach ($p in @("$env:TEMP\hufu-tsf.log", "$env:LOCALAPPDATA\HuFu\tsf.log", 'E:\DSH-KF\hufu\hufu-data\tsf.log', "$env:ProgramData\HuFu\tsf.log")) {
  if (Test-Path $p) {
    Write-Host "── 日志 $p（尾 10 行）:"
    Get-Content $p -Tail 10 | ForEach-Object { Write-Host "  $_" }
  }
}
Get-Process | ForEach-Object {
  $proc = $_
  $hit = $false
  foreach ($m in $proc.Modules) {
    if ($m.ModuleName -eq 'hufu_tsf.dll') { $hit = $true; break }
  }
  if ($hit) { Write-Host ("加载进程: {0} (PID {1})" -f $proc.ProcessName, $proc.Id) }
}
exit 0
