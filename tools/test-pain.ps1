param([string]$PainRoot = 'C:\Users\yc\Desktop\paindutch_next', [ValidateSet('x86','x64')][string]$Arch='x86')
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$out = Join-Path $root ('evidence\' + $Arch)
$presenter = Join-Path $PainRoot 'TsfCandidatePresenter.cs'
$tests = Join-Path $PainRoot 'Tools\CandidatePresenterTests.cs'
$hashes = @($presenter, $tests) | ForEach-Object { Get-FileHash -LiteralPath $_ }
# Compile the presenter's actual source read-only. Extend a COPY of its fixture
# to pass the native COM object through Pain's normal Begin/Update/Render path.
$source = [IO.File]::ReadAllText($tests)
$source = $source.Replace('internal readonly Page Page = new Page();', 'internal readonly Page Page = new Page(); internal object Native;')
$source = $source.Replace('value = Page; }', 'value = Native ?? Page; }')
$integration = @'
    [System.Runtime.InteropServices.DllImport("hufu_tsf.dll", CallingConvention = System.Runtime.InteropServices.CallingConvention.Cdecl, ExactSpelling = true)]
    private static extern int CreateTestCandidate(uint page, out IntPtr value);
    [System.Runtime.InteropServices.DllImport("hufu_tsf.dll", CallingConvention = System.Runtime.InteropServices.CallingConvention.Cdecl, ExactSpelling = true)]
    private static extern int UpdateTestCandidate(uint page);
    [System.Runtime.InteropServices.DllImport("hufu_tsf.dll", CallingConvention = System.Runtime.InteropServices.CallingConvention.Cdecl, ExactSpelling = true)]
    private static extern int ProbeWindow(int show);
    private static void NativeHuFu() {
        using (var f = new Fixture()) {
            IntPtr pointer;
            Check(CreateTestCandidate(0, out pointer) == 0, "Native factory failed");
            object native = System.Runtime.InteropServices.Marshal.GetObjectForIUnknown(pointer);
            System.Runtime.InteropServices.Marshal.Release(pointer);
            f.Manager.Native = native;
            var window = (Window)Get(f.Presenter, "_window");
            var input = (TextBox)Get(f.Presenter, "_input");
            window.Content = input; window.Width = 400; window.Height = 150;
            window.Title = "HuFu isolated candidate bridge test";
            window.Show(); window.Activate(); input.Focus();
            Set(f.Presenter, "_nativeOnly", false);
            int show = 1;
            f.Presenter.Begin(1, ref show);
            Check(show == 0, "Pain did not request native UI suppression");
            Check(ProbeWindow(1) == 1, "Original HuFu D2D window failed to show");
            f.Presenter.Update(1); f.Render();
            Check((string)Get(f.Presenter, "_firstValue") == "first", "First-page text");
            Check(UpdateTestCandidate(1) == 0, "Native update failed");
            f.Presenter.Update(1);
            f.Render();
            Check((string)Get(f.Presenter, "_firstValue") == "remaining", "Native current page first candidate");
            Check(f.Rows.Children.Count == 1, "Native final-page count");
            var row = (Grid)f.Rows.Children[0];
            Check(((TextBlock)row.Children[0]).Text == "\ud840\udc00e\u0301", "Native Unicode display");
            Check(row.Background != Brushes.Transparent, "Native selection highlight");
            Check(((TextBlock)Get(f.Presenter, "_pageLabel")).Text == "2/2", "Native page label");
            var candidates = (TsfCandidates)native;
            candidates.Show(0); // Deliberately does not control the original window.
            int shown; candidates.IsShown(out shown);
            Check(shown != 0, "IsShown does not reflect original window visibility");
            f.Presenter.Update(1); f.Render();
            Check((string)Get(f.Presenter, "_firstValue") == "remaining", "Show(false) stopped candidates");
            Check(ProbeWindow(-1) == 1, "Show(false) hid original HuFu window");
            Check(ProbeWindow(0) == 1, "Original window hide failed");
            f.Manager.Native = null;
            window.Close();
            System.Runtime.InteropServices.Marshal.FinalReleaseComObject(native);
        }
        Console.WriteLine("PASS native HuFu COM through Pain Begin(false), Update and current-page rendering");
    }
'@
$source = $source.Replace('    [STAThread]', $integration + "`r`n    [STAThread]")
$source = $source.Replace('            new Application();', '            Environment.SetEnvironmentVariable("HUFU_TSF_SMOKE", "1"); new Application(); NativeHuFu();')
$copy = Join-Path $out 'PainBridgeTests.cs'
[IO.File]::WriteAllText($copy, $source, (New-Object Text.UTF8Encoding($true)))
$framework = "$env:WINDIR\Microsoft.NET\Framework\v4.0.30319"
$exe = Join-Path $out 'PainBridgeTests.exe'
& "$framework\csc.exe" /nologo /optimize+ /platform:$Arch "/out:$exe" `
    "/r:$framework\WPF\PresentationFramework.dll" "/r:$framework\WPF\PresentationCore.dll" `
    "/r:$framework\WPF\WindowsBase.dll" /r:System.Xaml.dll $presenter $copy
if ($LASTEXITCODE -ne 0) { throw 'Pain bridge compilation failed' }
& $exe
if ($LASTEXITCODE -ne 0) { throw 'Pain bridge checks failed' }
foreach ($hash in $hashes) {
    if ((Get-FileHash -LiteralPath $hash.Path).Hash -ne $hash.Hash) { throw 'Pain source changed during test' }
}
$hashes | ConvertTo-Json | Set-Content (Join-Path $out 'pain-source-hashes.json')
