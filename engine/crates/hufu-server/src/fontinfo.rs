//! 【字体族枚举 + 样张预览 2026-10-08】设置页「候选字体」下拉的系统
//! 字体段（GDI GB2312 族名枚举）与选中即时预览样张（GDI 渲 BMP）。
//!
//! 【死代码清扫 2026-10-26 审计】原「私用字体装载 + sfnt 族名解析」链
//!（ensure_private_gdi_fonts / scan / families / sfnt_family / font_dirs /
//! gdi_list_families / AddFontResourceExW 装载）在生产二进制零调用——
//! 文件夹字体的装载与族名解析已在 DLL 侧各自实现（candwin2 走 DWrite
//! 私有字体集、addword 走 GDI），server 侧只剩「枚举 + 预览」两职。
//! 整链连同其诊断测试一并删除（编译器 never-used 警告为证）。

// ── GDI 族名枚举（设置页字体下拉的系统字体段）──

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

/// 枚举**支持简体中文（GB2312=134）**的族名——设置页字体下拉的
/// 系统字体段（楷体/仿宋/黑体/雅黑/已安装的霞鹜…）。charset 过滤
/// 天然排除纯拉丁字体与未声明 936 的文件夹字体（不会与文件夹段
/// 重复；竖排 @ 开头的族名也一并滤除）。
#[cfg(windows)]
pub fn gdi_list_families_gb() -> Vec<String> {
    unsafe {
        let dc = CreateCompatibleDC(0);
        if dc == 0 {
            return Vec::new();
        }
        let mut lf: LOGFONTW = std::mem::zeroed();
        lf.lfCharSet = 134; // GB2312
        let mut out: Vec<String> = Vec::new();
        let lp = &mut out as *mut Vec<String> as isize;
        let _ = EnumFontFamiliesExW(dc as *mut _, &lf, Some(enum_font_cb), lp, 0);
        let _ = DeleteDC(dc);
        out.sort();
        out.dedup();
        out.into_iter()
            .filter(|n| !n.starts_with('@') && !n.is_empty())
            .collect()
    }
}

// ── 字体预览渲染（GDI，设置页字体下拉即时预览图 2026-10-09）──
//
// 「字体有什么区别」眼见为实：选中即渲一张该族名的样张（BMP）。
// 族名不匹配时 GDI 静默回退（系统默认体）——与真实候选窗回退行为
// 一致，预览所见即渲染所得（GetTextFaceW 探针会把回退现形打日志）。

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
// 【lint 2026-10-26 审计】签名与 candwin/tray 的既有声明对齐（isize
// 句柄风格）——rustc 1.98 跨模块同名 extern 签名不一致会告警。
extern "system" {
    fn CreateCompatibleDC(hdc: isize) -> isize;
    fn DeleteDC(hdc: isize) -> i32;
    fn CreateFontW(
        h: i32,
        w: i32,
        esc: i32,
        orient: i32,
        weight: i32,
        italic: u32,
        underline: u32,
        strikeout: u32,
        charset: u32,
        outprec: u32,
        clipprec: u32,
        quality: u32,
        pitch: u32,
        face: *const u16,
    ) -> isize;
    fn SelectObject(hdc: isize, o: isize) -> isize;
    fn DeleteObject(o: isize) -> i32;
    fn SetBkMode(hdc: isize, mode: i32) -> i32;
    fn SetTextColor(hdc: isize, color: u32) -> u32;
    fn CreateDIBSection(
        hdc: isize,
        bmi: *const BITMAPINFO,
        usage: u32,
        bits: *mut *mut core::ffi::c_void,
        section: isize,
        offset: u32,
    ) -> isize;
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

        let dc = CreateCompatibleDC(0);
        if dc == 0 {
            return None;
        }
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(dc, &bmi, 0, &mut bits, 0, 0);
        if dib == 0 {
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
        if font == 0 {
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
        let n = GetTextFaceW(dc as *mut _, 64, face_buf.as_mut_ptr());
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
        let _ = DrawTextW(
            dc as *mut _,
            txt.as_ptr(),
            txt.len() as i32,
            &mut rc as *mut RECT,
            0x25,
        );
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
    /// 【样张自验 2026-10-09】两个系统族名各渲染一张 BMP，供像素比对
    ///（宋体/黑体字形必须两两不同；失败即 GDI 静默回退）。
    /// 【死代码清扫 2026-10-26】私字体装载链删除后不再装文件夹字体，
    /// 只验系统字体段。
    #[test]
    #[cfg(windows)]
    fn preview_bmp_renders() {
        let out = std::path::PathBuf::from(r"E:\DSH-KF\tmp-test");
        let _ = std::fs::create_dir_all(&out);
        for (tag, fam) in [("yahei", "Microsoft YaHei"), ("simhei", "SimHei")] {
            eprintln!("fontpreview-test: {tag} 请求族名「{fam}」");
            match super::render_font_preview_bmp(fam, "永国专虎符 Aa01") {
                Some(b) => {
                    assert!(b.starts_with(b"BM"), "{fam} 非 BMP");
                    assert!(b.len() > 10_000, "{fam} 样张过小 {}", b.len());
                    std::fs::write(out.join(format!("prev-{tag}.bmp")), &b).unwrap();
                }
                None => panic!("{fam} 渲染失败"),
            }
        }
    }
}
