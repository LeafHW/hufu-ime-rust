#!/usr/bin/env python3
"""Read-only validation of this project's ARM64X pure forwarder, on any OS.

Parses IMAGE_LOAD_CONFIG_DIRECTORY64 and ARM64X dynamic value relocations,
applies the supported VALUE/ZEROFILL fixups to an in-memory PE view, then checks
both real export directories and generation-specific sidecars. Machine AA64
alone is deliberately insufficient. No DLL is loaded and no file is modified.
Definitions: Windows SDK winnt.h; LLVM BinaryFormat/COFF.h and Object/COFF.h.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import struct


class PE:
    def __init__(self, data):
        self.raw = bytes(data)
        if self.raw[:2] != b"MZ":
            raise ValueError("not an MZ image")
        self.pe = self.read("I", 0x3C, self.raw)
        if self.raw[self.pe:self.pe + 4] != b"PE\0\0":
            raise ValueError("not a PE image")
        self.optional = self.pe + 24
        if self.read("H", self.optional, self.raw) != 0x20B:
            raise ValueError("ARM64X/sidecars must use PE32+")
        self.base = self.read("Q", self.optional + 24, self.raw)
        image_size = self.read("I", self.optional + 56, self.raw)
        header_size = self.read("I", self.optional + 60, self.raw)
        if not 0 < header_size <= len(self.raw) or not header_size <= image_size <= 128 * 1024 * 1024:
            raise ValueError("invalid PE image/header size")
        self.image = bytearray(image_size)
        self.image[:header_size] = self.raw[:header_size]
        table = self.optional + self.read("H", self.pe + 20, self.raw)
        self.sections = []
        for index in range(self.read("H", self.pe + 6, self.raw)):
            at = table + index * 40
            if at < 0 or at + 40 > len(self.raw):
                raise ValueError("truncated PE section table")
            virtual_size, rva, size, offset = struct.unpack_from("<4I", self.raw, at + 8)
            if rva + max(size, virtual_size) > image_size or offset + size > len(self.raw):
                raise ValueError("section outside PE image")
            self.image[rva:rva + size] = self.raw[offset:offset + size]
            self.sections.append((rva, max(size, virtual_size)))

    def read(self, fmt, at, data=None):
        data = self.image if data is None else data
        size = struct.calcsize("<" + fmt)
        if at < 0 or at + size > len(data):
            raise ValueError("PE field outside image")
        return struct.unpack_from("<" + fmt, data, at)[0]

    def directory(self, index, image=None):
        image = self.image if image is None else image
        at = self.optional + 112 + index * 8
        rva, size = struct.unpack_from("<II", image, at)
        if not rva or not size or rva + size > len(image):
            raise ValueError(f"missing/invalid PE directory {index}")
        return rva, size

    def string(self, rva, image=None):
        image = self.image if image is None else image
        if not 0 < rva < len(image):
            raise ValueError("string RVA outside image")
        end = image.find(0, rva, min(rva + 4096, len(image)))
        if end < 0:
            raise ValueError("unterminated PE string")
        return bytes(image[rva:end]).decode("ascii")

    def exports(self, image=None, forwarders=False):
        image = self.image if image is None else image
        start, size = self.directory(0, image)
        functions = self.read("I", start + 20, image)
        count = self.read("I", start + 24, image)
        if not 0 < count == functions <= 4096:
            raise ValueError("expected named exports without ordinal-only entries")
        addresses = self.read("I", start + 28, image)
        names = self.read("I", start + 32, image)
        ordinals = self.read("I", start + 36, image)
        result = {}
        for index in range(count):
            name = self.string(self.read("I", names + index * 4, image), image)
            ordinal = self.read("H", ordinals + index * 2, image)
            if ordinal >= functions or name in result:
                raise ValueError("invalid/duplicate export ordinal/name")
            target = self.read("I", addresses + ordinal * 4, image)
            if forwarders:
                if not start <= target < start + size:
                    raise ValueError(f"non-forwarded facade export: {name}")
                result[name] = self.string(target, image)
            else:
                result[name] = target
        return result


def inspect_arm64x(data):
    pe = PE(data)
    if pe.read("H", pe.pe + 4) != 0xAA64:
        raise ValueError("expected native ARM64 ARM64X header")
    config, size = pe.directory(10)
    if size < 232 or pe.read("I", config) < 232:
        raise ValueError("load config lacks hybrid metadata/DVRT fields")
    metadata = pe.read("Q", config + 200) - pe.base
    if metadata <= 0 or pe.read("I", metadata) != 2:
        raise ValueError("missing/unsupported CHPE ARM64X metadata")
    code_map, code_count = pe.read("I", metadata + 4), pe.read("I", metadata + 8)
    if not 1 < code_count < 65536:
        raise ValueError("invalid hybrid code range count")
    code_types = {pe.read("I", code_map + index * 8) & 3 for index in range(code_count)}
    if 0 not in code_types or not code_types.intersection({1, 2}):
        raise ValueError("metadata does not describe both native and emulated code views")
    section = pe.read("H", config + 228)
    offset = pe.read("I", config + 224)
    if not 1 <= section <= len(pe.sections):
        raise ValueError("missing ARM64X dynamic relocation section")
    section_rva, section_size = pe.sections[section - 1]
    if offset + 8 > section_size:
        raise ValueError("dynamic relocation table outside section")
    table = section_rva + offset
    if pe.read("I", table) != 1:
        raise ValueError("unsupported dynamic relocation table version")
    end = table + 8 + pe.read("I", table + 4)
    if end > section_rva + section_size:
        raise ValueError("dynamic relocation table exceeds section")
    cursor = table + 8
    alternate = bytearray(pe.image)
    fixups = []
    while cursor < end:
        if cursor + 12 > end:
            raise ValueError("truncated dynamic relocation record")
        symbol = pe.read("Q", cursor)
        record_end = cursor + 12 + pe.read("I", cursor + 8)
        if record_end > end or record_end <= cursor + 12:
            raise ValueError("invalid dynamic relocation record length")
        cursor += 12
        if symbol != 6:  # IMAGE_DYNAMIC_RELOCATION_ARM64X
            raise ValueError("unexpected dynamic relocation symbol; ARM64X marker required")
        while cursor < record_end:
            page, block_size = pe.read("I", cursor), pe.read("I", cursor + 4)
            block_end = cursor + block_size
            if page % 4096 or block_size < 8 or block_size % 4 or block_end > record_end:
                raise ValueError("invalid ARM64X fixup block")
            cursor += 8
            while cursor < block_end:
                entry = pe.read("H", cursor)
                cursor += 2
                if entry == 0:
                    if any(pe.image[cursor:block_end]):
                        raise ValueError("nonzero data after fixup terminator")
                    cursor = block_end
                    break
                kind, exponent = (entry >> 12) & 3, entry >> 14
                rva, width = page + (entry & 0xFFF), 1 << exponent
                if kind not in (0, 1) or not exponent or rva % width or rva + width > len(alternate):
                    raise ValueError("unsupported/invalid ARM64X fixup (expected VALUE/ZEROFILL)")
                if kind == 1:
                    if cursor + width > block_end:
                        raise ValueError("truncated ARM64X value")
                    value = pe.image[cursor:cursor + width]
                    cursor += width
                else:
                    value = bytes(width)
                alternate[rva:rva + width] = value
                fixups.append({"rva": rva, "size": width, "kind": kind})
    if not fixups or pe.read("H", pe.pe + 4, alternate) != 0x8664:
        raise ValueError("ARM64X fixups do not produce the AMD64 view")
    if pe.directory(0) == pe.directory(0, alternate):
        raise ValueError("ARM64X does not switch export directories")
    return pe, alternate, fixups


def verify_forwarder(path, arm64_impl=None, x64_impl=None):
    """Return a JSON-compatible report or raise ValueError/OSError on rejection."""
    path = Path(path)
    pe, alternate, fixups = inspect_arm64x(path.read_bytes())
    native = pe.exports(forwarders=True)
    emulated = pe.exports(alternate, forwarders=True)
    if native.keys() != emulated.keys():
        raise ValueError("ARM64/x64 facade export sets differ")
    required = {"DllGetClassObject", "DllCanUnloadNow", "DllRegisterServer", "DllUnregisterServer", "hufu_test_skin_dump", "hufu_probe_registration_path"}
    if not required.issubset(native):
        raise ValueError("required COM/diagnostic/render exports missing")
    modules, generation = {}, None
    for arch, exports, expected, machine in (("arm64", native, arm64_impl, 0xAA64), ("x64", emulated, x64_impl, 0x8664)):
        module_names = set()
        for name, target in exports.items():
            module, separator, symbol = target.rpartition(".")
            if not separator or symbol != name:
                raise ValueError("forwarded symbol differs from facade export")
            module_names.add(module)
        if len(module_names) != 1:
            raise ValueError("expected one generation sidecar per architecture")
        module = module_names.pop()
        match = re.fullmatch(rf"hufu_tsf_{arch}_([a-f0-9]{{20}})", module)
        if not match or (generation is not None and match[1] != generation):
            raise ValueError("invalid/mismatched generation sidecar names")
        generation = match[1]
        sidecar = path.with_name(module + ".dll")
        if expected and Path(expected).name.lower() != sidecar.name.lower():
            raise ValueError("forwarder sidecar differs from manifest")
        side_pe = PE(sidecar.read_bytes())
        if side_pe.read("H", side_pe.pe + 4) != machine or side_pe.exports().keys() != exports.keys():
            raise ValueError("sidecar architecture/exports mismatch")
        modules[arch] = {"name": sidecar.name, "sha256": hashlib.sha256(side_pe.raw).hexdigest()}
    content_generation = hashlib.sha256((modules["arm64"]["sha256"] + "\n" + modules["x64"]["sha256"]).encode("ascii")).hexdigest()[:20]
    if generation != content_generation:
        raise ValueError("sidecar generation does not match the two payload hashes")
    return {"arm64x": True, "generation": generation, "chpe_metadata": True,
            "dynamic_relocation_symbol": 6, "fixups": fixups, "exports": sorted(native), "implementations": modules}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("facade", type=Path)
    parser.add_argument("--arm64-impl", type=Path)
    parser.add_argument("--x64-impl", type=Path)
    args = parser.parse_args()
    try:
        print(json.dumps(verify_forwarder(args.facade, args.arm64_impl, args.x64_impl), indent=2))
    except (ValueError, OSError, struct.error) as error:
        parser.exit(1, f"ARM64X verification failed: {error}\n")
