#!/usr/bin/env python3
"""Verify an independently extracted Inno EXE against both payloads and resources.

Run innoextract externally first. This tool never executes the installer or a DLL.
"""
import argparse
import json
from pathlib import Path

import windows_package as package


def verify(extracted, payload, data, inventory, artifact):
    report = dict(schema=1, scope='independent extraction; no installation',
                  artifact=package.check_update_artifact(artifact), architectures={})
    for arch in ('x64', 'arm64'):
        root = extracted / 'tmp/hufu-preflight' / arch
        manifest = package.validate_manifest(root)
        source = payload / arch
        names = {f['path'] for f in manifest['files']} | {package.MANIFEST}
        actual = {p.relative_to(root).as_posix() for p in root.rglob('*') if p.is_file()}
        if names != actual:
            raise ValueError(f'{arch}: extra/missing extracted files: {names ^ actual}')
        for name in sorted(names):
            if package.digest(root / name) != package.digest(source / name):
                raise ValueError(f'{arch}: extracted file differs: {name}')
        report['architectures'][arch] = dict(files=len(names),
            manifest_sha256=package.digest(root / package.MANIFEST),
            version=manifest['version'])
    resource_root = extracted / 'tmp/hufu-pkg'
    resources = json.loads(inventory.read_text(encoding='utf-8'))['resources']
    names = {e['path'] for e in resources}
    actual = {p.relative_to(resource_root).as_posix() for p in resource_root.rglob('*') if p.is_file()}
    if names != actual:
        raise ValueError(f'extracted resources differ: {names ^ actual}')
    for entry in resources:
        rel = package.relative_path(entry['path'])
        file = resource_root / rel
        if file.stat().st_size != entry['size'] or package.digest(file) != entry['sha256']:
            raise ValueError(f'extracted resource checksum mismatch: {rel}')
        if package.digest(data / rel) != entry['sha256']:
            raise ValueError(f'resource source changed: {rel}')
    report['resources'] = dict(files=len(resources), bytes=sum(e['size'] for e in resources),
                               source_hashes_preserved=True)
    report['passed'] = True
    return report


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('extracted', 'payload', 'data', 'inventory', 'artifact', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    result = verify(args.extracted, args.payload, args.data, args.inventory, args.artifact)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(result, ensure_ascii=False))
