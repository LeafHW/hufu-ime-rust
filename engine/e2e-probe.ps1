# 焦点探针：新进程模态窗，看焦点/前台归属
$out = "$env:TEMP\hufu-e2e-probe.txt"
Remove-Item $out -Force -ErrorAction SilentlyContinue
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public class P2 {
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr h, System.Text.StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
}
'@
$form = New-Object System.Windows.Forms.Form
$form.Text = 'HuFuProbe'
$form.TopMost = $true
$tb = New-Object System.Windows.Forms.TextBox
$tb.Dock = 'Fill'
$form.Controls.Add($tb)
function Pump([int]$ms) { $d=[DateTime]::Now.AddMilliseconds($ms); while([DateTime]::Now -lt $d){[System.Windows.Forms.Application]::DoEvents(); Start-Sleep -Milliseconds 15} }
$form.Add_Shown({
  [void][P2]::SetForegroundWindow($form.Handle)
  Pump 800
  [void]$tb.Focus()
  Pump 500
  $sb = New-Object System.Text.StringBuilder 256
  [void][P2]::GetWindowText([P2]::GetForegroundWindow(), $sb, 256)
  Add-Content $out ("Focused={0} Foreground=[{1}] Visible={2}" -f $tb.Focused, $sb.ToString(), $form.Visible) -Encoding UTF8
  $form.Close()
})
[void]$form.ShowDialog()
Add-Content $out "text=[$($tb.Text)]" -Encoding UTF8
