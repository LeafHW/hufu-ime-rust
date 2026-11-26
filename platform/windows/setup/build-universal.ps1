[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$Version,
    [Parameter(Mandatory=$true)][string]$DataSource,
    [string]$ToolsRoot = 'C:\Workspace\hufu-build-tools',
    [string]$BuildRoot = '',
    [string]$NativeBuildRoot = '',
    [string]$DistDir = '',
    [string]$Iscc = '',
    [string]$Python = 'python',
    [string]$Npm = 'npm.cmd',
    [switch]$UseExistingUi,
    [switch]$BuildOnly,
    [int]$Jobs = 4
)
# Builds only. Never invokes install, regsvr32, publishing, or an installed backend.
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..'))
if (!$BuildRoot) { $BuildRoot = Join-Path $repo '_tmp\universal-build' }
$BuildRoot = [IO.Path]::GetFullPath($BuildRoot)
if (!$NativeBuildRoot) { $NativeBuildRoot = Join-Path $BuildRoot 'native' }
if (!$DistDir) { $DistDir = Join-Path $BuildRoot 'dist' }
$DataSource = [IO.Path]::GetFullPath($DataSource)
$initialize = Join-Path $repo 'platform\windows\arm64x\Initialize-Toolchain.ps1'
$packageTool = Join-Path $repo 'tools\windows_package.py'
$cargoTarget = Join-Path $BuildRoot 'target'
$runRoot = Join-Path $BuildRoot ('payload-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $runRoot | Out-Null

function Invoke-Checked([string]$Executable, [string[]]$Arguments) {
    $saved = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    $global:LASTEXITCODE = $null
    # A GUI-subsystem EXE returns immediately when invoked without a pipeline in
    # Windows PowerShell. Out-Host forces a wait and a fresh LASTEXITCODE.
    try { & $Executable @Arguments | Out-Host; $code = $global:LASTEXITCODE }
    finally { $ErrorActionPreference = $saved }
    if ($null -eq $code -or $code -ne 0) { throw "$Executable failed ($code): $($Arguments -join ' ')" }
}
function Build-Rust([string]$Manifest, [string]$Target, [string[]]$Packages) {
    Push-Location (Split-Path -Parent $Manifest)
    try {
        $arguments = @('build','--locked','--release','--target',$Target,'--target-dir',$cargoTarget,'--jobs',"$Jobs",'--manifest-path',$Manifest)
        foreach ($package in $Packages) { $arguments += @('-p',$package) }
        Invoke-Checked 'cargo.exe' $arguments
    } finally { Pop-Location }
}
function Copy-Binary([string]$Target, [string]$Name, [string]$Destination, [string]$Rename = '') {
    if (!$Rename) { $Rename = $Name }
    Copy-Item -LiteralPath (Join-Path $cargoTarget "$Target\release\$Name") -Destination (Join-Path $Destination $Rename)
}

$ui = Join-Path $repo 'settings-app'
if ($UseExistingUi) { Invoke-Checked $Python @($packageTool,'ui-verify',$ui) }
else { Invoke-Checked $Python @($packageTool,'ui-build',$ui,'--npm',$Npm) }
$saved = @{}
foreach ($name in @('CARGO_PROFILE_RELEASE_DEBUG','CARGO_PROFILE_RELEASE_STRIP','CARGO_TARGET_DIR','HUFU_TSF_REGISTRATION_DLL')) {
    $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
try {
    $env:CARGO_PROFILE_RELEASE_DEBUG = '0'
    $env:CARGO_PROFILE_RELEASE_STRIP = 'symbols'
    $env:CARGO_TARGET_DIR = $cargoTarget
    $embedIcon = Join-Path $repo '发行脚本\embed-icon.py'
    $brandIcon = Join-Path $PSScriptRoot 'icons\虎符.ico'
    foreach ($arch in @('x64','arm64')) {
        $env:HUFU_TSF_REGISTRATION_DLL = 'hufu_tsf.dll'
        $vsArch = if ($arch -eq 'arm64') { 'ARM64' } else { 'x64' }
        . $initialize -Architecture $vsArch -ToolsRoot $ToolsRoot
        $target = if ($arch -eq 'arm64') { 'aarch64-pc-windows-msvc' } else { 'x86_64-pc-windows-gnu' }
        Build-Rust (Join-Path $repo 'platform\windows\Cargo.toml') $target @('hufu-tsf','hufu-tsf-smoke','hufu-setup-engine')
        Build-Rust (Join-Path $repo 'engine\Cargo.toml') $target @('hufu-server','hufu-render-helper')
        $payload = Join-Path $runRoot $arch
        New-Item -ItemType Directory -Path $payload | Out-Null
        foreach ($file in @('hufu_tsf.dll','hufu-tsf-smoke.exe','hufu-setup-engine.exe','hufu-server.exe','hufu-render-helper.exe')) {
            Copy-Binary $target $file $payload
        }
        # GNU builds still link the historical checked-in resource object.
        # Apply current branding to the plain DLL BEFORE the bridge hashes and
        # names its sidecars. Never rewrite resources in a linked ARM64X image.
        if ($arch -eq 'x64') {
            Invoke-Checked $Python @($embedIcon,(Join-Path $payload 'hufu_tsf.dll'),$brandIcon)
        }
        & (Join-Path $repo 'native\sentence\build.ps1') -Architecture $arch -BuildRoot $NativeBuildRoot -Jobs $Jobs -Python $Python
        if (!$?) { throw "Native scorer build failed: $arch" }
        Copy-Item (Join-Path $NativeBuildRoot "$arch\Release\TigerClaw.Sentence.Native.dll") $payload
    }
    . $initialize -Architecture x64 -ToolsRoot $ToolsRoot
    Build-Rust (Join-Path $repo 'platform\windows\hufu-settings\Cargo.toml') 'x86_64-pc-windows-gnu' @('hufu-settings')
    Build-Rust (Join-Path $repo 'trainer\Cargo.toml') 'x86_64-pc-windows-gnu' @('gale-app')
    $shared = Join-Path $runRoot 'shared'
    New-Item -ItemType Directory -Path $shared | Out-Null
    foreach ($file in @('hufu-settings.exe','hufu-trainer.exe')) {
        Copy-Binary 'x86_64-pc-windows-gnu' $file $shared
        Invoke-Checked $Python @($embedIcon,(Join-Path $shared $file),$brandIcon)
    }
    . $initialize -Architecture x86 -ToolsRoot $ToolsRoot
    Remove-Item Env:HUFU_TSF_REGISTRATION_DLL -ErrorAction SilentlyContinue
    Build-Rust (Join-Path $repo 'platform\windows\Cargo.toml') 'i686-pc-windows-gnu' @('hufu-tsf','hufu-tsf-smoke')
    $bridge = Join-Path $runRoot 'bridge'
    & (Join-Path $repo 'platform\windows\arm64x\Build-Forwarder.ps1') `
        -Arm64Dll (Join-Path $runRoot 'arm64\hufu_tsf.dll') -X64Dll (Join-Path $runRoot 'x64\hufu_tsf.dll') `
        -OutputDirectory $bridge -ToolsRoot $ToolsRoot
    if (!$?) { throw 'ARM64X forwarder build failed.' }
    Copy-Item (Join-Path $bridge '*.dll') (Join-Path $runRoot 'arm64') -Force
    Copy-Item (Join-Path $runRoot 'x64\hufu-tsf-smoke.exe') (Join-Path $runRoot 'arm64\hufu-tsf-smoke-x64.exe')
    foreach ($arch in @('x64','arm64')) {
        $payload = Join-Path $runRoot $arch
        Copy-Binary 'i686-pc-windows-gnu' 'hufu_tsf.dll' $payload 'hufu_tsf32.dll'
        Invoke-Checked $Python @($embedIcon,(Join-Path $payload 'hufu_tsf32.dll'),$brandIcon)
        Copy-Binary 'i686-pc-windows-gnu' 'hufu-tsf-smoke.exe' $payload 'hufu-tsf-smoke32.exe'
        foreach ($file in @('hufu-settings.exe','hufu-trainer.exe')) { Copy-Item (Join-Path $shared $file) $payload }
        Invoke-Checked $Python @($embedIcon,(Join-Path $payload 'hufu-server.exe'),$brandIcon)
        Invoke-Checked $Python @($packageTool,'ui-binary',(Join-Path $payload 'hufu-server.exe'),$ui)
        Copy-Item (Join-Path $repo 'platform\windows\hufu-settings\vendor\WebView2Loader.dll') $payload
        Copy-Item (Join-Path $PSScriptRoot 'icons\虎符.ico') (Join-Path $payload '图标.ico')
        [IO.File]::WriteAllText((Join-Path $payload 'version.txt'), "$Version`r`n", [Text.Encoding]::ASCII)
        Copy-Item (Join-Path $repo '发行脚本\更新日志.txt') $payload
        Copy-Item (Join-Path $PSScriptRoot 'helper\使用说明-setup版.txt') (Join-Path $payload '使用说明.txt')
        if (Test-Path (Join-Path $DataSource 'licenses')) { Copy-Item (Join-Path $DataSource 'licenses') $payload -Recurse }
        $licenses = Join-Path $payload 'licenses'
        New-Item -ItemType Directory -Force -Path $licenses | Out-Null
        Copy-Item (Join-Path $repo 'LICENSE') (Join-Path $licenses 'hufu-LICENSE.txt')
        Copy-Item (Join-Path $repo 'native\sentence\licenses') (Join-Path $licenses 'native-sentence') -Recurse
        $trainerLicenses = Join-Path $licenses 'trainer'
        New-Item -ItemType Directory -Force -Path $trainerLicenses | Out-Null
        Copy-Item (Join-Path $repo 'trainer\licenses\*') $trainerLicenses -Force
        Copy-Item (Join-Path $repo 'trainer\drills\NOTICE.md') $trainerLicenses -Force
        Invoke-Checked $Python @($packageTool,'manifest',$payload,'--arch',$arch,'--version',$Version)
    }
    # Check actual RT_ICON contents and Windows-rendered pixels, not merely
    # whether an HICON can be returned (the stale white mark also passed that).
    Invoke-Checked $Python @((Join-Path $repo 'tools\verify_windows_icons.py'),'--payload',$runRoot,'--icon',$brandIcon,'--report',(Join-Path $runRoot 'icon-verification.json'))
    # This host can only validate its OS payload; both packages still get static validation.
    $osArch = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
    $nativePayload = Join-Path $runRoot $osArch
    Invoke-Checked (Join-Path $nativePayload 'hufu-setup-engine.exe') @('preflight','--src',$nativePayload,'--log',(Join-Path $runRoot 'preflight.log'))
    [IO.File]::WriteAllText((Join-Path $BuildRoot 'latest-payload.txt'), $runRoot, [Text.Encoding]::UTF8)
    if (!$BuildOnly) {
        & (Join-Path $PSScriptRoot 'build-universal-setup.ps1') -Version $Version -PayloadRoot $runRoot -DataSource $DataSource -DistDir $DistDir -Iscc $Iscc -Python $Python
        if (!$?) { throw 'Universal EXE packaging failed.' }
    }
    Write-Output "Validated payload: $runRoot"
} finally {
    foreach ($name in $saved.Keys) { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
}
