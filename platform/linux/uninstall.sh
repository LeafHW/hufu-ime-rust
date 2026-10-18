#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 明雅流风 <crrvx@outlook.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# 虎符 Linux 卸载：默认删除除「模型」外的全部内容。
set -euo pipefail

usage() {
    cat <<'EOF'
虎符输入法 · Linux 卸载（在仓库根目录执行）
用法：platform/linux/uninstall.sh [选项]

  --purge       连「模型」一起删
  --no-system   跳过系统级文件（/usr/lib/fcitx5 等，无需 sudo）
  -h, --help    显示本用法

默认保留「模型」（约 880MB，由用户手动获取），结束时打印手动删除命令。
EOF
}

XDG_DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
XDG_CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}"
HUFU_ROOT="$XDG_DATA/hufu"
BIN_DIR="${XDG_BIN_HOME:-$HOME/.local/bin}"

PURGE=0
NO_SYSTEM=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --purge) PURGE=1; shift ;;
        --no-system) NO_SYSTEM=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "未知参数: $1（--help 看用法）" >&2; exit 2 ;;
    esac
done

if [[ "$EUID" -eq 0 ]]; then
    echo '✗ 请勿用 sudo 运行整个脚本（用户级清理会落到 /root）。' >&2
    exit 2
fi

say() { printf '\n\033[1m%s\033[0m\n' "$*"; }

say '① 停止 hufu-server'
if command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then
    systemctl --user disable --now hufu-server.service 2>/dev/null || true
    systemctl --user daemon-reload 2>/dev/null || true
fi
rm -f "$XDG_CONFIG/systemd/user/hufu-server.service"
pkill -x hufu-server 2>/dev/null || true
rm -f "${XDG_RUNTIME_DIR:-/tmp}/hufu-ime.sock" /tmp/hufu-ime.sock
rm -rf "${XDG_RUNTIME_DIR:-/tmp}/hufu-sound"
rm -f "${TMPDIR:-/tmp}/hufu-server-trace.log"

if [[ "$NO_SYSTEM" == 1 ]]; then
    say '② 跳过系统级文件（--no-system）'
    echo '  之后执行：sudo rm -f /usr/lib/fcitx5/libhufu.so \'
    echo '      /usr/share/fcitx5/addon/hufu.conf /usr/share/fcitx5/inputmethod/hufu.conf'
else
    say '② 删除 fcitx5 addon（需要 sudo）'
    sudo rm -f /usr/lib/fcitx5/libhufu.so \
        /usr/share/fcitx5/addon/hufu.conf \
        /usr/share/fcitx5/inputmethod/hufu.conf
fi

say '③ 删除用户级文件'
rm -f "$BIN_DIR/hufu-server" \
    "$XDG_DATA/applications/hufu-settings.desktop" \
    "$XDG_CONFIG/fcitx5/conf/hufu.conf" \
    "$XDG_DATA/icons/hicolor/scalable/apps/hufu.svg" \
    "$XDG_DATA/icons/hicolor/48x48/apps/hufu.png" \
    "$XDG_DATA/icons/hicolor/22x22/apps/hufu.png"
for theme_dir in "$XDG_DATA"/fcitx5/themes/hufu-*; do
    if [[ -d "$theme_dir" ]]; then
        rm -rf "$theme_dir"
    fi
done
rmdir --ignore-fail-on-non-empty \
    "$XDG_DATA/fcitx5/themes" "$XDG_DATA/fcitx5" \
    "$XDG_DATA/icons/hicolor/scalable/apps" \
    "$XDG_DATA/icons/hicolor/48x48/apps" \
    "$XDG_DATA/icons/hicolor/22x22/apps" \
    "$XDG_DATA/icons/hicolor/scalable" \
    "$XDG_DATA/icons/hicolor/48x48" \
    "$XDG_DATA/icons/hicolor/22x22" \
    "$XDG_DATA/icons/hicolor" "$XDG_DATA/icons" \
    "$XDG_DATA/applications" "$BIN_DIR" 2>/dev/null || true
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q -t -f "$XDG_DATA/icons/hicolor" 2>/dev/null || true
fi

say '④ 删除数据目录'
if [[ "$PURGE" == 1 ]]; then
    rm -rf "$HUFU_ROOT"
else
    if [[ -d "$HUFU_ROOT" ]]; then
        find "$HUFU_ROOT" -mindepth 1 -maxdepth 1 ! -name 模型 -exec rm -rf -- {} +
    fi
fi

say '完成 ✔'
if [[ "$PURGE" != 1 && -d "$HUFU_ROOT/模型" ]]; then
    echo "「模型」已保留；如不再需要：rm -rf $HUFU_ROOT/模型"
fi
echo '重启 fcitx5 生效：fcitx5 -r -d'
