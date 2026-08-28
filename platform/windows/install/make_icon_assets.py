# 从 BGRA bins 生成：① 传统 DIB 条目的 .ico（老加载器兼容）
#                   ② COFF .rsrc 目标文件（DLL 内嵌，mingw ld 可链）
import struct, os

BIN = os.path.join(os.environ['TEMP'], 'hufu-icon-bins')
ICO_SIZES = [16, 32, 48, 256]
RSRC_SIZES = [16, 32, 48]

def load(size):
    with open(os.path.join(BIN, f'{size}.bin'), 'rb') as f:
        return f.read()

def dib(px, size):
    stride = size * 4
    rows = [px[y*stride:(y+1)*stride] for y in range(size)]
    body = b''.join(reversed(rows))  # bottom-up
    and_stride = ((size + 31) // 32) * 4
    andm = b'\x00' * (and_stride * size)
    hdr = struct.pack('<IiiHHIIiiII', 40, size, size*2, 1, 32, 0,
                      len(body)+len(andm), 0, 0, 0, 0)
    return hdr + body + andm

# ── .ico ──
dibs = [(s, dib(load(s), s)) for s in ICO_SIZES]
head = struct.pack('<HHH', 0, 1, len(dibs))
off = 6 + 16*len(dibs)
entries = b''
body = b''
for s, d in dibs:
    b = 0 if s >= 256 else s
    entries += struct.pack('<BBBBHHII', b, b, 0, 0, 1, 32, len(d), off + len(body))
    body += d
ico = head + entries + body
dst = r'E:\DSH-KF\hufu\platform\windows\install\hufu.ico'
open(dst, 'wb').write(ico)
print('ico:', len(ico), '->', dst)

# ── COFF .rsrc ──
data = bytearray()
relocs = []
def emit(b): data.extend(b)
def emit_reloc(): relocs.append(len(data)); emit(struct.pack('<I', 0))

dibs2 = [dib(load(s), s) for s in RSRC_SIZES]
N = len(RSRC_SIZES)
l1_hdr = 0
l1_entries = l1_hdr + 16
off_l2_icon = l1_entries + 16
off_l2_group = off_l2_icon + 16 + N*8
de_base = off_l2_group + 16 + 8
group_de_off = de_base + N*16

emit(struct.pack('<IIHHHH', 0, 0, 0, 0, 0, 2))
emit(struct.pack('<II', 3, 0x80000000 | off_l2_icon))
emit(struct.pack('<II', 14, 0x80000000 | off_l2_group))
assert len(data) == off_l2_icon
emit(struct.pack('<IIHHHH', 0, 0, 0, 0, 0, N))
for i in range(N):
    emit(struct.pack('<II', i+1, de_base + i*16))
assert len(data) == off_l2_group
emit(struct.pack('<IIHHHH', 0, 0, 0, 0, 0, 1))
emit(struct.pack('<II', 1, group_de_off))
assert len(data) == de_base
for i in range(N):
    emit_reloc()
    emit(struct.pack('<I', len(dibs2[i])))
    emit(struct.pack('<II', 0, 0))
assert len(data) == group_de_off
emit_reloc()
emit(struct.pack('<I', 2 + 14*N))
emit(struct.pack('<II', 0, 0))

def align8():
    while len(data) % 8: data.append(0)
icon_data_offs = []
for d in dibs2:
    align8(); icon_data_offs.append(len(data)); emit(d)
align8()
group_data_off = len(data)
grp = struct.pack('<HHH', 0, 1, N)
for i, sz in enumerate(RSRC_SIZES):
    grp += struct.pack('<BBBBHHIH', sz, sz, 0, 0, 1, 32, len(dibs2[i]), i+1)
emit(grp)
while len(data) % 4: data.append(0)

rva_targets = icon_data_offs + [group_data_off]
assert len(rva_targets) == len(relocs)
for off_, tgt in zip(relocs, rva_targets):
    data[off_:off_+4] = struct.pack('<I', tgt)

machine = 0x8664
secdata = bytes(data)
nrel = len(relocs)
sec_raw_off = 60
reloc_off = sec_raw_off + len(secdata)
symtab_off = reloc_off + 10*nrel
hdr = struct.pack('<HHIIIHH', machine, 1, 0, symtab_off, 1, 0, 0)
sechdr = b'.rsrc\x00\x00\x00' + struct.pack('<IIIIIIHHI',
    len(secdata), 0, len(secdata), sec_raw_off, reloc_off, 0, nrel, 0, 0x40000040)
relocblob = b''.join(struct.pack('<IIH', o, 0, 3) for o in relocs)
symbol = b'.rsrc\x00\x00\x00' + struct.pack('<IhhBB', 0, 1, 0, 3, 0)
strtab = struct.pack('<I', 4)
obj = hdr + sechdr + secdata + relocblob + symbol + strtab
dst2 = r'E:\DSH-KF\hufu\platform\windows\hufu-tsf\assets\hufu_rsrc.o'
open(dst2, 'wb').write(obj)
print('obj:', len(obj), 'rsrc:', len(secdata), 'relocs:', nrel, '->', dst2)
