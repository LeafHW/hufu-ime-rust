#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 明雅流风 <crrvx@outlook.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# assets/ 台账：清单（path + 字节 + sha256）与实际文件一致，且覆盖 install.sh 装配的两棵子树。
#
#   bash platform/linux/checks/check-assets.sh          # 校验（install.sh 装配前调用）
#   bash platform/linux/checks/check-assets.sh --write  # 重新生成 assets/MANIFEST
#
# 覆盖范围：`assets/码表/**` 与 `assets/数据/**`（install.sh 装配的数据与资源两棵树）；
# `assets/README.md` 是说明文档，不入清单。来源与许可见 `docs/asset-sources.md`。
#
# 清单格式：注释行以 `#` 开头；数据行三列（TAB 分隔）`<sha256>  <字节数>  <仓库相对路径>`，
# 按路径排序。`MANIFEST=<路径>` 可覆盖清单位置（校验用）。
set -euo pipefail

root=$(cd "$(dirname "$0")/../../.." && pwd)
cd "$root"
manifest="${MANIFEST:-assets/MANIFEST}"

# 装配范围（与 install.sh 的 assemble_data / assemble_assets 对应）。
trees=(assets/码表 assets/数据)

list_files() {
    find "${trees[@]}" -type f -print0 2>/dev/null | xargs -0 -r printf '%s\n' | LC_ALL=C sort
}

file_sha() { sha256sum "$1" | cut -d' ' -f1; }
file_bytes() { wc -c <"$1" | tr -d ' '; }

write_manifest() {
    {
        printf '# assets/ 台账（install.sh 装配前按本清单校验）。\n'
        printf '# 格式：<sha256>\\t<字节数>\\t<仓库相对路径>；覆盖 assets/码表/** 与 assets/数据/**。\n'
        printf '# 重新生成：bash platform/linux/checks/check-assets.sh --write\n'
        printf '# 来源与许可：docs/asset-sources.md（虎码官方发布 / TigerClaw GPL-3.0 / OpenCC Apache-2.0）。\n'
        local path
        while IFS= read -r path; do
            printf '%s\t%s\t%s\n' "$(file_sha "$path")" "$(file_bytes "$path")" "$path"
        done < <(list_files)
    } >"$manifest"
    echo "check-assets: 已写入 $manifest（$(list_files | grep -c . || true) 个文件）"
}

check_manifest() {
    local failed=0
    fail() {
        echo "FAIL $*" >&2
        failed=1
    }
    if [ ! -f "$manifest" ]; then
        echo "FAIL 缺少 $manifest（用 --write 生成）" >&2
        exit 1
    fi

    local entries
    entries=$(sed -e 's/[[:space:]]*$//' "$manifest" | grep -vE '^[[:space:]]*(#|$)' || true)
    local count
    count=$(printf '%s\n' "$entries" | grep -c . || true)
    if [ "$count" -lt 1 ]; then
        fail "$manifest 没有有效行"
    fi

    local duplicates
    duplicates=$(printf '%s\n' "$entries" | awk -F'\t' '{print $3}' | sort | uniq -d || true)
    if [ -n "$duplicates" ]; then
        fail "$manifest 有重复路径：$duplicates"
    fi

    # ① 清单 → 文件：路径在装配范围内、文件存在、字节与 sha256 都与清单相符。
    local line sha bytes path actual_sha actual_bytes
    while IFS= read -r line; do
        [ -n "$line" ] || continue
        sha=$(printf '%s' "$line" | cut -f1)
        bytes=$(printf '%s' "$line" | cut -f2)
        path=$(printf '%s' "$line" | cut -f3)
        if [ -z "$sha" ] || [ -z "$bytes" ] || [ -z "$path" ]; then
            fail "清单行格式不对（应为 sha256/字节/路径 三列）：$line"
            continue
        fi
        case "$path" in
            assets/码表/* | assets/数据/*) ;;
            *) fail "清单行不在装配范围内：$path" ;;
        esac
        if [ ! -f "$path" ]; then
            fail "清单列出的文件不存在：$path"
            continue
        fi
        actual_bytes=$(file_bytes "$path")
        if [ "$actual_bytes" != "$bytes" ]; then
            fail "字节数不符：$path（清单 $bytes，实际 $actual_bytes）"
        fi
        actual_sha=$(file_sha "$path")
        if [ "$actual_sha" != "$sha" ]; then
            fail "sha256 不符：$path（清单 ${sha:0:12}…，实际 ${actual_sha:0:12}…）"
        fi
    done <<<"$entries"

    # ② 文件 → 清单：装配范围内的每个文件都必须登记（新增文件未登记即失败）。
    local listed path
    listed=$(printf '%s\n' "$entries" | awk -F'\t' '{print $3}' | LC_ALL=C sort || true)
    while IFS= read -r path; do
        [ -n "$path" ] || continue
        if ! printf '%s\n' "$listed" | grep -qxF "$path"; then
            fail "$path 存在但未登记在 $manifest"
        fi
    done < <(list_files)

    if [ "$failed" -ne 0 ]; then
        exit 1
    fi
    echo "check-assets: $count 个文件，清单与实况一致（字节 + sha256）"
}

case "${1:-}" in
    --write) write_manifest ;;
    "") check_manifest ;;
    *)
        echo "用法：$0 [--write]" >&2
        exit 2
        ;;
esac
