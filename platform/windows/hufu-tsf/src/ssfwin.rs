//! ssfwin —— .ssf 位图皮肤运行时（DLL 侧，自包含模块）。
//!
//! 职责（对照 tigirl 语义逐行移植，引用格式 `虎娘:文件:行号`）：
//! - **资源加载**：从 server 解包缓存目录（`皮肤缓存\{id}\`）按「归一小写相对
//!   名」读图 → image crate 解码（静态 PNG/GIF/WebP/JPEG + APNG/GIF 动图，
//!   解码骨架复用 overlaywin.rs:328-433 的已验证口径）→ 统一为逐帧全画布
//!   **premultiplied BGRA** 帧流（预乘换算同 overlaywin.rs:557-561）。
//! - **进程级缓存**：key=(cache_dir, 图片名)、值校验 fp 指纹，变化即失效重建
//!   （虎娘「路径+大小+mtime+刷新戳」的指纹化简版），LRU 上限 8 条（虎娘 2 条，
//!   DLL 侧多窗口共用放宽）。超预算/解码失败一律 Err，上层降级回 JSON 矢量
//!   皮肤，绝不让皮肤错误弄坏输入（设计方案 §4.6 降级矩阵）。
//! - **九宫格切片与绘制**：`slice_axis` 逐行移植虎娘 sliceAxis（SsfSkin.cpp:
//!   80-89），`draw_nineslice` 叉乘 ≤9 块 DrawBitmap + 目标四边 DPI snap 到
//!   物理像素防接缝（CandidateRendererSkin.cpp:70-84）。
//! - **APNG 帧时钟**：`frame_at` 移植虎娘 animationFrame（SsfAnimation.cpp:
//!   46-50）——每帧最小 20ms、plays 播完停末帧、按 elapsed 环形取帧。
//! - **设备位图缓存**：帧号/尺寸没变不重建（虎娘 skinBitmapFrame_ 机制，
//!   CandidateRendererSkin.cpp:62-69）。
//! - **皮肤 JSON 描述**：`SsfSkinSpec` 解析 `{skin:{kind:"ssf",cache_dir,fp,
//!   display,layouts}}`（双形态口径同 candwin2.rs:1399-1423 的 color_f/
//!   layout_f——`/skin/...` 包装层或顶层直挂都认）。
//!
//! 渲染管线接入点（candwin2.rs:4280 背景分支替换 FillRoundedRectangle）、
//! APNG 常驻时钟（SSF_TIMER_ID，不挂 anim_boost——400ms 自杀截止与循环播放
//! 冲突）等集成细节见 `E:\DSH-KF\.refs\_dll接入方案.md`。
//!
//! 安全预算（照虎娘全套，超限 Err）：
//! | 项                       | 上限                          |
//! |--------------------------|-------------------------------|
//! | 单图边长                 | ≤8192×8192                    |
//! | 每帧像素                 | ≤16,000,000                   |
//! | 帧数                     | ≤256                          |
//! | 全图解码内存（帧流总量） | ≤64MB                         |
//! | 单文件输入               | ≤16MiB（虎娘 4MiB，虎符放宽） |
//! PNG CRC/截断由 png 解码器校验，失败即 Err。

use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::Value;
use windows::core::Interface;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;

// ── 安全预算常量 ──

/// 单图边长上限（虎娘口径：≤8192×8192）。
const MAX_DIM: u32 = 8192;
/// 单帧像素上限（虎娘口径 ≤16M 像素/帧）。
const MAX_PIXELS: u64 = 16_000_000;
/// 帧数上限（虎娘口径 ≤256 帧）。
const MAX_FRAMES: usize = 256;
/// 全图解码内存预算：帧流总量 ≤64MB（虎娘口径，跨帧共享）。
const MAX_DECODE_BYTES: u64 = 64 * 1024 * 1024;
/// 单文件输入上限（虎娘 4MiB 运行时上限，设计方案 §1.1 放宽到 16MiB）。
const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;
/// 解码缓存条目上限（LRU；虎娘 ≤2 条，DLL 侧多窗口共用放宽到 8）。
const CACHE_LIMIT: usize = 8;
/// 动图每帧延迟下限（虎娘口径：最小 20ms）。
const MIN_FRAME_DELAY_MS: u32 = 20;

/// .ssf 皮肤 APNG 帧推进常驻定时器 id（wndproc WM_TIMER 消费；独立于
/// FADE_TIMER_ID/anim_boost——后者 400ms 自杀截止与循环播放冲突，见
/// candwin2.rs:6478-6566 注释与 _dll接入方案.md §4）。0x4945='IE'。
pub const SSF_TIMER_ID: usize = 0x4945;

// ============================================================================
// SsfBitmap：解码后的位图帧流
// ============================================================================

/// 一张皮肤图的解码产物：逐帧全画布 premultiplied BGRA（u8×4）。
/// 静态图=单帧、delay 0；动图帧数与 delays_ms 一一对应。
/// 帧内存总量受 64MB 预算约束（解码时强制）。
pub struct SsfBitmap {
    pub width: u32,
    pub height: u32,
    /// 播放次数：0=无限循环（APNG acTL num_plays / GIF Netscape loop）。
    pub plays: u32,
    /// 每帧 `width*height*4` 字节，BGRA 预乘、逐像素行主序。
    pub frames: Vec<Vec<u8>>,
    /// 每帧时长 ms（已钳 ≥20；静态图恒 0）。
    pub delays_ms: Vec<u32>,
}

impl SsfBitmap {
    pub fn is_animated(&self) -> bool {
        self.frames.len() > 1 && self.delays_ms.iter().any(|&d| d > 0)
    }
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }
}

/// RGBA 直排 → premultiplied BGRA（同 overlaywin.rs:557-561 的换算式：
/// `(c*a+127)/255`，四舍五入近真，通道顺序 R,G,B,A → B,G,R,A）。
fn rgba_to_premul_bgra(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    // n 取契约值与实参的较小者：解码器产物尺寸异常时不越界（防御，
    // 正常路径 w*h*4 == rgba.len()）。
    let n = ((w as usize) * (h as usize)).min(rgba.len() / 4);
    let mut out = vec![0u8; n * 4];
    for i in 0..n {
        let s = i * 4;
        let a = rgba[s + 3] as u32;
        out[s] = ((rgba[s + 2] as u32 * a + 127) / 255) as u8;
        out[s + 1] = ((rgba[s + 1] as u32 * a + 127) / 255) as u8;
        out[s + 2] = ((rgba[s] as u32 * a + 127) / 255) as u8;
        out[s + 3] = a as u8;
    }
    out
}

/// 动图帧 → (RgbaImage, 延迟 ms)（同 overlaywin.rs:435-439 的 frame_pair，
/// 除法改为四舍五入——虎娘 delayMs 同款 `(num*1000+den/2)/den` 取整）。
fn frame_pair(f: image::Frame) -> (image::RgbaImage, u32) {
    let (num, den) = f.delay().numer_denom_ms();
    let ms = if den == 0 { 100 } else { (num + den / 2) / den };
    (f.into_buffer(), ms.max(1))
}

/// 解码一张皮肤图（内存字节 → SsfBitmap）。
///
/// 解码骨架对齐 overlaywin.rs:353-433（已验证的 DLL 侧口径）：GIF 走
/// GifDecoder（image crate 内部完成 dispose/blend 合成，产物为全画布帧，
/// image-0.25.10 gif.rs:380-422）；PNG 先探 APNG（is_apng），动图走
/// apng() 帧流（mix_next_frame 已合成全画布，png.rs:525-553），静态与其
/// 余格式（WebP/JPEG）走 load_from_memory 单帧。plays 取自 LoopCount
/// （APNG=acTL num_plays / GIF=Netscape loop，Infinite→0）。
fn decode_bitmap(data: &[u8]) -> Result<SsfBitmap, String> {
    use image::AnimationDecoder;

    const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let is_gif = data.len() > 3 && &data[0..3] == b"GIF";
    let is_png = data.len() >= 8 && data[0..8] == PNG_MAGIC;

    // plays 提取：LoopCount::Infinite→0（无限循环，虎娘 plays=0 语义）。
    let plays_of = |lc: image::metadata::LoopCount| match lc {
        image::metadata::LoopCount::Infinite => 0u32,
        image::metadata::LoopCount::Finite(n) => n.get(),
    };

    let animated;
    let plays;
    let iter: Box<dyn Iterator<Item = (image::RgbaImage, u32)>> = if is_gif {
        let dec = image::codecs::gif::GifDecoder::new(Cursor::new(data))
            .map_err(|e| format!("GIF 解码失败: {e}"))?;
        plays = plays_of(dec.loop_count());
        animated = true;
        Box::new(
            dec.into_frames()
                .filter_map(|f| f.ok())
                .map(frame_pair),
        )
    } else if is_png {
        let dec = image::codecs::png::PngDecoder::new(Cursor::new(data))
            .map_err(|e| format!("PNG 解码失败: {e}"))?;
        if dec.is_apng().unwrap_or(false) {
            let apng = dec
                .apng()
                .map_err(|e| format!("APNG 打开失败: {e}"))?;
            plays = plays_of(apng.loop_count());
            animated = true;
            Box::new(
                apng.into_frames()
                    .filter_map(|f| f.ok())
                    .map(frame_pair),
            )
        } else {
            plays = 0;
            animated = false;
            let img = image::load_from_memory(data)
                .map_err(|e| format!("静态图解码失败: {e}"))?
                .to_rgba8();
            Box::new(std::iter::once((img, 0u32)))
        }
    } else {
        // WebP/JPEG（及其余 image crate 可吃格式）：静态单帧
        plays = 0;
        animated = false;
        let img = image::load_from_memory(data)
            .map_err(|e| format!("静态图解码失败: {e}"))?
            .to_rgba8();
        Box::new(std::iter::once((img, 0u32)))
    };

    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut delays_ms: Vec<u32> = Vec::new();
    let mut dims @ (w0, h0) = (0u32, 0u32);
    for (img, ms) in iter {
        if frames.is_empty() {
            let (w, h) = (img.width(), img.height());
            if w == 0 || h == 0 {
                return Err("图片尺寸为 0".to_string());
            }
            if w > MAX_DIM || h > MAX_DIM {
                return Err(format!("图片超限 {w}×{h}（>8192×8192）"));
            }
            if w as u64 * h as u64 > MAX_PIXELS {
                return Err(format!("单帧像素超限 {w}×{h}（>16M）"));
            }
            dims = (w, h);
        } else if (img.width(), img.height()) != dims {
            // image 的 APNG/GIF 合成保证全画布帧；不一致=解码器异常，拒收
            return Err(format!(
                "帧尺寸不一致 {}×{}（首帧 {w0}×{h0}）",
                img.width(),
                img.height()
            ));
        }
        if frames.len() >= MAX_FRAMES {
            return Err(format!("帧数超限（>{MAX_FRAMES}）"));
        }
        let per_frame = dims.0 as u64 * dims.1 as u64 * 4;
        if (frames.len() as u64 + 1) * per_frame > MAX_DECODE_BYTES {
            return Err("解码内存超 64MB 预算".to_string());
        }
        // 帧延迟：静态 0；动图钳 [20ms, 60s]（虎娘下限 20ms；上限防手改
        // 皮肤写 0x7FFFFFFF 把帧钟拖死）
        let delay = if animated {
            ms.clamp(MIN_FRAME_DELAY_MS, 60_000)
        } else {
            0
        };
        frames.push(rgba_to_premul_bgra(dims.0, dims.1, img.as_raw()));
        delays_ms.push(delay);
    }
    if frames.is_empty() {
        return Err("没有可解码帧".to_string());
    }
    Ok(SsfBitmap {
        width: dims.0,
        height: dims.1,
        plays,
        frames,
        delays_ms,
    })
}

// ============================================================================
// 进程级解码缓存：(cache_dir, 归一图名) → (fp, Arc<SsfBitmap>)，LRU≤8
// ============================================================================

struct CacheEntry {
    /// 皮肤指纹（源 .ssf 的 size+mtime+sha256，server 下发）。命中时校验，
    /// 变化即失效重建——虎娘「刷新戳进缓存 key」的等价实现（同尺寸同
    /// mtime 重存也能换装）。
    fp: String,
    bitmap: Arc<SsfBitmap>,
    last_used: u64,
}

// HashMap::new 非 const fn（工具链限制），静态里装 Option 惰性建表
//（同 candwin2 MENU_FONT 静态模式）。
static BITMAP_CACHE: Mutex<Option<HashMap<(PathBuf, String), CacheEntry>>> = Mutex::new(None);
static CACHE_TICK: AtomicU64 = AtomicU64::new(0);

/// 持锁访问缓存表（首访问建表）。
fn with_cache<R>(f: impl FnOnce(&mut HashMap<(PathBuf, String), CacheEntry>) -> R) -> R {
    let mut g = BITMAP_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(HashMap::new))
}

fn cache_tick() -> u64 {
    CACHE_TICK.fetch_add(1, Ordering::Relaxed)
}

/// 图片名归一：小写相对名 + DLL 侧二次消毒（server 解包落盘已消毒一遍，
/// 这里是第二道防线——设计方案 §六.11：写盘白名单之外读盘也要验）。
/// 拒绝：空/超长(>128)/路径分隔符/盘符/通配符/控制字符/`..`。
fn normalize_image_name(name: &str) -> Result<String, String> {
    let n = name.trim().to_lowercase();
    if n.is_empty() || n.len() > 128 {
        return Err(format!("图片名不合法: {name:?}"));
    }
    if n.contains("..")
        || n.chars()
            .any(|c| matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control())
    {
        return Err(format!("图片名不合法: {name:?}"));
    }
    Ok(n)
}

/// 加载（并缓存）一张皮肤图。失败一律 Err(String)——上层降级回 JSON 矢量
/// 皮肤或跳过该 Scheme 变体（降级矩阵见设计方案 §4.6）。
pub fn load_bitmap(cache_dir: &Path, fp: &str, image: &str) -> Result<Arc<SsfBitmap>, String> {
    let name = normalize_image_name(image)?;
    let key = (cache_dir.to_path_buf(), name.clone());
    // ① 快路径：命中且 fp 未变
    {
        let hit = with_cache(|cache| {
            if let Some(e) = cache.get_mut(&key) {
                if e.fp == fp {
                    e.last_used = cache_tick();
                    return Some(e.bitmap.clone());
                }
                // fp 变化 → 失效（虎娘皮肤刷新戳同款语义）
                cache.remove(&key);
            }
            None
        });
        if let Some(bmp) = hit {
            return Ok(bmp);
        }
    }
    // ② 解码（锁外读盘+解码，不阻塞其它窗口取图）
    let path = cache_dir.join(&name);
    let data =
        std::fs::read(&path).map_err(|e| format!("读图失败 {}: {e}", path.display()))?;
    if data.len() > MAX_INPUT_BYTES {
        return Err(format!("文件超限 {} bytes（>16MiB）", data.len()));
    }
    let bmp = Arc::new(decode_bitmap(&data)?);
    // ③ 回锁插入 + LRU 收缩（期间他线程先插同 key 则覆盖，二者等价：
    // 同 path 同 fp 的解码结果确定性一致）
    with_cache(|cache| {
        cache.insert(
            key,
            CacheEntry {
                fp: fp.to_string(),
                bitmap: bmp.clone(),
                last_used: cache_tick(),
            },
        );
        while cache.len() > CACHE_LIMIT {
            let victim = cache
                .iter()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(k, _)| k.clone());
            match victim {
                Some(k) => {
                    cache.remove(&k);
                }
                None => break,
            }
        }
    });
    Ok(bmp)
}

// ============================================================================
// 九宫格切片（逐行移植虎娘 SsfSkin.cpp:80-89 sliceAxis）
// ============================================================================

/// 一轴切片：源区间 [source, source+source_length) → 目标区间
/// [destination, destination+destination_length)。全 f32。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SsfAxisSlice {
    pub source: f32,
    pub source_length: f32,
    pub destination: f32,
    pub destination_length: f32,
}

/// 单轴九宫格切片（虎娘 sliceAxis 语义逐行移植）：
/// - 源 0 / 目标非有限或 ≤0 → 空切片（整轴不画）；
/// - 刀口和 ≥ 源宽 → 按 `(源宽-1)/刀口和` 比例收缩刀口（真机立绘压扁
///   根因：别整图硬缩）；
/// - 目标 < 刀口和 → 再等比收缩目标侧刀口（`destination/(before+after)`）；
/// - tile=true（mode 1 平铺）且 中段源宽>0 且 `可用宽/中段 ≤64` → 逐块
///   平铺（≤64 块绘制预算）；否则中段整体拉伸；
/// - 首尾块 source_length/destination_length ≤0 的丢弃（add 谓词）。
pub fn slice_axis(source: u32, destination: f32, tile: bool, before: i32, after: i32) -> Vec<SsfAxisSlice> {
    slice_axis_scaled(source, destination, tile, before, after, 1.0)
}

/// 【SSF 双域切片 2026-10-15】`scale`=源像素→目标逻辑px 换算系数
///（`user_pt/display.font_size`）。源域 before/after 为皮肤原始像素；
/// 目标域固定段长 = 源段×scale（等比缩放保真），中段吃掉剩余目标宽
///（拉伸带）。平铺模式每块目标长=源段×scale。修复实锤：此前调用方
/// 把刀口×scale 后与原始源宽混算 → 窗口画成原图左上角 1:1 局部+周边
/// 压扁条（四角保真校验 765/765 满分差）。
pub fn slice_axis_scaled(
    source: u32,
    destination: f32,
    tile: bool,
    before: i32,
    after: i32,
    scale: f32,
) -> Vec<SsfAxisSlice> {
    let mut out = Vec::new();
    if source == 0 || !destination.is_finite() || destination <= 0.0 || !(scale > 0.0) {
        return out;
    }
    let size = source as f32;
    let mut before = before.max(0) as f32;
    let mut after = after.max(0) as f32;
    // 刀口和 ≥ 源宽：源侧按比例收缩（留 1px 中段）——同旧语义
    if before + after >= size {
        let factor = (size - 1.0) / (before + after).max(1.0);
        before *= factor;
        after *= factor;
    }
    // 目标域固定段 = 源段 × scale；目标过小再等比收缩（源不变）
    let mut d_before = before * scale;
    let mut d_after = after * scale;
    if d_before + d_after > destination {
        let factor = destination / (d_before + d_after).max(1e-6);
        d_before *= factor;
        d_after *= factor;
    }
    let add = |s: f32, n: f32, d: f32, len: f32, out: &mut Vec<SsfAxisSlice>| {
        if n > 0.0 && len > 0.0 {
            out.push(SsfAxisSlice {
                source: s,
                source_length: n,
                destination: d,
                destination_length: len,
            });
        }
    };
    // 首段：源 [0,before] → 目标 [0,d_before]（固定区等比缩放）
    add(0.0, before, 0.0, d_before, &mut out);
    let middle = size - before - after;
    let available = (destination - d_before - d_after).max(0.0);
    if tile && middle > 0.0 && available / (middle * scale) <= 64.0 {
        // 平铺：中段按 scale 后尺寸逐块铺满可用宽（≤64 块），末块按剩余截短
        let mut d = 0.0f32;
        while d < available {
            let len = (middle * scale).min(available - d);
            if len <= 0.0 {
                break;
            }
            add(before, len / scale, d_before + d, len, &mut out);
            d += len;
        }
    } else {
        // 中段整体拉伸：源 [before,before+middle] → 目标剩余空间
        add(before, middle, d_before, available, &mut out);
    }
    // 尾段：源 [size-after,size] → 目标 [destination-d_after,destination]
    add(size - after, after, destination - d_after, d_after, &mut out);
    out
}

/// 九宫格绘制（虎娘 drawSkinBackground 逐行移植，CandidateRendererSkin.cpp:
/// 70-84）：两轴 slice_axis 叉乘 ≤9 块（平铺模式更多，仍受 64 块/轴预算）
/// DrawBitmap。
///
/// **目标四边按 DPI snap 到物理像素**：`round(v*dpi/96)*96/dpi`。虎娘注释
/// 原话教训：相邻切片必须共享物理像素边缘，分数边缘在透明背景上各自
/// 混合 → 接缝/半透明素材压暗。相邻块 destination 边缘天然相等（切片
/// 逐块累加），snap 后仍共享。
///
/// 坐标系：dst_w/dst_h 与 FillRoundedRectangle 同一逻辑坐标系（调用方
/// 已 SetTransform(dpi 缩放)）；`dpi` 参数仅用于 snap 折算（传窗口
/// dpi_scale，如 150%→1.5）。
///
/// 线性插值（LINEAR）与虎娘一致：拉伸段抗锯齿，角块 1:1 时无重采样损耗。
pub unsafe fn draw_nineslice(
    ctx: &ID2D1DeviceContext,
    bitmap: &ID2D1Bitmap,
    src_w: u32,
    src_h: u32,
    dst_w: f32,
    dst_h: f32,
    tile_h: bool,
    before_h: i32,
    after_h: i32,
    tile_v: bool,
    before_v: i32,
    after_v: i32,
    dpi: f32,
    scale: f32,
) {
    let hslices = slice_axis_scaled(src_w, dst_w, tile_h, before_h, after_h, scale);
    let vslices = slice_axis_scaled(src_h, dst_h, tile_v, before_v, after_v, scale);
    // 取证：九宫格实际切片（每次渲染一条，排障期）
    if crate::tsf::trace_on() {
        let fmt = |s: &[SsfAxisSlice]| {
            s.iter()
                .map(|x| format!("{}+{:.0}→{:.1}+{:.1}", x.source, x.source_length, x.destination, x.destination_length))
                .collect::<Vec<_>>()
                .join(" | ")
        };
        crate::tsf::trace(&format!(
            "ssf9: H({src_w}→{dst_w:.0} 刀{before_h}/{after_h} scale{scale:.3}) [{}]",
            fmt(&hslices)
        ));
        crate::tsf::trace(&format!(
            "ssf9: V({src_h}→{dst_h:.0} 刀{before_v}/{after_v} scale{scale:.3}) [{}]",
            fmt(&vslices)
        ));
    }
    let snap = |v: f32| -> f32 {
        // 【snap 单位修 2026-10-16】dpi 参数=显示缩放比（1.0/1.25/1.5），
        // 物理像素= DIP×dpi。旧式 round(v*dpi/96)*96/dpi 是「DPI 原值
        // （96 基准）」语义——比值传入时把目标边全吸到 96px 网格
        //（100% 缩放下 143→96、202→192，7/9 块塌缩被吞）。
        if dpi > 0.0 && dpi.is_finite() {
            (v * dpi).round() / dpi
        } else {
            v
        }
    };
    for y in &vslices {
        for x in &hslices {
            let dst = D2D_RECT_F {
                left: snap(x.destination),
                top: snap(y.destination),
                right: snap(x.destination + x.destination_length),
                bottom: snap(y.destination + y.destination_length),
            };
            if dst.right <= dst.left || dst.bottom <= dst.top {
                continue;
            }
            if crate::tsf::trace_on() {
                let (xs, xl, ys, yl) = (x.source, x.source_length, y.source, y.source_length);
                crate::tsf::trace(&format!(
                    "ssf9blk: src(x{xs:.0}+{xl:.0} y{ys:.0}+{yl:.0}) → dst({:.1},{:.1})-({:.1},{:.1})",
                    dst.left, dst.top, dst.right, dst.bottom
                ));
            }
            let src = D2D_RECT_F {
                left: x.source,
                top: y.source,
                right: x.source + x.source_length,
                bottom: y.source + y.source_length,
            };
            let _ = ctx.DrawBitmap(
                bitmap,
                Some(&dst as *const D2D_RECT_F),
                1.0,
                D2D1_INTERPOLATION_MODE_LINEAR,
                Some(&src as *const D2D_RECT_F),
                None,
            );
        }
    }
}

// ============================================================================
// APNG 帧时钟（逐行移植虎娘 SsfAnimation.cpp:46-50 animationFrame）
// ============================================================================

/// 按已播放时长取当前帧。返回 (帧号, 距下帧剩余 ms)。
/// - 单帧/静态 → (0, 0)；
/// - 每帧延迟最小按 20ms 计（delays_ms 解码时已钳，此处双保险）；
/// - plays>0 且 `elapsed/总时长 ≥ plays` → 停在最后一帧 (len-1, 0)；
/// - 否则按 elapsed 环形取帧，remaining=当前帧剩余展示时长。
pub fn frame_at(bmp: &SsfBitmap, elapsed_ms: u64) -> (usize, u32) {
    let delays = &bmp.delays_ms;
    if delays.len() < 2 || bmp.frames.len() < 2 {
        return (0, 0);
    }
    let eff: Vec<u64> = delays.iter().map(|&d| d.max(MIN_FRAME_DELAY_MS) as u64).collect();
    let duration: u64 = eff.iter().sum();
    if duration == 0 {
        return (0, 0);
    }
    if bmp.plays > 0 && elapsed_ms / duration >= bmp.plays as u64 {
        return (delays.len() - 1, 0);
    }
    let mut within = elapsed_ms % duration;
    for (i, &d) in eff.iter().enumerate() {
        if within < d {
            return (i, (d - within) as u32);
        }
        within -= d;
    }
    (0, 0)
}

/// APNG 帧钟：每窗一份，皮肤换装/代次重建时 restart()（虎娘 skinStarted_
/// 对位物）。
pub struct SsfFrameClock {
    start: Instant,
}

impl Default for SsfFrameClock {
    fn default() -> Self {
        Self::new()
    }
}

impl SsfFrameClock {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
    /// 重置时间基准（换装/重解码后调用，动画从头播）。
    pub fn restart(&mut self) {
        self.start = Instant::now();
    }
    pub fn elapsed_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }
    /// 当前 (帧号, 距下帧剩余 ms)。
    pub fn current(&self, bmp: &SsfBitmap) -> (usize, u32) {
        frame_at(bmp, self.elapsed_ms())
    }
}

// ============================================================================
// 设备位图缓存（每窗口一份；虎娘 skinBitmapFrame_ 机制）
// ============================================================================

/// 每窗口级 D2D 位图缓存：帧号+尺寸没变直接复用（重建=CreateBitmap 整图
/// 上传，动图帧率下必须靠这个平摊成本）。dims 参与比对：换图（尺寸变）
/// 但帧号恰相同（都从 0 起）时防串用旧位图——虎娘在 `image != skinImage_`
/// 时 Reset（CandidateRendererSkin.cpp:15-16），虎符以尺寸代差近似同判。
pub struct SsfDeviceBitmap {
    pub d2d_bitmap: ID2D1Bitmap,
    pub frame_idx: usize,
    dims: (u32, u32),
}

// ============================================================================
// 【SSF 缩小高清化·Profile E 2026-10-18】虎娘 SkinRasterFilter 移植
// ============================================================================
//
// 病根：大图皮肤（1536×1024 画布）在日常字号下被大幅缩小（5:1 上下），
// D2D DrawBitmap 的 LINEAR 采样只看 4 邻域 texel——细节被平均成糊，皮肤
// 「缩小后不清晰」。虎娘解法（native/tsf/SkinRasterFilter.h + Candidate
// RendererSkin.cpp:68-122「Profile E」，desktop sampling review 定稿）：
//
// ① 预过滤：按「物理目标 × 1.5」用高品质重采样核（核宽随缩小比加宽）
//    先把原图缩到中间尺寸（不超过原图=不放大），D2D 末段只做 ≤1.5:1
//    的轻采样——细节保留（ oversample 1.5 = Profile E 实测参数）；
// ② 预乘钳制：三次核会在半透明边缘过冲，重建后每通道 ≤ alpha 再交给 D2D
//   （虎娘注释：Filtering premultiplied color and alpha together avoids
//    dark/color fringes——重采样全程在预乘域做）；
// ③ 锐化一次（缓存 miss 时）：3×3 [1,2,1]⊗[1,2,1]/16 反锐化掩模，
//    amount=0.30、threshold=1.0，只碰不透明区（alpha≥250——自制 PNG
//    实心区常见 253/254，半透明剪影/光晕不动），直 RGB 域算细节再回预乘；
// ④ 位图 DPI=96×预过滤宽/原图宽：九宫格 source 矩形（画布坐标）无需
//    改动——D2D 设备上下文 DrawBitmap 的 source 以位图 DIP 计，DPI 自动
//    折算（虎娘「Bitmap DPI keeps source rectangles in authored coords」）。
//
// CPU 预过滤结果进 LRU（8 条/16MB，虎娘同款预算）——静态皮肤每档 DPI
// 只算一次；APNG 逐帧同路径。任何失败静默回退原图 1:1（保底出窗）。

/// Profile E 过采样系数（虎娘 skinRasterOversample=1.5f 同值）。
const SSF_RASTER_OVERSAMPLE: f32 = 1.5;
/// 预过滤 LRU：条数与字节预算（虎娘 budget=16MB/entries=8 同款）。
const SSF_RASTER_BUDGET: usize = 16 * 1024 * 1024;
const SSF_RASTER_ENTRIES: usize = 8;

/// 一条 CPU 侧预过滤帧（BGRA 预乘）。
struct SsfRaster {
    frame: usize,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

/// 每窗口（SsfRuntime）一份的预过滤 LRU 缓存。
pub struct SsfRasterCache {
    entries: Vec<SsfRaster>,
    bytes: usize,
}

impl Default for SsfRasterCache {
    fn default() -> Self {
        Self::new()
    }
}

impl SsfRasterCache {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            bytes: 0,
        }
    }
    /// 命中则移到尾（LRU）并返回 true。
    fn touch(&mut self, frame: usize, w: u32, h: u32) -> bool {
        if let Some(pos) = self
            .entries
            .iter()
            .position(|r| r.frame == frame && r.width == w && r.height == h)
        {
            let r = self.entries.remove(pos);
            self.entries.push(r);
            true
        } else {
            false
        }
    }
    /// 取条目像素（要求先 touch 命中；两段式避开跨分支借用统一）。
    fn pixels_of(&self, frame: usize, w: u32, h: u32) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|r| r.frame == frame && r.width == w && r.height == h)
            .map(|r| r.pixels.as_slice())
    }
    fn insert(&mut self, frame: usize, w: u32, h: u32, pixels: Vec<u8>) {
        let bytes = pixels.len();
        while !self.entries.is_empty()
            && (self.bytes + bytes > SSF_RASTER_BUDGET || self.entries.len() >= SSF_RASTER_ENTRIES)
        {
            let front = self.entries.remove(0);
            self.bytes -= front.pixels.len();
        }
        self.bytes += bytes;
        self.entries.push(SsfRaster {
            frame,
            width: w,
            height: h,
            pixels,
        });
    }
}

/// 【Profile E ③】反锐化掩模（虎娘 sharpenSkinRaster 逐行移植，BGRA 字节
/// 版）。pixels=BGRA 预乘；只改不透明区（9 邻域 alpha 全 ≥250），半透明
/// 边缘原样。宽高 <3 直接返回。
fn sharpen_skin_raster(pixels: &mut [u8], width: u32, height: u32) {
    let (w, h) = (width as usize, height as usize);
    if w < 3 || h < 3 || pixels.len() < w * h * 4 {
        return;
    }
    const AMOUNT: f32 = 0.30;
    const THRESHOLD: f32 = 1.0;
    const KERNEL: [f32; 3] = [1.0, 2.0, 1.0]; // ⊗ 自身 /16
    let orig = pixels.to_vec();
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let idx = (y * w + x) * 4;
            let mut opaque = true;
            let mut blur = [0f32; 3];
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let row = (y as i32 + dy) as usize;
                    let col = (x as i32 + dx) as usize;
                    let p = (row * w + col) * 4;
                    let a = orig[p + 3];
                    // 自制 PNG 实心区常见 alpha 253/254：计入不透明；
                    // 半透明剪影与光晕整块跳过（虎娘同款护栏）。
                    if a < 250 {
                        opaque = false;
                    }
                    let weight = KERNEL[(dy + 1) as usize] * KERNEL[(dx + 1) as usize] / 16.0;
                    let inv = 255.0 / (a as f32).max(1.0);
                    blur[0] += orig[p] as f32 * inv * weight;
                    blur[1] += orig[p + 1] as f32 * inv * weight;
                    blur[2] += orig[p + 2] as f32 * inv * weight;
                }
            }
            if !opaque {
                continue;
            }
            let alpha = orig[idx + 3] as f32;
            let inv = 255.0 / alpha.max(1.0);
            for c in 0..3 {
                let o = orig[idx + c] as f32 * inv;
                let detail = o - blur[c];
                let value = o + if detail.abs() > THRESHOLD {
                    AMOUNT * detail
                } else {
                    0.0
                };
                let premul = (value * alpha / 255.0).round().clamp(0.0, alpha);
                pixels[idx + c] = premul as u8;
            }
        }
    }
}

/// 【Profile E ①+②】BGRA 预乘帧 → 高品质缩小（核宽随比率加宽的 Lanczos3）
/// + 预乘钳制 + 锐化。返回 (宽, 高, 像素)；任何失败返回 None（回退 1:1）。
fn prefilter_frame(
    frame: &[u8],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
) -> Option<(u32, u32, Vec<u8>)> {
    if frame.len() < (src_w as usize) * (src_h as usize) * 4 || dst_w == 0 || dst_h == 0 {
        return None;
    }
    // BGRA → RGBA（image crate 原生序）
    let mut rgba = frame.to_vec();
    for px in rgba.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    let img = image::RgbaImage::from_raw(src_w, src_h, rgba)?;
    // 缩小在预乘域进行（alpha 与色一起滤波——虎娘：避免不可见 RGB
    // 渗出的暗边/彩边）。Lanczos3=核宽随缩小比加宽的高品质重采样核
    //（WIC HighQualityCubic 的 image crate 对应物）。
    let small = image::imageops::resize(&img, dst_w, dst_h, image::imageops::FilterType::Lanczos3);
    let mut out = small.into_raw();
    for px in out.chunks_exact_mut(4) {
        px.swap(0, 2); // RGBA → BGRA
        let a = px[3] as u16;
        // 三次核过冲钳制：预乘域每通道 ≤ alpha
        for c in 0..3 {
            if px[c] as u16 > a {
                px[c] = a as u8;
            }
        }
    }
    sharpen_skin_raster(&mut out, dst_w, dst_h);
    Some((dst_w, dst_h, out))
}

/// 取当前帧的 D2D 位图（B8G8R8A8_UNORM + PREMULTIPLIED）。帧号越界钳到
/// 末帧（防御：fp 切换瞬间帧流长度可能先于调用方更新）。
///
/// 【Profile E】`phys_scale`=源画布像素→屏幕物理像素 的总缩放系数
///（= 皮肤等比 scale × dpi_scale）。phys_scale×1.5 ≥ 1（放大/轻缩）时
/// 位图 1:1 dpi=96（老路径，源像素直传——小倍率重采样无收益）；显著
/// 缩小时先按 `ceil(src×min(1, phys_scale×1.5))` 预过滤（LRU 缓存），
/// 位图 dpi=96×预过滤宽/源宽——draw_nineslice 的画布坐标 source 矩阵
/// 无需任何改动。
pub unsafe fn device_bitmap<'a>(
    ctx: &ID2D1DeviceContext,
    bmp: &SsfBitmap,
    frame_idx: usize,
    phys_scale: f32,
    cache: &'a mut Option<SsfDeviceBitmap>,
    rasters: &'a mut SsfRasterCache,
) -> windows_core::Result<&'a ID2D1Bitmap> {
    let idx = frame_idx.min(bmp.frames.len().saturating_sub(1));
    // Profile E 目标尺寸（物理 ×1.5，封顶原图=不放大；虎娘 extent 同式）
    let factor = if phys_scale.is_finite() && phys_scale > 0.0 {
        (phys_scale * SSF_RASTER_OVERSAMPLE).min(1.0)
    } else {
        1.0
    };
    let pre_w = ((bmp.width as f32 * factor).ceil() as u32)
        .clamp(1, bmp.width.max(1));
    let pre_h = ((bmp.height as f32 * factor).ceil() as u32)
        .clamp(1, bmp.height.max(1));
    // 命中判定先做（共享借用立即结束，不与后续可变借用打架）
    let hit = cache
        .as_ref()
        .map(|c| c.frame_idx == idx && c.dims == (pre_w, pre_h))
        .unwrap_or(false);
    if hit {
        return Ok(&cache.as_ref().unwrap().d2d_bitmap);
    }
    let Some(frame) = bmp.frames.get(idx) else {
        return Err(windows_core::Error::from_hresult(windows::Win32::Foundation::E_FAIL));
    };
    // 【Profile E】显著缩小时走预过滤路径（LRU 两段式：先确保缓存就绪
    // 再单次借用）；预过滤失败静默回退源像素 1:1（保底出窗，防每帧
    // 重试风暴——dims 记实际进径）。
    let mut use_pre = false;
    if (pre_w, pre_h) != (bmp.width, bmp.height) {
        if !rasters.touch(idx, pre_w, pre_h) {
            if let Some((w, h, pixels)) =
                prefilter_frame(frame, bmp.width, bmp.height, pre_w, pre_h)
            {
                rasters.insert(idx, w, h, pixels);
            }
            use_pre = rasters.touch(idx, pre_w, pre_h);
        } else {
            use_pre = true;
        }
    }
    let (bw, bh) = if use_pre { (pre_w, pre_h) } else { (bmp.width, bmp.height) };
    let (dpi_x, dpi_y) = if use_pre {
        (
            96.0 * pre_w as f32 / bmp.width.max(1) as f32,
            96.0 * pre_h as f32 / bmp.height.max(1) as f32,
        )
    } else {
        (96.0, 96.0)
    };
    let src_pixels: &[u8] = if use_pre {
        rasters
            .pixels_of(idx, pre_w, pre_h)
            .filter(|p| p.len() >= (pre_w as usize) * (pre_h as usize) * 4)
            .unwrap_or(frame)
    } else {
        frame
    };
    let props = D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        // 位图 DPI=源画布坐标 → 预过滤像素的自动折算（虎娘
        // CandidateRendererSkin.cpp:116-119 同款；draw_nineslice 的
        // source 矩形恒为画布坐标，两处调用零改动）。
        dpiX: dpi_x,
        dpiY: dpi_y,
        bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
        colorContext: std::mem::ManuallyDrop::new(None),
    };
    let size = D2D_SIZE_U {
        width: bw.max(1),
        height: bh.max(1),
    };
    // ID2D1DeviceContext::CreateBitmap（扩展版，D2D1_BITMAP_PROPERTIES1）：
    // 直接从 BGRA 预乘内存建图（虎娘 CreateBitmap(SizeU, data, stride,
    // PBGRA,…) 同款；DPI 见 Profile E 注释）
    let b1: ID2D1Bitmap1 = ctx.CreateBitmap(
        size,
        Some(src_pixels.as_ptr() as *const core::ffi::c_void),
        bw.max(1) * 4,
        &props,
    )?;
    let b: ID2D1Bitmap = b1.cast()?;
    *cache = Some(SsfDeviceBitmap {
        d2d_bitmap: b,
        frame_idx: idx,
        dims: (pre_w, pre_h),
    });
    Ok(&cache.as_ref().unwrap().d2d_bitmap)
}

// ============================================================================
// 皮肤 JSON 描述解析（SsfSkinSpec）
// ============================================================================

/// 皮肤色读取：`#RRGGBB`（缺省 alpha=0xFF 不透明）或 `#RRGGBBAA`（A 在
/// 末尾）——与 candwin2.rs:1386-1396 parse_hex 完全同口径（server 契约：
/// skin.ini 的 0xBBGGRR 在下发前已转成 #RRGGBBAA）。坏值/非 ASCII 返回
/// None，由调用方落默认色（降级不失败）。
pub fn parse_hex_rgba(s: &str) -> Option<[u8; 4]> {
    let s = s.trim_start_matches('#');
    // 非 ASCII 颜色串按字节切片会切进 UTF-8 字符中间 panic（渲染线程
    // panic=宿主崩）——先校验（同 candwin2.rs:1388-1393 的防护）
    if !s.is_ascii() || (s.len() != 8 && s.len() != 6) {
        return None;
    }
    let b = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
    Some([b(0)?, b(2)?, b(4)?, if s.len() == 8 { b(6)? } else { 0xFF }])
}

/// [R,G,B,A] → D2D 颜色（供渲染分支直接取用）。
pub fn rgba_to_color_f(c: [u8; 4]) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: c[0] as f32 / 255.0,
        g: c[1] as f32 / 255.0,
        b: c[2] as f32 / 255.0,
        a: c[3] as f32 / 255.0,
    }
}

/// 双形态读取（candwin2.rs:1399-1423 的 color_f/layout_f 口径）：优先
/// `/skin/...` 包装层（server 管道下发形态），回退顶层直挂（本地 JSON/
/// 测试钩子形态）。`"/skin/display/font_size"` 与 `"display/font_size"`
/// 等价。
fn skin_get<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    v.pointer(path).or_else(|| {
        let stripped = path.strip_prefix("/skin/").unwrap_or(path);
        let mut cur = v;
        for seg in stripped.split('/').filter(|s| !s.is_empty()) {
            cur = cur.get(seg)?;
        }
        Some(cur)
    })
}

fn get_f32(v: &Value, path: &str, default: f32) -> f32 {
    skin_get(v, path).and_then(|x| x.as_f64()).unwrap_or(default as f64) as f32
}

fn get_i32(v: &Value, path: &str, default: i32) -> i32 {
    // JSON 数值可能是 270 或 270.0，统一走 f64 再取整
    skin_get(v, path)
        .and_then(|x| x.as_f64())
        .map(|x| x as i32)
        .unwrap_or(default)
        .clamp(0, 8191)
}

fn get_bool(v: &Value, path: &str, default: bool) -> bool {
    skin_get(v, path).and_then(|x| x.as_bool()).unwrap_or(default)
}

fn get_color(v: &Value, path: &str, default: &str) -> [u8; 4] {
    skin_get(v, path)
        .and_then(|x| x.as_str())
        .and_then(parse_hex_rgba)
        .or_else(|| parse_hex_rgba(default))
        .unwrap_or([32, 32, 34, 255])
}

fn get_str(v: &Value, path: &str) -> Option<String> {
    skin_get(v, path).and_then(|x| x.as_str()).map(|s| s.to_string())
}

/// 一轴刀口：tile=平铺（skin.ini layout mode 1）否则拉伸；before/after=
/// 固定边宽（画布单位）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SsfStretch {
    pub tile: bool,
    pub before: i32,
    pub after: i32,
}

impl Default for SsfStretch {
    fn default() -> Self {
        // mode 0=拉伸、刀口 0：整幅拉伸（真皮肤罕见、兜底形态）
        Self {
            tile: false,
            before: 0,
            after: 0,
        }
    }
}

/// 文字内边距（画布单位；skin.ini marge 顺序 top,bottom,left,right——
/// 虎娘 Insets{n0,n1,n2,n3} 与社区交叉验证一致）。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SsfInsets {
    pub top: i32,
    pub bottom: i32,
    pub left: i32,
    pub right: i32,
}

/// 布局变体：H1/V1=单窗横/竖，H2/V2 分体（Code=编码区图、Candidates=
/// 候选区图）。虎符单窗渲染，分体变体作回退资源用。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SsfVariant {
    Horizontal,
    Vertical,
    HorizontalCode,
    HorizontalCandidates,
    VerticalCode,
    VerticalCandidates,
}

/// 一个布局变体的完整描述。
#[derive(Clone, Debug, PartialEq)]
pub struct SsfVariantLayout {
    /// 背景图文件名（归一小写相对名，落于 cache_dir 下）。
    pub image: String,
    /// 水平轴刀口（layout_horizontal）。
    pub horizontal: SsfStretch,
    /// 垂直轴刀口（layout_vertical）。
    pub vertical: SsfStretch,
    /// 编码行内边距（pinyin_marge）。
    pub code: SsfInsets,
    /// 候选区内边距（zhongwen_marge）。
    pub candidates: SsfInsets,
}

impl SsfVariantLayout {
    /// 最小宽（画布域）：max(图宽, 水平刀口和+1)——虎娘 layoutSkin 公式
    /// （CandidateRendererSkin.cpp:58-60）。九宫格不退化的硬底。
    pub fn min_width(&self, img_w: u32) -> f32 {
        (img_w.max(
            self.horizontal.before.max(0) as u32 + self.horizontal.after.max(0) as u32 + 1,
        )) as f32
    }
    /// 最小高（画布域）：max(图高, 垂直刀口和+1)。
    pub fn min_height(&self, img_h: u32) -> f32 {
        (img_h.max(
            self.vertical.before.max(0) as u32 + self.vertical.after.max(0) as u32 + 1,
        )) as f32
    }
}

/// [Display] 节：字号/字体/四色/三间距（画布单位）。
#[derive(Clone, Debug, PartialEq)]
pub struct SsfDisplay {
    /// 作者画布字号（虎娘大图皮肤=200，真搜狗皮肤=16~18）。用户字号÷此值
    /// =全局等比缩放系数 scale。
    pub font_size: f32,
    pub font_ch: String,
    pub font_en: String,
    /// 编码行色（pinyin_color）。[R,G,B,A]
    pub code_color: [u8; 4],
    /// 第一候选文字色（zhongwen_first_color；.ssf 皮肤的高亮表达=颜色，
    /// 不画胶囊）。
    pub first_color: [u8; 4],
    /// 候选文字色（zhongwen_color）。
    pub text_color: [u8; 4],
    /// 注释色（comphint_color）。
    pub annotation_color: [u8; 4],
    pub candidate_spacing: f32,
    pub line_spacing: f32,
    pub character_spacing: f32,
}

/// .ssf 皮肤描述（DLL 收到的皮肤 JSON 中 `kind:"ssf"` 形态）。
#[derive(Clone, Debug)]
pub struct SsfSkinSpec {
    /// 解包缓存目录（绝对路径：`数据目录\皮肤缓存\{id}`）。
    pub cache_dir: PathBuf,
    /// 源 .ssf 指纹（size+mtime+sha256）——解码缓存失效判据。
    pub fp: String,
    pub display: SsfDisplay,
    /// 只含存在的变体（key=变体）。
    pub layouts: HashMap<SsfVariant, SsfVariantLayout>,
}

/// 皮肤形态检测：`/skin/kind` 或顶层 `kind` == "ssf"（双形态口径，参照
/// candwin2.rs:1399-1410；现有 JSON 皮肤顶层无 kind 键、material.kind 在
/// /skin/material/kind 下，不会被误判）。
pub fn is_ssf(v: &Value) -> bool {
    skin_get(v, "/skin/kind").and_then(|x| x.as_str()) == Some("ssf")
}

impl SsfSkinSpec {
    /// 解析 .ssf 皮肤描述。None=不是 ssf 形态或不可用（cache_dir/fp 缺失、
    /// layouts 全缺）——调用方回退现有 JSON 矢量渲染（降级矩阵 §4.6）。
    /// 坏字段一律落默认值（虎娘「尽力加载」铁律），不因单字段失败。
    pub fn parse(v: &Value) -> Option<Self> {
        if !is_ssf(v) {
            return None;
        }
        let cache_dir = get_str(v, "/skin/cache_dir")?;
        let fp = get_str(v, "/skin/fp")?;
        // 布局变体：layouts 只含存在变体（horizontal/vertical/
        // horizontal_code/horizontal_candidates/vertical_code/vertical_candidates）
        let mut layouts = HashMap::new();
        if let Some(obj) = skin_get(v, "/skin/layouts").and_then(|x| x.as_object()) {
            for (key, lv) in obj {
                let variant = match key.as_str() {
                    "horizontal" => Some(SsfVariant::Horizontal),
                    "vertical" => Some(SsfVariant::Vertical),
                    "horizontal_code" => Some(SsfVariant::HorizontalCode),
                    "horizontal_candidates" => Some(SsfVariant::HorizontalCandidates),
                    "vertical_code" => Some(SsfVariant::VerticalCode),
                    "vertical_candidates" => Some(SsfVariant::VerticalCandidates),
                    _ => None,
                };
                let Some(variant) = variant else {
                    continue;
                };
                let Some(image) = get_str(lv, "/image") else {
                    continue; // 无背景图的变体跳过（虎娘：pic 缺失跳过该 Scheme）
                };
                let stretch = |axis: &str| SsfStretch {
                    tile: get_bool(lv, &format!("/{axis}/tile"), false),
                    before: get_i32(lv, &format!("/{axis}/before"), 0),
                    after: get_i32(lv, &format!("/{axis}/after"), 0),
                };
                let insets = |name: &str| SsfInsets {
                    top: get_i32(lv, &format!("/{name}/top"), 0),
                    bottom: get_i32(lv, &format!("/{name}/bottom"), 0),
                    left: get_i32(lv, &format!("/{name}/left"), 0),
                    right: get_i32(lv, &format!("/{name}/right"), 0),
                };
                layouts.insert(
                    variant,
                    SsfVariantLayout {
                        image,
                        horizontal: stretch("horizontal"),
                        vertical: stretch("vertical"),
                        code: insets("code"),
                        candidates: insets("candidates"),
                    },
                );
            }
        }
        if layouts.is_empty() {
            // Scheme 全缺：该皮肤不可用，整体回退（虎娘 "SSF has no
            // supported candidate background"）
            return None;
        }
        let display = SsfDisplay {
            // 虎娘口径：font_size ∈ [3,200]，真皮肤缺省 16（1x 尺度）
            font_size: get_f32(v, "/skin/display/font_size", 16.0).clamp(3.0, 200.0),
            font_ch: get_str(v, "/skin/display/font_ch").unwrap_or_default(),
            font_en: get_str(v, "/skin/display/font_en").unwrap_or_default(),
            code_color: get_color(v, "/skin/display/code_color", "#202022FF"),
            first_color: get_color(v, "/skin/display/first_color", "#202022FF"),
            text_color: get_color(v, "/skin/display/text_color", "#202022FF"),
            annotation_color: get_color(v, "/skin/display/annotation_color", "#808090FF"),
            candidate_spacing: get_f32(v, "/skin/display/candidate_spacing", 0.0),
            line_spacing: get_f32(v, "/skin/display/line_spacing", 0.0),
            character_spacing: get_f32(v, "/skin/display/character_spacing", 0.0),
        };
        Some(Self {
            cache_dir: PathBuf::from(cache_dir),
            fp,
            display,
            layouts,
        })
    }

    /// 变体选择（虎娘 Definition::choose 移植 + 第 5 级回退，SsfSkin.cpp:
    /// 42-45）：请求变体 → 同向单窗 → 同向分体(Candidates) → 反向横竖
    /// 单窗 → **反向分体(Candidates)**，逐级回退保证任何皮肤都能出窗。
    /// 末级场景：竖排窗皮肤只带 horizontal_candidates 也能出窗（M1
    /// hufu-skin 侧同口径）。
    ///
    /// - 无候选（纯编码窗）→ Code 变体；无编码（纯候选窗）→ Candidates
    ///   变体；两者都有 → 单窗变体（虎符常态）。
    pub fn choose(&self, vertical: bool, has_code: bool, has_candidates: bool) -> Option<&SsfVariantLayout> {
        let requested = if !has_candidates {
            if vertical {
                SsfVariant::VerticalCode
            } else {
                SsfVariant::HorizontalCode
            }
        } else if !has_code {
            if vertical {
                SsfVariant::VerticalCandidates
            } else {
                SsfVariant::HorizontalCandidates
            }
        } else if vertical {
            SsfVariant::Vertical
        } else {
            SsfVariant::Horizontal
        };
        let chain = [
            requested,
            if vertical {
                SsfVariant::Vertical
            } else {
                SsfVariant::Horizontal
            },
            if vertical {
                SsfVariant::VerticalCandidates
            } else {
                SsfVariant::HorizontalCandidates
            },
            if vertical {
                SsfVariant::Horizontal
            } else {
                SsfVariant::Vertical
            },
            // 【第 5 级 2026-12 审查补】反朝向分体（Candidates）——
            // 虎娘 4 级链在此返回 None → 整皮回退；补齐后单变体皮肤
            // （尤其只有 *_candidates 的分体皮）任何朝向都能出窗
            if vertical {
                SsfVariant::HorizontalCandidates
            } else {
                SsfVariant::VerticalCandidates
            },
        ];
        chain.iter().find_map(|var| self.layouts.get(var))
    }

    /// 变体背景图完整路径（cache_dir\{image}；image 已在 parse 时保真，
    /// 读取前仍经 load_bitmap 的 normalize 二次消毒）。
    pub fn image_path(&self, layout: &SsfVariantLayout) -> PathBuf {
        self.cache_dir.join(&layout.image)
    }
}

// ============================================================================
// 测试（纯数据/算法路径，不触 COM；宿主目标 cargo test 可跑）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slice_axis_stretch_three_cuts() {
        // 常规九宫格：源 100、目标 200、刀口 20/20 → 3 块（角原样、中段拉伸）
        let s = slice_axis(100, 200.0, false, 20, 20);
        assert_eq!(s.len(), 3);
        assert_eq!(s[0], SsfAxisSlice { source: 0.0, source_length: 20.0, destination: 0.0, destination_length: 20.0 });
        assert_eq!(s[1], SsfAxisSlice { source: 20.0, source_length: 60.0, destination: 20.0, destination_length: 160.0 });
        assert_eq!(s[2], SsfAxisSlice { source: 80.0, source_length: 20.0, destination: 180.0, destination_length: 20.0 });
    }

    #[test]
    fn slice_axis_scaled_fixed_zones_shrink_proportionally() {
        // 【SSF 双域】scale=0.5：源 100（刀口 20/20），目标 30 → 固定段
        // 20×0.5=10（源不动），中段源 60 → 目标 30-10-10=10。
        let s = slice_axis_scaled(100, 30.0, false, 20, 20, 0.5);
        assert_eq!(s.len(), 3);
        assert_eq!(s[0], SsfAxisSlice { source: 0.0, source_length: 20.0, destination: 0.0, destination_length: 10.0 });
        assert_eq!(s[1], SsfAxisSlice { source: 20.0, source_length: 60.0, destination: 10.0, destination_length: 10.0 });
        assert_eq!(s[2], SsfAxisSlice { source: 80.0, source_length: 20.0, destination: 20.0, destination_length: 10.0 });
    }

    #[test]
    fn slice_axis_scaled_yuanqi_geometry() {
        // 元气虎娘实锤几何：源 1536（刀口 300/1235 → 中带 1px），目标 153，
        // scale=0.08 → 左固定 24、右固定 98.8、中带目标 30.2。
        let s = slice_axis_scaled(1536, 153.0, false, 300, 1235, 0.08);
        assert_eq!(s.len(), 3);
        assert!((s[0].destination_length - 24.0).abs() < 0.01);
        assert!((s[0].source_length - 300.0).abs() < 0.01);
        assert!((s[1].source_length - 1.0).abs() < 0.01);
        assert!((s[1].destination_length - 30.2).abs() < 0.05);
        assert!((s[2].destination - 54.2).abs() < 0.05);
        assert!((s[2].destination_length - 98.8).abs() < 0.01);
        assert!((s[2].source - 301.0).abs() < 0.01);
    }

    #[test]
    fn slice_axis_scaled_tile_step_is_scaled() {
        // 平铺：中段 60 源像素、scale 0.5 → 每块目标长 30，铺满 90 → 3 块
        let s = slice_axis_scaled(100, 110.0, true, 20, 20, 0.5);
        // 首尾 10+10，可用 90，步长 30 → 中段 3 块（source 恒为 before=20）
        let mids: Vec<_> = s.iter().filter(|x| (x.source - 20.0).abs() < 0.01 && x.source_length > 1.0).collect();
        assert_eq!(mids.len(), 3);
        assert!((mids[0].destination_length - 30.0).abs() < 0.01);
        assert!((mids[0].source_length - 60.0).abs() < 0.01);
    }

    #[test]
    fn slice_axis_cuts_shrink_to_source() {
        // 刀口和 120 ≥ 源宽 100 → 按 (100-1)/120 收缩 → ≈49.5/49.5，
        // 中段 1px；目标同宽时三块近似原样
        let s = slice_axis(100, 100.0, false, 60, 60);
        assert_eq!(s.len(), 3);
        let b = 60.0f32 * (99.0 / 120.0);
        assert!((s[0].source_length - b).abs() < 0.01);
        assert!((s[1].source_length - 1.0).abs() < 0.01);
        assert!((s[2].source - (100.0 - b)).abs() < 0.01);
    }

    #[test]
    fn slice_axis_destination_below_cuts() {
        // 目标 40 < 刀口和 60 → 目标侧刀口等比收缩到 20/20，可用宽 0
        // → 中段丢弃，仅剩两角（压缩绘制）
        let s = slice_axis(100, 40.0, false, 30, 30);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].destination_length, 20.0);
        assert_eq!(s[1].destination, 20.0);
        // 源刀口不缩：仍取源 [0,30) 与 [70,100)
        assert_eq!(s[0].source_length, 30.0);
        assert_eq!(s[1].source, 70.0);
    }

    #[test]
    fn slice_axis_tile_budget() {
        // 平铺：源 100、刀口 10/10、中段 80；目标 340 → 可用 320=4×80
        // → 4 平铺块 + 2 角 = 6 块
        let s = slice_axis(100, 340.0, true, 10, 10);
        assert_eq!(s.len(), 6);
        // 平铺块目标长=源长（1:1）
        assert_eq!(s[1].destination_length, s[1].source_length);
        // 超 64 块预算（可用/中段 >64）回退整段拉伸
        let far = slice_axis(100, 6000.0, true, 10, 10);
        assert_eq!(far.len(), 3);
    }

    #[test]
    fn slice_axis_degenerate() {
        assert!(slice_axis(0, 100.0, false, 10, 10).is_empty());
        assert!(slice_axis(100, 0.0, false, 10, 10).is_empty());
        assert!(slice_axis(100, f32::NAN, false, 10, 10).is_empty());
        // 负刀口按 0 收（整幅拉伸）
        assert_eq!(slice_axis(100, 100.0, false, -5, -5).len(), 1);
    }

    #[test]
    fn frame_at_semantics() {
        let bmp = SsfBitmap {
            width: 1,
            height: 1,
            plays: 0,
            frames: vec![vec![0; 4], vec![0; 4]],
            delays_ms: vec![20, 30],
        };
        // 环形：总 50ms
        assert_eq!(frame_at(&bmp, 0), (0, 20));
        assert_eq!(frame_at(&bmp, 19), (0, 1));
        assert_eq!(frame_at(&bmp, 20), (1, 30));
        assert_eq!(frame_at(&bmp, 49), (1, 1));
        assert_eq!(frame_at(&bmp, 50), (0, 20)); // 回绕
        // 单帧 → 静态
        let single = SsfBitmap {
            width: 1,
            height: 1,
            plays: 0,
            frames: vec![vec![0; 4]],
            delays_ms: vec![0],
        };
        assert_eq!(frame_at(&single, 999_999), (0, 0));
        // plays=1：elapsed ≥ 一轮时长即播完停末帧（虎娘 elapsed/duration>=plays）
        let once = SsfBitmap { plays: 1, ..bmp };
        assert_eq!(frame_at(&once, 49), (1, 1)); // 第一轮内（末帧余 1ms）
        assert_eq!(frame_at(&once, 50), (1, 0)); // 恰播完一轮 → 停末帧
        assert_eq!(frame_at(&once, 500), (1, 0)); // elapsed/50=10 ≥ 1
        // 低于 20ms 的延迟按 20 计（双保险）
        let tiny = SsfBitmap {
            width: 1,
            height: 1,
            plays: 0,
            frames: vec![vec![0; 4], vec![0; 4], vec![0; 4]],
            delays_ms: vec![1, 1, 1],
        };
        assert_eq!(frame_at(&tiny, 0), (0, 20));
        assert_eq!(frame_at(&tiny, 20), (1, 20));
        assert_eq!(frame_at(&tiny, 60), (0, 20));
    }

    #[test]
    fn frame_clock_current() {
        let bmp = SsfBitmap {
            width: 1,
            height: 1,
            plays: 0,
            frames: vec![vec![0; 4], vec![0; 4]],
            delays_ms: vec![50, 50],
        };
        let clock = SsfFrameClock::new();
        let (idx, rem) = clock.current(&bmp);
        assert_eq!(idx, 0);
        assert!(rem <= 50 && rem > 0);
    }

    #[test]
    fn hex_colors() {
        assert_eq!(parse_hex_rgba("#3B5179FF"), Some([0x3B, 0x51, 0x79, 0xFF]));
        assert_eq!(parse_hex_rgba("#3B5179"), Some([0x3B, 0x51, 0x79, 0xFF]));
        assert_eq!(parse_hex_rgba(""), None);
        assert_eq!(parse_hex_rgba("#12345"), None);
        assert_eq!(parse_hex_rgba("#红色背景"), None);
    }

    #[test]
    fn spec_parse_and_choose() {
        // 任务书示例形状（包装形态）
        let v: Value = serde_json::json!({
            "skin": {
                "kind": "ssf",
                "cache_dir": "C:\\皮肤缓存\\蓝鲸雪语",
                "fp": "a1b2c3-1098622",
                "display": {
                    "font_size": 200.0,
                    "font_ch": "微软雅黑", "font_en": "Segoe UI",
                    "code_color": "#FF7B513B", "first_color": "#FF2653A8",
                    "text_color": "#FF334462", "annotation_color": "#FF687B92",
                    "candidate_spacing": 120.0, "line_spacing": 24.0,
                    "character_spacing": 0.0
                },
                "layouts": {
                    "horizontal": {
                        "image": "Panel.PNG",
                        "horizontal": {"tile": false, "before": 270, "after": 271},
                        "vertical": {"tile": false, "before": 812, "after": 211},
                        "code": {"top": 770, "bottom": 32, "left": 250, "right": 320},
                        "candidates": {"top": 32, "bottom": 165, "left": 250, "right": 320}
                    }
                }
            }
        });
        assert!(is_ssf(&v));
        let spec = SsfSkinSpec::parse(&v).expect("parse");
        assert_eq!(spec.fp, "a1b2c3-1098622");
        assert_eq!(spec.display.font_size, 200.0);
        assert_eq!(spec.display.code_color, [0xFF, 0x7B, 0x51, 0x3B]);
        assert_eq!(spec.display.first_color, [0xFF, 0x26, 0x53, 0xA8]);
        assert_eq!(spec.layouts.len(), 1);
        let h = spec.layouts.get(&SsfVariant::Horizontal).unwrap();
        assert_eq!(h.image, "Panel.PNG");
        assert_eq!(h.horizontal, SsfStretch { tile: false, before: 270, after: 271 });
        assert_eq!(h.vertical.before, 812);
        assert_eq!(h.candidates.top, 32);
        assert_eq!(h.code.left, 250);
        // 只有 horizontal：任意请求都回退到它
        for &(vert, code, cand) in &[(false, true, true), (true, true, true), (true, false, false), (false, true, false)] {
            let chosen = spec.choose(vert, code, cand).expect("fallback");
            assert_eq!(chosen.image, "Panel.PNG");
        }
        // min 宽高（图 1536×1024）
        assert_eq!(h.min_width(1536), 1536.0);
        assert_eq!(h.min_height(1024), 1024.0);
        // 非空 fp/cache_dir 传递
        assert!(spec.image_path(h).ends_with("Panel.PNG"));

        // 顶层直挂形态（双形态口径）
        let top: Value = serde_json::json!({
            "kind": "ssf",
            "cache_dir": "X:\\d",
            "fp": "f",
            "layouts": {"vertical": {"image": "v.png"}}
        });
        assert!(is_ssf(&top));
        let spec2 = SsfSkinSpec::parse(&top).expect("parse2");
        assert!(spec2.layouts.contains_key(&SsfVariant::Vertical));
        // 现有 JSON 皮肤不误判
        let legacy: Value = serde_json::json!({"skin": {"colors": {}, "material": {"kind": "solid"}}});
        assert!(!is_ssf(&legacy));
        assert!(SsfSkinSpec::parse(&legacy).is_none());
        // layouts 全缺 → None
        let empty: Value = serde_json::json!({"kind": "ssf", "cache_dir": "X:\\d", "fp": "f"});
        assert!(SsfSkinSpec::parse(&empty).is_none());
    }

    #[test]
    fn choose_fifth_level_reverse_candidates() {
        // 【第 5 级回退】竖排窗 + 皮肤只带 horizontal_candidates → 仍能出窗
        //（4 级链在此返回 None → 整皮回退 JSON，审查补齐）
        let v: Value = serde_json::json!({
            "kind": "ssf", "cache_dir": "X:\\d", "fp": "f",
            "layouts": {"horizontal_candidates": {"image": "hc.png"}}
        });
        let spec = SsfSkinSpec::parse(&v).expect("parse");
        let chosen = spec.choose(true, true, true).expect("第 5 级命中");
        assert_eq!(chosen.image, "hc.png");
        // 横排窗 + 只带 vertical_candidates → 反向同理
        let v2: Value = serde_json::json!({
            "kind": "ssf", "cache_dir": "X:\\d", "fp": "f",
            "layouts": {"vertical_candidates": {"image": "vc.png"}}
        });
        let spec2 = SsfSkinSpec::parse(&v2).expect("parse2");
        assert_eq!(spec2.choose(false, true, true).map(|l| l.image.as_str()),
                   Some("vc.png"));
        // 回退顺序仍优先同向：同向 Candidates 在场时不落第 5 级
        let v3: Value = serde_json::json!({
            "kind": "ssf", "cache_dir": "X:\\d", "fp": "f",
            "layouts": {
                "vertical_candidates": {"image": "vc.png"},
                "horizontal_candidates": {"image": "hc.png"}
            }
        });
        let spec3 = SsfSkinSpec::parse(&v3).expect("parse3");
        assert_eq!(spec3.choose(true, true, true).map(|l| l.image.as_str()),
                   Some("vc.png"));
    }

    #[test]
    fn decode_static_png_premul() {
        // 2×1 PNG：红不透明 + 半透明蓝 → 预乘 BGRA 校验
        let img = image::RgbaImage::from_fn(2, 1, |x, _| {
            if x == 0 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 0, 255, 128])
            }
        });
        let mut buf = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        let bmp = decode_bitmap(buf.get_ref()).expect("decode");
        assert_eq!((bmp.width, bmp.height), (2, 1));
        assert_eq!(bmp.frames.len(), 1);
        assert_eq!(bmp.delays_ms, vec![0]);
        assert!(!bmp.is_animated());
        let px = &bmp.frames[0];
        // 红（不透明）：BGRA = (0,0,255,255)
        assert_eq!(&px[0..4], &[0, 0, 255, 255]);
        // 蓝 50%：B'=255*128/127→(255*128+127)/255=128；B=128,G=0,R=0,A=128
        assert_eq!(&px[4..8], &[128, 0, 0, 128]);
    }

    #[test]
    fn name_normalization() {
        assert!(normalize_image_name("Panel.PNG").is_ok());
        assert_eq!(normalize_image_name("Panel.PNG").unwrap(), "panel.png");
        assert!(normalize_image_name("../evil.png").is_err());
        assert!(normalize_image_name("a/b.png").is_err());
        assert!(normalize_image_name("a\\b.png").is_err());
        assert!(normalize_image_name("").is_err());
        let long = "x".repeat(129);
        assert!(normalize_image_name(&long).is_err());
    }

    // ──【SSF 缩小高清化·Profile E 2026-10-18】（虎娘 skin_raster_filter_
    //    test.cpp 同款语义）──

    /// 纯色图锐化不动（detail=0，threshold 内零增益）。
    #[test]
    fn skin_raster_sharpen_uniform_unchanged() {
        let mut px = [64u8, 128, 192, 255].repeat(9 * 9);
        let before = px.clone();
        sharpen_skin_raster(&mut px, 9, 9);
        assert_eq!(px, before, "flat raster must pass through unchanged");
    }

    /// 中心亮斑锐化后对比度增大（反锐化掩模把 detail 加回）。
    #[test]
    fn skin_raster_sharpen_amplifies_detail() {
        let mut px = [100u8, 100, 100, 255].repeat(9 * 9);
        let c = (4 * 9 + 4) * 4; // 中心像素
        px[c] = 180;
        px[c + 1] = 180;
        px[c + 2] = 180;
        sharpen_skin_raster(&mut px, 9, 9);
        assert!(px[c] > 180, "bright center gets sharper (>{})", px[c]);
        // 邻域被反向压暗（detail 负 → 减）
        let n = (4 * 9 + 3) * 4;
        assert!(px[n] < 100, "neighbor dips below flat ({})", px[n]);
    }

    /// 小图（<3 宽/高）不处理；极端棋盘不越界不炸（钳制在预乘域内）。
    #[test]
    fn skin_raster_sharpen_tiny_and_checker_safe() {
        let mut tiny = vec![1u8, 2, 3, 4, 5, 6, 7, 8];
        let before = tiny.clone();
        sharpen_skin_raster(&mut tiny, 1, 2);
        assert_eq!(tiny, before, "tiny raster unchanged");
        let mut checker = Vec::with_capacity(8 * 8 * 4);
        for y in 0..8 {
            for x in 0..8 {
                let v = if (x + y) % 2 == 0 { 255u8 } else { 0 };
                checker.extend_from_slice(&[v, v, v, 255]);
            }
        }
        sharpen_skin_raster(&mut checker, 8, 8);
        for px in checker.chunks_exact(4) {
            assert!(px[0] <= px[3] && px[1] <= px[3] && px[2] <= px[3]);
        }
    }

    /// 半透明区（alpha<250）完全不碰——剪影/光晕保护。
    #[test]
    fn skin_raster_sharpen_skips_translucent() {
        let mut px = Vec::with_capacity(5 * 5 * 4);
        for y in 0..5 {
            for x in 0..5 {
                let v: u8 = if x == 2 && y == 2 { 200 } else { 50 };
                // 预乘：alpha=100，色=v*100/255
                let prem = (v as u16 * 100 / 255) as u8;
                px.extend_from_slice(&[prem, prem, prem, 100]);
            }
        }
        let before = px.clone();
        sharpen_skin_raster(&mut px, 5, 5);
        assert_eq!(px, before, "translucent regions untouched");
    }

    /// 预过滤：目标尺寸正确、预乘钳制（通道≤alpha）、输出不透明像素
    /// 有锐化对比（比纯缩放更「硬」的边缘）。
    #[test]
    fn skin_raster_prefilter_downscale() {
        // 16×16 棋盘 → 4×4
        let mut src = Vec::with_capacity(16 * 16 * 4);
        for y in 0..16 {
            for x in 0..16 {
                let v: u8 = if (x / 4 + y / 4) % 2 == 0 { 240 } else { 10 };
                src.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let (w, h, out) = prefilter_frame(&src, 16, 16, 4, 4).expect("prefilter");
        assert_eq!((w, h), (4, 4));
        assert_eq!(out.len(), 4 * 4 * 4);
        for px in out.chunks_exact(4) {
            assert!(px[3] > 0);
            assert!(px[0] <= px[3] && px[1] <= px[3] && px[2] <= px[3], "premultiplied clamp");
        }
        // 4×4 采样 4×4 块：等值映射后应保留高低两位的对比（非全平均糊）
        let vals: Vec<u8> = out.chunks_exact(4).map(|p| p[0]).collect();
        let max = *vals.iter().max().unwrap();
        let min = *vals.iter().min().unwrap();
        assert!(max - min > 100, "contrast retained: min={min} max={max}");
        // 坏输入 → None（调用方回退 1:1）
        assert!(prefilter_frame(&src[..10], 16, 16, 4, 4).is_none());
        assert!(prefilter_frame(&src, 16, 16, 0, 4).is_none());
    }

    /// LRU：命中重排、预算驱逐、插入后可取。
    #[test]
    fn skin_raster_cache_lru() {
        let mut cache = SsfRasterCache::new();
        cache.insert(0, 2, 2, vec![9u8; 16]);
        assert!(cache.touch(0, 2, 2));
        assert_eq!(cache.pixels_of(0, 2, 2).unwrap(), &[9u8; 16]);
        assert!(!cache.touch(0, 3, 3), "miss on size mismatch");
        assert!(!cache.touch(1, 2, 2), "miss on frame mismatch");
        // 8 条预算：插满后最老条被驱逐
        for i in 1..=8 {
            cache.insert(i, 2, 2, vec![i as u8; 16]);
        }
        assert!(!cache.touch(0, 2, 2), "oldest evicted");
        assert!(cache.touch(8, 2, 2), "newest present");
    }
}
