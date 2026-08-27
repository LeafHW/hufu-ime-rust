# HuFu E2E v4：PostMessage 直投按键到 TextBox（不依赖系统前台）
$out = "$env:TEMP\hufu-e2e-result.txt"
Remove-Item $out -Force -ErrorAction SilentlyContinue
try {
  Add-Type -AssemblyName System.Windows.Forms
  Add-Type -AssemblyName System.Drawing
  Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public class K3 {
  [DllImport("user32.dll")]
  public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
  public static void Key(IntPtr h, ushort vk) {
    long scan = vk;
    IntPtr lp = new IntPtr((scan << 16) | 1);
    PostMessage(h, 0x0100, new IntPtr((long)vk), lp);
    System.Threading.Thread.Sleep(40);
    PostMessage(h, 0x0101, new IntPtr((long)vk), new IntPtr((scan << 16) | 0xC0000001L));
  }
}
'@

  $form = New-Object System.Windows.Forms.Form
  $form.Text = 'HuFu E2E'
  $form.Size = New-Object System.Drawing.Size(560, 240)
  $form.TopMost = $true
  $form.StartPosition = 'CenterScreen'
  $tb = New-Object System.Windows.Forms.TextBox
  $tb.Multiline = $true
  $tb.Dock = 'Fill'
  $tb.Font = New-Object System.Drawing.Font('Microsoft YaHei UI', 14)
  $form.Controls.Add($tb)

  $script:results = New-Object System.Collections.ArrayList
  function Pump([int]$ms) {
    $deadline = [DateTime]::Now.AddMilliseconds($ms)
    while ([DateTime]::Now -lt $deadline) { [System.Windows.Forms.Application]::DoEvents(); Start-Sleep -Milliseconds 15 }
  }
  function TypeKeys([int[]]$vks) {
    foreach ($v in $vks) { [K3]::Key($tb.Handle, [UInt16]$v); Pump 160 }
  }
  function Trial([string]$name, [int[]]$vks, [string]$expect) {
    $tb.Clear(); Pump 150
    [void]$tb.Focus(); Pump 100
    TypeKeys $vks; Pump 800
    $text = $tb.Text
    [void]$script:results.Add([pscustomobject]@{ Test = $name; Got = $text; Expect = $expect; OK = ($text -eq $expect) })
  }
  function Flush([string]$line) { Add-Content -Path $out -Value $line -Encoding UTF8 }

  $form.Add_Shown({
    try {
      Pump 400
      [void]$tb.Focus()
      Pump 300
      Trial '开头逗号' @(0xBC) '，'
      Trial '开头句号' @(0xBE) '。'
      Trial 'a+逗号' @(0x41, 0xBC) '来，'
      Trial 'jd+空格' @(0x4A, 0x44, 0x20) '人'
      Trial 'jd+数字2' @(0x4A, 0x44, 0x32) '什么'
    } finally {
      $form.Close()
    }
  })
  [void]$form.ShowDialog()

  foreach ($r in $results) {
    $mark = if ($r.OK) { 'PASS' } else { 'FAIL' }
    Flush ('  [{0}] {1}: got=[{2}] expect=[{3}]' -f $mark, $r.Test, $r.Got, $r.Expect)
  }
  $npass = @($results | Where-Object OK).Count
  Flush ('E2E: {0}/{1}' -f $npass, $results.Count)
} catch {
  Add-Content -Path $out -Value "EXCEPTION: $($_.Exception.Message)" -Encoding UTF8
}
exit 0
