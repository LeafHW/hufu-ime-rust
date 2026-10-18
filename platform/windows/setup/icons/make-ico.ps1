# 虎符图标生成：虎符-透明.png → 虎符.ico（多尺寸）
# 规范：256px 用 PNG 条目（官方支持），64/48/32/16 用 32 位 DIB 条目，
#       AND 掩码全零（Vista+ 按 alpha 通道渲染，掩码不参与）。
# 用法：powershell -File make-ico.ps1
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$dir = Split-Path -Parent $MyInvocation.MyCommand.Path
$srcPath = Join-Path $dir '虎符-透明.png'
$icoPath = Join-Path $dir '虎符.ico'

$src = [Drawing.Image]::FromFile($srcPath)
$side = [Math]::Max($src.Width, $src.Height)
$sq = New-Object Drawing.Bitmap($side, $side)
$g = [Drawing.Graphics]::FromImage($sq)
$g.Clear([Drawing.Color]::Transparent)
$g.DrawImage($src, [int](($side-$src.Width)/2), [int](($side-$src.Height)/2), $src.Width, $src.Height)
$g.Dispose()

function New-SizeBitmap([int]$s) {
    $b = New-Object Drawing.Bitmap($s, $s)
    $g = [Drawing.Graphics]::FromImage($b)
    $g.InterpolationMode = [Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.PixelOffsetMode = [Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.SmoothingMode = [Drawing.Drawing2D.SmoothingMode]::HighQuality
    $g.DrawImage($sq, 0, 0, $s, $s)
    $g.Dispose(); return $b
}

function Get-DibBytes([Drawing.Bitmap]$b) {
    $s = $b.Width
    $rect = New-Object Drawing.Rectangle(0, 0, $s, $s)
    $bd = $b.LockBits($rect, [Drawing.Imaging.ImageLockMode]::ReadOnly, [Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $rowBytes = $s * 4
    $px = New-Object byte[] ($rowBytes * $s)
    [Runtime.InteropServices.Marshal]::Copy($bd.Scan0, $px, 0, $rowBytes * $s)
    $b.UnlockBits($bd)

    $ms = New-Object IO.MemoryStream
    $bw = New-Object IO.BinaryWriter($ms)
    # BITMAPINFOHEADER（完整 11 字段 = 40 字节；height = 双倍：像素 + 掩码）
    # 坑备忘：clrUsed/clrImportant 是 UInt32×2——写成 UInt16 会少 8 字节全错位
    $bw.Write([UInt32]40)             # biSize
    $bw.Write([Int32]$s)              # biWidth
    $bw.Write([Int32]($s * 2))        # biHeight（双倍）
    $bw.Write([UInt16]1)              # biPlanes
    $bw.Write([UInt16]32)             # biBitCount
    $bw.Write([UInt32]0)              # biCompression
    $bw.Write([UInt32]0)              # biSizeImage
    $bw.Write([Int32]0)               # biXPelsPerMeter
    $bw.Write([Int32]0)               # biYPelsPerMeter
    $bw.Write([UInt32]0)              # biClrUsed
    $bw.Write([UInt32]0)              # biClrImportant
    # 像素：Format32bppArgb 内存序本就是 B,G,R,A——DIB 要的正是 BGRA，
    # 原样直拷即可（2026-10-13 蓝虎实锤：曾误按"ARGB→BGRA"换序，R/B 对调成蓝虎）
    for ($y = $s - 1; $y -ge 0; $y--) {
        for ($x = 0; $x -lt $s; $x++) {
            $i = ($y * $s + $x) * 4
            $bw.Write($px[$i]); $bw.Write($px[$i+1]); $bw.Write($px[$i+2]); $bw.Write($px[$i+3])
        }
    }
    # AND 掩码全零（1bpp 行按 4 字节对齐）
    $maskRow = [int]([Math]::Ceiling($s / 32.0)) * 4
    $zeros = New-Object byte[] ($maskRow * $s)
    $bw.Write($zeros)
    $bw.Flush()
    # 逗号包裹：防止 PS 管道把 byte[] 展开成逐元素流（2026-10-13 实锤坑）
    return ,$ms.ToArray()
}

$entries = @()
foreach ($s in @(256, 64, 48, 32, 16)) {
    $b = New-SizeBitmap $s
    if ($s -eq 256) {
        $ms = New-Object IO.MemoryStream
        $b.Save($ms, [Drawing.Imaging.ImageFormat]::Png)
        $d = $ms.ToArray()
    } else {
        $d = Get-DibBytes $b
    }
    $b.Dispose()
    $entries += , @($s, $d)
}

$ms2 = New-Object IO.MemoryStream
$bw2 = New-Object IO.BinaryWriter($ms2)
$bw2.Write([UInt16]0); $bw2.Write([UInt16]1); $bw2.Write([UInt16]$entries.Count)
$offset = 6 + 16 * $entries.Count
foreach ($e in $entries) {
    $s = $e[0]; $d = $e[1]
    $dim = 0; if ($s -lt 256) { $dim = $s }
    $bw2.Write([Byte]$dim); $bw2.Write([Byte]$dim); $bw2.Write([Byte]0); $bw2.Write([Byte]0)
    $bw2.Write([UInt16]1); $bw2.Write([UInt16]32)
    $bw2.Write([UInt32]$d.Length); $bw2.Write([UInt32]$offset)
    $offset += $d.Length
}
foreach ($e in $entries) { $bw2.Write($e[1]) }
$bw2.Flush()
[IO.File]::WriteAllBytes($icoPath, $ms2.ToArray())
$bw2.Close(); $src.Dispose(); $sq.Dispose()
Write-Host ("OK {0}（{1:N1}KB，尺寸 256/64/48/32/16）" -f $icoPath, ((Get-Item $icoPath).Length / 1KB))
