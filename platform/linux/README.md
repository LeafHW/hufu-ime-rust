<!-- SPDX-FileCopyrightText: 2026 明雅流风 <crrvx@outlook.com> -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# 虎符 · Linux（fcitx5 前端）

虎符输入法的 Linux 前端：fcitx5 addon（`libhufu.so`）+ 引擎守护进程 `hufu-server`。
薄壳架构与 Windows TSF / macOS IMK 一致——输入法只做事件转发与绘制，码表、整句、
用户词与数据全在引擎侧；两端通过 Unix socket 说同一套帧协议（4 字节小端长度 + JSON）：

```
按键 → fcitx5 addon（C++ 薄壳 + Rust 客户端）
     → $XDG_RUNTIME_DIR/hufu-ime.sock → hufu-server 引擎 → {outcome, state}
     → 上屏 commitString / 组段 preedit / 候选面板
```

内核引擎、码表、模型与随包资源与 Windows / macOS 端同源，本目录只做 Linux 适配；
运行时数据随仓库 `assets/` 分发，装完即用（仅整句/重排模型体积过大，需另行获取）。

## 功能

| 功能 | 说明 |
|---|---|
| 方案 | 虎整句（默认）、虎码字词、虎码单字、多多B；托盘/设置页可切换，改码表后「重载码表」热生效 |
| 整句 | 放入 ngram 模型即启用（默认方案）；可开 Qwen3 神经重排（停顿后介入，见设置页「整句」） |
| 反查 | `` ` `` + 拼音（全拼）反查，候选旁带虎码编码注释 |
| 字反查 | 按 `~` 看光标左侧汉字的拼音/虎码/拆分两排提示；方向键移动光标实时跟随 |
| 候选 | fcitx5 自带面板（跟随主题）；横排/竖排/跟随全局；候选窗内显示预编辑（默认开）；点击上屏 |
| 皮肤 | 随包 19 套（与引擎皮肤同名，含 7 套亮色），以 fcitx5 主题形式装到 `~/.local/share/fcitx5/themes/`；在 fcitx5 配置 → 外观 → 主题里选（主题全局生效，模糊/动效等自绘能力不可表达，见 platform/linux/theme/README.md） |
| 选重翻页 | 数字选重、`;` 次选、`'` 三选、`-=` 翻页（设置页可改，支持自定义选重键） |
| 注释 | 拆分 / 拼音 / unicode 分区注释（整句态下按引擎口径不显示） |
| 标点与转换 | 全角标点、OpenCC 简繁、emoji 变体、自定义符号表 |
| 按键音效 | 四类按键音（key/select/commit/page）；音量与开关即改即生效；需装 `paplay`/`pw-play`/`aplay`/`play` 任一 |
| 状态区菜单 | 「虎符」子菜单：重载码表 / 打开方案文件夹 / 按键音效 / 引擎状态 / 候选窗显示预编辑 |
| 中英切换 | 由 fcitx5 键盘布局提供（引擎不自带英文输入；Shift 单击 / CapsLock 不切中英） |
| 设置 | 应用菜单「虎符设置」，或浏览器开 <http://127.0.0.1:4390/>（systemd user 服务托管） |

## 安装 / 卸载

依赖：Rust 工具链、CMake ≥ 3.20、C++20 编译器、`Fcitx5Core` 开发包（≥ 5.1.15；
Arch `fcitx5`、Fedora `fcitx5-devel`、Debian/Ubuntu `libfcitx5core-dev`）与 fcitx5 本体。

```sh
# 安装：构建 + 装配数据 + 用户级服务 + 系统级 addon（中途要 sudo 密码）
platform/linux/install.sh

# 先预览：逐条列出将要执行的改动性动作（构建/拷贝/安装/sudo/systemctl），不落任何改动
platform/linux/install.sh --dry-run
platform/linux/uninstall.sh --dry-run

# 装完
fcitx5 -r -d            # 重启 fcitx5
fcitx5-configtool       # 输入法 → 添加「虎符」

# 卸载：除「模型」外全部删净（结束时给出手动删除模型的命令）
platform/linux/uninstall.sh
platform/linux/uninstall.sh --purge     # 连「模型」一起删
```

安装产物：系统级 `/usr/lib/fcitx5/libhufu.so` 与 `/usr/share/fcitx5/{addon,inputmethod}/hufu.conf`；
用户级 `~/.local/bin/hufu-server`、`~/.config/systemd/user/hufu-server.service`、
`~/.local/share/applications/hufu-settings.desktop`、`~/.config/fcitx5/conf/hufu.conf`
与自带图标 `~/.local/share/icons/hicolor/{scalable,48x48,22x22}/apps/hufu.{svg,png}`（输入法条目、
状态区菜单与桌面项都用它）、19 套 fcitx5 皮肤主题 `~/.local/share/fcitx5/themes/hufu-*`；
数据 `~/.local/share/hufu/{码表,模型,数据}`（`数据/` 存配置、皮肤、用户词、音效与诊断）。
落点遵守 XDG：`XDG_DATA_HOME` / `XDG_CONFIG_HOME` 改了，数据与配置跟着走；可执行文件用
`XDG_BIN_HOME`（默认 `~/.local/bin`——用户级 bin 的标准位置，`systemd-path user-binaries`
的输出），改了它安装脚本会同步改写 systemd 单元的 `ExecStart`。

**模型（可选）**：整句与神经重排模型约 880MB，不随仓库分发——安装脚本结束时以绿色打印
获取网址，下载解压后把「模型」文件夹整个放进 `~/.local/share/hufu/`（引擎自动探测装载；
缺模型即纯码表模式）。

**校验与外部源**：默认从仓库 `assets/` 装配，装配前按 `assets/MANIFEST` 台账校验来源、
装后逐项核对落盘文件（字节 + sha256）；`--from <虎码资源目录>` / `--tigerclaw <虎爪7z>`
改用外部数据源（内容由外部决定，跳过台账核对）。其余选项见 `install.sh --help`。

## 作者声明

- **前端**（`platform/linux/**`）：© 2026 明雅流风 <crrvx@outlook.com>，
  许可 GPL-3.0-or-later（逐文件 SPDX 头）。
- **引擎、码表与随包资源**：来自虎符（hufu-ime-rust）上游；码表、注释、拆分、反查、
  音效等资源出自虎码官方发布（<https://huma.ysepan.com>）与虎爪 TigerClaw
  （<https://github.com/lvyww/tigerclaw>，GPL-3.0）。
- 逐文件来源、版本与许可见 [docs/asset-sources.md](../../docs/asset-sources.md)；
  打包与再分发请一并保留上述声明与许可。
