//! 虎符跟打器的 Win32 平台层，沿用 gale 内部 crate 名称。
//!
//! `input_host` 使用真实可见的 RichEdit50W 接收系统输入法上屏和组字；
//! `edit` 提供弹窗的原生编辑框，`window` 管理窗口、焦点与事件队列。
//! Direct2D / DirectComposition 绘制正文及面板，原生子控件负责文本编辑。
//! `clipboard_watcher` / `clipboard_bridge` 提供显式启用的剪贴板能力。
//! `keys` 提供发群增强链专用的 Ctrl+V/回车注入。
//! `ime_imm`（自建 HIMC 路线）已删除：probe-imm REPORT 证伪，不再保留。

pub mod clipboard_bridge;
pub mod clipboard_watcher;
pub mod color_picker;
pub mod d2d;
pub mod edit;
pub mod fx;
pub mod input_host;
pub mod keys;
pub mod window;
