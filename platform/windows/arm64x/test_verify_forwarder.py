#!/usr/bin/env python3
"""Negative structural checks against actual built PE inputs (no Windows load)."""
import argparse
import json
from pathlib import Path
import shutil
import struct
import tempfile
from verify_forwarder import PE, verify_forwarder


def offset(pe, rva):
    table = pe.optional + pe.read("H", pe.pe + 20, pe.raw)
    for index in range(pe.read("H", pe.pe + 6, pe.raw)):
        at = table + index * 40
        _, start, size, raw = struct.unpack_from("<4I", pe.raw, at + 8)
        if start <= rva < start + size:
            return raw + rva - start
    raise ValueError("test mutation RVA is unmapped")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("facade", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    report = verify_forwarder(args.facade)
    records = [{"name": "actual-arm64x", "passed": True}]
    with tempfile.TemporaryDirectory(prefix="hufu-arm64x-structure-") as directory:
        directory = Path(directory)
        facade = directory / "hufu_tsf.dll"
        for implementation in report["implementations"].values():
            shutil.copyfile(args.facade.with_name(implementation["name"]), directory / implementation["name"])
        original = args.facade.read_bytes()
        pe = PE(original)
        config, _ = pe.directory(10)
        section = pe.read("H", config + 228)
        table = pe.sections[section - 1][0] + pe.read("I", config + 224)
        variants = {}
        variants["ordinary-arm64"] = (directory / report["implementations"]["arm64"]["name"]).read_bytes()
        no_metadata = bytearray(original)
        struct.pack_into("<Q", no_metadata, offset(pe, config + 200), 0)
        variants["missing-chpe-metadata"] = no_metadata
        no_marker = bytearray(original)
        struct.pack_into("<Q", no_marker, offset(pe, table + 8), 5)
        variants["wrong-dynamic-relocation-symbol"] = no_marker
        bad_forwarder = bytearray(original)
        module = report["implementations"]["x64"]["name"][:-4].encode()
        at = bad_forwarder.find(module + b".DllGetClassObject")
        assert at >= 0
        bad_forwarder[at:at + len(module)] = b"x" * len(module)
        variants["wrong-x64-export-target"] = bad_forwarder
        variants["truncated-pe"] = original[:256]
        for name, data in variants.items():
            facade.write_bytes(data)
            try:
                verify_forwarder(facade)
            except (ValueError, OSError, struct.error) as error:
                records.append({"name": name, "passed": True, "rejection": str(error)})
            else:
                raise SystemExit(f"Incorrectly accepted {name}")
        facade.write_bytes(original)
        (directory / report["implementations"]["x64"]["name"]).unlink()
        try:
            verify_forwarder(facade)
        except OSError as error:
            records.append({"name": "missing-sidecar", "passed": True, "rejection": str(error)})
        else:
            raise SystemExit("Incorrectly accepted missing sidecar")
    result = {"checks": records, "passed": len(records)}
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
