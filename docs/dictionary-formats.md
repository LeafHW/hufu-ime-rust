# HuFu 支持的码表格式

所有格式由 `hufu-dict::parse` 统一处理：编码探测 UTF-8 → GBK 回退，`sniff_format` 嗅探后分派解析器。解析产物为 `RawTable { rows: Vec<DictEntry>, meta: TableMeta }`（columns / import_tables / encoder 规则等元数据）。

## 1. HuFu 原生 TSV（`Native`）

```text
#hufu-dict v1
的	u	1035947.0
我们	tuja	50000
```

- 列序固定：`code \t text \t weight`（weight 可省）。
- 首行 `#hufu-dict` 头。无头时按首列是否含 CJK 与其他 TSV 区分。

## 2. Rime dict.yaml（`RimeYaml`）

```yaml
name: tiger
version: "2026.02.28"
sort: by_weight
use_preset_vocabulary: false
columns:
  - text
  - code
  - weight
...
的	u	10359470
```

- YAML 头 + `...` 结束行；部分文件省略 `---` 起始行（虎码发行版即如此，嗅探器按 `key: value` 形状识别）。
- `columns` 声明列序（默认 text/code/weight）；`import_tables` 声明合并导入；`encoder` 段落提取构词规则（`max_phrase_length`/`min_length_weight` 与根/子规则）。
- 主表选择：未被其他表导入者优先；其中聚合表（自身有 import_tables）> 与目录同名 > 已知主名（tiger/tigress）> 最大文件；随后按 import 闭包 BFS 合并。

## 3. 多多输入法（`Duoduo`）

```text
---config@ChangJie78
词组	dvdi
单字	l
#固顶词
的	d
显示文本=>实际输出	disp
```

- `---config@` 头；列序 `词 \t 码`（词在前）。
- `#固` 前缀行 = 固顶（pinned）。
- `显示=>输出` = 提交改写（commit_override，候选显示「显示」上屏「输出」）。

## 4. QQ五笔系（`WordFirstTsv`，无头）

```text
工	a
戈	a
工	aaaa
```

- 无头、`词 \t 码`、CRLF/UTF-8。与多多的区别：无 `---config@` 头、无 `=>` 语义（按普通词条处理）。

## 5. 虎整句（`SpaceCodeWords`）

```text
t 我 我们
u 的 工作
；a ！
/tm ™
```

- 空格分隔：`码 词1 词2 …`；词序即名次（rank）。
- `;x` / `/xx` 行进入符号命名空间（快符 / 斜杠符号）。

## 6. 补充语料 / 用户调整 / 注释（伴随文件）

按文件名角色识别（方案目录内）：

| 文件 | 角色 |
| --- | --- |
| `快符.txt` | 快符表 `;x` → 符号 |
| `常用符号.txt` | 斜杠符号表 `/xx` |
| `一简符号.txt` | 一简符号（单码符号） |
| `补充语料.txt` | 整句补充语料（词 + 权重） |
| `用户调整.txt` | 用户调整（置顶/调频/删词回放） |
| `用户词.txt` | 用户词库 |
| `*拼音.注释` | 拼音注释表 |
| `unicode.注释` | Unicode 分区注释 |
| `*.拆分` | 虎码拆分注释 |
| `*反查*.txt` | 反查码表（拼音/双拼 → 字） |

## TCSKNM02 ngram 模型（整句）

TigerClaw 生态 `sentence-ngram-*.bin`（明文布局；`TCSKNM01 v2` 为加密版，不支持）。

- 104 字节头（小端）：`magic[8]="TCSKNM02"`, `version:i32@8`, `header_size:i32@12`, `file_size:i64@16`, `index_stride:i64@24`, `uni_count:i64@32`, `uni_off:i64@40`, `bi_ctx_count:i32@48`, `bi_index_count:i32@52`, `bi_blocks_off:i64@56`, `bi_index_off:i64@64`, `tri_ctx_count:i64@72`, `tri_index_count:i32@80`, `tri_blocks_off:i64@88`, `tri_index_off:i64@96`。
- unigram：`uni_count × { cp:i32, prob:f32 }` 按码点升序；`[0]` = unknown 回退概率；`cp=0x03` 为 EOS。
- bigram / trigram：分页块（每页 `index_stride` 个上下文）+ 稀疏页索引（`{ first_ctx_key:i64, block_offset:i64 }` 按键升序）。
- 上下文记录：`{ key:i64, lambda:f32, succ_count:i32, succ[] × { cp:i32, prob:f32 } }`。
- 键：bigram ctx = 前一字符码点本身（BOS=0x02）；trigram ctx = `w1 × 2^21 + w2`。
- 概率：`P2(c|w) = p2 + λ2(w)·P1(c)`；`P3(c|w1,w2) = p3 + λ3(w1,w2)·P2(c|w2)`。

实测（sentence-ngram-mobile.bin，224MB）：加载 87ms，单次组句 0.4–2.8ms。
