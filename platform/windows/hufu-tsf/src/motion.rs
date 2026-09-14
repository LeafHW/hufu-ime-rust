//! 形态动效总控【八十三修·动效重做 2026-09-14 第一阶】
//!
//! 设计（用户定稿方向 + Material 3 container transform 范式）：
//! - 所有视觉形态 = 同一参数空间（Form）里的锚点：S0 隐匿 / S1 光标点
//!   （小圆，高亮色，跟光标）/ S3 候选框。动效 = 参数在锚点间的单曲线
//!   飞行（morph），任何时刻可重定目标（中断逆转自然成立——逆转只是
//!   「从当前参数值飞往新目标」）。
//! - 形变帧不画文本：内容透明度（content）独立于壳（shell），渲染层
//!   用它做内容层淡入淡出（PushLayer 同思路），morph 期间壳层的
//!   D2D 绘制量极小（一个圆角矩形+阴影）。
//! - 三级节奏自适应（Rhythm）：慢打=完整形变（morph_ms），中速=快速
//!   形变（fast_ms），快打（≥3 次收场/秒，单字打法 3-5 字/秒场景）=
//!   脉冲模式（壳不缩回小圆，85%↔100% 弹一下，位置直接滑过去）——
//!   快打时每秒 3-5 次全量形变既费 CPU 也视觉过闹。
//! - 全部动画值由真实时间驱动（Instant→插值），tick 只是采样密度；
//!   DWM 丢帧不产生时间漂移（高刷屏适配）。
//!
//! 渲染层契约（candwin2 消费）：
//! - show()/tick 重绘前调 sample(now) 得到本帧 Form；
//!   w/h 走既有 chrome_override（壳按形变尺寸画），
//!   shell/content 分别乘进整体/内容透明度。
//! - 断段（空格上屏）不再直接 hide：on_collapse() → Phase::Dot 时
//!   由渲染层画小圆（空内容+高亮色圆），驻留 idle_ms 后淡出隐藏。
//! - on_key() 在每次有效键入时调（Pulse 模式的脉冲重臂 + 呼吸归零）。

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// 形态锚点：参数空间的一个点。圆和候选框是同一种形状（圆角矩形），
/// 圆 = 圆角取 w/2 的特例——形变天然连续，无跳变。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Form {
    /// 壳宽高（逻辑 px；与 show() 的内容区 w_out/h_out 同口径，不含阴影边距）。
    pub w: f32,
    pub h: f32,
    /// 圆角半径（逻辑 px）。
    pub corner: f32,
    /// 壳不透明度 0..1（背景/边框/阴影整体）。
    pub shell: f32,
    /// 内容不透明度 0..1（编码行/胶囊/候选字——独立于壳，错峰编排用）。
    pub content: f32,
}

impl Form {
    /// S1 光标点：正圆小点。口径=窗口尺寸（含渲染边距，逻辑 px）——
    /// 与 panel 同口径（panel 含阴影边距），形变全程几何连续。
    pub fn dot(dot_size: f32) -> Self {
        // dot_size=圆直径；窗口边距每侧 6（render_dot 同款）
        let side = dot_size + 12.0;
        Form {
            w: side,
            h: side,
            corner: side * 0.5,
            shell: 1.0,
            content: 0.0,
        }
    }
    /// S3 候选框：尺寸来自布局测量，圆角来自皮肤。
    pub fn panel(w: f32, h: f32, corner: f32) -> Self {
        Form { w, h, corner, shell: 1.0, content: 1.0 }
    }
    /// S0 隐匿：透明度归零，几何保持（避免缩到 0 的除零病态）。
    pub fn hidden_like(cur: Form) -> Self {
        Form { shell: 0.0, content: 0.0, ..cur }
    }
}

/// 打字节奏画像：滚动 2s 窗口内的收场（空格上屏）次数。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tempo {
    /// 慢打（<1.5 次收场/s）：完整形变。
    Full,
    /// 中速（1.5~3 次/s）：快速形变。
    Fast,
    /// 快打（≥3 次/s，单字打法 3-5 字/s）：脉冲模式。
    Pulse,
}

struct Rhythm {
    collapses: VecDeque<Instant>,
}

impl Rhythm {
    fn new() -> Self {
        Rhythm { collapses: VecDeque::new() }
    }
    fn collapse(&mut self, now: Instant) {
        self.collapses.push_back(now);
        self.trim(now);
    }
    fn trim(&mut self, now: Instant) {
        let cutoff = now.checked_sub(Duration::from_secs(2)).unwrap_or(now);
        while let Some(front) = self.collapses.front() {
            if *front < cutoff {
                self.collapses.pop_front();
            } else {
                break;
            }
        }
    }
    fn tempo(&self, now: Instant) -> Tempo {
        let mut n = 0usize;
        let cutoff = now.checked_sub(Duration::from_secs(2)).unwrap_or(now);
        for t in self.collapses.iter().rev() {
            if *t >= cutoff {
                n += 1;
            } else {
                break;
            }
        }
        // 2s 窗口：≥6 次=≥3/s；≥3 次=≥1.5/s
        if n >= 6 {
            Tempo::Pulse
        } else if n >= 3 {
            Tempo::Fast
        } else {
            Tempo::Full
        }
    }
}

/// 一次飞行：from→to 单曲线，t 由真实时间驱动。
struct Flight {
    from: Form,
    to: Form,
    t0: Instant,
    dur: Duration,
}

/// emphasized-decelerate 近似（快出慢收，M3 风格）：二次出、缓收。
fn ease(p: f32) -> f32 {
    1.0 - (1.0 - p) * (1.0 - p)
}

impl Flight {
    fn sample(&self, now: Instant) -> (Form, f32) {
        let total = self.dur.as_secs_f64().max(1e-6);
        let p = (((now - self.t0).as_secs_f64() / total) as f32).clamp(0.0, 1.0);
        let e = ease(p);
        let mix = |a: f32, b: f32| a + (b - a) * e;
        (
            Form {
                w: mix(self.from.w, self.to.w),
                h: mix(self.from.h, self.to.h),
                corner: mix(self.from.corner, self.to.corner),
                shell: mix(self.from.shell, self.to.shell),
                content: mix(self.from.content, self.to.content),
            },
            p,
        )
    }
    fn done(&self, now: Instant) -> bool {
        now >= self.t0 + self.dur
    }
}

/// 动效相位（对外只读语义，变迁经 Motion 方法）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    /// S0：完全隐匿。
    Hidden,
    /// S1：小圆驻留（since 起计时，超 idle_ms 淡出回 Hidden）。
    Dot { since: Instant },
    /// S3：候选框稳态。
    Panel,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Dest {
    Dot,
    Panel,
}

/// sample() 的完整输出：本帧形态 + 派生信号。
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub form: Form,
    /// 0..1，本帧飞行进度（1=已到锚点）。
    pub progress: f32,
    /// 相位（含飞行中按目标计）。
    pub phase: Phase,
    /// Pulse 模式：85%↔100% 脉冲的本帧缩放因子（非 Pulse 恒 1.0）。
    pub pulse_scale: f32,
    /// 窗口是否应该显示（shell>0 且未 Hidden）。
    pub visible: bool,
}

/// 总控。candwin2 持有一个实例；全部方法在窗口线程调用。
pub struct Motion {
    // —— 皮肤参数（show() 时从 skin.motion 注入，缺省=代码默认）——
    pub enabled: bool,
    pub dot_size: f32,
    pub idle_ms: u32,
    pub morph_ms: u32,
    pub fast_ms: u32,
    pub fade_ms: u32,
    /// 小圆最低不透明度（皮肤高亮色自带 alpha 常低至 0.28——直用则
    /// 小圆淡成白点；渲染时取 max(皮肤alpha, dot_alpha)）。
    pub dot_alpha: f32,

    // —— 状态 ——
    phase: Phase,
    dest: Option<Dest>,
    flight: Option<Flight>,
    rhythm: Rhythm,
    last_form: Form,
    pulse_t0: Option<Instant>,
    /// Pulse 生效时最近一次 collapse 的目标形态（脉冲围绕它）。
    panel_form: Form,
    dot_form: Form,
}

impl Motion {
    pub fn new() -> Self {
        let dot = Form::dot(14.0);
        Motion {
            enabled: false, // 第一版默认关（皮肤 motion.enable 实测转正后开）
            dot_size: 14.0,
            idle_ms: 4000,
            morph_ms: 160,
            fast_ms: 90,
            fade_ms: 300,
            dot_alpha: 0.95,
            phase: Phase::Hidden,
            dest: None,
            flight: None,
            rhythm: Rhythm::new(),
            last_form: dot,
            pulse_t0: None,
            panel_form: Form::panel(120.0, 40.0, 10.0),
            dot_form: dot,
        }
    }

    /// 皮肤参数注入（show() 每帧调，幂等）。
    /// 【双形态 2026-09-14】DLL 从管道拿到的皮肤响应是 {skin:{...}}
    /// 嵌套形态（server pipe.rs op=skin），测试直连/设置页预览是顶层
    /// 平铺形态——与 candwin2 现有 layout_f/skin.pointer 双形态同款。
    pub fn apply_skin(&mut self, skin: &serde_json::Value) {
        let m = skin
            .pointer("/skin/motion")
            .or_else(|| skin.pointer("/motion"))
            .or_else(|| skin.get("motion"));
        let Some(m) = m else {
            // 两形态都无 motion 节：保持缺省（enable=false）
            return;
        };
        let b = |k: &str, d: bool| {
            m.get(k).and_then(|x| x.as_bool()).unwrap_or(d)
        };
        let f = |k: &str, d: f32| {
            m.get(k)
                .and_then(|x| x.as_f64())
                .map(|v| v as f32)
                .unwrap_or(d)
        };
        self.enabled = b("enable", false);
        self.dot_size = f("dot_size", 14.0).clamp(6.0, 40.0);
        self.idle_ms = f("idle_timeout_ms", 4000.0).clamp(500.0, 60_000.0) as u32;
        self.morph_ms = f("morph_ms", 160.0).clamp(40.0, 600.0) as u32;
        self.fast_ms = f("fast_morph_ms", 90.0).clamp(30.0, 400.0) as u32;
        self.fade_ms = f("fade_ms", 300.0).clamp(60.0, 1500.0) as u32;
        self.dot_alpha = f("dot_alpha", 0.95).clamp(0.1, 1.0);
        self.dot_form = Form::dot(self.dot_size);
    }

    /// 当前形态值（飞行中=插值值；稳态=锚点）。逆转/重定目标的起点。
    fn current_form(&self, now: Instant) -> Form {
        if let Some(fl) = &self.flight {
            fl.sample(now).0
        } else {
            match self.dest {
                Some(Dest::Dot) => self.dot_form,
                Some(Dest::Panel) => self.panel_form,
                None => self.last_form,
            }
        }
    }

    /// 键入涌现：面板参数=本帧布局测量。幂等（稳态重调只更新目标尺寸）。
    /// 返回 true=需要起动画 tick。
    pub fn on_emerge(&mut self, panel: Form, now: Instant) -> bool {
        self.panel_form = panel;
        self.pulse_t0 = None; // 呼吸/脉冲重臂由 on_key 管
        match (self.phase, self.dest) {
            (Phase::Panel, _) | (_, Some(Dest::Panel)) => {
                // 已是面板/正在飞向面板：尺寸目标更新（结构性变化交给
                // 既有 size_anim 语义，morph 不重复接管尺寸细节——
                // 涌现后稳态尺寸过渡仍走原路径）。
                if self.flight.is_none() {
                    self.last_form = panel;
                }
                false
            }
            _ => {
                // 从 Hidden/Dot 飞向 Panel（快打逆转也走这里：从当前值飞）。
                let tempo = self.rhythm.tempo(now);
                let dur = match tempo {
                    Tempo::Full => self.morph_ms,
                    _ => self.fast_ms,
                };
                let from = self.current_form(now);
                self.flight = Some(Flight {
                    from,
                    to: panel,
                    t0: now,
                    dur: Duration::from_millis(dur as u64),
                });
                self.dest = Some(Dest::Panel);
                self.phase = Phase::Panel; // 语义先就位，形态由飞行插值
                true
            }
        }
    }

    /// 空格上屏/断段收场。返回 true=需要起动画 tick（进入 Dot 或淡出）。
    pub fn on_collapse(&mut self, now: Instant) -> bool {
        self.rhythm.collapse(now);
        let tempo = self.rhythm.tempo(now);
        if tempo == Tempo::Pulse {
            // 脉冲模式：壳不缩回小圆——85%↔100% 弹一下，保持面板稳态。
            self.pulse_t0 = Some(now);
            self.phase = Phase::Panel;
            self.dest = Some(Dest::Panel);
            self.flight = None;
            self.last_form = self.panel_form;
            return true;
        }
        let from = self.current_form(now);
        let dur = match tempo {
            Tempo::Full => self.morph_ms,
            _ => self.fast_ms,
        };
        self.dot_form = Form::dot(self.dot_size);
        self.flight = Some(Flight {
            from,
            to: self.dot_form,
            t0: now,
            dur: Duration::from_millis(dur as u64),
        });
        self.dest = Some(Dest::Dot);
        self.phase = Phase::Dot { since: now };
        true
    }

    /// 任何有效键入（含非首键）：脉冲/呼吸相位重臂。Pulse 模式下这保证
    /// 「连打时壳始终活着」；非 Pulse 模式仅清 pulse（无操作代价）。
    pub fn on_key(&mut self, now: Instant) {
        if self.pulse_t0.is_some() {
            self.pulse_t0 = Some(now);
        }
    }

    /// 本帧形态采样。渲染层每帧调（show 与 tick 重绘都要）。
    pub fn sample(&self, now: Instant) -> Frame {
        // 飞行插值
        let (mut form, mut progress) = if let Some(fl) = &self.flight {
            let (f, p) = fl.sample(now);
            (f, p)
        } else {
            match self.dest {
                Some(Dest::Dot) => (self.dot_form, 1.0),
                Some(Dest::Panel) => (self.panel_form, 1.0),
                None => (self.last_form, 1.0),
            }
        };
        // S1 驻留超时淡出：shell/content 同时沉（几何不动）。
        if let Phase::Dot { since } = self.phase {
            if self.flight.is_none() || self.dest != Some(Dest::Dot) || progress >= 1.0 {
                let idle = Duration::from_millis(self.idle_ms as u64);
                if now >= since + idle {
                    let p = (((now - since - idle).as_secs_f64()
                        / self.fade_ms.max(1) as f64) as f32)
                        .clamp(0.0, 1.0);
                    let a = 1.0 - ease(p);
                    form.shell *= a;
                    form.content *= a;
                }
            }
        }
        // 脉冲因子：90ms 内 85%↔100%（正弦半周期）。
        let mut pulse_scale = 1.0f32;
        if let Some(t0) = self.pulse_t0 {
            let p = (((now - t0).as_secs_f64() / 0.090) as f32).clamp(0.0, 1.0);
            // 0→1：先缩到 0.85 再回 1（cos 半周期）
            pulse_scale = 0.85f32 + 0.15f32 * (-(std::f32::consts::PI * (1.0 - p)).cos()).min(1.0);
            // 收敛保护
            pulse_scale = pulse_scale.clamp(0.85, 1.0);
        }
        let visible = form.shell > 0.004 && self.phase != Phase::Hidden;
        Frame { form, progress, phase: self.phase, pulse_scale, visible }
    }

    /// tick 推进（FADE_TIMER 消费）：飞行结束/淡出结束的收尾。
    /// 返回 Some(true)=仍在动画需继续 tick；Some(false)=动画全部结束可
    /// 停 tick；淡出完自动转 Hidden（渲染层看到 visible=false 后收窗）。
    pub fn tick(&mut self, now: Instant) -> Option<bool> {
        // 飞行完成收尾
        if let Some(fl) = &self.flight {
            if fl.done(now) {
                self.last_form = fl.to;
                self.flight = None;
            }
        }
        // 脉冲结束
        if let Some(t0) = self.pulse_t0 {
            if now >= t0 + Duration::from_millis(90) {
                self.pulse_t0 = None;
            }
        }
        // 淡出完成 → Hidden
        if let Phase::Dot { since } = self.phase {
            let idle = Duration::from_millis(self.idle_ms as u64);
            let fade_done = now >= since + idle + Duration::from_millis(self.fade_ms as u64);
            if fade_done {
                self.phase = Phase::Hidden;
                self.dest = None;
                self.last_form = self.dot_form;
                return Some(false);
            }
        }
        let animating = self.flight.is_some()
            || self.pulse_t0.is_some()
            || matches!(
                (self.phase, self.dest),
                (Phase::Dot { since }, _) if now >= since + Duration::from_millis(self.idle_ms as u64)
            );
        Some(animating)
    }

    /// 硬隐藏（外部 SW_HIDE 路径同步状态——失焦收窗/宿主切换等）。
    pub fn on_external_hide(&mut self) {
        self.phase = Phase::Hidden;
        self.dest = None;
        self.flight = None;
        self.pulse_t0 = None;
        self.last_form = self.dot_form;
    }

    /// 当前是否处于 S1 小圆驻留（渲染层决定画小圆还是面板）。
    pub fn is_dot_dwell(&self) -> bool {
        matches!(self.phase, Phase::Dot { .. }) && self.dest == Some(Dest::Dot)
    }

    /// 是否正处于形态飞行中（涌现/收场 morph——渲染层据此接管壳
    /// 尺寸/圆角/内容透明度）。
    pub fn in_flight(&self) -> bool {
        self.flight.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel() -> Form {
        Form::panel(240.0, 90.0, 10.0)
    }

    #[test]
    fn pipe_form_skin_enables_motion() {
        // 管道形态：server op=skin 返回 {skin:{motion:...}}（嵌套）——
        // v1 实机断点实证（只查 /motion 落空→enabled=false 恒关）。
        let mut m = Motion::new();
        let skin = serde_json::json!({
            "skin": {"motion": {"enable": true, "dot_size": 18.0, "idle_timeout_ms": 5000.0}}
        });
        m.apply_skin(&skin);
        assert!(m.enabled, "管道嵌套形态必须开启 motion");
        assert!((m.dot_size - 18.0).abs() < 0.01);
        assert_eq!(m.idle_ms, 5000);
        // 平铺形态（测试直连/设置页预览）也通
        let mut m2 = Motion::new();
        let flat = serde_json::json!({"motion": {"enable": true}});
        m2.apply_skin(&flat);
        assert!(m2.enabled);
    }

    #[test]
    fn emerge_flies_from_dot_to_panel() {
        let mut m = Motion::new();
        m.enabled = true;
        let now = Instant::now();
        m.on_emerge(panel(), now);
        let f0 = m.sample(now);
        // 起点接近 dot（尺寸从 14 起飞）
        assert!(f0.form.w < 40.0, "起点应从小尺寸起飞，got {}", f0.form.w);
        let f_end = m.sample(now + Duration::from_millis(200));
        assert!((f_end.form.w - 240.0).abs() < 1.0, "终点应到面板宽");
        assert!(f_end.form.content > 0.9, "终点内容全显");
    }

    #[test]
    fn collapse_then_idle_fade_to_hidden() {
        let mut m = Motion::new();
        m.enabled = true;
        let now = Instant::now();
        m.on_emerge(panel(), now);
        m.tick(now + Duration::from_millis(300));
        let t1 = now + Duration::from_millis(400);
        assert!(m.on_collapse(t1));
        assert!(m.is_dot_dwell());
        // 驻留中：小圆全显
        let mid = m.sample(t1 + Duration::from_millis(500));
        assert!(mid.visible && mid.form.shell > 0.99);
        // 超时+淡出完成
        let t2 = t1 + Duration::from_millis(4000 + 400);
        m.tick(t2);
        let end = m.sample(t2);
        assert!(!end.visible, "淡出后不可见");
        assert_eq!(m.phase, Phase::Hidden);
    }

    #[test]
    fn fast_typing_switches_to_pulse() {
        let mut m = Motion::new();
        m.enabled = true;
        let now = Instant::now();
        m.on_emerge(panel(), now);
        // 模拟 1.2s 内 4 次收场（>3/2s → Fast；再两次 → Pulse）
        let mut t = now + Duration::from_millis(500);
        for _ in 0..6 {
            m.on_collapse(t);
            t += Duration::from_millis(250);
        }
        // 第 6 次收场后（2s 窗口内 6 次）应为 Pulse：不再进入 Dot
        assert!(!m.is_dot_dwell(), "快打收场应保持面板（脉冲）");
        let f = m.sample(t);
        assert!((f.pulse_scale - 0.85).abs() < 0.15, "脉冲因子应在谷底附近");
    }

    #[test]
    fn midflight_reverse_to_panel() {
        let mut m = Motion::new();
        m.enabled = true;
        let now = Instant::now();
        m.on_emerge(panel(), now);
        m.tick(now + Duration::from_millis(200));
        // 收场到一半时又键入——从当前值飞回面板
        let t1 = now + Duration::from_millis(300);
        m.on_collapse(t1);
        let mid = now + Duration::from_millis(370);
        let f_mid = m.sample(mid);
        assert!(f_mid.form.w < 240.0, "收场中应小于面板宽");
        let f_rev = m.sample(mid);
        let _ = f_rev;
        // 逆转：on_emerge 从当前 last_form 起飞——但 last_form 在飞行中
        // 尚未更新（tick 收尾才更新），逆转起点应取当前采样值。
        m.on_emerge(panel(), mid);
        let f2 = m.sample(mid + Duration::from_millis(1));
        assert!(f2.form.w < 240.0, "逆转起点应接近中途值而非 14");
        let f3 = m.sample(mid + Duration::from_millis(300));
        assert!((f3.form.w - 240.0).abs() < 1.0);
    }
}
