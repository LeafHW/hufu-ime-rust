//! 动效系统（疾风视觉语言延续，全部重做）：
//! 补间/easing、平滑光标（指数趋近+闪烁）、打对闪光、打错抖动、数字滚动、扫描线、粒子。

pub mod cursor;
pub mod easing;
pub mod flash;
pub mod roll;
pub mod shake;

pub use cursor::*;
pub use easing::*;
pub use flash::*;
pub use roll::*;
