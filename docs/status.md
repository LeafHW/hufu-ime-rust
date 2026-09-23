# 虎符 HuFu · 当前进度与测试

> 本页由 README 的「当前进度 / 测试」段拆出，随版本持续更新。

## 模块进度

| 模块                     | 状态       | 实测                                                         |
| ------------------------ | ---------- | ------------------------------------------------------------ |
| `hufu-types`             | ✅          | —                                                            |
| `hufu-dict`              | ✅          | 虎码单字 113k 条/272ms、虎码字词（import 闭包）246k 条/1.2s、QQ五笔 96k/114ms、多多 B 定制 33k/81ms；置顶/软删回放（最新在前、无重复、pinned 标记） |
| `hufu-config`            | ✅          | —                                                            |
| `hufu-engine`            | ✅          | 真实码表 REPL：顶屏（`tuj`+死端字母推字 𪚠）、`jd`+`;` 次选、注释/拼音/分区回显；动态变量 `\da`→真实日期、`\n12345`→一万二千三百四十五、`\N1234`→壹萬贰仟大写金额、**`\calc(1+2)*3`→上屏 9**（HTTP 在线实测）、**`\w就就`→构码 jj 入库**（Rime encoder fixture）；Ctrl+Shift+数字 置顶 / Ctrl+Delete 软删（日志落盘+回放）；音效标签 key/select/commit/page；OpenCC 繁体变体（真实 ST 表：来→來、那个→那個 ⚑繁 + emoji） |
| `hufu-sentence`          | ✅          | 真实 TCSKNM02 224MB 模型加载 87ms；`tujatuja`→「我们我们」、`mfyto`→「大一点我是」；单次组句 0.4–2.8ms；提前上屏提案 |
| `hufu-skin`              | ✅          | 19 颜色角色 + 材质模型；weasel 配色互导（含 0xAABBGGRR ↔ #RRGGBBAA） |
| `hufu-cli`               | ✅          | check / convert / repl                                       |
| `hufu-server` + 设置 GUI | ✅          | 20 REST 路由（+候选置顶/隐藏/音效试听/全量快照导出）+ `\\.\pipe\hufu-ime` 命名管道 + Unix socket（macOS）；pipeclient 全操作通过；40KB 单文件设置 UI（试用台/方案/整句权重 10 滑杆/皮肤编辑器实况预览/用户词+置顶隐藏/任意候选调整/音效开关+试听/繁简开关/快照导出/导入导出） |
| Windows TSF              | ✅ 真机全通 | `hufu_tsf.dll`（纯 Rust + windows-rs 0.58）；**系统级激活实测**：Win+空格 第 4 项（虎图标）、汉字上屏、候选窗贴光标跟随、选区顺序正确；**应用矩阵**：记事本/浏览器/VSCode/QQ/DSH/Listary 全通过。注册九步一键化（install.ps1 + reg-fix.ps1）；DLL 轨迹日志 `%TEMP%\hufu-tsf-trace.log`。运行时铁律：EditSession 用 ASYNCDONTCARE、组段走 GetSelection→StartComposition、GetTextExt 即屏幕坐标 |
| macOS IMK                | 🔨 骨架     | HuFuInputController（键码→Unix socket→组段/上屏）+ CandidatePanel（NSVisualEffectView 四材质）+ Info.plist + build.sh；帧协议与 Windows 管道一致；**需在 Mac 上编译迭代** |
| Linux fcitx5             | ✅ 可用     | `platform/linux`：Rust staticlib（Unix socket 客户端 + C ABI）+ C++ 薄壳；候选/组段/上屏/**候选点击上屏**；fcitx5-configtool 设置页；**英文输入由 fcitx5 键盘布局提供**（引擎不带中英切换，Shift/Caps 不下发）；**状态栏「虎符」托盘菜单**（重载码表 / 打开方案文件夹 / 按键音效 / 引擎状态 / 候选窗显示预编辑，后者默认开、切换即时生效）；**字反查**（默认 `~`，取光标左侧汉字，两排显示拼音与虎码·拆分，数据取自随包资源）；install.sh 系统级装 addon + systemd user 服务 + **仓库 `assets/` 自带码表/资源装配**（注释/拆分/全拼反查/符号/音效 wav；含默认「虎整句」方案），装/卸按 `assets/MANIFEST` 台账校验、两个脚本都支持 `--dry-run`；**待实机 fcitx5 全量回归** |

## 测试

- 引擎 workspace：**95 测试 0 失败**（Linux/Windows 双端跑；1 个作者本机对照件默认 `#[ignore]`）（字典格式/引擎状态机/动态变量/数字转中文/置顶回放/整句/Shift 标点/音效标签/皮肤/配置/GGUF f16/GEMM/q8 对 llama.cpp F32 基准/wav 解析）
- Linux 前端单测：hufu-fcitx5-client 13/13（mock socket：commit/update/回删/透传/断线直通；含 重载码表·打开方案文件夹·音效开关与读态 四个薄封装、字反查索引解析与三类降级）
- Linux 冒烟：真实码表 server + Unix socket（ping/key/state/中英切换）+ HTTP 设置页 + 三方案装配（脚本 `_tmp/dev-data/smoke.py`）
- Linux 回归电池：`cargo run -p hufu-cli --example socketbattery` **22/22**（协议帧/键流：候选·数字选重·`;` 次选·`'` 三选·退格·Esc·顶屏·空格上屏·`-/=` 翻页/方案列表与切换/音效开关/Shift·Caps 不切中英（Linux 策略）/focus·reset/HTTP）
- 管道回归电池：lock 12/12、battery2 16/16、edge 17/17、flow 全过、设置生效性 7/7（皮肤热反映/横排/序号/延时/音效/调整日志）
- Windows 冒烟：12 步 exit=0（COM 层 + msctf + 管道 + 候选窗 v2 四材质，横竖排各验一轮）
- 重排端到端：`bwjdsk` → Qwen3 翻转 `[弱斗该,嫁𡀲]→[嫁𡀲,弱斗该]`，二次输入缓存即时生效
- **万句基准（tbench v2）**：新语料 1 万句、整句虎规则逐字全码连打——准率 **99.50%**、提前上屏覆盖 ~40%、残留码长 3.5 键、触达 p50 ~6ms；各版本基准与口径见 [benchmark-latency-100.md](benchmark-latency-100.md) / [benchmark-qwen-vs-ngram.md](benchmark-qwen-vs-ngram.md)

## 整句 A/B 压测（5 万句语料）

```powershell
# 语料：prepare_corpus_50k.py 生成（LCCTS/THUCNews/评论 混配，4~30 字纯汉字句）
cd engine
cargo run --release -p hufu-rerank --bin sentence-bench -- `
  <语料目录>\test_sentences_50k.txt --arm AB --sample 2000 --wa 8 --wb 2 `
  --out ..\docs\benchmark-qwen-vs-ngram.md
```

录入规则完全按整句虎：逐字全码连打（一简字取 2 码全码）、一句打完才空格；提前上屏前缀由引擎提交累计。A 臂 ngram 全量；B 臂 +Qwen 重排在停顿后空格前一次性介入（同生产路径）。输出 Wilson 95% CI、按句长分桶、打捞/拖累翻转统计。报告见 [benchmark-qwen-vs-ngram.md](benchmark-qwen-vs-ngram.md)。
