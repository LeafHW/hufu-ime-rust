//! 数字滚动/滚动条目动画独立模块（主体在 flash.rs 的 RollNumber；此处放段落进度条等）。

use super::easing::{apply, Ease};

/// 进度条平滑（0..1）
#[derive(Debug, Clone)]
pub struct SmoothProgress {
    pub cur: f32,
    pub target: f32,
}

impl SmoothProgress {
    pub fn new() -> Self {
        Self {
            cur: 0.0,
            target: 0.0,
        }
    }
    pub fn set(&mut self, p: f32) {
        self.target = p.clamp(0.0, 1.0);
    }
    pub fn tick(&mut self, dt: f32) {
        let a = apply((dt * 5.0).clamp(0.0, 1.0), Ease::CubicOut).max(0.04);
        self.cur += (self.target - self.cur) * a;
    }
    pub fn frac(&self) -> f32 {
        self.cur
    }
}

impl Default for SmoothProgress {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_smooth_to_end() {
        let mut p = SmoothProgress::new();
        p.set(1.0);
        for _ in 0..600 {
            p.tick(1.0 / 60.0);
        }
        assert!((p.frac() - 1.0).abs() < 0.01, "{}", p.frac());
    }

    #[test]
    fn clamp_input() {
        let mut p = SmoothProgress::new();
        p.set(3.0);
        assert_eq!(p.target, 1.0);
    }
}
