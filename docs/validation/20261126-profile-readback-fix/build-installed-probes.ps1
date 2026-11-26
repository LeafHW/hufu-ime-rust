$ErrorActionPreference='Stop'
$root='C:\Workspace\hufu-ime-rust\_tmp\arm64\profile-fix'
foreach($arch in @('ARM64','x64','x86')){
 . C:\Workspace\hufu-ime-rust\platform\windows\arm64x\Initialize-Toolchain.ps1 -Architecture $arch
 & cl.exe /nologo /std:c++17 /EHsc /MT /O2 "/Fo$root\inspect-installed-$arch.obj" "/Fe$root\inspect-installed-$arch.exe" "$root\inspect-installed.cpp" ole32.lib uuid.lib psapi.lib /link /MANIFEST:EMBED "/MANIFESTUAC:level='asInvoker' uiAccess='false'"
 if($LASTEXITCODE){exit $LASTEXITCODE}
}
