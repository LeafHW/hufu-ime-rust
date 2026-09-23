#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 crux <crrvx@outlook.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# 虎符输入法 · Linux 卸载
#
# 用法与选项见 usage()（platform/linux/uninstall.sh --help）。
set -euo pipefail

usage() {
    cat <<'EOF'
虎符输入法 · Linux 卸载（在仓库根目录执行）
用法：platform/linux/uninstall.sh [选项]

  --purge       连用户数据一起删（用户词与调整/日志/配置/皮肤/模型；整树删除）
  --no-system   跳过系统级删除（/usr/lib/fcitx5 等，无需 sudo）
  --dry-run     只打印将要执行的每一个改动性动作（rm/sudo/systemctl/pkill），
                不产生任何副作用，退出码 0
  -h, --help    显示本用法

默认按 assets/MANIFEST 台账删数据：install.sh 落盘的就是台账里登记的那些文件，
逐个删掉再清理空目录（装卸对称），用户数据（用户词与调整/日志/配置/皮肤/模型）保留。
台账缺失（不完整检出）时给出提示并退回原行为：默认整树保留，--purge 整树删除。
EOF
}

PURGE=0
NO_SYSTEM=0
DRY_RUN=0
# 仓库根（用于读 assets/MANIFEST）；数据目录与 install.sh 同一默认值。
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
MANIFEST="$ROOT/assets/MANIFEST"
HUFU_ROOT="${XDG_DATA_HOME:-$HOME/.local/share}/hufu"
for a in "$@"; do
    case "$a" in
        --purge) PURGE=1 ;;
        --no-system) NO_SYSTEM=1 ;;
        --dry-run) DRY_RUN=1 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "未知参数: $a（--help 看用法）" >&2; exit 2 ;;
    esac
done

# 防呆：不要用 sudo 跑整个脚本（用户级清理会落到 /root）
if [[ "$EUID" -eq 0 ]]; then
    echo '✗ 请勿用 sudo 运行整个脚本（用户级清理会落到 /root）。' >&2
    echo '  正确用法：./platform/linux/uninstall.sh（系统级删除由脚本内部调 sudo）' >&2
    exit 2
fi

say() { printf '\n\033[1m%s\033[0m\n' "$*"; }

# ── dry-run 支撑 ───────────────────────────────────────────────────────────
# 约定：每一个改动性动作（rm / sudo / systemctl / pkill）都必须经 run() 落地，
# 不允许直接调用——漏一处，--dry-run 就少报一次真实副作用。
# 只读探测（command -v / systemctl show-environment）不走这里。
run() { # --dry-run 时只回显命令行
    if [[ "$DRY_RUN" == 1 ]]; then
        printf '  [dry-run]'; printf ' %q' "$@"; printf '\n'
        return 0
    fi
    "$@"
}
ok() { # 结果提示：dry-run 下不能报「已完成」（此刻什么都没做）
    if [[ "$DRY_RUN" == 1 ]]; then
        printf '  · [dry-run] 将会：%s\n' "$*"
    else
        printf '  ✓ %s\n' "$*"
    fi
}
finish() { # 收尾提示：dry-run 不报「完成」，避免与真实卸载混淆
    if [[ "$DRY_RUN" == 1 ]]; then
        say 'dry-run 结束：以上为将要执行的全部动作，未做任何改动'
    else
        say "$1"
        echo '重启 fcitx5 生效：fcitx5 -r -d'
    fi
}

say '① 停止并禁用 hufu-server（systemd user）'
if command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then
    run systemctl --user disable --now hufu-server.service 2>/dev/null || true
    run rm -f "$HOME/.config/systemd/user/hufu-server.service"
    run systemctl --user daemon-reload 2>/dev/null || true
fi
# 收尾：手动拉起（非 systemd）的残留引擎进程 + 运行期 socket 文件
run pkill -x hufu-server 2>/dev/null || true
run rm -f "${XDG_RUNTIME_DIR:-/tmp}/hufu-ime.sock" /tmp/hufu-ime.sock 2>/dev/null || true

if [[ "$NO_SYSTEM" == 1 ]]; then
    say '② 跳过系统级删除（--no-system）'
    echo '  之后执行：sudo rm -f /usr/lib/fcitx5/libhufu.so \'
    echo '      /usr/share/fcitx5/addon/hufu.conf /usr/share/fcitx5/inputmethod/hufu.conf'
else
    say '② 删除 fcitx5 addon（需要 sudo）'
    run sudo rm -f /usr/lib/fcitx5/libhufu.so \
        /usr/share/fcitx5/addon/hufu.conf \
        /usr/share/fcitx5/inputmethod/hufu.conf
fi

say '③ 删除用户级文件'
run rm -f "$HOME/.local/bin/hufu-server" \
    "$HOME/.local/share/applications/hufu-settings.desktop" \
    "$HOME/.config/fcitx5/conf/hufu.conf"

# ── 按台账删已装配的数据（与 install.sh 同一份 assets/MANIFEST）────────────
# 清单路径是仓库相对（assets/码表/…），落地路径去掉 assets/ 前缀：$HUFU_ROOT/码表/…。
# 只删台账登记的文件：用户词、调整日志、config.json、模型等不在台账里，默认保留。
# 台账缺失（不完整检出/脚本被单独拷走）时返回 1，由调用处退回原行为。
remove_manifest_data() {
    if [[ ! -f "$MANIFEST" ]]; then
        echo "  • 缺少 $MANIFEST（不完整检出？）：无法按台账逐项删除" >&2
        echo '    → 退回原行为：默认整个数据目录原样保留（要清除用 --purge 整树删）' >&2
        return 1
    fi
    local line path dst removed=0
    while IFS= read -r line; do
        line="${line%$'\r'}"
        [[ "$line" =~ ^[[:space:]]*(#|$) ]] && continue
        path="$(printf '%s' "$line" | cut -f3)"
        [[ -n "$path" ]] || continue
        dst="$HUFU_ROOT/${path#assets/}"
        [[ -e "$dst" || -L "$dst" ]] || continue
        run rm -f "$dst"
        removed=$((removed + 1))
    done <"$MANIFEST"
    # 目录收尾：删空的方案/资源目录（-delete 隐含 -depth，父目录同趟一起处理）。
    # 只删空目录——用户词/配置/模型所在目录非空，自然留下。
    run find "$HUFU_ROOT" -mindepth 1 -type d -empty -delete
    # 数据根目录本身：全空（没有用户词/配置/模型）时一并收掉，非空则保持不动。
    if [[ -d "$HUFU_ROOT" ]] && [[ -z "$(ls -A "$HUFU_ROOT" 2>/dev/null)" ]]; then
        run rmdir "$HUFU_ROOT"
    fi
    if [[ "$DRY_RUN" == 1 ]]; then
        echo "  · [dry-run] 以上列出的是当前存在的 $removed 个台账文件（真实运行同样按存在与否跳过）"
    fi
    ok "按 assets/MANIFEST 删除 $HUFU_ROOT 下装配的 $removed 个文件 + 清空目录"
    return 0
}

if [[ "$PURGE" == 1 ]]; then
    say '④ 删除用户数据（--purge：台账文件 + 用户词与调整/配置/皮肤/模型 整树删）'
    if [[ -d "$HUFU_ROOT" ]]; then
        run rm -rf "$HUFU_ROOT"
        ok "删除 $HUFU_ROOT（整树）"
    else
        echo "  · $HUFU_ROOT 不存在，跳过"
    fi
else
    say '④ 按台账删除已装配的数据（保留用户词与调整/日志/配置/皮肤/模型；整树清除用 --purge）'
    if [[ -d "$HUFU_ROOT" ]]; then
        if ! remove_manifest_data; then
            echo "  · 保留 $HUFU_ROOT（未做任何删除）"
        fi
    else
        echo "  · $HUFU_ROOT 不存在，跳过"
    fi
fi

finish '完成 ✔'
