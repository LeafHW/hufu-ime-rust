[CmdletBinding()]
param(
    [ValidateSet('ARM64', 'x64', 'x86')][string[]]$Architectures = @('ARM64', 'x64', 'x86'),
    [Parameter(Mandatory = $true)][string]$TargetDirectory,
    [Parameter(Mandatory = $true)][string]$OutputDirectory,
    [string]$ToolsRoot = 'C:\Workspace\hufu-build-tools',
    [switch]$Offline
)
$ErrorActionPreference = 'Stop'
$workspace = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$TargetDirectory = [IO.Path]::GetFullPath($TargetDirectory)
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
foreach ($arch in $Architectures) {
    . (Join-Path $PSScriptRoot 'Initialize-Toolchain.ps1') -Architecture $arch -ToolsRoot $ToolsRoot
    $triple = @{ ARM64 = 'aarch64-pc-windows-msvc'; x64 = 'x86_64-pc-windows-gnu'; x86 = 'i686-pc-windows-gnu' }[$arch]
    if ($arch -eq 'x86') { Remove-Item Env:HUFU_TSF_REGISTRATION_DLL -ErrorAction SilentlyContinue } else { $env:HUFU_TSF_REGISTRATION_DLL = 'hufu_tsf.dll' }
    $arguments = @('build','--locked','--manifest-path',(Join-Path $workspace 'Cargo.toml'),'-p','hufu-tsf','-p','hufu-tsf-smoke','--release','--target',$triple,'--target-dir',$TargetDirectory)
    if ($Offline) { $arguments += '--offline' }
    $saved = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    & cargo.exe @arguments 2>&1 | ForEach-Object { $_.ToString() } | Tee-Object -FilePath (Join-Path $OutputDirectory "build-$arch.log")
    $code = $LASTEXITCODE
    $ErrorActionPreference = $saved
    if ($code -ne 0) { throw "TSF build failed: $arch ($code)." }
    $destination = Join-Path $OutputDirectory $arch.ToLowerInvariant()
    New-Item -ItemType Directory -Force -Path $destination | Out-Null
    $release = Join-Path $TargetDirectory "$triple\release"
    $dllName = if ($arch -eq 'x86') { 'hufu_tsf32.dll' } else { 'hufu_tsf.dll' }
    Copy-Item (Join-Path $release 'hufu_tsf.dll') (Join-Path $destination $dllName) -Force
    Copy-Item (Join-Path $release 'hufu-tsf-smoke.exe') (Join-Path $destination 'hufu-tsf-smoke.exe') -Force
}
