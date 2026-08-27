# HuFu 架构设计

## 1. 设计原则

1. **单引擎，双前端**：所有输入逻辑（键处理、码表检索、整句组句、用户词、滤镜）都在 Rust 引擎内，
   平台层只做「按键进、文字/候选出」的桥接。Windows TSF 与 macOS IMK 是薄壳。
2. **配置完全图形化**：设置存 JSON（`config.json` / `skins/*.json`），由 Web 设置界面读写。
   平台不暴露 yaml/lua 给用户；导入 Rime/虎爪配置仅作为一次性迁移。
3. **码表格式宽松兼容**：原生格式之外，多多 / Rime dict.yaml / QQ五笔 / 虎整句可直接放入方案目录，
   引擎按文件特征自动识别加载。
4. **可逆副作用**：用户操作（调序/置顶/删词）写追加式日志，可全量导出与回放。

## 2. 数据流（一次按键）

```
平台按键事件 (TSF keyEvent / IMK handle)
   │ 序列化为 KeyCommand {vk, ch, shift, ctrl, alt, caps, is_press}
   ▼
hufu-engine Session::process_key()
   ├─ 1 键位分类器：可打印/功能/选重/翻页/切换/命令
   ├─ 2 模式层：中英切换、全半角、标点风格、Caps
   ├─ 3 命令层：`\` 命名空间、`` ` `` 反查、`/` 符号、`;x` 快符
   ├─ 4 编码层：alphabet 校验 → raw 追加/回删
   │     ├─ 顶功判定（max_code_length + auto_select/auto_clear）
   │     ├─ 唯一候选自动上屏（auto_select_unique）
   │     └─ 生成 CandidateList：
   │          a) 精确码命中（按 权重→码表序→用户调整序 排序）
   │          b) 整句模式：hufu-sentence beam 组句（raw>4 或方案启用）
   │          c) 用户词注入、反查候选注入
   ├─ 5 滤镜链：简繁 → 拼音注 → 拆分注 → emoji 注 → 字集过滤 → 去重
   └─ 6 输出 EngineReply
        { Commit(text) | Update{preedit, aux, candidates, caret} | Passthrough | ToggleMode }
   ▼
平台层执行：TSF SetText/InsertTextAtSelection；候选窗刷新
```

## 3. 会话与实例

- `Engine`（进程级）：配置 + 全部方案 + ngram 模型 + 用户词库；可热重载。
- `Session`（每输入上下文一个）：raw 编码、候选页、命令状态、英文缓冲、中英状态。
- daemon `hufu-server` 持有 Engine；前端通过 IPC 连接（Windows 命名管道 `\\.\pipe\hufu-ime`，
  macOS Unix domain socket `~/Library/Application Support/HuFu/hufu.sock`）。
  协议为 JSON 行（每行一个请求/响应），保证可调试、可模拟回放。

## 4. 码表子系统

- 方案（Schema）= `dictionaries/<方案名>/` 目录，加载器按角色识别文件：
  - 主码表：`*.dict.yaml`（rime）、`*多多*`（多多）、`*.txt`（按内容嗅探：`词\t码` vs `码 词 词`）
  - `快符.txt`、`常用符号.txt`、`一简符号.txt`、`补充语料.txt`、`用户调整.txt`
  - `*.注释`（拼音/unicode）、`虎码.拆分`、`*.反查.txt`
- 内存结构：`DictTrie`（前缀树，节点存候选切片）+ `code_index: HashMap<String, Vec<Entry>>`
  （精确命中）+ `rank` 保序。113k 行加载 < 150ms（UTF-8 直读）。
- 原生格式（导出/交换用）：TSV，`码\t词\t权重`，`#hufu-dict v1 name=...` 头。

## 5. 整句子系统（hufu-sentence）

- **模型**：直接加载 TigerClaw 生态的 `sentence-ngram-*.bin`（TCSKNM02 明文布局：
  104B 头 / unigram 数组 / bigram、trigram 分页块 + 稀疏页索引；Kneser-Ney 插值概率）。
  详见 docs/research/rime-config-analysis.md 第三节。
- **解码**：按 raw 前缀位置分桶的 beam search（beam_width 默认 200），
  字级 trigram 打分 + 每字出字奖励 + 名次惩罚 + 孤立生僻惩罚 + 补充语料 AC 自动机奖励。
- **提前上屏**：候选前缀质量占比 ≥ confidence(0.995) 的最长公共前缀，连续 3 键一致才提交。
- **权重全部可调**（config.sentence.*）：beam_width、candidate_limit、emitted_character_reward、
  rank_penalty、isolation_threshold/lambda、confidence、supplement_baseline/scale/maximum、
  以及与码表首选融合的 dict_bias。
- **LLM 重排**：`Reranker` trait，v1 提供 NoopReranker；llama.cpp 子进程（Qwen3 GGUF）作为
  可选实现（与 TigerClaw 相同架构：独立进程、top-5 重排、超时回退 ngram 序）。

## 6. 皮肤子系统（hufu-skin）

- 皮肤 JSON：`colors`(19 角色，兼容 weasel 全部字段名)、`layout`(圆角/间距/边距/阴影/字体)、
  `material`(none/solid/translucent/frosted/glass + tint、noise、blur_radius)。
- Windows：Win11 用 `DWMWA_SYSTEMBACKDROP_TYPE`（Acrylic/Mica）+ DirectComposition，
  Win10 降级 `SetWindowCompositionAttribute` 亚克力，再降级半透明 layered window。
- macOS：`NSVisualEffectView`（material: sidebar/underWindowBackground/hudWindow…，blending: behindWindow）。
- 编辑器：设置 Web UI 内置实时预览（CSS backdrop-filter 模拟毛玻璃），一键导入
  weasel/squirrel 的 `preset_color_schemes`。

## 7. Windows 前端（platform/windows）

- `hufu-tsf`：Rust cdylib，实现 TSF TIP（`ITfTextInputProcessorEx`、`ITfKeyEventSink`、
  `ITfCompositionSink`、`ITfDisplayAttributeProvider`、`ITfThreadMgrEventSink`），
  自注册 DllRegisterServer（写 CLSID + Profile + Categories），参考 weasel WeaselTSF 结构。
- 候选窗：独立线程 + Direct2D 无边框顶层窗（WS_EX_NOACTIVATE|TOPMOST|LAYERED），
  圆角 + 材质背景 + DWrite 文本渲染；跟随光标（ITfContextView::GetTextExt）。
- 托盘/状态：daemon 进程持有托盘图标与中英状态胶囊。

## 8. macOS 前端（platform/macos）

- `HuFuIME`：InputMethodKit（`IMKInputController` 子类 + `IMKServer`），
  候选窗 `NSPanel` + `NSVisualEffectView`；连接 daemon Unix socket。
- 工程为 SPM 包 + bundle 组装脚本（Info.plist：tsInputMethodCharacterIconKey 等），
  参照 Squirrel 的 SquirrelInputController/ SquirrelPanel 结构。

## 9. 设置子系统（settings-ui + hufu-server）

- daemon 监听 `127.0.0.1:动态端口`（端口写入用户目录文件），提供：
  - `GET /` 设置页（SPA，无外部依赖）
  - `WS /ws`：读写 config、皮肤 CRUD + 实时预览推送、用户词管理、码表导入导出、方案切换
- 所有修改经 daemon 校验后原子写入（tmp+rename），并热重载 Engine。

## 10. 安全与边界

- IPC 仅本机（命名管道 ACL 当前用户；socket 文件 0600）。
- 设置页仅监听 loopback，绑定时检查 Referer/Origin 防 CSRF。
- 无遥测；模型与大文件留在用户数据目录。
