#!/usr/bin/env python3
"""Stage pinned Fedora GNU libraries; produce a portable deterministic ZIP.

Prepare on Fedora 44 aarch64 with the exact packages in gnu-libraries.lock.json.
The resulting ZIP works on either x64 or ARM64 Windows build hosts. No compiler
executables from Linux are copied. --verify works on Windows with only Python.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import zipfile


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def verify(root):
    manifest = json.loads((root / "provenance.json").read_text())
    for item in manifest["files"]:
        target = root / item["path"]
        if not target.is_file() or digest(target) != item["sha256"]:
            raise SystemExit(f"GNU library verification failed: {target}")
    print(f"Verified {len(manifest['files'])} GNU library/license files")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tools-root", required=True, type=Path)
    parser.add_argument("--archive", type=Path)
    parser.add_argument("--verify", action="store_true")
    args = parser.parse_args()
    root = args.tools_root / "mingw"
    if args.verify:
        verify(root)
        return
    lock_path = Path(__file__).with_name("gnu-libraries.lock.json")
    lock = json.loads(lock_path.read_text())
    packages = lock["packages"]
    actual = subprocess.check_output(["rpm", "-q", *packages], text=True).splitlines()
    if actual != packages:
        raise SystemExit("Installed Fedora package identities differ from the lock")
    # Signed distro package metadata checks every installed input before copying.
    subprocess.run(["rpm", "-V", *packages], check=True)
    owners = set(packages)
    inputs = []
    for triple in ("x86_64-w64-mingw32", "i686-w64-mingw32"):
        for base in (Path("/usr") / triple / "sys-root/mingw/lib",
                     Path("/usr/lib/gcc") / triple / lock["gcc_directory"]):
            inputs += [(path, Path(triple) / "lib" / path.name)
                       for path in sorted(base.iterdir())
                       if path.is_file() and path.suffix in (".a", ".o")]
    input_owners = subprocess.check_output(
        ["rpm", "-qf", *[str(path) for path, _ in inputs]], text=True).splitlines()
    if not set(input_owners).issubset(owners):
        raise SystemExit("Unexpected package owns a selected GNU library")
    for package in packages:
        name = package.split("-13.")[0].split("-16.")[0]
        licenses = Path("/usr/share/licenses") / name
        if licenses.is_dir():
            inputs += [(path, Path("licenses") / name / path.relative_to(licenses))
                       for path in sorted(licenses.rglob("*")) if path.is_file()]
    records = {}
    for source, relative in inputs:
        target = root / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
        records[relative.as_posix()] = {"path": relative.as_posix(),
                                      "source": str(source), "sha256": digest(target)}
    manifest = {"schema": 1, "lock_sha256": digest(lock_path), "packages": packages,
                "files": [records[name] for name in sorted(records)]}
    (root / "provenance.json").write_text(json.dumps(manifest, indent=2) + "\n")
    verify(root)
    if args.archive:
        args.archive.parent.mkdir(parents=True, exist_ok=True)
        with zipfile.ZipFile(args.archive, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
            for relative in sorted(["provenance.json", *records]):
                info = zipfile.ZipInfo("mingw/" + relative, (2026, 11, 26, 0, 0, 0))
                info.compress_type = zipfile.ZIP_DEFLATED
                info.external_attr = 0o644 << 16
                archive.writestr(info, (root / relative).read_bytes())
        print(f"{digest(args.archive)}  {args.archive}")


if __name__ == "__main__":
    main()
