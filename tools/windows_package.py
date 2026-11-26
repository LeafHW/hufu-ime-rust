#!/usr/bin/env python3
"""Windows 通用 EXE 的清单、内容去重和发布门禁；不安装或注册输入法。"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import re
import shutil
import struct
import subprocess
from pathlib import Path, PureWindowsPath

UPDATE_LIMIT = 64 * 1024 * 1024  # 已发布客户端的硬上限，不能只修改新客户端。
MANIFEST = "package-architecture.json"
MACHINES = {"x86": 0x14C, "x64": 0x8664, "arm64": 0xAA64, "arm64x": 0xAA64}
ROLES = {
    "hufu_tsf.dll": "tsf-native", "hufu_tsf32.dll": "tsf-x86",
    "hufu-tsf-smoke.exe": "registration-native",
    "hufu-tsf-smoke32.exe": "registration-x86",
    "hufu-tsf-smoke-x64.exe": "probe-x64",
    "hufu-render-helper.exe": "render-helper", "图标.ico": "tsf-resource",
}
REQUIRED = {
    "hufu-server.exe", "hufu-settings.exe", "WebView2Loader.dll",
    "hufu-trainer.exe", "hufu_tsf.dll", "hufu_tsf32.dll",
    "hufu-tsf-smoke.exe", "hufu-tsf-smoke32.exe", "hufu-render-helper.exe",
    "hufu-setup-engine.exe", "TigerClaw.Sentence.Native.dll", "图标.ico",
}


def verify_hybrid(path: Path) -> dict:
    source = Path(__file__).resolve().parents[1] / 'platform/windows/arm64x/verify_forwarder.py'
    spec = importlib.util.spec_from_file_location('hufu_verify_forwarder', source)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.verify_forwarder(path)


def digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def relative_path(value: str) -> str:
    p = PureWindowsPath(value)
    reserved = re.compile(r'^(?:CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])$', re.I)
    if (not value or p.is_absolute() or p.drive or p.root or any(x in ("", ".", "..") for x in value.replace("\\", "/").split("/"))
            or any(c in value for c in ':*?"<>|\r\n;') or "{" in value or "}" in value
            or any(ord(c) < 32 for c in value)
            or any(x.endswith((".", " ")) or reserved.fullmatch(x.split('.')[0]) for x in p.parts)):
        raise ValueError(f"不安全的包内路径：{value!r}")
    return p.as_posix()


def pe_machine(path: Path) -> int:
    with path.open("rb") as f:
        header = f.read(64)
        if len(header) != 64 or header[:2] != b"MZ":
            raise ValueError(f"不是 PE：{path}")
        f.seek(struct.unpack_from("<I", header, 60)[0])
        header = f.read(6)
        if len(header) != 6 or header[:4] != b"PE\0\0":
            raise ValueError(f"无效的 PE 头：{path}")
        return struct.unpack_from("<H", header, 4)[0]


def expected_machine(name: str, arch: str) -> str:
    if Path(name).suffix.lower() not in (".dll", ".exe"):
        return "data"
    if name in ("hufu_tsf32.dll", "hufu-tsf-smoke32.exe"):
        return "x86"
    if name in ("hufu-settings.exe", "WebView2Loader.dll", "hufu-trainer.exe", "hufu-tsf-smoke-x64.exe"):
        return "x64"
    if arch == "arm64" and name == "hufu_tsf.dll":
        return "arm64x"
    if name.startswith("hufu_tsf_x64_"):
        return "x64"
    return arch


def validate_manifest(root: Path, version: str | None = None) -> dict:
    manifest = json.loads((root / MANIFEST).read_text(encoding="utf-8-sig"))
    arch = manifest.get("system_architecture")
    if manifest.get("schema") != 1 or arch not in ("arm64", "x64"):
        raise ValueError(f"不支持的架构清单：{root}")
    if version is not None and manifest.get("version") != version:
        raise ValueError(f"清单版本不符：{root}")
    seen: set[str] = set()
    for entry in manifest["files"]:
        rel = relative_path(entry["path"])
        if rel.casefold() in seen:
            raise ValueError(f"清单路径重复：{rel}")
        seen.add(rel.casefold())
        path = root / rel
        if path.is_symlink() or not path.resolve().is_relative_to(root.resolve()):
            raise ValueError(f"清单路径越界：{rel}")
        if digest(path) != entry["sha256"]:
            raise ValueError(f"SHA256 不符：{path}")
        machine = expected_machine(rel, arch)
        if entry["machine"] != machine:
            raise ValueError(f"声明架构与组件角色不符：{path}")
        if machine != "data" and pe_machine(path) != MACHINES[machine]:
            raise ValueError(f"PE 架构不符：{path}")
        expected_role = ROLES.get(rel, "tsf-native-dependency" if rel.startswith("hufu_tsf_") else "program")
        if entry["role"] != expected_role:
            raise ValueError(f"组件角色不符：{path}")
    needed = REQUIRED | ({"hufu-tsf-smoke-x64.exe"} if arch == "arm64" else set())
    if not {x.casefold() for x in needed}.issubset(seen):
        raise ValueError(f"缺少必须组件：{root}")
    if arch == "arm64":
        for prefix in ("hufu_tsf_arm64_", "hufu_tsf_x64_"):
            if sum(x.startswith(prefix) and x.endswith(".dll") for x in seen) != 1:
                raise ValueError(f"ARM64X 缺少唯一实现 DLL：{prefix}")
        # AA64 alone also describes ordinary ARM64 DLLs, which fail in x64 hosts.
        verify_hybrid(root / 'hufu_tsf.dll')
    return manifest


def make_manifest(root: Path, arch: str, version: str) -> dict:
    if arch not in ("arm64", "x64") or not re.fullmatch(r"\d+\.\d+\.\d+(\.\d+)?", version):
        raise ValueError("架构或版本格式不符")
    files = []
    for path in sorted(root.rglob("*")):
        if not path.is_file() or path.name == MANIFEST:
            continue
        rel = relative_path(path.relative_to(root).as_posix())
        machine = expected_machine(rel, arch)
        role = ROLES.get(rel, "tsf-native-dependency" if rel.startswith("hufu_tsf_") else "program")
        files.append(dict(path=rel, machine=machine, sha256=digest(path), role=role))
    data = dict(schema=1, system_architecture=arch, version=version, files=files)
    (root / MANIFEST).write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return validate_manifest(root, version)


def write_text(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"\xef\xbb\xbf" + text.replace("\r\n", "\n").replace("\n", "\r\n").encode("utf-8"))


def stage(payload: Path, data: Path, out: Path, version: str) -> dict:
    # 每次使用独立空目录，避免沿用旧产物，且不递归删除调用者目录。
    if out.exists() and any(out.iterdir()):
        raise ValueError(f"staging 必须为空：{out}")
    manifests = {arch: validate_manifest(payload / arch, version) for arch in ("x64", "arm64")}
    out.mkdir(parents=True, exist_ok=True)
    objects = out / "objects"
    objects.mkdir()
    programs: list[str] = []
    names: set[str] = set()
    all_bytes = 0

    def store(path: Path) -> str:
        nonlocal all_bytes
        sha = digest(path)
        obj = objects / sha
        all_bytes += path.stat().st_size
        if not obj.exists():
            shutil.copyfile(path, obj)
        return "staging\\objects\\" + sha

    for arch, manifest in manifests.items():
        predicate = "IsArm64System" if arch == "arm64" else "IsX64System"
        entries = [e["path"] for e in manifest["files"]] + [MANIFEST]
        for rel in entries:
            path = PureWindowsPath(rel)
            source = store(payload / arch / relative_path(rel))
            subdir = "" if str(path.parent) == "." else "\\" + str(path.parent)
            programs.append(f'Source: "{source}"; DestDir: "{{app}}{subdir}"; DestName: "{path.name}"; Check: {predicate}; Flags: ignoreversion')
            programs.append(f'Source: "{source}"; DestDir: "{{tmp}}\\hufu-preflight\\{arch}{subdir}"; DestName: "{path.name}"; Flags: dontcopy')
            names.add(str(path))
    write_text(out / "staging-programs.inc", "\n".join(programs) + "\n")
    write_text(out / "program-files.lst", "\n".join(sorted(names)) + "\n")

    fresh, upgrade, data_inc = [], [], []
    resource_entries = []
    for dirname in ("码表", "数据"):
        if not (data / dirname).is_dir():
            raise ValueError(f"缺少资源目录：{data / dirname}")
        for file in sorted((data / dirname).rglob("*")):
            if not file.is_file():
                continue
            rel = relative_path(file.relative_to(data).as_posix())
            if file.name in ("用户调整.txt", "adj-audit.log", "user-adjust.log") or file.name.startswith("导出 -"):
                raise ValueError(f"资源源含用户数据，禁止打包：{rel}")
            path = PureWindowsPath(rel)
            source = store(file)
            data_inc.append(f'Source: "{source}"; DestDir: "{{tmp}}\\hufu-pkg\\{path.parent}"; DestName: "{path.name}"; Flags: ignoreversion deleteafterinstall')
            frozen = rel.startswith(("码表/", "数据/皮肤/")) or rel == "数据/config.json"
            fresh.append(("!" if frozen else "") + str(path))
            if dirname == "数据":
                upgrade.append("!" + str(path))
            resource_entries.append(dict(path=rel, sha256=digest(file), size=file.stat().st_size))
    if not fresh:
        raise ValueError("资源清单为空")
    write_text(out / "hufu-files.lst", "\n".join(fresh) + "\n")
    write_text(out / "hufu-files-upgrade.lst", "\n".join(upgrade) + "\n")
    write_text(out / "staging-files.inc", "\n".join(data_inc) + "\n")
    shutil.copyfile(payload / "x64" / "图标.ico", out / "图标.ico")
    report = dict(schema=1, version=version, architectures=list(manifests), resources=resource_entries,
                  expanded_bytes=all_bytes, unique_bytes=sum(p.stat().st_size for p in objects.iterdir()),
                  objects=len(list(objects.iterdir())), validation="static-only")
    (out / "inventory.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return report


def check_update_artifact(path: Path) -> dict:
    size = path.stat().st_size
    if not 0 < size <= UPDATE_LIMIT:
        raise ValueError(f"旧版更新器仅接受 1..{UPDATE_LIMIT} 字节，实际 {size}：{path}")
    return dict(file=path.name, size=size, sha256=digest(path))


def ui_fingerprint(root: Path) -> dict:
    sources = [p for p in root.rglob('*') if p.is_file() and
               not {'node_modules', 'dist', '.git'}.intersection(p.relative_to(root).parts) and
               not p.name.endswith('.tsbuildinfo')]
    outputs = [p for p in (root / 'dist').rglob('*') if p.is_file() and p.name != 'hufu-ui-build.json']
    if not outputs or not (root / 'dist/index.html').is_file():
        raise ValueError('设置页 dist 不完整')
    return dict(sources={p.relative_to(root).as_posix(): digest(p) for p in sorted(sources)},
                outputs={p.relative_to(root / 'dist').as_posix(): digest(p) for p in sorted(outputs)})


def verify_ui(root: Path) -> dict:
    receipt = json.loads((root / 'dist/hufu-ui-build.json').read_text(encoding='utf-8'))
    if receipt != ui_fingerprint(root):
        raise ValueError('设置页源码或 dist 已改变，请重新运行 ui-build')
    return receipt


def verify_ui_binary(binary: Path, root: Path) -> dict:
    receipt = verify_ui(root)
    image = binary.read_bytes()
    index = (root / 'dist/index.html').read_bytes()
    if index not in image:
        raise ValueError(f'后台未嵌入本轮设置页：{binary}')
    for name in receipt['outputs']:
        if name.startswith('assets/') and name.removeprefix('assets/').encode() not in image:
            raise ValueError(f'后台缺少本轮前端资源名：{name}')
        if (root / 'dist' / name).read_bytes() not in image:
            raise ValueError(f'后台未嵌入本轮前端资源内容：{name}')
    return dict(file=binary.name, ui_index_sha256=receipt['outputs']['index.html'])


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    p = sub.add_parser("manifest")
    p.add_argument("root", type=Path)
    p.add_argument("--arch", required=True, choices=("x64", "arm64"))
    p.add_argument("--version", required=True)
    p = sub.add_parser("verify")
    p.add_argument("root", type=Path)
    p = sub.add_parser("stage")
    p.add_argument("--payload", type=Path, required=True)
    p.add_argument("--data", type=Path, required=True)
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--version", required=True)
    p = sub.add_parser("check-update")
    p.add_argument("exe", type=Path)
    p = sub.add_parser("ui-build")
    p.add_argument("root", type=Path)
    p.add_argument("--npm", default="npm")
    p = sub.add_parser("ui-verify")
    p.add_argument("root", type=Path)
    p = sub.add_parser("ui-binary")
    p.add_argument("exe", type=Path)
    p.add_argument("root", type=Path)
    args = parser.parse_args()
    if args.command == "manifest":
        result = make_manifest(args.root, args.arch, args.version)
    elif args.command == "verify":
        result = validate_manifest(args.root)
    elif args.command == "stage":
        result = stage(args.payload, args.data, args.out, args.version)
    elif args.command == 'ui-build':
        if not (args.root / 'node_modules').is_dir():
            subprocess.run([args.npm, 'ci', '--no-audit', '--no-fund'], cwd=args.root, check=True)
        subprocess.run([args.npm, 'run', 'build'], cwd=args.root, check=True)
        result = ui_fingerprint(args.root)
        (args.root / 'dist/hufu-ui-build.json').write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')
    elif args.command == 'ui-verify':
        result = verify_ui(args.root)
    elif args.command == 'ui-binary':
        result = verify_ui_binary(args.exe, args.root)
    else:
        result = check_update_artifact(args.exe)
    print(json.dumps(result, ensure_ascii=True, indent=2))


if __name__ == "__main__":
    main()
