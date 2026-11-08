# HuFu 架构设计

## 0. 模块总览

```
┌─────────────────────────────────────────────────────────────┐
│                     engine/ (Rust, 跨平台核心)                │
│  hufu-types    按键/候选/会话 等共享类型                       │
│  hufu-dict     多格式码表解析 + Trie + 用户词 + 注释表          │
│  hufu-engine   会话状态机：顶屏/选重/反查/符号/滤镜链            │
│  hufu-sentence NM03 五阶 / NM02、04 三阶 + beam 组句          │
│  hufu-rerank   进程内 llama.cpp DLL / 纯 Rust GGUF/Qwen3     │
│  hufu-config   设置模型(JSON) + 热更新                          │
│  hufu-skin     皮肤模型(JSON) + weasel/squirrel 互导           │
│  hufu-server   常驻 daemon：引擎实例 + IPC + 设置 Web UI 托管   │
│  hufu-cli      码表转换 / REPL 测试 / 安装辅助                  │
└─────────────────────────────────────────────────────────────┘
        ▲                                    ▲
        │ IPC (命名管道 / Unix socket)        │ HTTP (localhost)
┌───────┴────────────┐ ┌──────────────────┐ ┌────────┴─────────┐
│ platform/windows   │ │ platform/linux    │ │ platform/macos    │
│ hufu-tsf: TSF COM  │ │ fcitx5 addon:     │ │ HuFuIME:          │
│ 组件(Rust+windows-rs)│ │ C++ 薄壳 + Rust  │ │ InputMethodKit    │
│ 候选窗 D2D/DWrite   │ │ staticlib（socket）│ │ NSVisualEffectView│
└────────────────────┘ └──────────────────┘ └──────────────────┘
        设置 UI = 浏览器/WebView 打开 daemon 的 Web 设置页
```

仓库目录：

```
hufu/
├── engine/          Rust workspace（核心引擎，全平台共享）
├── platform/windows Windows TSF 输入法前端
├── platform/linux   Linux fcitx5 前端（Rust staticlib + C++ 薄壳 + CMake）
├── platform/macos   macOS InputMethodKit 前端（开发中，IMK 骨架已落地）
├── settings-app/    当前设置 Web UI（构建产物嵌入 hufu-server，/next）
├── settings-ui/     保留的旧设置页（hufu-server 根页 /）
├── assets/          码表与数据资源（多方案码表/注释/拆分/反查/音效）
└── docs/            架构与调研文档
```

## 1. 设计原则

1. **共享核心，平台前端**：码表检索、整句组句、用户词和滤镜在 Rust 引擎内；
   Windows TSF、Linux fcitx5 与开发中的 macOS IMK 负责平台按键、组段、焦点及候选呈现。
2. **配置完全图形化**：设置存 JSON（`config.json` / `skins/*.json`），由 Web 设置界面读写。
   平台不暴露 yaml/lua 给用户；导入 Rime/虎爪配置仅作为一次性迁移。
3. **码表格式宽松兼容**：原生格式之外，多多 / Rime dict.yaml / QQ五笔 / 虎整句可直接放入方案目录，
   引擎按文件特征自动识别加载。
4. **可逆副作用**：用户操作（调序/置顶/删词）写追加式日志，可全量导出与回放。

## 2. 数据流（一次按键）

```
平台按键事件 (TSF keyEvent / IMK handle)
   │ 平台键名/修饰键经 server parse_key 转为 KeyInput {key, modifiers, is_press}
   ▼
hufu-engine Engine::process_key(&mut Session, KeyInput)
   ├─ 1 键位分类器：可打印/功能/选重/翻页/切换/命令
   ├─ 2 模式层：中英切换、全半角、标点风格、Caps
   ├─ 3 命令层：`\` 命名空间、`` ` `` 反查、`/` 符号、`;x` 快符
   ├─ 4 编码层：alphabet 校验 → raw 追加/回删
   │     ├─ 顶屏判定（max_code_length + auto_select/auto_clear）
   │     ├─ 唯一候选自动上屏（auto_select_unique）
   │     └─ 生成 CandidateList：
   │          a) 精确码命中（按 权重→码表序→用户调整序 排序）
   │          b) 整句模式：配置启用且解码器已装载时，hufu-sentence beam 组句
   │          c) 用户词注入、反查候选注入
   ├─ 5 滤镜链：简繁 → 拼音注 → 拆分注 → emoji 注 → 字集过滤 → 去重
   └─ 6 输出 KeyOutcome {consumed, commit, back, state, sound}
        Host 将 SessionState 放在协议响应顶层 state，供前端更新组段/候选
   ▼
平台层执行：TSF SetText/InsertTextAtSelection；候选窗刷新
```

## 3. 会话与实例

- `Engine`（server 进程级）：配置 + 当前已装载方案及可用方案清单 + 可选句模型 + 用户词库；可热重载。
- 当前 server 的 `Host` 持有一个共享 `Session`：raw 编码、候选页、命令状态、英文缓冲、中英状态。
  前端按各自实例保存本地 TSF/窗口状态，焦点切换通过 pipe `focus` 清空 server 组段和文章尾部上下文，
  并非每个输入上下文在 server 内都有独立会话。
- daemon `hufu-server` 持有 Engine；前端通过 IPC 连接（Windows 命名管道 `\\.\pipe\hufu-ime`，
  Unix socket 为 `$XDG_RUNTIME_DIR/hufu-ime.sock`，环境变量缺失时回退 `/tmp/hufu-ime.sock`；macOS 骨架同此路径）。
  协议为 4 字节小端长度前缀 + UTF-8 JSON 请求/响应；命名管道与 Unix socket 使用相同帧格式。

## 4. 码表子系统

- 方案（Schema）= 配置 schema.dir 下的方案目录（默认 `码表/<方案名>/`），加载器按角色识别文件：
  - 主码表：`*.dict.yaml`（rime）、`*多多*`（多多）、`*.txt`（按内容嗅探：`词\t码` vs `码 词 词`）
  - `快符.txt`、`常用符号.txt`、`一简符号.txt`、`补充语料.txt`、`用户调整.txt`
  - `*.注释`（拼音/unicode）、`虎码.拆分`、`*.反查.txt`
- 内存结构：arena 数组布局的 `Trie`（节点存条目下标）+ `by_code: HashMap<String, Vec<u32>>`
  精确索引 + `text_to_codes` 词到编码索引；条目按权重和原序建立稳定名次。
- 原生格式（导出/交换用）：TSV，`码\t词\t权重`，`#hufu-dict v1 name=...` 头。

## 5. 整句子系统（hufu-sentence）

- **模型**：按文件魔数选择 TigerClaw TCSKNM03 五阶模型，或兼容 TCSKNM02/04 三阶模型
  （04 为量化变体）。五阶通过词表、按阶目录、稀疏索引及页记录查询并逐阶回退；
  三阶格式背景见 [dictionary-formats.md](dictionary-formats.md) 的 TCSKNM02 一节。
- **解码**：按 raw 前缀位置分桶的 beam search（beam_width 默认 200），
  字级三阶/五阶模型打分 + 每字出字奖励 + 名次惩罚 + 孤立生僻惩罚 + 补充语料 AC 自动机奖励。
- **提前上屏**：按候选前缀质量占比阈值（confidence 默认 0.99）提出前缀，结合证据窗
  （sentence.early_need 默认 3）、分歧保护和词边界决定提交；行尾等条件可缩短确认等待。
- **整句权重可调**（config.sentence.weights.*）：beam_width、candidate_limit、emitted_character_reward、
  rank_penalty、isolation_threshold/lambda、confidence、supplement_baseline/scale/maximum、
  high_freq_limit（整句高频字上限，0 = 不限制）、以及与码表首选融合的 dict_bias。
- **LLM 重排**：可选 Qwen3 GGUF；`Host::setup_rerank` 创建后台工作线程，优先使用进程内
  llama.cpp 原生 DLL scorer，失败回退纯 Rust GGUF/Qwen 推理。结果写缓存，经轮询或后续按键
  应用；缓存隔离方案、模型代际与前文，关闭重排时保留基础整句顺序。此路径不启动解释器或
  llama.cpp 子进程。

## 6. 皮肤子系统（hufu-skin）

- 皮肤 JSON：`colors`(19 角色，兼容 weasel 全部字段名)、`layout`(圆角/间距/边距/阴影/字体)、
  `material`(none/solid/translucent/frosted/glass + tint、noise、blur_radius)。
- Windows：当前候选窗以 Direct2D/DWrite 绘制颜色、透明度、圆角及阴影；旧 glass/frosted
  材质兼容为半透明。毛玻璃路径已退役，accent 置为 disabled，不再承诺 Acrylic/Mica 背景模糊。
- macOS：开发中的候选窗使用 `NSVisualEffectView`；尚未发布，也未完成 Mac 实机验证。
- 编辑器：设置 Web UI 提供颜色与布局预览、SSF 位图预览，可导入
  weasel/squirrel 的 `preset_color_schemes`；Web 页自身的 CSS 模糊不代表 Windows 候选窗提供背景模糊。

## 7. Windows 前端（platform/windows）

- `hufu-tsf`：Rust cdylib，实现 TSF TIP（`ITfTextInputProcessorEx`、`ITfKeyEventSink`、
  `ITfCompositionSink`、`ITfDisplayAttributeProvider`、`ITfThreadMgrEventSink`），
  自注册 DllRegisterServer（写 CLSID + Profile + Categories），参考 weasel WeaselTSF 结构。
- 候选窗：常规宿主在所属 TSF 线程维护主候选实例，以 D3D/DirectComposition + Direct2D/DWrite 呈现；
  加词等小窗线程使用独立的线程局部 TL 候选实例，消息与动画 tick 按窗口归属路由。
  打包宿主可走 owned 分层窗的 WARP/UpdateLayeredWindow 路径，失败时回退 server 代画。
  候选窗使用无激活顶层窗、圆角、透明背景与阴影，跟随光标（ITfContextView::GetTextExt），
  不能笼统理解为所有候选都在一个独立渲染线程。
- 托盘/状态：daemon 进程持有托盘图标与中英状态胶囊。

## 8. macOS 前端（platform/macos）

- **状态**：开发中（IMK 骨架已落地，待 Mac 实机迭代；暂无安装包）。
- `HuFuIME`：InputMethodKit（`IMKInputController` 子类 + `IMKServer`），
  候选窗 `NSPanel` + `NSVisualEffectView`；连接 daemon Unix socket。
- 工程为 SPM 包 + bundle 组装脚本（Info.plist：tsInputMethodCharacterIconKey 等），
  参照 Squirrel 的 SquirrelInputController/ SquirrelPanel 结构。

## 9. 设置子系统（settings-app + hufu-server）

- daemon 监听 `127.0.0.1:动态端口`（端口写入用户目录文件），提供：
  - `GET /next` 当前设置页与 `/assets/*` 静态资源；Windows `hufu-settings.exe` 使用 Win32
    WebView2 外壳访问 `/next?shell=1`，内嵌的是 `settings-app/dist` 构建产物。
  - `GET /` 保留旧 `settings-ui` 设置页。
  - 本地 HTTP `/api/*`：配置、皮肤、预览、用户词、码表导入导出与方案切换；当前没有 WS 路由。
- 配置保存使用版本号 CAS，前端串行提交并以执行时最新配置构造修改；daemon 校验后写盘并热应用。
  方案装载在 Host 锁外执行，应用前检查配置版本，坏方案不会替换有效配置。
- WebDAV 同步是独立后台线程，按文件修改时间同步所选方案目录和可同步配置，覆盖/删除前保留
  回收站；2026-10-30 已移除端到端加密，云端为用户自己 WebDAV 中的明文文件，本机保存应用凭据。

## 10. 安全与边界

- IPC 仅本机（命名管道 ACL 当前用户；socket 文件 0600）。
- 设置页仅监听 loopback，绑定时检查 Referer/Origin 防 CSRF。
- 无遥测；模型与大文件留在用户数据目录。
