//! hufu-dict —— 多格式码表加载与检索。
//!
//! 支持的输入格式（全部自动嗅探，无需配置）：
//! - HuFu 原生（TSV：`码<TAB>词[<TAB>权重[<TAB>stem]]`）
//! - Rime `*.dict.yaml`（YAML 头 + columns/import_tables/encoder + 表体）
//! - 多多（`---config@` 头 + `词<TAB>码`，`#固` 置顶、`显示=>输出` 定向输出）
//! - QQ五笔（`词<TAB>码`，无头）
//! - 词前空格式（`词  码`，空格分隔、词在前——与 TAB 行可混排）
//! - 虎整句（`码 词1 词2 …` 空格分隔，顺序即优先级）
//!
//! 【数字编码码表自动适配 2026-09-05】词条编码含数字字符（如
//! `a8=来`、`u3=的` 的数字第二码位体系）时，加载后 Dict.digit_coded
//! 自动置位，引擎据此把 raw 里的数字当编码字符而非「选重第 N」
//! （跨段切分如 `vv|b8`=比如 也支持），选重锁（分号/单引号）不受
//! 影响。普通码表（虎码类）行为不变。
//!
//! 另有符号表 / 注释表 / 拆分表 / 反查表 / 用户调整 / 补充语料的专用加载器。

pub mod annotation;
pub mod dict;
pub mod entry;
pub mod opencc;
pub mod parse;
pub mod schema;
pub mod supplement;
pub mod symbols;
pub mod user;

pub use annotation::AnnotationTable;
pub use dict::Dict;
pub use entry::DictEntry;
pub use opencc::OpenCc;
pub use schema::Schema;
