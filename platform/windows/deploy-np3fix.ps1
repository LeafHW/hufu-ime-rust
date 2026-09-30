# Notepad3 seg1sel wide-box fix deploy (x64+x86 slim) - elevated
$ErrorActionPreference = 'Continue'
$log = 'E:\DSH-KF\.deploy-np3fix-log.txt'
'START ' + (Get-Date -Format 'HH:mm:ss') | Out-File $log -Encoding utf8
$src64 = 'E:\DSH-KF\hufu\platform\windows\target-slim\x86_64-pc-windows-gnu\release\hufu_tsf.dll'
$src32 = 'E:\DSH-KF\hufu\platform\windows\target-slim\i686-pc-windows-gnu\release\hufu_tsf.dll'
$dst64 = 'C:\Windows\SystemIME\HuFu\hufu_tsf.dll'
$dst32 = 'C:\Windows\SysWOW64\SystemIME\HuFu\hufu_tsf32.dll'
foreach ($pair in @(@($src64, $dst64), @($src32, $dst32))) {
    $d = $pair[1]
    if (Test-Path $d) {
        try {
            Remove-Item $pair[1] -Force -ErrorAction SilentlyContinue
            if (Test-Path $d) { Rename-Item $d ("hufu_tsf.old{0}" -f (Get-Random -Minimum 1000 -Maximum 9999)) -Force }
            "removed/renamed old: $d" | Out-File $log -Append -Encoding utf8
        } catch { "rename failed: $d - $($_.Exception.Message)" | Out-File $log -Append -Encoding utf8 }
    }
    Copy-Item $pair[0] $d -Force
    if (Test-Path $d) { "copied OK: $d ($(Get-Item $d).Length bytes)" | Out-File $log -Append -Encoding utf8 }
    else { "COPY FAILED: $d" | Out-File $log -Append -Encoding utf8 }
}
Stop-Process -Name ctfmon -Force -ErrorAction SilentlyContinue
'ctfmon stopped (auto restart by system)' | Out-File $log -Append -Encoding utf8
'DONE' | Out-File $log -Append -Encoding utf8
Get-Content $log
