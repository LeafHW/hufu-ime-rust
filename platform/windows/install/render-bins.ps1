Add-Type -AssemblyName System.Drawing
$dir = "$env:TEMP\hufu-icon-bins"
New-Item -ItemType Directory -Force -Path $dir | Out-Null
$master = [System.Drawing.Image]::FromFile('E:\DSH-KF\hufu-icon-final-256.png')
foreach ($sz in @(16,32,48,256)) {
  $bmp = New-Object System.Drawing.Bitmap -ArgumentList $sz, $sz
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
  $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
  $g.Clear([System.Drawing.Color]::Transparent)
  $g.DrawImage($master, 0, 0, $sz, $sz)
  $g.Dispose()
  $bd = $bmp.LockBits((New-Object System.Drawing.Rectangle 0,0,$sz,$sz), [System.Drawing.Imaging.ImageLockMode]::ReadOnly, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
  $bytes = New-Object byte[] ($bd.Stride * $sz)
  [System.Runtime.InteropServices.Marshal]::Copy($bd.Scan0, $bytes, 0, $bytes.Length)
  $bmp.UnlockBits($bd)
  $bmp.Dispose()
  # stride==sz*4 here; strip to exact, keep top-down BGRA
  $out = New-Object byte[] ($sz*$sz*4)
  for ($y=0; $y -lt $sz; $y++) {
    [Array]::Copy($bytes, ($y*$bd.Stride), $out, ($y*$sz*4), ($sz*4))
  }
  [IO.File]::WriteAllBytes("$dir\$sz.bin", $out)
  Write-Host "wrote $dir\$sz.bin ($($out.Length) bytes)"
}
$master.Dispose()
