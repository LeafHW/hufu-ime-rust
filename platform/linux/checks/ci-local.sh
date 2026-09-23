#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 明雅流风 <crrvx@outlook.com>
# SPDX-License-Identifier: GPL-3.0-or-later

# 虎符 linux 前端自测：一条命令跑完 CI 的检查部分（本地自测 / 上游评审前核对）。
#
#   bash platform/linux/checks/ci-local.sh
#
# 覆盖（与 .github/workflows/ci.yml 的检查一一对应）：
#   ① 依赖缓存探测   ② 前端单测   ③ 构建 addon   ④ C ABI 符号一致
#   ⑤ DESTDIR 安装布局   ⑥ 逐脚本 bash -n   ⑦ assets 台账   ⑧ dry-run 冒烟
#
# 与 workflow 的职责划分：
#   · workflow 管**环境**——apt 装依赖（cmake / g++ / make / binutils / libfcitx5core-dev）、
#     Rust 工具链、`cargo fetch --locked`（联网预热依赖缓存）；
#   · 本脚本管**检查**——只读或可重复：不装包、不 sudo、不写用户目录。
#     唯一会写的是构建目录 `platform/linux/build`、`platform/linux/target` 与
#     DESTDIR 暂存 `/tmp/stage`：都是构建产物（前两者在 .gitignore 里），
#     与手工构建落的是同一份东西。
#
# 为什么构建前必须先 `cargo fetch`：`hufu-addon/CMakeLists.txt` 里走的是
# `cargo build --offline`（Cargo.lock 已提交，离线环境不至于挂在索引刷新上）；
# 全新机器缓存为空时 `--offline` 解析不出 serde_json 等依赖。本脚本**不联网**，
# 只跑一条 `cargo fetch --offline --locked` 做只读探测：过了说明缓存够用，
# 没过就把补救命令打出来（见 ①）。
#
# 静态检查两项也在本脚本里（⑨ 排版 / ⑩ clippy）：这两个 crate 的代码是我们自己的，
# 排版与 lint 就该由守卫钉住，而不是靠自觉；`hufu-fcitx5-client` 的 C ABI 导出统一是
# `unsafe extern "C" fn`（ABI 与 `hufu_abi.h` 不变），因此 clippy 的裸指针告警已消除。
#
# 退出码：全部通过 0；任一项失败即 1（失败即停，不留半套结论）。
set -euo pipefail

# 取真实路径（pwd -P）：从软链入口（如 _external/ 下的快捷方式）跑时，日志与传给
# cmake 的路径只有一套，不会同一次运行里出现两种写法。
root=$(cd "$(dirname "$0")/../../.." && pwd -P)
cd "$root"

build=platform/linux/build   # 与 CMakeLists / install.sh / workflow 同一构建目录
stage=/tmp/stage             # DESTDIR 暂存：免 sudo 核对安装布局
xdg_tmp=/tmp/hufu-ci         # dry-run 用的临时 XDG_DATA_HOME（跑完必须仍不存在）

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

step() { printf '\n\033[1m── %s\033[0m\n' "$*"; }
ok() { printf '  ✓ %s\n' "$*"; }
# 失败即停。`::error::` 会被 GitHub Actions 渲染成错误注解，本地只是普通一行。
fail() {
    printf '  ✗ %s\n' "$*" >&2
    printf '::error::%s\n' "$*" >&2
    exit 1
}

step '① 依赖缓存探测（后面的构建走 cargo build --offline）'
if (cd platform/linux && cargo fetch --offline --locked) >/dev/null 2>&1; then
    ok '缓存够用：--offline 能解析全部依赖'
else
    fail '本地 cargo 缓存缺本工作区的依赖：先在 platform/linux/ 跑一次 cargo fetch --locked（联网），再重跑本脚本'
fi

step '② 前端单测（hufu-fcitx5-client）'
if ! test_out=$(cd platform/linux && cargo test -p hufu-fcitx5-client 2>&1); then
    printf '%s\n' "$test_out"
    fail 'cargo test 非 0 退出'
fi
printf '%s\n' "$test_out" | grep -E '^test result:' || true
if printf '%s\n' "$test_out" | grep -q 'test result: FAILED'; then
    printf '%s\n' "$test_out"
    fail '有失败用例（test result: FAILED）'
fi
printf '%s\n' "$test_out" | grep -q 'test result: ok' || fail '没看到 test result: ok 汇总行'
printf '%s\n' "$test_out" | grep -q '0 failed' || fail '汇总行里不是 0 failed'
ok "单测通过：$(printf '%s\n' "$test_out" | grep -c '^test .* \.\.\. ok$') 个用例，0 failed"

step '③ 构建 addon（Rust staticlib + C++ 薄壳）'
cmake -S platform/linux/hufu-addon -B "$build" \
    -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr || fail 'cmake 配置失败'
cmake --build "$build" -j"$(nproc)" || fail 'cmake 构建失败'
[ -f "$build/libhufu.so" ] || fail "构建没有产出 $build/libhufu.so"
ok "产出 $build/libhufu.so（$(wc -c <"$build/libhufu.so") 字节）"

step '④ C ABI 符号一致（hufu_abi.h 声明 ↔ libhufu.so 导出）'
# 两侧都取集合后逐行 diff：少声明（链接期才炸）与多导出（符号泄漏）都能拦下。
nm -D --defined-only "$build/libhufu.so" \
    | awk '$2=="T"||$2=="i"{print $3}' | grep '^hufu_' | sort -u >"$tmp/so.txt" || true
grep -oE '\bhufu_[a-z_0-9]+[[:space:]]*\(' platform/linux/hufu-addon/shell/hufu_abi.h \
    | tr -d ' (' | sort -u >"$tmp/hdr.txt" || true
[ -s "$tmp/so.txt" ] || fail 'libhufu.so 没导出任何 hufu_ 符号（守卫不能空比对）'
[ -s "$tmp/hdr.txt" ] || fail 'hufu_abi.h 没抽出任何声明（守卫不能空比对）'
if ! diff -u "$tmp/hdr.txt" "$tmp/so.txt"; then
    fail 'hufu_abi.h 声明与 libhufu.so 导出不一致（C ABI 漂移：漏声明或多导出）'
fi
ok "符号一一对应：$(wc -l <"$tmp/so.txt") 个"

step '⑤ DESTDIR 安装布局（免 sudo）'
rm -rf "$stage"
DESTDIR="$stage" cmake --install "$build" || fail 'cmake --install 失败'
# addon 落点由 fcitx5 的 FCITX_INSTALL_ADDONDIR 决定（Debian/Ubuntu 是 multiarch 的
# lib/<triplet>/fcitx5），故按实际落点核对，不写死 lib 还是 lib64。
addon_so=$(find "$stage" -name libhufu.so -print -quit)
[ -n "$addon_so" ] || fail '安装布局里没有 libhufu.so'
case "$addon_so" in
    */fcitx5/libhufu.so) ok "addon 落点：$addon_so" ;;
    *) fail "libhufu.so 不在 fcitx5 addon 目录：$addon_so" ;;
esac
for f in usr/share/fcitx5/addon/hufu.conf usr/share/fcitx5/inputmethod/hufu.conf; do
    [ -f "$stage/$f" ] || fail "安装布局缺 /$f"
done
ok '两个 hufu.conf 各归其位（addon / inputmethod）'

step '⑥ 逐脚本语法检查（bash -n）'
# 必须逐个查：`bash -n a b c` 只检查第一个文件，多出来的参数被当成位置参数吃掉。
shopt -s nullglob
checked=0
for f in platform/linux/*.sh platform/linux/checks/*.sh; do
    bash -n "$f" || fail "语法错误：$f"
    checked=$((checked + 1))
done
shopt -u nullglob
[ "$checked" -gt 0 ] || fail '没找到任何 shell 脚本（守卫不能空跑）'
ok "$checked 个脚本语法通过"

step '⑦ assets 台账（字节 + sha256）'
bash platform/linux/checks/check-assets.sh || fail 'assets/ 与 assets/MANIFEST 不一致'

step '⑧ install / uninstall --dry-run 冒烟（临时 XDG_DATA_HOME，不落任何改动）'
rm -rf "$xdg_tmp"
if ! XDG_DATA_HOME="$xdg_tmp" bash platform/linux/install.sh --dry-run >"$tmp/install.log" 2>&1; then
    tail -20 "$tmp/install.log"
    fail 'install.sh --dry-run 非 0 退出'
fi
if ! XDG_DATA_HOME="$xdg_tmp" bash platform/linux/uninstall.sh --dry-run >"$tmp/uninstall.log" 2>&1; then
    tail -20 "$tmp/uninstall.log"
    fail 'uninstall.sh --dry-run 非 0 退出'
fi
if [ -e "$xdg_tmp" ]; then
    fail "dry-run 落了改动：$xdg_tmp 被创建（dry-run 应当只打印，不产生副作用）"
fi
ok "两个 dry-run 都是 exit 0，且 $xdg_tmp 未被创建"

step '⑨ 排版（cargo fmt -p hufu-fcitx5-client --check）'
if ! fmt_out=$(cd platform/linux && cargo fmt -p hufu-fcitx5-client --check 2>&1); then
    printf '%s\n' "$fmt_out"
    fail 'rustfmt 有差异：在 platform/linux 跑 cargo fmt -p hufu-fcitx5-client 后重试'
fi
ok '排版干净'

step '⑩ 静态检查（cargo clippy -p hufu-fcitx5-client --all-targets -- -D warnings）'
if ! clippy_out=$(cd platform/linux &&
    cargo clippy -p hufu-fcitx5-client --all-targets -- -D warnings 2>&1); then
    printf '%s\n' "$clippy_out"
    fail 'clippy 有告警（-D warnings 视作错误）'
fi
ok 'clippy 无告警'

printf '\n\033[1m全部通过 ✔\033[0m\n'
