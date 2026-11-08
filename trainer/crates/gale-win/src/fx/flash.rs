//! 打对闪光 / 打错抖动 / 数字滚动（疾风视觉语言）。

use super::easing::{apply, Ease};

/// 一次性闪光（打对时字符高亮渐隐）
#[derive(Debug, Clone)]
pub struct Flash {
    pub t: f32,
    pub dur: f32,
}

impl Flash {
    pub fn start(dur: f32) -> Self {
        Self { t: 0.0, dur }
    }
    pub fn tick(&mut self, dt: f32) -> bool {
        self.t += dt;
        self.t < self.dur
    }
    /// 剩余强度 0..1
    pub fn strength(&self) -> f32 {
        1.0 - (self.t / self.dur).clamp(0.0, 1.0)
    }
}

/// 打错抖动（水平衰减正弦）
#[derive(Debug, Clone)]
pub struct Shake {
    pub t: f32,
    pub dur: f32,
    pub amp: f32,
}

impl Shake {
    pub fn start(dur: f32, amp: f32) -> Self {
        Self { t: 0.0, dur, amp }
    }
    pub fn tick(&mut self, dt: f32) -> bool {
        self.t += dt;
        self.t < self.dur
    }
    /// 当前偏移量（px）
    pub fn offset(&self) -> f32 {
        let p = (self.t / self.dur).clamp(0.0, 1.0);
        self.amp * (1.0 - p) * (p * 40.0).sin()
    }
}

/// 数字滚动（顶栏速度数字平滑过渡）
#[derive(Debug, Clone)]
pub struct RollNumber {
    pub cur: f32,
    pub target: f32,
}

impl RollNumber {
    pub fn new(v: f32) -> Self {
        Self { cur: v, target: v }
    }
    pub fn set(&mut self, v: f32) {
        self.target = v;
    }
    pub fn tick(&mut self, dt: f32) {
        let a = apply((dt * 6.0).clamp(0.0, 1.0), Ease::QuadOut);
        self.cur += (self.target - self.cur) * a.max(0.05);
    }
    pub fn display(&self) -> f32 {
        self.cur
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flash_fades() {
        let mut f = Flash::start(0.3);
        assert!((f.strength() - 1.0).abs() < 1e-3);
        assert!(f.tick(0.1));
        assert!(f.strength() < 1.0);
        while f.tick(0.1) {}
        assert_eq!(f.strength(), 0.0);
    }

    #[test]
    fn shake_decays_and_stops() {
        let mut s = Shake::start(0.4, 8.0);
        // t=0 时 sin(0)=0，起跳后才到峰值；包络 (1-p) 线性衰减
        let mut offs = vec![s.offset().abs()];
        while s.tick(0.05) {
            offs.push(s.offset().abs());
        }
        let half = offs.len() / 2;
        let first = offs[..half].iter().cloned().fold(0.0f32, f32::max);
        let second = offs[half..].iter().cloned().fold(0.0f32, f32::max);
        assert!(first > 0.0, "前半段应有抖动");
        assert!(second < first, "包络应衰减: {second} vs {first}");
        assert_eq!(s.offset(), 0.0);
    }

    #[test]
    fn roll_converges() {
        let mut r = RollNumber::new(0.0);
        r.set(200.0);
        for _ in 0..300 {
            r.tick(1.0 / 60.0);
        }
        assert!((r.display() - 200.0).abs() < 1.0, "{}", r.display());
    }
}
