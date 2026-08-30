# 提权补全：HKLM 全局分类库（切换器认键盘 TIP 的依据）
$ours = '{8F5C2A10-3E77-4B9C-A1D4-9E0B7C2F5A88}'
$kbd = '{34745C63-B2F0-4784-8B67-5E12C8701A31}'
$master = '{533C5E0E-5AC0-4ABD-B6F1-251B82B7BE7D}'
$root = 'HKLM:\SOFTWARE\Microsoft\CTF\Category'
foreach ($cat in @($kbd, $master)) {
    New-Item -Path "$root\Category\$cat\$ours" -Force | Out-Null
    New-Item -Path "$root\Item\$ours\$cat" -Force | Out-Null
}
$r1 = Test-Path "$root\Category\$kbd\$ours"
$r2 = Test-Path "$root\Item\$ours\$kbd"
"HKLM Category kbd=$r1 item=$r2" | Out-File "$env:TEMP\hufu-lmcat.log" -Encoding utf8
