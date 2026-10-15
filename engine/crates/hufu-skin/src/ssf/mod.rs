//! 【SSF M1】.ssf 皮肤文件解析——三代容器 + skin.ini + 定义模型。
//!
//! 支持三种容器（参考《虎符SSF皮肤系统设计方案》§二）：
//! - 明文 ZIP（皮肤编辑器产物；虎娘自制皮肤同此）；
//! - "Skin" v1 明文容器（安装目录 Skin.dat 用）；
//! - "Skin" v3 加密容器（搜狗商店分发：AES-256-CBC 固定公开密钥 + zlib + 偏移表）。
//!
//! 安全基线（对齐虎娘 native/SsfArchive.cpp，详见设计文档 §1.1）：
//! 全程内存解析、永不落盘；输入/解压/单资产三级预算；CRC32 校验；
//! 路径消毒（拒绝穿越/设备名/非法字符）；条目重叠检测。
//! 解析原则：**尽力加载**——坏字段降级为 warning（进 [`SsfDefinition::warnings`]），
//! 整包不可读才返回 Err。皮肤错误绝不能弄坏输入法。
//!
//! 本模块只做数据：不解码图片像素（那是渲染端/overlaywin 的活），
//! 资源以归一路径（小写、`/` 分隔）→ 原始字节的形式交给上层。

pub mod container;
pub mod ini;
pub mod model;

#[cfg(test)]
mod tests;

pub use container::{decode_archive, sanitize_path, Files};
pub use ini::decode_text;
pub use model::{
    parse_definition, SsfDecoration, SsfDefinition, SsfDisplay, SsfInsets, SsfLayout, SsfSchemes,
    SsfStretch,
};

/// 单个 .ssf 输入上限（64 MiB，虎娘口径）。
pub const MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;
/// 解压总预算（128 MiB，防 zip 炸弹）。
pub const MAX_EXPANDED_BYTES: usize = 128 * 1024 * 1024;
/// 单条目（skin.ini / 图片）上限（64 MiB）。
pub const MAX_ASSET_BYTES: usize = 64 * 1024 * 1024;
/// 条目数上限。
pub const MAX_ENTRIES: usize = 4096;
/// 条目名长度上限（字节）。
pub const MAX_NAME_BYTES: usize = 4096;

/// 解析错误。全部为面向用户的中文消息（可直接进设置页显示），
/// 与虎娘 "皮肤加载失败，已使用内置默认外观：<原因>" 的呈现方式对齐。
#[derive(Debug, thiserror::Error)]
pub enum SsfError {
    #[error("{0}")]
    Message(String),
}

/// 一站式：字节 → (条目表, 皮肤定义)。
pub fn parse(bytes: &[u8]) -> Result<(Files, SsfDefinition), SsfError> {
    let files = decode_archive(bytes)?;
    let definition = parse_definition(&files)?;
    Ok((files, definition))
}
