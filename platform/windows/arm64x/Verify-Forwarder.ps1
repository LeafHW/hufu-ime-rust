[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$FacadeDirectory,
    [Parameter(Mandatory = $true)][string]$Arm64Probe,
    [Parameter(Mandatory = $true)][string]$X64Probe,
    [Parameter(Mandatory = $true)][string]$X86Probe,
    [Parameter(Mandatory = $true)][string]$X86Dll,
    [Parameter(Mandatory = $true)][string]$OutputDirectory
)
$ErrorActionPreference = 'Stop'
$FacadeDirectory = [IO.Path]::GetFullPath($FacadeDirectory)
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$manifest = Get-Content (Join-Path $FacadeDirectory 'arm64x-manifest.json') -Raw | ConvertFrom-Json
$facade = Join-Path $FacadeDirectory 'hufu_tsf.dll'

function Registry-Snapshot {
    $rows = New-Object 'System.Collections.Generic.List[string]'
    function Walk($key, [string]$prefix) {
        if ($null -eq $key) { $rows.Add("$prefix=<missing>"); return }
        try {
            $rows.Add("$prefix=<key>")
            foreach ($name in @($key.GetValueNames() | Sort-Object)) {
                $value = $key.GetValue($name, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
                $rows.Add("$prefix|$name|$($key.GetValueKind($name))|$($value | ConvertTo-Json -Compress)")
            }
            foreach ($child in @($key.GetSubKeyNames() | Sort-Object)) { Walk ($key.OpenSubKey($child, $false)) "$prefix\$child" }
        } finally { $key.Dispose() }
    }
    foreach ($hive in @([Microsoft.Win32.RegistryHive]::CurrentUser, [Microsoft.Win32.RegistryHive]::LocalMachine)) {
        foreach ($view in @([Microsoft.Win32.RegistryView]::Registry64, [Microsoft.Win32.RegistryView]::Registry32)) {
            $base = [Microsoft.Win32.RegistryKey]::OpenBaseKey($hive, $view)
            try {
                foreach ($path in @('Software\Classes\CLSID\{8F5C2A10-3E77-4B9C-A1D4-9E0B7C2F5A88}', 'Software\Microsoft\CTF\TIP\{8F5C2A10-3E77-4B9C-A1D4-9E0B7C2F5A88}')) {
                    Walk ($base.OpenSubKey($path, $false)) "$hive/$view/$path"
                }
            } finally { $base.Dispose() }
        }
    }
    return @($rows)
}
function Probe([string]$Name, [string]$Executable, [string]$Dll, [bool]$ExpectedSuccess, [string]$ExpectedArch = '', [string]$ExpectedImplementation = '') {
    $stdout = Join-Path $OutputDirectory "$Name.stdout.txt"
    $stderr = Join-Path $OutputDirectory "$Name.stderr.txt"
    $process = New-Object System.Diagnostics.Process
    $process.StartInfo.FileName = $Executable
    $process.StartInfo.Arguments = 'probe-dll "' + $Dll + '"'
    $process.StartInfo.UseShellExecute = $false
    $process.StartInfo.CreateNoWindow = $true
    $process.StartInfo.RedirectStandardOutput = $true
    $process.StartInfo.RedirectStandardError = $true
    $process.StartInfo.StandardOutputEncoding = [Text.Encoding]::UTF8
    $process.StartInfo.StandardErrorEncoding = [Text.Encoding]::UTF8
    if (!$process.Start()) { throw "$Name probe process did not start." }
    $outputTask = $process.StandardOutput.ReadToEndAsync()
    $errorTask = $process.StandardError.ReadToEndAsync()
    if (!$process.WaitForExit(30000)) { $process.Kill(); throw "$Name loader probe timed out." }
    $process.WaitForExit()
    $code = $process.ExitCode
    $text = $outputTask.Result
    [IO.File]::WriteAllText($stdout, $text, [Text.Encoding]::UTF8)
    [IO.File]::WriteAllText($stderr, $errorTask.Result, [Text.Encoding]::UTF8)
    $process.Dispose()
    if (($code -eq 0) -ne $ExpectedSuccess) { throw "$Name unexpected exit $code; see $stderr" }
    if ($ExpectedSuccess) {
        if ($text -notmatch '(?m)^probe_status=ok\r?$' -or $text -notmatch "(?m)^host_arch=$ExpectedArch\r?$") { throw "$Name invalid success/architecture output." }
        $match = [regex]::Match($text, '(?m)^implementation=(.+)\r?$')
        $actual = $match.Groups[1].Value.Trim()
        if (![string]::Equals([IO.Path]::GetFullPath($actual), [IO.Path]::GetFullPath($ExpectedImplementation), [StringComparison]::OrdinalIgnoreCase)) { throw "$Name wrong implementation: $actual" }
    }
    return @{ name = $Name; exit_code = $code; expected_success = $ExpectedSuccess; stdout = (Split-Path -Leaf $stdout); stderr = (Split-Path -Leaf $stderr) }
}
$before = @(Registry-Snapshot)
$before | Set-Content (Join-Path $OutputDirectory 'registry-before.txt') -Encoding UTF8
$armImpl = Join-Path $FacadeDirectory ($manifest.files | Where-Object role -eq 'tsf-arm64-implementation').path
$x64Impl = Join-Path $FacadeDirectory ($manifest.files | Where-Object role -eq 'tsf-x64-implementation').path
$results = @()
$results += Probe 'arm64-facade' $Arm64Probe $facade $true 'aarch64' $armImpl
$results += Probe 'x64-facade' $X64Probe $facade $true 'x86_64' $x64Impl
$results += Probe 'x86-native' $X86Probe $X86Dll $true 'x86' $X86Dll
$results += Probe 'arm64-rejects-x86' $Arm64Probe $X86Dll $false
$results += Probe 'x86-rejects-facade' $X86Probe $facade $false
$missing = Join-Path $OutputDirectory 'missing-implementation'
New-Item -ItemType Directory -Force -Path $missing | Out-Null
Copy-Item $facade (Join-Path $missing 'hufu_tsf.dll') -Force
$results += Probe 'arm64-rejects-missing-implementation' $Arm64Probe (Join-Path $missing 'hufu_tsf.dll') $false
$results += Probe 'x64-rejects-missing-implementation' $X64Probe (Join-Path $missing 'hufu_tsf.dll') $false
$after = @(Registry-Snapshot)
$after | Set-Content (Join-Path $OutputDirectory 'registry-after.txt') -Encoding UTF8
if (@(Compare-Object $before $after).Count) { throw 'HuFu registration changed during read-only probes.' }
$hashes = @($Arm64Probe, $X64Probe, $X86Probe, $X86Dll, $facade, $armImpl, $x64Impl) | ForEach-Object { @{ path = $_; sha256 = (Get-FileHash $_ -Algorithm SHA256).Hash.ToLowerInvariant() } }
@{ schema = 1; registry_unchanged = $true; scope = 'isolated DLL load/export/COM construction only; no registration, activation, server or typing'; results = $results; binaries = $hashes } | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $OutputDirectory 'verification.json') -Encoding UTF8
Write-Output "Passed $($results.Count) loader checks; HuFu HKCU/HKLM 32/64-bit COM/TIP unchanged."
