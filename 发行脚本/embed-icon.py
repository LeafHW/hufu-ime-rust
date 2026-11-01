"""给 PE 嵌入 .ico 作为 RT_GROUP_ICON + RT_ICON 资源（等效 rcedit --set-icon）。"""
import ctypes, ctypes.wintypes as wt, struct, sys, os

k32 = ctypes.WinDLL('kernel32', use_last_error=True)
BeginUpdateResourceW = k32.BeginUpdateResourceW
UpdateResourceW = k32.UpdateResourceW
EndUpdateResourceW = k32.EndUpdateResourceW
# 64 位句柄必须显式声明，否则 restype=c_int 截断句柄 → err=87
BeginUpdateResourceW.restype = ctypes.c_void_p
BeginUpdateResourceW.argtypes = [wt.LPCWSTR, wt.BOOL]
UpdateResourceW.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p, wt.WORD, ctypes.c_void_p, wt.DWORD]
EndUpdateResourceW.argtypes = [ctypes.c_void_p, wt.BOOL]

RT_ICON = 3
RT_GROUP_ICON = 14
LANG_NEUTRAL = 0

def MAKEINTRESOURCE(i):
    return ctypes.c_void_p(i)

def embed(exe_path, ico_path):
    ico = open(ico_path, 'rb').read()
    reserved, ico_type, count = struct.unpack_from('<HHH', ico, 0)
    assert ico_type == 1 and count > 0, 'not a valid .ico'
    entries = []
    images = []
    off = 6
    for i in range(count):
        w, h, colors, _res, planes, bpp, size, img_off = struct.unpack_from('<BBBBHHII', ico, off)
        entries.append((w, h, colors, planes, bpp, size))
        images.append(ico[img_off:img_off + size])
        off += 16

    grp = struct.pack('<HHH', 0, 1, count)
    for i, (w, h, colors, planes, bpp, size) in enumerate(entries, 1):
        w = 0 if w >= 256 else w
        h = 0 if h >= 256 else h
        grp += struct.pack('<BBBBHHIH', w, h, colors, 0, planes, bpp, size, i)

    h = BeginUpdateResourceW(exe_path, False)
    if not h:
        raise OSError('BeginUpdateResource failed err=%d' % ctypes.get_last_error())
    ok = True
    for i, img in enumerate(images, 1):
        # 【关键】RT_ICON 必须用整数 ID（GRPICONDIR.nID 按 MAKEINTRESOURCE 查找）；
        # 传宽字符串名会写成字符串名资源，图标组引用落空 → PrivateExtractIcons 提取 0 个、任务管理器无图标。
        ok = UpdateResourceW(h, MAKEINTRESOURCE(RT_ICON), MAKEINTRESOURCE(i), LANG_NEUTRAL,
                             ctypes.create_string_buffer(img, len(img)), len(img)) and ok
    buf = ctypes.create_string_buffer(grp, len(grp))
    ok = UpdateResourceW(h, MAKEINTRESOURCE(RT_GROUP_ICON), MAKEINTRESOURCE(1), LANG_NEUTRAL, buf, len(grp)) and ok
    if not ok:
        raise OSError('UpdateResource failed err=%d' % ctypes.get_last_error())
    if not EndUpdateResourceW(h, False):
        raise OSError('EndUpdateResource failed err=%d' % ctypes.get_last_error())
    print('embedded %d icon images -> %s' % (count, exe_path))

if __name__ == '__main__':
    embed(sys.argv[1], sys.argv[2])
