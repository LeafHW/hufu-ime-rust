<!-- SPDX-FileCopyrightText: 2026 明雅流风 <crrvx@outlook.com> -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# 虎符 · Linux（fcitx5 前端）

虎符输入法的 Linux 前端：fcitx5 addon（`libhufu.so`）+ 引擎守护进程 `hufu-server`。
与 Windows / macOS 共用引擎、码表与资源，Linux 侧只做按键转发与候选面板。

```text
按键 → fcitx5 addon → $XDG_RUNTIME_DIR/hufu-ime.sock → hufu-server → 上屏 / 候选
```

## 功能

- **方案**：虎整句（默认）、虎码字词、虎码单字、多多B；设置页切换，改码表后可热重载
- **整句**：放入 ngram 模型即启用；可开 Qwen3 神经重排
- **反查**：`` ` `` + 拼音反查；`~` 查看光标左侧汉字的拼音/虎码/拆分
- **候选**：fcitx5 原生面板；数字/`;`/`'` 选重，`-=` 翻页；可显示预编辑
- **皮肤**：随包 19 套 fcitx5 主题，在 fcitx5 配置 → 外观 → 主题中选择
- **其他**：全角标点、OpenCC 简繁、emoji、按键音
- **状态区菜单**：候选置顶/前移/后移/置底/删除/与下条对调、加词、重载码表、打开方案文件夹、打开设置页、导出码表、音效
- **加词**：状态区「加词…」原生表单，或设置页「方案 → 加词」表单；写当前方案 `用户调整.txt`
- **中英**：交给 fcitx5 键盘布局（Shift / CapsLock 不切中英）
- **设置**：应用菜单「虎符设置」，或浏览器打开 <http://127.0.0.1:4390/>

## 安装

依赖：Rust 工具链、CMake ≥ 3.20、C++20 编译器、`Fcitx5Core` ≥ 5.1.15
（Arch `fcitx5`、Fedora `fcitx5-devel`、Debian/Ubuntu `libfcitx5core-dev`）及 fcitx5。

```sh
platform/linux/install.sh   # 构建 + 装配码表/数据 + 用户服务 + 系统级 addon
fcitx5 -r -d                # 重启
fcitx5-configtool           # 输入法 → 添加「虎符」
```

安装脚本会同时装配 `assets/码表/` 与 `assets/数据/`；只跑 `cmake --install` 会缺数据。
默认落点遵守 XDG：数据 `~/.local/share/hufu/`，服务端 `~/.local/bin/`。

整句/重排模型约 880MB，不随仓库分发；安装结束会打印获取地址，下载后把「模型」文件夹
放进 `~/.local/share/hufu/` 即可（缺模型为纯码表模式）。

卸载：

```sh
platform/linux/uninstall.sh           # 保留模型
platform/linux/uninstall.sh --purge   # 连模型一起删
```

其余选项见 `install.sh --help`。

## 作者

- 前端（`platform/linux/**`）：© 2026 明雅流风 <crrvx@outlook.com>，GPL-3.0-or-later。
- 引擎、码表与资源来自虎符上游；码表/注释/拆分/反查/音效出自虎码官方与虎爪 TigerClaw（GPL-3.0）。
- 逐文件来源与许可见 [docs/asset-sources.md](../../docs/asset-sources.md)。
