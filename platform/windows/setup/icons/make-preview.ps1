# 生成虎符图标预览拼图（棋盘底显示透明度）
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$ico = 'E:\DSH-KF\hufu\platform\windows\setup\icons\虎符.ico'
$out = 'E:\DSH-KF\hufu\platform\windows\setup\icons\预览.png'

$sizes = @(256, 64, 48, 32, 16)
$W = 40; $H = 40
foreach ($s in $sizes) { $W += ($s + 40) }
$H += 256
$bmp = New-Object Drawing.Bitmap ($W, $H)
$g = [Drawing.Graphics]::FromImage($bmp)
# 棋盘格（10px）显透明
$cb1 = [Drawing.Color]::FromArgb(255, 204, 204, 204); $cb2 = [Drawing.Color]::FromArgb(255, 255, 255, 255)
for ($y = 0; $y -lt $H; $y += 10) {
    for ($x = 0; $x -lt $W; $x += 10) {
        $c = $cb1; if (([int]($x / 10) + [int]($y / 10)) % 2 -eq 0) { $c = $cb2 }
        $g.FillRectangle((New-Object Drawing.SolidBrush($c)), $x, $y, 10, 10)
    }
}
$x0 = 40
foreach ($s in $sizes) {
    $ic = [Drawing.Icon]::New($ico, $s, $s)
    $g.DrawIcon($ic, $x0, ($H - $s) / 2)
    $ic.Dispose()
    $x0 += ($s + 40)
}
$g.Dispose()
$bmp.Save($out, [Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Host ("OK {0}" -f $out)
