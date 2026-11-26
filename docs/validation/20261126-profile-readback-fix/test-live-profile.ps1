$ErrorActionPreference='Stop'
Start-Transcript -Path 'C:\Workspace\hufu-ime-rust\_tmp\arm64\profile-fix\live-profile.log' -Force
try {
 $root='C:\Windows\SystemIME\HuFu\generations\1.9.9.11-e0e7a15ec8f7'
 $clsid='{8F5C2A10-3E77-4B9C-A1D4-9E0B7C2F5A88}'
 $profile='{8F5C2A11-3E77-4B9C-A1D4-9E0B7C2F5A88}'
 $manifest=Get-Content "$root\package-architecture.json" -Raw | ConvertFrom-Json
 foreach($spec in @(@('Registry64','tsf-native'),@('Registry32','tsf-x86'))){
  $hive=[Microsoft.Win32.RegistryKey]::OpenBaseKey('LocalMachine',$spec[0])
  $c=$hive.CreateSubKey("Software\Classes\CLSID\$clsid")
  $c.SetValue('','HuFu TSF Service');$ips=$c.CreateSubKey('InprocServer32')
  $file=($manifest.files | Where-Object role -eq $spec[1]).path
  $ips.SetValue('',"$root\$file");$ips.SetValue('ThreadingModel','Apartment');$ips.Dispose();$c.Dispose();$hive.Dispose()
 }
 $hive=[Microsoft.Win32.RegistryKey]::OpenBaseKey('LocalMachine','Registry64')
 $tip=$hive.CreateSubKey("Software\Microsoft\CTF\TIP\$clsid")
 $desc=$tip.CreateSubKey('Description');$desc.SetValue('','HuFu TSF');$desc.Dispose()
 $cats=@('{046B8C80-1647-40F7-9B21-B93B81AABC1B}','{13A016DF-560B-46CD-947A-4C3AF1E0E35D}','{25504FB4-7BAB-4BC1-9C69-CF81890F0EF5}','{34745C63-B2F0-4784-8B67-5E12C8701A31}','{364215D9-75BC-11D7-A6EF-00065B84435C}','{49D2F9CE-1F5E-11D7-A6D3-00065B84435C}','{49D2F9CF-1F5E-11D7-A6D3-00065B84435C}','{CCF05DD7-4A87-11D7-A6E2-00065B84435C}')
 foreach($cat in $cats){
  $tip.CreateSubKey("Category\Category\$cat\$clsid").Dispose()
  $tip.CreateSubKey("Category\Item\$clsid\$cat").Dispose()
  $hive.CreateSubKey("Software\Microsoft\CTF\Category\Category\$cat\$clsid").Dispose()
  $hive.CreateSubKey("Software\Microsoft\CTF\Category\Item\$clsid\$cat").Dispose()
 }
 $lp=$tip.CreateSubKey("LanguageProfile\0x00000804\$profile")
 $lp.SetValue('Description','HuFu TSF');$lp.SetValue('Display Description','HuFu TSF')
 $lp.SetValue('Enable',1,'DWord');$lp.SetValue('IconFile',"$root\hufu_tsf.dll");$lp.SetValue('IconIndex',0,'DWord');$lp.Dispose();$tip.Dispose();$hive.Dispose()
 & "$root\hufu-tsf-smoke.exe" reg "$root\hufu_tsf.dll" | Out-Host
 if($LASTEXITCODE){throw "register failed $LASTEXITCODE"}
 $timer=[Diagnostics.Stopwatch]::StartNew()
 foreach($pause in @(0,100,500,2000,5000,10000)){
  if($pause){Start-Sleep -Milliseconds $pause}
  Write-Output "inspect elapsed_ms=$($timer.ElapsedMilliseconds)"
  & "$root\hufu-tsf-smoke.exe" inspect-profile 2>&1 | Out-Host
  Write-Output "inspect_exit=$LASTEXITCODE"
 }
} finally {Stop-Transcript}
