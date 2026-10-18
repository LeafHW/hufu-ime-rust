#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 明雅流风 <crrvx@outlook.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# 虎符 Linux 卸载：-s 系统级、-u 用户级、-m 模型；可组合，空参无效。
set -euo pipefail

usage() {
    cat <<'USAGE'
虎符输入法 · Linux 卸载（在仓库根目录执行）
用法：platform/linux/uninstall.sh -s|-u|-m [...]
      platform/linux/uninstall.sh -s -u -m

  -s, --system  清理系统级 fcitx5 文件（需要 sudo）
  -u, --user    清理用户级 addon、服务与数据
  -m, --model   删除「模型」目录（约 880MB）
  -h, --help    显示本用法

至少指定一项，可任意组合：
  -s            只清系统级
  -u            只清用户级，保留模型
  -m            只删模型，其余用户数据保留
  -s -u         系统级 + 用户级
  -u -m         用户级 + 模型
  -s -u -m      全部清理
短选项可合并，如 -su、-sum、-um。
空参、-h、--help 均显示本用法。
USAGE
}

XDG_DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
XDG_CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}"
HUFU_ROOT="$XDG_DATA/hufu"
BIN_DIR="${XDG_BIN_HOME:-$HOME/.local/bin}"

system_addon_dir() {
    local libdir=''
    if command -v pkg-config >/dev/null 2>&1; then
        libdir=$(pkg-config --variable=libdir Fcitx5Core 2>/dev/null || true)
    fi
    [[ -n "$libdir" ]] || libdir=/usr/lib
    printf '%s/fcitx5\n' "$libdir"
}
SYSTEM_ADDON_DIR="$(system_addon_dir)"
SYSTEM_ADDON_DIRS=("$SYSTEM_ADDON_DIR")
[[ "$SYSTEM_ADDON_DIR" != /usr/lib/fcitx5 ]] && SYSTEM_ADDON_DIRS+=(/usr/lib/fcitx5)

if (($# == 0)); then
    usage
    exit 0
fi

DO_SYSTEM=0
DO_USER=0
DO_MODEL=0
set_flag() {
    case "$1" in
        s) DO_SYSTEM=1 ;;
        u) DO_USER=1 ;;
        m) DO_MODEL=1 ;;
    esac
}
while [[ $# -gt 0 ]]; do
    case "$1" in
        --system) set_flag s; shift ;;
        --user) set_flag u; shift ;;
        --model) set_flag m; shift ;;
        --help) usage; exit 0 ;;
        --) shift; break ;;
        --*) echo "✗ 未知选项: $1（--help 看用法）" >&2; exit 2 ;;
        -?*)
            cluster=${1#-}
            while [[ -n "$cluster" ]]; do
                ch=${cluster:0:1}
                cluster=${cluster:1}
                case "$ch" in
                    s|u|m) set_flag "$ch" ;;
                    h) usage; exit 0 ;;
                    *) echo "✗ 未知选项: -$ch（--help 看用法）" >&2; exit 2 ;;
                esac
            done
            shift ;;
        *) echo "✗ 不支持位置参数: $1（--help 看用法）" >&2; exit 2 ;;
    esac
done
if (($# > 0)); then
    echo "✗ 不支持位置参数: $1（--help 看用法）" >&2
    exit 2
fi
if ((!DO_SYSTEM && !DO_USER && !DO_MODEL)); then
    echo '✗ 必须至少指定 -s、-u、-m 之一（--help 看用法）' >&2
    exit 2
fi

if [[ "$EUID" -eq 0 ]]; then
    echo '✗ 请勿用 sudo 运行整个脚本（用户级清理会落到 /root）。' >&2
    exit 2
fi

say() { printf '\n\033[1m%s\033[0m\n' "$*"; }
die() { printf '✗ %s\n' "$*" >&2; exit 1; }

if ((DO_USER)); then
    say '① 停止 hufu-server'
    if command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then
        systemctl --user disable --now hufu-server.service 2>/dev/null || true
        systemctl --user daemon-reload 2>/dev/null || true
    fi
    rm -f "$XDG_CONFIG/systemd/user/hufu-server.service"
    pkill -x hufu-server 2>/dev/null || true
    rm -f "${XDG_RUNTIME_DIR:-/tmp}/hufu-ime.sock" /tmp/hufu-ime.sock 2>/dev/null || true
    rm -rf "${XDG_RUNTIME_DIR:-/tmp}/hufu-sound" 2>/dev/null || true
    rm -f "${TMPDIR:-/tmp}/hufu-server-trace.log" 2>/dev/null || true
else
    say '① 未指定 -u：保留用户服务与运行时'
fi

if ((DO_SYSTEM)); then
    system_files=()
    while IFS= read -r f; do
        [[ -n "$f" ]] && system_files+=("$f")
    done < <(
        for dir in "${SYSTEM_ADDON_DIRS[@]}"; do
            [[ -f "$dir/libhufu.so" ]] && printf '%s\n' "$dir/libhufu.so"
        done
        for f in /usr/share/fcitx5/addon/hufu.conf \
                 /usr/share/fcitx5/inputmethod/hufu.conf; do
            [[ -e "$f" ]] && printf '%s\n' "$f"
        done
    )
    if ((${#system_files[@]})); then
        say '② 删除系统级 fcitx5 文件（需要 sudo）'
        command -v sudo >/dev/null 2>&1 || die '需要 sudo 删除系统级文件；可改用 -u'
        sudo rm -f "${system_files[@]}"
    else
        say '② 系统级 fcitx5 文件未安装，跳过'
    fi
else
    say '② 未指定 -s：保留系统级文件'
fi

if ((DO_USER)); then
    say '③ 删除用户级 addon'
    rm -f "$XDG_DATA/fcitx5/addon/hufu.conf" \
        "$XDG_DATA/fcitx5/inputmethod/hufu.conf" \
        "$XDG_DATA/fcitx5/lib/libhufu.so"
    rmdir --ignore-fail-on-non-empty \
        "$XDG_DATA/fcitx5/addon" \
        "$XDG_DATA/fcitx5/inputmethod" \
        "$XDG_DATA/fcitx5/lib" 2>/dev/null || true

    say '④ 删除用户级文件'
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
else
    say '③④ 未指定 -u：保留用户级 addon、文件与桌面项'
fi

if ((DO_USER || DO_MODEL)); then
    if ((DO_USER && DO_MODEL)); then
        say '⑤ 删除数据与模型'
        rm -rf "$HUFU_ROOT"
    elif ((DO_USER)); then
        say '⑤ 删除数据（保留模型）'
        if [[ -d "$HUFU_ROOT" ]]; then
            find "$HUFU_ROOT" -mindepth 1 -maxdepth 1 ! -name 模型 -exec rm -rf -- {} +
        fi
    else
        say '⑤ 只删除模型'
        rm -rf "$HUFU_ROOT/模型"
    fi
else
    say '⑤ 未指定 -u/-m：保留数据与模型'
fi

say '完成 ✔'
if ((DO_SYSTEM)); then echo '系统级文件已清理。'; fi
if ((DO_USER)); then echo '用户级文件与数据已清理。'; fi
if ((DO_MODEL)); then echo '模型已删除。'; fi
if ((!DO_MODEL)) && [[ -d "$HUFU_ROOT/模型" ]]; then
    echo "「模型」未动；如不再需要：platform/linux/uninstall.sh -m"
fi
if ((DO_SYSTEM || DO_USER)); then
    echo '重启 fcitx5 生效：fcitx5 -r -d'
fi
