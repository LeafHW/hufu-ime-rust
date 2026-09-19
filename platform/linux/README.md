# hufu linux — fcitx5 前端

薄壳架构（与 Windows TSF / macOS IMK 一致）：输入法进程只做事件转发与绘制，
引擎在 `hufu-server` 守护进程里。

```
按键 → fcitx5 addon（libhufu.so：C++ 薄壳 + Rust staticlib）
      → Unix socket（$XDG_RUNTIME_DIR/hufu-ime.sock，4B 小端长度 + JSON 帧）
      → hufu-server 引擎 → {outcome, state}
      → fcitx5：commitString 上屏 / setPreedit 组段 / 候选面板
```

## 文件

- `hufu-fcitx5-client/` — Rust staticlib：Unix socket 客户端 + C ABI
  （`hufu_client_key/reset/focus/ping`，宿主回调 commit/update），含 mock
  socket 单测；C++ 侧经 `hufu-addon/shell/hufu_abi.h` 调用。
- `hufu-addon/shell/hufu.cpp` — C++ 薄壳：fcitx5 接口适配（键名映射、
  `filterAndAccept`、`commitString`/回删、`CommonCandidateList`、中英副模式）。
- `hufu-addon/conf/` — addon 与输入法条目（`Library=libhufu`、`OnDemand=True`）。
- `hufu-addon/CMakeLists.txt` — 构建并安装 `libhufu.so` 与 conf。
- `systemd/hufu-server.service` — 引擎常驻（user 服务，Restart=always）。
- `desktop/hufu-settings.desktop` — 设置页入口（xdg-open 本地设置页）。
- `install.sh` / `uninstall.sh` — 一键安装/卸载。

## 依赖

- Rust（构建 `hufu-server` 与 staticlib）
- CMake 3.20+、C++20 编译器、`Fcitx5Core` 开发包
  （Arch：`fcitx5`；Fedora：`fcitx5-devel`；Debian/Ubuntu：`libfcitx5core-dev`）
- fcitx5（运行）

## 构建

```sh
# 引擎守护进程
cd engine && cargo build --release -p hufu-server

# fcitx5 addon
cmake -S platform/linux/hufu-addon -B platform/linux/build \
    -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr
cmake --build platform/linux/build -j
```

## 安装 / 卸载

```sh
# 一条：构建 + 装配数据 + 用户级服务 + 系统级 addon（中途会要 sudo 密码）
platform/linux/install.sh

# 已有产物、只装配数据/装服务：
platform/linux/install.sh --no-build --no-system   # 跳过 sudo 部分
sudo cmake --install platform/linux/build          # 系统级 addon

# 卸载（--purge 连用户数据一起删）
platform/linux/uninstall.sh [--purge]
```

安装产物：

- 系统级：`/usr/lib/fcitx5/libhufu.so`、`/usr/share/fcitx5/{addon,inputmethod}/hufu.conf`
- 用户级：`~/.local/bin/hufu-server`、`~/.config/systemd/user/hufu-server.service`、
  `~/.local/share/applications/hufu-settings.desktop`
- 数据：`~/.local/share/hufu/{码表,模型,数据}`（`数据/` 存配置/皮肤/用户词/音效）

装完后：

```sh
fcitx5 -r -d                 # 重启 fcitx5
fcitx5-configtool            # 输入法 → 添加「虎符」
```

设置页：应用菜单「虎符设置」，或浏览器打开 `http://127.0.0.1:4390/`。

## 数据装配（install.sh --from <目录>）

默认数据源 `/home/crux/下载/_res/zhmn`（虎码官方 Rime 资源包）：

| 目标 | 来源 |
|---|---|
| `码表/虎码字词/`（默认方案） | `虎码秃版 小狼毫（Win）/tigress*.dict.yaml`（import 闭包 ≈250k 条） |
| `码表/虎码单字/` | `虎码秃版 小狼毫（Win）/tiger.dict.yaml` |
| `码表/多多B/` | `publish/定制/b/多多B*.txt`（多多格式） |
| 各方案 `补充语料.txt` | 仓库 `发行临时/补充语料.txt` |
| `数据/转换词典/` | `opencc/{STPhrases,STCharacters_Tu,TSPhrases,TSCharacters,emoji}.txt` |

**资源装配**（`--tigerclaw <7z>`，默认从资源目录自动探测 `虎爪输入法-*.7z`；
`7z e -so` 按需流式取单文件，不解整包；`--no-assets` 可跳过）：

| 目标 | 来源（虎爪 7z 内） |
|---|---|
| `数据/注释/拼音.注释` | `码表/虎码字词/1拼音.注释` |
| `数据/注释/unicode.注释` | `码表/虎码字词/unicode.注释` |
| `数据/拆分/虎码.拆分` | `码表/虎码字词/虎码.拆分`（config `split_scheme=虎码`） |
| `数据/拼音反查/拼音.txt` | `拼音反查码表/拼音.txt`（config `reverse.scheme=拼音`，**全拼反查**） |
| 各方案 `快符.txt / 常用符号.txt / 一简符号.txt` | `码表/虎码字词/*` + `码表/虎码单字/一简符号.txt` |
| `数据/音效/{key,select,commit,page}.wav` | `sounds/{KeyNormal,KeyPop,KeySpace,KeyFunc}.wav` |

装配同时打开「unicode 注释 + 拆分显示」（`show_unicode_comment/show_split`）；
拼音注释保持默认关（设置页可开，无拼音表时回落反查表注释）。
小鹤双拼反查（`小鹤双拼.txt`）待做：当前用虎爪全拼表。

首次安装生成 `数据/config.json`：默认方案 `虎码字词`；反查/拆分/音效默认关
（对应资源未就位，后续补齐）。

## 回归电池（Unix socket）

```sh
# 1) 专用数据目录 + 专用 runtime 起一个测试 server（不碰日常安装）
mkdir -p _tmp/battery/数据 _tmp/battery/模型 && cp -r ~/.local/share/hufu/码表 _tmp/battery/
cp ~/.local/share/hufu/数据/config.json _tmp/battery/数据/
XDG_RUNTIME_DIR=/tmp/hufu-battery engine/target/release/hufu-server \
    --data _tmp/battery/数据 --port 4393 &

# 2) 22 项电池：协议/键流（候选、数字选重、; 次选、' 三选、退格、Esc、顶屏、
#    空格上屏、-/= 翻页）/方案列表与切换/音效开关/Shift·Caps 不切中英（Linux 策略）/
#    focus/reset/HTTP（state、schemas、设置页）
XDG_RUNTIME_DIR=/tmp/hufu-battery HUFU_PORT=4393 \
    cargo run --release -p hufu-cli --example socketbattery
```

Windows 侧对应的是 `engine/pipe-*.ps1` 电池（命名管道）。

## 中英切换 / 英文输入（Linux 策略）

Linux 上**引擎不自带英文输入**：英文由 fcitx5 的键盘布局输入法提供
（`Ctrl+Space` 切到 `keyboard-us*` 布局）。因此：

- addon 不向引擎转发独立的 `Shift` / `CapsLock` 按键（`Shift`+标点等
  带修饰键的可打印键不受影响，`Shift+,` → 《 照常）；
- 引擎默认配置关闭中英切换：`general.shift_switch=false`、
  `ctrl_space_switch=false`、`caps_action=None`（`install.sh` 生成）；
- 设置页在非 Windows 平台隐藏这组「中英切换」开关（`/api/platform` 门控）；
- 引擎 `chinese` 恒为中文态，状态栏不显示中/英副模式；
- **有编码/候选态按 `Shift`+字母**：引擎对编码态 `Shift`+字母是「吞键」
  语义（Windows 防漏进宿主）；Linux 侧壳按「**首选上屏 + 打字母**」处理
  （顶字：首选候选上屏 → 清组段 → 字母交回应用）。

Windows 侧行为不变（仍由引擎自带中英切换）。

## 与 Windows / macOS 前端对齐

| 能力 | Windows TSF | macOS IMK | Linux fcitx5 |
|---|---|---|---|
| 键→引擎 IPC | 命名管道 | Unix socket | Unix socket（同帧协议） |
| 组段 | ITfComposition | setMarkedText | preedit + clientPreedit |
| 上屏 | SetText+EndComposition | insertText | commitString（回删走 forwardKey/deleteSurroundingText） |
| 候选窗 | D2D+Acrylic 自绘 | NSVisualEffectView | fcitx5 自带面板（classicui/kimpanel） |
| 设置 | localhost Web UI | 同 | 同（systemd user 服务托管） |
| 中英切换 | Shift | Shift | Shift（引擎内态，subMode 显示） |

## 已知限制（第一版）

- 候选点击已支持上屏（`CandidateWord::select` → 引擎 `select` op），与数字选重同语义（学习、无闪帧）；候选窗样式为 fcitx5 主题，未复刻虎符皮肤材质/动效。
- 选重上屏的「闪帧确认」在 Linux 上即时清窗（Windows 侧是 150ms 收场钟 + 高亮滑动；无皮肤动效时不做此动画）。
- 拼音反查当前为**全拼**（虎爪 `拼音.txt`）；小鹤双拼表待转换。音效 wav 已就位但前端播放未接（第二批次），开关默认关。
- 引擎单会话（与 Windows 一致），焦点切换靠 `focus` 清态。
- 注释/拆分/拼音反查/符号/音效资源未装配，对应功能关闭。
- `多多拼音反查表`（`$ddcmd` 格式）需转换后才可用。

## 排障

```sh
systemctl --user status hufu-server     # 引擎状态
journalctl --user -u hufu-server -f     # 引擎日志
ls -l "$XDG_RUNTIME_DIR/hufu-ime.sock"  # IPC socket（应为 0600）
fcitx5 -r -d                            # 重载 addon / 输入法列表
```

- fcitx5 配置工具里没有「虎符」：确认 `/usr/share/fcitx5/addon/hufu.conf` 与
  `/usr/share/fcitx5/inputmethod/hufu.conf` 存在，然后 `fcitx5 -r -d`。
- 打字没反应：确认 `hufu-server` 在跑（引擎不可达时按键直通，不阻塞输入）。
- addon 加载失败看 fcitx5 日志：`fcitx5 -D --verbose default=5`（前台调试）。
