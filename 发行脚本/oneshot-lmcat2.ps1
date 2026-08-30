# 提权：HKLM 全局分类库 + TIP 树分类全套补齐（照抄虎爪/主流 IME 的 8 分类）
$ours = '{8F5C2A10-3E77-4B9C-A1D4-9E0B7C2F5A88}'
$cats = @(
  '{046B8C80-1647-40F7-9B21-B93B81AABC1B}',
  '{13A016DF-560B-46CD-947A-4C3AF1E0E35D}',
  '{25504FB4-7BAB-4BC1-9C69-CF81890F0EF5}',
  '{34745C63-B2F0-4784-8B67-5E12C8701A31}',
  '{364215D9-75BC-11D7-A6EF-00065B84435C}',
  '{49D2F9CE-1F5E-11D7-A6D3-00065B84435C}',
  '{49D2F9CF-1F5E-11D7-A6D3-00065B84435C}',
  '{CCF05DD7-4A87-11D7-A6E2-00065B84435C}'
)
$lmCat = 'HKLM:\SOFTWARE\Microsoft\CTF\Category'
$lmTip = "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$ours"
foreach ($c in $cats) {
  New-Item -Path "$lmCat\Category\$c\$ours" -Force | Out-Null
  New-Item -Path "$lmCat\Item\$ours\$c" -Force | Out-Null
  New-Item -Path "$lmTip\Category\Category\$c\$ours" -Force | Out-Null
  New-Item -Path "$lmTip\Category\Item\$ours\$c" -Force | Out-Null
}
$n1 = @(Get-ChildItem "$lmCat\Category" -EA SilentlyContinue | Where-Object { Test-Path "$($_.PSPath)\$ours" }).Count
$n2 = @(Get-ChildItem "$lmTip\Category\Category" -EA SilentlyContinue).Count
"DONE 全局库分类数=$n1 TIP树分类数=$n2" | Out-File "$env:TEMP\hufu-lmcat2.log" -Encoding utf8
