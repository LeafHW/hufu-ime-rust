#!/usr/bin/env python3
"""Verify committed wrapper files and fetch the exact, SHA256-pinned llama source."""
import argparse, hashlib, json, shutil, tarfile, urllib.request
from pathlib import Path
ROOT = Path(__file__).resolve().parent

def digest(path):
    h = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''): h.update(block)
    return h.hexdigest()

def main():
    p = argparse.ArgumentParser()
    p.add_argument('--cache', required=True, type=Path)
    p.add_argument('--archive', type=Path)
    a = p.parse_args()
    lock = json.loads((ROOT / 'sources.lock.json').read_text(encoding='utf-8'))
    for f in lock['files']:
        if digest(ROOT / f['path']) != f['sha256']: raise SystemExit('source hash mismatch: ' + f['path'])
    if digest(ROOT / 'licenses/llama.cpp-LICENSE') != lock['llama_license_sha256']: raise SystemExit('llama license hash mismatch')
    a.cache.mkdir(parents=True, exist_ok=True)
    arc = a.archive or a.cache / ('llama-' + lock['llama_commit'] + '.tar.gz')
    if not arc.exists():
        if a.archive: raise SystemExit('explicit archive is missing')
        temp = arc.with_suffix('.download')
        with urllib.request.urlopen(lock['llama_archive']['url'], timeout=120) as r, temp.open('wb') as f: shutil.copyfileobj(r, f)
        if digest(temp) != lock['llama_archive']['sha256']: raise SystemExit('download hash mismatch')
        temp.replace(arc)
    if digest(arc) != lock['llama_archive']['sha256']: raise SystemExit('llama archive hash mismatch')
    target = a.cache / ('llama.cpp-' + lock['llama_commit'])
    # Verify existing sources against every archive member; never reuse a modified cache.
    with tarfile.open(arc, 'r:gz') as tf:
        files = [m for m in tf.getmembers() if m.isfile()]
        for m in tf.getmembers():
            if m.issym() or m.islnk() or '..' in Path(m.name).parts or Path(m.name).is_absolute():
                raise SystemExit('unsafe archive member: ' + m.name)
        expected = {Path(*Path(m.name).parts[1:]) for m in files}
        if target.exists():
            extras = [p for p in target.rglob('*') if p.is_file() and p.relative_to(target) not in expected]
            if extras: raise SystemExit('unexpected cached source: ' + str(extras[0]))
        for m in files:
            rel = Path(*Path(m.name).parts[1:]); dst = target / rel
            data = tf.extractfile(m).read()
            if dst.exists():
                if dst.read_bytes() != data: raise SystemExit('modified cached source: ' + str(dst))
            else:
                dst.parent.mkdir(parents=True, exist_ok=True); dst.write_bytes(data)
    print(str(target.resolve()))
if __name__ == '__main__': main()
