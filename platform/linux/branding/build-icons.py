#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 明雅流风 <crrvx@outlook.com>
# SPDX-License-Identifier: GPL-3.0-or-later
"""虎符品牌图形：主源 `hufu.png` → 自包含 `hufu.svg` + 各尺寸位图（零第三方依赖）。

主源是艺术位图（1600×1600 RGBA），本脚本只做**版式归一**，不重绘、不降画质：

1. 取可见内容的外接矩形（alpha 高于 `ALPHA_THRESHOLD`，忽略几乎不可见的辉光尾）；
2. 画布 = 内容外接**正方形** + 四周 8% 留白，内容居中（越出主源时就近贴边）；
3. `hufu.svg`：内嵌主源字节（base64 data URI，与主源逐字节相同），`viewBox` 即该画布——自包含、
   无外部引用，缩小渲染到任意尺寸都清晰（只有放大超过主源分辨率才会软化）；
4. 各尺寸位图：同一画布做**面积平均**缩小（下采样最稳，无振铃）。

用法：

    python3 platform/linux/branding/build-icons.py            # 写回 branding/
    python3 platform/linux/branding/build-icons.py --out /tmp/x --quiet   # 生成到别处（守卫比对用）

产物必须**可重复**：同一份主源、同一 zlib 实现下逐字节一致（`checks/check-branding.sh` 按
`hufu.svg` 逐字节、位图逐像素比对——PNG 的压缩字节随 zlib 实现不同，见 `pixel_digest`）。
PNG 的解码/编码都在本文件内实现，不引第三方依赖（与 `theme/build-themes.py` 同风格）。
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import struct
import sys
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
MASTER = HERE / "hufu.png"
SVG = HERE / "hufu.svg"
SIZES = (22, 48)
PADDING_RATIO = 0.08
# 取内容外接框时忽略的 alpha 上限（0–255）：辉光尾部的 alpha 只有个位数，肉眼不可见，
# 但它会把外接框撑到接近整幅，导致小尺寸下图形式微、四周留白不均。
ALPHA_THRESHOLD = 8
PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"

HEADER = """<!-- SPDX-FileCopyrightText: 2026 明雅流风 <crrvx@outlook.com> -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<!-- 虎符图标：由 platform/linux/branding/build-icons.py 生成，勿手改；
     主源 platform/linux/branding/hufu.png（艺术位图）在下方以 data URI 内嵌，与主源逐字节相同。 -->
"""


def fail(message: str) -> None:
    print(f"build-icons: {message}", file=sys.stderr)
    raise SystemExit(1)


def read_png(path: Path) -> tuple[int, int, bytearray]:
    """读 8 位 RGBA、非隔行 PNG，返回 (宽, 高, 逐行字节)。其余格式直接报错。"""
    data = path.read_bytes()
    if data[:8] != PNG_SIGNATURE:
        fail(f"{path.name} 不是 PNG")
    width = height = 0
    idat = bytearray()
    pos = 8
    while pos + 8 <= len(data):
        (length,) = struct.unpack(">I", data[pos : pos + 4])
        tag = data[pos + 4 : pos + 8]
        body = data[pos + 8 : pos + 8 + length]
        pos += 12 + length
        if tag == b"IHDR":
            width, height, depth, color, _, _, interlace = struct.unpack(">IIBBBBB", body)
            if (depth, color, interlace) != (8, 6, 0):
                fail(f"{path.name} 需为 8 位 RGBA 非隔行（实际 depth={depth} color={color} interlace={interlace}）")
        elif tag == b"IDAT":
            idat += body
        elif tag == b"IEND":
            break
    if width == 0 or height == 0 or not idat:
        fail(f"{path.name} 结构不完整（缺 IHDR 或 IDAT）")

    raw = zlib.decompress(bytes(idat))
    stride = width * 4
    pixels = bytearray(height * stride)
    prev = bytearray(stride)
    offset = 0
    for row in range(height):
        kind = raw[offset]
        offset += 1
        line = bytearray(raw[offset : offset + stride])
        offset += stride
        if kind == 0:
            pass
        elif kind == 1:
            for i in range(4, stride):
                line[i] = (line[i] + line[i - 4]) & 0xFF
        elif kind == 2:
            for i in range(stride):
                line[i] = (line[i] + prev[i]) & 0xFF
        elif kind == 3:
            for i in range(stride):
                left = line[i - 4] if i >= 4 else 0
                line[i] = (line[i] + ((left + prev[i]) >> 1)) & 0xFF
        elif kind == 4:
            for i in range(stride):
                left = line[i - 4] if i >= 4 else 0
                up = prev[i]
                upleft = prev[i - 4] if i >= 4 else 0
                estimate = left + up - upleft
                da, db, dc = abs(estimate - left), abs(estimate - up), abs(estimate - upleft)
                if da <= db and da <= dc:
                    predictor = left
                elif db <= dc:
                    predictor = up
                else:
                    predictor = upleft
                line[i] = (line[i] + predictor) & 0xFF
        else:
            fail(f"{path.name} 第 {row} 行用了未知过滤器 {kind}")
        pixels[row * stride : (row + 1) * stride] = line
        prev = line
    return width, height, pixels


def png_bytes(width: int, height: int, rows: list[bytearray]) -> bytes:
    """手写 PNG（8 位 RGBA，filter 0，zlib 9 级 ⇒ 同输入同字节；不引第三方依赖）。"""
    raw = b"".join(b"\x00" + bytes(row) for row in rows)

    def chunk(tag: bytes, payload: bytes) -> bytes:
        return struct.pack(">I", len(payload)) + tag + payload + struct.pack(">I", zlib.crc32(tag + payload) & 0xFFFFFFFF)

    return (
        PNG_SIGNATURE
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def visible_bbox(width: int, height: int, pixels: bytearray) -> tuple[int, int, int, int]:
    """可见内容（alpha > ALPHA_THRESHOLD）的外接矩形；全透明即报错。"""
    left, top, right, bottom = width, height, -1, -1
    for y in range(height):
        base = y * width * 4
        row_alpha = pixels[base + 3 : base + width * 4 : 4]
        if max(row_alpha, default=0) <= ALPHA_THRESHOLD:
            continue
        top = min(top, y)
        bottom = y
        for x, alpha in enumerate(row_alpha):
            if alpha > ALPHA_THRESHOLD:
                left = min(left, x)
                right = max(right, x)
    if right < 0:
        fail(f"主源在 alpha > {ALPHA_THRESHOLD} 上没有可见内容")
    return left, top, right + 1, bottom + 1


def canvas_box(width: int, height: int, bbox: tuple[int, int, int, int]) -> tuple[int, int, int]:
    """内容外接矩形 → 正方形画布（左上角 + 边长）：内容居中，四周留 PADDING_RATIO 留白。"""
    left, top, right, bottom = bbox
    content = max(right - left, bottom - top)
    side = min(content + 2 * round(content * PADDING_RATIO), min(width, height))
    x = min(max(round((left + right) / 2 - side / 2), 0), width - side)
    y = min(max(round((top + bottom) / 2 - side / 2), 0), height - side)
    return x, y, side


def resize_box(
    width: int, pixels: bytearray, x: int, y: int, side: int, size: int
) -> list[bytearray]:
    """把画布区域按**面积平均**缩到 size×size。

    先按 alpha 预乘再平均（全透明像素带的无意义 RGB 因此不参与），最后除回 alpha——
    否则透明边缘会把颜色拉向那些不可见的像素值，缩小后出现脏边。
    """
    rows: list[bytearray] = []
    for ty in range(size):
        y0 = y + side * ty // size
        y1 = max(y0 + 1, y + side * (ty + 1) // size)
        row = bytearray()
        for tx in range(size):
            x0 = x + side * tx // size
            x1 = max(x0 + 1, x + side * (tx + 1) // size)
            r = g = b = a = count = 0
            for sy in range(y0, y1):
                base = (sy * width + x0) * 4
                for sx in range(x1 - x0):
                    off = base + sx * 4
                    alpha = pixels[off + 3]
                    r += pixels[off] * alpha
                    g += pixels[off + 1] * alpha
                    b += pixels[off + 2] * alpha
                    a += alpha
                    count += 1
            if a == 0:
                row += bytes((0, 0, 0, 0))
            else:
                # 全整数四舍五入（round-half-up）：不碰浮点，跨实现/跨版本结果一致。
                row += bytes((
                    (r + a // 2) // a,
                    (g + a // 2) // a,
                    (b + a // 2) // a,
                    (a + count // 2) // count,
                ))
        rows.append(row)
    return rows


def pixel_digest(path: Path) -> str:
    """位图的**像素**摘要（`宽x高 + RGBA 的 sha256`）。

    守卫用它比对仓内产物与重跑结果：PNG 的压缩字节由 zlib 实现决定（zlib-ng 与上游 zlib
    对同一份像素产出的字节并不相同），逐字节比对会把环境差异误报成回归。
    """
    width, height, pixels = read_png(path)
    return f"{width}x{height} {hashlib.sha256(bytes(pixels)).hexdigest()}"


def main() -> int:
    parser = argparse.ArgumentParser(description="生成 platform/linux/branding 的图标产物")
    parser.add_argument("--master", default=str(MASTER), help="主源位图（缺省 branding/hufu.png）")
    parser.add_argument("--out", default=str(HERE), help="产物目录（缺省 branding/）")
    parser.add_argument("--quiet", action="store_true", help="只输出错误（守卫重跑时用）")
    parser.add_argument("--digest", metavar="PNG", help="只打印该 PNG 的像素摘要后退出（守卫比对用）")
    args = parser.parse_args()

    if args.digest:
        print(pixel_digest(Path(args.digest)))
        return 0

    master = Path(args.master)
    out = Path(args.out)
    if not master.is_file():
        fail(f"缺少主源 {master}")
    out.mkdir(parents=True, exist_ok=True)

    payload = master.read_bytes()
    width, height, pixels = read_png(master)
    bbox = visible_bbox(width, height, pixels)
    x, y, side = canvas_box(width, height, bbox)

    svg_path = out / SVG.name
    svg_path.write_text(
        HEADER
        + '<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"\n'
        + f'     viewBox="{x} {y} {side} {side}" width="{side}" height="{side}">\n'
        + f'  <image x="0" y="0" width="{width}" height="{height}"'
        + f' xlink:href="data:image/png;base64,{base64.b64encode(payload).decode("ascii")}"/>\n'
        + "</svg>\n",
        encoding="utf-8",
    )

    sizes = []
    for size in SIZES:
        target = out / f"hufu-{size}.png"
        target.write_bytes(png_bytes(size, size, resize_box(width, pixels, x, y, side, size)))
        sizes.append(target)

    if not args.quiet:
        print(f"主源 {master.name}：{width}x{height} RGBA，内容外接 {bbox[2] - bbox[0]}x{bbox[3] - bbox[1]}")
        print(f"画布（viewBox）：{x} {y} {side} {side}")
        print(f"已写出 {svg_path.name}（内嵌 {len(payload)} 字节）+ " + "、".join(t.name for t in sizes))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
