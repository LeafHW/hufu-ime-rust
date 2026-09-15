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
/// 球→框。【四修补】用户「速度慢一倍」=放慢：150→300ms（初解反了
/// 提成 80ms——本条回摆）。
pub const MORPH_OPEN_MS: i32 = 300;
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
/// 【morph v3·屏位锚】形变=CPU 插值帧（15ms tick）；几何与内容帧同帧
/// 应用。**位置全部屏幕坐标语义**：from 恒=当前视觉位置（合成线程自
/// 跟踪 cur+窗口原点换算）——打断（连打反转）/重复命令（poll 重复收
/// 球）天然无位置跳变（v2 的 from=命令指定=跳变抖动源，用户实证）。
pub enum BallCmd {
    /// 画球帧（环色/心色；热更色用——纯重画不动几何）。
    PaintBall { ring: u32, core: u32 },
    /// 球→框（快，可打断）。w/h=框尺寸；tx/ty=框屏位（左上）。
    MorphOpen { w: f32, h: f32, tx: f32, ty: f32 },
    /// 框→球（慢，可被打断反转）。tx/ty=球屏位（元素框左上）。
    MorphClose { tx: f32, ty: f32 },
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
    pub fn morph_open(&self, w: f32, h: f32, tx: f32, ty: f32) {
        self.send(BallCmd::MorphOpen { w, h, tx, ty });
    }
    pub fn morph_close(&self, tx: f32, ty: f32) {
        self.send(BallCmd::MorphClose { tx, ty });
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
    //    的树挂上不 flush，本机实证不渲染）。15ms tick 兼任 morph 帧
    //    驱动（≈66fps 插值帧）。
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, PeekMessageW, MSG, PM_REMOVE,
    };
    // 【morph v3 状态机·屏位锚】cur=当前几何（尺寸+**屏幕**位置，含
    // 中间态）；morph=进行中的插值。命令只给「目标」——from 恒=cur
    //（当前视觉位）——打断/重复命令无跳变（抖动根治）。窗口原点
    // win_org 缓存：命令到达时刷新（SWP 已由窗口线程先做）；morph
    // 期间窗口恒定不动（包围盒设计）。
    struct Geo {
        w: f32,
        h: f32,
        sx: f32,
        sy: f32,
    }
    let ball_side = BALL_D + 2.0 * BALL_PAD;
    let mut cur = Geo { w: ball_side, h: ball_side, sx: 0.0, sy: 0.0 };
    let mut cur_valid = false; // 首命令前 cur 无效——from=to（原地起步）
    let mut win_org: (f32, f32) = (0.0, 0.0);
    let mut morph: Option<(Geo, Geo, std::time::Instant, u32, bool)> = None; // (from,to,t0,ms,closing)
    let mut ballness = 1.0f32; // 球接近度（1=纯球：core 满显+inset=pad；0=纯框）
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
                // 【六修】仅球态重画：框态（morph 完成后 morph=None、
                // cur=框尺寸）paint_ball 会把球帧盖进框 content——连打
                // 时 show 每帧覆盖=框里只剩小球（「形变不是球变框」+
                // 交替闪的根源，用户实证）。色缓存照常（morph 帧自取）。
                if morph.is_none()
                    && cur.w <= ball_side + 0.5
                    && cur.h <= ball_side + 0.5
                {
                    let ok = paint_ball_into(&chain, &d2d_dev, ring, core);
                    crate::tsf::diag_note(&format!("v5球T: paint={} ring={:08X}", ok, ring));
                }
            }
            BallCmd::MorphOpen { w, h, tx, ty } => {
                // 屏位锚：from=cur（当前视觉位——打断续动/无跳变）；
                // 首命令 cur 无效→from=to（原地起步——球在框位冒出
                // 长大成框，不从屏幕外飞入）。
                refresh_win_org(hwnd, &mut win_org);
                let to = Geo { w, h, sx: tx, sy: ty };
                let from = if cur_valid { Geo { w: cur.w, h: cur.h, sx: cur.sx, sy: cur.sy } } else { Geo { w: ball_side, h: ball_side, sx: tx, sy: ty } };
                cur_valid = true;
                morph = Some((
                    from, to,
                    std::time::Instant::now(),
                    MORPH_OPEN_MS as u32,
                    false,
                ));
                // morph 中停呼吸（Scale 干扰逐帧 SetSize）。
                content.StopAnimation(&HSTRING::from("Scale"));
                let _ = content.SetScale(N::Vector3 { X: 1.0, Y: 1.0, Z: 1.0 });
            }
            BallCmd::MorphClose { tx, ty } => {
                refresh_win_org(hwnd, &mut win_org);
                let to = Geo { w: ball_side, h: ball_side, sx: tx, sy: ty };
                let from = if cur_valid { Geo { w: cur.w, h: cur.h, sx: cur.sx, sy: cur.sy } } else { Geo { w: ball_side, h: ball_side, sx: tx, sy: ty } };
                cur_valid = true;
                morph = Some((
                    from, to,
                    std::time::Instant::now(),
                    MORPH_CLOSE_MS as u32,
                    true,
                ));
                content.StopAnimation(&HSTRING::from("Scale"));
                let _ = content.SetScale(N::Vector3 { X: 1.0, Y: 1.0, Z: 1.0 });
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
                // 【防闪·五修】翻转仅在树不可见时做（观察窗内框本就
                // 可见，show 每键盲翻 false→true=一帧闪，用户实证
                // 「打下一个编码会闪一下」的嫌疑主源）。
                let vis = root.IsVisible().unwrap_or(true);
                if !vis {
                    let _ = root.SetIsVisible(false);
                    let _ = root.SetIsVisible(true);
                }
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
            // 【morph v2 插值帧驱动】15ms/tick：progress→ease→几何+内容
            // 同帧应用（SetSize/SetOffset+画插值帧+Present——Stretch.None
            // 下合成器动画只是揭幕裁剪，形变必须内容帧逐帧画）。
            if let Some((from, to, t0, ms, closing)) = &morph {
                let t = (t0.elapsed().as_millis() as f32 / *ms as f32).min(1.0);
                let e = if *closing { ease_in_out(t) } else { ease_out(t) };
                cur.w = lerp(from.w, to.w, e);
                cur.h = lerp(from.h, to.h, e);
                cur.sx = lerp(from.sx, to.sx, e);
                cur.sy = lerp(from.sy, to.sy, e);
                ballness = if *closing { e } else { 1.0 - e };
                let _ = content.SetSize(N::Vector2 { X: cur.w.max(2.0), Y: cur.h.max(2.0) });
                // 屏位→窗口 Offset（morph 期间窗口恒定——win_org 命令
                // 时已刷新）。
                let _ = content.SetOffset(N::Vector3 {
                    X: cur.sx - win_org.0,
                    Y: cur.sy - win_org.1,
                    Z: 0.0,
                });
                let _ = paint_morph_frame(
                    &chain, &d2d_dev, cur.w, cur.h, ballness,
                    last_ring, last_core,
                );
                // 【树重提交】本机实证：morph 逐帧 SetSize/SetOffset 后
                // 合成器不自动提交新 surface 帧——IsVisible 翻转强制整
                // 棵树重评估（open 路径靠 show 每键 fade_in 翻转显示，
                // close 靠收球时一次；中间帧靠这里的逐帧翻转）。
                let _ = root.SetIsVisible(false);
                let _ = root.SetIsVisible(true);
                if t >= 1.0 {
                    let closing = *closing;
                    morph = None;
                    let _ = closing;
                    crate::tsf::diag_note("v5球T: 收球完成");
                    // 【取证】收球终态帧 readback（swapchain 里有没有球）。
                    readback_probe(&chain, &d3d_dev);
                }
            } else {
                // 空闲计数（15ms/tick）：BALL_IDLE_MS 无命令→球渐隐→隐藏。
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
                }
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

/// 插值数学：线性/easeOut（快出）/easeInOut（慢收用——收球要缓）。
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t) * (1.0 - t)
}
fn ease_in_out(t: f32) -> f32 {
    if t < 0.5 {
        2.0 * t * t
    } else {
        let x = -2.0 * t + 2.0;
        1.0 - x * x / 2.0
    }
}

/// 窗口原点刷新（命令到达时调——SWP 已由窗口线程先做）。
unsafe fn refresh_win_org(hwnd: HWND, org: &mut (f32, f32)) {
    let mut rr = windows::Win32::Foundation::RECT::default();
    if windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut rr).is_ok() {
        *org = (rr.left as f32, rr.top as f32);
    }
}

/// 【morph v2 插值帧】画「ballness 球接近度」的中间形态进 swapchain。
/// 形状族=内缩圆角矩形：球端 inset=BALL_PAD（主体=22px，corner=11=
/// 正圆）；框端 inset=0（主体=全幅 w×h，corner=12）。渐变/阴影/core
/// 随 ballness 连续过渡——一整个元素无级形变的视觉本体。
unsafe fn paint_morph_frame(
    chain: &windows::Win32::Graphics::Dxgi::IDXGISwapChain1,
    d2d_dev: &ID2D1Device,
    w: f32,
    h: f32,
    ballness: f32,
    ring: u32,
    core: u32,
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
    let ctx: ID2D1DeviceContext =
        match d2d_dev.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE) {
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
    // 主体矩形：inset 随 ballness（球端=pad 内缩=22px 圆，框端=全幅）。
    let inset = BALL_PAD * ballness;
    let bx0 = inset;
    let by0 = inset;
    let bx1 = (w - inset).max(bx0 + 2.0);
    let by1 = (h - inset).max(by0 + 2.0);
    let bw = bx1 - bx0;
    let bh = by1 - by0;
    let bcx = bx0 + bw * 0.5;
    let bcy = by0 + bh * 0.5;
    let corner = lerp(12.0, BALL_D * 0.5, ballness);
    let main_rect = D2D1_ROUNDED_RECT {
        rect: D2D_RECT_F { left: bx0, top: by0, right: bx1, bottom: by1 },
        radiusX: corner,
        radiusY: corner,
    };
    let rt: ID2D1RenderTarget = ctx.cast().unwrap();
    // 底部软阴影（帧内径向，随主体缩放）。
    let sh_prop = D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
        center: D2D_POINT_2F { x: bcx + 1.5, y: bcy + 3.0 },
        gradientOriginOffset: D2D_POINT_2F { x: 0.0, y: 0.0 },
        radiusX: bw * 0.75,
        radiusY: bh * 0.85,
    };
    let sh_stops = [
        D2D1_GRADIENT_STOP { position: 0.0, color: D2D1_COLOR_F { a: 0.35, r: 0.0, g: 0.0, b: 0.0 } },
        D2D1_GRADIENT_STOP { position: 0.7, color: D2D1_COLOR_F { a: 0.16, r: 0.0, g: 0.0, b: 0.0 } },
        D2D1_GRADIENT_STOP { position: 1.0, color: D2D1_COLOR_F { a: 0.0, r: 0.0, g: 0.0, b: 0.0 } },
    ];
    if let Ok(gsc) = rt.CreateGradientStopCollection(&sh_stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP) {
        if let Ok(shb) = ctx.CreateRadialGradientBrush(&sh_prop, None, &gsc) {
            ctx.FillRoundedRectangle(&main_rect, &shb);
        }
    }
    // 主体：ring 色径向渐变（高光偏左上——球感延续到框）。
    let fill_prop = D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
        center: D2D_POINT_2F { x: bcx, y: bcy },
        gradientOriginOffset: D2D_POINT_2F { x: -bw * 0.18, y: -bh * 0.2 },
        radiusX: bw * 0.9,
        radiusY: bh * 1.0,
    };
    let stops = [
        D2D1_GRADIENT_STOP { position: 0.0, color: argb_to_color(lighten(ring, 0.3)) },
        D2D1_GRADIENT_STOP { position: 1.0, color: argb_to_color(darken(ring, 0.12)) },
    ];
    if let Ok(gsc) = rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP) {
        if let Ok(br) = ctx.CreateRadialGradientBrush(&fill_prop, None, &gsc) {
            ctx.FillRoundedRectangle(&main_rect, &br);
        }
    }
    // core 高亮点（球心——morph 中渐隐/渐显）。
    if ballness > 0.02 {
        let cstops = [
            D2D1_GRADIENT_STOP { position: 0.0, color: argb_to_color(core) },
            D2D1_GRADIENT_STOP { position: 1.0, color: fade_a(core, 0.0) },
        ];
        if let Ok(gsc) = rt.CreateGradientStopCollection(&cstops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP) {
            if let Ok(cb) = ctx.CreateRadialGradientBrush(
                &D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                    center: D2D_POINT_2F { x: bcx - bw * 0.06, y: bcy - bh * 0.08 },
                    gradientOriginOffset: D2D_POINT_2F { x: 0.0, y: 0.0 },
                    radiusX: 5.0,
                    radiusY: 5.0,
                },
                None,
                &gsc,
            ) {
                let r = (BALL_D * 0.24) * (0.4 + 0.6 * ballness);
                ctx.FillEllipse(
                    &D2D1_ELLIPSE {
                        point: D2D_POINT_2F { x: bcx, y: bcy },
                        radiusX: r,
                        radiusY: r,
                    },
                    &cb,
                );
            }
        }
    }
    let hr = ctx.EndDraw(None, None);
    if hr.is_err() {
        return false;
    }
    chain.Present(0, DXGI_PRESENT_RESTART).is_ok()
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

/// 同色降 alpha（core 渐隐渐显）。
fn fade_a(argb: u32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        a,
        r: ((argb >> 16) & 0xFF) as f32 / 255.0,
        g: ((argb >> 8) & 0xFF) as f32 / 255.0,
        b: (argb & 0xFF) as f32 / 255.0,
    }
}
