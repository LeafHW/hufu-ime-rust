$ErrorActionPreference='Stop'
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
$root='C:\Workspace\hufu-ime-rust\_tmp\arm64\profile-fix'
$app=Join-Path $env:LOCALAPPDATA 'Programs\HuFu'
$setup="$root\package\HuFuSetup-1.9.9.12.exe"
$results=[Collections.Generic.List[object]]::new()
function Run-Installer([string]$exe,[string]$label){
 $args="/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /LOG=$root\$label.log"
 $p=Start-Process -FilePath $exe -ArgumentList $args -PassThru
 $handle=$p.Handle
 $p.WaitForExit();$p.Refresh();$code=$p.ExitCode
 "action=$label exit=$code"
 $results.Add(@{action=$label;exit=$code})
 $results | ConvertTo-Json | Set-Content -Encoding UTF8 "$root\installer-cycle.json"
 if($null -eq $code -or $code -ne 0){throw "Installer action failed: $label code=$code"}
}
function Check-Registered([string]$label){
 foreach($arch in @('ARM64','x64','x86')){
  "action=$label architecture=$arch"
  & "$root\inspect-installed-$arch.exe" | Out-Host
  if($LASTEXITCODE){throw "Registered COM failed: $label $arch $LASTEXITCODE"}
 }
 if(-not (Get-Process hufu-server -ErrorAction SilentlyContinue)){throw 'Installed server is not running'}
 Copy-Item "$app\setup-engine.log" "$root\$label-engine.log" -Force
}
function Check-Absent {
 $clsid='{8F5C2A10-3E77-4B9C-A1D4-9E0B7C2F5A88}'
 foreach($view in @('Registry64','Registry32')){
  foreach($scope in @('LocalMachine','CurrentUser')){
   $h=[Microsoft.Win32.RegistryKey]::OpenBaseKey($scope,$view)
   foreach($key in @("Software\Microsoft\CTF\TIP\$clsid","Software\Classes\CLSID\$clsid")){
    $k=$h.OpenSubKey($key);if($k){$k.Dispose();throw "HuFu remains: $scope $view $key"}
   }
   $h.Dispose()
  }
 }
 if(Get-Process hufu-server -ErrorAction SilentlyContinue){throw 'Server remains after uninstall'}
 if(Test-Path "$app\hufu-server.exe"){throw 'Program remains after uninstall'}
 if(Test-Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{2F8A5D41-7B63-4C3E-9E15-A8D4B0C6F293}_is1'){throw 'ARP entry remains'}
 'uninstall verification: own COM/TIP absent in both views; server, program and ARP absent'
}
Start-Transcript -Path "$root\installer-cycle.log" -Force
try {
 Check-Absent
 Run-Installer $setup 'reinstall'
 Check-Registered 'reinstall'
 Run-Installer $setup 'upgrade-same-version'
 Check-Registered 'upgrade-same-version'
 Run-Installer "$app\unins000.exe" 'uninstall-verified'
 Check-Absent
 Copy-Item "$app\setup-engine.log" "$root\uninstall-verified-engine.log" -Force
 Run-Installer $setup 'install-final'
 Check-Registered 'install-final'
 'all installer cycle checks passed'
} finally {Stop-Transcript}
