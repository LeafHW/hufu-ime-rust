[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Arm64Dll,
    [Parameter(Mandatory = $true)][string]$X64Dll,
    [Parameter(Mandatory = $true)][string]$OutputDirectory,
    [string]$ToolsRoot = 'C:\Workspace\hufu-build-tools'
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'Initialize-Toolchain.ps1') -Architecture ARM64 -ToolsRoot $ToolsRoot
$Arm64Dll = [IO.Path]::GetFullPath($Arm64Dll)
$X64Dll = [IO.Path]::GetFullPath($X64Dll)
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$work = Join-Path $OutputDirectory (Join-Path 'forwarder-build' ([Guid]::NewGuid().ToString('N')))
New-Item -ItemType Directory -Force -Path $work | Out-Null

function Run([string]$Executable, [string[]]$Arguments) {
    $saved = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    $lines = & $Executable @Arguments 2>&1
    $code = $LASTEXITCODE
    $ErrorActionPreference = $saved
    $lines | Out-File (Join-Path $work 'build.log') -Append -Encoding utf8
    if ($code -ne 0) { throw "$Executable failed ($code): $($lines -join [Environment]::NewLine)" }
    return $lines
}
function Get-Machine([string]$Path) {
    $b = [IO.File]::ReadAllBytes($Path)
    if ($b.Length -lt 64 -or $b[0] -ne 77 -or $b[1] -ne 90) { throw "Not PE: $Path" }
    $pe = [BitConverter]::ToInt32($b, 60)
    if ($pe -lt 0 -or $pe + 6 -gt $b.Length -or [BitConverter]::ToUInt32($b, $pe) -ne 17744) { throw "Invalid PE: $Path" }
    return [BitConverter]::ToUInt16($b, $pe + 4)
}
function Get-Exports([string]$Path) {
    $lines = Run 'dumpbin.exe' @('/nologo', '/exports', $Path)
    $names = @($lines | ForEach-Object { if ("$_" -match '^\s+\d+\s+[0-9A-Fa-f]+\s+(?:[0-9A-Fa-f]{8}\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*(?:=|\(forwarded|$)') { $matches[1] } } | Sort-Object -Unique)
    if ($names.Count -eq 0) { throw "No named exports: $Path" }
    return $names
}
if ((Get-Machine $Arm64Dll) -ne 0xAA64) { throw 'Arm64Dll must be native ARM64.' }
if ((Get-Machine $X64Dll) -ne 0x8664) { throw 'X64Dll must be AMD64.' }
$armExports = @(Get-Exports $Arm64Dll)
$x64Exports = @(Get-Exports $X64Dll)
if (@(Compare-Object $armExports $x64Exports).Count -ne 0) { throw 'ARM64 and x64 export sets differ.' }
foreach ($required in @('DllGetClassObject','DllCanUnloadNow','DllRegisterServer','DllUnregisterServer','hufu_test_skin_dump','hufu_probe_registration_path')) {
    if ($armExports -notcontains $required) { throw "Missing required export $required" }
}
$armHash = (Get-FileHash $Arm64Dll -Algorithm SHA256).Hash.ToLowerInvariant()
$x64Hash = (Get-FileHash $X64Dll -Algorithm SHA256).Hash.ToLowerInvariant()
$hash = [Security.Cryptography.SHA256]::Create()
try { $buildId = ([BitConverter]::ToString($hash.ComputeHash([Text.Encoding]::ASCII.GetBytes("$armHash`n$x64Hash")))).Replace('-', '').Substring(0, 20).ToLowerInvariant() } finally { $hash.Dispose() }
$armName = "hufu_tsf_arm64_${buildId}"
$x64Name = "hufu_tsf_x64_${buildId}"
$armDef = Join-Path $work 'arm64.def'
$x64Def = Join-Path $work 'x64.def'
[IO.File]::WriteAllLines($armDef, @('EXPORTS') + @($armExports | ForEach-Object { "    $_ = $armName.$_" }), [Text.Encoding]::ASCII)
[IO.File]::WriteAllLines($x64Def, @('EXPORTS') + @($x64Exports | ForEach-Object { "    $_ = $x64Name.$_" }), [Text.Encoding]::ASCII)
Copy-Item $Arm64Dll (Join-Path $OutputDirectory "$armName.dll") -Force
Copy-Item $X64Dll (Join-Path $OutputDirectory "$x64Name.dll") -Force
$empty = Join-Path $work 'empty.cpp'
[IO.File]::WriteAllText($empty, "// ARM64X pure forwarder: intentionally no executable code.`r`n", [Text.Encoding]::ASCII)
$armObj = Join-Path $work 'empty_arm64.obj'
$ecObj = Join-Path $work 'empty_arm64ec.obj'
$armLib = Join-Path $work 'arm64.lib'
$x64Lib = Join-Path $work 'x64.lib'
Run 'cl.exe' @('/nologo','/c',"/Fo$armObj",$empty) | Out-Null
Run 'cl.exe' @('/nologo','/c','/arm64EC',"/Fo$ecObj",$empty) | Out-Null
Run 'lib.exe' @('/nologo','/machine:arm64',"/def:$armDef","/out:$armLib") | Out-Null
Run 'lib.exe' @('/nologo','/machine:x64',"/def:$x64Def","/out:$x64Lib") | Out-Null
$icon = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\setup\icons\虎符.ico'))
$rc = Join-Path $work 'facade.rc'
$res = Join-Path $work 'facade.res'
[IO.File]::WriteAllText($rc, "1 ICON `"$($icon.Replace('\','\\'))`"`r`n", (New-Object Text.UTF8Encoding($false)))
Run $env:HUFU_RC @('/nologo','/c65001','/fo',$res,$rc) | Out-Null
$facade = Join-Path $OutputDirectory 'hufu_tsf.dll'
Run 'link.exe' @('/nologo','/dll','/noentry','/machine:arm64x','/Brepro',"/defArm64Native:$armDef","/def:$x64Def",$armObj,$ecObj,"/out:$facade",$armLib,$x64Lib,$res) | Out-Null
# dumpbin sees the image's default export view; runtime probes additionally
# resolve the full named-export set in both native ARM64 and emulated x64 hosts.
$facadeExports = @(Get-Exports $facade)
if (@(Compare-Object $armExports $facadeExports).Count -ne 0) { throw 'Facade export set differs from implementation.' }
$files = @(
    @{ role = 'tsf-facade'; path = 'hufu_tsf.dll' },
    @{ role = 'tsf-arm64-implementation'; path = "$armName.dll" },
    @{ role = 'tsf-x64-implementation'; path = "$x64Name.dll" }
) | ForEach-Object {
    $path = Join-Path $OutputDirectory $_.path
    $_.sha256 = (Get-FileHash $path -Algorithm SHA256).Hash.ToLowerInvariant()
    $_.size = (Get-Item $path).Length
    $_.machine = ('0x{0:X4}' -f (Get-Machine $path))
    $_
}
@{ schema = 1; build_id = $buildId; exports = $armExports; files = @($files) } | ConvertTo-Json -Depth 6 | Set-Content (Join-Path $OutputDirectory 'arm64x-manifest.json') -Encoding UTF8
Write-Output "ARM64X facade: $facade"
Write-Output "Generation: $buildId; exports: $($armExports.Count)"
