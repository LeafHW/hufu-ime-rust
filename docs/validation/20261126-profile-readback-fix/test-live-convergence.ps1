$ErrorActionPreference='Stop'
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
Start-Transcript -Path 'C:\Workspace\hufu-ime-rust\_tmp\arm64\profile-fix\live-convergence.log' -Force
function Invoke-Probe([string]$Mode) {
 $ErrorActionPreference='Continue'
 & $script:tool $Mode 2>&1 | ForEach-Object {"$_"} | Out-Host
 return $LASTEXITCODE
}
try {
 $root='C:\Windows\SystemIME\HuFu\generations\1.9.9.11-e0e7a15ec8f7'
 $script:tool="$root\hufu-tsf-smoke.exe"
 foreach($cycle in 1..3){
  Write-Output "cycle=$cycle unregister"
  if((Invoke-Probe 'unreg') -ne 0){throw 'unregister failed'}
  $timer=[Diagnostics.Stopwatch]::StartNew()
  $absent=$false
  foreach($n in 0..40){
   $code=Invoke-Probe 'inspect-profile'
   Write-Output "unregister elapsed_ms=$($timer.ElapsedMilliseconds) exit=$code"
   if($code -eq 2){$absent=$true;break}
   Start-Sleep -Milliseconds 250
  }
  if(-not $absent){throw 'unregister never became visible'}
  $ErrorActionPreference='Continue'
  & $tool reg "$root\hufu_tsf.dll" 2>&1 | ForEach-Object {"$_"} | Out-Host
  $code=$LASTEXITCODE
  $ErrorActionPreference='Stop'
  if($code -ne 0){throw 'register failed'}
  $timer.Restart();$present=$false
  foreach($n in 0..40){
   $code=Invoke-Probe 'inspect-profile'
   Write-Output "register elapsed_ms=$($timer.ElapsedMilliseconds) exit=$code"
   if($code -eq 0){$present=$true;break}
   Start-Sleep -Milliseconds 250
  }
  if(-not $present){throw 'register never became visible'}
 }
} finally {Stop-Transcript}
