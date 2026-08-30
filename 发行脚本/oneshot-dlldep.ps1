# 提权：SystemIME 新 DLL 部署（占用腾位）
$src = 'E:\DSH-KF\hufu\platform\windows\target\release\hufu_tsf.dll'
$dst = 'C:\Windows\SystemIME\HuFu\hufu_tsf.dll'
$log = "$env:TEMP\hufu-dlldep.log"
Remove-Item $log -Force -ErrorAction SilentlyContinue
try {
    Copy-Item $src $dst -Force -ErrorAction Stop
    'COPIED' | Out-File $log -Encoding utf8
} catch {
    $n = 1
    while (Test-Path "$dst.old$n") { $n++ }
    Rename-Item $dst "hufu_tsf.dll.old$n" -Force
    Copy-Item $src $dst -Force
    "RENAMED old$n" | Out-File $log -Encoding utf8
}
