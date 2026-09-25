#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 明雅流风 <crrvx@outlook.com>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# 品牌图形自检（platform/linux/branding/）：
#   1) 主源 hufu.png 存在、是 8 位 RGBA，且边长足够（图标要透明背景，位图由它缩小）；
#   2) hufu.svg 自包含：`href` 只允许内嵌 data URI，且**内嵌字节的 sha256 == 主源 sha256**
#      （贴错图、换过主源没重跑都抓得住）；
#   3) 每个位图**尺寸与文件名一致**（hufu-22.png = 22×22，hufu-48.png = 48×48）、是 RGBA；
#   4) 重跑 build-icons.py 到临时目录，与仓内产物比对（`hufu.svg` 逐字节、位图**逐像素**）——
#      换过主源没重跑、或手改过产物都在这里失败。位图不逐字节比：PNG 的压缩字节由 zlib 实现
#      决定（zlib-ng 与上游 zlib 对同一份像素产出的字节不同），逐字节会把环境差异误报成回归；
#   5) 有渲染器时把 SVG 渲染一次确认可渲染（**不比对字节**——不同 librsvg 版本渲染结果可能不同，
#      逐字节会把版本差异误报成回归）。
# 只读自检：临时目录用完即删，不改仓内产物；重跑生成请用 build-icons.py。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$ROOT"

BRANDING="platform/linux/branding"
MASTER="$BRANDING/hufu.png"
SVG="$BRANDING/hufu.svg"
PNGS=("$BRANDING/hufu-22.png" "$BRANDING/hufu-48.png")
TOOL="$BRANDING/build-icons.py"
MIN_MASTER=256

fail() { echo "✗ $*" >&2; exit 1; }

dim() {  # PNG 信息 → 第一维边长
    printf '%s' "$1" | sed -n 's/^PNG image data, \([0-9]*\) x \([0-9]*\).*/\1/p'
}

# ① 主源
[[ -s "$MASTER" ]] || fail "缺主源：$MASTER（唯一源是它，SVG 与位图都由它生成）"
info="$(file -b "$MASTER")"
printf '%s' "$info" | grep -q '^PNG image data' || fail "主源不是 PNG：$MASTER（$info）"
printf '%s' "$info" | grep -q 'RGBA' || fail "主源不是 RGBA（图标需要透明背景）：$MASTER（$info）"
mw="$(dim "$info")"
mh="$(printf '%s' "$info" | sed -n 's/^PNG image data, [0-9]* x \([0-9]*\).*/\1/p')"
[[ -n "$mw" && -n "$mh" ]] || fail "主源尺寸解析失败：$MASTER（$info）"
(( mw >= MIN_MASTER && mh >= MIN_MASTER )) \
    || fail "主源太小（${mw}×${mh}）：位图由它缩小，至少 ${MIN_MASTER}×${MIN_MASTER}"
echo "  ✓ 主源 hufu.png：${mw}×${mh} RGBA（sha256 $(sha256sum "$MASTER" | cut -c1-12)…）"

# ② 自包含 SVG，且内嵌的就是当前主源
[[ -s "$SVG" ]] || fail "缺自包含 SVG：$SVG（生成：python3 $TOOL）"
grep -q '<svg' "$SVG" || fail "SVG 不含 <svg> 根元素：$SVG"
while IFS= read -r href; do
    case "$href" in
    "data:image/png;base64,"*) ;;
    *) fail "SVG 含外部引用（应自包含）：$href" ;;
    esac
done < <(grep -o 'href="[^"]*"' "$SVG" | sed 's/^href="//; s/"$//')
embedded="$(sed -n 's/.*base64,\([A-Za-z0-9+/=]*\)".*/\1/p' "$SVG" | base64 -d | sha256sum | cut -d' ' -f1)"
[[ -n "$embedded" ]] || fail "SVG 没有内嵌 base64 位图：$SVG"
master_sha="$(sha256sum "$MASTER" | cut -d' ' -f1)"
[[ "$embedded" == "$master_sha" ]] \
    || fail "SVG 内嵌的不是当前主源（内嵌 ${embedded:0:12}… ≠ 主源 ${master_sha:0:12}…）：跑 python3 $TOOL 重新生成"
echo "  ✓ hufu.svg 自包含，内嵌主源一致（$(wc -c <"$SVG") 字节）"

# ③ 位图尺寸与格式
for png in "${PNGS[@]}"; do
    [[ -s "$png" ]] || fail "缺位图：$png（生成：python3 $TOOL）"
    want="$(basename "$png" .png)"; want="${want##*-}"      # hufu-22.png → 22
    info="$(file -b "$png")"
    got="$(dim "$info")"
    [[ "$got" == "$want" && "$got" != "" ]] \
        || fail "位图尺寸与文件名不符：$png 是 $got px，名字要求 $want px（$info）"
    printf '%s' "$info" | grep -q "RGBA" \
        || fail "位图不是 RGBA（图标需要透明背景）：$png（$info）"
    echo "  ✓ $(basename "$png")：${want}×${want} RGBA"
done

# ④ 重跑生成并与仓内产物比对（SVG 逐字节；位图逐像素，见下）
[[ -f "$TOOL" ]] || fail "缺生成脚本：$TOOL"
command -v python3 >/dev/null 2>&1 \
    || fail "缺 python3：本守卫要重跑 $TOOL 比对产物（说明见 branding/README.md）"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
python3 "$TOOL" --out "$tmp" --quiet || fail "生成脚本执行失败：$TOOL"
svg_name="$(basename "$SVG")"
cmp -s "$SVG" "$tmp/$svg_name" \
    || fail "仓内 $svg_name 与重跑结果不一致（换过主源或手改过产物？跑 python3 $TOOL 重新生成并提交）"
for png in "${PNGS[@]}"; do
    name="$(basename "$png")"
    mine="$(python3 "$TOOL" --digest "$png")" || fail "读不出位图像素：$png"
    fresh="$(python3 "$TOOL" --digest "$tmp/$name")" || fail "读不出重跑产物：$tmp/$name"
    [[ "$mine" == "$fresh" ]] \
        || fail "仓内 $name 与重跑结果像素不一致（$mine ≠ $fresh）：换过主源或手改过产物？跑 python3 $TOOL 重新生成并提交"
done
echo "  ✓ 重跑生成与仓内产物一致（$svg_name 逐字节、${#PNGS[@]} 个位图逐像素）"

# ⑤ 可渲染（不比字节）
if command -v rsvg-convert >/dev/null 2>&1; then
    rsvg-convert -w 48 -h 48 "$SVG" -o "$tmp/render.png" >/dev/null 2>&1 \
        || fail "SVG 渲染失败（rsvg-convert）：$SVG"
    [[ -s "$tmp/render.png" ]] || fail "SVG 渲染产物为空：$SVG"
    echo "  ✓ SVG 可渲染（rsvg-convert，未比对字节）"
else
    echo "  · 未装 rsvg-convert：跳过「SVG 可渲染」检查（主源/内嵌/位图/重跑已校验）"
fi

echo "check-branding: 品牌图形自检通过（主源 + 自包含 SVG + ${#PNGS[@]} 个位图，重跑一致）"
