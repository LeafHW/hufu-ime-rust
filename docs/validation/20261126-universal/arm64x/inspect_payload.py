"""Reproduce final payload PE imports, icon bytes, and embedded UI checks."""
import hashlib
import importlib.util
import json
from pathlib import Path
import struct
import sys

sys.dont_write_bytecode = True
REPO = Path(__file__).resolve().parents[4]
PAYLOAD = REPO / '_tmp/arm64/build/payload-95b54d2fba2246e98c97fe7f937d54e8'
OUT = Path(__file__).resolve().parent


def sha(data):
    return hashlib.sha256(data).hexdigest()


def inspect(path):
    b = path.read_bytes()
    u16 = lambda n: struct.unpack_from('<H', b, n)[0]
    u32 = lambda n: struct.unpack_from('<I', b, n)[0]
    pe = u32(60)
    op = pe + 24
    wide = u16(op) == 0x20b
    dirs = op + (112 if wide else 96)
    table = op + u16(pe + 20)
    sections = [struct.unpack_from('<IIII', b, table + i * 40 + 8) for i in range(u16(pe + 6))]

    def offset(rva):
        if rva < u32(op + 60):
            return rva
        for vs, va, size, at in sections:
            if va <= rva < va + max(vs, size):
                return at + rva - va
        raise ValueError(f'unmapped RVA {rva:x}')

    def string(rva):
        at = offset(rva)
        return b[at:b.index(b'\0', at)].decode('ascii')

    imports, delays = [], []
    for index, step, name_field, result in [(1, 20, 12, imports), (13, 32, 4, delays)]:
        rva, _ = struct.unpack_from('<II', b, dirs + index * 8)
        if not rva:
            continue
        pos = offset(rva)
        for _ in range(1000):
            if not any(b[pos:pos + step]):
                break
            name = u32(pos + name_field)
            if index == 13 and not u32(pos) & 1:
                name -= struct.unpack_from('<Q' if wide else '<I', b, op + (24 if wide else 28))[0]
            result.append(string(name))
            pos += step

    resources = {}
    rva, _ = struct.unpack_from('<II', b, dirs + 16)
    if rva:
        base = offset(rva)

        def walk(at, ids):
            for i in range(u16(base + at + 12) + u16(base + at + 14)):
                key, val = struct.unpack_from('<II', b, base + at + 16 + 8 * i)
                if key & 0x80000000:
                    keyat = base + (key & 0x7fffffff)
                    key = b[keyat + 2:keyat + 2 + 2 * u16(keyat)].decode('utf-16-le')
                current = ids + (key,)
                if val & 0x80000000:
                    walk(val & 0x7fffffff, current)
                else:
                    target, size = struct.unpack_from('<II', b, base + val)
                    resources[current] = b[offset(target):offset(target) + size]
        walk(0, ())
    return dict(file=path.name, sha256=sha(b), machine=hex(u16(pe + 4)), imports=imports,
                delay_imports=delays, resource_types=sorted({key[0] for key in resources})), resources


def ico_images(path):
    data = path.read_bytes()
    images = []
    for i in range(struct.unpack_from('<H', data, 4)[0]):
        size, offset = struct.unpack_from('<II', data, 6 + 16 * i + 8)
        images.append(data[offset:offset + size])
    return sorted(map(sha, images))


def main():
    spec = importlib.util.spec_from_file_location('forwarder', REPO / 'platform/windows/arm64x/verify_forwarder.py')
    forwarder = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(forwarder)
    hybrid = forwarder.verify_forwarder(PAYLOAD / 'arm64/hufu_tsf.dll')
    bridge_matches = {}
    for name in ['hufu_tsf.dll'] + [v['name'] for v in hybrid['implementations'].values()]:
        bridge_matches[name] = sha((PAYLOAD / 'bridge' / name).read_bytes())
        assert bridge_matches[name] == sha((PAYLOAD / 'arm64' / name).read_bytes())
    image_files = {p.name.casefold() for p in (PAYLOAD / 'arm64').glob('*.dll')}
    system_imports = {
        'advapi32.dll', 'bcrypt.dll', 'bcryptprimitives.dll', 'comdlg32.dll',
        'crypt32.dll', 'd2d1.dll', 'd3d11.dll', 'dcomp.dll', 'dwmapi.dll',
        'dwrite.dll', 'dxgi.dll', 'gdi32.dll', 'imm32.dll', 'kernel32.dll',
        'msvcrt.dll', 'ntdll.dll', 'ole32.dll', 'oleacc.dll', 'oleaut32.dll',
        'propsys.dll', 'secur32.dll', 'shell32.dll', 'user32.dll', 'uxtheme.dll',
        'winhttp.dll', 'winmm.dll', 'ws2_32.dll',
    }
    brand = REPO / 'platform/windows/setup/icons/虎符.ico'
    tsf_icon = REPO / 'platform/windows/hufu-tsf/assets/hufu.ico'
    all_pe, icons, ui = {}, {}, {}
    dist = REPO / 'settings-app/dist'
    for arch in ('arm64', 'x64'):
        all_pe[arch] = []
        for path in sorted((PAYLOAD / arch).iterdir()):
            if path.suffix.lower() not in ('.dll', '.exe'):
                continue
            report, resources = inspect(path)
            for dependency in report['imports'] + report['delay_imports']:
                name = dependency.casefold()
                assert name in system_imports or name in image_files or name.startswith(('api-ms-', 'ext-ms-')), (path, dependency)
                assert not name.startswith(('vcruntime', 'msvcp', 'libgcc', 'libwinpthread')), (path, dependency)
            all_pe[arch].append(report)
            needs_icon = path.name in ('hufu-server.exe', 'hufu-settings.exe', 'hufu-trainer.exe') or path.name.startswith('hufu_tsf')
            if needs_icon:
                groups = {key: value for key, value in resources.items() if key[0] == 14}
                images = {key: value for key, value in resources.items() if key[0] == 3}
                assert groups and images, f'missing icon: {path}'
                legacy_group = path.name.startswith('hufu_tsf') and report['machine'] != '0xaa64'
                for group_id, group in groups.items():
                    count = struct.unpack_from('<H', group, 4)[0]
                    # Preserve the old GNU resource bytes for this ARM64 work.
                    # Their declared data length is 44 (rather than 48 for 3
                    # entries). The Windows extraction probe is authoritative
                    # for their existing behavior; do not silently call the
                    # legacy resource format canonical.
                    if legacy_group:
                        continue
                    for i in range(count):
                        icon_id = struct.unpack_from('<H', group, 6 + 14 * i + 12)[0]
                        assert (3, icon_id, *group_id[2:]) in images, (path, icon_id)
                if legacy_group:
                    # Existing GNU COFF has two resource levels. Windows accepts
                    # it (see resource-only probe), and its three images differ
                    # from the six-image icon used by the new SDK rc build.
                    filename = 'hufu_rsrc32.o' if report['machine'] == '0x14c' else 'hufu_rsrc.o'
                    expected = tsf_icon.with_name(filename)
                    assert all(value in expected.read_bytes() for value in images.values())
                else:
                    expected = tsf_icon if path.name.startswith('hufu_tsf') else brand
                    assert sorted(map(sha, images.values())) == ico_images(expected), f'icon bytes differ: {path}'
                icons[f'{arch}/{path.name}'] = dict(groups=len(groups), images=len(images),
                    matches=str(expected.relative_to(REPO)), image_sha256=sorted(map(sha, images.values())),
                    resource_tree_depths=sorted({len(key) for key in images}),
                    group_data_bytes=[len(value) for value in groups.values()], legacy_gnu_resource=legacy_group)
        image = (PAYLOAD / arch / 'hufu-server.exe').read_bytes()
        ui[arch] = {}
        for path in sorted(dist.rglob('*')):
            if not path.is_file() or path.name == 'hufu-ui-build.json':
                continue
            contents = path.read_bytes()
            assert contents in image, f'UI not embedded: {arch}/{path.name}'
            ui[arch][path.relative_to(dist).as_posix()] = sha(contents)
    report = dict(payload=str(PAYLOAD.relative_to(REPO)), hybrid=hybrid, bridge_matches_payload=bridge_matches,
                  pe_images=all_pe, icons=icons, embedded_ui=ui,
                  scope='Static ordinary/delay imports, icon group references and exact image bytes, exact UI bytes; WebView2 runtime remains an external system prerequisite.')
    (OUT / 'payload-inspection.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(dict(pe_images=sum(map(len, all_pe.values())), icon_binaries=len(icons), ui_files_each=len(ui['arm64']), generation=hybrid['generation'])))


if __name__ == '__main__':
    main()
