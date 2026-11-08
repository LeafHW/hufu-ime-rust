//! 平滑光标（疾风1代视觉语言：指数趋近 + 闪烁）。

/// 光标动画状态
#[derive(Debug, Clone)]
pub struct SmoothCursor {
    /// 当前显示位置
    pub x: f32,
    pub y: f32,
    /// 目标位置
    pub tx: f32,
    pub ty: f32,
    /// 闪烁相位（0..1 循环）
    pub blink: f32,
}

/// 帧时长(秒) → 相位推进；全速 1.1s 一个周期（可见 0.55 + 隐藏 0.55）
pub const BLINK_PERIOD: f32 = 1.1;

impl SmoothCursor {
    pub fn new(x: f32, y: f32) -> Self {
        Self {
            x,
            y,
            tx: x,
            ty: y,
            blink: 0.0,
        }
    }

    pub fn set_target(&mut self, x: f32, y: f32) {
        self.tx = x;
        self.ty = y;
    }

    /// 每帧：指数趋近（速率与 dt 无关的帧率无关形式：1-e^(-k*dt)）
    pub fn tick(&mut self, dt: f32) {
        let k = 18.0; // 趋近速率
        let a = 1.0 - (-k * dt).exp();
        self.x += (self.tx - self.x) * a;
        self.y += (self.ty - self.y) * a;
        self.blink = (self.blink + dt / BLINK_PERIOD) % 1.0;
    }

    /// 光标当前是否可见（前半周期可见）
    pub fn visible(&self) -> bool {
        self.blink < 0.5
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converges_to_target() {
        let mut c = SmoothCursor::new(0.0, 0.0);
        c.set_target(100.0, 0.0);
        for _ in 0..120 {
            c.tick(1.0 / 60.0);
        }
        assert!((c.x - 100.0).abs() < 0.5, "x={}", c.x);
    }

    #[test]
    fn blink_cycles() {
        let mut c = SmoothCursor::new(0.0, 0.0);
        assert!(c.visible());
        c.tick(0.6); // > 半周期 0.55s → 隐藏
        assert!(!c.visible(), "0.6s 后应隐藏 blink={}", c.blink);
        c.tick(0.6); // 1.2s → 相位回卷到 0.09 → 可见
        assert!(c.visible(), "1.2s 回卷后应可见 blink={}", c.blink);
    }
}
