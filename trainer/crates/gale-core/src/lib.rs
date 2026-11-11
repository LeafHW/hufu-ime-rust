//! # gale-core —— 疾风2代跟打器·平台无关核心
//!
//! 铁律（见 `_research/architecture.md` §0）：
//! 1. 本 crate 禁止任何 GUI / Win32 / 平台依赖——它必须能被任何前端（Win32/未来macOS）复用。
//! 2. 所有对外类型实现 Serialize/Deserialize + Clone + Debug（成绩要落盘、配置要持久化）。
//! 3. 统计口径以晴跟打 Pro / 疾风1代实测为准，改动必须配单元测试。

pub mod compare;
pub mod config;
pub mod library;
pub mod mini_toml;
pub mod normalize;
pub mod report;
pub mod rules;
pub mod score;
pub mod scorecard;
pub mod segment;
pub mod single;
pub mod stats;
pub mod theme;
pub mod wenlai;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const APP_NAME: &str = "虎符跟打";

/// 本项目自有 Result/错误类型（网络隔离期无法拉取 anyhow；语义等价，接口稳定）
pub type GaleResult<T> = Result<T, GaleError>;

#[derive(Debug, thiserror::Error)]
pub enum GaleError {
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("数据格式: {0}")]
    Format(String),
    #[error("{0}")]
    Msg(String),
}

#[macro_export]
macro_rules! gale_err {
    ($($arg:tt)*) => {
        Err($crate::GaleError::Msg(format!($($arg)*)))
    };
}

#[cfg(test)]
pub(crate) mod test_util {
    /// 测试临时目录：火绒行为检测会拦截“高频自删临时目录的 exe”写
    /// %TEMP%（os error 5 拒绝访问，与 ACL 无关——见 _research 排障记录），
    /// 改用 E 盘 crate 旁的固定测试目录，避开误杀。
    pub(crate) fn tmp(tag: &str) -> std::path::PathBuf {
        let base = std::env::temp_dir().join("hufu-trainer-core-tests");
        let _ = std::fs::create_dir_all(&base);
        base.join(format!("{tag}-{}", std::process::id()))
    }
}
