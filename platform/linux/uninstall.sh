#!/usr/bin/env bash
# 虎符输入法 · Linux 卸载
#
# 用法：platform/linux/uninstall.sh [--purge]
#   --purge  连同用户数据（~/.local/share/hufu）一并删除
set -euo pipefail

PURGE=0
[[ "${1:-}" == "--purge" ]] && PURGE=1

say() { printf '\n\033[1m%s\033[0m\n' "$*"; }

say '① 停止并禁用 hufu-server（systemd user）'
if command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then
    systemctl --user disable --now hufu-server.service 2>/dev/null || true
    rm -f "$HOME/.config/systemd/user/hufu-server.service"
    systemctl --user daemon-reload 2>/dev/null || true
fi

say '② 删除 fcitx5 addon（需要 sudo）'
sudo rm -f /usr/lib/fcitx5/libhufu.so \
    /usr/share/fcitx5/addon/hufu.conf \
    /usr/share/fcitx5/inputmethod/hufu.conf

say '③ 删除用户级文件'
rm -f "$HOME/.local/bin/hufu-server" \
    "$HOME/.local/share/applications/hufu-settings.desktop"

if [[ "$PURGE" == 1 ]]; then
    say '④ 删除用户数据（--purge）'
    rm -rf "${XDG_DATA_HOME:-$HOME/.local/share}/hufu"
else
    say '④ 保留用户数据（如需清除：uninstall.sh --purge）'
    echo "  ${XDG_DATA_HOME:-$HOME/.local/share}/hufu"
fi

say '完成 ✔'
echo '重启 fcitx5 生效：fcitx5 -r -d'
