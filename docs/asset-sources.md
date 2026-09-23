# 资源来源与许可（assets/）

本仓库 `assets/` 是 Linux 端（fcitx5 前端）随包分发的数据资源，安装脚本默认从这里装配，
**无需外部下载**。各文件的来源、版本与许可如下。

> 模型（ngram 整句 / Qwen3 重排）体积过大，不随仓库分发，见文末「模型」。

## 虎码官方发布（huma.ysepan.com）

**获取路径**：虎码官方资源站 <https://huma.ysepan.com> → 「03 虎码输入法下载」→「①Windows」→「小狼毫」。

| 文件 | 内容 | 官方包 | 版本 | 许可 |
|---|---|---|---|---|
| `码表/虎码字词/tigress.dict.yaml` | 虎码字词主表（Rime `text/weight/code/stem` 列） | 虎码秃版 小狼毫（Win）2026.08.15.7z | 包 2026.08.15；表内 `version: 2026.08.14` | 随本仓库分发，保留原声明 |
| `码表/虎码字词/tigress_ci.dict.yaml` | 词表（tigress `import_tables` 闭包） | 同上 | 同上 | 同上 |
| `码表/虎码字词/tigress_simp_ci.dict.yaml` | 简体词补充（闭包） | 同上 | 同上 | 同上 |
| `码表/虎码单字/tiger.dict.yaml` | 虎码单字表（≈117k 条） | 同上 | 同上 | 同上 |
| `码表/虎整句/`（默认方案） | 整句方案：与「虎码字词」同一套 tigress 单字+词闭包（≈250k）+ 快符/常用符号/一简符号 + 补充语料；**放入模型即启用整句** | 同上 | 同上 | 同上 |

**转换词典（OpenCC 词典数据）**

**获取路径**：与「码表」相同 —— 虎码官方资源站 <https://huma.ysepan.com> →「03 虎码输入法下载」→
「①Windows」→「小狼毫」→ `虎码秃版 小狼毫（Win）2026.08.15.7z`（与码表同包，位于包内 `opencc/` 目录）。

OpenCC 是开源的简繁转换项目；该 `opencc/` 目录是一套 OpenCC 数据目录（含标准配置与虎码的自定义配置
`st_tu.json` 等）。

| 文件 | 内容 | 许可 |
|---|---|---|
| `数据/转换词典/STPhrases.txt`、`STCharacters_Tu.txt` | 简 → 繁：词组表 + 单字表（`STCharacters_Tu` 为 `st_tu` 自定义配置所用的台版变体单字表） | OpenCC 词典数据 Apache-2.0 |
| `数据/转换词典/TSPhrases.txt`、`TSCharacters.txt` | 繁 → 简（词组 / 单字） | 同上 |
| `数据/转换词典/emoji.txt` | 词语 → emoji 注解（候选 emoji 变体用） | 同上 |

**「多多」格式字词**（官方「其它码表包」）：

**获取路径**：同上资源站 → 「03 虎码输入法下载」→「其它码表包」。

| 文件 | 内容 | 官方包 | 许可 |
|---|---|---|---|
| `码表/多多B/多多B常用字词.txt` | 多多格式常用字词（备用方案/格式回归） | 虎码官方版 2026_08_15.7z | 随本仓库分发，保留原声明 |
| `码表/多多B/多多B生僻字.txt` | 多多格式生僻字 | 同上 | 同上 |

## 虎爪输入法（TigerClaw）

**获取**：虎爪输入法发布包，仓库 <https://github.com/lvyww/tigerclaw>（Releases）。

许可：**GPL-3.0**（TigerClaw 项目）。随本仓库分发，保留原项目声明。

| 文件 | 包内路径 | 内容 | 许可 |
|---|---|---|---|
| `数据/注释/拼音.注释` | `TigerClaw/码表/虎码字词/1拼音.注释` | 字 → 拼音（≈42k 行） | GPL-3.0 |
| `数据/注释/unicode.注释` | `TigerClaw/码表/虎码字词/unicode.注释` | 字 → Unicode 分区名 | 同上 |
| `数据/拆分/虎码.拆分` | `TigerClaw/码表/虎码字词/虎码.拆分` | 字 → 部件拆解（候选注释显示） | 同上 |
| `数据/拼音反查/拼音.txt` | `TigerClaw/拼音反查码表/拼音.txt` | 全拼反查表（`` ` `` 反查） | 同上 |
| `码表/*/快符.txt` | `TigerClaw/码表/虎码字词/快符.txt` | `;x` 快符表 | 同上 |
| `码表/*/常用符号.txt` | `TigerClaw/码表/虎码字词/常用符号.txt` | `/xx` 分类符号表 | 同上 |
| `码表/*/一简符号.txt` | `TigerClaw/码表/虎码单字/一简符号.txt` | 一简符号表 | 同上 |
| `数据/音效/key.wav` | `TigerClaw/sounds/KeyNormal.wav` | 按键音 | 同上 |
| `数据/音效/select.wav` | `TigerClaw/sounds/KeyPop.wav` | 选词音 | 同上 |
| `数据/音效/commit.wav` | `TigerClaw/sounds/KeySpace.wav` | 上屏音 | 同上 |
| `数据/音效/page.wav` | `TigerClaw/sounds/KeyFunc.wav` | 翻页音 | 同上 |

## 本项目自撰

| 文件 | 内容 |
|---|---|
| `码表/*/补充语料.txt` | 整句补充语料（新词/个人词权重表，`词 [权重]` 格式；用于提升模型未收录词） |

## 模型（不随仓库分发）

**获取**：GitHub Releases 的「**模型文件**」发布页 ——
<https://github.com/LeafHW/hufu-ime-rust/releases/tag/模型>（Releases 列表第 2 页；资源名 `default.7z`，约 880MB；
每个版本 release 正文的【下载说明】块也给出该链接）。

**Linux 放置**：解压后把「模型」文件夹整体放到 `~/.local/share/hufu/` 下（成为
`~/.local/share/hufu/模型/`；目录已存在时把文件夹内的文件放进去）。

引擎自动探测（`*.bin` = ngram 整句、`*.gguf` = 神经重排，文件名不必匹配），并每 2 秒扫描新放入的模型自动装载。

**整句启用**：Linux 随包方案为 虎整句 / 虎码字词 / 虎码单字 / 多多B，**默认方案即「虎整句」**；
模型就位即自动启用整句（引擎按方案名自动启用，见 config `sentence.auto_enable`；无模型时为纯码表模式）。

| 文件 | 内容 | 许可 |
|---|---|---|
| `模型/…ngram*.bin` | TCSKNM02 ngram 整句模型（≈224MB） | 来源：虎爪生态（TCSKNM02），随「模型文件」发布页分发；许可：以「模型文件」发布页说明为准（未单独声明） |
| `模型/*.gguf` | Qwen3 神经重排模型 | Apache-2.0（Qwen3） |

说明：缺模型时引擎为**纯码表模式**，输入/候选/符号等功能不受影响（仅无整句与神经重排）。

## 机器可校验的台账

上面各节的来源与许可是散文描述；**完整性**由台账保证，改资源时两步走：

```sh
bash platform/linux/checks/check-assets.sh          # 校验：清单 ↔ 实况（路径 / 字节 / sha256）
bash platform/linux/checks/check-assets.sh --write  # 重新生成 assets/MANIFEST
```

- 覆盖范围：`assets/码表/**` 与 `assets/数据/**`（`install.sh` 装配的两棵子树）；`assets/README.md` 不入清单。
- `platform/linux/install.sh` 装配前自动校验，不符即中止（防误替换 / 半途拷贝的资源进用户目录）。
- 清单只记路径 + 字节 + sha256，不重复来源与许可——那两栏以本文档为准。
