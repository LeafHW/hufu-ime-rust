//! 【字体族解析 + 私用字体装载 2026-10-08】设置页「候选字体」下拉
//! 数据源 + 各渲染端（server GDI 预览）私用字体装载。
//!
//! 背景：此前「字体」设置是自由文本框——用户把 Plangothic.ttc 放进
//! 数据\字体\ 后照网上查的族名「遍黑体P1」填进去并不生效：渲染端
//! （DLL DWrite / server GDI）从未装载过该文件夹里的字体文件，族名
//! 对不上就静默回退雅黑。本轮改造：字体文件夹成为一等公民——
//! server 扫描解析真名族名（TTC 多族全列）供下拉选择，并把文件以
//! AddFontResourceExW(FR_PRIVATE) 装进本进程（GDI 按族名即得）；
//! DLL 侧各自装载（candwin2 走 DWrite 私有字体集，addword 走 GDI）。
//!
//! 解析：最小 sfnt name 表读取（无需外部 crate）——ID16（版式族名，
//! 多字重族的规范名）优先，回退 ID1；Windows 平台（3）UTF-16BE 优先，
//! Mac（1）按 latin1 近似兜底。TTC（ttcf）逐子字体各取一族。

use std::path::{Path, PathBuf};

fn u16be(b: &[u8], i: usize) -> Option<u16> {
    Some(((*b.get(i)? as u16) << 8) | *b.get(i + 1)? as u16)
}

fn u32be(b: &[u8], i: usize) -> Option<u32> {
    Some((u16be(b, i)? as u32) << 16 | u16be(b, i + 2)? as u32)
}

/// name 表记录值：平台 3/0（UTF-16BE）；平台 1（Mac Roman 按 latin1）。
fn name_string(b: &[u8], base: usize, off: usize, len: usize, platform: u16) -> Option<String> {
    let s = b.get(base + off..base + off + len)?;
    if platform == 3 || platform == 0 {
        let units: Vec<u16> = s
            .chunks_exact(2)
            .map(|c| ((c[0] as u16) << 8) | c[1] as u16)
            .collect();
        String::from_utf16(&units).ok()
    } else {
        Some(s.iter().map(|&c| c as char).collect())
    }
}

/// 单个 sfnt 的最优族名：ID16+zh > ID16 > ID1+zh > ID1。
/// base = sfnt 起始字节偏移（TTC 子字体在文件内；表目录偏移一律
/// 相对文件头而非子字体头——Plangothic.ttc 实测：glyf 恰在
/// 子字体目录末尾、DSIG 末尾恰抵下一子字体偏移，按子字体头算则
/// 全部越位错读）。
fn sfnt_family(b: &[u8], base: usize) -> Option<String> {
    let nt = u16be(b, base + 4)? as usize;
    // 表记录 @base+12：tag(4) checksum(4) offset(4) length(4)——offset
    // 相对文件头
    let mut name_at = None;
    for i in 0..nt {
        let rec = base + 12 + i * 16;
        if b.get(rec..rec + 4) == Some(b"name") {
            name_at = u32be(b, rec + 8).map(|v| v as usize);
        }
    }
    let na = name_at?;
    let count = u16be(b, na + 2)? as usize;
    let sto = u16be(b, na + 4)? as usize + na;
    let mut best: Option<(u8, String)> = None;
    for i in 0..count {
        let rec = na + 6 + i * 12;
        let (Some(plat), Some(_enc), Some(lang), Some(nid), Some(len), Some(off)) = (
            u16be(b, rec),
            u16be(b, rec + 2),
            u16be(b, rec + 4),
            u16be(b, rec + 6),
            u16be(b, rec + 8),
            u16be(b, rec + 10),
        ) else {
            continue;
        };
        if nid != 16 && nid != 1 {
            continue;
        }
        if !(plat == 3 || plat == 0 || plat == 1) {
            continue;
        }
        // zh-CN=0x0804（简中字体最常见），en=0x0409，其余语言次之
        let rank = match (nid, lang) {
            (16, 0x0804) => 0,
            (16, 0x0409) => 1,
            (16, _) => 2,
            (1, 0x0804) => 3,
            (1, 0x0409) => 4,
            _ => 5,
        };
        if best.as_ref().is_some_and(|(r, _)| *r <= rank) {
            continue;
        }
        if let Some(s) = name_string(b, sto, off as usize, len as usize, plat) {
            if !s.is_empty() && s.len() < 128 {
                best = Some((rank, s));
            }
        }
    }
    best.map(|(_, s)| s)
}

/// 字体文件字节 → 族名列表（TTF/OTF 一族；TTC 每子字体一族，去重）。
pub fn families(bytes: &[u8]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if bytes.len() < 12 {
        return out;
    }
    if &bytes[..4] == b"ttcf" {
        if let Some(n) = u32be(bytes, 8) {
            for i in 0..n.min(64) as usize {
                if let Some(off) = u32be(bytes, 12 + i * 4).map(|v| v as usize) {
                    if bytes.get(off..off + 4).is_some() {
                        if let Some(f) = sfnt_family(bytes, off) {
                            if !out.contains(&f) {
                                out.push(f);
                            }
                        }
                    }
                }
            }
        }
    } else if let Some(f) = sfnt_family(bytes, 0) {
        out.push(f);
    }
    out
}

/// 字体目录扫描 → (文件名, 族名) 列表（按文件名+族名稳定排序）。
/// 坏文件/不可解析跳过；目录不存在返回空。
pub fn scan(dir: &Path) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut files: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            matches!(
                p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_lowercase())
                    .as_deref(),
                Some("ttf") | Some("ttc") | Some("otf")
            )
        })
        .collect();
    files.sort();
    for p in files {
        if let Ok(bytes) = std::fs::read(&p) {
            for fam in families(&bytes) {
                let file = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
                out.push((file, fam));
            }
        }
    }
    out
}

/// 字体目录两处候选（与 Engine::resolve_data_sub 同序：安装根一级
/// 优先，数据\ 内回退——两处都扫，合并去重）。
pub fn font_dirs(data_dir: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(root) = data_dir.parent() {
        let p = root.join("字体");
        if p.is_dir() {
            v.push(p);
        }
    }
    let p = data_dir.join("字体");
    if p.is_dir() {
        v.push(p);
    }
    v
}

// ── 私用字体装载（GDI）──

#[cfg(windows)]
#[link(name = "gdi32")]
extern "system" {
    fn AddFontResourceExW(file: *const u16, fl: u32, pdv: *const core::ffi::c_void) -> i32;
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn GetLastError() -> u32;
}

// ── GDI 族名枚举探针（找 AddFontResourceExW 装入后 GDI 眼中的真名）──

#[cfg(windows)]
#[repr(C)]
#[allow(non_snake_case)]
struct LOGFONTW {
    lfHeight: i32,
    lfWidth: i32,
    lfEscapement: i32,
    lfOrientation: i32,
    lfWeight: i32,
    lfItalic: u8,
    lfUnderline: u8,
    lfStrikeOut: u8,
    lfCharSet: u8,
    lfOutPrecision: u8,
    lfClipPrecision: u8,
    lfQuality: u8,
    lfPitchAndFamily: u8,
    lfFaceName: [u16; 32],
}

#[cfg(windows)]
#[repr(C)]
#[allow(non_snake_case)]
struct ENUMLOGFONTEXW {
    elfLogFont: LOGFONTW,
    elfFullName: [u16; 64],
    elfStyle: [u16; 32],
    elfScript: [u16; 32],
}

#[cfg(windows)]
type ENUMRESULETPROC = Option<
    unsafe extern "system" fn(
        lpelfe: *const ENUMLOGFONTEXW,
        lpntme: *const core::ffi::c_void,
        fonttype: u32,
        lparam: isize,
    ) -> i32,
>;

#[cfg(windows)]
#[link(name = "gdi32")]
extern "system" {
    fn EnumFontFamiliesExW(
        hdc: *mut core::ffi::c_void,
        lplogfont: *const LOGFONTW,
        lpproc: ENUMRESULETPROC,
        lparam: isize,
        dwflags: u32,
    ) -> i32;
}

#[cfg(windows)]
unsafe extern "system" fn enum_font_cb(
    lpelfe: *const ENUMLOGFONTEXW,
    _ntme: *const core::ffi::c_void,
    _t: u32,
    lparam: isize,
) -> i32 {
    let out = lparam as *mut Vec<String>;
    let name = {
        let units: Vec<u16> = (*lpelfe)
            .elfLogFont
            .lfFaceName
            .iter()
            .take_while(|&&u| u != 0)
            .copied()
            .collect();
        String::from_utf16_lossy(&units)
    };
    (*out).push(name);
    1 // 继续枚举
}

/// 枚举本进程 GDI 字体表全部族名（装私字体后调用——看 GDI 真名）。
#[cfg(windows)]
pub fn gdi_list_families() -> Vec<String> {
    unsafe {
        let dc = CreateCompatibleDC(std::ptr::null());
        if dc.is_null() {
            return Vec::new();
        }
        let mut lf: LOGFONTW = std::mem::zeroed();
        lf.lfCharSet = 1; // DEFAULT_CHARSET=全表
        let mut out: Vec<String> = Vec::new();
        let lp = &mut out as *mut Vec<String> as isize;
        let _ = EnumFontFamiliesExW(dc, &lf, Some(enum_font_cb), lp, 0);
        let _ = DeleteDC(dc);
        out.sort();
        out.dedup();
        out
    }
}

/// 枚举**支持简体中文（GB2312=134）**的族名——设置页字体下拉的
/// 系统字体段（楷体/仿宋/黑体/雅黑/已安装的霞鹜…）。charset 过滤
/// 天然排除纯拉丁字体与未声明 936 的文件夹字体（不会与文件夹段
/// 重复；竖排 @ 开头的族名也一并滤除）。
#[cfg(windows)]
pub fn gdi_list_families_gb() -> Vec<String> {
    unsafe {
        let dc = CreateCompatibleDC(std::ptr::null());
        if dc.is_null() {
            return Vec::new();
        }
        let mut lf: LOGFONTW = std::mem::zeroed();
        lf.lfCharSet = 134; // GB2312
        let mut out: Vec<String> = Vec::new();
        let lp = &mut out as *mut Vec<String> as isize;
        let _ = EnumFontFamiliesExW(dc, &lf, Some(enum_font_cb), lp, 0);
        let _ = DeleteDC(dc);
        out.sort();
        out.dedup();
        out.into_iter()
            .filter(|n| !n.starts_with('@') && !n.is_empty())
            .collect()
    }
}

/// 进程内一次性装载（FR_PRIVATE=0x10：进程私有，进程退出自动释放，
/// 不污染系统字体表）。失败静默跳过（族名解析/渲染自会回退）。
static GDI_FONTS_ONCE: std::sync::Once = std::sync::Once::new();

pub fn ensure_private_gdi_fonts(dirs: &[PathBuf]) {
    GDI_FONTS_ONCE.call_once(|| {
        for d in dirs {
            let Ok(rd) = std::fs::read_dir(d) else {
                continue;
            };
            let mut files: Vec<PathBuf> = rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    matches!(
                        p.extension()
                            .and_then(|e| e.to_str())
                            .map(|e| e.to_ascii_lowercase())
                            .as_deref(),
                        Some("ttf") | Some("ttc") | Some("otf")
                    )
                })
                .collect();
            files.sort();
            for p in files {
                #[cfg(windows)]
                unsafe {
                    use std::os::windows::ffi::OsStrExt;
                    let w: Vec<u16> =
                        p.as_os_str().encode_wide().chain([0]).collect();
                    // 32MB 级 TTC（如 Plangothic）装载耗秒级但只此一次
                    let n = AddFontResourceExW(w.as_ptr(), 0x10, std::ptr::null());
                    if n == 0 || std::env::var("HUFU_FONT_DEBUG").is_ok() {
                        eprintln!(
                            "privgdi: {:?} AddFontResourceExW={} err={}",
                            p,
                            n,
                            GetLastError()
                        );
                    }
                }
                #[cfg(not(windows))]
                let _ = p;
            }
        }
    });
}

// ── 字体预览渲染（GDI，设置页字体下拉即时预览图 2026-10-09）──
//
// 「字体有什么区别」眼见为实：选中即渲一张该族名的样张（BMP）。
// 前提 ensure_private_gdi_fonts 已装载（server 启动即做），文件夹字体
// 按族名直出；系统字体（雅黑/霞鹜…）族名本来就在 GDI 表里，同路渲染。

#[repr(C)]
#[allow(non_snake_case)]
struct BITMAPINFOHEADER {
    biSize: u32,
    biWidth: i32,
    biHeight: i32,
    biPlanes: u16,
    biBitCount: u16,
    biCompression: u32,
    biSizeImage: u32,
    biXPelsPerMeter: i32,
    biYPelsPerMeter: i32,
    biClrUsed: u32,
    biClrImportant: u32,
}

#[repr(C)]
#[allow(non_snake_case)]
struct BITMAPINFO {
    bmiHeader: BITMAPINFOHEADER,
    bmiColors: [u32; 1],
}

#[repr(C)]
#[allow(non_snake_case)]
struct RECT {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[cfg(windows)]
#[link(name = "gdi32")]
extern "system" {
    fn CreateCompatibleDC(hdc: *const core::ffi::c_void) -> *mut core::ffi::c_void;
    fn DeleteDC(hdc: *mut core::ffi::c_void) -> i32;
    fn CreateFontW(
        h: i32,
        w: i32,
        esc: i32,
        ori: i32,
        weight: i32,
        italic: u32,
        underline: u32,
        strikeout: u32,
        charset: u32,
        outprec: u32,
        clipprec: u32,
        quality: u32,
        pitchfamily: u32,
        face: *const u16,
    ) -> *mut core::ffi::c_void;
    fn SelectObject(hdc: *mut core::ffi::c_void, h: *mut core::ffi::c_void)
        -> *mut core::ffi::c_void;
    fn DeleteObject(h: *mut core::ffi::c_void) -> i32;
    fn SetBkMode(hdc: *mut core::ffi::c_void, mode: i32) -> i32;
    fn SetTextColor(hdc: *mut core::ffi::c_void, color: u32) -> u32;
    fn CreateDIBSection(
        hdc: *mut core::ffi::c_void,
        bmi: *const BITMAPINFO,
        usage: u32,
        bits: *mut *mut core::ffi::c_void,
        hsection: *const core::ffi::c_void,
        offset: u32,
    ) -> *mut core::ffi::c_void;
    fn GdiFlush() -> i32;
}

#[cfg(windows)]
#[link(name = "gdi32")]
extern "system" {
    fn GetTextFaceW(
        hdc: *mut core::ffi::c_void,
        count: i32,
        face: *mut u16,
    ) -> i32;
}

#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn DrawTextW(
        hdc: *mut core::ffi::c_void,
        s: *const u16,
        count: i32,
        rect: *mut RECT,
        flags: u32,
    ) -> i32;
}

/// 渲染样张 BMP 字节流（440x64，白底深灰字）。None=非 Windows。
/// 字体族名不匹配时 GDI 静默回退（系统默认体）——与真实候选窗回退
/// 行为一致，预览所见即渲染所得。
#[allow(non_snake_case)]
pub fn render_font_preview_bmp(family: &str, sample: &str) -> Option<Vec<u8>> {
    #[cfg(not(windows))]
    {
        let _ = (family, sample);
        None
    }
    #[cfg(windows)]
    unsafe {
        const W: i32 = 440;
        const H: i32 = 64;
        let stride = ((W as usize * 3 + 3) & !3) as usize;
        let img = stride * H as usize;

        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: W,
                biHeight: H, // 正高=bottom-up，BMP 文件序直接可用
                biPlanes: 1,
                biBitCount: 24,
                biCompression: 0, // BI_RGB
                biSizeImage: img as u32,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [0],
        };

        let dc = CreateCompatibleDC(std::ptr::null());
        if dc.is_null() {
            return None;
        }
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(
            dc,
            &bmi,
            0, // DIB_RGB_COLORS
            &mut bits,
            std::ptr::null(),
            0,
        );
        if dib.is_null() {
            let _ = DeleteDC(dc);
            return None;
        }
        let old_bmp = SelectObject(dc, dib);
        // 白底（24bpp BGR，直接填）
        std::ptr::write_bytes(bits as *mut u8, 0xFF, img);

        let face: Vec<u16> = std::os::windows::ffi::OsStrExt::encode_wide(std::ffi::OsStr::new(
            family,
        ))
        .chain([0])
        .collect();
        // DEFAULT_CHARSET=1：只按族名匹配。曾用 GB2312_CHARSET=134——
        // 文件夹字体（Plangothic/文津宋体）主打扩展平面，未声明 936 代码
        // 页，GDI 映射器因字符集不符把它们甩给宋体（GetTextFaceW 实测
        // 映射「宋体」），样张全成同一个回退体。
        let font = CreateFontW(
            36,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            1, // DEFAULT_CHARSET
            0,
            0,
            0,
            0,
            face.as_ptr(),
        );
        if font.is_null() {
            let _ = SelectObject(dc, old_bmp);
            let _ = DeleteObject(dib);
            let _ = DeleteDC(dc);
            return None;
        }
        let old_font = SelectObject(dc, font);
        // 【GDI 实选族名探针】CreateFontW 对不存在的族名不报错而是静默
        // 映射默认体——GetTextFaceW 读实际选中的族名（装载失败/名字不
        // 对时这里会现原形）。
        let mut face_buf = [0u16; 64];
        let n = GetTextFaceW(dc, 64, face_buf.as_mut_ptr());
        let actual_face = String::from_utf16_lossy(&face_buf[..n.max(0) as usize]);
        if actual_face != family {
            eprintln!("fontpreview: 请求「{family}」实际 GDI 映射到「{actual_face}」");
        }
        let _ = SetBkMode(dc, 1 /* TRANSPARENT */);
        let _ = SetTextColor(dc, 0x0030_3030); // BGR 深灰
        let txt: Vec<u16> = sample.encode_utf16().collect();
        let mut rc = RECT {
            left: 10,
            top: 6,
            right: W - 10,
            bottom: H - 6,
        };
        // DT_SINGLELINE|DT_VCENTER|DT_LEFT = 0x25
        let _ = DrawTextW(dc, txt.as_ptr(), txt.len() as i32, &mut rc as *mut RECT, 0x25);
        let _ = GdiFlush();

        // 【像素先拷再释放】DIB 内存归 DeleteObject(dib) 管——必须在
        // 释放前拷出（曾犯 Use-After-Free：先 Delete 后读，测试崩
        // 0xC0000005）。
        let px = std::slice::from_raw_parts(bits as *const u8, img).to_vec();

        // 还原并释放（DIB 内存归我们，DeleteObject(dib) 释放之）
        let _ = SelectObject(dc, old_font);
        let _ = SelectObject(dc, old_bmp);
        let _ = DeleteObject(font);
        let _ = DeleteObject(dib);
        let _ = DeleteDC(dc);

        // BMP 文件 = 14B FILEHEADER + 40B INFOHEADER + 像素
        let file_size = 14 + 40 + img;
        let mut out = Vec::with_capacity(file_size);
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&(file_size as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&54u32.to_le_bytes());
        out.extend_from_slice(&bmi.bmiHeader.biSize.to_le_bytes());
        out.extend_from_slice(&W.to_le_bytes());
        out.extend_from_slice(&H.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&24u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(img as u32).to_le_bytes());
        out.extend_from_slice(&0i32.to_le_bytes());
        out.extend_from_slice(&0i32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&px);
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 【样张自验 2026-10-09】三个族名各渲染一张 BMP 落 tmp-test，
    /// 供像素比对（宋体/黑体字形必须两两不同；失败即 GDI 静默回退）。
    #[test]
    #[cfg(windows)]
    fn preview_bmp_renders() {
        // 直接用已部署字体文件夹（与生产同源）；不存在则只验系统字体。
        let mut deployed: Vec<std::path::PathBuf> = [
            r"D:\HUFU\HuFu虎符输入法-v1.7.3-无模型\字体",
            r"D:\HUFU\HuFu虎符输入法-v1.7.3-无模型\数据\字体",
        ]
        .iter()
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_dir())
        .collect();
        if deployed.is_empty() {
            deployed.push(std::path::PathBuf::from("."));
        }
        // ⚠ Once 单次语义：必须一次传全部目录（曾逐目录调用——首次空
        // 目录吞掉 Once，真目录被跳过，GDI 没装上字体。生产 main.rs
        // 本就全目录一次调，无此问题）。
        ensure_private_gdi_fonts(&deployed);
        let out = std::path::PathBuf::from(r"E:\DSH-KF\tmp-test");
        let _ = std::fs::create_dir_all(&out);
        for (tag, fam) in [
            ("yahei", "Microsoft YaHei"),
            ("pianhei", "遍黑体P1"),
            ("mincho", "文津宋体 第3平面"),
        ] {
            eprintln!("fontpreview-test: {tag} 请求族名「{fam}」");
            match render_font_preview_bmp(fam, "永国专虎符 Aa01") {
                Some(b) => {
                    assert!(b.starts_with(b"BM"), "{fam} 非 BMP");
                    assert!(b.len() > 10_000, "{fam} 样张过小 {}", b.len());
                    std::fs::write(out.join(format!("prev-{tag}.bmp")), &b).unwrap();
                }
                None => panic!("{fam} 渲染失败"),
            }
        }
    }

    /// 内嵌最小 sfnt（name 表 ID16 zh-CN）验证解析路径。
    #[test]
    fn parse_minimal_sfnt() {
        // name 表：format=0 count=1 stringOffset=18（=6 头+1 条记录 12）
        let mut name = Vec::new();
        name.extend_from_slice(&0u16.to_be_bytes());
        name.extend_from_slice(&1u16.to_be_bytes());
        name.extend_from_slice(&18u16.to_be_bytes());
        // 记录：plat=3 enc=1 lang=0x804 id=16 len=4 off=0
        //（「测试」2 个 UTF-16 码元 = 4 字节）
        name.extend_from_slice(&3u16.to_be_bytes());
        name.extend_from_slice(&1u16.to_be_bytes());
        name.extend_from_slice(&0x0804u16.to_be_bytes());
        name.extend_from_slice(&16u16.to_be_bytes());
        name.extend_from_slice(&4u16.to_be_bytes());
        name.extend_from_slice(&0u16.to_be_bytes());
        // 「测试」UTF-16BE
        let s = "测试".encode_utf16().collect::<Vec<_>>();
        for u in s {
            name.extend_from_slice(&u.to_be_bytes());
        }
        // header: sfntVersion(4) numTables(2) searchRange..(6)
        let mut b = vec![0u8; 12];
        b[4..6].copy_from_slice(&1u16.to_be_bytes());
        // name 表记录（16 字节）：tag(4) checksum(4) offset(4) length(4)——
        // name 表紧随其后 @28
        let mut rec = vec![0u8; 16];
        rec[0..4].copy_from_slice(b"name");
        rec[8..12].copy_from_slice(&28u32.to_be_bytes());
        rec[12..16].copy_from_slice(&(name.len() as u32).to_be_bytes());
        b.extend_from_slice(&rec);
        b.extend_from_slice(&name);
        assert_eq!(families(&b), vec!["测试".to_string()]);
    }

    /// 真机文件冒烟（Plangothic.ttc 存在才跑）：cargo test -p hufu-server
    /// fontinfo -- --ignored --nocapture
    #[test]
    #[ignore]
    fn plangothic_real() {
        let d = PathBuf::from(r"E:\DSH-KF\tmp-test\fontdata\字体");
        let v = scan(&d);
        eprintln!("scan = {v:?}");
        assert!(!v.is_empty(), "Plangothic 未解析出族名");
    }
}
