[CmdletBinding()]
param(
    [ValidateSet('ARM64', 'x64', 'x86')][string]$Architecture = 'ARM64',
    [string]$ToolsRoot = 'C:\Workspace\hufu-build-tools',
    [string]$Toolchain = ''
)
# Dot-source this file. All environment changes apply to this process only.
$ErrorActionPreference = 'Stop'
# Repeated architecture switches must start from one environment baseline;
# otherwise vcvars appends SDK/VS paths until cmd.exe exceeds its line limit.
if (!$env:HUFU_TOOLCHAIN_BASE_PATH) { $env:HUFU_TOOLCHAIN_BASE_PATH = $env:PATH }
$env:PATH = $env:HUFU_TOOLCHAIN_BASE_PATH
foreach ($name in @('INCLUDE','LIB','LIBPATH','__VSCMD_PREINIT_PATH')) { Remove-Item "Env:$name" -ErrorAction SilentlyContinue }
Get-ChildItem Env:VSCMD_* | ForEach-Object { Remove-Item "Env:$($_.Name)" }
$env:RUSTUP_HOME = Join-Path $ToolsRoot 'rustup'
$env:CARGO_HOME = Join-Path $ToolsRoot 'cargo'
$hostArm = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString() -eq 'Arm64'
if (!$Toolchain) { $Toolchain = $env:HUFU_RUST_TOOLCHAIN }
if (!$Toolchain) { $Toolchain = if ($hostArm) { 'stable-aarch64-pc-windows-msvc' } else { 'stable-x86_64-pc-windows-msvc' } }
$env:HUFU_RUST_TOOLCHAIN = $Toolchain
$env:RUSTUP_TOOLCHAIN = $Toolchain
if ($env:CARGO_TARGET_AARCH64_PC_WINDOWS_MSVC_RUSTFLAGS -notmatch 'target-feature=\+crt-static') {
    $env:CARGO_TARGET_AARCH64_PC_WINDOWS_MSVC_RUSTFLAGS = ($env:CARGO_TARGET_AARCH64_PC_WINDOWS_MSVC_RUSTFLAGS + ' -C target-feature=+crt-static').Trim()
}
$rustBin = Join-Path $env:CARGO_HOME 'bin'
if (!(Test-Path (Join-Path $rustBin 'cargo.exe'))) { throw "Missing isolated Rust toolchain: $rustBin" }
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.ARM64 -property installationPath
if (!$vs) { throw 'Visual Studio ARM64/ARM64EC C++ tools are required.' }
$vcvars = Join-Path $vs 'VC\Auxiliary\Build\vcvarsall.bat'
$vcArg = if ($hostArm) { @{ ARM64 = 'arm64'; x64 = 'arm64_x64'; x86 = 'arm64_x86' }[$Architecture] } else { @{ ARM64 = 'amd64_arm64'; x64 = 'amd64'; x86 = 'amd64_x86' }[$Architecture] }
$oldPreference = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
$environment = & cmd.exe /d /s /c "call `"$vcvars`" $vcArg >nul && set"
$vcExit = $LASTEXITCODE
$ErrorActionPreference = $oldPreference
if ($vcExit -ne 0) { throw "vcvarsall failed ($vcExit)." }
foreach ($line in $environment) {
    if ($line -match '^([^=]+)=(.*)$') { [Environment]::SetEnvironmentVariable($matches[1], $matches[2], 'Process') }
}
$env:PATH = "$rustBin;$(Join-Path $vs 'Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin');$env:PATH"
$env:HUFU_RC = (Get-Command rc.exe -ErrorAction Stop).Source
$env:HUFU_VS_ROOT = $vs
$sysroot = & (Join-Path $rustBin 'rustc.exe') --print sysroot
if ($LASTEXITCODE -ne 0) { throw 'Isolated Rust compiler is not ready.' }
$compilerHost = (& (Join-Path $rustBin 'rustc.exe') -vV | Select-String '^host: ').ToString().Substring(6)
$lld = Join-Path $sysroot "lib\rustlib\$compilerHost\bin\rust-lld.exe"
if (!(Test-Path $lld)) { throw "Missing rust-lld for compiler host $compilerHost" }
$env:PATH = "$(Split-Path -Parent $lld);$env:PATH"
$env:HUFU_RUST_LLD = $lld
$env:HUFU_GNU_TOOLS_ROOT = $ToolsRoot
$launcherSource = Join-Path $PSScriptRoot 'gnu-link.cpp'
$launcherDirectory = Join-Path $ToolsRoot 'bin'
$launcher = Join-Path $launcherDirectory 'hufu-gnu-link.exe'
New-Item -ItemType Directory -Force -Path $launcherDirectory | Out-Null
$llvmHost = if ($hostArm) { 'ARM64' } else { 'x64' }
$dlltoolSource = Join-Path $vs "VC\Tools\Llvm\$llvmHost\bin\llvm-dlltool.exe"
if (!(Test-Path $dlltoolSource)) { throw "LLVM dlltool is required for Rust raw-dylib imports: $dlltoolSource" }
$dlltool = Join-Path $launcherDirectory 'dlltool.exe'
if (!(Test-Path $dlltool) -or (Get-FileHash $dlltool).Hash -ne (Get-FileHash $dlltoolSource).Hash) {
    Copy-Item $dlltoolSource $dlltool -Force
}
$env:PATH = "$launcherDirectory;$env:PATH"
$launcherMutex = New-Object System.Threading.Mutex($false, 'Local\HuFuGnuLinkerInitialize')
$launcherLock = $false
try {
    $launcherLock = $launcherMutex.WaitOne(60000)
    if (!$launcherLock) { throw 'Timed out waiting for GNU linker initialization.' }
    if (!(Test-Path $launcher) -or (Get-Item $launcher).LastWriteTimeUtc -lt (Get-Item $launcherSource).LastWriteTimeUtc) {
        New-Item -ItemType Directory -Force -Path $launcherDirectory | Out-Null
        $compilerOutput = & cl.exe /nologo /std:c++17 /EHsc /O2 /MT "/Fe$launcher" "/Fo$(Join-Path $launcherDirectory 'gnu-link.obj')" $launcherSource
        if ($LASTEXITCODE -ne 0) { throw "GNU linker launcher build failed: $compilerOutput" }
    }
} finally {
    if ($launcherLock) { $launcherMutex.ReleaseMutex() }
    $launcherMutex.Dispose()
}
# GNU std ships startup objects, but not MinGW import/unwind libraries.
foreach ($item in @(
    @{ Prefix = 'X86_64'; Triple = 'x86_64-w64-mingw32' },
    @{ Prefix = 'I686'; Triple = 'i686-w64-mingw32' }
)) {
    $library = Join-Path $ToolsRoot "mingw\$($item.Triple)\lib"
    [Environment]::SetEnvironmentVariable("CARGO_TARGET_$($item.Prefix)_PC_WINDOWS_GNU_LINKER", $launcher, 'Process')
    if (Test-Path (Join-Path $library 'libmingw32.a')) {
        $flags = "-C linker-flavor=ld.lld -C link-self-contained=yes -L native=$library"
        [Environment]::SetEnvironmentVariable("CARGO_TARGET_$($item.Prefix)_PC_WINDOWS_GNU_RUSTFLAGS", $flags, 'Process')
    }
}
