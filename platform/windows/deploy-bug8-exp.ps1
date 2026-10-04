# deploy-bug8-exp: BUG8 prep experiment - de-specialized DLLs to all 4 registration points.
# ASCII only. Engine server NOT touched. Renames old DLLs to .oldNNNN for rollback.
$log = 'E:\DSH-KF\.deploy-bug8-exp-log.txt'
try {
    'START ' + (Get-Date -Format 'HH:mm:ss') | Out-File $log -Encoding utf8
    $ErrorActionPreference = 'Continue'
    $new64 = 'E:\DSH-KF\hufu\platform\windows\target\x86_64-pc-windows-gnu\release\hufu_tsf.dll'
    $new32 = 'E:\DSH-KF\hufu\platform\windows\target\i686-pc-windows-gnu\release\hufu_tsf.dll'
    # resolve release dir without Chinese literals
    $rel = Get-ChildItem 'D:\HUFU' -Directory | Where-Object { $_.Name -like 'HuFu*' -and $_.Name -like '*1.6.9*' } | Select-Object -First 1
    if (-not $rel) { throw 'release dir not found under D:\HUFU' }
    "release dir: $($rel.FullName)" | Out-File $log -Append -Encoding utf8
    $pairs = @(
        @($new64, 'C:\Windows\SystemIME\HuFu\hufu_tsf.dll'),
        @($new32, 'C:\Windows\SysWOW64\SystemIME\HuFu\hufu_tsf32.dll'),
        @($new32, (Join-Path $rel.FullName 'hufu_tsf32.dll')),
        @($new64, (Join-Path $rel.FullName 'hufu_tsf.dll'))
    )
    foreach ($pair in $pairs) {
        $src = $pair[0]; $d = $pair[1]
        if (-not (Test-Path $src)) { throw "src missing: $src" }
        if (Test-Path $d) {
            try {
                Rename-Item $d ("hufu_tsf.old{0}" -f (Get-Random -Minimum 1000 -Maximum 9999)) -Force
                "renamed old: $d" | Out-File $log -Append -Encoding utf8
            } catch {
                "rename failed: $d - $($_.Exception.Message)" | Out-File $log -Append -Encoding utf8
            }
        }
        Copy-Item $src $d -Force
        if (Test-Path $d) { "copied OK: $d ($(Get-Item $d).Length bytes)" | Out-File $log -Append -Encoding utf8 }
        else { throw "copy failed: $d" }
    }
    Stop-Process -Name ctfmon -Force -ErrorAction SilentlyContinue
    'ctfmon stopped (auto restart by system)' | Out-File $log -Append -Encoding utf8
    'DONE' | Out-File $log -Append -Encoding utf8
    Write-Output 'DEPLOY OK'
} catch {
    ('ERROR: ' + $_.Exception.Message) | Out-File $log -Append -Encoding utf8
    Write-Output ('DEPLOY FAIL: ' + $_.Exception.Message)
}
