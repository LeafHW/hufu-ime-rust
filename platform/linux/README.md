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
  （`hufu_client_key/reset/focus/ping`、配置读写、状态区菜单动作
  `hufu_client_reload_schema/open_schema_dir/sound_toggle/sound_state`，宿主回调
  commit/update），含 mock socket 单测；C++ 侧经 `hufu-addon/shell/hufu_abi.h` 调用。
- `hufu-addon/shell/hufu.cpp` — C++ 薄壳：fcitx5 接口适配（键名映射、
  `filterAndAccept`、`commitString`/回删、`CommonCandidateList`、状态区菜单）。
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

CI（`.github/workflows/ci.yml`，ubuntu-26.04）与本地自测跑同一套检查：addon 构建、
C ABI 符号一致（`hufu_abi.h` ↔ `libhufu.so`）、DESTDIR 安装布局、逐脚本 `bash -n`、
assets 台账、install/uninstall `--dry-run` 冒烟——本地一条命令（依赖与依赖缓存需先备好）：

```sh
bash platform/linux/checks/ci-local.sh
```

## 安装 / 卸载

```sh
# 一条：构建 + 装配数据 + 用户级服务 + 系统级 addon（中途会要 sudo 密码）
platform/linux/install.sh

# 先预览：把要执行的每一个改动性动作（构建/拷贝/安装/sudo/systemctl）逐条列出，不落任何改动
platform/linux/install.sh --dry-run
platform/linux/uninstall.sh --dry-run

# 已有产物、只装配数据/装服务：
platform/linux/install.sh --no-build --no-system   # 跳过 sudo 部分
sudo cmake --install platform/linux/build          # 系统级 addon

# 卸载：默认按台账删掉装配进去的数据（用户词/配置/模型保留）
platform/linux/uninstall.sh
platform/linux/uninstall.sh --purge                # 连用户数据一起删（整树）
```

安装产物：

- 系统级：`/usr/lib/fcitx5/libhufu.so`、`/usr/share/fcitx5/{addon,inputmethod}/hufu.conf`
- 用户级：`~/.local/bin/hufu-server`、`~/.config/systemd/user/hufu-server.service`、
  `~/.local/share/applications/hufu-settings.desktop`、`~/.config/fcitx5/conf/hufu.conf`
- 数据：`~/.local/share/hufu/{码表,模型,数据}`（`数据/` 存配置/皮肤/用户词/音效）

装完即校验：装配后按 `assets/MANIFEST` 台账逐项核对 `~/.local/share/hufu/` 下的落盘文件
（字节 + sha256），任一不符即报错退出；`--from`/`--tigerclaw` 外部源模式的内容由外部数据源
决定，不做台账核对（脚本会显式说明跳过）。

卸载与安装对称：默认按同一份 `assets/MANIFEST` 逐个删掉装配进去的文件并清理空目录；
用户数据（`码表/<方案>/用户调整.txt` 用户词与调整、`数据/user-adjust.log` 调整日志、
`数据/config.json` 配置、`数据/皮肤/`、`模型/`）不在台账里，默认保留（`--purge` 才整树删除）。
检出里缺 `assets/MANIFEST` 时，卸载会明确提示原因并退回原行为（默认整树保留）。

装完后：

```sh
fcitx5 -r -d                 # 重启 fcitx5
fcitx5-configtool            # 输入法 → 添加「虎符」
```

设置页：应用菜单「虎符设置」，或浏览器打开 `http://127.0.0.1:4390/`。

## 数据与资源（仓库自带 `assets/`）

Linux 端所需数据**全部随仓库分发**（`assets/`，与安装布局同构），
安装脚本默认从此装配，**无需外部下载**：

完整步骤（安装 / 资源获取 / 卸载）见 **[../../docs/linux-install.md](../../docs/linux-install.md)**；
各资源来源、版本与许可见 **[../../docs/asset-sources.md](../../docs/asset-sources.md)**。

| assets 目录 | 安装位置（`~/.local/share/hufu/`） | 内容 |
|---|---|---|
| `码表/虎整句/`（默认方案） | `码表/虎整句/` | 整句方案：tigress 单字+词 import 闭包（≈250k）、快符/常用符号/一简符号、补充语料；放入模型即启用整句 |
| `码表/虎码字词/` | `码表/虎码字词/` | tigress 单字+词 import 闭包（≈250k）、快符/常用符号/一简符号、补充语料 |
| `码表/虎码单字/` | `码表/虎码单字/` | tiger 单字表（≈117k）+ 符号 |
| `码表/多多B/` | `码表/多多B/` | 多多格式常用字词/生僻字 |
| `数据/注释/` | `数据/注释/` | 拼音.注释、unicode.注释 |
| `数据/拆分/` | `数据/拆分/` | 虎码.拆分 |
| `数据/拼音反查/` | `数据/拼音反查/` | 拼音.txt（全拼反查；小鹤待转换） |
| `数据/转换词典/` | `数据/转换词典/` | OpenCC ST/TS 简繁表 + emoji 表 |
| `数据/音效/` | `数据/音效/` | key/select/commit/page.wav |

模型（ngram 整句 / Qwen3 重排 GGUF）**不在仓库**（体积过大），随「模型文件」单独分发；
缺模型时引擎为纯码表模式。

外部源覆盖（换版本用）：`--from <虎码资源目录>`（码表）+ `--tigerclaw <虎爪7z>`
（注释/拆分/反查/符号/音效，`7z e -so` 按需流式取单文件）；`--no-assets` 跳过资源装配。
外部源模式不走 `assets/MANIFEST` 台账（装配前不校验来源，装后校验显式跳过）。

首次安装生成 `数据/config.json`：默认方案 `虎整句`（放入模型即启用整句）；反查=拼音、
拆分=`虎码`、unicode 注释/拆分显示开；中英切换交由 fcitx5 布局（引擎不带英文输入）。

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

## 状态区菜单（托盘「虎符」）

本输入法激活时，状态区出现「虎符」子菜单（`SimpleAction` + 自定义 `Action`，五项）：

| 菜单项 | 行为 |
|---|---|
| 重载码表 | 引擎侧当前方案原样重载（op `reload_schema`）：改码表/补充语料后免重启 server |
| 打开方案文件夹 | 引擎侧打开当前方案码表目录（op `open_schema_dir`） |
| 按键音效 | 勾选态读引擎（op `sound_state`），点击引擎侧取反并落盘（op `sound_toggle`）；默认关。Linux 前端目前不播放音效——wav 播放未接，本项改的是引擎配置 |
| 引擎状态 | 信息行（不可点）：连接状态（`ping`；不可达时附 `hufu_client_status` 的失败原因）+ 当前方案名（配置键 `schema.current`） |
| 候选窗显示预编辑 | 宿主项（`~/.config/fcitx5/conf/hufu.conf` 的 `PanelPreedit`），**默认开**；切换后落盘并立即按该输入上下文最近一次 UI 快照重放 |

引擎不在线时的降级：动作类 op 失败只记 `hufu` 类别日志、菜单状态不变（不本地假翻转）；
信息行如实显示「不可达」。信息行文案与音效勾选态在状态区刷新（输入法激活）与每次
菜单动作后各取一次并缓存——菜单文案会被 UI 线程反复取用，不在那里做 socket 往返。

### 候选排列（配置迁移）

设置页的宿主项「强制竖排候选」（`conf/hufu.conf` 的 `ForceVertical`，布尔）已换成三态
「候选排列」（`CandidateLayout`）：**跟随全局**（默认，随 fcitx5 全局「候选竖排」）/ 横排 / 竖排。
键名变了，旧的 `ForceVertical=True` **不再被读取**（静默回到默认「跟随全局」）——需要强制竖排的话，
在 `fcitx5-configtool` 的「虎符 → 行为」里把「候选排列」选成「竖排」一次即可。

## 字反查（纯宿主侧）

默认按 `~`（设置页「快捷键 → 字反查」，存 `~/.config/fcitx5/conf/hufu.conf` 的
`Hotkey/CharLookupKey`；清空该项即关闭本功能）：取屏幕上光标左侧 1 个汉字，在候选窗
上方显示两排——上排 `咅 <拼音>`、下排 `虍 <虎码>`（有拆分数据时在虎码后追加 `· <拆分>`）。

- 数据来自已装配的资源（`$HUFU_ROOT` = `${XDG_DATA_HOME:-$HOME/.local/share}/hufu`，
  与 `install.sh` 同一口径）：`数据/注释/拼音.注释`（每行 `字\t拼音`，多音以空格分隔）、
  `码表/虎码单字/tiger.dict.yaml`（Rime 词典：按 `columns:` 声明里的 `text`/`code` 列取字与码，
  同字多码以 `/` 连接）、`数据/拆分/虎码.拆分`（每行 `字\t拆解`，可选——缺文件只是没有拆分列）。
  索引**首次触发才装载**，不在 addon 构造期读文件。
- 只写输入面板的两排 aux：不占用候选列表、也不伪造预编辑（引擎的候选与预编辑原样留着）；
  显示期间上排暂时由字反查占用，清除后还原引擎 aux。
- 触发键被本层消费（与引擎方案的反查触发键同语义，不再作为普通字符输入）；再按任意键
  （含 `Esc`）或失焦即清除，重新按触发键等于按当前光标位置刷新。
- 降级路径：应用不支持周边文本（如终端）或光标左侧不是汉字时不显示；数据缺项的那一列显示
  `?`；索引装载失败（`拼音.注释` 与 `tiger.dict.yaml` 都读不到）只记一条 `hufu` 类别
  Warn（`hufu: 字反查数据不可用（…）`），功能静默不可用——触发键仍被消费，改绑/清空该键
  即可让 `~` 回到普通字符。

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
| 组段 | ITfComposition | setMarkedText | clientPreedit（组段内联）；候选窗内预编辑默认开（托盘可切） |
| 上屏 | SetText+EndComposition | insertText | commitString（回删走 forwardKey/deleteSurroundingText） |
| 候选窗 | D2D+Acrylic 自绘 | NSVisualEffectView | fcitx5 自带面板（classicui/kimpanel） |
| 设置 | localhost Web UI | 同 | 同（systemd user 服务托管） |
| 中英切换 | Shift | Shift | Shift（引擎内态，subMode 显示） |

## 已知限制（第一版）

- 候选点击已支持上屏（`CandidateWord::select` → 引擎 `select` op），与数字选重同语义（学习、无闪帧）；候选窗样式为 fcitx5 主题，未复刻虎符皮肤材质/动效。
- 选重上屏的「闪帧确认」在 Linux 上即时清窗（Windows 侧是 150ms 收场钟 + 高亮滑动；无皮肤动效时不做此动画）。
- 拼音反查当前为**全拼**（虎爪 `拼音.txt`）；小鹤双拼表待转换。音效 wav 已就位但前端播放未接（第二批次），开关默认关。
- **多个输入上下文共享一个引擎会话**（最后激活者胜，与 Windows 一致），焦点切换靠 `focus` 清态；宿主侧另有每输入上下文的 UI 快照，只用于「候选窗显示预编辑」切换后的面板重放。
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
