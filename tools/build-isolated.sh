#!/usr/bin/env bash
set -euo pipefail
root=$(cd -- "$(dirname -- "$0")/.." && pwd)
export CARGO_INCREMENTAL=0
mkdir -p "$root/evidence/build-logs"
cd "$root/platform/windows"
for arch in x86 x64; do
    if [ "$arch" = x86 ]; then target=i686-pc-windows-gnu; else target=x86_64-pc-windows-gnu; fi
    out="$root/evidence/$arch"
    mkdir -p "$out/normal"
    cargo build -p hufu-tsf --target "$target" > "$root/evidence/build-logs/final-$arch-normal.log" 2>&1
    cp "target/$target/debug/hufu_tsf.dll" "$out/normal/hufu_tsf.dll"
    cargo build -p hufu-tsf --target "$target" --features candidate-probe > "$root/evidence/build-logs/final-$arch-probe.log" 2>&1
    cp "target/$target/debug/hufu_tsf.dll" "$out/hufu_tsf.dll"
    cargo test -p hufu-tsf --target "$target" --no-run --message-format=json > "$root/evidence/build-logs/final-$arch-test.jsonl" 2> "$root/evidence/build-logs/final-$arch-test.log"
    python3 - "$root/evidence/build-logs/final-$arch-test.jsonl" "$out/ComTests.exe" <<'PY'
import json,sys,shutil
rows=[json.loads(s) for s in open(sys.argv[1]) if s.startswith('{')]
exe=[r['executable'] for r in rows if r.get('executable') and r.get('profile',{}).get('test')]
assert len(exe)==1,exe
shutil.copy2(exe[0],sys.argv[2])
PY
done
cd "$root/engine"
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=${CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER:-x86_64-w64-mingw32-gcc}
cargo build -p hufu-server --target x86_64-pc-windows-gnu > "$root/evidence/build-logs/final-server.log" 2>&1
cp target/x86_64-pc-windows-gnu/debug/hufu-server.exe "$root/evidence/x64/normal/"
cargo test -p hufu-server --target x86_64-pc-windows-gnu --no-run --message-format=json > "$root/evidence/build-logs/final-server-test.jsonl" 2> "$root/evidence/build-logs/final-server-test.log"
python3 - "$root/evidence/build-logs/final-server-test.jsonl" "$root/evidence/x64/ServerTests.exe" <<'PY'
import json,sys,shutil
rows=[json.loads(s) for s in open(sys.argv[1]) if s.startswith('{')]
exe=[r['executable'] for r in rows if r.get('executable') and r.get('profile',{}).get('test')]
assert len(exe)==1,exe
shutil.copy2(exe[0],sys.argv[2])
PY
