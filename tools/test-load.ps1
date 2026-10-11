param([ValidateSet('x86','x64')][string]$Arch='x86')
$ErrorActionPreference='Stop'
$root=Split-Path $PSScriptRoot -Parent
$out=Join-Path $root ('evidence\'+$Arch)
$src=@'
using System;
using System.Runtime.InteropServices;
[ComImport, Guid("00000001-0000-0000-C000-000000000046"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface Factory { [PreserveSig] int CreateInstance(IntPtr outer, ref Guid iid, out IntPtr obj); [PreserveSig] int LockServer(bool value); }
class LoadTest {
 [DllImport("kernel32", CharSet=CharSet.Unicode, SetLastError=true)] static extern IntPtr LoadLibrary(string path);
 [DllImport("kernel32", CharSet=CharSet.Ansi)] static extern IntPtr GetProcAddress(IntPtr dll,string name);
 [DllImport("kernel32")] static extern bool FreeLibrary(IntPtr dll);
 [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int GetClass(ref Guid cls, ref Guid iid, out IntPtr obj);
 static int Main(string[] args) {
  var dll=LoadLibrary(args[0]); if(dll==IntPtr.Zero)throw new Exception("LoadLibrary "+Marshal.GetLastWin32Error());
  var entry=GetProcAddress(dll,"DllGetClassObject"); if(entry==IntPtr.Zero)throw new Exception("Missing COM export");
  var get=(GetClass)Marshal.GetDelegateForFunctionPointer(entry,typeof(GetClass));
  var cls=new Guid("8f5c2a10-3e77-4b9c-a1d4-9e0b7c2f5a88"); var iid=new Guid("00000001-0000-0000-C000-000000000046");
  IntPtr p; Marshal.ThrowExceptionForHR(get(ref cls,ref iid,out p));
  var f=(Factory)Marshal.GetObjectForIUnknown(p); Marshal.Release(p);
  var tip=new Guid("aa80e7f7-2021-11d2-93e0-0060b067b86e");
  Marshal.ThrowExceptionForHR(f.CreateInstance(IntPtr.Zero,ref tip,out p)); Marshal.Release(p);
  Marshal.FinalReleaseComObject(f); FreeLibrary(dll);
  Console.WriteLine("PASS DLL entry, DllGetClassObject, ITfTextInputProcessor construction/release; no activation/registration");return 0;
 }
}
'@
$file=Join-Path $out 'LoadTest.cs'; [IO.File]::WriteAllText($file,$src)
$exe=Join-Path $out 'LoadTest.exe'
& "$env:WINDIR\Microsoft.NET\Framework\v4.0.30319\csc.exe" /nologo /platform:$Arch "/out:$exe" $file
if($LASTEXITCODE -ne 0){throw 'compile failed'}
& $exe (Join-Path $out 'normal\hufu_tsf.dll')
if($LASTEXITCODE -ne 0){throw 'DLL load test failed'}
