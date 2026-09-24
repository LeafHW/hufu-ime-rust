#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 明雅流风 <crrvx@outlook.com>
# SPDX-License-Identifier: GPL-3.0-or-later
"""虎符皮肤 → fcitx5 主题包（离线转换；产物入仓，由 checks/check-themes.sh 钉住）。

皮肤的唯一源是引擎侧 `engine/crates/hufu-server/official-skins/*.json`（Windows 自绘候选窗
消费的那份）。Linux 的候选窗是 fcitx5 的面板，样式只能来自 **fcitx5 主题**，所以这里把每套
皮肤**离线**转成一套主题包（`hufu-<id>`，中文名进 `[Metadata] Name`），随包装到
`~/.local/share/fcitx5/themes/`——用户在哪端选的都是同一套皮肤名。

用法：
    python3 platform/linux/theme/build-themes.py                 # 生成到 platform/linux/themes
    python3 platform/linux/theme/build-themes.py --out /tmp/x    # 生成到别处（校验脚本比对用）

映射与取舍见 platform/linux/theme/README.md（不做的：模糊/动效/每候选配色/字体）。
产物必须**可重复**：同一份皮肤 JSON 必须逐字节生成同样的文件（校验脚本据此比对）。
"""

from __future__ import annotations

import argparse
import json
import struct
import sys
import zlib
from pathlib import Path

# ── 常量 ───────────────────────────────────────────────────────────────────
REPO = Path(__file__).resolve().parents[3]
SKINS_DIR = REPO / "engine/crates/hufu-server/official-skins"
OUT_DIR = REPO / "platform/linux/themes"

# 位图尺寸：面板/高亮是九宫格（尺寸 = 2×边距 + 中间可拉伸段），小图标固定 16×16。
CENTER = 8          # 九宫格中间可拉伸段像素
ICON = 16           # prev/next/arrow/radio 尺寸
SUPERSAMPLE = 4     # 圆角/图形抗锯齿的超采样倍数
MIN_BACK_ALPHA = 0.85  # fcitx5 无模糊合成：底几乎全透明（如「迷雾改」alpha=0.08）会看不清


# ── 颜色 ───────────────────────────────────────────────────────────────────
def parse_color(text: str) -> tuple[int, int, int, int]:
    """`#RRGGBB` / `#RRGGBBAA` → (r, g, b, a)；解析不了就抛错（宁可不生成，也不出半套主题）。"""
    t = text.strip().lstrip("#")
    if len(t) not in (6, 8):
        raise ValueError(f"颜色格式不认识：{text!r}")
    r, g, b = (int(t[i:i + 2], 16) for i in (0, 2, 4))
    a = int(t[6:8], 16) if len(t) == 8 else 255
    return r, g, b, a


def rgba_hex(color: tuple[int, int, int, int], min_alpha: float = 0.0) -> str:
    """(r,g,b,a) → fcitx5 的 `#rrggbbaa`（Color::toString 的原生格式）。"""
    r, g, b, a = color
    if min_alpha > 0:
        a = max(a, int(round(min_alpha * 255)))
    return f"#{r:02x}{g:02x}{b:02x}{a:02x}"


def with_alpha(color: tuple[int, int, int, int], alpha: float) -> tuple[int, int, int, int]:
    r, g, b, _ = color
    return r, g, b, max(0, min(255, int(round(alpha * 255))))


# ── 位图 ───────────────────────────────────────────────────────────────────
def png_bytes(width: int, height: int, pixels: list[bytearray]) -> bytes:
    """手写 PNG（8 位 RGBA，filter 0，zlib 9 级 ⇒ 同输入同字节；不引第三方依赖）。"""
    raw = b"".join(b"\x00" + bytes(row) for row in pixels)

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (struct.pack(">I", len(data)) + tag + data
                + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF))

    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9))
            + chunk(b"IEND", b""))


def over(dst: tuple[float, float, float, float],
         src: tuple[float, float, float, float]) -> tuple[float, float, float, float]:
    """标准 source-over 合成（颜色都用 0–1 的浮点，alpha 预乘前的形式）。"""
    sr, sg, sb, sa = src
    dr, dg, db, da = dst
    out_a = sa + da * (1 - sa)
    if out_a <= 0:
        return 0.0, 0.0, 0.0, 0.0
    out_r = (sr * sa + dr * da * (1 - sa)) / out_a
    out_g = (sg * sa + dg * da * (1 - sa)) / out_a
    out_b = (sb * sa + db * da * (1 - sa)) / out_a
    return out_r, out_g, out_b, out_a


def in_round_rect(x: float, y: float, w: float, h: float, radius: float, inset: float) -> bool:
    x0, y0, x1, y1 = inset, inset, w - inset, h - inset
    if x < x0 or x > x1 or y < y0 or y > y1:
        return False
    r = max(0.0, min(radius, (x1 - x0) / 2, (y1 - y0) / 2))
    cx = min(max(x, x0 + r), x1 - r)
    cy = min(max(y, y0 + r), y1 - r)
    return (x - cx) ** 2 + (y - cy) ** 2 <= r * r


def in_circle(x: float, y: float, cx: float, cy: float, radius: float) -> bool:
    return (x - cx) ** 2 + (y - cy) ** 2 <= radius * radius


def in_polygon(x: float, y: float, points: list[tuple[float, float]]) -> bool:
    inside = False
    n = len(points)
    for i in range(n):
        x1, y1 = points[i]
        x2, y2 = points[(i + 1) % n]
        if (y1 > y) != (y2 > y):
            xt = x1 + (y - y1) * (x2 - x1) / (y2 - y1)
            if x < xt:
                inside = not inside
    return inside


def render(width: int, height: int, shade) -> list[bytearray]:
    """逐像素超采样渲染：shade(px, py) 返回该子采样点的 (r,g,b,a)（0–1 浮点，可返回 None）。"""
    rows: list[bytearray] = []
    step = 1.0 / SUPERSAMPLE
    for y in range(height):
        row = bytearray()
        for x in range(width):
            acc = (0.0, 0.0, 0.0, 0.0)
            for sy in range(SUPERSAMPLE):
                for sx in range(SUPERSAMPLE):
                    px = x + (sx + 0.5) * step
                    py = y + (sy + 0.5) * step
                    col = shade(px, py)
                    if col is None:
                        continue
                    r, g, b, a = col
                    acc = over(acc, (r, g, b, a / (SUPERSAMPLE * SUPERSAMPLE)))
            row += bytes((int(round(acc[0] * 255)), int(round(acc[1] * 255)),
                          int(round(acc[2] * 255)), int(round(acc[3] * 255))))
        rows.append(row)
    return rows


def norm(color: tuple[int, int, int, int]) -> tuple[float, float, float, float]:
    r, g, b, a = color
    return r / 255, g / 255, b / 255, a / 255


def panel_png(radius: int, border: int, fill: tuple[int, int, int, int],
              stroke: tuple[int, int, int, int]) -> bytes:
    """九宫格面板：外圈圆角矩形填充 + 内侧描边（border ≤ 0 时无描边）。"""
    edge = radius + border
    size = 2 * edge + CENTER
    fill_c = norm(fill)
    stroke_c = norm(stroke)

    def shade(px: float, py: float):
        if not in_round_rect(px, py, size, size, radius + border, 0.5):
            return None
        if border > 0 and not in_round_rect(px, py, size, size, radius, 0.5 + border):
            return stroke_c
        return fill_c

    return png_bytes(size, size, render(size, size, shade))


def chevron_png(direction: int, color: tuple[int, int, int, int]) -> bytes:
    """翻页箭头：direction=-1 左（上一页）/ +1 右（下一页）。"""
    c = norm(color)
    w = ICON
    pts = [(10.5, 3.5), (10.5, 12.5), (5.5, 8.0)] if direction > 0 else \
          [(5.5, 3.5), (5.5, 12.5), (10.5, 8.0)]
    return png_bytes(w, w, render(w, w, lambda x, y: c if in_polygon(x, y, pts) else None))


def arrow_png(color: tuple[int, int, int, int]) -> bytes:
    """子菜单箭头（右向小三角）。"""
    c = norm(color)
    pts = [(6.5, 4.0), (6.5, 12.0), (11.0, 8.0)]
    return png_bytes(ICON, ICON, render(ICON, ICON, lambda x, y: c if in_polygon(x, y, pts) else None))


def radio_png(color: tuple[int, int, int, int]) -> bytes:
    """菜单勾选框：外环 + 内点。"""
    c = norm(color)
    cx = cy = ICON / 2

    def shade(x: float, y: float):
        if not in_circle(x, y, cx, cy, 5.6):
            return None
        if in_circle(x, y, cx, cy, 4.2):
            return None
        if in_circle(x, y, cx, cy, 2.0):
            return c
        return None

    return png_bytes(ICON, ICON, render(ICON, ICON, shade))


# ── theme.conf ─────────────────────────────────────────────────────────────
def theme_conf(skin: dict) -> str:
    c = {k: parse_color(v) for k, v in skin["colors"].items()}
    l = skin["layout"]
    name = skin["name"]
    sid = skin["id"]

    radius = max(1, int(round(float(l["corner_radius"]))))
    hilite_radius = max(1, int(round(float(l["hilited_corner_radius"]))))
    border = 1 if float(l["border_width"]) >= 0.25 else 0
    margin_x = max(1, int(round(float(l["margin_x"]))))
    margin_y = max(1, int(round(float(l["margin_y"]))))
    pad = max(1, int(round(float(l["hilite_padding"]))))

    back = c["back_color"]
    border_color = c["border_color"]
    text = c.get("candidate_text_color", c["text_color"])
    hilite_text = c.get("hilited_candidate_text_color", c["hilited_text_color"])
    hilite_label = c.get("hilited_candidate_label_color", hilite_text)
    hilite_back = c["hilited_candidate_back_color"]

    # 底/高亮的 alpha 交给位图（面板图带 alpha），颜色键则用 fcitx5 原生 #rrggbbaa。
    return f"""# 虎符皮肤「{sid}」（{name}）→ fcitx5 主题
# 由 platform/linux/theme/build-themes.py 从 engine/crates/hufu-server/official-skins/{sid}.json 生成，
# **不要手改**：改皮肤源或改转换脚本后重跑，`checks/check-themes.sh` 会比对。
# 映射与取舍（模糊/动效/每候选配色/字体不做）见 platform/linux/theme/README.md。

[Metadata]
Name={name}
Name[zh_CN]={name}
Name[zh_TW]={name}
Name[en_US]={sid}
Version=1
Author=HuFu
Description=虎符输入法皮肤「{name}」的 fcitx5 主题版
ScaleWithDPI=True

[InputPanel]
NormalColor={rgba_hex(text)}
HighlightColor={rgba_hex(hilite_label)}
HighlightCandidateColor={rgba_hex(hilite_text)}
HighlightBackgroundColor={rgba_hex(hilite_back)}
PageButtonAlignment=Last Candidate

[InputPanel/ContentMargin]
Left={margin_x}
Right={margin_x}
Top={margin_y}
Bottom={margin_y}

[InputPanel/TextMargin]
Left={pad}
Right={pad}
Top={pad}
Bottom={pad}

[InputPanel/Background]
Image=panel.png

[InputPanel/Background/Margin]
Left={radius + border}
Right={radius + border}
Top={radius + border}
Bottom={radius + border}

[InputPanel/Highlight]
Image=highlight.png

[InputPanel/Highlight/Margin]
Left={hilite_radius}
Right={hilite_radius}
Top={hilite_radius}
Bottom={hilite_radius}

[InputPanel/PrevPage]
Image=prev.png

[InputPanel/NextPage]
Image=next.png

[Menu]
Spacing=2

[Menu/Background]
Color={rgba_hex(back, MIN_BACK_ALPHA)}
BorderColor={rgba_hex(border_color)}
BorderWidth={border}

[Menu/Background/Margin]
Left={radius + border}
Right={radius + border}
Top={radius + border}
Bottom={radius + border}

[Menu/ContentMargin]
Left={margin_x}
Right={margin_x}
Top={margin_y}
Bottom={margin_y}

[Menu/TextMargin]
Left={pad}
Right={pad}
Top={pad}
Bottom={pad}

[Menu/Highlight]
Color={rgba_hex(hilite_back)}

[Menu/Highlight/Margin]
Left={hilite_radius}
Right={hilite_radius}
Top={hilite_radius}
Bottom={hilite_radius}

[Menu/CheckBox]
Image=radio.png

[Menu/SubMenu]
Image=arrow.png
"""


def build_theme(skin: dict, out_root: Path) -> Path:
    c = {k: parse_color(v) for k, v in skin["colors"].items()}
    l = skin["layout"]
    radius = max(1, int(round(float(l["corner_radius"]))))
    hilite_radius = max(1, int(round(float(l["hilited_corner_radius"]))))
    border = 1 if float(l["border_width"]) >= 0.25 else 0
    # fcitx5 没有模糊合成：底几乎全透明的皮肤（如「迷雾改」alpha=0.08）要抬到可读下限。
    a = c["back_color"][3] / 255
    fill = with_alpha(c["back_color"], max(a, MIN_BACK_ALPHA))
    text = c.get("candidate_text_color", c["text_color"])

    theme_dir = out_root / skin["id"]
    theme_dir.mkdir(parents=True, exist_ok=True)
    (theme_dir / "theme.conf").write_text(theme_conf(skin), encoding="utf-8")
    (theme_dir / "panel.png").write_bytes(panel_png(radius, border, fill, c["border_color"]))
    (theme_dir / "highlight.png").write_bytes(
        panel_png(hilite_radius, 0, c["hilited_candidate_back_color"], c["hilited_candidate_back_color"]))
    (theme_dir / "prev.png").write_bytes(chevron_png(-1, text))
    (theme_dir / "next.png").write_bytes(chevron_png(+1, text))
    (theme_dir / "arrow.png").write_bytes(arrow_png(text))
    (theme_dir / "radio.png").write_bytes(radio_png(text))
    return theme_dir


def main() -> int:
    ap = argparse.ArgumentParser(description="虎符皮肤 → fcitx5 主题包（离线转换）")
    ap.add_argument("--skins", type=Path, default=SKINS_DIR, help=f"皮肤 JSON 目录（默认 {SKINS_DIR}）")
    ap.add_argument("--out", type=Path, default=OUT_DIR, help=f"输出目录（默认 {OUT_DIR}）")
    ap.add_argument("--quiet", action="store_true")
    args = ap.parse_args()

    files = sorted(args.skins.glob("*.json"))
    if not files:
        print(f"✗ 没有皮肤 JSON：{args.skins}", file=sys.stderr)
        return 1

    for path in files:
        skin = json.loads(path.read_text(encoding="utf-8"))
        theme_dir = build_theme(skin, args.out)
        if not args.quiet:
            print(f"  ✓ {theme_dir.relative_to(REPO) if theme_dir.is_relative_to(REPO) else theme_dir}"
                  f"（{skin['name']}）")
    if not args.quiet:
        print(f"build-themes: 生成 {len(files)} 套 fcitx5 主题 → {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
