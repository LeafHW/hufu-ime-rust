//! 候选窗贴图挂件（兄弟窗）—— HuFuCandOverlay。
//!
//! 候选窗旁跟随显示一张立绘/挂件图/动图的第二个顶层窗。与外挂方案（外部
//! 进程 WinEventHook 猜窗口）的本质区别：候选窗是本 dll 自己画的，兄弟窗
//! 直接读候选窗最终矩形取位——零跟随延迟、零 z 序争抢、零锚点竞态。
//!
//! 设计约束（2026-10-02 用户拍板「兄弟窗」）：
//! - 绝不触碰候选窗（candwin2）的锚点/几何代码——只读不写；
//! - ULW 分层窗 + WS_EX_TRANSPARENT：全程鼠标穿透，对打字/游戏零干扰；
//! - 皮肤 JSON `overlay` 节两条数据路径：
//!   ① 静态图 = 设置页烤好的 PNG data-URL（proc 缺省），解码即用；
//!   ② 动图   = 原图 data-URL（GIF/APNG）+ proc 处理参数，渲染端逐帧
//!      套「裁剪→缩放→抠图→圆角/羽化 SDF→预乘」管线（与设置页 JS 同式），
//!      定时器逐帧 ULW 播放（隐藏即停，省 CPU）；整段循环播放。
//! - 任何失败（建窗/解码/参数坏）→ 本地静默禁用，候选窗路径一行不改。
//! 【动画组(上屏反应)已搁置 2026-10-03】idle_until/burst/idle_take/
//! action_take/notify_commit/驻留整套机制按用户决定拆除,素材与实现
//! 留存于 E:\DSH-KF\项目可行性\ 与 git 历史;重做时从本段注释回溯。
//!
//! 生命周期挂钩（全部一行级调用，见 candwin2）：
//! - show() 最终 SetWindowPos 后与 fade_tick_shared 收尾 → sync_for；
//! - WM_APP_HIDE_CAND（全部隐藏源的唯一必经点）→ hide_for；
//! - 候选窗 WM_NCDESTROY → destroy_for；
//! - 皮肤拉取成功（tsf::load_skin）→ note_skin（overlay 子树变了才推进
//!   代次——2.5s 例行重拉不触发重解码）。
//!
//! 状态不用 Shared（避开 cand2 take/put-back 让渡协议）：模块级注册表
//! 以候选窗 hwnd 为键（同 CAND_SHADOW_INSET 模式）。

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Value;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_core::PCWSTR;

/// 与候选窗同款的进程级一次注册
static CLASS_REGISTERED: AtomicBool = AtomicBool::new(false);
/// 皮肤代次：overlay 子树内容变化才 +1
static OVERLAY_GEN: AtomicU64 = AtomicU64::new(0);
/// 注册表：cand hwnd → 兄弟窗状态（数量=宿主 UI 线程数，个位数）
static OVERLAYS: Mutex<Vec<OverlayEntry>> = Mutex::new(Vec::new());
/// 动图播放钟 id（挂在兄弟窗自身 hwnd 上，消息落在候选窗线程）
pub(crate) const OVERLAY_TIMER_ID: usize = 0x4F56; // "OV"
/// 延时消失钟 id（hide_delay_ms>0 时挂短钟，到点收窗）
pub(crate) const OVERLAY_HIDE_TIMER_ID: usize = 0x4F48; // "OH"
/// 【补拍钟 2026-11】首帧解码未就绪时的 30ms 自查钟：就绪即上屏——
/// 旧实现「首帧未就绪直接 return、等下一次 sync」=挂件永远慢一拍
///（导入后要点一两下才出 / 一搜就走的宿主永远赶不上的病根）。
pub(crate) const OVERLAY_RESYNC_TIMER_ID: usize = 0x4F52; // "OR"

static OVERLAY_SKIN_HASH: AtomicU64 = AtomicU64::new(0);

/// 沙盒取证（diag 目录授过全应用包写权限；AppContainer 宿主 trace
/// 写不进去）：关键分支一次性落 notes-<pid>.txt，只作诊断不进热路径。
fn note_once(key: &'static str, msg: &str) {
    static SEEN: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
    let mut g = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if !g.contains(&key) {
        g.push(key);
        drop(g);
        crate::tsf::diag_note(msg);
    }
}

/// 【预解码缓存 2026-11】皮肤到位即开解（不等首键 sync）：
/// (img_key, built_for, proc_key) → 解码态。新进程首段/导入后的首段即显。
/// 【处理指纹 2026-10-07】键加 proc_key——纯改处理参数也要重解。
static PREWARM: Mutex<Option<(u64, u32, u64, Arc<DecodeState>)>> = Mutex::new(None);

/// 系统主屏 DPI（预解码目标高估算；真窗口 DPI 不同则 sync 按
/// built_for 失配重解——多数进程同屏即命中）。
fn sys_dpi_scale() -> f32 {
    unsafe { GetDpiForSystem().max(96) as f32 / 96.0 }
}

/// 皮肤到位:仅当 overlay 子树内容变化才推进代次——皮肤的 2.5s 例行重拉
/// 不再触发整段 GIF 重解码(首键出图慢+无谓 CPU 的根因)。
/// 【预解码 2026-11】代次推进时顺手预解码：不等首键 sync 才起跑。
pub fn note_skin(skin: &Value) {
    let s = skin
        .pointer("/skin/overlay")
        .or_else(|| skin.get("overlay"))
        .map(|v| v.to_string())
        .unwrap_or_default();
    let h = fnv64(s.as_bytes());
    let prev = OVERLAY_SKIN_HASH.swap(h, Ordering::AcqRel);
    if prev != h {
        OVERLAY_GEN.fetch_add(1, Ordering::Release);
        if let Some(cfg) = parse_cfg(skin) {
            let target_h = cfg.base_height * sys_dpi_scale();
            let key = (cfg.img_key, target_h.round() as u32, cfg.proc_key);
            let mut pw = PREWARM.lock().unwrap_or_else(|e| e.into_inner());
            // 同键已在（解码中/已就绪/已失败）不重启——失败随下代次重试
            let need = !pw
                .as_ref()
                .is_some_and(|(k, b, pk, _)| *k == key.0 && *b == key.1 && *pk == key.2);
            if need {
                let dec = start_decode(cfg, target_h);
                *pw = Some((key.0, key.1, key.2, dec));
            }
        }
    }
}

// ── 配置（从皮肤 JSON 解析；Arc 化避免每拍克隆图片字节）──

pub(crate) struct ProcSpec {
    /// 源图裁剪区 [x,y,w,h]（源图像素）；None=全图
    pub crop: Option<[u32; 4]>,
    /// 抠图键色 RGB；None=不抠
    pub key: Option<[u8; 3]>,
    pub tol: u32,
    pub corner: u32,
    pub feather: u32,
    /// 【四角星 2026-11】圆角样式：false=圆角（标准圆角矩形 SDF）；
    /// true=四角星（内项取 min 的旧式 SDF——曾为 bug，尖角朝四边中点、
    /// 腰部内凹，用户实测拉满时效果意外不错，收编为正式样式）。
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
    /// ULW SourceConstantAlpha（0-255）
    pub alpha: u8,
    pub v_align: VAlign,
    /// 【处理指纹 2026-10-07】影响「处理完的帧」的全部参数（crop/键色/
    /// 容差/圆角/羽化/四角星/翻转）的指纹——解码缓存存的是成品帧，
    /// 缓存键必须含它：纯改处理参数（滑杆）时代次推进但 img/高度没变，
    /// 旧键不变=复用旧参数成品帧（用户实录「改圆角/羽化要拖一下高度
    /// 才生效」的根因）。image 字节由 img_key 表达，不重复入指纹。
    pub proc_key: u64,
    pub image: Vec<u8>,
    pub img_key: u64,
    /// 动图处理参数；None=静态烤成品路径
    pub proc: Option<ProcSpec>,
    /// 延时消失(毫秒)：0=候选窗一收立即收(默认)；>0=候选窗收起后
    /// 停留该时长再隐藏(200-2000)——候选窗在延时期内重现则撤销收窗
    pub hide_delay_ms: u32,
}

#[derive(PartialEq, Clone, Copy)]
enum VAlign {
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

/// 解析皮肤里的 overlay 节；返回 None = 未启用/参数坏（渲染端静默跳过）。
/// 两形态读取（与 anim/color_f 同款口径）：`/skin/overlay` 或顶层 `overlay`。
fn parse_cfg(skin: &Value) -> Option<Arc<OverlayCfg>> {
    let v = skin
        .pointer("/skin/overlay")
        .or_else(|| skin.get("overlay"))?;
    if !v.get("enabled").and_then(|x| x.as_bool()).unwrap_or(false) {
        return None;
    }
    let image = v.get("image").and_then(|x| x.as_str()).unwrap_or("");
    // data URL 形态（data:image/png;base64,xxx）或裸 base64 都收
    let payload = match image.split_once(',') {
        Some((head, tail)) if head.contains("base64") => tail,
        _ => image,
    };
    if payload.len() < 32 {
        return None;
    }
    let bytes = crate::sound::base64_decode(payload)?;
    // 魔数 sanity（PNG/APNG、GIF、WEBP；其余一律不收）+ 10MB 上限
    if bytes.len() < 12 || bytes.len() > (10 << 20) {
        return None;
    }
    let png = [0x89u8, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    // 【JPEG 入白名单 2026-11】FFD8FF 魔数（与 server 侧 overlay.rs 同款；
    // image crate 已加 jpeg 特性，静态走下方最终 else 的 load_from_memory）。
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
        .map(|n| (n.clamp(16, 2048) as f32))
        .unwrap_or(240.0);
    let v_align = match v.get("v_align").and_then(|x| x.as_str()) {
        Some("top") => VAlign::Top,
        Some("bottom") => VAlign::Bottom,
        _ => VAlign::Center,
    };
    // 动图处理参数（proc 节存在且非全空即启用参数管线）
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
            return None; // 全空参数 = 无处理必要，走静态路径
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
    // 【处理指纹 2026-10-07】crop/键色/容差/圆角/羽化/四角星/翻转 的字节
    // 序指纹（影响成品帧的全部参数；image 由 img_key 表达不重复入内）
    let proc_key = {
        let mut buf: Vec<u8> = Vec::with_capacity(48);
        match &proc {
            None => buf.push(0),
            Some(p) => {
                buf.push(1);
                match p.crop {
                    None => buf.push(0),
                    Some(c) => {
                        buf.push(1);
                        for x in c {
                            buf.extend_from_slice(&x.to_le_bytes());
                        }
                    }
                }
                match p.key {
                    None => buf.push(0),
                    Some(k) => {
                        buf.push(1);
                        buf.extend_from_slice(&k);
                    }
                }
                buf.extend_from_slice(&p.tol.to_le_bytes());
                buf.extend_from_slice(&p.corner.to_le_bytes());
                buf.extend_from_slice(&p.feather.to_le_bytes());
                buf.push(p.star as u8);
            }
        }
        buf.push(v.get("flip_h").and_then(|x| x.as_bool()).unwrap_or(false) as u8);
        fnv64(&buf)
    };
    let flip_h = v.get("flip_h").and_then(|x| x.as_bool()).unwrap_or(false);
    Some(Arc::new(OverlayCfg {
        side_left: v.get("side").and_then(|x| x.as_str()) == Some("left"),
        base_height,
        offset_x: jf_i32(v, "offset_x", 0, -4000, 4000),
        offset_y: jf_i32(v, "offset_y", 0, -4000, 4000),
        gap: jf_i32(v, "gap", 8, -400, 2000),
        flip_h,
        above: v.get("layer").and_then(|x| x.as_str()) == Some("above"),
        alpha,
        v_align,
        img_key: fnv64(&bytes),
        proc_key,
        image: bytes,
        proc,
        hide_delay_ms: v
            .get("hide_delay_ms")
            .and_then(|x| x.as_u64())
            .map(|n| n.clamp(0, 10_000) as u32)
            .unwrap_or(0),
    }))
}

fn fnv64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

// ── 解码与逐帧处理（后台线程，不占宿主 UI 线程）──

struct DecodeState {
    ready: AtomicBool,
    /// 首帧就绪:解完第 0 帧即可显示(候选窗第一键就出人物),其余帧后台续
    ready_first: AtomicBool,
    /// 显示尺寸(帧已处理到此尺寸;解码线程写、候选窗线程读)
    w: AtomicU32,
    h: AtomicU32,
    /// 解码时用的目标高度(base_height×DPI);变了要重解码
    built_for: AtomicU32,
    /// 【处理指纹 2026-10-07】解码时用的处理参数指纹(proc_key);变了要
    /// 重解码——成品帧缓存着按旧圆角/羽化处理过的像素,纯参数改动必须
    /// 让缓存失配(用户实录「改圆角/羽化要拖一下高度才生效」的根因)
    proc_key: AtomicU64,
    /// 处理完的 BGRA 预乘帧(尺寸 w×h,未翻转)
    frames: Mutex<Vec<Vec<u8>>>,
    /// 每帧时长 ms(钳 20-1000;静态帧空)
    delays: Mutex<Vec<u32>>,
    animated: AtomicBool,
}

fn start_decode(cfg: Arc<OverlayCfg>, target_h: f32) -> Arc<DecodeState> {
    let st = Arc::new(DecodeState {
        ready: AtomicBool::new(false),
        ready_first: AtomicBool::new(false),
        w: AtomicU32::new(0),
        h: AtomicU32::new(0),
        built_for: AtomicU32::new(target_h.round() as u32),
        proc_key: AtomicU64::new(cfg.proc_key),
        frames: Mutex::new(Vec::new()),
        delays: Mutex::new(Vec::new()),
        animated: AtomicBool::new(false),
    });
    let st2 = st.clone();
    std::thread::spawn(move || {
        let _ = decode_worker(&cfg, target_h, &st2);
        let ok = !st2.frames.lock().unwrap_or_else(|e| e.into_inner()).is_empty();
        if ok {
            st2.ready.store(true, Ordering::Release);
        } else {
            crate::tsf::trace("ov: 解码失败(图坏/格式不支持)");
        }
    });
    st
}

fn decode_worker(cfg: &OverlayCfg, target_h: f32, st: &DecodeState) -> Option<()> {
    use image::AnimationDecoder;
    let bytes = &cfg.image;
    let png_magic = [0x89u8, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let is_gif = bytes.len() > 3 && &bytes[0..3] == b"GIF";
    let is_png = bytes.len() >= 8 && bytes[0..8] == png_magic;
    // 流式逐帧(不再整段收集:65 帧 640x640 的中间态要 106MB 内存)
    let mut animated = false;
    let mut iter: Box<dyn Iterator<Item = (image::RgbaImage, u32)>> = if is_gif {
        animated = true;
        Box::new(
            image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes))
                .ok()?
                .into_frames()
                .take(240)
                .filter_map(|f| f.ok())
                .map(frame_pair),
        )
    } else if is_png {
        let dec = image::codecs::png::PngDecoder::new(std::io::Cursor::new(bytes)).ok()?;
        if dec.is_apng().unwrap_or(false) {
            animated = true;
            Box::new(
                dec.apng()
                    .ok()?
                    .into_frames()
                    .take(240)
                    .filter_map(|f| f.ok())
                    .map(frame_pair),
            )
        } else {
            Box::new(std::iter::once((
                image::load_from_memory(bytes).ok()?.to_rgba8(),
                0,
            )))
        }
    } else {
        Box::new(std::iter::once((
            image::load_from_memory(bytes).ok()?.to_rgba8(),
            0,
        )))
    };
    // 首帧定显示尺寸 → 立即处理入库 → ready_first(首键即显)
    let first = iter.next()?;
    let (sw, sh) = (first.0.width().max(1), first.0.height().max(1));
    let ch = target_h.round().clamp(8.0, 4096.0) as u32;
    // 纵横比取「实际参与缩放的内容」：有裁剪时用裁剪区，否则整图——
    // 用整图比会把竖向裁剪区拉宽(用户实测「打出来变胖、预览正常」，
    // 预览 JS 是先裁后算比例，两条路径由此对齐)
    let (aw, ah) = match cfg
        .proc
        .as_ref()
        .and_then(|p| p.crop)
        .filter(|c| c[2] >= 2 && c[3] >= 2)
    {
        Some([_, _, w, h]) => (w, h),
        None => (sw, sh),
    };
    let cw = ((ch as f32) * (aw as f32) / (ah as f32))
        .round()
        .clamp(1.0, 4096.0) as u32;
    st.w.store(cw, Ordering::Release);
    st.h.store(ch, Ordering::Release);
    st.animated.store(animated, Ordering::Release);
    emit_frame(st, cfg, 0, &first.0, first.1, cw, ch, animated);
    let mut count = 1usize;
    let per_frame = (cw as usize) * (ch as usize) * 4;
    let cap = (64 * 1024 * 1024 / per_frame.max(1)).clamp(1, 180);
    for (idx, (img, ms)) in iter.enumerate() {
        if count >= cap {
            break;
        }
        emit_frame(st, cfg, idx + 1, &img, ms, cw, ch, animated);
        count += 1;
    }
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

/// 单帧处理管线（与设置页 JS 逐式一致）:
/// 裁剪 → 缩放到显示尺寸 → 抠图(软边) → 圆角/羽化 SDF → 预乘 BGRA。
/// 参数 corner/feather 以 240px 输出高为基准折算。
fn process_frame(
    src: &image::RgbaImage,
    proc: Option<&ProcSpec>,
    cw: u32,
    ch: u32,
) -> Vec<u8> {
    // 1+2) 裁剪+缩放:无裁剪直缩免整帧 clone;Triangle 比 Lanczos3 快
    // 2-3×(解码热路径,240px 档无观感差)
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
    let raw = img.into_raw(); // ImageBuffer 索引是坐标元组,取平铺字节
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
    // 3) 逐像素 + 4) 预乘 BGRA
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
                    // 【圆角公式修正 2026-11】内项应为 max(qx,qy).min(0)（标准
                    // 圆角矩形 SDF，与设置页预览同式）。此前误写成
                    // min(qx,qy).max(0)——常规半径下四角比预览更瘦、半径拉满
                    //（rr ≥ 半宽/半高）时对角线区被多出的正项过量裁剪，实机
                    // 呈四角星而预览呈圆（用户实测报告）。修正后圆角与预览
                    // 一致；旧式公式保留为 cshape=star（四角星）样式。
                    let sd = if star {
                        qx.min(qy).max(0.0) + (qxo * qxo + qyo * qyo).sqrt() - rr
                    } else {
                        qx.max(qy).min(0.0) + (qxo * qxo + qyo * qyo).sqrt() - rr
                    };
                    if corner > 0.0 && sd >= 0.0 {
                        a = 0.0;
                    } else if feather > 0.0 {
                        let t = (-sd / feather).clamp(0.0, 1.0);
                        a *= t;
                    }
                }
            }
            let a8 = a.clamp(0.0, 255.0) as u32;
            bgra[di] = ((raw[si + 2] as u32 * a8 + 127) / 255) as u8;
            bgra[di + 1] = ((raw[si + 1] as u32 * a8 + 127) / 255) as u8;
            bgra[di + 2] = ((raw[si] as u32 * a8 + 127) / 255) as u8;
            bgra[di + 3] = a8 as u8;
        }
    }
    bgra
}

// ── 兄弟窗本体 ──

struct OverlayWin {
    /// HWND 存 isize:raw pointer 非 Send/Sync,注册表是跨线程 static
    hwnd: isize,
    /// DIB 资源(尺寸变化重建)
    dc: isize,
    hbm: isize,
    bits: isize,
    dib_w: i32,
    dib_h: i32,
}

struct OverlayEntry {
    cand: isize,
    ov: Option<OverlayWin>,
    /// 已应用的皮肤代次
    gen: u64,
    /// 建窗/参数失败 → 本次代次内不再尝试
    disabled: bool,
    cfg: Option<Arc<OverlayCfg>>,
    decoded: Option<Arc<DecodeState>>,
    /// 已推上屏的位图键:(w,h,flip) —— 变了才重落 DIB/重推首帧
    drawn: Option<(i32, i32, bool)>,
    /// 当前播放帧号
    frame_idx: usize,
    /// 当前帧的起播时刻(时间基准节拍:帧切换由时钟判定,不靠倒计时存活)
    play_start: std::time::Instant,
    /// 播放钟是否已 armed(防动效期 sync 每 5ms 无条件 SetTimer 重置倒计时)
    timer_armed: bool,
    /// 兄弟窗是否正显示
    shown: bool,
    /// 已应用的目标矩形
    last_rect: Option<(i32, i32, i32, i32)>,
    /// 延时消失截止时刻（None=未在延时收窗期）
    hide_at: Option<std::time::Instant>,
    /// 补拍钟封顶时刻（None=未在补拍期；解码失败 3s 后停臂）
    retry_until: Option<std::time::Instant>,
}

impl OverlayWin {
    /// cand=候选窗句柄：候选窗若是 owned 模式（沉浸宿主——SearchHost/
    /// UWP 里候选窗挂靠宿主视图窗以避开 DWM cloak），挂件窗**必须同
    /// owner 挂靠**——普通顶层窗在打包宿主进程里会被 DWM 整体 cloak
    ///（=挂件在开始菜单/UWP 永远隐身的根因，2026-10-06 实测定案：
    /// 本机沉浸候选走 DLL owned 窗而非 server 代画）。经典模式
    /// owner=0 时与旧行为完全一致。
    unsafe fn create_for(cand: HWND) -> Option<OverlayWin> {
        unsafe {
            Self::create_impl(Some(cand))
        }
    }

    unsafe fn create() -> Option<OverlayWin> {
        unsafe { Self::create_impl(None) }
    }

    unsafe fn create_impl(cand: Option<HWND>) -> Option<OverlayWin> {
        let mut reg_err = 0u32;
        if !CLASS_REGISTERED.swap(true, Ordering::AcqRel) {
            let class: Vec<u16> = "HuFuCandOverlay\0".encode_utf16().collect();
            let wc = WNDCLASSW {
                lpfnWndProc: Some(overlay_wndproc),
                hCursor: LoadCursorW(HINSTANCE(std::ptr::null_mut()), IDC_ARROW)
                    .unwrap_or(HCURSOR(std::ptr::null_mut())),
                lpszClassName: PCWSTR(class.as_ptr()),
                hbrBackground: HBRUSH(std::ptr::null_mut()),
                ..Default::default()
            };
            let _atom = RegisterClassW(&wc);
            if _atom == 0 {
                reg_err = GetLastError().0;
            }
        }
        // 挂件 owner=候选窗的 owner（owned 模式）；经典模式=0（顶层窗）
        let mut owner = HWND(std::ptr::null_mut());
        if let Some(c) = cand {
            let op = GetWindowLongPtrW(c, GWLP_HWNDPARENT);
            if op != 0 {
                owner = HWND(op as *mut _);
            }
        }
        let class: Vec<u16> = "HuFuCandOverlay\0".encode_utf16().collect();
        // TRANSPARENT+LAYERED = 全窗鼠标穿透;TOPMOST|NOACTIVATE|TOOLWINDOW
        // 与候选窗同款。类名含 "HuFuCand" → 切输入法时 hide_all_cand_windows
        // 扫尸自动波及本窗(wndproc 处理 WM_APP_HIDE_CAND 自收)。
        // 【沙盒建窗 2026-11】Microsoft Store 实测 CreateWindowExW 0x5 拒
        //（候选窗同款参数可活——差异元素排查中）：先按全量参数建，失败
        // 依次减 ex 元素/去 owner 阶梯重试，首个成功者即沙盒可活形态。
        let full_ex = WS_EX_LAYERED.0 | WS_EX_TOPMOST.0 | WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0;
        // 【沙盒建窗 2026-11】Microsoft Store 实测三连拒：创建带宿主视图
        // owner=0x5、事后 attach owner=静默失败、无主顶层=DWM cloak。但
        // 候选窗自己（同线程亲窗）当 owner 从未被拒——owned-to-cand 同样
        // 把挂件挂进候选窗的 owner 链（overlay→cand→宿主视图），借道逃
        // cloak（实测 A/B/A 亮度对比=渲染成功）。阶梯：rung1=原宿主视图
        // owner（常规宿主/SearchHost 与 2026-10-06 行为零变化）；拒了才
        // rung2=cand 自身（Store 类沙盒）；再拒做 ex 降级与无主兜底。
        let cand_owner = owner;
        let cand_self = cand.map(|c| c).unwrap_or(HWND(std::ptr::null_mut()));
        let mut hwnd = HWND(std::ptr::null_mut());
        let mut used_ex = 0u32;
        let mut used_owner = HWND(std::ptr::null_mut());
        let ladder: [(u32, HWND); 7] = [
            (full_ex, cand_owner),
            (full_ex, cand_self),
            (full_ex & !WS_EX_LAYERED.0, cand_self),
            (full_ex & !WS_EX_TOPMOST.0, cand_self),
            (full_ex & !WS_EX_NOACTIVATE.0, cand_self),
            (full_ex & !WS_EX_TOOLWINDOW.0, cand_self),
            (full_ex, HWND(std::ptr::null_mut())),
        ];
        let mut errs = String::new();
        let mut hit_rung = 0usize;
        for (i, (ex_bits, o)) in ladder.iter().enumerate() {
            // rung1 的 owner=0 合法（无主顶层=常规宿主原行为）；仅 cand_self
            // 源无效时跳过 cand 系档（i=2..5；i=6=有意无主兜底不跳）
            if i >= 2 && i <= 5 && o.0.is_null() {
                continue;
            }
            match CreateWindowExW(
                WINDOW_EX_STYLE(*ex_bits),
                PCWSTR(class.as_ptr()),
                PCWSTR::null(),
                WINDOW_STYLE(WS_POPUP.0),
                0,
                0,
                10,
                10,
                *o,
                HMENU(std::ptr::null_mut()),
                HINSTANCE(std::ptr::null_mut()),
                None,
            ) {
                Ok(h) if !h.0.is_null() => {
                    hwnd = h;
                    used_ex = *ex_bits;
                    used_owner = *o;
                    hit_rung = i + 1;
                    if !errs.is_empty() {
                        crate::tsf::diag_note(&format!(
                            "ov: 阶梯第 {hit_rung} 档建成 ex=0x{ex_bits:X} owner=0x{:X}",
                            o.0 as usize
                        ));
                    }
                    break;
                }
                r => {
                    let code = r.err().map(|e| e.code().0).unwrap_or(0);
                    errs.push_str(&format!("0x{code:X};"));
                }
            }
        }
        if hwnd.0.is_null() {
            crate::tsf::diag_note(&format!(
                "ov: 建窗全档被拒 errs={errs} reg_err=0x{reg_err:X} owner=0x{:X}",
                owner.0 as usize
            ));
            return None;
        }
        // 沙盒缺失的穿透元素建后补挂（失败仅损失穿透，HTTRANSPARENT 兜底）
        unsafe {
            let want = used_ex | WS_EX_TRANSPARENT.0;
            if want != used_ex {
                let cur = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
                if cur != want {
                    // `as _`：x64=isize/i686=i32（windows-rs 按目标定型）
                    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, want as _);
                }
            }
        }
        let _ = used_owner;
        Some(OverlayWin {
            hwnd: hwnd.0 as isize,
            dc: 0,
            hbm: 0,
            bits: 0,
            dib_w: 0,
            dib_h: 0,
        })
    }

    unsafe fn destroy(&mut self) {
        if self.hwnd != 0 {
            let _ = KillTimer(HWND(self.hwnd as *mut _), OVERLAY_TIMER_ID);
            let _ = KillTimer(HWND(self.hwnd as *mut _), OVERLAY_HIDE_TIMER_ID);
            let _ = KillTimer(HWND(self.hwnd as *mut _), OVERLAY_RESYNC_TIMER_ID);
            let _ = DestroyWindow(HWND(self.hwnd as *mut _));
            self.hwnd = 0;
        }
        self.drop_dib();
    }

    fn drop_dib(&mut self) {
        unsafe {
            if self.hbm != 0 {
                let _ = DeleteObject(HGDIOBJ(self.hbm as *mut _));
                self.hbm = 0;
            }
            if self.dc != 0 {
                let _ = DeleteDC(HDC(self.dc as *mut _));
                self.dc = 0;
            }
        }
        self.bits = 0;
        self.dib_w = 0;
        self.dib_h = 0;
    }

    /// 推第 idx 帧:帧已是显示尺寸预乘 BGRA,只需落 DIB(+翻转镜像)+ ULW。
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
        // DIB(尺寸变化重建;top-down 32bpp premultiplied)
        if self.hbm == 0 || self.dib_w != w || self.dib_h != h {
            self.drop_dib();
            let bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: 0,
                    biSizeImage: (w * h * 4) as u32,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
            let Ok(hbm) = CreateDIBSection(
                HDC(std::ptr::null_mut()),
                &bi,
                DIB_RGB_COLORS,
                &mut bits,
                None,
                0,
            ) else {
                return false;
            };
            if hbm.is_invalid() || bits.is_null() {
                return false;
            }
            let dc = CreateCompatibleDC(HDC(std::ptr::null_mut()));
            if dc.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(hbm.0));
                return false;
            }
            let old = SelectObject(dc, HGDIOBJ(hbm.0));
            if old.is_invalid() {
                let _ = DeleteDC(dc);
                let _ = DeleteObject(HGDIOBJ(hbm.0));
                return false;
            }
            self.dc = dc.0 as isize;
            self.hbm = hbm.0 as isize;
            self.bits = bits as isize;
            self.dib_w = w;
            self.dib_h = h;
        }
        // 帧数据 → DIB(flip_h 时行内镜像)
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
                    std::ptr::copy_nonoverlapping(
                        src_row.as_ptr().add(sx * 4),
                        dst_row.add(x * 4),
                        4,
                    );
                }
            }
        } else {
            std::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
        }
        drop(frames);
        // ULW:像素级 alpha;整体不透明度走 SourceConstantAlpha
        let blend = BLENDFUNCTION {
            BlendOp: 0, // AC_SRC_OVER
            BlendFlags: 0,
            SourceConstantAlpha: cfg.alpha,
            AlphaFormat: 1, // AC_SRC_ALPHA(预乘)
        };
        let pt = POINT { x: 0, y: 0 };
        let sz = SIZE { cx: w, cy: h };
        let ok = UpdateLayeredWindow(
            HWND(self.hwnd as *mut _),
            None,
            None,
            Some(&sz as *const SIZE),
            HDC(self.dc as *mut _),
            Some(&pt as *const POINT),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );
        if ok.is_err() {
            crate::tsf::trace("ov: UpdateLayeredWindow FAIL");
        }
        ok.is_ok()
    }
}

/// 兄弟窗过程:隐藏扫尸 + 动图播放钟,其余全放行。
unsafe extern "system" fn overlay_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == crate::candwin2::WM_APP_HIDE_CAND {
        // 广播清场(hide_all_cand_windows 扫尸)也会命中本窗——但延时
        // 消失语义下不能立即收:转为 hide_for 同款「挂钟延时」。
        // (本消息此前无条件 SW_HIDE+杀双钟——用户实测「勾了延时消失、
        // 消失前切应用就永不消失」的根因正是切换路径的 hide_all 广播
        // 到本窗把延时钟杀掉、窗口却又被下面 sync/ULW 留在屏上)
        let mut list = OVERLAYS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = list
            .iter_mut()
            .find(|e| e.ov.as_ref().is_some_and(|o| o.hwnd == hwnd.0 as isize))
        {
            let delay = entry.cfg.as_ref().map(|c| c.hide_delay_ms).unwrap_or(0);
            if delay > 0 {
                // 保留播放钟(呼吸);记截止(补挂不延期)+挂钟,到点以
                // hide_at 截止为准收窗(见 hide 钟分支)
                arm_hide(entry, hwnd, delay);
                return LRESULT(0);
            }
            let _ = KillTimer(hwnd, OVERLAY_TIMER_ID);
            let _ = KillTimer(hwnd, OVERLAY_HIDE_TIMER_ID);
            let _ = KillTimer(hwnd, OVERLAY_RESYNC_TIMER_ID);
            let _ = ShowWindow(hwnd, SW_HIDE);
            entry.shown = false;
            entry.drawn = None;
            entry.timer_armed = false;
            entry.frame_idx = 0;
            entry.hide_at = None;
            entry.retry_until = None;
            // 隐藏=播放钟废弃:对表到现在,防下段显现带陈旧 play_start
            // (欠账帧持续补放=每键都「到点」走帧,播放越打越快)
            entry.play_start = std::time::Instant::now();
        }
        return LRESULT(0);
    }
    if msg == 0x0113 && wparam.0 as usize == OVERLAY_HIDE_TIMER_ID {
        // 延时消失到点:以 hide_at 截止为准(单一真相),未到点重挂钟、
        // 到点无条件收窗。候选窗若已重现,它的 sync_for 会清 hide_at
        // 并杀本钟——先查可见性会与跨线程 sync 竞态(用户实测「消失前
        // 切到别的应用就不消失」:切换路径候选窗藏着但 IsWindowVisible
        // 误报/竞态漏收,挂件永久滞留)。
        let now = std::time::Instant::now();
        let mut list = OVERLAYS.lock().unwrap_or_else(|e| e.into_inner());
        let Some(entry) = list
            .iter_mut()
            .find(|e| e.ov.as_ref().is_some_and(|o| o.hwnd == hwnd.0 as isize))
        else {
            let _ = KillTimer(hwnd, OVERLAY_HIDE_TIMER_ID);
            return LRESULT(0);
        };
        match entry.hide_at {
            Some(t) if t > now => {
                // 提前到点(多路补挂/丢拍自愈):按剩余时长重挂
                let _ = KillTimer(hwnd, OVERLAY_HIDE_TIMER_ID);
                let _ = SetTimer(
                    hwnd,
                    OVERLAY_HIDE_TIMER_ID,
                    (t - now).as_millis().min(u32::MAX as u128) as u32 + 1,
                    None,
                );
            }
            _ => {
                entry.hide_at = None;
                if let Some(ov) = entry.ov.as_mut() {
                    let h = HWND(ov.hwnd as *mut _);
                    let _ = KillTimer(h, OVERLAY_TIMER_ID);
                    let _ = KillTimer(h, OVERLAY_HIDE_TIMER_ID);
                    let _ = KillTimer(h, OVERLAY_RESYNC_TIMER_ID);
                    let _ = ShowWindow(h, SW_HIDE);
                }
                entry.shown = false;
                entry.timer_armed = false;
                entry.retry_until = None;
                entry.frame_idx = 0;
                entry.play_start = std::time::Instant::now();
            }
        }
        return LRESULT(0);
    }
    if msg == 0x0113 && wparam.0 as usize == OVERLAY_TIMER_ID {
        // WM_TIMER:动图下一帧
        advance_frame(hwnd);
        return LRESULT(0);
    }
    if msg == 0x0113 && wparam.0 as usize == OVERLAY_RESYNC_TIMER_ID {
        // 【补拍钟 2026-11】首帧解码就绪自查：重跑 sync 尾段（含可见
        // 性门/布局/上屏；仍未就绪会自行重臂，候选窗已藏则被门收掉）。
        resync_by_ov_hwnd(hwnd);
        return LRESULT(0);
    }
    if msg == 0x0084 {
        // WM_NCHITTEST→HTTRANSPARENT：穿透兜底（沙盒建窗期带
        // WS_EX_TRANSPARENT 被 0x5 拒、建后补挂也可能被拒时，
        // 鼠标事件转发给下方窗口；同线程候选窗区域等效穿透）
        return LRESULT(-1); // HTTRANSPARENT
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// 播放钟到点:按【时间基准】判定是否该走帧(与 tick_playback 同式、
/// 同一时钟 entry.frame_idx/play_start)——双路驱动谁先到点谁推进 1 帧，
/// 另一路再看时钟必「未到点」不推。旧实现无脑 +1 帧且不动 play_start，
/// 与 sync 路叠加=每键多走一帧,打字越密 GIF 越快(用户实测
/// 「再按键播放就加快」的根因)。
unsafe fn advance_frame(ov_hwnd: HWND) {
    let key = ov_hwnd.0 as isize;
    let mut list = OVERLAYS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(entry) = list
        .iter_mut()
        .find(|e| e.ov.as_ref().is_some_and(|o| o.hwnd == key))
    else {
        return;
    };
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
    let Some(ov) = entry.ov.as_mut() else { return };
    let delays = dec.delays.lock().unwrap_or_else(|e| e.into_inner());
    let now = std::time::Instant::now();
    let mut idx = entry.frame_idx % n;
    let mut start = entry.play_start;
    // 时钟欠账超一整轮→对表丢弃(隐藏期残留 play_start/宿主长卡顿):
    // 保留欠账会让此后每拍都「到点」,播放率退化成拍率(越按越快)
    let loop_ms: u64 = delays.iter().map(|d| (*d).max(1) as u64).sum();
    if now.saturating_duration_since(start).as_millis() as u64 > loop_ms {
        start = now;
        entry.play_start = start;
    }
    // 到点才走,一拍至多 1 帧(与 tick_playback 逐字同式)
    if now.duration_since(start).as_millis() as u32 >= delays[idx].max(1) {
        start += std::time::Duration::from_millis(delays[idx].max(1) as u64);
        idx = (idx + 1) % n;
        entry.frame_idx = idx;
        entry.play_start = start;
        if !ov.push_frame(&dec, idx, &cfg) {
            entry.timer_armed = false; // 本钟已随 WM_TIMER 消化,推帧失败别让 sync 以为还有钟
            return;
        }
    }
    // 续钟:按当前帧剩余时长(WM_TIMER 刚触发,本钟必处未 armed 态)
    let remain = delays[entry.frame_idx % n]
        .max(1)
        .saturating_sub(
            now.duration_since(entry.play_start).as_millis() as u32,
        )
        .max(10);
    let _ = SetTimer(ov_hwnd, OVERLAY_TIMER_ID, remain, None);
    entry.timer_armed = true;
}

// ── 挂钩入口(candwin2 调用;全部在候选窗线程执行)──

/// show/tick 尾部同步:读候选窗最终矩形 → 布局兄弟窗。
/// skin=本线程 Shared.skin(管道响应包装层)。
pub unsafe fn sync_for(cand: HWND, skin: &Value) {
    // 加词小窗(TL 线程)不挂贴图:词框候选无立绘语义
    if crate::tsf::addword_tl_thread() {
        return;
    }
    let cand_key = cand.0 as isize;
    if !IsWindow(cand).as_bool() {
        return;
    }
    let gen = OVERLAY_GEN.load(Ordering::Acquire);
    note_once("sync-enter", &format!("ov: sync_for 进入 gen={gen}"));
    let mut list = OVERLAYS.lock().unwrap_or_else(|e| e.into_inner());
    let pos = match list.iter().position(|e| e.cand == cand_key) {
        Some(p) => p,
        None => {
            list.push(OverlayEntry {
                cand: cand_key,
                ov: None,
                gen: u64::MAX, // 强制首次走代次解析
                disabled: false,
                cfg: None,
                decoded: None,
                drawn: None,
                frame_idx: 0,
                play_start: std::time::Instant::now(),
                timer_armed: false,
                shown: false,
                last_rect: None,
                hide_at: None,
                retry_until: None,
            });
            list.len() - 1
        }
    };
    {
        let entry = &mut list[pos];
        if entry.gen != gen {
            entry.gen = gen;
            entry.cfg = parse_cfg(skin);
            entry.decoded = None;
            entry.drawn = None;
            entry.disabled = false;
            entry.retry_until = None;
            note_once(
                "cfg-parse",
                &format!(
                    "ov: 代次解析 cfg={}（None=皮肤未载/无 overlay 节）skin_null={}",
                    entry.cfg.is_some(),
                    skin.is_null()
                ),
            );
        }
    }
    sync_tail_locked(cand, &mut list, pos);
}

/// 补拍钟处理器入口：按挂件 hwnd 反查条目重跑 sync 尾段（含可见性
/// 门/布局/上屏/z 序；未就绪会自行重臂补拍钟）。
unsafe fn resync_by_ov_hwnd(hwnd: HWND) {
    let mut list = OVERLAYS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(pos) = list
        .iter()
        .position(|e| e.ov.as_ref().is_some_and(|o| o.hwnd == hwnd.0 as isize))
    else {
        let _ = KillTimer(hwnd, OVERLAY_RESYNC_TIMER_ID);
        return;
    };
    let _ = KillTimer(hwnd, OVERLAY_RESYNC_TIMER_ID);
    let cand = list[pos].cand;
    if cand != 0 {
        let ch = HWND(cand as *mut _);
        if IsWindow(ch).as_bool() {
            sync_tail_locked(ch, &mut list, pos);
        }
    }
}

/// sync 尾段（持有 OVERLAYS 锁调用）：候选窗可见性门 → 代次配置应用
/// → 建窗 → 解码（预解码缓存优先）→ 布局上屏 → 播放节拍 → z 序。
/// 从 sync_for 与补拍钟两条路进入，单一实现防口径漂移。
unsafe fn sync_tail_locked(cand: HWND, list: &mut Vec<OverlayEntry>, pos: usize) {
    let cand_key = cand.0 as isize;
    let entry = &mut list[pos];
    // 候选窗不可见 → 兄弟窗必藏(自愈所有漏网隐藏路径);
    // 延时消失期(hide_at 有值)例外:挂件原地呼吸,由 hide 钟到点收窗
    if !IsWindowVisible(cand).as_bool() {
        if let Some(ov) = entry.ov.as_ref() {
            if entry.hide_at.is_none() {
                let _ = KillTimer(HWND(ov.hwnd as *mut _), OVERLAY_TIMER_ID);
                let _ = KillTimer(HWND(ov.hwnd as *mut _), OVERLAY_RESYNC_TIMER_ID);
                let _ = ShowWindow(HWND(ov.hwnd as *mut _), SW_HIDE);
                entry.shown = false;
                entry.timer_armed = false;
                entry.hide_at = None;
                entry.retry_until = None;
                // 隐藏期残留 play_start 是时钟欠账源(下段显现后每拍都
                // 「到点」=播放加速)——隐藏即对表到现在
                entry.play_start = std::time::Instant::now();
                entry.frame_idx = 0;
            }
        } else {
            entry.shown = false;
            entry.timer_armed = false;
        }
        return;
    }
    // 候选窗重现 → 撤销延时收窗:清 hide_at(handler 见 None 即收窗,
    // 故必须连钟一起杀;万一杀钟与在途 WM_TIMER 竞态漏杀,handler 收
    // 一次窗后下一拍 sync 经 !shown 重新显示——瞬时闪烁,无滞留)。
    if entry.hide_at.is_some() {
        entry.hide_at = None;
        if let Some(ov) = entry.ov.as_ref() {
            let _ = KillTimer(HWND(ov.hwnd as *mut _), OVERLAY_HIDE_TIMER_ID);
        }
    }
    let Some(cfg) = entry.cfg.clone() else {
        if let Some(ov) = entry.ov.as_mut() {
            let _ = KillTimer(HWND(ov.hwnd as *mut _), OVERLAY_TIMER_ID);
            let _ = KillTimer(HWND(ov.hwnd as *mut _), OVERLAY_RESYNC_TIMER_ID);
            let _ = ShowWindow(HWND(ov.hwnd as *mut _), SW_HIDE);
        }
        entry.shown = false;
        entry.timer_armed = false;
        entry.retry_until = None;
        return;
    };
    if entry.disabled {
        return;
    }
    // 建窗(惰性;失败本轮代次禁用)——带 cand：owned 模式下挂件同 owner
    // 挂靠（避 DWM cloak；见 create_for 注释）
    if entry.ov.is_none() {
        match OverlayWin::create_for(cand) {
            Some(w) => entry.ov = Some(w),
            None => {
                entry.disabled = true;
                crate::tsf::trace("ov: 兄弟窗创建失败,本代次禁用");
                note_once("create-fail", "ov: 兄弟窗创建失败（沙盒建窗被拒?）→本代次禁用");
                return;
            }
        }
    }
    // 目标显示高度/处理参数(DPI/高度/圆角/羽化等变了 → 重解码重处理)
    let target_h = cfg.base_height * dpi_scale(cand);
    let need_decode = match entry.decoded.as_ref() {
        None => true,
        Some(d) => {
            d.built_for.load(Ordering::Acquire) != target_h.round() as u32
                || d.proc_key.load(Ordering::Acquire) != cfg.proc_key
        }
    };
    if need_decode {
        // 【预解码缓存 2026-11】皮肤到位时已起跑的解码直接采用（首段
        // 即显——不等首键 sync 才开解）；未命中/失配才现场开解。
        // 【处理指纹 2026-10-07】命中条件加 proc_key——纯参数改动不复用
        // 旧参数的成品帧。
        let prewarmed = {
            let pw = PREWARM.lock().unwrap_or_else(|e| e.into_inner());
            pw.as_ref().and_then(|(k, b, pk, d)| {
                (d.ready_first.load(Ordering::Acquire)
                    && *k == cfg.img_key
                    && *b == target_h.round() as u32
                    && *pk == cfg.proc_key)
                    .then(|| d.clone())
            })
        };
        entry.decoded = Some(match prewarmed {
            Some(d) => d,
            None => start_decode(cfg.clone(), target_h),
        });
        entry.drawn = None;
        entry.frame_idx = 0;
        entry.play_start = std::time::Instant::now();
        entry.timer_armed = false;
    }
    let Some(dec) = entry
        .decoded
        .as_ref()
        .filter(|d| d.ready_first.load(Ordering::Acquire))
    else {
        // 【补拍钟 2026-11】首帧未就绪不再干等下一次 sync（下一段/下
        // 一键才有机会=「导入后要点一两下才出」「一搜就走赶不上」的
        // 病根）：30ms 短钟自查，就绪即上屏；3s 封顶防解码失败空转。
        if let Some(ov) = entry.ov.as_ref() {
            let oh = HWND(ov.hwnd as *mut _);
            let now = std::time::Instant::now();
            let until = *entry
                .retry_until
                .get_or_insert(now + std::time::Duration::from_secs(3));
            if now < until {
                note_once("not-ready", "ov: 首帧未就绪→补拍钟自查");
                let _ = SetTimer(oh, OVERLAY_RESYNC_TIMER_ID, 30, None);
            } else {
                let _ = KillTimer(oh, OVERLAY_RESYNC_TIMER_ID);
            }
        }
        return;
    };
    let Some(ov) = entry.ov.as_mut() else { return };
    let ov_h = HWND(ov.hwnd as *mut _);

    // ── 布局:候选窗窗口矩形 → 内容盒(内缩阴影物理边距)→ 贴图矩形 ──
    let mut wr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    if GetWindowRect(cand, &mut wr).is_err() {
        return;
    }
    let inset = crate::candwin2::shadow_inset_for(cand);
    let (cl, ct, cr, cb) = (
        wr.left + inset,
        wr.top + inset,
        wr.right - inset,
        wr.bottom - inset,
    );
    let (cl, ct, cr, cb) = if cr - cl > 0 && cb - ct > 0 {
        (cl, ct, cr, cb)
    } else {
        (wr.left, wr.top, wr.right, wr.bottom)
    };
    let cw = dec.w.load(Ordering::Acquire).max(1) as i32;
    let ch = dec.h.load(Ordering::Acquire).max(1) as i32;
    let x = if cfg.side_left {
        cl - cfg.gap - cw + cfg.offset_x
    } else {
        cr + cfg.gap + cfg.offset_x
    };
    let y = match cfg.v_align {
        VAlign::Center => (ct + cb) / 2 - ch / 2 + cfg.offset_y,
        VAlign::Top => ct + cfg.offset_y,
        VAlign::Bottom => cb - ch + cfg.offset_y,
    };

    // 首帧(或翻转/尺寸变化)落屏:播放时钟重置
    let key = (cw, ch, cfg.flip_h);
    if entry.drawn != Some(key) {
        entry.frame_idx = 0;
        entry.play_start = std::time::Instant::now();
        if !ov.push_frame(&dec, 0, &cfg) {
            return;
        }
        entry.drawn = Some(key);
        entry.last_rect = None; // 强制 SWP 落位/显示
    }
    // 位置/尺寸/显示
    if entry.last_rect != Some((x, y, cw, ch)) || !entry.shown {
        let _ = SetWindowPos(
            ov_h,
            HWND_TOPMOST,
            x,
            y,
            cw,
            ch,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        entry.last_rect = Some((x, y, cw, ch));
        entry.shown = true;
    }
    // 上屏成功：补拍钟退役（未臂时 KillTimer 无害）
    let _ = KillTimer(ov_h, OVERLAY_RESYNC_TIMER_ID);
    entry.retry_until = None;
    note_once(
        "shown",
        &format!("ov: 上屏 {cw}x{ch} at ({x},{y}) cand_visible={}", {
            let v = IsWindowVisible(cand).as_bool();
            v
        }),
    );
    // 动图节拍:时间基准推进,每拍至多 1 帧(宿主线程卡顿后逐帧补放,
    // 绝不跳帧);WM_TIMER 与 sync 双路驱动同一时钟
    tick_playback(
        ov,
        &mut entry.frame_idx,
        &mut entry.play_start,
        &mut entry.timer_armed,
        &dec,
        &cfg,
        ov_h,
    );
    // z 序:below=贴图紧贴候选窗正下(重叠也被压,候选文字永不被盖,
    // 吸取 v7 特效窗教训);above=反向插序(同线程确定性操作)。
    if cfg.above {
        if let Ok(prev) = GetWindow(cand, GW_HWNDPREV) {
            if prev.0 as isize != ov.hwnd {
                let _ = SetWindowPos(
                    cand,
                    ov_h,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }
    } else if let Ok(prev) = GetWindow(ov_h, GW_HWNDPREV) {
        if prev.0 as isize != cand_key {
            let _ = SetWindowPos(
                ov_h,
                cand,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }
}

/// 动图节拍(时间基准):到点该显示哪帧由时钟判定,每拍至多推进 1 帧。
/// 续钟只在未 armed 时发(动效期 sync 不重置倒计时=冻结构的根)。
unsafe fn tick_playback(
    ov: &mut OverlayWin,
    frame_idx: &mut usize,
    play_start: &mut std::time::Instant,
    timer_armed: &mut bool,
    dec: &DecodeState,
    cfg: &OverlayCfg,
    ov_h: HWND,
) {
    if !dec.animated.load(Ordering::Acquire) {
        if *timer_armed {
            let _ = KillTimer(ov_h, OVERLAY_TIMER_ID);
            *timer_armed = false;
        }
        return;
    }
    if !dec.ready.load(Ordering::Acquire) {
        // 帧序列未解码完:保持首帧;续短钟,解完下一拍自动开播
        if !*timer_armed {
            let _ = SetTimer(ov_h, OVERLAY_TIMER_ID, 50, None);
            *timer_armed = true;
        }
        return;
    }
    let n = dec.frames.lock().unwrap_or_else(|e| e.into_inner()).len();
    if n <= 1 {
        return;
    }
    let delays = dec.delays.lock().unwrap_or_else(|e| e.into_inner());
    let now = std::time::Instant::now();
    let mut idx = *frame_idx % n;
    let mut start = *play_start;
    let mut advanced = 0usize;
    // 时钟欠账超一整轮→对表丢弃(与 advance_frame 同款:隐藏期残留
    // play_start/宿主长卡顿;不丢弃=此后每拍都「到点」走 1 帧,
    // 播放率退化成拍率=打字越密 GIF 越快,观感「首段正常之后加速」)
    let loop_ms: u64 = delays.iter().map(|d| (*d).max(1) as u64).sum();
    if now.saturating_duration_since(start).as_millis() as u64 > loop_ms {
        start = now;
        *play_start = start;
    }
    // 每拍至多推进 1 帧:宿主线程卡顿后逐帧补放,绝不跳帧——跳帧会让
    // 观感帧在卡顿窗口里被整段越过(Code 实测「有时看不到」的真根因)
    while now.duration_since(start).as_millis() as u32 >= delays[idx].max(1) && advanced < 1 {
        start += std::time::Duration::from_millis(delays[idx].max(1) as u64);
        idx = (idx + 1) % n;
        advanced += 1;
    }
    if advanced > 0 {
        *frame_idx = idx;
        *play_start = start;
        ov.push_frame(dec, idx, cfg);
    }
    if !*timer_armed {
        let remain = delays[idx]
            .max(1)
            .saturating_sub(now.duration_since(start).as_millis() as u32)
            .max(10);
        let _ = SetTimer(ov_h, OVERLAY_TIMER_ID, remain, None);
        *timer_armed = true;
    }
}

/// 延时收窗统一臂钟:hide_at 截止时刻是**唯一真相**,钟只是叫醒手段——
/// 已有更早截止不覆盖(多路隐藏信号都来臂钟,取最早=延时语义不因
/// 广播风暴被拉长);钟挂到「最早截止+150% 兜底」(WM_TIMER 最小 ~
/// 15.6ms 分辨率、丢拍后无再臂——真实到点由 handler 对表 hide_at
/// 重挂收窗,自愈丢拍)。
unsafe fn arm_hide(entry: &mut OverlayEntry, ov_hwnd: HWND, delay: u32) {
    let now = std::time::Instant::now();
    let deadline = match entry.hide_at {
        Some(t) if t <= now + std::time::Duration::from_millis(delay as u64) => t,
        _ => now + std::time::Duration::from_millis(delay as u64),
    };
    entry.hide_at = Some(deadline);
    let span = (deadline - now).as_millis() as u32;
    let _ = KillTimer(ov_hwnd, OVERLAY_HIDE_TIMER_ID);
    let _ = SetTimer(ov_hwnd, OVERLAY_HIDE_TIMER_ID, span.saturating_add(span / 2).max(16), None);
}

/// 隐藏同步(WM_APP_HIDE_CAND 处理器内调用)。
/// hide_delay_ms=0(默认):候选窗一收立即收,干净利落;
/// >0:挂件原地停留该时长——期间候选窗重现(sync_for)则撤销收窗,
/// 到点由 hide 钟到点收窗。播放钟继续走(呼吸不停)。
pub unsafe fn hide_for(cand: HWND) {
    let cand_key = cand.0 as isize;
    let mut list = OVERLAYS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = list.iter_mut().find(|e| e.cand == cand_key) {
        let delay = entry.cfg.as_ref().map(|c| c.hide_delay_ms).unwrap_or(0);
        if delay > 0 && entry.ov.is_some() {
            // 动图继续播(呼吸),静图无需钟;延时收窗由 hide 钟对表 hide_at
            if let Some(ov) = entry.ov.as_ref() {
                arm_hide(entry, HWND(ov.hwnd as *mut _), delay);
            }
            return;
        }
        if let Some(ov) = entry.ov.as_mut() {
            let _ = KillTimer(HWND(ov.hwnd as *mut _), OVERLAY_TIMER_ID);
            let _ = KillTimer(HWND(ov.hwnd as *mut _), OVERLAY_HIDE_TIMER_ID);
            let _ = KillTimer(HWND(ov.hwnd as *mut _), OVERLAY_RESYNC_TIMER_ID);
            let _ = ShowWindow(HWND(ov.hwnd as *mut _), SW_HIDE);
        }
        entry.shown = false;
        entry.timer_armed = false;
        entry.retry_until = None;
        entry.frame_idx = 0;
        entry.hide_at = None;
        // 与广播/延时收窗臂同款:隐藏即对表 play_start,防下段显现
        // 带陈旧时钟欠账持续补放(播放越打越快)
        entry.play_start = std::time::Instant::now();
    }
}

/// 候选窗销毁 → 兄弟窗同线程随葬(WM_NCDESTROY 处理器内调用)。
pub unsafe fn destroy_for(cand: HWND) {
    let cand_key = cand.0 as isize;
    let mut list = OVERLAYS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(pos) = list.iter().position(|e| e.cand == cand_key) {
        let mut entry = list.remove(pos);
        if let Some(ov) = entry.ov.as_mut() {
            ov.destroy();
        }
    }
}

/// 与 show() 同源 DPI 口径(含 HUFU_FAKE_DPI 测试旋钮)。
unsafe fn dpi_scale(cand: HWND) -> f32 {
    let dpi = GetDpiForWindow(cand).max(96) as f32 / 96.0;
    std::env::var("HUFU_FAKE_DPI")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|v| *v >= 96.0 && *v <= 480.0)
        .map(|v| v / 96.0)
        .unwrap_or(dpi)
}
