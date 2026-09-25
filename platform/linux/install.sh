#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 明雅流风 <crrvx@outlook.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# 虎符输入法 · Linux 安装
#
# 系统级：fcitx5 addon（/usr/lib/fcitx5/libhufu.so + /usr/share/fcitx5/{addon,inputmethod}/hufu.conf）
# 用户级：hufu-server（~/.local/bin）+ 数据（~/.local/share/hufu）+ systemd user 服务 + 设置入口
#
# 默认数据/资源来源 = 仓库 assets/（自足，无需外部下载；模型除外）；
# 外部源（--from / --tigerclaw）只作覆盖，内容由外部数据源决定。
#
# 用法与选项见 usage()（platform/linux/install.sh --help）。
set -euo pipefail

usage() {
    cat <<'EOF'
虎符输入法 · Linux 安装（在仓库根目录执行）
用法：platform/linux/install.sh [选项]

  --from <目录>     改用外部虎码资源目录（码表；资源另从虎爪 7z 取）
  --tigerclaw <7z>  虎爪安装包路径（外部源模式的注释/拆分/反查/符号/音效）
  --no-build        跳过构建（用已有产物）| --no-system 跳过系统级 addon（sudo 那步）
  --no-assets       跳过资源装配 | --data-only 只装配数据+资源 | --assets-only 只装配资源
  --dry-run         只打印将要执行的每一个改动性动作（构建/拷贝/安装/sudo/systemctl），
                    不产生任何副作用，退出码 0
  -h, --help        显示本用法

默认走仓库 assets/：装配前按 assets/MANIFEST 台账校验来源，装配后按同一清单逐项
核对落盘文件（字节 + sha256）；外部源模式不做清单核对（输出里会说明）。
模型（整句与神经重排）体积大，不随包分发：结束时打印手动获取网址。
EOF
}

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
ASSETS_DIR="$ROOT/assets"
# 外部数据源（仅 --from 或 assets/ 缺失时使用；无默认值——本机路径不该进仓库）
SRC="${HUFU_DATA_SRC:-}"
# 用户级落点：遵守 XDG（与 HUFU_ROOT 同一口径）；卸载脚本里是同一套定义。
XDG_DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
XDG_CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}"
HUFU_ROOT="$XDG_DATA/hufu"
DATA_DIR="$HUFU_ROOT/数据"
BIN_DIR="$XDG_DATA/bin"
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
DRY_RUN=0
DATA_ASSEMBLED=0
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
        --dry-run) DRY_RUN=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "未知参数: $1（--help 看用法）" >&2; exit 2 ;;
    esac
done

say() { printf '\n\033[1m%s\033[0m\n' "$*"; }
die() { printf '✗ %s\n' "$*" >&2; exit 1; }

# 收尾提示的两色（终端且未设 NO_COLOR 时才上色：管道/日志里不留转义码）
if [[ -t 1 && -z "${NO_COLOR:-}" ]]; then
    C_ORANGE=$'\033[38;5;208m'; C_GREEN=$'\033[32m'; C_OFF=$'\033[0m'
else
    C_ORANGE=''; C_GREEN=''; C_OFF=''
fi
hint() { printf '%s%s%s\n' "$C_ORANGE" "$*" "$C_OFF"; }      # 橙色：中文提示
show_code() { printf '%s%s%s\n' "$C_GREEN" "$*" "$C_OFF"; }  # 绿色：网址 / 命令

# 模型（整句 / 神经重排）体积大，不随仓库分发——安装结束时给出获取地址。
MODEL_URL='https://github.com/LeafHW/hufu-ime-rust/releases/tag/模型'
model_hint() {
    hint '模型（整句与神经重排，约 880MB）不随安装包分发，需要时请手动获取：'
    show_code "手动获取模型网址：$MODEL_URL"
    hint "下载解压后把「模型」文件夹整个放进 $HUFU_ROOT/（引擎自动探测装载；缺模型即纯码表模式）"
}

# 图标随包自带（branding/ → 用户图标主题，装完已尽力刷新 icon-theme.cache）：
# 输入法条目 / 状态区菜单 / 桌面项都按主题名 hufu 取用；这里说明两个常见困惑。
icon_hint() {
    hint '图标：本包自带，装到 ~/.local/share/icons/hicolor/（条目、状态区菜单与桌面项都按主题名 hufu 取）'
    hint '      托盘显示「符」而不是图标，是经典界面开了「优先使用文字图标」（「符」是条目的 Label）——'
    hint '      fcitx5-configtool → 附加组件 → 经典界面 里取消勾选即可'
    hint '      重装后仍是旧图标属桌面面板/进程内的图标缓存：重启 fcitx5 与桌面面板'
    show_code '      KDE：kquitapp6 plasmashell && kstart plasmashell'
    hint '      （其它桌面重启各自的面板，或直接注销重登）'
}

# ── dry-run 支撑 ───────────────────────────────────────────────────────────
# 约定：脚本里每一个改动性动作（构建、拷贝、安装、sudo、systemctl、生成配置…）
# 都必须经 run() / run_in() / ok() 之一落地，不允许直接调用——漏一处，--dry-run
# 就少报一次真实副作用，用户据预览做的判断随即失真。
# 只读探测（test / command -v / grep / check-assets 校验）不走这里。
run() { # 在调用点当前目录执行；--dry-run 时只回显命令行
    if [[ "$DRY_RUN" == 1 ]]; then
        printf '  [dry-run]'; printf ' %q' "$@"; printf '\n'
        return 0
    fi
    "$@"
}
run_in() { # <目录> <命令...>：需要特定工作目录的动作（cargo 依赖相对路径）
    local dir="$1"; shift
    if [[ "$DRY_RUN" == 1 ]]; then
        printf '  [dry-run] (cd %q &&' "$dir"; printf ' %q' "$@"; printf ')\n'
        return 0
    fi
    ( cd "$dir" && "$@" )
}
ok() { # 结果提示：dry-run 下不能报「已完成」（此刻什么都没做）
    if [[ "$DRY_RUN" == 1 ]]; then
        printf '  · [dry-run] 将会：%s\n' "$*"
    else
        printf '  ✓ %s\n' "$*"
    fi
}
finish() { # 收尾提示：dry-run 不报「完成」，避免与真实安装混淆
    if [[ "$DRY_RUN" == 1 ]]; then
        say 'dry-run 结束：以上为将要执行的全部动作，未做任何改动'
    else
        say "$1"
    fi
}

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
# 仓库 assets/ 装配前先按台账校验收（字节 + sha256）：挡住误替换/半途拷贝进来的资源。
if [[ "$USE_ASSETS" == 1 ]]; then
    bash "$ROOT/platform/linux/checks/check-assets.sh" \
        || die 'assets/ 台账校验失败（用 platform/linux/checks/check-assets.sh --write 重新登记）'
fi

# ── 1) 构建 ────────────────────────────────────────────────────────────────
if [[ "$DO_BUILD" == 1 ]]; then
    say '① 构建 hufu-server（Rust）'
    run_in "$ROOT/engine" cargo build --release -p hufu-server
    say '② 构建 fcitx5 addon（Rust staticlib + C++ 薄壳）'
    run cmake -S "$ROOT/platform/linux/hufu-addon" -B "$BUILD_DIR" \
        -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr
    run cmake --build "$BUILD_DIR" -j "$(nproc)"
fi

SERVER_BIN="$ROOT/engine/target/release/hufu-server"
ADDON_SO="$BUILD_DIR/libhufu.so"
# 产物前置检查：真实运行缺产物即失败。--dry-run 下构建本身被跳过，产物多半还没
# 生成——降级为提示，预览不该被「还没构建」卡死（退出码仍是 0）。
check_artifact() { # <test 标志> <路径>
    if test "$1" "$2"; then
        return 0
    fi
    if [[ "$DRY_RUN" == 1 ]]; then
        echo "  · [dry-run] 产物尚未就绪：$2（真实运行会先构建；--no-build 时需已构建）"
        return 0
    fi
    die "缺少 $2（先构建，或去掉 --no-build）"
}
check_artifact -x "$SERVER_BIN"
check_artifact -f "$ADDON_SO"

# 首次安装写默认配置（含资源就位后的推荐值；已存在则保持不动）。
write_default_config() {
    if [[ -f "$DATA_DIR/config.json" ]]; then
        echo '  · 已存在 数据/config.json，保持不动（如需切换默认方案/开关请在设置页操作）'
        return 0
    fi
    if [[ "$DRY_RUN" == 1 ]]; then
        echo "  [dry-run] 写入 $DATA_DIR/config.json（默认方案：虎整句；反查=拼音；注释/拆分显示开）"
        return 0
    fi
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
    ok '生成 数据/config.json（默认方案：虎整句；反查=拼音；注释/拆分显示开；中英切换交由 fcitx5 布局）'
}

# ── 2) 数据装配（码表 + 转换词典）──────────────────────────────────────────
assemble_data() {
    DATA_ASSEMBLED=1 # 供装后校验判断「本次是否真往 $HUFU_ROOT 落了清单内的文件」
    run mkdir -p "$DATA_DIR" "$HUFU_ROOT/码表" "$HUFU_ROOT/模型"

    if [[ "$USE_ASSETS" == 1 ]]; then
        say '③ 装配数据（来源：仓库 assets/）'
        run cp -a "$ASSETS_DIR/码表/." "$HUFU_ROOT/码表/"
        run cp -a "$ASSETS_DIR/数据/." "$DATA_DIR/"
        ok '码表（虎整句/虎码字词/虎码单字/多多B）+ 数据（注释/拆分/反查/转换词典/音效）'
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
        run mkdir -p "$dir"
        local f
        for f in "$@"; do
            [[ -f "$f" ]] || die "缺少码表文件：$f"
            run cp -f "$f" "$dir/"
        done
        run cp -f "$ROOT/发行临时/补充语料.txt" "$dir/补充语料.txt"
        ok "码表/$name/ ← $(basename "$1") 等 $# 个文件 + 补充语料"
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
        run mkdir -p "$dir"
        run cp -f "$duoduo/多多B常用字词.txt" "$duoduo/多多B生僻字.txt" "$dir/" 2>/dev/null || true
        run cp -f "$ROOT/发行临时/补充语料.txt" "$dir/补充语料.txt"
        ok "码表/多多B/ ← publish/定制/b/"
    fi

    # OpenCC 转换词典（简繁/emoji 候选滤镜）
    if [[ -d "$opencc" ]]; then
        run mkdir -p "$DATA_DIR/转换词典"
        local f
        for f in STPhrases.txt STCharacters.txt STCharacters_Tu.txt TSPhrases.txt TSCharacters.txt emoji.txt; do
            [[ -f "$opencc/$f" ]] && run cp -f "$opencc/$f" "$DATA_DIR/转换词典/$f"
        done
        ok "数据/转换词典/ ← opencc/"
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
    if [[ "$DRY_RUN" == 1 ]]; then
        tmp='<mktemp -d>'
    else
        tmp="$(mktemp -d)"
    fi
    local extract
    extract() { # <7z 内路径> <目标文件>；缺失/空内容即报错
        local dst="$2"
        run mkdir -p "$(dirname "$dst")"
        if [[ "$DRY_RUN" == 1 ]]; then
            printf '  [dry-run] 7z e -y -so %q %q > %q\n' "$tc7z" "$1" "$dst"
            return 0
        fi
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
        run cp -f "$tmp/快符.txt" "$tmp/常用符号.txt" "$tmp/一简符号.txt" "$d"
    done
    run rm -rf "$tmp"
    # 音效（开关默认关；标签→文件按语义映射，设置页可开+试听）
    extract 'TigerClaw/sounds/KeyNormal.wav' "$DATA_DIR/音效/key.wav"
    extract 'TigerClaw/sounds/KeyPop.wav' "$DATA_DIR/音效/select.wav"
    extract 'TigerClaw/sounds/KeySpace.wav' "$DATA_DIR/音效/commit.wav"
    extract 'TigerClaw/sounds/KeyFunc.wav' "$DATA_DIR/音效/page.wav"
    ok '注释/拆分/反查/符号/音效 已就位'
    if command -v jq >/dev/null 2>&1 && [[ -f "$DATA_DIR/config.json" ]]; then
        if [[ "$(jq -r '.reverse.scheme // ""' "$DATA_DIR/config.json" 2>/dev/null)" == "" ]]; then
            echo '  · 提示：反查方案为空——如需拼音反查，请在设置页把「反查方案」设为 拼音'
        fi
    fi
}

# ── 4) 用户级安装（引擎 + 服务 + 设置入口） ────────────────────────────────
install_user() {
    say '⑤ 安装 hufu-server + systemd user 服务 + 设置入口'
    run mkdir -p "$BIN_DIR"
    run install -m 755 "$SERVER_BIN" "$BIN_DIR/hufu-server"

    run mkdir -p "$XDG_CONFIG/systemd/user"
    run install -m 644 "$ROOT/platform/linux/systemd/hufu-server.service" \
        "$XDG_CONFIG/systemd/user/hufu-server.service"

    run mkdir -p "$XDG_DATA/applications"
    run install -m 644 "$ROOT/platform/linux/desktop/hufu-settings.desktop" \
        "$XDG_DATA/applications/hufu-settings.desktop"

    # 自带图标（platform/linux/branding/：主源位图 + 生成的自包含 SVG / 位图）：装进用户图标主题，
    # 输入法条目（conf 的 Icon）、状态区菜单（menuAction_.setIcon）与桌面项都按主题名 hufu 解析。
    run mkdir -p "$XDG_DATA/icons/hicolor/scalable/apps" \
        "$XDG_DATA/icons/hicolor/48x48/apps" \
        "$XDG_DATA/icons/hicolor/22x22/apps"
    run install -m 644 "$ROOT/platform/linux/branding/hufu.svg" \
        "$XDG_DATA/icons/hicolor/scalable/apps/hufu.svg"
    run install -m 644 "$ROOT/platform/linux/branding/hufu-48.png" \
        "$XDG_DATA/icons/hicolor/48x48/apps/hufu.png"
    run install -m 644 "$ROOT/platform/linux/branding/hufu-22.png" \
        "$XDG_DATA/icons/hicolor/22x22/apps/hufu.png"
    # 有缓存工具就刷一次：某些桌面环境不刷会继续显示旧图标/缺图占位
    if command -v gtk-update-icon-cache >/dev/null 2>&1; then
        run gtk-update-icon-cache -q -t -f "$XDG_DATA/icons/hicolor" 2>/dev/null || true
    fi

    # 皮肤（fcitx5 主题形式）：随包 19 套，由 platform/linux/themes/ 离线转换自引擎皮肤
    # （同 id、同中文名）。装进用户主题目录后，在 fcitx5 配置 → 外观 → 主题 里选。
    if compgen -G "$ROOT/platform/linux/themes/hufu-*" >/dev/null; then
        run mkdir -p "$XDG_DATA/fcitx5/themes"
        for old_theme in "$XDG_DATA"/fcitx5/themes/hufu-*; do
            if [[ -d "$old_theme" ]]; then
                run rm -rf "$old_theme"      # 清旧包：皮肤改了圆角/文件集时不残留
            fi
        done
        run cp -r "$ROOT"/platform/linux/themes/hufu-* "$XDG_DATA/fcitx5/themes/"
    fi

    if command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then
        run systemctl --user daemon-reload
        run systemctl --user enable hufu-server.service
        # 【必须 restart】enable --now 对已运行服务不重启——升级二进制后
        # 旧进程继续服务（/api/platform 曾因此 404）。
        run systemctl --user restart hufu-server.service
        ok 'systemd user 服务已重启（hufu-server.service）'
    else
        echo '  · 无 systemd user 会话：请手动运行 hufu-server（~/.local/bin/hufu-server）'
    fi
}

# ── 5) 系统级安装（fcitx5 addon） ──────────────────────────────────────────
install_system() {
    say '⑥ 安装 fcitx5 addon（需要 sudo）'
    run sudo install -Dm755 "$ADDON_SO" /usr/lib/fcitx5/libhufu.so
    run sudo install -Dm644 "$ROOT/platform/linux/hufu-addon/conf/hufu.addon.conf" \
        /usr/share/fcitx5/addon/hufu.conf
    run sudo install -Dm644 "$ROOT/platform/linux/hufu-addon/conf/hufu.inputmethod.conf" \
        /usr/share/fcitx5/inputmethod/hufu.conf
    if [[ "$DRY_RUN" == 1 ]]; then
        echo '  · [dry-run] 跳过后置自检（两个 conf 各归其位）'
        return 0
    fi
    # 自检：两个 conf 必须各归其位（历史上出现过把 addon conf 覆盖到
    # inputmethod 的手误——fcitx5 会看不到输入法条目）
    grep -q '^\[Addon\]' /usr/share/fcitx5/addon/hufu.conf \
        || die '/usr/share/fcitx5/addon/hufu.conf 内容异常'
    grep -q '^\[InputMethod\]' /usr/share/fcitx5/inputmethod/hufu.conf \
        || die '/usr/share/fcitx5/inputmethod/hufu.conf 内容异常（被覆盖？）'
    ok '/usr/lib/fcitx5/libhufu.so + /usr/share/fcitx5/{addon,inputmethod}/hufu.conf'
}

# ── 装后校验：按台账逐项核对落盘文件 ──────────────────────────────────────
# 装配前用 check-assets.sh 校验来源，装配后用同一份 assets/MANIFEST 核对落点——
# 保证「清单里登记了什么，装完就必须原样在 $HUFU_ROOT 下」。
# 清单路径是仓库相对（assets/码表/…），落地路径去掉 assets/ 前缀：$HUFU_ROOT/码表/…。
# 只按清单核对（清单 → 文件单向）：之后由用户/引擎放进数据目录的文件
# （码表/<方案>/用户调整.txt、数据/user-adjust.log、数据/config.json、模型/ 等）不受影响。
verify_installed() {
    say '装后校验（按 assets/MANIFEST 逐项核对落盘文件）'
    if [[ "$USE_ASSETS" != 1 ]]; then
        echo '  · 外部源模式（--from/--tigerclaw）：内容由外部数据源决定，无台账可比对——跳过'
        return 0
    fi
    if [[ "$DATA_ASSEMBLED" != 1 ]]; then
        echo '  · 本次未装配数据树（--assets-only 且来源为仓库 assets/），无落盘可比对——跳过'
        return 0
    fi
    local manifest="$ASSETS_DIR/MANIFEST"
    if [[ ! -f "$manifest" ]]; then
        echo "  • 缺少 $manifest（不完整检出？）——跳过装后校验" >&2
        return 0
    fi
    if [[ "$DRY_RUN" == 1 ]]; then
        echo "  · [dry-run] 将按 $manifest 逐项核对 $HUFU_ROOT 下的落盘文件（字节 + sha256）"
        return 0
    fi
    local failed=0 checked=0 line sha bytes path dst actual_bytes actual_sha
    while IFS= read -r line; do
        line="${line%$'\r'}"
        [[ "$line" =~ ^[[:space:]]*(#|$) ]] && continue
        sha="$(printf '%s' "$line" | cut -f1)"
        bytes="$(printf '%s' "$line" | cut -f2)"
        path="$(printf '%s' "$line" | cut -f3)"
        if [[ -z "$sha" || -z "$bytes" || -z "$path" ]]; then
            echo "  ✗ 清单行格式不对（应为 sha256/字节/路径 三列）：$line" >&2
            failed=1
            continue
        fi
        dst="$HUFU_ROOT/${path#assets/}"
        checked=$((checked + 1))
        if [[ ! -f "$dst" ]]; then
            echo "  ✗ 未落地：$dst（清单：$path）" >&2
            failed=1
            continue
        fi
        actual_bytes="$(wc -c <"$dst" | tr -d ' ')"
        if [[ "$actual_bytes" != "$bytes" ]]; then
            echo "  ✗ 字节数不符：$dst（清单 $bytes，实际 $actual_bytes）" >&2
            failed=1
        fi
        actual_sha="$(sha256sum "$dst" | cut -d' ' -f1)"
        if [[ "$actual_sha" != "$sha" ]]; then
            echo "  ✗ sha256 不符：$dst（清单 ${sha:0:12}…，实际 ${actual_sha:0:12}…）" >&2
            failed=1
        fi
    done <"$manifest"
    if [[ "$failed" != 0 ]]; then
        die "装后校验失败：$HUFU_ROOT 下的落盘文件与 assets/MANIFEST 不符（已核对 $checked 项）。
  例外路径：--no-assets 只跳过资源装配（数据树照旧按台账核对）；--from/--tigerclaw 改用外部源，
  不做任何清单核对。若 assets/ 本身刚更新过，先跑 platform/linux/checks/check-assets.sh --write 重新登记。"
    fi
    ok "装后校验通过：$checked 个文件与 assets/MANIFEST 逐项一致（字节 + sha256）"
}

if [[ "$ASSETS_ONLY" == 1 ]]; then
    assemble_assets
    verify_installed
    finish '资源装配完成（--assets-only）'
    exit 0
fi

if [[ "$DATA_ONLY" == 1 ]]; then
    assemble_data
    [[ "$NO_ASSETS" == 1 ]] || assemble_assets
    verify_installed
    finish '数据装配完成（--data-only）'
    model_hint
    exit 0
fi

assemble_data
[[ "$NO_ASSETS" == 1 ]] || assemble_assets
verify_installed
install_user
if [[ "$NO_SYSTEM" == 0 ]]; then
    install_system
else
    say '⑥ 跳过系统级 addon 安装（--no-system）'
    echo "  之后执行：sudo install -Dm755 $ADDON_SO /usr/lib/fcitx5/libhufu.so"
    echo "            sudo install -Dm644 $ROOT/platform/linux/hufu-addon/conf/hufu.addon.conf /usr/share/fcitx5/addon/hufu.conf"
    echo "            sudo install -Dm644 $ROOT/platform/linux/hufu-addon/conf/hufu.inputmethod.conf /usr/share/fcitx5/inputmethod/hufu.conf"
fi

finish '完成 ✔'
cat <<'EOF'
后续：
  1) 重启 fcitx5：        fcitx5 -r -d
  2) 配置工具里加「虎符」：fcitx5-configtool → 输入法 → 添加「虎符」
  3) 设置页：             应用菜单搜「虎符设置」，或浏览器开 http://127.0.0.1:4390/
  4) 引擎状态：           systemctl --user status hufu-server
EOF
echo
model_hint
echo
icon_hint
