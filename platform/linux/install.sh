#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 明雅流风 <crrvx@outlook.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# 虎符 Linux 安装：构建 → 装配 assets → 用户级服务 → 系统级 addon。
set -euo pipefail

usage() {
    cat <<'EOF'
虎符输入法 · Linux 安装（在仓库根目录执行）
用法：platform/linux/install.sh [选项]

  --no-build    跳过构建（使用已有产物）
  --no-system   跳过系统级 addon（不调用 sudo）
  -h, --help    显示本用法

安装会同时装配仓库 assets/码表/ 与 assets/数据/；模型需另行获取。
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

if [[ "$EUID" -eq 0 ]]; then
    echo '✗ 请勿用 sudo 运行整个脚本（用户级文件会装到 /root）。' >&2
    echo '  正确用法：platform/linux/install.sh [--no-build] [--no-system]' >&2
    exit 2
fi

DO_BUILD=1
NO_SYSTEM=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --no-build) DO_BUILD=0; shift ;;
        --no-system) NO_SYSTEM=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "未知参数: $1（--help 看用法）" >&2; exit 2 ;;
    esac
done

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

if compgen -G "$ROOT/platform/linux/themes/hufu-*" >/dev/null; then
    mkdir -p "$XDG_DATA/fcitx5/themes"
    for old_theme in "$XDG_DATA"/fcitx5/themes/hufu-*; do
        if [[ -d "$old_theme" ]]; then
            rm -rf "$old_theme"
        fi
    done
    cp -r "$ROOT"/platform/linux/themes/hufu-* "$XDG_DATA/fcitx5/themes/"
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

if [[ "$NO_SYSTEM" == 1 ]]; then
    say '⑤ 跳过系统级 addon（--no-system）'
    echo "  之后执行：sudo install -Dm755 $ADDON_SO /usr/lib/fcitx5/libhufu.so"
    echo "            sudo install -Dm644 $ROOT/platform/linux/hufu-addon/conf/hufu.addon.conf /usr/share/fcitx5/addon/hufu.conf"
    echo "            sudo install -Dm644 $ROOT/platform/linux/hufu-addon/conf/hufu.inputmethod.conf /usr/share/fcitx5/inputmethod/hufu.conf"
else
    say '⑤ 安装 fcitx5 addon（需要 sudo）'
    sudo install -Dm755 "$ADDON_SO" /usr/lib/fcitx5/libhufu.so
    sudo install -Dm644 "$ROOT/platform/linux/hufu-addon/conf/hufu.addon.conf" \
        /usr/share/fcitx5/addon/hufu.conf
    sudo install -Dm644 "$ROOT/platform/linux/hufu-addon/conf/hufu.inputmethod.conf" \
        /usr/share/fcitx5/inputmethod/hufu.conf
    grep -q '^\[Addon\]' /usr/share/fcitx5/addon/hufu.conf \
        || die '/usr/share/fcitx5/addon/hufu.conf 内容异常'
    grep -q '^\[InputMethod\]' /usr/share/fcitx5/inputmethod/hufu.conf \
        || die '/usr/share/fcitx5/inputmethod/hufu.conf 内容异常'
    ok 'addon 与两个 conf 已安装'
fi

say '完成 ✔'
cat <<'EOF'
后续：
  1) 重启 fcitx5：    fcitx5 -r -d  # 或以发行版的方式重启
  2) 添加「虎符」：   fcitx5-configtool → 输入法
  3) 设置页：         fcitx5-configtool 或 http://127.0.0.1:4390/
  4) 引擎状态：       systemctl --user status hufu-server
EOF

echo
model_hint
icon_hint
