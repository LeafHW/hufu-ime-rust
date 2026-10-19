<!-- SPDX-FileCopyrightText: 2026 明雅流风 <crrvx@outlook.com> -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# 虎符 · Linux（fcitx5 前端）

虎符输入法的 Linux 前端：fcitx5 addon（`libhufu.so`）+ 引擎守护进程 `hufu-server`。
与 Windows / macOS 共用引擎、码表与资源；Linux 侧负责按键转发、fcitx5 原生候选面板、
状态菜单与加词/加权/方案切换等原生交互。

```text
按键 → fcitx5 addon → $XDG_RUNTIME_DIR/hufu-ime.sock → hufu-server → 上屏 / 候选
```

## 功能

- **方案**：虎整句（默认）、虎码字词、虎码单字、多多B；设置页或状态区「切换方案」切换，改码表后可热重载
- **整句**：用 `-m` 安装 ngram/Qwen3 模型后启用；可开神经重排
- **反查**：`` ` `` + 拼音反查；`~` 查看光标左侧汉字的拼音/虎码/拆分
- **候选**：fcitx5 原生面板；数字/`;`/`'` 选重，`-=` 翻页；可显示预编辑
- **皮肤**：`-t` 安装 19 套 fcitx5 主题，在 fcitx5 配置 → 外观 → 主题中选择
- **其他**：全角标点、OpenCC 简繁、emoji、按键音
- **状态区菜单**：候选置顶/前移/后移/置底/删除/与下条对调、加词、加权、个人存储表、切换方案、重载码表、打开方案文件夹、打开设置页、导出码表、音效
- **加词/加权**：状态区「加词…」「加权…」原生表单、设置页「方案 → 加词/加权」表单，或输入 `/jc`（加词）/`/jq`（加权）；加词写当前方案 `用户调整.txt`，加权写 `补充语料.txt` 并后台重载整句模型；个人存储表用系统编辑器打开
- **中英**：交给 fcitx5 键盘布局（Shift / CapsLock 不切中英）
- **设置**：应用菜单「虎符设置」，或浏览器打开 <http://127.0.0.1:4390/>

当前已在隔离 fcitx5 + DBus 实例中验证按键、候选、上屏与 Web 加词链路；
系统级安装后的状态菜单/原生对话框全量回归仍待真机执行。SSF 等 Windows 独占皮肤能力
不在 Linux 端实现，Linux 使用 fcitx5 原生主题。

## 安装

依赖：Rust 工具链、CMake ≥ 3.20、C++20 编译器、`Fcitx5Core` ≥ 5.1.15
（Arch `fcitx5`、Fedora `fcitx5-devel`、Debian/Ubuntu `libfcitx5core-dev`）及 fcitx5。
自动安装模型另需 `curl`/`wget` 与 `7z`（Debian/Ubuntu 为 `p7zip-full`）。

platform/linux/install.sh 选项：
- -s  系统级 addon（sudo）+ 用户运行时
- -u  用户级 addon（无需 sudo）
- -t  安装/更新主题（可单独，也可与 -s/-u 组合）
- -m  下载并安装模型（约 880MB）

```sh
# 推荐做法
platform/linux/install.sh -umt # 用户级 一键全量安装
fcitx5 -r -d       # 重启 fcitx5（或以发行版的方式）
fcitx5-configtool  # 手动添加「虎符」（或以发行版的方式）
```

空参、`-h`、`--help` 显示用法；安装时须显式指定 `-s` 或 `-u`，或用 `-t`/`-m`。
`-s/-u` **不隐含 `-t`/`-m`**：完整安装只装配 `assets/码表`、`assets/数据`、`hufu-server`、
systemd user 服务、桌面入口与图标；主题和模型都只在显式给出对应开关时处理。
`-t` 可单独使用（不构建、不装 addon），把 `platform/linux/themes/hufu-*` 装到
`~/.local/share/fcitx5/themes/`；`-m` 也可单独使用，只下载/安装模型。

- `-s/--system`：addon 与两个 conf 装到系统目录（需要 sudo）
- `-u/--user`：addon 与 conf 装到 `~/.local/share/fcitx5/`，`Library=` 指向同目录绝对路径（无需 sudo）

安装脚本会同时装配 `assets/码表/` 与 `assets/数据/`；只跑 `cmake --install` 会缺数据。
默认落点遵守 XDG：数据 `~/.local/share/hufu/`，服务端 `~/.local/bin/`。

整句/重排模型约 880MB，不随仓库分发；缺模型为纯码表模式：

```sh
platform/linux/install.sh -m
```

默认从 GitHub Release 的
[`default.7z`](https://github.com/LeafHW/hufu-ime-rust/releases/download/%E6%A8%A1%E5%9E%8B/default.7z)
下载，验证后解压到 `~/.local/share/hufu/模型/`（服务端按 `.bin`/`.gguf` 探测，文件名不限）。已有完整模型时跳过，
`HUFU_MODEL_FORCE=1` 可强制重装；
镜像可通过 `HUFU_MODEL_URL` 覆盖。也可手动把「模型」文件夹放进 `~/.local/share/hufu/`。

卸载：

```sh
platform/linux/uninstall.sh -s           # 只清系统级
platform/linux/uninstall.sh -u           # 只清用户级，保留模型与主题
platform/linux/uninstall.sh -m           # 只删模型
platform/linux/uninstall.sh -t           # 只清主题
platform/linux/uninstall.sh -ut          # 用户级 + 主题
platform/linux/uninstall.sh -s -u        # 系统级 + 用户级
platform/linux/uninstall.sh -u -m        # 用户级 + 模型
platform/linux/uninstall.sh -s -u -m     # 系统级 + 用户级 + 模型（主题保留）
platform/linux/uninstall.sh -s -t        # 系统级 + 主题
platform/linux/uninstall.sh -sum         # 同上（短选项可合并；主题保留）
```

空参、`-h`、`--help` 显示卸载用法；至少指定 `-s`、`-u`、`-m`、`-t` 之一。

其余选项见 `uninstall.sh --help`。

## 作者

- 前端（`platform/linux/**`）：© 2026 明雅流风 <crrvx@outlook.com>，GPL-3.0-or-later。
- 引擎、码表与资源来自虎符上游；码表/注释/拆分/反查/音效出自虎码官方与虎爪 TigerClaw（GPL-3.0）。
- 逐文件来源与许可见 [docs/asset-sources.md](../../docs/asset-sources.md)。
