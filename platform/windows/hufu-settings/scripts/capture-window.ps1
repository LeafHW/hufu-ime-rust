param(
    [string]$ProcessName = 'hufu-settings',
    [string]$OutDir = (Join-Path $PSScriptRoot '..\screenshots')
)

$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
Add-Type -AssemblyName System.Drawing

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class SpikeCap32
{
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }

    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdcBlt, uint nFlags);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
}
'@

[SpikeCap32]::SetProcessDPIAware() | Out-Null

$proc = Get-Process -Name $ProcessName -ErrorAction Stop | Select-Object -First 1
$proc.Refresh()
$hwnd = $proc.MainWindowHandle
if ($hwnd -eq [IntPtr]::Zero) {
    throw "process $($proc.Id) has no main window handle"
}

[SpikeCap32]::ShowWindow($hwnd, 9) | Out-Null   # SW_RESTORE
[SpikeCap32]::SetForegroundWindow($hwnd) | Out-Null
Start-Sleep -Milliseconds 600

$rect = New-Object SpikeCap32+RECT
if (-not [SpikeCap32]::GetWindowRect($hwnd, [ref]$rect)) {
    throw "GetWindowRect failed"
}
$width = $rect.Right - $rect.Left
$height = $rect.Bottom - $rect.Top
if ($width -le 0 -or $height -le 0) {
    throw "bad window rect: $width x $height"
}

# 1) PrintWindow with PW_RENDERFULLCONTENT (0x2). Works for many
#    DirectComposition-backed windows, but WebView2 may return a blank bitmap.
$bmp = New-Object System.Drawing.Bitmap($width, $height)
$gfx = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $gfx.GetHdc()
$printOk = [SpikeCap32]::PrintWindow($hwnd, $hdc, 2)
$gfx.ReleaseHdc($hdc)
$gfx.Dispose()
$printPath = Join-Path $OutDir 'window-printwindow.png'
$bmp.Save($printPath, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()

# 2) Screen-region capture as a fallback; this shows the actual composited
#    WebView2 surface.
Start-Sleep -Milliseconds 300
$bmp2 = New-Object System.Drawing.Bitmap($width, $height)
$gfx2 = [System.Drawing.Graphics]::FromImage($bmp2)
$gfx2.CopyFromScreen($rect.Left, $rect.Top, 0, 0, (New-Object System.Drawing.Size($width, $height)))
$gfx2.Dispose()
$screenPath = Join-Path $OutDir 'window-screen.png'
$bmp2.Save($screenPath, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp2.Dispose()

$info = [ordered]@{
    pid = $proc.Id
    process = $proc.ProcessName
    title = $proc.MainWindowTitle
    hwnd = $hwnd.ToInt64()
    rect = @{ left = $rect.Left; top = $rect.Top; right = $rect.Right; bottom = $rect.Bottom }
    width = $width
    height = $height
    visible = [SpikeCap32]::IsWindowVisible($hwnd)
    printwindow_ok = $printOk
    printwindow_png = $printPath
    screen_png = $screenPath
}
$info | ConvertTo-Json -Depth 4 | Out-File -FilePath (Join-Path $OutDir 'window-info.json') -Encoding utf8
$info | ConvertTo-Json -Depth 4
