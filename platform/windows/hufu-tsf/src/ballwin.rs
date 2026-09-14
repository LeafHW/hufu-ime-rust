//! 动效 v5·球窗（ball window）：候选框=一个可变形元素（球↔框）。
//!
//! 架构（用户定稿）：
//! - **一个元素**：`ContainerVisual`（挂 `DesktopWindowTarget`=候选 HWND）。
//!   球态和框态都是它——clip 从球小圆无级展开成框圆角矩形（可动画
//!   `CompositionRoundedRectangleGeometry`），内容层=框尺寸 surface，
//!   D2D 把球帧/框帧画进同一张。
//! - **合成器线程驱动**：morph/呼吸/跟随全部 WinRT KeyFrame/Expression/
//!   Implicit 动画——帧率=显示器刷新率（高刷自适应），IME 进程动画期间
//!   零 CPU（对比 v4 的 CPU 16ms tick：固定 60Hz+timer 静默杀坑）。
//! - **真阴影**：`Compositor.CreateDropShadow()`（默认跟内容 alpha 轮廓；
//!   v4 的 IDCompositionDevice3 QI 失败坑在这套 API 不存在）。
//! - **专用合成线程**：宿主（TSF 按键线程）apartment 不可控——本机
//!   实证宿主线程 CreateDispatcherQueueController 返回 ok 但线程实际
//!   无 DQ（Compositor→80070005 "caller must initialize DispatcherQueue"）。
//!   干净专线程 100% 可行（composite_probe 探针实证）。所有合成调用
//!   困在线程内；窗口线程只 Send 命令（非阻塞）。
//! - **节奏（用户定稿）**：球→框快（~150ms）；框→球慢（~260ms）且
//!   **可被键入打断反转**（同属性 StartAnimation 替换旧动画、从当前
//!   视觉值继续——连打空格防闪烁）；3-5s 无键入渐隐。

use std::sync::mpsc::Sender;
use windows::core::{Interface, HSTRING};
use windows::Foundation::Numerics as N;
use windows::Foundation::TimeSpan;
use windows::UI::Composition::Desktop::DesktopWindowTarget;
use windows::UI::Composition::{
    Compositor, ContainerVisual, DropShadow, SpriteVisual,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::WinRT::Composition::{
    ICompositorDesktopInterop, ICompositorInterop,
};

/// 球径（px）。
pub const BALL_D: f32 = 22.0;
/// 球画布边距（阴影余量；画布=BALL_D+2*PAD）。
pub const BALL_PAD: f32 = 11.0;
/// 球→框（快）。
pub const MORPH_OPEN_MS: i32 = 150;
/// 框→球（慢——连打空格防闪烁；可被键入打断反转）。
pub const MORPH_CLOSE_MS: i32 = 260;
/// 渐隐时长。
pub const BALL_FADE_MS: i32 = 260;
/// 无键入驻留（渐隐启动点，皮肤 motion.idle_timeout_ms 可覆盖）。
pub const BALL_IDLE_MS: u32 = 4000;

fn dur(ms: i32) -> TimeSpan {
    TimeSpan::from(std::time::Duration::from_millis(ms as u64))
}

/// v5 球窗状态机形态。
#[derive(Clone, Copy, PartialEq)]
pub enum BallForm {
    /// 球（驻留态）。
    Ball,
    /// 框（候选态）。
    Panel,
    /// 隐藏。
    Hidden,
}

/// 窗口线程 → 合成线程 命令（全部非阻塞 Send）。
pub enum BallCmd {
    /// 画球帧（环色/心色）。
    PaintBall { ring: u32, core: u32 },
    /// 元素平移到（窗口坐标，相对 HWND 左上角）。
    MoveTo { x: f32, y: f32 },
    /// 球→框 morph（快）。(w,h,corner)。
    MorphOpen { w: f32, h: f32, corner: f32 },
    /// 框→球 morph（慢，可被 MorphOpen 打断反转）。
    MorphClose,
    /// 画框帧进 surface（框尺寸=(w,h)——内容=候选面板渲染结果）。
    ResizeSurface { w: f32, h: f32 },
    /// 渐隐。
    FadeOut,
    /// 渐显（Opacity=1）。
    FadeIn,
    /// 呼吸微动效 开/关。
    BreathOn,
    BreathOff,
    /// 整树显/隐（root.IsVisible）。
    Show,
    Hide,
}

/// v5 球窗句柄（窗口线程持有；COM 全困在合成线程）。
pub struct BallWin {
    tx: Option<Sender<BallCmd>>,
}

impl BallWin {
    /// 起合成线程建树。`hwnd`=候选窗 HWND。立即返回（建树在线程内，
    /// 失败经 diag 日志报告+回落 v4）。
    pub fn new(hwnd: HWND) -> Option<Self> {
        let (tx, rx) = std::sync::mpsc::channel::<BallCmd>();
        // HWND 裸指针非 Send——RFC2229 精确捕获会绕过包装 struct，
        // 直接转 usize（值语义）跨线程。
        let h = hwnd.0 as usize;
        let builder = std::thread::Builder::new().name("hufu-ball5".into());
        let spawned = builder.spawn(move || {
            let hwnd2 = HWND(h as *mut _);
            let state = unsafe { synth_thread(hwnd2, &rx) };
            match state {
                Ok(()) => crate::tsf::diag_note("v5球: 合成线程退出"),
                Err(code) => crate::tsf::diag_note(&format!("v5球: 合成线程失败 {:08X}", code)),
            }
        });
        match spawned {
            Ok(_) => Some(BallWin { tx: Some(tx) }),
            Err(_) => None,
        }
    }

    fn send(&self, cmd: BallCmd) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(cmd);
        }
    }

    pub fn paint_ball(&self, ring: u32, core: u32) {
        self.send(BallCmd::PaintBall { ring, core });
    }
    pub fn move_to(&self, x: f32, y: f32) {
        self.send(BallCmd::MoveTo { x, y });
    }
    pub fn morph_open(&self, w: f32, h: f32, corner: f32) {
        self.send(BallCmd::MorphOpen { w, h, corner });
    }
    pub fn morph_close(&self) {
        self.send(BallCmd::MorphClose);
    }
    pub fn resize_surface(&self, w: f32, h: f32) {
        self.send(BallCmd::ResizeSurface { w, h });
    }
    pub fn fade_out(&self) {
        self.send(BallCmd::FadeOut);
    }
    pub fn fade_in(&self) {
        self.send(BallCmd::FadeIn);
    }
    pub fn breath_on(&self) {
        self.send(BallCmd::BreathOn);
    }
    pub fn breath_off(&self) {
        self.send(BallCmd::BreathOff);
    }
    pub fn show(&self) {
        self.send(BallCmd::Show);
    }
    pub fn hide(&self) {
        self.send(BallCmd::Hide);
    }
}

impl Drop for BallWin {
    fn drop(&mut self) {
        // 丢弃 Sender 即通道关闭——线程 recv Err 退出。
        self.tx = None;
    }
}

/// 合成线程本体：干净 apartment + DQ + Compositor + 整棵树 + 命令泵。
/// Err(hr)=建树失败码。
unsafe fn synth_thread(
    hwnd: HWND,
    rx: &std::sync::mpsc::Receiver<BallCmd>,
) -> Result<(), u32> {
    use windows::Win32::System::WinRT as wrt;
    // ① 公寓：STA（干净线程——RoInitialize 真正生效；宿主线程 S_FALSE
    //    的坑在这不存在）。
    let hr = wrt::RoInitialize(wrt::RO_INIT_SINGLETHREADED);
    match &hr {
        Ok(()) => crate::tsf::diag_note("v5球T: RoInit ok"),
        Err(e) => crate::tsf::diag_note(&format!("v5球T: RoInit {:08X}", e.code().0 as u32)),
    }
    if let Err(e) = hr {
        return Err(e.code().0 as u32);
    }
    // 线程 apartment 定性。
    let mut apt = windows::Win32::System::Com::APTTYPE(0);
    let mut qual = windows::Win32::System::Com::APTTYPEQUALIFIER(0);
    if windows::Win32::System::Com::CoGetApartmentType(&mut apt, &mut qual).is_ok() {
        crate::tsf::diag_note(&format!("v5球T: apt={} q={}", apt.0, qual.0));
    }
    // ② DispatcherQueue（桌面 WinRT 合成前置）。controller 必须保活
    //    到线程尾——部分宿主（powershell ASTA 进程）里 drop 即拆队列。
    let opts = wrt::DispatcherQueueOptions {
        dwSize: std::mem::size_of::<wrt::DispatcherQueueOptions>() as u32,
        threadType: wrt::DQTYPE_THREAD_CURRENT,
        apartmentType: wrt::DQTAT_COM_ASTA,
    };
    let _dqc = match wrt::CreateDispatcherQueueController(opts) {
        Ok(c) => {
            crate::tsf::diag_note("v5球T: DQ ok");
            c
        }
        Err(e) => {
            crate::tsf::diag_note(&format!("v5球T: DQ {:08X}", e.code().0 as u32));
            return Err(e.code().0 as u32);
        }
    };
    // ③ Compositor + HWND target。
    let compositor = match Compositor::new() {
        Ok(c) => c,
        Err(e) => {
            crate::tsf::diag_note(&format!(
                "v5球T: Compositor {:08X} msg={}",
                e.code().0 as u32,
                e.message()
            ));
            return Err(e.code().0 as u32);
        }
    };
    crate::tsf::diag_note("v5球T: Compositor ok");
    // 步进日志宏（定位建树失败点）。
    macro_rules! step {
        ($name:expr, $e:expr) => {
            match $e {
                Ok(v) => v,
                Err(e) => {
                    crate::tsf::diag_note(&format!(
                        "v5球T: {} 失败 {:08X}",
                        $name,
                        e.code().0 as u32
                    ));
                    return Err(e.code().0 as u32);
                }
            }
        };
    }
    let dinterop: ICompositorDesktopInterop =
        step!("DWT-QI", compositor.cast().map_err(|e| e.clone()));
    // isTopmost=true：树画窗口最顶。false 需要 redirection surface
    // 作画布（NOREDIRECTIONBITMAP 窗口没有——树无层可画，本机实证
    // 不渲染）。首轮 true 不显示是因为线程没泵 DQ 消息（见⑥注释）。
    let target: DesktopWindowTarget = step!(
        "CreateDWT",
        dinterop
            .CreateDesktopWindowTarget(hwnd, true)
            .map_err(|e| e.clone())
    )
    .cast()
    .map_err(|e| {
        crate::tsf::diag_note(&format!("v5球T: DWT cast {:08X}", e.code().0 as u32));
        e.code().0 as u32
    })?;
    // ④ D3D11（硬件→WARP 兜底，BGRA 供 D2D 互操作）→ D2D 设备
    //（合成线程自建——与宿主 D2D 设备无关，避免跨线程）。
    let mut d3d: Option<windows::Win32::Graphics::Direct3D11::ID3D11Device> = None;
    //（WARP 实证 EndDraw 失败——回硬件优先。）
    for dt in [
        windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE,
        windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_WARP,
    ] {
        let ok = windows::Win32::Graphics::Direct3D11::D3D11CreateDevice(
            None,
            dt,
            windows::Win32::Foundation::HMODULE::default(),
            windows::Win32::Graphics::Direct3D11::D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            windows::Win32::Graphics::Direct3D11::D3D11_SDK_VERSION,
            Some(&mut d3d),
            None,
            None,
        )
        .is_ok();
        if ok && d3d.is_some() {
            break;
        }
    }
    let d3d_dev = match d3d {
        Some(d) => d,
        None => {
            crate::tsf::diag_note("v5球T: D3D 全失败");
            return Err(0x887A0001);
        }
    };
    let dxgi_dev: windows::Win32::Graphics::Dxgi::IDXGIDevice =
        step!("DXGI-cast", d3d_dev.cast().map_err(|e| e.clone()));
    let factory: ID2D1Factory1 = step!(
        "D2D-Factory",
        D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).map_err(|e| e.clone())
    );
    let d2d_dev = step!("D2D-Device", factory.CreateDevice(&dxgi_dev).map_err(|e| e.clone()));
    let cinterop: ICompositorInterop =
        step!("InteropsQI", compositor.cast().map_err(|e| e.clone()));
    // ⑤【swapchain 路线】DrawingSurface 本机不出像素（exe/DLL 双实证
    // EndDraw ok 但不渲染——atlas/offset 玄学）；CreateSwapChainForComposition
    // 是官方合成专用形态，v4 的 swapchain 渲染链（D2D on buffer）成熟。
    let dxgi_factory: IDXGIFactory2 =
        step!("DXGIF2", windows::Win32::Graphics::Dxgi::CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).map_err(|e| e.clone()));
    let sd = DXGI_SWAP_CHAIN_DESC1 {
        // 大画布常驻（球/框/插值帧都画左上区——ResizeBuffers 在
        // FLIP+D2D 组合上后继绘制假成功，本机读回实证全透明）。
        Width: 256,
        Height: 256,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        Stereo: windows::Win32::Foundation::BOOL(0),
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        Scaling: DXGI_SCALING_STRETCH,
        AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
        Flags: 0,
    };
    let chain = step!(
        "ChainComp",
        dxgi_factory.CreateSwapChainForComposition(&d3d_dev, &sd, None).map_err(|e| e.clone())
    );
    let comp_surface = step!(
        "CompSurf",
        cinterop.CreateCompositionSurfaceForSwapChain(&chain).map_err(|e| e.clone())
    );
    let brush = step!("SBrush", compositor.CreateSurfaceBrush().map_err(|e| e.clone()));
    step!("SetSurf", brush.SetSurface(&comp_surface).map_err(|e| e.clone()));
    // Stretch=None：画布像素 1:1（默认适配缩放——morph 后尺寸错配时
    // 内容被缩成小点，本机实证）。
    let _ = brush.SetStretch(windows::UI::Composition::CompositionStretch::None);
    // 【真凶修复】AlignmentRatio 默认 Center——大画布在小于画布的
    // content 里居中=显示画布中央空白（球 (11..33) 在画布左上，永远
    // 显示不到；morph 框只露右下裁剪残条 95x20，全实证）。TopLeft 对
    // 齐=画布左上角↔content 左上角，球/框都在位。
    let _ = brush.SetHorizontalAlignmentRatio(0.0);
    let _ = brush.SetVerticalAlignmentRatio(0.0);
    // gdev 保底字段（不再用于内容——DrawingSurface 路线已弃）。
    let gdev = step!(
        "GraphicsDevice",
        cinterop.CreateGraphicsDevice(&d2d_dev).map_err(|e| e.clone())
    );
    let _ = gdev;
    let root = step!("CtnVisual", compositor.CreateContainerVisual().map_err(|e| e.clone()));
    step!("Root-Sz", root
        .SetSize(N::Vector2 { X: 400.0, Y: 300.0 })
        .map_err(|e| e.clone()));
    // 【v5.1 架构·定稿】CompositionGeometricClip 吞整树（本机实证）；
    // windows-0.58 缺 GradientStops/Shapes API（ShapeVisual 路线不可）。
    // 全部内容画进 swapchain（已证渲染）：球帧=渐变球；框帧=面板；
    // morph=CPU 画插值帧（形状 lerp）。阴影=DropShadow 挂 content 跟
    // alpha 轮廓（球圆/框角矩形自动随形变）。动画：Offset 跟随/
    // Opacity 渐隐/Scale 呼吸全合成器；morph 帧节奏=本线程空闲态。
    let content = step!("SprVisual", compositor.CreateSpriteVisual().map_err(|e| e.clone()));
    step!("SetBrush", content.SetBrush(&brush).map_err(|e| e.clone()));
    step!("Ctr-Sz", content
        .SetSize(N::Vector2 {
            X: BALL_D + 2.0 * BALL_PAD,
            Y: BALL_D + 2.0 * BALL_PAD,
        })
        .map_err(|e| e.clone()));
    step!("Ctr-Off", content
        .SetOffset(N::Vector3 { X: 0.0, Y: 0.0, Z: 0.0 })
        .map_err(|e| e.clone()));
    // 跟随平滑：Offset 键帧（光标移动时合成器插值——「一整个元素
    // 顺滑平移」，球/框态都生效；0.58 缺 ImplicitAnimationCollection
    // 的 InsertAnimation——MoveTo 命令手动起 KeyFrame 等效）。
    fn move_to(compositor: &Compositor, root: &ContainerVisual, x: f32, y: f32) {
        if let Ok(a) = compositor.CreateVector3KeyFrameAnimation() {
            let from = root.Offset().unwrap_or(N::Vector3 { X: 0.0, Y: 0.0, Z: 0.0 });
            let _ = a.InsertKeyFrame(0.0, from);
            let _ = a.InsertKeyFrame(1.0, N::Vector3 { X: x, Y: y, Z: 0.0 });
            let _ = a.SetDuration(dur(120));
            let _ = root.StartAnimation(&HSTRING::from("Offset"), &a);
        }
    }
    let shadow = step!("Shadow", compositor.CreateDropShadow().map_err(|e| e.clone()));
    // 【阴影画帧内】DropShadow 挂 content 吞内容（本机 DWT 实证）——
    // 阴影改为 D2D 画进 swapchain 帧（球下模糊圆=v4 同款，morph 同帧
    // 插值）。DropShadow 对象保留（不挂）。
    let _ = &shadow;
    step!("Insert-ctr", root
        .Children()
        .map_err(|e| e.clone())
        .and_then(|ch| ch.InsertAtTop(&content)));
    step!("T-Root", target.SetRoot(&root).map_err(|e| e.clone()));
    // 【红块探针已删】通道验证完成（树渲染/树级 hide 全链已证）；
    // 红块曾长期遮挡球位（InsertAtTop 在 content 之上）误导排障。
    crate::tsf::diag_note("v5球: 合成树建立 ok");
    // ⑥ 命令泵：recv 超时+线程消息泵混合——**WinRT 合成的提交调度
    //    走创建线程的消息队列**（DispatcherQueue 需要泵；纯阻塞 recv
    //    的树挂上不 flush，本机实证不渲染）。15ms 只在空闲时醒（动画
    //    期间合成器线程独立跑，不占 IME 线程）。
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, PeekMessageW, MSG, PM_REMOVE,
    };
    let mut panel_w = BALL_D;
    let mut panel_h = BALL_D;
    let mut corner = 12.0f32;
    let mut idle_ticks: u32 = 0;
    let mut last_ring: u32 = 0xFF101014;
    let mut last_core: u32 = 0xFFE8E8EC;
    loop {
        let got = rx.recv_timeout(std::time::Duration::from_millis(15)).ok();
        if let Some(cmd) = got {
            // 键入=活跃：复位 idle 计数（渐隐中再键入→FadeIn 恢复）。
            if idle_ticks as u32 >= BALL_IDLE_MS / 15 {
                root.StopAnimation(&HSTRING::from("Opacity"));
                let _ = root.SetOpacity(1.0);
                let _ = root.SetIsVisible(true);
            }
            idle_ticks = 0;
            match cmd {
            BallCmd::PaintBall { ring, core } => {
                last_ring = ring;
                last_core = core;
                let ok = paint_ball_into(&chain, &d2d_dev, ring, core);
                crate::tsf::diag_note(&format!("v5球T: paint={} ring={:08X}", ok, ring));
            }
            BallCmd::MoveTo { x, y } => {
                move_to(&compositor, &root, x, y);
            }
            BallCmd::MorphOpen { w, h, corner: c2 } => {
                panel_w = w;
                panel_h = h;
                corner = c2;
                // 形变走 CPU 插值帧（morph 帧）；视觉尺寸/阴影同步动画。
                animate_geo(
                    &compositor, &content, &shadow,
                    N::Vector2 { X: w, Y: h },
                    24.0, MORPH_OPEN_MS,
                );
                // 展开帧：面板尺寸的 ring 色圆角矩形（渐变延续球感；
                // 面板文字层接入前=球色块展开）。
                let _ = paint_frame_into(&chain, &d2d_dev, w, h, c2, last_ring);
            }
            BallCmd::MorphClose => {
                corner = BALL_D * 0.5;
                // 框→球：慢收（260ms，可被下一键入的 MorphOpen 打断
                // 反转——animate_geo 的 from=current 语义天然可打断）。
                animate_geo(
                    &compositor, &content, &shadow,
                    N::Vector2 { X: BALL_D + 2.0 * BALL_PAD, Y: BALL_D + 2.0 * BALL_PAD },
                    10.0, MORPH_CLOSE_MS,
                );
                let ok = paint_ball_into(&chain, &d2d_dev, last_ring, last_core);
                crate::tsf::diag_note(&format!("v5球T: close paint={}", ok));
            }
            BallCmd::ResizeSurface { w, h } => {
                // swapchain ResizeBuffers（0=保留配置）+视觉尺寸。
                let _ = chain.ResizeBuffers(
                    0,
                    w as u32,
                    h as u32,
                    DXGI_FORMAT_B8G8R8A8_UNORM,
                    DXGI_SWAP_CHAIN_FLAG(0),
                );
                let _ = content.SetSize(N::Vector2 { X: w, Y: h });
                panel_w = w;
                panel_h = h;
            }
            BallCmd::FadeOut => {
                if let Ok(a) = compositor.CreateScalarKeyFrameAnimation() {
                    let _ = a.InsertKeyFrame(0.0, 1.0);
                    let _ = a.InsertKeyFrame(1.0, 0.0);
                    let _ = a.SetDuration(dur(BALL_FADE_MS));
                    let _ = root.StartAnimation(&HSTRING::from("Opacity"), &a);
                }
            }
            BallCmd::FadeIn => {
                root.StopAnimation(&HSTRING::from("Opacity"));
                let _ = root.SetOpacity(1.0);
                // 【移动后刷新】窗口 SWP 移动+缩尺后树疑似不重提交
                //（本机实证 morph_close 后不渲染）——IsVisible 翻转强制
                // 合成器重评估整棵树。
                let _ = root.SetIsVisible(false);
                let _ = root.SetIsVisible(true);
            }
            BallCmd::BreathOn => {
                if let Ok(a) = compositor.CreateExpressionAnimation() {
                    let _ = a.SetExpression(&HSTRING::from(
                        "Vector3(1+0.035*sin(this.currentTime/480), 1+0.035*sin(this.currentTime/480), 1)",
                    ));
                    let _ = content.StartAnimation(&HSTRING::from("Scale"), &a);
                }
                let _ = content.SetCenterPoint(N::Vector3 {
                    X: BALL_D * 0.5,
                    Y: BALL_D * 0.5,
                    Z: 0.0,
                });
            }
            BallCmd::BreathOff => {
                content.StopAnimation(&HSTRING::from("Scale"));
                let _ = content.SetScale(N::Vector3 {
                    X: 1.0,
                    Y: 1.0,
                    Z: 1.0,
                });
            }
            BallCmd::Show => {
                let _ = root.SetIsVisible(true);
            }
            BallCmd::Hide => {
                let _ = root.SetIsVisible(false);
            }
            }
        } else {
            // 空闲计数（15ms/tick）：BALL_IDLE_MS 无命令→球渐隐→隐藏。
            // 任何命令在别处已重置 idle_ticks（Show/键入路径）。
            idle_ticks += 1;
            if idle_ticks as u32 == BALL_IDLE_MS / 15 {
                if let Ok(a) = compositor.CreateScalarKeyFrameAnimation() {
                    let _ = a.InsertKeyFrame(0.0, 1.0);
                    let _ = a.InsertKeyFrame(1.0, 0.0);
                    let _ = a.SetDuration(dur(BALL_FADE_MS));
                    let _ = root.StartAnimation(&HSTRING::from("Opacity"), &a);
                }
                crate::tsf::diag_note("v5球T: idle→渐隐");
            } else if idle_ticks as u32 > (BALL_IDLE_MS + (BALL_FADE_MS as u32) * 4) / 15 {
                let _ = root.SetIsVisible(false);
                crate::tsf::diag_note("v5球T: idle→隐藏");
            }
        }
        // 泵线程消息（DQ 提交调度——WinRT 合成的 flush 必需）。
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = DispatchMessageW(&msg);
        }
    }
    let _ = target.SetRoot(None);
    wrt::RoUninitialize();
    Ok(())
}

/// 视觉 Size morph（形变动画：content Size+阴影模糊同步 KeyFrame；
/// KeyFrame 起点取当前值——中断续动语义。形状内容=morph 插值帧由
/// 命令泵空闲态补齐）。
fn animate_geo(
    compositor: &Compositor,
    content: &SpriteVisual,
    shadow: &DropShadow,
    to_size: N::Vector2,
    to_blur: f32,
    ms: i32,
) {
    let d = dur(ms);
    if let Ok(a) = compositor.CreateVector2KeyFrameAnimation() {
        let from = content.Size().unwrap_or(N::Vector2 { X: BALL_D, Y: BALL_D });
        let _ = a.InsertKeyFrame(0.0, from);
        let _ = a.InsertKeyFrame(1.0, to_size);
        let _ = a.SetDuration(d);
        let _ = content.StartAnimation(&HSTRING::from("Size"), &a);
    }
    if let Ok(a) = compositor.CreateScalarKeyFrameAnimation() {
        let from = shadow.BlurRadius().unwrap_or(10.0);
        let _ = a.InsertKeyFrame(0.0, from);
        let _ = a.InsertKeyFrame(1.0, to_blur);
        let _ = a.SetDuration(d);
        let _ = shadow.StartAnimation(&HSTRING::from("BlurRadius"), &a);
    }
}

/// 画球帧（swapchain 路线：D2D 直接画 backbuffer→Present；渐变立体球
/// +高光，alpha 轮廓即球——阴影自动跟随）。
unsafe fn paint_ball_into(
    chain: &windows::Win32::Graphics::Dxgi::IDXGISwapChain1,
    d2d_dev: &ID2D1Device,
    ring: u32,
    core: u32,
) -> bool {

    let buffer: windows::Win32::Graphics::Direct3D11::ID3D11Texture2D =
        match chain.GetBuffer(0) {
            Ok(b) => b,
            Err(_) => return false,
        };
    // backbuffer → D2D bitmap（TARGET 选项）→ ctx 渲染到它。
    let props = D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: 96.0,
        dpiY: 96.0,
        bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
        colorContext: std::mem::ManuallyDrop::new(None),
    };
    let ctx: ID2D1DeviceContext = match d2d_dev.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let dxgi_surf: IDXGISurface = match buffer.cast() {
        Ok(s) => s,
        Err(_) => return false,
    };
    let bmp: ID2D1Bitmap1 = match ctx.CreateBitmapFromDxgiSurface(&dxgi_surf, Some(&props)) {
        Ok(b) => b,
        Err(_) => return false,
    };
    let target_img: ID2D1Image = match bmp.cast() {
        Ok(i) => i,
        Err(_) => return false,
    };
    ctx.SetTarget(Some(&target_img));
    let _ = ctx.BeginDraw();
    ctx.SetTransform(&N::Matrix3x2::identity());
    ctx.Clear(None);
    // 阴影（帧内）：球下偏移 1.5px 的黑径向渐变（r 放大 1.25）——
    // 「浮」感；morph 时阴影与形状同帧插值（面板帧同画法）。
    let bc = BALL_PAD + BALL_D * 0.5;
    let sh_prop = D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
        center: D2D_POINT_2F { x: bc + 1.5, y: bc + 2.0 },
        gradientOriginOffset: D2D_POINT_2F { x: 0.0, y: 0.0 },
        radiusX: BALL_D * 0.5 * 1.25,
        radiusY: BALL_D * 0.5 * 1.25,
    };
    let sh_stops = [
        D2D1_GRADIENT_STOP { position: 0.0, color: D2D1_COLOR_F { a: 0.35, r: 0.0, g: 0.0, b: 0.0 } },
        D2D1_GRADIENT_STOP { position: 0.7, color: D2D1_COLOR_F { a: 0.18, r: 0.0, g: 0.0, b: 0.0 } },
        D2D1_GRADIENT_STOP { position: 1.0, color: D2D1_COLOR_F { a: 0.0, r: 0.0, g: 0.0, b: 0.0 } },
    ];
    let rt: ID2D1RenderTarget = ctx.cast().unwrap();
    if let Ok(gsc) = rt.CreateGradientStopCollection(&sh_stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP) {
        if let Ok(shb) = ctx.CreateRadialGradientBrush(&sh_prop, None, &gsc) {
            ctx.FillEllipse(
                &D2D1_ELLIPSE {
                    point: D2D_POINT_2F { x: bc + 1.5, y: bc + 2.0 },
                    radiusX: BALL_D * 0.5 * 1.25,
                    radiusY: BALL_D * 0.5 * 1.25,
                },
                &shb,
            );
        }
    }
    // 渐变球：左上高光=立体感；皮肤主色由 ring/core 参数传入（皮肤无关）。
    fill_grad_ball(&ctx, bc, bc, BALL_D * 0.5, ring);
    fill_grad_ball(&ctx, bc, bc, 5.0, core);
    let hr = ctx.EndDraw(None, None);
    if hr.is_err() {
        return false;
    }
    // Present(0)=异步提交（Present(1) 在合成 swapchain 上会等 DWM
    // 消费——泵线程阻塞→树 flush 停→全树黑，本机实证；打字节奏的
    // 内容更新不需要撕裂同步）。
    chain.Present(0, DXGI_PRESENT_RESTART).is_ok()
}

/// 画框帧（morph 展开/面板态）：ring 色径向渐变圆角矩形+底部阴影
///（球色延续=形色同源；文字层接入前为纯色块）。画布 256 常驻——框画
/// 在 (0..w,0..h) 左上区，零 ResizeBuffers。
unsafe fn paint_frame_into(
    chain: &windows::Win32::Graphics::Dxgi::IDXGISwapChain1,
    d2d_dev: &ID2D1Device,
    w: f32,
    h: f32,
    corner: f32,
    ring: u32,
) -> bool {
    let buffer: windows::Win32::Graphics::Direct3D11::ID3D11Texture2D =
        match chain.GetBuffer(0) {
            Ok(b) => b,
            Err(_) => return false,
        };
    let props = D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: 96.0,
        dpiY: 96.0,
        bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
        colorContext: std::mem::ManuallyDrop::new(None),
    };
    let ctx: ID2D1DeviceContext = match d2d_dev.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let dxgi_surf: IDXGISurface = match buffer.cast() {
        Ok(s) => s,
        Err(_) => return false,
    };
    let bmp: ID2D1Bitmap1 = match ctx.CreateBitmapFromDxgiSurface(&dxgi_surf, Some(&props)) {
        Ok(b) => b,
        Err(_) => return false,
    };
    let target_img: ID2D1Image = match bmp.cast() {
        Ok(i) => i,
        Err(_) => return false,
    };
    ctx.SetTarget(Some(&target_img));
    let _ = ctx.BeginDraw();
    ctx.SetTransform(&N::Matrix3x2::identity());
    ctx.Clear(None);
    // 底部软阴影（帧内）。
    let sh_prop = D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
        center: D2D_POINT_2F { x: w * 0.5 + 1.5, y: h * 0.5 + 3.0 },
        gradientOriginOffset: D2D_POINT_2F { x: 0.0, y: 0.0 },
        radiusX: w * 0.75,
        radiusY: h * 0.85,
    };
    let sh_stops = [
        D2D1_GRADIENT_STOP { position: 0.0, color: D2D1_COLOR_F { a: 0.35, r: 0.0, g: 0.0, b: 0.0 } },
        D2D1_GRADIENT_STOP { position: 0.7, color: D2D1_COLOR_F { a: 0.16, r: 0.0, g: 0.0, b: 0.0 } },
        D2D1_GRADIENT_STOP { position: 1.0, color: D2D1_COLOR_F { a: 0.0, r: 0.0, g: 0.0, b: 0.0 } },
    ];
    let rt: ID2D1RenderTarget = ctx.cast().unwrap();
    if let Ok(gsc) = rt.CreateGradientStopCollection(&sh_stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP) {
        if let Ok(shb) = ctx.CreateRadialGradientBrush(&sh_prop, None, &gsc) {
            ctx.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F { left: 0.0, top: 0.0, right: w, bottom: h },
                    radiusX: corner,
                    radiusY: corner,
                },
                &shb,
            );
        }
    }
    // ring 色径向渐变圆角矩形（左上高光延续球感）。
    let fill_prop = D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
        center: D2D_POINT_2F { x: w * 0.5, y: h * 0.5 },
        gradientOriginOffset: D2D_POINT_2F { x: -w * 0.18, y: -h * 0.2 },
        radiusX: w * 0.9,
        radiusY: h * 1.0,
    };
    let stops = [
        D2D1_GRADIENT_STOP { position: 0.0, color: argb_to_color(lighten(ring, 0.3)) },
        D2D1_GRADIENT_STOP { position: 1.0, color: argb_to_color(darken(ring, 0.12)) },
    ];
    if let Ok(gsc) = rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP) {
        if let Ok(br) = ctx.CreateRadialGradientBrush(&fill_prop, None, &gsc) {
            ctx.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F { left: 0.0, top: 0.0, right: w, bottom: h },
                    radiusX: corner,
                    radiusY: corner,
                },
                &br,
            );
        }
    }
    let hr = ctx.EndDraw(None, None);
    if hr.is_err() {
        return false;
    }
    chain.Present(0, DXGI_PRESENT_RESTART).is_ok()
}

/// 读回探针：backbuffer→Staging→Map，打几个采样像素（排障用）。
/// 【FLIP 双缓冲定位】读 buffer 0 和 1——看球帧落在哪块。
unsafe fn readback_probe(
    chain: &windows::Win32::Graphics::Dxgi::IDXGISwapChain1,
    dev: &windows::Win32::Graphics::Direct3D11::ID3D11Device,
) {
    for bi in 0..2u32 {
        let buf: windows::Win32::Graphics::Direct3D11::ID3D11Texture2D =
            match chain.GetBuffer(bi) {
                Ok(b) => b,
                Err(_) => continue,
            };
        let _ = sample_buf(&buf, dev, bi);
    }
}

unsafe fn sample_buf(
    buf: &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
    dev: &windows::Win32::Graphics::Direct3D11::ID3D11Device,
    idx: u32,
) -> bool {
    use windows::Win32::Graphics::Direct3D11::*;
    use windows::Win32::Graphics::Direct3D11 as Direct3D11;
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    buf.GetDesc(&mut desc);
    let ctx = match dev.GetImmediateContext() {
        Ok(c) => c,
        Err(_) => return false,
    };
    let sd = D3D11_TEXTURE2D_DESC {
        Width: desc.Width,
        Height: desc.Height,
        MipLevels: 1,
        ArraySize: 1,
        Format: desc.Format,
        SampleDesc: desc.SampleDesc,
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut out_tex: Option<Direct3D11::ID3D11Texture2D> = None;
    if unsafe {
        dev.CreateTexture2D(&sd, None, Some(&mut out_tex as *mut _))
    }
    .is_err()
    {
        return false;
    }
    let tex = match out_tex {
        Some(t) => t,
        None => return false,
    };
    let res_tex: Direct3D11::ID3D11Resource = match tex.cast() {
        Ok(r) => r,
        Err(_) => return false,
    };
    let res_buf: Direct3D11::ID3D11Resource = match buf.cast() {
        Ok(r) => r,
        Err(_) => return false,
    };
    unsafe { ctx.CopyResource(&res_tex, &res_buf) };
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    if unsafe { ctx.Map(&res_tex, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }.is_err() {
        return false;
    }
    let w = desc.Width as usize;
    let _ = w;
    let sample = |x: usize, y: usize| -> u32 {
        let p = mapped.pData as *const u8;
        let off = y * (mapped.RowPitch as usize) + x * 4;
        let b = unsafe { *p.add(off) };
        let g = unsafe { *p.add(off + 1) };
        let r = unsafe { *p.add(off + 2) };
        let a = unsafe { *p.add(off + 3) };
        (a as u32) << 24 | (r as u32) << 16 | (g as u32) << 8 | b as u32
    };
    // 球心（44 球在 256 画布的 (11..33)，中心=(22,22)）。
    let c1 = sample(22, 22);
    let c2 = sample(22, 60);
    let c3 = sample(75, 50);
    unsafe { ctx.Unmap(&res_tex, 0) };
    crate::tsf::diag_note(&format!(
        "v5球T: 读回[buf{}] {}x{} 球心={:08X} (22,60)={:08X} (75,50)={:08X}",
        idx, desc.Width, desc.Height, c1, c2, c3
    ));
    true
}

/// 径向渐变小球（左上高光偏移→立体感）。
unsafe fn fill_grad_ball(ctx: &ID2D1DeviceContext, cx: f32, cy: f32, r: f32, argb: u32) {
    let prop = D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
        center: D2D_POINT_2F { x: cx, y: cy },
        gradientOriginOffset: D2D_POINT_2F {
            x: -r * 0.3,
            y: -r * 0.35,
        },
        radiusX: r * 1.15,
        radiusY: r * 1.15,
    };
    let stops = [
        D2D1_GRADIENT_STOP {
            position: 0.0,
            color: argb_to_color(lighten(argb, 0.3)),
        },
        D2D1_GRADIENT_STOP {
            position: 1.0,
            color: argb_to_color(darken(argb, 0.15)),
        },
    ];
    // DeviceContext 上是 6 参版（colorspace 系）——RenderTarget 基接口
    // 的 3 参版（gamma 系）才是常规用法：cast 后调用。
    let rt: ID2D1RenderTarget = match ctx.cast() {
        Ok(r) => r,
        Err(_) => return,
    };
    if let Ok(gsc) = rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP) {
        if let Ok(br) = ctx.CreateRadialGradientBrush(&prop, None, &gsc) {
            ctx.FillEllipse(
                &D2D1_ELLIPSE {
                    point: D2D_POINT_2F { x: cx, y: cy },
                    radiusX: r,
                    radiusY: r,
                },
                &br,
            );
        }
    }
}

/// 0xAARRGGBB → D2D 色（straight alpha）。
fn argb_to_color(argb: u32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        a: ((argb >> 24) & 0xFF) as f32 / 255.0,
        r: ((argb >> 16) & 0xFF) as f32 / 255.0,
        g: ((argb >> 8) & 0xFF) as f32 / 255.0,
        b: (argb & 0xFF) as f32 / 255.0,
    }
}

/// 向白提亮 k（0..1）。
fn lighten(argb: u32, k: f32) -> u32 {
    let ch = |shift: u32| -> u32 {
        let v = ((argb >> shift) & 0xFF) as f32;
        let v = v + (255.0 - v) * k;
        v.clamp(0.0, 255.0) as u32
    };
    (argb & 0xFF000000) | (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

/// 向黑压暗 k。
fn darken(argb: u32, k: f32) -> u32 {
    let ch = |shift: u32| -> u32 {
        let v = ((argb >> shift) & 0xFF) as f32 * (1.0 - k);
        v.clamp(0.0, 255.0) as u32
    };
    (argb & 0xFF000000) | (ch(16) << 16) | (ch(8) << 8) | ch(0)
}
