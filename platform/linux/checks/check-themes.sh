#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 明雅流风 <crrvx@outlook.com>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# 皮肤 → fcitx5 主题包自检（platform/linux/themes/）：
#   1) 引擎皮肤源与仓内主题包一一对应（不多不少）；
#   2) 重跑转换脚本到临时目录，与仓内产物**逐字节比对**——皮肤改了没重跑转换会被这里抓住；
#   3) 每套主题的必需键齐全（Metadata/InputPanel 四个颜色 + 背景与高亮图 + 边距）。
# 只读自检：临时目录用完即删，不改仓内产物。
# （皮肤源在 engine/ 下；本脚本不生成产物，重跑转换请用 build-themes.py。）
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$ROOT"

TOOL="platform/linux/theme/build-themes.py"
SKINS="engine/crates/hufu-server/official-skins"
THEMES="platform/linux/themes"
REQUIRED_KEYS=(
    "[Metadata]"
    "Name="
    "[InputPanel]"
    "NormalColor="
    "HighlightColor="
    "HighlightCandidateColor="
    "HighlightBackgroundColor="
    "[InputPanel/Background]"
    "Image=panel.png"
    "[InputPanel/Highlight]"
    "Image=highlight.png"
    "[InputPanel/Background/Margin]"
    "[InputPanel/Highlight/Margin]"
)
REQUIRED_FILES=(theme.conf panel.png highlight.png prev.png next.png arrow.png radio.png)

fail() { echo "✗ $*" >&2; exit 1; }

[[ -f "$TOOL" ]] || fail "缺转换脚本：$TOOL"
[[ -d "$SKINS" ]] || fail "缺引擎皮肤源：$SKINS"
[[ -d "$THEMES" ]] || fail "缺主题产物目录：$THEMES（先跑 $TOOL）"

# ① 皮肤源 ↔ 主题包一一对应
skins=()
for f in "$SKINS"/*.json; do
    skins+=("$(basename "$f" .json)")
done
[[ ${#skins[@]} -gt 0 ]] || fail "引擎皮肤源里没有 *.json"
missing=0
for id in "${skins[@]}"; do
    [[ -f "$THEMES/$id/theme.conf" ]] || { echo "  ✗ 缺主题包：$THEMES/$id（皮肤 $id 未转换）" >&2; missing=1; }
done
[[ "$missing" == 0 ]] || fail "有皮肤没有对应主题包（跑 $TOOL 重新生成）"
for d in "$THEMES"/*/; do
    id="$(basename "$d")"
    [[ -f "$SKINS/$id.json" ]] || fail "主题包 $id 在引擎皮肤源里不存在（皮肤已改名/删除？重跑 $TOOL 并删掉旧目录）"
done
echo "  ✓ 皮肤源 ↔ 主题包一一对应（${#skins[@]} 套）"

# ② 必需文件与必需键
for d in "$THEMES"/*/; do
    id="$(basename "$d")"
    for f in "${REQUIRED_FILES[@]}"; do
        [[ -s "$d$f" ]] || fail "$id 缺文件或为空：$f"
    done
    for key in "${REQUIRED_KEYS[@]}"; do
        grep -qF -- "$key" "$d/theme.conf" || fail "$id/theme.conf 缺必需键：$key"
    done
    # 颜色键必须是 #rrggbbaa（fcitx5 原生格式，转换脚本统一这么写）
    bad="$(grep -E '^(NormalColor|HighlightColor|HighlightCandidateColor|HighlightBackgroundColor)=' "$d/theme.conf" \
        | grep -vE '=#[0-9a-f]{8}$' || true)"
    [[ -z "$bad" ]] || fail "$id/theme.conf 颜色格式不是 #rrggbbaa："$'\n'"$bad"
done
echo "  ✓ ${#skins[@]} 套主题的必需文件与必需键齐全（颜色为 #rrggbbaa）"

# ③ 重跑转换并与仓内产物逐字节比对
tmp="$(mktemp -d)"
diffout="$(mktemp)"
trap 'rm -rf "$tmp" "$diffout"' EXIT
python3 "$TOOL" --out "$tmp" --quiet >/dev/null || fail "转换脚本执行失败：$TOOL"
# 注意：diff 的输出**不能**落在被比对的目录里（否则它自己就是「多余文件」）
if ! diff -r "$THEMES" "$tmp" >"$diffout" 2>&1; then
    sed 's/^/  /' "$diffout" | head -30
    fail "仓内主题包与重跑结果不一致（改过皮肤或转换脚本？跑 $TOOL 重新生成并提交）"
fi
echo "  ✓ 重跑转换与仓内产物逐字节一致（$(find "$THEMES" -type f | wc -l) 个文件）"

echo "check-themes: 皮肤→fcitx5 主题包自检通过（${#skins[@]} 套）"
