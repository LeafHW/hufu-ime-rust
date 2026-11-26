"""Windows resource-only icon extraction; never executes TSF code."""
import ctypes as C
from ctypes import wintypes as W
import json
from pathlib import Path

root = Path(__file__).resolve().parents[4]
payload = root / '_tmp/arm64/build/payload-95b54d2fba2246e98c97fe7f937d54e8'
kernel = C.WinDLL('kernel32', use_last_error=True)
user = C.WinDLL('user32', use_last_error=True)
kernel.LoadLibraryExW.argtypes = [W.LPCWSTR, W.HANDLE, W.DWORD]
kernel.LoadLibraryExW.restype = W.HMODULE
kernel.FindResourceW.argtypes = [W.HMODULE, C.c_void_p, C.c_void_p]
kernel.FindResourceW.restype = W.HRSRC
kernel.FreeLibrary.argtypes = [W.HMODULE]
user.PrivateExtractIconsW.argtypes = [W.LPCWSTR, C.c_int, C.c_int, C.c_int, C.POINTER(W.HICON), C.POINTER(W.UINT), W.UINT, W.UINT]
user.PrivateExtractIconsW.restype = W.UINT
user.DestroyIcon.argtypes = [W.HICON]
results = []
for arch in ('arm64', 'x64'):
    for path in sorted((payload / arch).iterdir()):
        if not (path.name.startswith('hufu_tsf') or path.name in ('hufu-server.exe', 'hufu-settings.exe', 'hufu-trainer.exe')):
            continue
        module = kernel.LoadLibraryExW(str(path), None, 0x22)
        assert module
        resource = kernel.FindResourceW(module, C.c_void_p(1), C.c_void_p(14))
        error = C.get_last_error() if not resource else 0
        kernel.FreeLibrary(module)
        sizes = []
        for size in (16, 32, 48, 256):
            icon = W.HICON()
            index = W.UINT()
            extracted = user.PrivateExtractIconsW(str(path), 0, size, size, C.byref(icon), C.byref(index), 1, 0)
            sizes.append(dict(size=size, icons_extracted=extracted, valid_hicon=bool(icon)))
            if icon:
                user.DestroyIcon(icon)
        results.append(dict(file=f'{arch}/{path.name}', group_resource_found=bool(resource), resource_error=error, sizes=sizes))
out = Path(__file__).with_name('windows-icon-probe.json')
out.write_text(json.dumps(results, indent=2) + '\n', encoding='utf-8')
print(json.dumps(results, indent=2))
