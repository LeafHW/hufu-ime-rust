//! 平台小件收口：托盘 / 候选窗代画 / 打开路径 / 设置页入口。
//!
//! 【Linux 适配 2026-09-19】此前 `pipe.rs` 直接调用 `tray::*`（cfg(windows)
//! 模块）与 `candwin::*`（win32 绘制），非 Windows 编译不过。此处按平台
//! 收口：Windows 行为不变；Linux 用 `xdg-open` 打开设置页/目录，候选窗由
//! 前端自绘（server 侧 no-op），托盘不存在。

use std::path::Path;

/// HTTP 设置页端口（main 解析 `--port` 后写入；打开设置页用）。
static HTTP_PORT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(4390);

pub fn set_http_port(port: u16) {
    HTTP_PORT.store(port, std::sync::atomic::Ordering::Relaxed);
}

/// 仅非 Windows（`open_settings`）使用；Windows 侧暂无读者。
#[cfg_attr(windows, allow(dead_code))]
pub fn http_port() -> u16 {
    HTTP_PORT.load(std::sync::atomic::Ordering::Relaxed)
}

/// 输入法激活态上报（Windows 托盘图标显隐；其他平台 no-op）。
pub fn on_ime_state(active: bool) {
    #[cfg(windows)]
    crate::tray::on_ime_state(active);
    #[cfg(not(windows))]
    let _ = active;
}

/// 打开设置页（Windows 走托盘线程的应用窗口；其他平台 `xdg-open` 浏览器）。
pub fn open_settings() {
    #[cfg(windows)]
    {
        crate::tray::open_settings();
    }
    #[cfg(not(windows))]
    {
        let url = format!("http://127.0.0.1:{}/", http_port());
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

/// 用系统文件管理器打开路径（Windows explorer / Linux xdg-open）。
pub fn open_path(p: &Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(p).spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("xdg-open").arg(p).spawn();
    }
}

/// 越进程候选窗（Windows：server 按用户皮肤代画；其他平台：前端自绘，no-op）。
#[cfg(windows)]
pub fn cand_show(
    items: Vec<(String, String)>,
    raw: String,
    selected: usize,
    x: i32,
    y: i32,
    skin: serde_json::Value,
) {
    crate::candwin::show(
        crate::candwin::CandFrame {
            items,
            raw,
            selected,
            skin,
        },
        x,
        y,
    );
}

#[cfg(not(windows))]
pub fn cand_show(
    _items: Vec<(String, String)>,
    _raw: String,
    _selected: usize,
    _x: i32,
    _y: i32,
    _skin: serde_json::Value,
) {
}

pub fn cand_hide() {
    #[cfg(windows)]
    crate::candwin::hide();
}
