#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 明雅流风 <crrvx@outlook.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# 虎符输入法 · Linux 卸载
#
# 用法与选项见 usage()（platform/linux/uninstall.sh --help）。
set -euo pipefail

usage() {
    cat <<'EOF'
虎符输入法 · Linux 卸载（在仓库根目录执行）
用法：platform/linux/uninstall.sh [选项]

  --purge       连「模型」一起删（整树删除，事后无需手动清理）
  --no-system   跳过系统级删除（/usr/lib/fcitx5 等，无需 sudo）
  --dry-run     只打印将要执行的每一个改动性动作（rm/sudo/systemctl/pkill），
                不产生任何副作用，退出码 0
  -h, --help    显示本用法

默认**除「模型」外全部删净**：系统级 addon、用户级文件与 systemd 服务、运行期 socket 与
音效缓存，以及数据目录 `~/.local/share/hufu/` 下除 `模型/` 以外的全部内容（码表、配置、
用户词与调整日志、皮肤、diag）。模型体积大且由用户手动获取，脚本不擅自删——结束时给出
「手动删除模型命令」。
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

# 收尾提示的两色（终端且未设 NO_COLOR 时才上色：管道/日志里不留转义码）
if [[ -t 1 && -z "${NO_COLOR:-}" ]]; then
    C_ORANGE=$'\033[38;5;208m'; C_GREEN=$'\033[32m'; C_OFF=$'\033[0m'
else
    C_ORANGE=''; C_GREEN=''; C_OFF=''
fi
hint() { printf '%s%s%s\n' "$C_ORANGE" "$*" "$C_OFF"; }      # 橙色：中文提示
show_code() { printf '%s%s%s\n' "$C_GREEN" "$*" "$C_OFF"; }  # 绿色：命令

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
# 运行期残留：按键音效缓存目录（addon 落的 wav）与引擎诊断日志
run rm -rf "${XDG_RUNTIME_DIR:-/tmp}/hufu-sound" 2>/dev/null || true
run rm -f "${TMPDIR:-/tmp}/hufu-server-trace.log" 2>/dev/null || true

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
    "$HOME/.config/fcitx5/conf/hufu.conf" \
    "$HOME/.local/share/icons/hicolor/scalable/apps/hufu.svg" \
    "$HOME/.local/share/icons/hicolor/48x48/apps/hufu.png" \
    "$HOME/.local/share/icons/hicolor/22x22/apps/hufu.png"
# 图标缓存里的残留记录：有工具就重刷一次（没有工具时图标按目录实时解析，无碍）
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    run gtk-update-icon-cache -q -t -f "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
fi
# 收空目录：install 建过的这几处若空着就一并收掉（用户本来就有内容时 rmdir 失败，无副作用）
run rmdir --ignore-fail-on-non-empty \
    "$HOME/.local/share/icons/hicolor/scalable/apps" \
    "$HOME/.local/share/icons/hicolor/48x48/apps" \
    "$HOME/.local/share/icons/hicolor/22x22/apps" \
    "$HOME/.local/share/icons/hicolor/scalable" \
    "$HOME/.local/share/icons/hicolor/48x48" \
    "$HOME/.local/share/icons/hicolor/22x22" \
    "$HOME/.local/share/icons/hicolor" \
    "$HOME/.local/share/icons" \
    "$HOME/.local/share/applications" \
    "$HOME/.local/bin" \
    "$HOME/.config/systemd/user" \
    "$HOME/.config/systemd" \
    "$HOME/.local/share" \
    "$HOME/.local" \
    "$HOME/.config" 2>/dev/null || true

# ── 数据目录：默认除「模型」外全删（模型给出手动删除命令）──────────────────
# 为什么不用台账逐个删：台账只登记 install.sh 装配进去的随包文件，用户词与调整
# （码表/<方案>/用户调整.txt）、调整日志、config.json、皮肤、diag 都不在里面——
# 逐个删会留下这些「用起来才有」的残留。默认按「除模型全删」执行，一次清干净。
# 模型体积大（约 880MB）且由用户手动获取，脚本不擅自删：结束时打印手动删除命令。
if [[ "$PURGE" == 1 ]]; then
    say '④ 删除数据目录（--purge：含「模型」整树删除）'
    if [[ -d "$HUFU_ROOT" ]]; then
        run rm -rf "$HUFU_ROOT"
        ok "删除 $HUFU_ROOT（整树）"
    else
        echo "  · $HUFU_ROOT 不存在，跳过"
    fi
else
    say '④ 删除数据目录（除「模型」外全部删净；模型由你手动删）'
    if [[ -d "$HUFU_ROOT" ]]; then
        # 顶层逐项删（-maxdepth 1），只跳过「模型」：码表/数据 整树走，模型留下。
        while IFS= read -r -d '' entry; do
            run rm -rf "$entry"
        done < <(find "$HUFU_ROOT" -mindepth 1 -maxdepth 1 ! -name 模型 -print0)
        ok "删除 $HUFU_ROOT 下除「模型」以外的全部内容"
    else
        echo "  · $HUFU_ROOT 不存在，跳过"
    fi
fi

if [[ "$PURGE" == 1 ]]; then
    finish '完成 ✔（含模型，已全部删除）'
else
    finish '完成 ✔'
    if [[ -d "$HUFU_ROOT/模型" ]]; then
        hint '模型（约 880MB）由你手动获取，卸载脚本不擅自删除；如不再需要：'
        show_code "手动删除模型命令：rm -rf $HUFU_ROOT/模型"
    fi
    if command -v fcitx5-configtool >/dev/null 2>&1; then
        hint '输入法列表里若还留着「虎符」条目，在 fcitx5-configtool 里移除即可。'
    fi
fi
