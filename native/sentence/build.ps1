param(
 [Parameter(Mandatory=$true)][ValidateSet('x64','arm64')][string]$Architecture,
 [Parameter(Mandatory=$true)][string]$BuildRoot,
 [string]$LlamaArchive='',
 [string]$CMake='',
 [string]$Python='python',
 [string]$Generator='Visual Studio 18 2026',
 [int]$Jobs=4,
 [switch]$Fresh
)
$ErrorActionPreference='Stop'
if(!$CMake) {
 $vswhere="${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
 $vs=& $vswhere -latest -products * -requires Microsoft.Component.MSBuild -property installationPath
 $CMake=Join-Path $vs 'Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe'
}
if(!(Test-Path $CMake)){throw 'CMake unavailable; pass -CMake explicitly.'}
$BuildRoot=[IO.Path]::GetFullPath($BuildRoot)
New-Item -ItemType Directory -Force $BuildRoot|Out-Null
$prepare=@((Join-Path $PSScriptRoot 'prepare.py'),'--cache',(Join-Path $BuildRoot 'sources'))
if($LlamaArchive){$prepare+=@('--archive',$LlamaArchive)}
$llama=& $Python @prepare
if($LASTEXITCODE){throw 'Pinned source preparation failed.'}
$build=Join-Path $BuildRoot $Architecture
$platform=if($Architecture -eq 'arm64'){'ARM64'}else{'x64'}
$configure=@('-S',$PSScriptRoot,'-B',$build,'-G',$Generator,'-A',$platform,"-DHUFU_LLAMA_SOURCE_DIR=$llama")
# The pinned ggml ARM backend requires clang; ClangCL retains the MSVC ABI/CRT.
if($Architecture -eq 'arm64'){$configure+=@('-T','ClangCL')}
if($Fresh){$configure+='--fresh'}
& $CMake @configure
if($LASTEXITCODE){throw 'CMake configure failed.'}
& $CMake --build $build --config Release --target TigerClaw.Sentence.Native hufu-native-score-probe --parallel $Jobs
if($LASTEXITCODE){throw 'Native scorer build failed.'}
$dll=Join-Path $build 'Release\TigerClaw.Sentence.Native.dll'
$probe=Join-Path $build 'Release\hufu-native-score-probe.exe'
$expected=if($Architecture -eq 'arm64'){0xaa64}else{0x8664}
foreach($path in @($dll,$probe)) {
 $bytes=[IO.File]::ReadAllBytes($path);$pe=[BitConverter]::ToInt32($bytes,0x3c)
 if([BitConverter]::ToUInt16($bytes,$pe+4) -ne $expected){throw "Wrong machine: $path"}
}
[ordered]@{schema=1;architecture=$Architecture;dll=$dll;probe=$probe;sha256=(Get-FileHash $dll -Algorithm SHA256).Hash;source_lock_sha256=(Get-FileHash (Join-Path $PSScriptRoot 'sources.lock.json')).Hash;validation='built-only; published-x64 differential and ARM64 scoring required'}|ConvertTo-Json|Set-Content (Join-Path $build 'native-build.json') -Encoding UTF8
Write-Output $dll
