//! hufu-dict —— 多格式码表加载与检索。
//!
//! 支持的输入格式：
//! - HuFu 原生（TSV：`码<TAB>词[<TAB>权重[<TAB>stem]]`）
//! - Rime `*.dict.yaml`（YAML 头 + columns/import_tables/encoder + 表体）
//! - 多多（`---config@` 头 + `词<TAB>码`，`#固` 置顶、`显示=>输出` 定向输出）
//! - QQ五笔（`词<TAB>码`，无头）
//! - 虎整句（`码 词1 词2 …` 空格分隔，顺序即优先级）
//!
//! 另有符号表 / 注释表 / 拆分表 / 反查表 / 用户调整 / 补充语料的专用加载器。

pub mod annotation;
pub mod dict;
pub mod entry;
pub mod parse;
pub mod schema;
pub mod supplement;
pub mod symbols;
pub mod user;

pub use dict::Dict;
pub use entry::DictEntry;
pub use schema::Schema;
