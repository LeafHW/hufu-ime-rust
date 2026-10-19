#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 明雅流风 <crrvx@outlook.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# 虎符 Linux 安装：构建 → 装配 assets → 用户运行时 → 系统级或用户级 fcitx5 addon。
set -euo pipefail

usage() {
    cat <<'EOF'
虎符输入法 · Linux 安装（在仓库根目录执行）
用法：platform/linux/install.sh -s|--system [--no-build]
      platform/linux/install.sh -u|--user   [--no-build]
      platform/linux/install.sh -t|--theme

  -s, --system  安装系统级 fcitx5 addon（需要 sudo）
  -u, --user    安装用户级 fcitx5 addon（无需 sudo）
  -t, --theme   安装/更新 fcitx5 主题（独立于 addon；可单用或与 -s/-u 组合）
  --no-build    跳过构建（使用已有产物）
  -h, --help    显示本用法

短选项可合并（如 -sh、-st），但 -s 与 -u 互斥。
空参、-h、--help 均显示本用法；安装须指定 -s/-u，或使用 -t。
-s/-u 不隐含 -t：主题只在显式给出 -t 时安装。完整安装会装配用户运行时
（码表/数据、hufu-server、systemd user 服务、桌面入口、图标），模型另行获取。
EOF
}

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
ASSETS_DIR="$ROOT/assets"
XDG_DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
XDG_CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}"
HUFU_ROOT="$XDG_DATA/hufu"
DATA_DIR="$HUFU_ROOT/数据"
BIN_DIR="${XDG_BIN_HOME:-$HOME/.local/bin}"
BUILD_DIR="$ROOT/platform/linux/build"
SERVER_BIN="$ROOT/engine/target/release/hufu-server"
ADDON_SO="$BUILD_DIR/libhufu.so"
ADDON_CONF_SRC="$ROOT/platform/linux/hufu-addon/conf/hufu.addon.conf"
IM_CONF_SRC="$ROOT/platform/linux/hufu-addon/conf/hufu.inputmethod.conf"
THEMES_DIR="$ROOT/platform/linux/themes"
THEME_TARGET="$XDG_DATA/fcitx5/themes"

# 用户级 addon：conf 放 XDG 数据目录，Library 指向同一用户目录下的绝对路径。
USER_FCITX_DATA="$XDG_DATA/fcitx5"
USER_ADDON_DIR="$USER_FCITX_DATA/addon"
USER_IM_DIR="$USER_FCITX_DATA/inputmethod"
USER_LIB_DIR="$USER_FCITX_DATA/lib"

# 系统级 addon：Debian/Ubuntu 的 libdir 可能带 multiarch。
system_addon_dir() {
    local libdir=''
    if command -v pkg-config >/dev/null 2>&1; then
        libdir=$(pkg-config --variable=libdir Fcitx5Core 2>/dev/null || true)
    fi
    [[ -n "$libdir" ]] || libdir=/usr/lib
    printf '%s/fcitx5\n' "$libdir"
}
SYSTEM_ADDON_DIR="$(system_addon_dir)"

if (($# == 0)); then
    usage
    exit 0
fi

MODE=''
DO_BUILD=1
DO_THEMES=0
set_mode() {
    case "$1" in
        s)
            if [[ "$MODE" == user ]]; then
                echo '✗ -s/--system 与 -u/--user 不能同时使用' >&2
                exit 2
            fi
            MODE=system ;;
        u)
            if [[ "$MODE" == system ]]; then
                echo '✗ -s/--system 与 -u/--user 不能同时使用' >&2
                exit 2
            fi
            MODE=user ;;
    esac
}
while [[ $# -gt 0 ]]; do
    case "$1" in
        --system) set_mode s; shift ;;
        --user) set_mode u; shift ;;
        --theme) DO_THEMES=1; shift ;;
        --no-build) DO_BUILD=0; shift ;;
        --help) usage; exit 0 ;;
        --) shift; break ;;
        --*) echo "✗ 未知选项: $1（--help 看用法）" >&2; exit 2 ;;
        -?*)
            cluster=${1#-}
            while [[ -n "$cluster" ]]; do
                ch=${cluster:0:1}
                cluster=${cluster:1}
                case "$ch" in
                    s|u) set_mode "$ch" ;;
                    t) DO_THEMES=1 ;;
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
if [[ -z "$MODE" && "$DO_THEMES" == 0 ]]; then
    echo '✗ 必须指定 -s/-u，或使用 -t/--theme（--help 看用法）' >&2
    exit 2
fi

if [[ "$EUID" -eq 0 ]]; then
    echo '✗ 请勿用 sudo 运行整个脚本（用户级文件会装到 /root）。' >&2
    echo '  正确用法：platform/linux/install.sh -s|--system、-u|--user 或 -t|--theme' >&2
    exit 2
fi

say() { printf '\n\033[1m%s\033[0m\n' "$*"; }
die() { printf '✗ %s\n' "$*" >&2; exit 1; }
ok() { printf '  ✓ %s\n' "$*"; }

model_hint() {
    echo '模型，整句与神经重排，约 880MB：'
    echo '    不随安装包分发，缺模型为纯码表模式'
    echo '    https://github.com/LeafHW/hufu-ime-rust/releases/tag/模型'
    echo "    下载后，把「模型」文件夹直接放进 $HUFU_ROOT/"
    echo
}

icon_hint() {
    echo 'logo 图标已随包安装，若托盘 logo 显示文字而非图标：'
    echo '    fcitx5-configtool → 附加组件 → 经典界面：关闭「优先使用文字图标」'
    echo 'logo 图标不正确：'
    echo '    kquitapp6 plasmashell && kstart plasmashell'
    echo
}

install_themes() {
    compgen -G "$THEMES_DIR/hufu-*" >/dev/null || die "缺少 $THEMES_DIR/hufu-*（主题产物未生成？）"
    mkdir -p "$THEME_TARGET"
    for old_theme in "$THEME_TARGET"/hufu-*; do
        if [[ -d "$old_theme" ]]; then
            rm -rf "$old_theme"
        fi
    done
    cp -r "$THEMES_DIR"/hufu-* "$THEME_TARGET/"
    ok "fcitx5 主题已安装到 $THEME_TARGET"
}

if [[ "$DO_THEMES" == 1 && -z "$MODE" ]]; then
    say '① 安装 fcitx5 主题'
    install_themes
    say '完成 ✔'
    echo '重启 fcitx5 生效：fcitx5 -r -d'
    exit 0
fi

[[ -d "$ASSETS_DIR/码表" ]] || die "缺少 assets/码表（不完整检出？）"
[[ -d "$ASSETS_DIR/数据" ]] || die "缺少 assets/数据（不完整检出？）"
bash "$ROOT/platform/linux/checks/check-assets.sh" \
    || die 'assets/ 台账校验失败（用 check-assets.sh --write 重新登记）'

if [[ "$DO_BUILD" == 1 ]]; then
    say '① 构建 hufu-server'
    ( cd "$ROOT/engine" && cargo build --release -p hufu-server )
    say '② 构建 fcitx5 addon'
    cmake -S "$ROOT/platform/linux/hufu-addon" -B "$BUILD_DIR" \
        -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr
    cmake --build "$BUILD_DIR" -j "$(nproc)"
fi
[[ -x "$SERVER_BIN" ]] || die "缺少 $SERVER_BIN（先构建，或去掉 --no-build）"
[[ -f "$ADDON_SO" ]] || die "缺少 $ADDON_SO（先构建，或去掉 --no-build）"

say '③ 装配码表与数据'
mkdir -p "$DATA_DIR" "$HUFU_ROOT/码表" "$HUFU_ROOT/模型"
cp -a "$ASSETS_DIR/码表/." "$HUFU_ROOT/码表/"
cp -a "$ASSETS_DIR/数据/." "$DATA_DIR/"
ok '码表与数据已装配'

if [[ ! -f "$DATA_DIR/config.json" ]]; then
    cat > "$DATA_DIR/config.json" <<'JSON'
{
  "schema": { "dir": "码表", "current": "虎整句" },
  "reverse": { "scheme": "拼音" },
  "candidates": {
    "split_scheme": "虎码",
    "show_unicode_comment": true,
    "show_split": true
  },
  "general": {
    "shift_switch": false,
    "ctrl_space_switch": false,
    "caps_action": "None"
  }
}
JSON
    ok '生成默认 数据/config.json'
fi

say '④ 安装用户级文件'
mkdir -p "$BIN_DIR" "$XDG_CONFIG/systemd/user" "$XDG_DATA/applications" \
    "$XDG_DATA/icons/hicolor/scalable/apps" \
    "$XDG_DATA/icons/hicolor/48x48/apps" \
    "$XDG_DATA/icons/hicolor/22x22/apps"

install -m 755 "$SERVER_BIN" "$BIN_DIR/hufu-server"
install -m 644 "$ROOT/platform/linux/systemd/hufu-server.service" \
    "$XDG_CONFIG/systemd/user/hufu-server.service"
if [[ "$BIN_DIR" != "$HOME/.local/bin" ]]; then
    sed -i "s|^ExecStart=.*|ExecStart=$BIN_DIR/hufu-server|" \
        "$XDG_CONFIG/systemd/user/hufu-server.service"
fi
install -m 644 "$ROOT/platform/linux/desktop/hufu-settings.desktop" \
    "$XDG_DATA/applications/hufu-settings.desktop"

install -m 644 "$ROOT/platform/linux/branding/hufu.svg" \
    "$XDG_DATA/icons/hicolor/scalable/apps/hufu.svg"
install -m 644 "$ROOT/platform/linux/branding/hufu-48.png" \
    "$XDG_DATA/icons/hicolor/48x48/apps/hufu.png"
install -m 644 "$ROOT/platform/linux/branding/hufu-22.png" \
    "$XDG_DATA/icons/hicolor/22x22/apps/hufu.png"
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q -t -f "$XDG_DATA/icons/hicolor" 2>/dev/null || true
fi

if [[ "$DO_THEMES" == 1 ]]; then
    say '⑥ 安装 fcitx5 主题'
    install_themes
fi

if command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then
    systemctl --user daemon-reload
    systemctl --user enable hufu-server.service
    systemctl --user restart hufu-server.service
    ok 'systemd user 服务已启动'
else
    echo "  · 无 systemd user 会话：请手动运行 $BIN_DIR/hufu-server"
fi
[[ ":$PATH:" == *":$BIN_DIR:"* ]] || echo "  · 提示：$BIN_DIR 不在 PATH 中"

if [[ "$MODE" == user ]]; then
    say '⑤ 安装用户级 fcitx5 addon（无需 sudo）'
    mkdir -p "$USER_ADDON_DIR" "$USER_IM_DIR" "$USER_LIB_DIR"
    install -m 755 "$ADDON_SO" "$USER_LIB_DIR/libhufu.so"
    install -m 644 "$ADDON_CONF_SRC" "$USER_ADDON_DIR/hufu.conf"
    install -m 644 "$IM_CONF_SRC" "$USER_IM_DIR/hufu.conf"
    sed -i "s|^Library=.*|Library=$USER_LIB_DIR/libhufu|" \
        "$USER_ADDON_DIR/hufu.conf"
    grep -q "^Library=$USER_LIB_DIR/libhufu$" "$USER_ADDON_DIR/hufu.conf" \
        || die '用户级 hufu.conf 的 Library 写入失败'
    ok "addon 已装到 $USER_LIB_DIR（无需 sudo）"
else
    say '⑤ 安装系统级 fcitx5 addon（需要 sudo）'
    command -v sudo >/dev/null 2>&1 || die '系统级安装需要 sudo；可改用 -u/--user'
    sudo install -Dm755 "$ADDON_SO" "$SYSTEM_ADDON_DIR/libhufu.so"
    sudo install -Dm644 "$ADDON_CONF_SRC" /usr/share/fcitx5/addon/hufu.conf
    sudo install -Dm644 "$IM_CONF_SRC" /usr/share/fcitx5/inputmethod/hufu.conf
    grep -q '^\[Addon\]' /usr/share/fcitx5/addon/hufu.conf \
        || die '/usr/share/fcitx5/addon/hufu.conf 内容异常'
    grep -q '^\[InputMethod\]' /usr/share/fcitx5/inputmethod/hufu.conf \
        || die '/usr/share/fcitx5/inputmethod/hufu.conf 内容异常'
    ok "addon 已装到 $SYSTEM_ADDON_DIR"
fi

say '完成 ✔'
cat <<EOF
后续：
  1) 重启 fcitx5：    fcitx5 -r -d  # 或以发行版的方式重启
  2) 添加「虎符」：   fcitx5-configtool → 输入法
  3) 设置页：         fcitx5-configtool 或 http://127.0.0.1:4390/
  4) 引擎状态：       systemctl --user status hufu-server
EOF

echo
model_hint
icon_hint
