#!/usr/bin/env bash
# 虎符输入法 · Linux 安装
#
# 系统级：fcitx5 addon（/usr/lib/fcitx5/libhufu.so + /usr/share/fcitx5/{addon,inputmethod}/hufu.conf）
# 用户级：hufu-server（~/.local/bin）+ 数据（~/.local/share/hufu）+ systemd user 服务 + 设置入口
#
# 用法（仓库根目录执行）：platform/linux/install.sh [选项]
#   默认数据/资源来源 = 仓库 assets/（自足，无需外部下载；模型除外）
#   --from <目录>     改用外部虎码资源目录（码表；资源另从虎爪 7z 取）
#   --tigerclaw <7z>  虎爪安装包路径（外部源模式的注释/拆分/反查/符号/音效）
#   --no-build 跳过构建 | --no-system 跳过系统级 addon(sudo) | --no-assets 跳过资源装配
#   --data-only 只装配数据+资源 | --assets-only 只装配资源
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
ASSETS_DIR="$ROOT/assets"
# 外部数据源（仅 --from 或 assets/ 缺失时使用；无默认值——本机路径不该进仓库）
SRC="${HUFU_DATA_SRC:-}"
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
NO_ASSETS=0
ASSETS_ONLY=0
SRC_OVERRIDE=0
TC7Z=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --from) SRC="$2"; SRC_OVERRIDE=1; shift 2 ;;
        --from=*) SRC="${1#--from=}"; SRC_OVERRIDE=1; shift ;;
        --tigerclaw) TC7Z="$2"; shift 2 ;;
        --tigerclaw=*) TC7Z="${1#--tigerclaw=}"; shift ;;
        --no-build) DO_BUILD=0; shift ;;
        --data-only) DATA_ONLY=1; DO_BUILD=0; shift ;;
        --no-system) NO_SYSTEM=1; shift ;;
        --no-assets) NO_ASSETS=1; shift ;;
        --assets-only) ASSETS_ONLY=1; DO_BUILD=0; shift ;;
        -h|--help) sed -n '2,15p' "$0"; exit 0 ;;
        *) echo "未知参数: $1（--help 看用法）" >&2; exit 2 ;;
    esac
done

say() { printf '\n\033[1m%s\033[0m\n' "$*"; }
die() { printf '✗ %s\n' "$*" >&2; exit 1; }

# 数据/资源来源：默认仓库 assets/；--from 走外部目录（资源再从虎爪 7z 取）
USE_ASSETS=1
if [[ "$SRC_OVERRIDE" == 1 ]]; then
    USE_ASSETS=0
elif [[ ! -d "$ASSETS_DIR/码表" ]]; then
    echo "• 仓库 assets/ 缺失（不完整检出？）——将使用外部数据源" >&2
    USE_ASSETS=0
fi
if [[ "$USE_ASSETS" == 0 && -z "$SRC" ]]; then
    die 'assets/ 缺失且未指定外部源：请用 --from <虎码资源目录> 指定码表目录'
fi

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

# 首次安装写默认配置（含资源就位后的推荐值；已存在则保持不动）。
write_default_config() {
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
        echo '  ✓ 生成 数据/config.json（默认方案：虎整句；反查=拼音；注释/拆分显示开；中英切换交由 fcitx5 布局）'
    else
        echo '  · 已存在 数据/config.json，保持不动（如需切换默认方案/开关请在设置页操作）'
    fi
}

# ── 2) 数据装配（码表 + 转换词典）──────────────────────────────────────────
assemble_data() {
    mkdir -p "$DATA_DIR" "$HUFU_ROOT/码表" "$HUFU_ROOT/模型"

    if [[ "$USE_ASSETS" == 1 ]]; then
        say '③ 装配数据（来源：仓库 assets/）'
        cp -a "$ASSETS_DIR/码表/." "$HUFU_ROOT/码表/"
        cp -a "$ASSETS_DIR/数据/." "$DATA_DIR/"
        echo '  ✓ 码表（虎整句/虎码字词/虎码单字/多多B）+ 数据（注释/拆分/反查/转换词典/音效）'
        write_default_config
        return 0
    fi

    say "③ 装配数据（来源：$SRC）"
    [[ -d "$SRC" ]] || die "数据源不存在：$SRC"
    local rime="$SRC/虎码秃版 小狼毫（Win）"
    local opencc="$rime/opencc"
    local duoduo="$SRC/publish/定制/b"

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

    write_default_config
}

# ── 3) 资源装配（注释/拆分/反查/符号/音效）────────────────────────────────
# 默认随 assets/ 一起装配；仅外部源模式（--from）从虎爪 7z 按需流式取
# 单文件（7z e -so，不解整包）。缺 7z/找不到包时跳过（不阻断安装）。
assemble_assets() {
    if [[ "$USE_ASSETS" == 1 ]]; then
        echo '  · 资源已随仓库 assets/ 装配（跳过虎爪 7z）'
        return 0
    fi
    local tc7z="${TC7Z:-}"
    if [[ -z "$tc7z" ]]; then
        tc7z="$(ls "$SRC"/虎爪输入法-*.7z 2>/dev/null | head -1 || true)"
    fi
    if [[ -z "$tc7z" || ! -f "$tc7z" ]]; then
        echo '  • 未找到虎爪 7z（--tigerclaw <路径> 可指定），跳过资源装配'
        return 0
    fi
    if ! command -v 7z >/dev/null 2>&1; then
        echo '  • 缺少 7z（Arch: p7zip；Debian/Ubuntu: p7zip-full），跳过资源装配'
        return 0
    fi
    say "④ 资源装配（来源：$(basename "$tc7z")）"

    local tmp
    tmp="$(mktemp -d)"
    local extract
    extract() { # <7z 内路径> <目标文件>；缺失/空内容即报错
        local dst="$2"
        mkdir -p "$(dirname "$dst")"
        7z e -y -so "$tc7z" "$1" > "$dst" 2>/dev/null || true
        [[ -s "$dst" ]] || die "7z 内缺文件或内容为空：$1"
    }

    # 全局资产（引擎 apply_global_assets 从 数据/注释、数据/拆分、数据/拼音反查 读）
    extract 'TigerClaw/码表/虎码字词/1拼音.注释' "$DATA_DIR/注释/拼音.注释"
    extract 'TigerClaw/码表/虎码字词/unicode.注释' "$DATA_DIR/注释/unicode.注释"
    extract 'TigerClaw/码表/虎码字词/虎码.拆分' "$DATA_DIR/拆分/虎码.拆分"
    extract 'TigerClaw/拼音反查码表/拼音.txt' "$DATA_DIR/拼音反查/拼音.txt"
    # 符号表：Schema 按方案目录读（快符/常用符号/一简符号）——取一份分发到各方案
    extract 'TigerClaw/码表/虎码字词/快符.txt' "$tmp/快符.txt"
    extract 'TigerClaw/码表/虎码字词/常用符号.txt' "$tmp/常用符号.txt"
    extract 'TigerClaw/码表/虎码单字/一简符号.txt' "$tmp/一简符号.txt"
    local d
    for d in "$HUFU_ROOT/码表"/*/; do
        [[ -d "$d" ]] || continue
        cp -f "$tmp/快符.txt" "$tmp/常用符号.txt" "$tmp/一简符号.txt" "$d"
    done
    rm -rf "$tmp"
    # 音效（开关默认关；标签→文件按语义映射，设置页可开+试听）
    extract 'TigerClaw/sounds/KeyNormal.wav' "$DATA_DIR/音效/key.wav"
    extract 'TigerClaw/sounds/KeyPop.wav' "$DATA_DIR/音效/select.wav"
    extract 'TigerClaw/sounds/KeySpace.wav' "$DATA_DIR/音效/commit.wav"
    extract 'TigerClaw/sounds/KeyFunc.wav' "$DATA_DIR/音效/page.wav"
    echo '  ✓ 注释/拆分/反查/符号/音效 已就位'
    if command -v jq >/dev/null 2>&1 && [[ -f "$DATA_DIR/config.json" ]]; then
        if [[ "$(jq -r '.reverse.scheme // ""' "$DATA_DIR/config.json" 2>/dev/null)" == "" ]]; then
            echo '  · 提示：反查方案为空——如需拼音反查，请在设置页把「反查方案」设为 拼音'
        fi
    fi
}

# ── 4) 用户级安装（引擎 + 服务 + 设置入口） ────────────────────────────────
install_user() {
    say '⑤ 安装 hufu-server + systemd user 服务 + 设置入口'
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

# ── 5) 系统级安装（fcitx5 addon） ──────────────────────────────────────────
install_system() {
    say '⑥ 安装 fcitx5 addon（需要 sudo）'
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

if [[ "$ASSETS_ONLY" == 1 ]]; then
    assemble_assets
    say '资源装配完成（--assets-only）'
    exit 0
fi

if [[ "$DATA_ONLY" == 1 ]]; then
    assemble_data
    [[ "$NO_ASSETS" == 1 ]] || assemble_assets
    say '数据装配完成（--data-only）'
    exit 0
fi

assemble_data
[[ "$NO_ASSETS" == 1 ]] || assemble_assets
install_user
if [[ "$NO_SYSTEM" == 0 ]]; then
    install_system
else
    say '⑥ 跳过系统级 addon 安装（--no-system）'
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
