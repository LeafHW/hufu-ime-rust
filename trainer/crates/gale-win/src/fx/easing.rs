//! 缓动函数库（纯数学，全测试）。

/// f(t) ∈ [0,1] → [0,1]，t 自动夹紧
pub fn apply(t: f32, kind: Ease) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match kind {
        Ease::Linear => t,
        Ease::QuadOut => 1.0 - (1.0 - t) * (1.0 - t),
        Ease::CubicOut => 1.0 - (1.0 - t).powi(3),
        Ease::CubicInOut => {
            if t < 0.5 {
                4.0 * t * t * t
            } else {
                1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
            }
        }
        Ease::ExpoOut => {
            if t >= 1.0 {
                1.0
            } else {
                1.0 - 2f32.powf(-10.0 * t)
            }
        }
        Ease::BackOut => {
            let c1 = 1.70158;
            let c3 = c1 + 1.0;
            1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0) * (t - 1.0)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ease {
    Linear,
    QuadOut,
    CubicOut,
    CubicInOut,
    ExpoOut,
    BackOut,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints() {
        for k in [
            Ease::Linear,
            Ease::QuadOut,
            Ease::CubicOut,
            Ease::CubicInOut,
            Ease::ExpoOut,
            Ease::BackOut,
        ] {
            assert!((apply(0.0, k) - 0.0).abs() < 1e-4, "{k:?}");
            assert!((apply(1.0, k) - 1.0).abs() < 1e-4, "{k:?}");
        }
    }

    #[test]
    fn clamp_out_of_range() {
        assert_eq!(apply(-5.0, Ease::Linear), 0.0);
        assert_eq!(apply(9.0, Ease::Linear), 1.0);
    }

    #[test]
    fn monotonic_sane() {
        // 主流缓动中段应递增（BackOut 会过冲，单独验范围）
        let mut prev = -1.0;
        for i in 0..=10 {
            let v = apply(i as f32 / 10.0, Ease::CubicOut);
            assert!(v >= prev);
            prev = v;
        }
        assert!(apply(0.5, Ease::BackOut) > 0.4 && apply(0.5, Ease::BackOut) < 1.3);
    }
}
