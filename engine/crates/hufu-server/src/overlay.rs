//! 【T7b·server 代画挂件 2026-10-06】沉浸宿主（UWP/开始菜单/搜索框）
//! 的候选窗走 server 代画（candwin.rs 自持 ULW 窗）——DLL 侧 overlaywin
//! 兄弟窗不在这条链上，挂件从不显示。本模块把 DLL overlaywin.rs 的挂件
//! 管线移植到 server：纯 Rust 解码/处理核心原样照搬（image crate 同版
//! 本 0.25），Win32 部分改裸 FFI（与 candwin.rs 同风格），单候选窗 =
//! 单挂件态（DLL 版的多 hwnd 注册表在此无意义）。
//!
//! 生命周期挂钩（candwin.rs wnd_proc 内调用，全部在 tray 窗口线程）：
//! - WM_APP_CAND ULW 上屏后 → sync(cand_hwnd, skin, 内容矩形, dpi)
//! - WM_APP_HIDE → hide(cand_hwnd)
//! 皮肤代次：DLL 侧靠 load_skin 推进；server 侧皮肤随 CandFrame 每帧
//! 到达——overlay 子树内容地址（长度+首尾 64B）变化才推代次，避免每键
//! 重解码、也避免全量 base64 哈希（MB 级每键哈希不可接受）。
//!
//! 任何失败（建窗/解码/参数坏）→ 本地静默禁用，代画候选窗路径零改动。

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::Value;

// ── 裸 FFI（candwin.rs 之外本模块自需部分）──
#[repr(C)]
struct POINT {
    x: i32,
    y: i32,
}
#[repr(C)]
struct SIZE {
    cx: i32,
    cy: i32,
}
#[repr(C)]
#[allow(non_snake_case)]
struct BLENDFUNCTION {
    blend_op: u8,
    blend_flags: u8,
    source_constant_alpha: u8,
    alpha_format: u8,
}
#[repr(C)]
struct BITMAPINFOHEADER {
    bi_size: u32,
    bi_width: i32,
    bi_height: i32,
    bi_planes: u16,
    bi_bit_count: u16,
    bi_compression: u32,
    bi_size_image: u32,
    bi_x_pels: i32,
    bi_y_pels: i32,
    bi_clr_used: u32,
    bi_clr_important: u32,
}
#[repr(C)]
struct BITMAPINFO {
    bmi_header: BITMAPINFOHEADER,
    bmi_colors: [u32; 1],
}
#[repr(C)]
struct WNDCLASSW {
    style: u32,
    lpfn_wnd_proc: extern "system" fn(isize, u32, usize, isize) -> isize,
    cb_cls_extra: i32,
    cb_wnd_extra: i32,
    h_instance: isize,
    h_icon: isize,
    h_cursor: isize,
    hbr_background: isize,
    lpsz_menu_name: *const u16,
    lpsz_class_name: *const u16,
}

#[link(name = "user32")]
extern "system" {
    fn RegisterClassW(wc: *const WNDCLASSW) -> u16;
    fn CreateWindowExW(
        ex: u32,
        cls: *const u16,
        name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: isize,
        menu: isize,
        inst: isize,
        param: *const core::ffi::c_void,
    ) -> isize;
    fn DefWindowProcW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn SetWindowPos(
        hwnd: isize,
        after: isize,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        flags: u32,
    ) -> i32;
    fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
    fn DestroyWindow(hwnd: isize) -> i32;
    fn SetTimer(hwnd: isize, id: usize, ms: u32, cb: Option<unsafe extern "system" fn(isize, u32, usize, isize)>) -> usize;
    fn KillTimer(hwnd: isize, id: usize) -> i32;
    fn IsWindow(hwnd: isize) -> i32;
    fn GetWindow(hwnd: isize, cmd: u32) -> isize;    fn UpdateLayeredWindow(
        hwnd: isize,
        hdcdst: isize,
        pptdst: *const POINT,
        psize: *const SIZE,
        hdcsrc: isize,
        pptsrc: *const POINT,
        crkey: u32,
        pblend: *const BLENDFUNCTION,
        flags: u32,
    ) -> i32;
}
#[link(name = "gdi32")]
extern "system" {
    fn CreateCompatibleDC(hdc: isize) -> isize;
    fn DeleteDC(hdc: isize) -> i32;
    fn DeleteObject(o: isize) -> i32;
    fn SelectObject(hdc: isize, o: isize) -> isize;
    fn CreateDIBSection(
        hdc: isize,
        bmi: *const BITMAPINFO,
        usage: u32,
        bits: *mut *mut core::ffi::c_void,
        section: isize,
        offset: u32,
    ) -> isize;
}

const WM_TIMER: u32 = 0x0113;
const WS_POPUP: u32 = 0x8000_0000;
const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
const WS_EX_TOPMOST: u32 = 0x0000_0008;
const WS_EX_NOACTIVATE: u32 = 0x0800_0000;
const WS_EX_LAYERED: u32 = 0x0008_0000;
const WS_EX_TRANSPARENT: u32 = 0x0000_0020; // 鼠标全穿透
const ULW_ALPHA: u32 = 2;
const SW_HIDE: i32 = 0;
const SWP_NOACTIVATE: u32 = 0x0010;
const SWP_NOMOVE: u32 = 0x0002;
const SWP_NOSIZE: u32 = 0x0001;
const SWP_SHOWWINDOW: u32 = 0x0040;
const GW_HWNDPREV: u32 = 3;

/// 动图播放钟（"OV"）
const OVERLAY_TIMER_ID: usize = 0x4F56;
/// 延时消失钟（"OH"）
const OVERLAY_HIDE_TIMER_ID: usize = 0x4F48;

// ── 配置（与 DLL overlaywin.rs 同字段同口径）──

pub(crate) struct ProcSpec {
    pub crop: Option<[u32; 4]>,
    pub key: Option<[u8; 3]>,
    pub tol: u32,
    pub corner: u32,
    pub feather: u32,
    pub star: bool,
}

pub(crate) struct OverlayCfg {
    pub side_left: bool,
    pub base_height: f32,
    pub offset_x: i32,
    pub offset_y: i32,
    pub gap: i32,
    pub flip_h: bool,
    pub above: bool,
    pub alpha: u8,
    pub v_align: VAlign,
    pub image: Vec<u8>,
    pub proc: Option<ProcSpec>,
    pub hide_delay_ms: u32,
}

#[derive(PartialEq, Clone, Copy)]
pub(crate) enum VAlign {
    Center,
    Top,
    Bottom,
}

fn jf_i32(v: &Value, key: &str, default: i32, lo: i32, hi: i32) -> i32 {
    v.get(key)
        .and_then(|x| x.as_i64())
        .map(|n| n.clamp(lo as i64, hi as i64) as i32)
        .unwrap_or(default)
}

fn fnv64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// 标准 base64（含 URL-safe 与无填充容忍）——DLL 侧用 crate::sound::
/// base64_decode，server 无依赖故内置同款。
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a') as u32 + 26),
            b'0'..=b'9' => Some((c - b'0') as u32 + 52),
            b'+' | b'-' => Some(62),
            b'/' | b'_' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut nbits = 0u32;
    for &c in s.as_bytes() {
        if c == b'=' || c == b'\r' || c == b'\n' || c == b' ' || c == b'\t' {
            continue;
        }
        acc = (acc << 6) | val(c)?;
        nbits += 6;
        if nbits >= 8 {
            nbits -= 8;
            out.push((acc >> nbits) as u8);
        }
    }
    Some(out)
}

/// 解析皮肤 overlay 节；None = 未启用/参数坏（渲染端静默跳过）。
fn parse_cfg(skin: &Value) -> Option<Arc<OverlayCfg>> {
    let v = skin
        .pointer("/skin/overlay")
        .or_else(|| skin.get("overlay"))?;
    if !v.get("enabled").and_then(|x| x.as_bool()).unwrap_or(false) {
        return None;
    }
    let image = v.get("image").and_then(|x| x.as_str()).unwrap_or("");
    let payload = match image.split_once(',') {
        Some((head, tail)) if head.contains("base64") => tail,
        _ => image,
    };
    if payload.len() < 32 {
        return None;
    }
    let bytes = base64_decode(payload)?;
    if bytes.len() < 12 || bytes.len() > (10 << 20) {
        return None;
    }
    let png = [0x89u8, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    // 【JPEG 入白名单 2026-11】FFD8FF 魔数（设置页 accept=image/* 一直
    // 能选 JPG 且皮肤里存的是完好的 base64——此前在此被静默拒收，
    // 「已载入并生效」但挂件永不出现）。image crate 已加 jpeg 特性。
    let ok = bytes[0..8] == png
        || &bytes[0..3] == b"GIF"
        || (bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF)
        || (&bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP");
    if !ok {
        return None;
    }
    let alpha = v
        .get("opacity")
        .and_then(|x| x.as_u64())
        .map(|n| n.clamp(0, 100) as u32 * 255 / 100)
        .unwrap_or(255) as u8;
    if alpha == 0 {
        return None;
    }
    let base_height = v
        .get("base_height")
        .and_then(|x| x.as_u64())
        .map(|n| n.clamp(16, 2048) as f32)
        .unwrap_or(240.0);
    let v_align = match v.get("v_align").and_then(|x| x.as_str()) {
        Some("top") => VAlign::Top,
        Some("bottom") => VAlign::Bottom,
        _ => VAlign::Center,
    };
    let proc = v.get("proc").and_then(|p| {
        let crop = p.get("crop").and_then(|c| c.as_array()).and_then(|a| {
            let n: Vec<u32> = a.iter().filter_map(|x| x.as_u64().map(|n| n as u32)).collect();
            (n.len() == 4).then_some([n[0], n[1], n[2], n[3]])
        });
        let key = p.get("key").and_then(|c| c.as_array()).and_then(|a| {
            let n: Vec<u8> = a.iter().filter_map(|x| x.as_u64().map(|n| n as u8)).collect();
            (n.len() == 3).then_some([n[0], n[1], n[2]])
        });
        let crop = crop.filter(|c| c[2] >= 2 && c[3] >= 2);
        let corner = p.get("corner").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
        let feather = p.get("feather").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
        if crop.is_none() && key.is_none() && corner == 0 && feather == 0 {
            return None;
        }
        Some(ProcSpec {
            crop,
            key,
            tol: p.get("tol").and_then(|x| x.as_u64()).unwrap_or(30).clamp(2, 200) as u32,
            corner: corner.min(400),
            feather: feather.min(400),
            star: p.get("cshape").and_then(|x| x.as_str()) == Some("star"),
        })
    });
    Some(Arc::new(OverlayCfg {
        side_left: v.get("side").and_then(|x| x.as_str()) == Some("left"),
        base_height,
        offset_x: jf_i32(v, "offset_x", 0, -4000, 4000),
        offset_y: jf_i32(v, "offset_y", 0, -4000, 4000),
        gap: jf_i32(v, "gap", 8, -400, 2000),
        flip_h: v.get("flip_h").and_then(|x| x.as_bool()).unwrap_or(false),
        above: v.get("layer").and_then(|x| x.as_str()) == Some("above"),
        alpha,
        v_align,
        image: bytes,
        proc,
        hide_delay_ms: v
            .get("hide_delay_ms")
            .and_then(|x| x.as_u64())
            .map(|n| n.clamp(0, 10_000) as u32)
            .unwrap_or(0),
    }))
}

// ── 解码与逐帧处理（与 DLL 同款；首帧就绪即显示，其余后台续）──

struct DecodeState {
    ready: AtomicBool,
    ready_first: AtomicBool,
    w: AtomicU32,
    h: AtomicU32,
    animated: AtomicBool,
    frames: Mutex<Vec<Vec<u8>>>,
    delays: Mutex<Vec<u32>>,
}

impl DecodeState {
    fn new() -> Arc<DecodeState> {
        Arc::new(DecodeState {
            ready: AtomicBool::new(false),
            ready_first: AtomicBool::new(false),
            w: AtomicU32::new(0),
            h: AtomicU32::new(0),
            animated: AtomicBool::new(false),
            frames: Mutex::new(Vec::new()),
            delays: Mutex::new(Vec::new()),
        })
    }
}

fn start_decode(cfg: Arc<OverlayCfg>, target_h_px: u32) -> Option<Arc<DecodeState>> {
    let st = DecodeState::new();
    let st2 = st.clone();
    std::thread::spawn(move || {
        let _ = decode_worker(&st2, &cfg, target_h_px);
    });
    Some(st)
}

fn decode_worker(st: &Arc<DecodeState>, cfg: &OverlayCfg, target_h_px: u32) -> Option<()> {
    use image::AnimationDecoder;
    let bytes = cfg.image.clone();
    let is_gif = bytes.starts_with(b"GIF");
    // 显示尺寸：按 base_height × DPI 折算的目标像素高
    let ch = target_h_px.max(16);
    use image::GenericImageView;
    let (iw, ih) = image::load_from_memory(&bytes).ok()?.dimensions();
    if iw == 0 || ih == 0 {
        return None;
    }
    let cw = ((iw as f64) * (ch as f64) / (ih as f64)).round().max(1.0) as u32;
    st.w.store(cw, Ordering::Release);
    st.h.store(ch, Ordering::Release);
    let mut animated = false;
    let mut idx = 0usize;
    if is_gif {
        if let Ok(dec) = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(&bytes)) {
            animated = true;
            for f in dec.into_frames() {
                match f {
                    Ok(f) => {
                        let (img, ms) = frame_pair(f);
                        emit_frame(st, cfg, idx, &img, ms, cw, ch, true);
                        idx += 1;
                    }
                    Err(_) => break,
                }
            }
        }
    } else {
        // 【非 GIF 分发重构 2026-11】PNG/APNG 走专用解码器；其余格式
        //（静态 WEBP、JPEG——魔数门已放行）load_from_memory 静态兜底。
        // 旧版对非 GIF 一律先 PngDecoder::new(...).ok()?：非 PNG 字节
        // 在此整函数退出，下方「WEBP 兜底」块不可达=静态 WEBP 在代画
        // 路径一直是死的；JPEG 只加特性不改这里会死在同一处。
        match image::codecs::png::PngDecoder::new(std::io::Cursor::new(&bytes)) {
            Ok(dec) if dec.is_apng().unwrap_or(false) => {
                animated = true;
                let frames = dec.apng().ok()?.into_frames();
                for f in frames {
                    match f {
                        Ok(f) => {
                            let (img, ms) = frame_pair(f);
                            emit_frame(st, cfg, idx, &img, ms, cw, ch, true);
                            idx += 1;
                        }
                        Err(_) => break,
                    }
                }
            }
            _ => {
                let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
                emit_frame(st, cfg, 0, &img, 0, cw, ch, false);
            }
        }
    }
    if !animated && st.frames.lock().unwrap_or_else(|e| e.into_inner()).is_empty() {
        // WEBP 静态：load_from_memory 兜底
        let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
        emit_frame(st, cfg, 0, &img, 0, cw, ch, false);
    }
    st.animated.store(animated, Ordering::Release);
    let count = st.frames.lock().unwrap_or_else(|e| e.into_inner()).len();
    if count == 0 {
        return None;
    }
    st.ready.store(true, Ordering::Release);
    Some(())
}

fn frame_pair(f: image::Frame) -> (image::RgbaImage, u32) {
    let (num, den) = f.delay().numer_denom_ms();
    let ms = if den == 0 { 100 } else { num / den };
    (f.into_buffer(), ms.max(1))
}

fn emit_frame(
    st: &DecodeState,
    cfg: &OverlayCfg,
    idx: usize,
    img: &image::RgbaImage,
    delay_ms: u32,
    cw: u32,
    ch: u32,
    animated: bool,
) {
    let processed = process_frame(img, cfg.proc.as_ref(), cw, ch);
    let d = if animated { delay_ms.clamp(20, 1000) } else { 0 };
    st.frames
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(processed);
    st.delays
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(d);
    if idx == 0 {
        st.ready_first.store(true, Ordering::Release);
    }
}

/// 单帧处理管线（与 DLL/设置页同式）：裁剪→缩放→抠图→圆角/羽化 SDF→
/// 预乘 BGRA。
fn process_frame(src: &image::RgbaImage, proc: Option<&ProcSpec>, cw: u32, ch: u32) -> Vec<u8> {
    let crop = proc
        .and_then(|p| p.crop)
        .filter(|c| c[2] >= 2 && c[3] >= 2);
    let img: image::RgbaImage = match crop {
        Some([cx, cy, cwid, chi]) => {
            let x0 = cx.min(src.width().saturating_sub(1));
            let y0 = cy.min(src.height().saturating_sub(1));
            let w0 = cwid.min(src.width() - x0).max(1);
            let h0 = chi.min(src.height() - y0).max(1);
            let c = image::imageops::crop_imm(src, x0, y0, w0, h0).to_image();
            if c.width() != cw || c.height() != ch {
                image::imageops::resize(&c, cw, ch, image::imageops::FilterType::Triangle)
            } else {
                c
            }
        }
        None => {
            if src.width() != cw || src.height() != ch {
                image::imageops::resize(src, cw, ch, image::imageops::FilterType::Triangle)
            } else {
                src.clone()
            }
        }
    };
    let raw = img.into_raw();
    let (w, h) = (cw as usize, ch as usize);
    let key = proc.and_then(|p| p.key);
    let tol = proc.map(|p| p.tol).unwrap_or(0) as f32;
    let soft = (tol * 0.35).max(6.0);
    let corner = proc.map(|p| p.corner).unwrap_or(0) as f32 * (ch as f32 / 240.0);
    let feather = proc.map(|p| p.feather).unwrap_or(0) as f32 * (ch as f32 / 240.0);
    let star = proc.map(|p| p.star).unwrap_or(false);
    let rr = corner.max(feather);
    let half_w = w as f32 / 2.0;
    let half_h = h as f32 / 2.0;
    let mut bgra = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let si = (y * w + x) * 4;
            let di = si;
            let mut a = raw[si + 3] as f32;
            if a > 0.0 {
                if let Some(k) = key {
                    let dist = (raw[si] as f32 - k[0] as f32)
                        .abs()
                        .max((raw[si + 1] as f32 - k[1] as f32).abs())
                        .max((raw[si + 2] as f32 - k[2] as f32).abs());
                    if dist <= tol {
                        a = 0.0;
                    } else if dist < tol + soft {
                        a *= (dist - tol) / soft;
                    }
                }
                if rr > 0.0 && a > 0.0 {
                    let px = x as f32 - half_w + 0.5;
                    let py = y as f32 - half_h + 0.5;
                    let qx = px.abs() - half_w + rr;
                    let qy = py.abs() - half_h + rr;
                    let qxo = qx.max(0.0);
                    let qyo = qy.max(0.0);
                    let sd = if star {
                        qx.min(qy).max(0.0) + (qxo * qxo + qyo * qyo).sqrt() - rr
                    } else {
                        qx.max(qy).min(0.0) + (qxo * qxo + qyo * qyo).sqrt() - rr
                    };
                    let cov = (0.5 - sd).clamp(0.0, 1.0);
                    if cov < a {
                        a = cov;
                    }
                }
            }
            let a8 = a.clamp(0.0, 255.0) as u32;
            // 预乘 BGRA；整体不透明度由 ULW SourceConstantAlpha 承担
            //（与 DLL 同口径）
            bgra[di] = (raw[si + 2] as u32 * a8 / 255) as u8;
            bgra[di + 1] = (raw[si + 1] as u32 * a8 / 255) as u8;
            bgra[di + 2] = (raw[si] as u32 * a8 / 255) as u8;
            bgra[di + 3] = a8 as u8;
        }
    }
    bgra
}

// ── 窗口（单实例；DIB 常驻复用，尺寸变化才重建）──

struct OverlayWin {
    hwnd: isize,
    dc: isize,
    hbm: isize,
    bits: isize,
    dib_w: i32,
    dib_h: i32,
}

impl OverlayWin {
    unsafe fn drop_dib(&mut self) {
        unsafe {
            if self.hbm != 0 {
                let _ = DeleteObject(self.hbm);
                self.hbm = 0;
            }
            if self.dc != 0 {
                let _ = DeleteDC(self.dc);
                self.dc = 0;
            }
        }
        self.bits = 0;
        self.dib_w = 0;
        self.dib_h = 0;
    }

    unsafe fn push_frame(&mut self, dec: &DecodeState, idx: usize, cfg: &OverlayCfg) -> bool {
        let n = {
            let frames = dec.frames.lock().unwrap_or_else(|e| e.into_inner());
            frames.len()
        };
        if n == 0 || self.hwnd == 0 {
            return false;
        }
        let idx = idx % n;
        let w = dec.w.load(Ordering::Acquire).max(1) as i32;
        let h = dec.h.load(Ordering::Acquire).max(1) as i32;
        if w <= 0 || h <= 0 {
            return false;
        }
        if self.hbm == 0 || self.dib_w != w || self.dib_h != h {
            self.drop_dib();
            let bi = BITMAPINFO {
                bmi_header: BITMAPINFOHEADER {
                    bi_size: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    bi_width: w,
                    bi_height: -h,
                    bi_planes: 1,
                    bi_bit_count: 32,
                    bi_compression: 0,
                    bi_size_image: (w * h * 4) as u32,
                    bi_x_pels: 0,
                    bi_y_pels: 0,
                    bi_clr_used: 0,
                    bi_clr_important: 0,
                },
                bmi_colors: [0],
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let hbm = CreateDIBSection(0, &bi, 0, &mut bits, 0, 0);
            if hbm == 0 || bits.is_null() {
                return false;
            }
            let dc = CreateCompatibleDC(0);
            if dc == 0 {
                let _ = DeleteObject(hbm);
                return false;
            }
            let old = SelectObject(dc, hbm);
            if old == 0 {
                let _ = DeleteDC(dc);
                let _ = DeleteObject(hbm);
                return false;
            }
            self.dc = dc;
            self.hbm = hbm;
            self.bits = bits as isize;
            self.dib_w = w;
            self.dib_h = h;
        }
        let row = (w as usize) * 4;
        let dst = self.bits as *mut u8;
        let frames = dec.frames.lock().unwrap_or_else(|e| e.into_inner());
        let data = &frames[idx];
        if cfg.flip_h {
            for y in 0..h as usize {
                let src_row = &data[y * row..(y + 1) * row];
                let dst_row = dst.add(y * row);
                for x in 0..w as usize {
                    let sx = w as usize - 1 - x;
                    std::ptr::copy_nonoverlapping(src_row.as_ptr().add(sx * 4), dst_row.add(x * 4), 4);
                }
            }
        } else {
            std::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
        }
        drop(frames);
        let blend = BLENDFUNCTION {
            blend_op: 1, // AC_SRC_OVER
            blend_flags: 0,
            source_constant_alpha: cfg.alpha,
            alpha_format: 1, // AC_SRC_ALPHA
        };
        let pt = POINT { x: 0, y: 0 };
        let sz = SIZE { cx: w, cy: h };
        UpdateLayeredWindow(self.hwnd, 0, std::ptr::null(), &sz, self.dc, &pt, 0, &blend, ULW_ALPHA) != 0
    }

    unsafe fn destroy(&mut self) {
        unsafe {
            if self.hwnd != 0 {
                let _ = DestroyWindow(self.hwnd);
                self.hwnd = 0;
            }
            self.drop_dib();
        }
    }
}

// ── 单实例状态 ──

struct OverlayEntry {
    ov: Option<OverlayWin>,
    gen: u64,
    disabled: bool,
    cfg: Option<Arc<OverlayCfg>>,
    decoded: Option<Arc<DecodeState>>,
    frame_idx: usize,
    play_start: Instant,
    timer_armed: bool,
    shown: bool,
    last_rect: Option<(i32, i32, i32, i32)>,
    hide_at: Option<Instant>,
}

impl Default for OverlayEntry {
    fn default() -> Self {
        OverlayEntry {
            ov: None,
            gen: u64::MAX,
            disabled: false,
            cfg: None,
            decoded: None,
            frame_idx: 0,
            play_start: Instant::now(),
            timer_armed: false,
            shown: false,
            last_rect: None,
            hide_at: None,
        }
    }
}

static GEN: AtomicU64 = AtomicU64::new(0);
static SKIN_ADDR: AtomicU64 = AtomicU64::new(0);
static ENTRY: Mutex<Option<OverlayEntry>> = Mutex::new(None);
static CLASS_REGISTERED: AtomicBool = AtomicBool::new(false);

/// overlay 子树内容地址（长度+首尾 64B 指纹）：变化才推代次——每帧皮肤
/// 全量 base64 哈希不可接受（MB 级），纯字段哈希会漏图换内容。
fn note_skin_addr(skin: &Value) {
    let v = skin
        .pointer("/skin/overlay")
        .or_else(|| skin.get("overlay"))
        .cloned()
        .unwrap_or(Value::Null);
    let img = v.get("image").and_then(|x| x.as_str()).unwrap_or("");
    let b = img.as_bytes();
    let mut h = fnv64(&(b.len() as u64).to_le_bytes());
    let head = &b[..b.len().min(64)];
    let tail = &b[b.len().saturating_sub(64).max(b.len().min(64))..];
    h = fnv64_combine(h, head);
    h = fnv64_combine(h, tail);
    // 其余小字段一并入指纹
    let rest = v.to_string();
    let rest = rest.replace(img, "");
    h = fnv64_combine(h, rest.as_bytes());
    let prev = SKIN_ADDR.swap(h, Ordering::AcqRel);
    if prev != h {
        GEN.fetch_add(1, Ordering::Release);
    }
}

fn fnv64_combine(mut h: u64, data: &[u8]) -> u64 {
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// 挂件窗过程：两个钟 + 默认过程。
extern "system" fn overlay_wnd_proc(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize {
    if msg == WM_TIMER {
        let id = wparam;
        let mut guard = ENTRY.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = guard.as_mut() {
            let ov_hwnd = entry.ov.as_ref().map(|o| o.hwnd).unwrap_or(0);
            if ov_hwnd == 0 || ov_hwnd != hwnd {
                return 0;
            }
            if id == OVERLAY_TIMER_ID {
                unsafe { advance_frame(entry, hwnd) };
            } else if id == OVERLAY_HIDE_TIMER_ID {
                if let Some(deadline) = entry.hide_at {
                    if Instant::now() >= deadline {
                        if let Some(ov) = entry.ov.as_ref() {
                            unsafe {
                                let _ = KillTimer(ov.hwnd, OVERLAY_TIMER_ID);
                                let _ = KillTimer(ov.hwnd, OVERLAY_HIDE_TIMER_ID);
                                let _ = ShowWindow(ov.hwnd, SW_HIDE);
                            }
                        }
                        entry.shown = false;
                        entry.timer_armed = false;
                        entry.hide_at = None;
                        entry.frame_idx = 0;
                        entry.play_start = Instant::now();
                        return 0;
                    }
                    // 未到点（丢拍兜底重挂）
                    let span = (deadline - Instant::now()).as_millis() as u32;
                    unsafe {
                        let _ = SetTimer(hwnd, OVERLAY_HIDE_TIMER_ID, span.saturating_add(span / 2).max(16), None);
                    }
                }
            }
        }
        return 0;
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

unsafe fn ensure_window(entry: &mut OverlayEntry) -> Option<isize> {
    unsafe {
        if let Some(ov) = entry.ov.as_ref() {
            if IsWindow(ov.hwnd) != 0 {
                return Some(ov.hwnd);
            }
        }
        if entry.ov.is_some() {
            let _ = entry.ov.as_mut().map(|o| o.destroy());
            entry.ov = None;
        }
        if !CLASS_REGISTERED.load(Ordering::Acquire) {
            let cls: Vec<u16> = "HuFuSrvOverlay\0".encode_utf16().collect();
            let wc = WNDCLASSW {
                style: 0,
                lpfn_wnd_proc: overlay_wnd_proc,
                cb_cls_extra: 0,
                cb_wnd_extra: 0,
                h_instance: 0,
                h_icon: 0,
                h_cursor: 0,
                hbr_background: 0,
                lpsz_menu_name: std::ptr::null(),
                lpsz_class_name: cls.as_ptr(),
            };
            if RegisterClassW(&wc) == 0 {
                return None;
            }
            CLASS_REGISTERED.store(true, Ordering::Release);
        }
        let cls: Vec<u16> = "HuFuSrvOverlay\0".encode_utf16().collect();
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT,
            cls.as_ptr(),
            std::ptr::null(),
            WS_POPUP,
            0,
            0,
            4,
            4,
            0,
            0,
            0,
            std::ptr::null(),
        );
        if hwnd == 0 {
            entry.disabled = true;
            return None;
        }
        entry.ov = Some(OverlayWin {
            hwnd,
            dc: 0,
            hbm: 0,
            bits: 0,
            dib_w: 0,
            dib_h: 0,
        });
        entry.last_rect = None;
        Some(hwnd)
    }
}

/// 代画候选窗 ULW 上屏后同步（tray 线程）：
/// content=(l,t,r,b) 候选内容矩形（屏幕坐标）；scale=DPI/96。
pub fn sync(cand_hwnd: isize, skin: &Value, content: (i32, i32, i32, i32), scale: f32) {
    note_skin_addr(skin);
    let mut guard = ENTRY.lock().unwrap_or_else(|e| e.into_inner());
    let entry = guard.get_or_insert_with(OverlayEntry::default);
    let gen = GEN.load(Ordering::Acquire);
    unsafe {
        if entry.gen != gen {
            entry.gen = gen;
            entry.cfg = parse_cfg(skin);
            entry.decoded = None;
            entry.disabled = false;
        }
        if entry.disabled || entry.cfg.is_none() {
            if let Some(ov) = entry.ov.as_ref() {
                let _ = ShowWindow(ov.hwnd, SW_HIDE);
                entry.shown = false;
            }
            return;
        }
        if IsWindow(cand_hwnd) == 0 {
            return;
        }
        let (Some(cfg), Some(dec)) = (entry.cfg.clone(), entry.decoded.clone()) else {
            // 首见：开后台解码（目标高 = base_height × DPI）
            if entry.cfg.is_some() && entry.decoded.is_none() {
                let cfg = entry.cfg.clone().unwrap();
                let target = (cfg.base_height * scale).round().max(16.0) as u32;
                entry.decoded = start_decode(cfg, target);
                entry.frame_idx = 0;
                entry.play_start = Instant::now();
            }
            return;
        };
        if !dec.ready_first.load(Ordering::Acquire) {
            return; // 首帧未就绪：下一拍 sync 再来
        }
        let Some(ov_hwnd) = ensure_window(entry) else {
            return;
        };
        if entry.ov.is_none() {
            return;
        }
        // ── 布局：与 DLL 同式（内容矩形旁，按 side/gap/offset/v_align）
        let (l, t, r, b) = content;
        let ch = dec.h.load(Ordering::Acquire).max(1) as i32;
        let cw = dec.w.load(Ordering::Acquire).max(1) as i32;
        let cand_h_px = b - t;
        let ay = match cfg.v_align {
            VAlign::Top => t,
            VAlign::Bottom => b - ch,
            VAlign::Center => t + (cand_h_px - ch) / 2,
        };
        let gap = (cfg.gap as f32 * scale).round() as i32;
        let ax = if cfg.side_left {
            l - gap - cw
        } else {
            r + gap
        };
        let x = ax + (cfg.offset_x as f32 * scale).round() as i32;
        let y = ay + (cfg.offset_y as f32 * scale).round() as i32;
        let rect = (x, y, x + cw, y + ch);
        if entry.last_rect != Some(rect) || !entry.shown {
            // 挂件层随候选窗 z 序：below=贴正下（默认）
            let _ = SetWindowPos(
                ov_hwnd,
                if cfg.above { 0 } else { cand_hwnd },
                x,
                y,
                cw,
                ch,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            if cfg.above {
                // above=插到候选窗之上（同线程确定性）
                let prev = GetWindow(cand_hwnd, GW_HWNDPREV);
                if prev != ov_hwnd {
                    let _ = SetWindowPos(ov_hwnd, prev, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                }
            }
            entry.last_rect = Some(rect);
            entry.shown = true;
        }
        // 帧推进 + 首帧上屏（tick_playback 只借 entry——ov 是其字段，
        // 分开借=双可变借用）
        tick_playback(entry, &dec, &cfg, ov_hwnd);
    }
}

/// 动图节拍（时间基准，每拍至多 1 帧；WM_TIMER 与 sync 双驱动同钟）。
unsafe fn tick_playback(
    entry: &mut OverlayEntry,
    dec: &Arc<DecodeState>,
    cfg: &Arc<OverlayCfg>,
    ov_h: isize,
) {
    unsafe {
        let Some(ov) = entry.ov.as_mut() else {
            return;
        };
        if !dec.animated.load(Ordering::Acquire) {
            if entry.timer_armed {
                let _ = KillTimer(ov_h, OVERLAY_TIMER_ID);
                entry.timer_armed = false;
            }
            // 静图：确保首帧已上屏
            if entry.last_rect.is_some() {
                ov.push_frame(dec, 0, cfg);
            }
            return;
        }
        if !dec.ready.load(Ordering::Acquire) {
            if !entry.timer_armed {
                let _ = SetTimer(ov_h, OVERLAY_TIMER_ID, 50, None);
                entry.timer_armed = true;
            }
            return;
        }
        let n = dec.frames.lock().unwrap_or_else(|e| e.into_inner()).len();
        if n <= 1 {
            return;
        }
        let delays = dec.delays.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let mut idx = entry.frame_idx % n;
        let mut start = entry.play_start;
        let loop_ms: u64 = delays.iter().map(|d| (*d).max(1) as u64).sum();
        if now.saturating_duration_since(start).as_millis() as u64 > loop_ms {
            start = now;
            entry.play_start = start;
        }
        let mut advanced = 0usize;
        while now.duration_since(start).as_millis() as u32 >= delays[idx].max(1) && advanced < 1 {
            start += std::time::Duration::from_millis(delays[idx].max(1) as u64);
            idx = (idx + 1) % n;
            advanced += 1;
        }
        if advanced > 0 {
            entry.frame_idx = idx;
            entry.play_start = start;
            ov.push_frame(dec, idx, cfg);
        } else if entry.last_rect.is_some() && !entry.timer_armed {
            // 首显静置（未推进）也要有帧在屏
            ov.push_frame(dec, entry.frame_idx % n, cfg);
        }
        if !entry.timer_armed {
            let remain = delays[entry.frame_idx % n]
                .max(1)
                .saturating_sub(now.duration_since(entry.play_start).as_millis() as u32)
                .max(10);
            let _ = SetTimer(ov_h, OVERLAY_TIMER_ID, remain, None);
            entry.timer_armed = true;
        }
    }
}

/// WM_TIMER 驱动的播放推进（与 tick_playback 同钟，别双推）。
unsafe fn advance_frame(entry: &mut OverlayEntry, ov_h: isize) {
    unsafe {
        entry.timer_armed = false; // 本钟已随 WM_TIMER 消化
        let (Some(dec), Some(cfg)) = (entry.decoded.clone(), entry.cfg.clone()) else {
            return;
        };
        if !dec.animated.load(Ordering::Acquire) || !dec.ready.load(Ordering::Acquire) {
            return;
        }
        let n = dec.frames.lock().unwrap_or_else(|e| e.into_inner()).len();
        if n <= 1 {
            return;
        }
        let Some(ov) = entry.ov.as_mut() else {
            return;
        };
        let delays = dec.delays.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let mut idx = entry.frame_idx % n;
        let mut start = entry.play_start;
        let loop_ms: u64 = delays.iter().map(|d| (*d).max(1) as u64).sum();
        if now.saturating_duration_since(start).as_millis() as u64 > loop_ms {
            start = now;
            entry.play_start = start;
        }
        if now.duration_since(start).as_millis() as u32 >= delays[idx].max(1) {
            start += std::time::Duration::from_millis(delays[idx].max(1) as u64);
            idx = (idx + 1) % n;
            entry.frame_idx = idx;
            entry.play_start = start;
            if !ov.push_frame(&dec, idx, &cfg) {
                return;
            }
        }
        let remain = delays[entry.frame_idx % n]
            .max(1)
            .saturating_sub(now.duration_since(entry.play_start).as_millis() as u32)
            .max(10);
        let _ = SetTimer(ov_h, OVERLAY_TIMER_ID, remain, None);
        entry.timer_armed = true;
    }
}

/// 代画候选窗隐藏时（WM_APP_HIDE 处理器内调用）。
pub fn hide(cand_hwnd: isize) {
    let mut guard = ENTRY.lock().unwrap_or_else(|e| e.into_inner());
    let Some(entry) = guard.as_mut() else {
        return;
    };
    let delay = entry.cfg.as_ref().map(|c| c.hide_delay_ms).unwrap_or(0);
    if delay > 0 && entry.ov.is_some() {
        let now = Instant::now();
        let deadline = match entry.hide_at {
            Some(t) if t <= now + std::time::Duration::from_millis(delay as u64) => t,
            _ => now + std::time::Duration::from_millis(delay as u64),
        };
        entry.hide_at = Some(deadline);
        if let Some(ov) = entry.ov.as_ref() {
            unsafe {
                let span = (deadline - now).as_millis() as u32;
                let _ = KillTimer(ov.hwnd, OVERLAY_HIDE_TIMER_ID);
                let _ = SetTimer(
                    ov.hwnd,
                    OVERLAY_HIDE_TIMER_ID,
                    span.saturating_add(span / 2).max(16),
                    None,
                );
            }
        }
        return;
    }
    if let Some(ov) = entry.ov.as_ref() {
        unsafe {
            let _ = KillTimer(ov.hwnd, OVERLAY_TIMER_ID);
            let _ = KillTimer(ov.hwnd, OVERLAY_HIDE_TIMER_ID);
            let _ = ShowWindow(ov.hwnd, SW_HIDE);
        }
    }
    entry.shown = false;
    entry.timer_armed = false;
    entry.frame_idx = 0;
    entry.hide_at = None;
    entry.play_start = Instant::now();
    let _ = cand_hwnd;
}

// ── 纯函数测试 ──
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_roundtrip_and_magic_gate() {
        // 1x1 PNG（标准最小字节串）
        let png: &[u8] = &[
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D,
        ];
        let mut b64 = String::new();
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut acc: u32 = 0;
        let mut nbits = 0u32;
        for &b in png {
            acc = (acc << 8) | b as u32;
            nbits += 8;
            while nbits >= 6 {
                nbits -= 6;
                b64.push(T[((acc >> nbits) & 0x3F) as usize] as char);
            }
        }
        if nbits > 0 {
            acc <<= 6 - nbits;
            b64.push(T[(acc & 0x3F) as usize] as char);
        }
        let dec = base64_decode(&b64).unwrap();
        assert_eq!(dec, png, "base64 往返");
        // enabled 但 payload 过短 → None
        let skin: Value = serde_json::json!({
            "overlay": {"enabled": true, "image": "data:image/png;base64,QUJD"}
        });
        assert!(parse_cfg(&skin).is_none(), "payload<32B 拒收");
        // 未 enabled → None
        let skin2: Value = serde_json::json!({"overlay": {"image": "x"}});
        assert!(parse_cfg(&skin2).is_none(), "未启用");
    }

    #[test]
    fn magic_gate_jpeg_webp_bmp() {
        // 【JPEG 入白名单 2026-11】FFD8FF 头须过门（用户反馈挂件不能用
        // JPG）；静态 WEBP 头须过门；未启用的魔数（BMP=42 4D）仍拒收。
        let b64_of = |raw: &[u8]| -> String {
            const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let mut out = String::new();
            let (mut acc, mut nbits) = (0u32, 0u32);
            for &b in raw {
                acc = (acc << 8) | b as u32;
                nbits += 8;
                while nbits >= 6 {
                    nbits -= 6;
                    out.push(T[((acc >> nbits) & 0x3F) as usize] as char);
                }
            }
            if nbits > 0 {
                acc <<= 6 - nbits;
                out.push(T[(acc & 0x3F) as usize] as char);
            }
            out
        };
        let mk_skin = |bytes: &[u8]| {
            serde_json::json!({
                "overlay": {"enabled": true, "image": format!("data:image/jpeg;base64,{}", b64_of(bytes))}
            })
        };
        // JPEG：SOI+APP0 骨架补零到 32B（过 payload≥32 字符与字节≥12 两道门）
        let mut jpeg = vec![0xFFu8, 0xD8, 0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F'];
        jpeg.resize(32, 0);
        let c = parse_cfg(&mk_skin(&jpeg)).expect("JPEG 魔数应过门");
        assert_eq!(c.image.len(), jpeg.len());
        // 静态 WEBP：RIFF+size+WEBP 骨架补零
        let mut webp = vec![b'R', b'I', b'F', b'F', 0, 0, 0, 0, b'W', b'E', b'B', b'P'];
        webp.resize(32, 0);
        assert!(parse_cfg(&mk_skin(&webp)).is_some(), "WEBP 魔数应过门");
        // BMP（42 4D…）不在白名单 → 拒收
        let mut bmp = vec![0x42u8, 0x4D];
        bmp.resize(32, 0);
        assert!(parse_cfg(&mk_skin(&bmp)).is_none(), "非白名单魔数仍拒收");
    }

    #[test]
    fn process_frame_crops_and_premultiplies() {
        // 4x4 全不透明红图 → 处理后 BGRA 预乘：B=G=0, R=A=255
        let img = image::RgbaImage::from_fn(4, 4, |_, _| image::Rgba([255, 0, 0, 255]));
        let out = process_frame(&img, None, 4, 4);
        assert_eq!(out.len(), 4 * 4 * 4);
        assert_eq!(out[0], 0, "B=0");
        assert_eq!(out[2], 255, "R=255");
        assert_eq!(out[3], 255, "A=255");
    }
}
