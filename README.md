# 虎符 HuFu — 以虎码为核心的跨平台输入法平台

> 名字取自古代调兵信物「虎符」：以虎码为主码的输入法平台，Windows / macOS 双端。
>
> 目标：吸收 **虎爪输入法（TigerClaw）** 与 **Rime（虎码配置）** 的全部能力，重新实现为一个
> 统一引擎 + 双平台前端 + 图形化设置的产品级输入法。

## 功能总览

| 能力 | 说明 |
|---|---|
| 虎码顶功 | 27 码元（a–z + `;` `'`）、最大码长 4、第 5 码顶屏首选、四码唯一上屏、空码清屏 |
| 多码表格式 | HuFu 原生、Rime dict.yaml（columns/import_tables/encoder）、多多（---config@ 头、`#固`、`=>`）、QQ五笔、虎整句 `码 词 词`、符号/注释/拆分/反查/补充语料 |
| 方案管理 | 方案=目录；一键切换最近方案对；每方案独立用户词 |
| 整句输入 | 兼容 TigerClaw TCSKNM02 明文 ngram 模型（Kneser-Ney trigram），beam search 组句，置信前缀提前上屏，补充语料 Aho–Corasick 奖励；预留 LLM 重排接口（Qwen3 GGUF）；**全部权重可调** |
| 候选交互 | 数字/分号/引号/自定义选重键、翻页、竖排/横排、调序、置顶、软删、延时显示 |
| 反查 | `` ` `` 引导小鹤双拼反查；注释显示拼音/Unicode 分区/拆分 |
| 符号系统 | 快符 `;a`–`;z`、一简符号、`/xx` 分类符号、动态变量（日期/时间/星期）、`\` 命令空间 |
| 繁简与注解 | OpenCC txt 对照（简↔繁、异体、拼音注、emoji 注、拆分注），候选滤镜链 |
| 用户数据 | 用户词四态（加/隐/权/序）、追加式调整日志、按时间戳导出 |
| 皮肤 | JSON 皮肤：19 个颜色角色 + 布局参数 + **材质**（纯色/半透明/毛玻璃磨砂/玻璃边框），Windows 用 DWM Acrylic/Mica + Direct2D，macOS 用 NSVisualEffectView；兼容导入 weasel/squirrel 配色 |
| 设置界面 | 本地 Web UI（daemon 托管），全图形化：方案/候选/键位/整句权重/皮肤编辑器/用户词管理/导入导出。**不碰 yaml/lua** |
| 中英切换 | Shift / Ctrl+Space / Caps / 跟随系统；中文态英文标点；大小写保留混输 |
| 音效 | 4 类按键音 + 音量（可选） |

## 架构

```
┌─────────────────────────────────────────────────────────────┐
│                     engine/ (Rust, 跨平台核心)                │
│  hufu-types    按键/候选/会话 等共享类型                       │
│  hufu-dict     多格式码表解析 + Trie + 用户词 + 注释表          │
│  hufu-engine   会话状态机：顶功/选重/反查/符号/滤镜链            │
│  hufu-sentence TCSKNM02 ngram 加载 + beam 组句 + 提前上屏      │
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
| `hufu-engine` | ✅ | 真实码表 REPL：顶功（`tuj`+死端字母推屏 𪚠）、`jd`+`;` 次选、注释/拼音/分区回显；动态变量 `\da`→真实日期、`\n12345`→一万二千三百四十五、`\N1234`→壹萬贰仟大写金额（HTTP 在线上屏实测）；Ctrl+Shift+数字 置顶 / Ctrl+Delete 软删（日志落盘+回放） |
| `hufu-sentence` | ✅ | 真实 TCSKNM02 224MB 模型加载 87ms；`tujatuja`→「我们我们」、`mfyto`→「大一点我是」；单次组句 0.4–2.8ms；提前上屏提案 |
| `hufu-skin` | ✅ | 19 颜色角色 + 材质模型；weasel 配色互导（含 0xAABBGGRR ↔ #RRGGBBAA） |
| `hufu-cli` | ✅ | check / convert / repl |
| `hufu-server` + 设置 GUI | ✅ | 18 REST 路由（+候选置顶/隐藏）+ `\\.\pipe\hufu-ime` 命名管道 + Unix socket（macOS）；pipeclient 全操作通过；40KB 单文件设置 UI（试用台/方案/整句权重 10 滑杆/皮肤编辑器实况预览/用户词+置顶隐藏/任意候选调整/导入导出） |
| Windows TSF | 🔨 v1→v2 | `hufu_tsf.dll`（纯 Rust + windows-rs 0.58）；冒烟 12 步全绿：COM 注册、msctf 激活链（AdviseKeyEventSink 前景接收器 ✓）、管道引擎链；**候选窗 v2 已实测**：D3D11+DComp+D2D+DWM accent 四材质（solid/translucent/frosted 磨砂/glass 玻璃）各真实渲染一帧 ✓，皮肤近热更新（会话间重拉）；**待**：管理员 HKLM 注册后真实系统激活（UAC 一次）、插入点跟随 |
| macOS IMK | 🔨 骨架 | HuFuInputController（键码→Unix socket→组段/上屏）+ CandidatePanel（NSVisualEffectView 四材质）+ Info.plist + build.sh；帧协议与 Windows 管道一致；**需在 Mac 上编译迭代** |

### 测试

- 引擎 workspace：**44 测试 0 失败**（字典格式/引擎状态机/动态变量/数字转中文/置顶回放/整句/皮肤/配置）
- Windows 冒烟：12 步 exit=0（COM 层 + msctf + 管道 + 候选窗 v2 四材质）

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

## 文档

- [docs/architecture.md](docs/architecture.md) — 架构设计与数据流
- [docs/research/ime-frontends.md](docs/research/ime-frontends.md) — TSF / IMK 前端研究纪要与 TigerClaw 行为语义
- [docs/dictionary-formats.md](docs/dictionary-formats.md) — 支持的码表格式规范（含 TCSKNM02 模型布局实测）
