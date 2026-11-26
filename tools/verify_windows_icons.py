#!/usr/bin/env python3
"""Read-only Windows icon content gate; load PEs as data, never execute them."""
import argparse
import ctypes as C
from ctypes import wintypes as W
import hashlib
import json
from pathlib import Path
import struct


def verify(payload, icon, report, inspect_only=False):
    kernel, user, gdi = (C.WinDLL(name, use_last_error=True) for name in ('kernel32', 'user32', 'gdi32'))
    def api(dll, name, result, *args):
        function = getattr(dll, name)
        function.restype, function.argtypes = result, args
        return function
    load = api(kernel, 'LoadLibraryExW', W.HMODULE, W.LPCWSTR, W.HANDLE, W.DWORD)
    free = api(kernel, 'FreeLibrary', W.BOOL, W.HMODULE)
    names_cb = C.WINFUNCTYPE(W.BOOL, W.HMODULE, C.c_void_p, C.c_void_p, W.LPARAM)
    langs_cb = C.WINFUNCTYPE(W.BOOL, W.HMODULE, C.c_void_p, C.c_void_p, W.WORD, W.LPARAM)
    names = api(kernel, 'EnumResourceNamesW', W.BOOL, W.HMODULE, C.c_void_p, names_cb, W.LPARAM)
    languages = api(kernel, 'EnumResourceLanguagesW', W.BOOL, W.HMODULE, C.c_void_p, C.c_void_p, langs_cb, W.LPARAM)
    find = api(kernel, 'FindResourceExW', W.HRSRC, W.HMODULE, C.c_void_p, C.c_void_p, W.WORD)
    size = api(kernel, 'SizeofResource', W.DWORD, W.HMODULE, W.HRSRC)
    read = api(kernel, 'LoadResource', W.HGLOBAL, W.HMODULE, W.HRSRC)
    lock = api(kernel, 'LockResource', C.c_void_p, W.HGLOBAL)
    extract = api(user, 'PrivateExtractIconsW', W.UINT, W.LPCWSTR, C.c_int, C.c_int, C.c_int, C.POINTER(W.HICON), C.POINTER(W.UINT), W.UINT, W.UINT)
    destroy = api(user, 'DestroyIcon', W.BOOL, W.HICON)
    dc_create = api(gdi, 'CreateCompatibleDC', W.HDC, W.HDC)
    dc_delete = api(gdi, 'DeleteDC', W.BOOL, W.HDC)
    dib = api(gdi, 'CreateDIBSection', W.HBITMAP, W.HDC, C.c_void_p, W.UINT, C.POINTER(C.c_void_p), W.HANDLE, W.DWORD)
    select = api(gdi, 'SelectObject', W.HANDLE, W.HDC, W.HANDLE)
    delete = api(gdi, 'DeleteObject', W.BOOL, W.HANDLE)
    draw = api(user, 'DrawIconEx', W.BOOL, W.HDC, C.c_int, C.c_int, W.HICON, C.c_int, C.c_int, W.UINT, W.HBRUSH, W.UINT)
    flush = api(gdi, 'GdiFlush', W.BOOL)

    ico = icon.read_bytes()
    reserved, kind, count = struct.unpack_from('<HHH', ico)
    if reserved or kind != 1 or count < 1:
        raise ValueError('invalid reference ICO')
    expected = []
    for index in range(count):
        entry = struct.unpack_from('<BBBBHHII', ico, 6 + index * 16)
        expected.append(ico[entry[-1]:entry[-1] + entry[-2]])

    def resource(module, kind, name, language):
        item = find(module, kind, name, language)
        if not item:
            raise C.WinError(C.get_last_error())
        memory = read(module, item)
        pointer = lock(memory)
        if not pointer:
            raise C.WinError(C.get_last_error())
        return C.string_at(pointer, size(module, item))

    def pixels(path, n):
        handle, index = W.HICON(), W.UINT()
        if extract(str(path), 0, n, n, C.byref(handle), C.byref(index), 1, 0) != 1 or not handle:
            raise ValueError(f'icon extraction failed: {path} at {n}')
        dc = dc_create(None)
        bitmap = previous = None
        try:
            header = C.create_string_buffer(struct.pack('<IiiHHIIiiII', 40, n, -n, 1, 32, 0, n*n*4, 0, 0, 0, 0))
            bits = C.c_void_p()
            bitmap = dib(dc, header, 0, C.byref(bits), None, 0)
            if not bitmap or not bits:
                raise C.WinError(C.get_last_error())
            previous = select(dc, bitmap)
            C.memset(bits, 0, n*n*4)
            if not draw(dc, 0, 0, handle, n, n, 0, None, 3):
                raise C.WinError(C.get_last_error())
            flush()
            return C.string_at(bits, n*n*4)
        finally:
            if previous: select(dc, previous)
            if bitmap: delete(bitmap)
            dc_delete(dc)
            destroy(handle)

    sizes = (16, 24, 32, 48, 64, 256)
    reference = {n: pixels(icon, n) for n in sizes}
    results = []
    for arch in ('arm64', 'x64'):
        for path in sorted((payload / arch).glob('*')):
            if not (path.name.startswith('hufu_tsf') and path.suffix.lower() == '.dll'):
                continue
            module = load(str(path), None, 0x22)  # AS_DATAFILE | AS_IMAGE_RESOURCE
            if not module:
                raise C.WinError(C.get_last_error())
            groups = []
            errors = []
            try:
                @names_cb
                def each_name(module, kind, name, context):
                    @langs_cb
                    def each_language(module, kind, name, language, context):
                        try:
                            blob = resource(module, 14, name, language)
                            _, kind, number = struct.unpack_from('<HHH', blob)
                            images = []
                            for i in range(number):
                                entry = struct.unpack_from('<BBBBHHIH', blob, 6 + i*14)
                                images.append(resource(module, 3, entry[-1], language))
                            groups.append(dict(group=name if name < 65536 else C.wstring_at(name),
                                               language=language, frames=number,
                                               matches_reference=kind == 1 and images == expected))
                        except Exception as error:
                            errors.append(str(error))
                        return True
                    if not languages(module, 14, name, each_language, 0):
                        errors.append(f'EnumResourceLanguagesW: {C.get_last_error()}')
                    return True
                if not names(module, 14, each_name, 0):
                    errors.append(f'EnumResourceNamesW: {C.get_last_error()}')
            finally:
                free(module)
            rendered = []
            for n in sizes:
                actual = pixels(path, n)
                rendered.append(dict(size=n, matches_reference=actual == reference[n],
                                     bgra_sha256=hashlib.sha256(actual).hexdigest()))
            passed = bool(groups) and not errors and all(g['matches_reference'] for g in groups) and all(r['matches_reference'] for r in rendered)
            results.append(dict(file=f'{arch}/{path.name}', groups=groups, errors=errors, rendered=rendered, passed=passed))
    if not results:
        raise ValueError('no TSF DLLs found')
    result = dict(reference=str(icon), reference_sha256=hashlib.sha256(ico).hexdigest(),
                  files=results, passed=all(r['passed'] for r in results))
    report.parent.mkdir(parents=True, exist_ok=True)
    report.write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(f"TSF icon content: {sum(r['passed'] for r in results)}/{len(results)} files match reference")
    if not result['passed'] and not inspect_only:
        raise SystemExit(1)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('payload', 'icon', 'report'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--inspect-only', action='store_true')
    args = parser.parse_args()
    verify(args.payload, args.icon, args.report, args.inspect_only)
