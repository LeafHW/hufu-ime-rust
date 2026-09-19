# 三十一修·活锚即显 — SystemIME 双通道部署（需提权一次）
# 部署后给 Users 授 Modify 权，后续部署无需再提权。
$ErrorActionPreference = 'Stop'
$src64 = 'E:\DSH-KF\hufu\platform\windows\target-fix\release\hufu_tsf.dll'
$src32 = 'E:\DSH-KF\hufu\platform\windows\target-fix\i686-pc-windows-gnu\release\hufu_tsf.dll'
$dst64 = 'C:\Windows\SystemIME\HuFu\hufu_tsf.dll'
$dst32 = 'C:\Windows\SysWOW64\SystemIME\HuFu\hufu_tsf32.dll'
$stamp = Get-Date -Format 'HHmmss'
$log = 'E:\DSH-KF\.deploy-31fix-log.txt'
try {
    "START " + (Get-Date -Format 'HH:mm:ss') | Out-File $log -Encoding utf8
    foreach ($d in @($dst64, $dst32)) {
        if (Test-Path $d) {
            try { Rename-Item $d ($d + '.old-' + $stamp) -Force; "renamed: $d" | Out-File $log -Append -Encoding utf8 }
            catch { "rename failed: $d — $($_.Exception.Message)" | Out-File $log -Append -Encoding utf8 }
        }
    }
    Copy-Item $src64 $dst64 -Force; 'copied x64 OK' | Out-File $log -Append -Encoding utf8
    Copy-Item $src32 $dst32 -Force; 'copied x86 OK' | Out-File $log -Append -Encoding utf8
    icacls 'C:\Windows\SystemIME\HuFu' /grant 'Users:(OI)(CI)M' | Out-Null
    icacls 'C:\Windows\SysWOW64\SystemIME\HuFu' /grant 'Users:(OI)(CI)M' | Out-Null
    icacls $dst64 /grant 'Users:M' | Out-Null
    icacls $dst32 /grant 'Users:M' | Out-Null
    'DONE' | Out-File $log -Append -Encoding utf8
}
catch {
    ('ERROR: ' + $_.Exception.Message) | Out-File $log -Append -Encoding utf8
}
