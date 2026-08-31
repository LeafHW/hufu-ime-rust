# 虎符 HuFu — 以虎码为核心的跨平台输入法平台

> 名字取自古代调兵信物「虎符」：以虎码为主码的输入法平台，Windows / macOS 双端（还没弄）。
>
> 目标：吸收 **虎爪输入法（TigerClaw）** 与 **Rime（虎码配置）** 的全部能力，重新实现为一个
> 统一引擎 + 双平台前端 + 图形化设置的产品级输入法。

## 功能总览

| 能力 | 说明 |
|---|---|
| 虎码顶功 | 27 码元（a–z + `;` `'`）、最大码长 4、第 5 码顶屏首选、四码唯一上屏、空码清屏 |
| 多码表格式 | HuFu 原生、Rime dict.yaml（columns/import_tables/encoder）、多多（---config@ 头、`#固`、`=>`）、QQ五笔、虎整句 `码 词 词`、符号/注释/拆分/反查/补充语料 |
| 方案管理 | 方案=目录；一键切换最近方案对；每方案独立用户词 |
| 整句输入 | 兼容 TigerClaw TCSKNM02 明文 ngram 模型（Kneser-Ney trigram），beam search 组句，置信前缀提前上屏，补充语料 Aho–Corasick 奖励；**Qwen3-0.6B GGUF 神经重排已接入**（纯 Rust GGUF/q8_0 解码 + GEMM，停顿后异步重排 top-5，下一次按键生效）；**全部权重可调** |
| 候选交互 | 数字/分号/引号/自定义选重键、翻页、竖排/横排、调序、置顶、软删、延时显示 |
| 反查 | `` ` `` 引导小鹤双拼反查；注释显示拼音/Unicode 分区/拆分 |
| 符号系统 | 快符 `;a`–`;z`、一简符号、`/xx` 分类符号、动态变量（日期/时间/星期）、`\` 命令空间、**`\calc` 真计算器**（+ - * / % ^ 括号，全角符号兼容，上屏纯数值）、**`\w` 造词**（Rime encoder 规则构码，选词自动入用户词库） |
| 繁简与注解 | OpenCC txt 对照（简↔繁、emoji 注），候选滤镜链：前 3 候选自动追加繁体（⚑繁）与 emoji（😊）变体，设置界面开关+方向可选 |
| 用户数据 | 用户词四态（加/隐/权/序）、追加式调整日志、按时间戳导出 |
| 皮肤 | JSON 皮肤：19 个颜色角色 + 布局参数 + **材质**（纯色/半透明/毛玻璃磨砂/玻璃边框），Windows 用 DWM Acrylic/Mica + Direct2D，macOS 用 NSVisualEffectView；兼容导入 weasel/squirrel 配色 |
| 设置界面 | 本地 Web UI（daemon 托管），全图形化：方案/候选/键位/整句权重/皮肤编辑器/用户词管理/导入导出。**不碰 yaml/lua** |
| 中英切换 | Shift / Ctrl+Space / Caps / 跟随系统；中文态英文标点；大小写保留混输 |
| 音效 | 4 类按键音 + 音量（可选）：引擎按键即出标签（key/select/commit/page），DLL waveOut 播放（管道取 base64 音频，音量 0–100，**任意声道/位深 PCM**），设置界面开关+试听；默认音源 TigerClaw sounds（KeyNormal/KeySpace/KeyPop/KeyFunc） |
| 数据安全 | 全量用户数据快照导出（配置+用户词+调整日志，ISO 日期时间戳）；HTTP BOM 容错 |

## 架构

```
┌─────────────────────────────────────────────────────────────┐
│                     engine/ (Rust, 跨平台核心)                │
│  hufu-types    按键/候选/会话 等共享类型                       │
│  hufu-dict     多格式码表解析 + Trie + 用户词 + 注释表          │
│  hufu-engine   会话状态机：顶功/选重/反查/符号/滤镜链            │
│  hufu-sentence TCSKNM02 ngram 加载 + beam 组句 + 提前上屏      │
│  hufu-rerank   纯 Rust GGUF/Qwen3 推理（q8_0 解码 + GEMM）     │
│  hufu-config   设置模型(JSON) + 热更新                          │
│  hufu-skin     皮肤模型(JSON) + weasel/squirrel 互导           │
│  hufu-server   常驻 daemon：引擎实例 + IPC + 设置 Web UI 托管   │
│  hufu-cli      码表转换 / REPL 测试 / 安装辅助                  │
└─────────────────────────────────────────────────────────────┘
        ▲                                    ▲
        │ IPC (命名管道 / Unix socket)        │ HTTP+WS (localhost)
┌───────┴────────────┐              ┌────────┴─────────┐
│ platform/windows   │              │ platform/macos    │
│ hufu-tsf: TSF COM  │              │ HuFuIME:          │
│ 组件(Rust+windows-rs)│             │ InputMethodKit    │
│ 候选窗 D2D+Acrylic  │              │ NSVisualEffectView│
└────────────────────┘              └──────────────────┘
        设置 UI = 浏览器/WebView 打开 daemon 的 Web 设置页
```

## 目录

```
hufu/
├── engine/          Rust workspace（核心引擎，双平台共享）
├── platform/windows Windows TSF 输入法前端
├── platform/macos   macOS InputMethodKit 前端（需在 Mac 上构建）
├── settings-ui/     设置 Web UI（由 hufu-server 托管）
├── dictionaries/    预置/转换出的码表（HuFu 原生格式）
├── tools/           辅助脚本
└── docs/            架构与调研文档（含两份逆向规格报告）
```

## 当前进度（持续更新）

| 模块 | 状态 | 实测 |
|---|---|---|
| `hufu-types` | ✅ | — |
| `hufu-dict` | ✅ | 虎码单字 113k 条/272ms、虎码字词（import 闭包）246k 条/1.2s、QQ五笔 96k/114ms、多多 B 定制 33k/81ms；置顶/软删回放（最新在前、无重复、pinned 标记） |
| `hufu-config` | ✅ | — |
| `hufu-engine` | ✅ | 真实码表 REPL：顶功（`tuj`+死端字母推屏 𪚠）、`jd`+`;` 次选、注释/拼音/分区回显；动态变量 `\da`→真实日期、`\n12345`→一万二千三百四十五、`\N1234`→壹萬贰仟大写金额、**`\calc(1+2)*3`→上屏 9**（HTTP 在线实测）、**`\w就就`→构码 jj 入库**（Rime encoder fixture）；Ctrl+Shift+数字 置顶 / Ctrl+Delete 软删（日志落盘+回放）；音效标签 key/select/commit/page；OpenCC 繁体变体（真实 ST 表：来→來、那个→那個 ⚑繁 + emoji） |
| `hufu-sentence` | ✅ | 真实 TCSKNM02 224MB 模型加载 87ms；`tujatuja`→「我们我们」、`mfyto`→「大一点我是」；单次组句 0.4–2.8ms；提前上屏提案 |
| `hufu-skin` | ✅ | 19 颜色角色 + 材质模型；weasel 配色互导（含 0xAABBGGRR ↔ #RRGGBBAA） |
| `hufu-cli` | ✅ | check / convert / repl |
| `hufu-server` + 设置 GUI | ✅ | 20 REST 路由（+候选置顶/隐藏/音效试听/全量快照导出）+ `\\.\pipe\hufu-ime` 命名管道 + Unix socket（macOS）；pipeclient 全操作通过；40KB 单文件设置 UI（试用台/方案/整句权重 10 滑杆/皮肤编辑器实况预览/用户词+置顶隐藏/任意候选调整/音效开关+试听/繁简开关/快照导出/导入导出） |
| Windows TSF | ✅ 真机全通 | `hufu_tsf.dll`（纯 Rust + windows-rs 0.58）；**系统级激活实测**：Win+空格 第 4 项（虎图标）、汉字上屏、候选窗贴光标跟随、选区顺序正确；**应用矩阵**：记事本/浏览器/VSCode/QQ/DSH/Listary 全通过。注册九步一键化（install.ps1 + reg-fix.ps1）；DLL 轨迹日志 `%TEMP%\hufu-tsf-trace.log`。运行时铁律：EditSession 用 ASYNCDONTCARE、组段走 GetSelection→StartComposition、GetTextExt 即屏幕坐标 |
| macOS IMK | 🔨 骨架 | HuFuInputController（键码→Unix socket→组段/上屏）+ CandidatePanel（NSVisualEffectView 四材质）+ Info.plist + build.sh；帧协议与 Windows 管道一致；**需在 Mac 上编译迭代** |

### 测试

- 引擎 workspace：**58 测试 0 失败**（字典格式/引擎状态机/动态变量/数字转中文/置顶回放/整句/皮肤/配置/GGUF f16/GEMM/q8 对 llama.cpp F32 基准/wav 解析）
- 管道回归电池：lock 12/12、battery2 16/16、edge 17/17、flow 全过、设置生效性 7/7（皮肤热反映/横排/序号/延时/音效/调整日志）
- Windows 冒烟：12 步 exit=0（COM 层 + msctf + 管道 + 候选窗 v2 四材质，横竖排各验一轮）
- 重排端到端：`bwjdsk` → Qwen3 翻转 `[弱斗该,嫁𡀲]→[嫁𡀲,弱斗该]`，二次输入缓存即时生效

### 整句 A/B 压测（5 万句语料）

```powershell
# 语料：prepare_corpus_50k.py 生成（LCCTS/THUCNews/评论 混配，4~30 字纯汉字句）
cd engine
cargo run --release -p hufu-rerank --bin sentence-bench -- `
  <语料目录>\test_sentences_50k.txt --arm AB --sample 2000 --wa 8 --wb 2 `
  --out ..\docs\benchmark-qwen-vs-ngram.md
```

录入规则完全按整句虎：逐字全码连打（一简字取 2 码全码）、一句打完才空格；提前上屏前缀由引擎提交累计。A 臂 ngram 全量；B 臂 +Qwen 重排在停顿后空格前一次性介入（同生产路径）。输出 Wilson 95% CI、按句长分桶、打捞/拖累翻转统计。报告见 `docs/benchmark-qwen-vs-ngram.md`。

## 安装（终端用户，免构建）

从 [Releases](../../releases) 下载 `HuFu虎符输入法-安装包.zip`（约 700MB，含码表、ngram 整句模型、Qwen3 重排模型与全部数据），解压到任意稳定位置（勿放临时目录），双击 **`安装.bat`**。绿色安装：程序与数据原地在安装文件夹内运行，C 盘零残留；卸载跑 `卸载.bat` 后删除目录即可。Windows 10/11 x64。

## 构建

```powershell
# 引擎 + CLI（Windows / macOS / Linux 均可）
cd engine
cargo build --release
cargo test

# REPL 体验引擎
cargo run -p hufu-cli -- repl --dict ../dictionaries/虎码单字

# Windows TSF 前端（需要 Windows；x86_64-pc-windows-gnu 工具链）
cd platform/windows
cargo build --release          # hufu_tsf.dll + hufu-tsf-smoke.exe
./target/release/hufu-tsf-smoke.exe   # 冒烟：COM 层 + 管道引擎链
# 注册见 platform/windows/install/README.md
```

### 系统激活（真机安装）

```powershell
# 管理员终端，一条完成全部注册（COM + msctf 档案 + 分类 + 语言列表 + 切换器）
powershell -ExecutionPolicy Bypass -File "platform\windows\install\install.ps1"
# 若切换器不显示第 4 项（全局分类被清）：
powershell -ExecutionPolicy Bypass -File "platform\windows\install\reg-fix.ps1"
```

注意：
- `hufu-server.exe` 需先运行（管道与设置页）
- **启动早于注册的应用要重启才能用 HuFu**（QQ/DSH 实测；TSF 应用启动时缓存输入法列表）
- ctfmon 重启/注销后当前输入法会重置回默认，需 Win+空格 重选（系统行为）
- 搜索启动器（Listary 等）打中文时字母进候选——按 `Shift` 切英文直通

## 文档

- [docs/architecture.md](docs/architecture.md) — 架构设计与数据流
- [docs/research/ime-frontends.md](docs/research/ime-frontends.md) — TSF / IMK 前端研究纪要与 TigerClaw 行为语义
- [docs/dictionary-formats.md](docs/dictionary-formats.md) — 支持的码表格式规范（含 TCSKNM02 模型布局实测）

## 许可证

- 本项目代码以 [GPL-3.0](LICENSE) 发布
- 随发行包分发的第三方组件保留其原始许可证：Qwen3 模型（Apache-2.0）、llama.cpp（MIT）、OpenCC 词典数据（Apache-2.0）
