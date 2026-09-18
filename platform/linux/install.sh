#!/usr/bin/env bash
# 虎符输入法 · Linux 安装
#
# 系统级：fcitx5 addon（/usr/lib/fcitx5/libhufu.so + /usr/share/fcitx5/{addon,inputmethod}/hufu.conf）
# 用户级：hufu-server（~/.local/bin）+ 数据（~/.local/share/hufu）+ systemd user 服务 + 设置入口
#
# 用法（仓库根目录执行）：
#   platform/linux/install.sh [--from <码表资源目录>] [--no-build] [--data-only]
#
# 默认码表数据源：/home/crux/下载/_res/zhmn
#   （虎码秃版 小狼毫（Win）的 tigress*/tiger*.dict.yaml + opencc 转换表 + 定制/多多B）
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SRC="${HUFU_DATA_SRC:-/home/crux/下载/_res/zhmn}"
HUFU_ROOT="${XDG_DATA_HOME:-$HOME/.local/share}/hufu"
DATA_DIR="$HUFU_ROOT/数据"
BIN_DIR="$HOME/.local/bin"
BUILD_DIR="$ROOT/platform/linux/build"

# 防呆：整个脚本不要用 sudo 跑——用户级部分会装进 /root（systemd user 服务
# 也会因 root 无用户会话被跳过）。系统级 addon 由脚本内部单独调 sudo。
if [[ "$EUID" -eq 0 ]]; then
    echo '✗ 请勿用 sudo 运行整个脚本（用户级部分会装到 /root）。' >&2
    echo '  正确用法：./platform/linux/install.sh [--no-build]' >&2
    echo '  （脚本内部仅在安装 fcitx5 addon 时调用 sudo）' >&2
    exit 2
fi

DO_BUILD=1
DATA_ONLY=0
NO_SYSTEM=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --from) SRC="$2"; shift 2 ;;
        --from=*) SRC="${1#--from=}"; shift ;;
        --no-build) DO_BUILD=0; shift ;;
        --data-only) DATA_ONLY=1; DO_BUILD=0; shift ;;
        --no-system) NO_SYSTEM=1; shift ;;
        -h|--help) sed -n '2,14p' "$0"; exit 0 ;;
        *) echo "未知参数: $1（--help 看用法）" >&2; exit 2 ;;
    esac
done

say() { printf '\n\033[1m%s\033[0m\n' "$*"; }
die() { printf '✗ %s\n' "$*" >&2; exit 1; }

# ── 1) 构建 ────────────────────────────────────────────────────────────────
if [[ "$DO_BUILD" == 1 ]]; then
    say '① 构建 hufu-server（Rust）'
    (cd "$ROOT/engine" && cargo build --release -p hufu-server)
    say '② 构建 fcitx5 addon（Rust staticlib + C++ 薄壳）'
    cmake -S "$ROOT/platform/linux/hufu-addon" -B "$BUILD_DIR" \
        -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr
    cmake --build "$BUILD_DIR" -j "$(nproc)"
fi

SERVER_BIN="$ROOT/engine/target/release/hufu-server"
ADDON_SO="$BUILD_DIR/libhufu.so"
[[ -x "$SERVER_BIN" ]] || die "缺少 $SERVER_BIN（先构建，或去掉 --no-build）"
[[ -f "$ADDON_SO" ]] || die "缺少 $ADDON_SO（先构建，或去掉 --no-build）"

# ── 2) 数据装配 ────────────────────────────────────────────────────────────
assemble_data() {
    say "③ 装配数据（来源：$SRC）"
    [[ -d "$SRC" ]] || die "数据源不存在：$SRC"
    local rime="$SRC/虎码秃版 小狼毫（Win）"
    local opencc="$rime/opencc"
    local duoduo="$SRC/publish/定制/b"

    mkdir -p "$DATA_DIR" "$HUFU_ROOT/码表" "$HUFU_ROOT/模型"

    copy_schema() { # <目标方案名> <文件...>
        local name="$1"; shift
        local dir="$HUFU_ROOT/码表/$name"
        mkdir -p "$dir"
        local f
        for f in "$@"; do
            [[ -f "$f" ]] || die "缺少码表文件：$f"
            cp -f "$f" "$dir/"
        done
        cp -f "$ROOT/发行临时/补充语料.txt" "$dir/补充语料.txt"
        echo "  ✓ 码表/$name/ ← $(basename "$1") 等 $# 个文件 + 补充语料"
    }

    # 默认方案：虎码字词（tigress import 闭包 ≈250k 条）
    copy_schema 虎码字词 \
        "$rime/tigress.dict.yaml" \
        "$rime/tigress_ci.dict.yaml" \
        "$rime/tigress_simp_ci.dict.yaml"
    # 备选：虎码单字
    copy_schema 虎码单字 "$rime/tiger.dict.yaml"
    # 多多格式（格式回归用；取常用字词 + 生僻字）
    if [[ -d "$duoduo" ]]; then
        local dir="$HUFU_ROOT/码表/多多B"
        mkdir -p "$dir"
        cp -f "$duoduo/多多B常用字词.txt" "$duoduo/多多B生僻字.txt" "$dir/" 2>/dev/null || true
        cp -f "$ROOT/发行临时/补充语料.txt" "$dir/补充语料.txt"
        echo "  ✓ 码表/多多B/ ← publish/定制/b/"
    fi

    # OpenCC 转换词典（简繁/emoji 候选滤镜）
    if [[ -d "$opencc" ]]; then
        mkdir -p "$DATA_DIR/转换词典"
        local f
        for f in STPhrases.txt STCharacters.txt STCharacters_Tu.txt TSPhrases.txt TSCharacters.txt emoji.txt; do
            [[ -f "$opencc/$f" ]] && cp -f "$opencc/$f" "$DATA_DIR/转换词典/$f"
        done
        echo "  ✓ 数据/转换词典/ ← opencc/"
    fi

    # 首次安装写默认配置：默认方案=虎码字词；反查/拆分/音效先关（资源未就位）；
    # 中英切换交由 fcitx5 键盘布局（Linux 不用引擎自带英文输入）。
    if [[ ! -f "$DATA_DIR/config.json" ]]; then
        cat > "$DATA_DIR/config.json" <<'JSON'
{
  "schema": { "dir": "码表", "current": "虎码字词" },
  "reverse": { "scheme": "" },
  "candidates": { "split_scheme": "" },
  "general": {
    "shift_switch": false,
    "ctrl_space_switch": false,
    "caps_action": "None"
  }
}
JSON
        echo "  ✓ 生成 数据/config.json（默认方案：虎码字词；中英切换交由 fcitx5 布局）"
    else
        echo "  · 已存在 数据/config.json，保持不动（如需切换默认方案请在设置页操作）"
    fi
}

# ── 3) 用户级安装（引擎 + 服务 + 设置入口） ────────────────────────────────
install_user() {
    say '④ 安装 hufu-server + systemd user 服务 + 设置入口'
    mkdir -p "$BIN_DIR"
    install -m 755 "$SERVER_BIN" "$BIN_DIR/hufu-server"

    mkdir -p "$HOME/.config/systemd/user"
    install -m 644 "$ROOT/platform/linux/systemd/hufu-server.service" \
        "$HOME/.config/systemd/user/hufu-server.service"

    mkdir -p "$HOME/.local/share/applications"
    install -m 644 "$ROOT/platform/linux/desktop/hufu-settings.desktop" \
        "$HOME/.local/share/applications/hufu-settings.desktop"

    if command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then
        systemctl --user daemon-reload
        systemctl --user enable hufu-server.service
        # 【必须 restart】enable --now 对已运行服务不重启——升级二进制后
        # 旧进程继续服务（/api/platform 曾因此 404）。
        systemctl --user restart hufu-server.service
        echo '  ✓ systemd user 服务已重启（hufu-server.service）'
    else
        echo '  · 无 systemd user 会话：请手动运行 hufu-server（~/.local/bin/hufu-server）'
    fi
}

# ── 4) 系统级安装（fcitx5 addon） ──────────────────────────────────────────
install_system() {
    say '⑤ 安装 fcitx5 addon（需要 sudo）'
    sudo install -Dm755 "$ADDON_SO" /usr/lib/fcitx5/libhufu.so
    sudo install -Dm644 "$ROOT/platform/linux/hufu-addon/conf/hufu.addon.conf" \
        /usr/share/fcitx5/addon/hufu.conf
    sudo install -Dm644 "$ROOT/platform/linux/hufu-addon/conf/hufu.inputmethod.conf" \
        /usr/share/fcitx5/inputmethod/hufu.conf
    # 自检：两个 conf 必须各归其位（历史上出现过把 addon conf 覆盖到
    # inputmethod 的手误——fcitx5 会看不到输入法条目）
    grep -q '^\[Addon\]' /usr/share/fcitx5/addon/hufu.conf \
        || die '/usr/share/fcitx5/addon/hufu.conf 内容异常'
    grep -q '^\[InputMethod\]' /usr/share/fcitx5/inputmethod/hufu.conf \
        || die '/usr/share/fcitx5/inputmethod/hufu.conf 内容异常（被覆盖？）'
    echo '  ✓ /usr/lib/fcitx5/libhufu.so + /usr/share/fcitx5/{addon,inputmethod}/hufu.conf'
}

if [[ "$DATA_ONLY" == 1 ]]; then
    assemble_data
    say '数据装配完成（--data-only）'
    exit 0
fi

assemble_data
install_user
if [[ "$NO_SYSTEM" == 0 ]]; then
    install_system
else
    say '⑤ 跳过系统级 addon 安装（--no-system）'
    echo "  之后执行：sudo install -Dm755 $ADDON_SO /usr/lib/fcitx5/libhufu.so"
    echo "            sudo install -Dm644 $ROOT/platform/linux/hufu-addon/conf/hufu.addon.conf /usr/share/fcitx5/addon/hufu.conf"
    echo "            sudo install -Dm644 $ROOT/platform/linux/hufu-addon/conf/hufu.inputmethod.conf /usr/share/fcitx5/inputmethod/hufu.conf"
fi

say '完成 ✔'
cat <<'EOF'
后续：
  1) 重启 fcitx5：        fcitx5 -r -d
  2) 配置工具里加「虎符」：fcitx5-configtool → 输入法 → 添加「虎符」
  3) 设置页：             应用菜单搜「虎符设置」，或浏览器开 http://127.0.0.1:4390/
  4) 引擎状态：           systemctl --user status hufu-server
EOF
