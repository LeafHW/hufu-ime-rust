# Linux 安装 / 资源获取 / 卸载

> fcitx5 前端（KDE、GNOME 等桌面均可，Wayland / X11 皆可）。
> 面向终端用户；前端结构与构建细节见 [`platform/linux/README.md`](../platform/linux/README.md)；
> 数据来源与许可见 [`asset-sources.md`](asset-sources.md)。

## 0. 前置条件

| 项 | 说明 |
|---|---|
| 输入法框架 | fcitx5 |
| 构建工具（安装时用） | Rust、CMake ≥ 3.20、C++20 编译器、Fcitx5Core 开发包<br>（Arch：`fcitx5`；Fedora：`fcitx5-devel`；Debian/Ubuntu：`libfcitx5core-dev`） |
| 常驻方式 | systemd 用户会话（推荐，用于引擎自启与自动重启） |
| 磁盘 | 数据约 20MB（码表 + 资源）；模型可选（另约 1GB） |

## 1. 安装

```sh
git clone https://github.com/LeafHW/hufu-ime-rust
cd hufu-ime-rust
platform/linux/install.sh
```

脚本依次完成四件事：

1. **构建**：`hufu-server`（Rust）与 fcitx5 addon（Rust staticlib + C++ 薄壳）；
2. **装配数据**：把仓库 `assets/`（码表 + 注释/拆分/反查/符号/音效）复制到
   `~/.local/share/hufu/`，**无需任何外部下载**；装配后按 `assets/MANIFEST` 台账逐项
   核对落盘文件（字节 + sha256），任一不符即报错退出；
3. **用户级安装**：`~/.local/bin/hufu-server`、systemd 用户服务、应用菜单「虎符设置」；
4. **系统级安装**：`/usr/lib/fcitx5/libhufu.so` 与 `/usr/share/fcitx5/{addon,inputmethod}/hufu.conf`
   （需要 sudo 授权，脚本会提示输入密码）。

不确定脚本会动什么，可以先预览：

```sh
platform/linux/install.sh --dry-run     # 逐条列出构建/拷贝/安装/sudo/systemctl，不做任何改动
platform/linux/uninstall.sh --dry-run   # 逐条列出要删的文件，不做任何删除
```

### 常用参数

| 参数 | 作用 |
|---|---|
| `--dry-run` | 只打印将要执行的每一个改动性动作（构建/拷贝/安装/sudo/systemctl），不产生任何副作用 |
| `--no-build` | 跳过构建（已构建过，只重新安装） |
| `--no-system` | 跳过系统级安装（无 sudo / 分步执行） |
| `--data-only` | 只装配数据与资源 |
| `--assets-only` | 只装配资源（注释/拆分/反查/符号/音效） |
| `--no-assets` | 跳过资源装配 |
| `--from <目录>` | 改用外部码表资源目录（自备数据源） |
| `--tigerclaw <包>` | 外部资源包（注释/拆分/反查/符号/音效） |

`--from` / `--tigerclaw` 走外部源，内容由外部数据决定，**不做台账核对**（装配前不校验来源、
装后校验显式跳过并在输出里说明）。

### 启用

```sh
fcitx5 -r -d          # 重启 fcitx5，使其发现新输入法
fcitx5-configtool     # 输入法 → 添加「虎符」
```

- 设置页（Web）：应用菜单「虎符设置」，或浏览器打开 `http://127.0.0.1:4390/`；
- 也可在 `fcitx5-configtool` 的「输入法」或「附加组件」页点「虎符 → 配置」
  （与 Web 设置页**同一份配置**，改动即时生效）；
- 英文输入：切到 fcitx5 自带的键盘布局（默认 `Ctrl+Space` 轮换）。
- 字反查：按 `~` 显示光标左侧汉字的拼音（上排）与虎码（下排），需应用支持周边文本；
  触发键在 `fcitx5-configtool` 的「虎符 → 快捷键 → 字反查」里改绑（清空即关闭）。

## 2. 资源获取

| 资源 | 获取方式 |
|---|---|
| 码表（虎码单字/字词/多多B） | **随仓库 `assets/`**，安装脚本自动装配 |
| 注释（拼音/Unicode）、拆分、反查、符号、音效 | 同上 |
| 模型（可选：ngram 整句、Qwen3 重排） | 从 Releases 的「模型文件」发布页下载 `default.7z`（<https://github.com/LeafHW/hufu-ime-rust/releases/tag/模型>，约 880MB）；解压后把「模型」文件夹整体放到 `~/.local/share/hufu/`（成为 `…/hufu/模型/`；引擎自动探测并自动装载）。**默认方案即「虎整句」，模型就位即启用整句**（也可从 Windows 完整安装包的 `模型/` 目录拷贝） |
| 外部源覆盖（换版本/自备数据） | `--from <目录>`、`--tigerclaw <包>`（见上表） |

资源清单、来源、版本与许可见 [`asset-sources.md`](asset-sources.md)。缺模型时引擎为纯码表模式，
输入/候选/符号等功能不受影响。

## 3. 安装位置

| 位置 | 内容 |
|---|---|
| `/usr/lib/fcitx5/libhufu.so` | fcitx5 addon（Rust staticlib + C++ 薄壳） |
| `/usr/share/fcitx5/addon/hufu.conf`、`.../inputmethod/hufu.conf` | addon 与输入法条目 |
| `~/.local/bin/hufu-server` | 引擎守护进程 |
| `~/.config/systemd/user/hufu-server.service` | 用户服务（自启 + 崩溃自动重启） |
| `~/.local/share/applications/hufu-settings.desktop` | 设置入口 |
| `~/.config/fcitx5/conf/hufu.conf` | fcitx5-configtool「虎符」设置页的保存项（引擎映射项与 Web 设置页同源） |
| `~/.local/share/hufu/码表/` | 方案目录（每方案独立：码表、符号、用户调整） |
| `~/.local/share/hufu/模型/` | 模型（可选） |
| `~/.local/share/hufu/数据/` | `config.json`、皮肤、注释/拆分/反查/转换词典/音效、用户词与调整日志 |
| `$XDG_RUNTIME_DIR/hufu-ime.sock` | 前端 ↔ 引擎 IPC（权限 0600） |

## 4. 卸载

```sh
platform/linux/uninstall.sh --dry-run   # 先看会删哪些文件（不做任何删除）
platform/linux/uninstall.sh             # 默认：按台账删已装配的数据，用户数据保留
platform/linux/uninstall.sh --purge     # 连用户数据一起删（整树）
```

无 sudo 环境：`uninstall.sh --no-system`，按脚本提示手动删除 `/usr` 三个文件。

卸载动作：停用并移除 systemd 用户服务 → 删除 `/usr` 三件 → 删除用户级二进制、桌面项与
`~/.config/fcitx5/conf/hufu.conf` → 清理残留引擎进程与运行期 socket → 按 `assets/MANIFEST`
逐个删掉 `~/.local/share/hufu/` 下装配进去的文件并清理空目录（与安装同一份台账：
装得上就卸得掉）。最后 `fcitx5 -r -d` 使输入法列表刷新。

用户数据不在台账里，默认保留：`码表/<方案>/用户调整.txt`（用户词与置顶/删除调整）、
`数据/user-adjust.log`（调整日志）、`数据/config.json`（方案与开关）、`数据/皮肤/`、`模型/`。
要一并清除用 `--purge`（整树删除 `~/.local/share/hufu`）。检出里缺 `assets/MANIFEST`
（不完整检出、或脚本被单独拷走）时，卸载会明确提示原因并退回原行为：默认不删数据，
`--purge` 仍是整树删除。

## 5. 排障

```sh
systemctl --user status hufu-server      # 引擎状态
journalctl --user -u hufu-server -f      # 引擎日志
ls -l "$XDG_RUNTIME_DIR/hufu-ime.sock"   # IPC socket（应为 0600）
fcitx5 -r -d                             # 重载 addon / 输入法列表
```

- **配置工具里没有「虎符」**：确认 `/usr/share/fcitx5/{addon,inputmethod}/hufu.conf`
  存在，然后 `fcitx5 -r -d`；
- **打字没反应**：确认 `hufu-server` 在运行；引擎不可达时按键直通（不会卡住输入）；
- **候选窗不出现**：确认当前输入法为「虎符」（fcitx5 面板/托盘指示）；
- **切换输入法无名称气泡**：上游（fcitx5 经典界面）对自定义输入法的展示行为随版本而异，
  以托盘图标为准。
