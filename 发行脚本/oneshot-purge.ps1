# 提权：彻底卸载（机器级注册 + SystemIME + 8 分类 + 全部残留）
$ours = '{8F5C2A10-3E77-4B9C-A1D4-9E0B7C2F5A88}'
$profile = '{8F5C2A11-3E77-4B9C-A1D4-9E0B7C2F5A88}'
'== 1) 杀所有 hufu-server ==' | Out-File "$env:TEMP\hufu-purge.log" -Encoding utf8
Get-Process hufu-server -EA SilentlyContinue | Stop-Process -Force -EA SilentlyContinue
Start-Sleep -Seconds 1
"   剩余: $(@(Get-Process hufu-server -EA SilentlyContinue).Count)" | Out-File "$env:TEMP\hufu-purge.log" -Append -Encoding utf8
'== 2) HKLM 注册全清 ==' | Out-File "$env:TEMP\hufu-purge.log" -Append -Encoding utf8
foreach ($k in @(
    "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$ours",
    "HKLM:\SOFTWARE\Classes\CLSID\$ours",
    "HKLM:\SOFTWARE\WOW6432Node\Microsoft\CTF\TIP\$ours",
    "HKCU:\Software\Microsoft\CTF\TIP\$ours",
    "HKCU:\Software\Classes\CLSID\$ours",
    "HKCU:\Software\HuFu"
)) {
    if (Test-Path $k) { Remove-Item $k -Recurse -Force -EA SilentlyContinue; "   清: $k" | Out-File "$env:TEMP\hufu-purge.log" -Append -Encoding utf8 }
}
'== 3) 全局分类库条目（8 项） ==' | Out-File "$env:TEMP\hufu-purge.log" -Append -Encoding utf8
$lmCat = 'HKLM:\SOFTWARE\Microsoft\CTF\Category'
if (Test-Path $lmCat) {
    Get-ChildItem "$lmCat\Category" -EA SilentlyContinue | ForEach-Object {
        $t = "$($_.PSPath)\$ours"
        if (Test-Path $t) { Remove-Item $t -Recurse -Force -EA SilentlyContinue; "   清分类: $($_.PSChildName)" | Out-File "$env:TEMP\hufu-purge.log" -Append -Encoding utf8 }
    }
    if (Test-Path "$lmCat\Item\$ours") { Remove-Item "$lmCat\Item\$ours" -Recurse -Force -EA SilentlyContinue; "   清 Item" | Out-File "$env:TEMP\hufu-purge.log" -Append -Encoding utf8 }
}
'== 4) SystemIME DLL ==' | Out-File "$env:TEMP\hufu-purge.log" -Append -Encoding utf8
Remove-Item 'C:\Windows\SystemIME\HuFu' -Recurse -Force -EA SilentlyContinue
"   SystemIME\HuFu 存在: $(Test-Path 'C:\Windows\SystemIME\HuFu')" | Out-File "$env:TEMP\hufu-purge.log" -Append -Encoding utf8
'== 5) Run 自启 / 快捷方式 ==' | Out-File "$env:TEMP\hufu-purge.log" -Append -Encoding utf8
Remove-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' 'HuFu' -EA SilentlyContinue
Remove-Item "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\HuFu 虎符输入法设置.lnk" -Force -EA SilentlyContinue
'== 6) 语言列表移除（若在） ==' | Out-File "$env:TEMP\hufu-purge.log" -Append -Encoding utf8
$tipStr = "0804:$ours$profile"
$list = Get-WinUserLanguageList
$chg = $false
foreach ($l in $list) {
    $new = @($l.InputMethodTips | Where-Object { $_ -ne $tipStr })
    if ($new.Count -ne @($l.InputMethodTips).Count) { $l.InputMethodTips.Clear(); $new | ForEach-Object { [void]$l.InputMethodTips.Add($_) }; $chg = $true }
}
if ($chg) { Set-WinUserLanguageList $list -Force -WarningAction SilentlyContinue }
'== 7) 刷新宿主（让挂载的 DLL 释放） ==' | Out-File "$env:TEMP\hufu-purge.log" -Append -Encoding utf8
Stop-Process -Name TextInputHost, ShellExperienceHost, SearchHost, ctfmon -Force -EA SilentlyContinue
Start-Sleep -Seconds 2
Start-Process ctfmon -EA SilentlyContinue
'DONE 彻底卸载完成' | Out-File "$env:TEMP\hufu-purge.log" -Append -Encoding utf8
