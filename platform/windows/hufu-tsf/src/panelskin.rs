//! 【虎娘面板皮肤 2026-10-06】独立贴图皮肤通道（经典皮肤系统零改动）。
//!
//! 来源：虎娘 2026.10.5.1 逆向实测（E:\DSH-KF\虎娘\逆向-2026.10.5）。
//! 立绘皮肤 = 一张整图（气泡板 + 右上角立绘），渲染语义（实测钉死）：
//! - 窗口高度 = 整图高 × 缩放，纵向永不拉伸（立绘头顶透明区在窗内）；
//! - 横向：右帽（立绘 + 气泡右端）固定锚右缘，中带压缩/拉伸补宽度，
//!   窗口最窄 ≈ 右帽 + min_middle；
//! - 内容（候选行）右对齐，右边距 = 内容区右缘到图右缘；
//! - 缩放 s = font_point / design_font（与经典皮肤 Layout::scale_geometry
//!   的「设置字号÷皮肤字号」同思想）。
//!
//! 数据面：server 管道 op "panel_skin" 返回 panel-skins/{id}.json 全文
//! （立绘为 base64 data-URL，与经典 overlay 同形态）。本模块自管缓存
//! （2.5s 例行保鲜），不碰 g.skin。经典路径只在 candwin2::show() 头部
//! 有一个分支点：面板未就绪/渲染失败 → 回落经典路径，绝不空白。

use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromRect, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows_core::PCWSTR;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::sound::base64_decode;

// ── 缓存与开关 ──────────────────────────────────────────────────────

struct PanelState {
    enabled: bool,
    look: Option<PanelLook>,
    /// 解码后的立绘 BGRA（预乘）：(w, h, data, fnv 键)
    art: Option<(u32, u32, Vec<u8>, u64)>,
    fetched_at: Option<Instant>,
}

static STATE: Mutex<Option<PanelState>> = Mutex::new(None);

/// 面板模式是否生效（enabled 且皮肤+立绘就绪）。各守卫共用，必须廉价。
pub fn enabled() -> bool {
    match STATE.lock() {
        Ok(g) => matches!(
            g.as_ref(),
            Some(s) if s.enabled && s.look.is_some() && s.art.is_some()
        ),
        Err(_) => false,
    }
}

/// 面板落位记忆（锚点丢失帧沿用上次位置）
static LAST_POS: Mutex<Option<(i32, i32)>> = Mutex::new(None);

pub fn note_pos(p: (i32, i32)) {
    if let Ok(mut g) = LAST_POS.lock() {
        *g = Some(p);
    }
}

pub fn last_pos_pub() -> Option<(i32, i32)> {
    match LAST_POS.lock() {
        Ok(g) => *g,
        Err(_) => None,
    }
}

/// 在面板就绪态下执行 f（锁内访问皮肤参数与立绘，免逐帧克隆）。
/// 未就绪返回 None（调用方回落经典路径）。
pub fn with_active<R>(f: impl FnOnce(&PanelLook, &(u32, u32, Vec<u8>, u64)) -> R) -> Option<R> {
    let g = STATE.lock().ok()?;
    let s = g.as_ref()?;
    if !s.enabled {
        return None;
    }
    let look = s.look.as_ref()?;
    let art = s.art.as_ref()?;
    Some(f(look, art))
}

/// 当前立绘键（candwin2 位图缓存比对用）
pub fn art_key() -> u64 {
    match STATE.lock() {
        Ok(g) => g.as_ref().and_then(|s| s.art.as_ref()).map(|a| a.3).unwrap_or(0),
        Err(_) => 0,
    }
}

/// 由 tsf.rs load_skin 尾部调用：按同节奏（2.5s 例行）保鲜面板皮肤，
/// 独立管道 op "panel_skin"，不碰经典皮肤缓存。
pub fn ensure_loaded() {
    let stale = {
        let g = match STATE.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        match g.as_ref().and_then(|s| s.fetched_at) {
            None => true,
            Some(t) => t.elapsed() > Duration::from_millis(2500),
        }
    };
    if !stale {
        return;
    }
    let mut fetched = false;
    if let Some(v) = crate::ipc::call(&serde_json::json!({ "op": "panel_skin" })) {
        let en = v.get("enabled").and_then(|x| x.as_bool()).unwrap_or(false);
        let skin_v = v.get("skin").cloned().filter(|x| !x.is_null());
        apply_response(en, skin_v);
        fetched = true;
    }
    if fetched {
        if let Ok(mut g) = STATE.lock() {
            if let Some(s) = g.as_mut() {
                s.fetched_at = Some(Instant::now());
            }
        }
    }
}

fn apply_response(enabled: bool, skin_v: Option<serde_json::Value>) {
    let key = skin_v
        .as_ref()
        .map(|v| fnv64(v.to_string().as_bytes()))
        .unwrap_or(0);
    let look = skin_v.as_ref().and_then(PanelLook::parse);
    let art = look.as_ref().and_then(|l| decode_art(&l.art_b64)).map(|(w, h, d)| (w, h, d, key));
    if let Ok(mut g) = STATE.lock() {
        match g.as_mut() {
            Some(s) => {
                s.enabled = enabled;
                s.look = look;
                s.art = art;
            }
            None => {
                *g = Some(PanelState {
                    enabled,
                    look,
                    art,
                    fetched_at: None,
                });
            }
        }
    }
}

fn fnv64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

// ── 面板皮肤模型 ────────────────────────────────────────────────────

pub struct PanelLook {
    /// 皮肤设计字号（虎娘皮肤恒 200；缩放基准）
    pub design_font: f32,
    /// 用户字号（pt，候选文字大小）
    pub font_point: f32,
    pub font_face: String,
    pub art_b64: String,
    pub art_w: u32,
    pub art_h: u32,
    /// [左帽, 中带自然宽, 右帽]（设计 px；右帽锚图右缘、左帽锚图左缘，
    /// 中带 = 其间全部）
    pub slice: [f32; 3],
    /// 气泡内容区 [x, y, w, h]（设计 px）
    pub content: [f32; 4],
    /// 窗口最窄时中带保留宽（设计 px）
    pub min_middle: f32,
    /// 内容驱动增宽时的左垫（设计 px；虎娘实测 ~474）
    pub grow_left: f32,
    /// 候选间距（设计 px）
    pub candidate_spacing: f32,
    /// 文字色 / 选中色 / 注释色 / 编码色
    pub text_color: [u8; 4],
    pub first_color: [u8; 4],
    pub comment_color: [u8; 4],
    pub code_color: [u8; 4],
}

fn hex_color(v: Option<&serde_json::Value>, def: [u8; 4]) -> [u8; 4] {
    let s = match v.and_then(|x| x.as_str()) {
        Some(s) => s,
        None => return def,
    };
    let t = s.trim().trim_start_matches('#');
    if !t.is_ascii() {
        return def;
    }
    let n = t.len() / 2;
    if !matches!(n, 3 | 4) {
        return def;
    }
    let mut buf = [0u8, 0, 0, 0xFF];
    for i in 0..n {
        buf[i] = u8::from_str_radix(&t[i * 2..i * 2 + 2], 16).unwrap_or(0);
    }
    buf
}

impl PanelLook {
    fn parse(v: &serde_json::Value) -> Option<PanelLook> {
        let get = |k: &str| v.get(k);
        let art_w = get("art_w").and_then(|x| x.as_u64())? as u32;
        let art_h = get("art_h").and_then(|x| x.as_u64())? as u32;
        if art_w == 0 || art_h == 0 || art_w > 8192 || art_h > 8192 {
            return None;
        }
        let art_b64 = get("art").and_then(|x| x.as_str()).unwrap_or("").to_string();
        if art_b64.is_empty() {
            return None;
        }
        let f3 = |arr: Option<&Vec<serde_json::Value>>, def: [f32; 3]| -> [f32; 3] {
            match arr {
                Some(a) if a.len() == 3 => [
                    a[0].as_f64().unwrap_or(def[0] as f64) as f32,
                    a[1].as_f64().unwrap_or(def[1] as f64) as f32,
                    a[2].as_f64().unwrap_or(def[2] as f64) as f32,
                ],
                _ => def,
            }
        };
        let content = match get("content").and_then(|x| x.as_array()) {
            Some(a) if a.len() == 4 => [
                a[0].as_f64().unwrap_or(0.0) as f32,
                a[1].as_f64().unwrap_or(0.0) as f32,
                a[2].as_f64().unwrap_or(0.0) as f32,
                a[3].as_f64().unwrap_or(0.0) as f32,
            ],
            // 缺省：图下 1/3 整宽作内容带（宽松回退，转换器总会写出）
            _ => [0.0, art_h as f32 * 0.6, art_w as f32, art_h as f32 * 0.35],
        };
        let colors = get("colors");
        Some(PanelLook {
            design_font: get("design_font")
                .and_then(|x| x.as_f64())
                .unwrap_or(200.0)
                .clamp(8.0, 400.0) as f32,
            font_point: get("font_point")
                .and_then(|x| x.as_f64())
                .unwrap_or(17.0)
                .clamp(8.0, 72.0) as f32,
            font_face: {
                let f = get("font_face").and_then(|x| x.as_str()).unwrap_or("");
                if f.is_empty() {
                    "Microsoft YaHei UI".into()
                } else {
                    f.to_string()
                }
            },
            art_b64,
            art_w,
            art_h,
            slice: f3(get("slice").and_then(|x| x.as_array()), [0.0, art_w as f32, art_w as f32]),
            content,
            min_middle: get("min_middle")
                .and_then(|x| x.as_f64())
                .unwrap_or(40.0)
                .clamp(0.0, 500.0) as f32,
            grow_left: get("grow_left")
                .and_then(|x| x.as_f64())
                .unwrap_or(0.0)
                .clamp(0.0, 2000.0) as f32,
            candidate_spacing: get("candidate_spacing")
                .and_then(|x| x.as_f64())
                .unwrap_or(60.0)
                .clamp(0.0, 400.0) as f32,
            text_color: hex_color(colors.and_then(|c| c.get("text")), [32, 32, 34, 255]),
            first_color: hex_color(colors.and_then(|c| c.get("first")), [24, 120, 190, 255]),
            comment_color: hex_color(colors.and_then(|c| c.get("comment")), [120, 120, 128, 255]),
            code_color: hex_color(colors.and_then(|c| c.get("code")), [32, 32, 34, 255]),
        })
    }
}

fn decode_art(b64: &str) -> Option<(u32, u32, Vec<u8>)> {
    let payload = if let Some((head, tail)) = b64.split_once(',') {
        if head.contains("base64") {
            tail
        } else {
            b64
        }
    } else {
        b64
    };
    let bytes = base64_decode(payload)?;
    let img = image::load_from_memory(&bytes).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    if w == 0 || h == 0 || w > 8192 || h > 8192 {
        return None;
    }
    // RGBA 直通 → BGRA 预乘（D2D PREMULTIPLIED 要求）
    let mut bgra = rgba.into_raw();
    for px in bgra.chunks_exact_mut(4) {
        let a = px[3] as u32;
        let r = (px[0] as u32 * a + 127) / 255;
        let g = (px[1] as u32 * a + 127) / 255;
        let b = (px[2] as u32 * a + 127) / 255;
        px[0] = b as u8;
        px[1] = g as u8;
        px[2] = r as u8;
    }
    Some((w, h, bgra))
}

// ── 帧几何 ──────────────────────────────────────────────────────────

/// 一条文本的绘制位置（逻辑像素）
pub struct PanelItem {
    pub text: String,
    pub color: [u8; 4],
    /// em 相对主字号的倍率（注释 0.78）
    pub scale: f32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// 本帧全部量化结果（逻辑像素；渲染层 SetTransform(dpi_scale) 统一
/// 缩放，窗口物理尺寸 = 逻辑 × dpi_scale）。
pub struct PanelFrame {
    pub w: i32,
    pub h: i32,
    pub items: Vec<PanelItem>,
    /// 三片源/目标区间（逻辑 px）：[左帽宽, 中带宽, 右帽宽]
    pub slice_dst: [(f32, f32); 3],
    /// 三片源区间（设计 px，即图内坐标）：左帽 [0,slice0)、
    /// 中带 [slice0,slice0+slice1)、右帽 [slice0+slice1,art_w)。
    /// paint 按 src→dst 各自独立映射；src 必须用源坐标（早期版本
    /// 误用 dst 宽当 src 宽，s≠1 时帽区只采到图角透明带）。
    pub slice_src: [(f32, f32); 3],
    pub pos: (i32, i32),
}

fn measure(dwrite: &IDWriteFactory, tf: &IDWriteTextFormat, s: &str) -> Option<f32> {
    let ws: Vec<u16> = s.encode_utf16().collect();
    if ws.is_empty() {
        return None;
    }
    unsafe {
        let layout = dwrite.CreateTextLayout(&ws, tf, f32::MAX, f32::MAX).ok()?;
        let mut m = std::mem::zeroed();
        layout.GetMetrics(&mut m).ok()?;
        Some(m.width)
    }
}

/// 计算一帧的窗口几何与文本排布。
/// `draw_code`: 经典皮肤 inline_preedit=false 时把编码串画进行首
///（true 时编码在宿主内联，窗口不画）。
#[allow(clippy::too_many_arguments)]
pub fn build_frame(
    look: &PanelLook,
    dwrite: &IDWriteFactory,
    tf: &Option<IDWriteTextFormat>,
    tf_small: &Option<IDWriteTextFormat>,
    cands: &[(String, String)],
    raw: &str,
    selected: usize,
    anchor: Option<&RECT>,
    draw_code: bool,
    prev_pos: Option<(i32, i32)>,
) -> Option<PanelFrame> {
    let tf = tf.as_ref()?;
    let s = look.font_point / look.design_font;
    let em = look.font_point * 96.0 / 72.0;
    let em_small = em * 0.78;
    let spacing = (look.candidate_spacing * s).max(2.0);

    let mut items: Vec<PanelItem> = Vec::new();

    // 编码前缀（可选）
    if draw_code && !raw.is_empty() {
        if let Some(w) = measure(dwrite, tf, raw) {
            if w > 0.0 {
                items.push(PanelItem {
                    text: raw.to_string(),
                    color: look.code_color,
                    scale: 1.0,
                    x: 0.0,
                    y: 0.0,
                    w,
                    h: em,
                });
            }
        }
    }
    let sel = selected.min(cands.len().saturating_sub(1));
    for (i, (t, c)) in cands.iter().enumerate() {
        let color = if i == sel { look.first_color } else { look.text_color };
        if let Some(w) = measure(dwrite, tf, t) {
            if w > 0.0 {
                items.push(PanelItem {
                    text: t.clone(),
                    color,
                    scale: 1.0,
                    x: 0.0,
                    y: 0.0,
                    w,
                    h: em,
                });
            }
        }
        let ct = c.trim();
        if !ct.is_empty() {
            let Some(tf_sm) = tf_small.as_ref() else { continue };
            if let Some(w) = measure(dwrite, tf_sm, ct) {
                if w > 0.0 {
                    items.push(PanelItem {
                        text: format!(" {ct}"),
                        color: look.comment_color,
                        scale: 0.78,
                        x: 0.0,
                        y: 0.0,
                        w,
                        h: em_small,
                    });
                }
            }
        }
    }
    if items.is_empty() {
        return None;
    }

    // 总内容宽（含项间距）
    let mut content_w = 0.0f32;
    let mut first = true;
    for it in &items {
        if !first {
            content_w += spacing;
        }
        content_w += it.w;
        first = false;
    }

    // 窗口宽：内容驱动 vs 最小宽。最小宽 = 整图缩放宽（中带不被压缩到
    // 自然宽以下）——蜜桃 n=2 帧（152×75）实测：by_content 分支把中带
    // 501px 压到 43.8px，气泡板+立绘糊成一团，文字叠在压缩板上，即
    // 「挤压」观感的根因。虎娘本尊：内容不足时窗停在整图宽×s，中带
    // 保持自然宽，只在其上拉伸补宽（659 帧实测 501→550 拉伸正常）。
    let right_gap = (look.art_w as f32 - (look.content[0] + look.content[2])) * s;
    let min_w = look.art_w as f32 * s;
    let by_content = content_w + right_gap.max(0.0) + look.grow_left * s;
    let w = by_content.max(min_w).min(4000.0);
    let h = look.art_h as f32 * s;

    // 右对齐排布：行右缘 = 窗宽 − right_gap；行带 = content 纵带垂直居中
    let band_top = look.content[1] * s;
    let band_h = look.content[3] * s;
    let mut right = w - right_gap.max(0.0);
    for it in items.iter_mut().rev() {
        it.x = right - it.w;
        it.y = band_top + (band_h - it.h) / 2.0;
        right = it.x - spacing;
    }

    let pos = place(w as i32, h as i32, anchor, prev_pos);
    // 中带目标宽 = 窗宽 − 左帽 − 右帽；w ≥ 全图宽×s（min 分支）时恒 ≥
    // 自然中带，即中带只会被拉伸、永不被压缩。
    let mid_l = look.slice[0] * s;
    let mid_r = (w - look.slice[2] * s).max(mid_l + 1.0);
    Some(PanelFrame {
        w: w as i32,
        h: h as i32,
        items,
        slice_dst: [
            (0.0, mid_l),
            (mid_l, mid_r),
            (mid_r, w),
        ],
        slice_src: [
            (0.0, look.slice[0]),
            (look.slice[0], look.slice[0] + look.slice[1]),
            (look.slice[0] + look.slice[1], look.art_w as f32),
        ],
        pos,
    })
}

fn place(w: i32, h: i32, anchor: Option<&RECT>, prev: Option<(i32, i32)>) -> (i32, i32) {
    let (mut x, mut y) = match anchor {
        Some(a) => (a.left, a.bottom + 4),
        None => match prev {
            Some(p) => return p,
            None => (64, 64),
        },
    };
    // 钳到锚点所在显示器工作区（物理像素）
    let ar = RECT { left: x, top: y, right: x + 1, bottom: y + 1 };
    let mut mi: MONITORINFO = unsafe { std::mem::zeroed() };
    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    let ok = unsafe { GetMonitorInfoW(MonitorFromRect(&ar, MONITOR_DEFAULTTONEAREST), &mut mi) };
    if ok.as_bool() {
        let wa = mi.rcWork;
        if x + w > wa.right {
            x = wa.right - w;
        }
        if y + h > wa.bottom {
            // 下方放不下 → 翻到插入符上方
            y = match anchor {
                Some(a) => (a.top - h - 4).max(wa.top),
                None => wa.bottom - h,
            };
        }
        x = x.max(wa.left);
        y = y.max(wa.top);
    } else {
        let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        x = x.clamp(0, (sw - w).max(0));
        y = y.clamp(0, (sh - h).max(0));
    }
    (x, y)
}

/// 面板皮肤文本格式两件套（主/注释），调用方缓存。
pub unsafe fn make_formats(dwrite: &IDWriteFactory, look: &PanelLook) -> (Option<IDWriteTextFormat>, Option<IDWriteTextFormat>) {
    let em = look.font_point * 96.0 / 72.0;
    let locale: Vec<u16> = "zh-CN\0".encode_utf16().collect();
    let mk = |fam: &str, size: f32| -> Option<IDWriteTextFormat> {
        let mut b: Vec<u16> = fam.encode_utf16().collect();
        b.push(0);
        dwrite
            .CreateTextFormat(
                PCWSTR(b.as_ptr()),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                PCWSTR(locale.as_ptr()),
            )
            .ok()
    };
    let main = mk(&look.font_face, em).or_else(|| mk("Microsoft YaHei UI", em));
    let small = mk(&look.font_face, em * 0.78).or_else(|| mk("Microsoft YaHei UI", em * 0.78));
    for t in [&main, &small] {
        if let Some(t) = t {
            let _ = t.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
        }
    }
    (main, small)
}

/// 绘制三片立绘 + 右对齐文本行。调用方已 BeginDraw/SetTransform(dpi_scale)。
/// `panel_bm`: 立绘位图缓存（键不符自动重建；设备失败返回 false 回落）。
#[allow(clippy::too_many_arguments)]
pub unsafe fn paint(
    ctx: &ID2D1DeviceContext,
    frame: &PanelFrame,
    tf: &Option<IDWriteTextFormat>,
    tf_small: &Option<IDWriteTextFormat>,
    art: &(u32, u32, Vec<u8>, u64),
    panel_bm: &mut Option<(u64, ID2D1Bitmap1)>,
) -> bool {
    ctx.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));

    let (aw, ah, data, key) = art;
    let cached = panel_bm.as_ref().map(|(k, _)| *k == *key).unwrap_or(false);
    if !cached {
        let bp = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
            colorContext: std::mem::ManuallyDrop::new(None),
        };
        let pitch = *aw * 4;
        match ctx.CreateBitmap(
            D2D_SIZE_U { width: *aw, height: *ah },
            Some(data.as_ptr() as *const std::ffi::c_void),
            pitch,
            &bp,
        ) {
            Ok(b) => {
                *panel_bm = Some((*key, b));
            }
            Err(_) => return false,
        }
    }
    let bm = match panel_bm.as_ref() {
        Some((_, b)) => b,
        None => return false,
    };

    // 三片映射：src=图内设计坐标区间，dst=窗口逻辑 px 区间，各自独立缩放。
    // 左帽/右帽只平移不缩放（宽×s），中带压缩/拉伸补宽。
    let w = frame.w as f32;
    let h = frame.h as f32;
    let mut draw_slice = |src_l: f32, src_r: f32, dst_l: f32, dst_r: f32| {
        if dst_r - dst_l <= 0.5 || src_r - src_l <= 0.5 {
            return;
        }
        let src = D2D_RECT_F { left: src_l, top: 0.0, right: src_r, bottom: *ah as f32 };
        let dst = D2D_RECT_F { left: dst_l, top: 0.0, right: dst_r, bottom: h };
        let _ = ctx.DrawBitmap(
            bm,
            Some(&dst as *const D2D_RECT_F),
            1.0,
            D2D1_INTERPOLATION_MODE_LINEAR,
            Some(&src as *const D2D_RECT_F),
            None,
        );
    };
    for i in 0..3 {
        let (sl, sr) = frame.slice_src[i];
        let (dl, dr) = frame.slice_dst[i];
        draw_slice(sl, sr, dl, dr);
    }

    // 右对齐文本行
    for it in &frame.items {
        let fmt = if it.scale >= 1.0 { tf } else { tf_small };
        let Some(f) = fmt.as_ref() else { continue };
        let sc = match ctx.CreateSolidColorBrush(
            &D2D1_COLOR_F {
                r: it.color[0] as f32 / 255.0,
                g: it.color[1] as f32 / 255.0,
                b: it.color[2] as f32 / 255.0,
                a: it.color[3] as f32 / 255.0,
            },
            None,
        ) {
            Ok(b) => b,
            Err(_) => return false,
        };
        let ws: Vec<u16> = it.text.encode_utf16().collect();
        if ws.is_empty() {
            continue;
        }
        let rect = D2D_RECT_F {
            left: it.x,
            top: it.y,
            right: it.x + it.w + 6.0,
            bottom: it.y + it.h,
        };
        let _ = ctx.DrawText(
            &ws,
            f,
            &rect,
            &sc,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );
    }
    true
}
