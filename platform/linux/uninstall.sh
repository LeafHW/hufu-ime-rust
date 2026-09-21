#!/usr/bin/env bash
# 虎符输入法 · Linux 卸载
#
# 用法：platform/linux/uninstall.sh [--purge] [--no-system]
#   --purge       连同用户数据（~/.local/share/hufu）一并删除
#   --no-system   跳过系统级删除（/usr/lib/fcitx5 等，无需 sudo）
set -euo pipefail

PURGE=0
NO_SYSTEM=0
for a in "$@"; do
    case "$a" in
        --purge) PURGE=1 ;;
        --no-system) NO_SYSTEM=1 ;;
        -h|--help) sed -n '2,8p' "$0"; exit 0 ;;
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

say '① 停止并禁用 hufu-server（systemd user）'
if command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then
    systemctl --user disable --now hufu-server.service 2>/dev/null || true
    rm -f "$HOME/.config/systemd/user/hufu-server.service"
    systemctl --user daemon-reload 2>/dev/null || true
fi
# 收尾：手动拉起（非 systemd）的残留引擎进程 + 运行期 socket 文件
pkill -x hufu-server 2>/dev/null || true
rm -f "${XDG_RUNTIME_DIR:-/tmp}/hufu-ime.sock" /tmp/hufu-ime.sock 2>/dev/null || true

if [[ "$NO_SYSTEM" == 1 ]]; then
    say '② 跳过系统级删除（--no-system）'
    echo '  之后执行：sudo rm -f /usr/lib/fcitx5/libhufu.so \'
    echo '      /usr/share/fcitx5/addon/hufu.conf /usr/share/fcitx5/inputmethod/hufu.conf'
else
    say '② 删除 fcitx5 addon（需要 sudo）'
    sudo rm -f /usr/lib/fcitx5/libhufu.so \
        /usr/share/fcitx5/addon/hufu.conf \
        /usr/share/fcitx5/inputmethod/hufu.conf
fi

say '③ 删除用户级文件'
rm -f "$HOME/.local/bin/hufu-server" \
    "$HOME/.local/share/applications/hufu-settings.desktop" \
    "$HOME/.config/fcitx5/conf/hufu.conf"

if [[ "$PURGE" == 1 ]]; then
    say '④ 删除用户数据（--purge）'
    rm -rf "${XDG_DATA_HOME:-$HOME/.local/share}/hufu"
else
    say '④ 保留用户数据（如需清除：uninstall.sh --purge）'
    echo "  ${XDG_DATA_HOME:-$HOME/.local/share}/hufu"
fi

say '完成 ✔'
echo '重启 fcitx5 生效：fcitx5 -r -d'
