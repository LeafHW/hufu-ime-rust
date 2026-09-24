#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 明雅流风 <crrvx@outlook.com>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# 品牌图形自检（platform/linux/branding/）：
#   1) 矢量源 hufu.svg 存在且非空；
#   2) 每个位图的**尺寸与文件名一致**（hufu-22.png = 22×22，hufu-48.png = 48×48）、
#      PNG 为 8 位 RGBA（图标主题要透明背景）；
#   3) 有渲染器时把 SVG 渲染到临时目录，确认可渲染（**不比对字节**——不同 librsvg
#      版本渲染结果可能不同，逐字节会把版本差异误报成回归）。
# 只读自检，不落任何改动；缺渲染器时第 3 步降级为提示（不算失败）。
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
BRANDING="$(cd "$HERE/../branding" && pwd)"
SVG="$BRANDING/hufu.svg"
PNGS=("$BRANDING/hufu-22.png" "$BRANDING/hufu-48.png")

fail() { echo "✗ $*" >&2; exit 1; }

[[ -s "$SVG" ]] || fail "缺矢量源：$SVG（唯一源是它，位图都由它生成）"
grep -q "<svg" "$SVG" || fail "矢量源不含 <svg> 根元素：$SVG"

for png in "${PNGS[@]}"; do
    [[ -s "$png" ]] || fail "缺位图：$png（生成命令见 branding/README.md）"
    want="$(basename "$png" .png)"; want="${want##*-}"      # hufu-22.png → 22
    info="$(file -b "$png")"
    got="$(printf '%s' "$info" | sed -n 's/^PNG image data, \([0-9]*\) x \([0-9]*\).*/\1/p')"
    [[ "$got" == "$want" && "$got" != "" ]] \
        || fail "位图尺寸与文件名不符：$png 是 $got px，名字要求 $want px（$info）"
    printf '%s' "$info" | grep -q "RGBA" \
        || fail "位图不是 RGBA（图标需要透明背景）：$png（$info）"
    echo "  ✓ $(basename "$png")：${want}×${want} RGBA"
done

if command -v rsvg-convert >/dev/null 2>&1; then
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    rsvg-convert -w 48 -h 48 "$SVG" -o "$tmp/out.png" >/dev/null 2>&1 \
        || fail "矢量源渲染失败（rsvg-convert）：$SVG"
    [[ -s "$tmp/out.png" ]] || fail "矢量源渲染产物为空：$SVG"
    echo "  ✓ 矢量源可渲染（rsvg-convert，未比对字节）"
else
    echo "  · 未装 rsvg-convert：跳过「矢量源可渲染」检查（位图尺寸/格式已校验）"
fi

echo "check-branding: 品牌图形自检通过（源 + ${#PNGS[@]} 个位图）"
